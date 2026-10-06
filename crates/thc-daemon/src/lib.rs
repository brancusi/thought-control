//! `thc daemon run`: watches the vault, replays other devices' events, pushes changes to
//! clients over a Unix socket, fires alerts and keeps the Markdown export fresh
//! (SPEC §6.4, docs/design/daemon.md). The daemon is optional: everything it does, the CLI and
//! TUI can do on demand. It is a cache warmer, an event bus and an alarm clock.

mod notifier;
mod server;
pub use server::today_panel;

use anyhow::{Context, Result, bail};
use notify::{RecursiveMode, Watcher};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use thc_core::alerts::{self, Delivery};
use thc_core::event::Actor;
use thc_core::proto::{self, DaemonInfo, EventMsg, PROTO_VERSION, VERSION};
use thc_core::vault::{Paths, Vault};

pub use notifier::{Channel, LOGGED};

/// How long a delivering client (ThoughtBar) has to claim an alert before the OS fallback.
pub const DELIVERY_GRACE: Duration = Duration::from_secs(2);
const DEBOUNCE: Duration = Duration::from_millis(100);
const RESCAN_EVERY: Duration = Duration::from_secs(300);
const EXPORT_AFTER: Duration = Duration::from_secs(2);

pub struct RunOpts {
    pub json: bool,
    /// Override the OS notification channel (tests use `Channel::Log`).
    pub channel: Option<Channel>,
    /// Set by the host for a vault other than home: its name leads its notifications.
    pub label: Option<String>,
    /// Also serve every other registered vault (the CLI's `thc daemon run`; never in-process
    /// tests, which must not reach the person's real vaults).
    pub host: bool,
}

pub struct ClientConn {
    pub id: u64,
    pub kind: String,
    pub topics: HashSet<String>,
    pub deliver: bool,
    pub writer: std::os::unix::net::UnixStream,
}

/// An alert fired here that a client may still claim.
struct PendingDelivery {
    delivery: Delivery,
    alerts: Vec<String>,
    deadline: Instant,
}

pub struct Shared {
    /// The vault's name in its notifications when it isn't home (`acme · Book the venue`).
    pub vault_label: Option<String>,
    pub vault: Mutex<Vault>,
    pub clients: Mutex<Vec<ClientConn>>,
    pub delivered: Mutex<HashSet<String>>,
    pub shutdown: AtomicBool,
    pub next_client: AtomicU64,
    pub started: Instant,
    pub started_ms: i64,
    pub socket: PathBuf,
    pub channel: Channel,
    pub json_log: bool,
    pub color: bool,
    /// Set by writes made through the socket so the main loop pushes them promptly.
    pub dirty: AtomicBool,
    /// The file watcher is up: changes are pushed as they happen (until then, on the next pass).
    pub watching: AtomicBool,
    pub last_change: Mutex<Option<(String, String)>>,
}

impl Shared {
    /// One log line: `10:58:02  replay   studio-mini  3 events · 1 tx by claude`.
    pub fn log(&self, kind: &str, detail: &str) {
        let now = chrono::Local::now();
        let mut out = std::io::stdout().lock();
        if self.json_log {
            let _ = writeln!(out, "{}", json!({ "at": now.format("%Y-%m-%dT%H:%M:%S").to_string(), "kind": kind, "detail": detail }));
        } else if self.color {
            let _ = writeln!(out, "\x1b[2m{}\x1b[0m  {kind:<8} {detail}", now.format("%H:%M:%S"));
        } else {
            let _ = writeln!(out, "{}  {kind:<8} {detail}", now.format("%H:%M:%S"));
        }
        let _ = out.flush();
    }

    /// Push an event to every client subscribed to `topic`. Dead clients are dropped.
    pub fn broadcast(&self, topic: &str, event: &str, data: Value) {
        let line = match serde_json::to_string(&EventMsg { event: event.into(), data }) {
            Ok(l) => l,
            Err(_) => return,
        };
        let mut clients = self.clients.lock().unwrap();
        clients.retain_mut(|c| {
            if !c.topics.contains(topic) && !c.topics.contains("*") {
                return true;
            }
            c.writer.write_all(line.as_bytes()).and_then(|_| c.writer.write_all(b"\n")).is_ok()
        });
    }

    /// Push an event to every client, whatever it subscribed to.
    pub fn broadcast_all(&self, event: &str, data: Value) {
        let Ok(line) = serde_json::to_string(&EventMsg { event: event.into(), data }) else { return };
        let mut clients = self.clients.lock().unwrap();
        for c in clients.iter_mut() {
            let _ = c.writer.write_all(line.as_bytes()).and_then(|_| c.writer.write_all(b"\n"));
        }
    }

    fn has_deliverer(&self) -> bool {
        self.clients.lock().unwrap().iter().any(|c| c.deliver)
    }
}

/// Run in the foreground until `shutdown` (RPC or signal): this vault, and every other vault in
/// the registry on threads of this one process (vaults-architecture.md V6). Each vault keeps its
/// own socket, cache, watcher and alerts, so clients find it as before; reminders fire from
/// every vault, not only home. The registry is watched (read every half second, compared): a
/// vault added starts at once, one removed stops, one renamed restarts under its new name.
pub fn run(paths: Paths, opts: RunOpts) -> Result<()> {
    if !opts.host {
        return serve(paths, opts, true);
    }
    HOST_STOP.store(false, Ordering::SeqCst);
    let primary = paths.vault.clone();
    let channel = opts.channel.clone();
    let json = opts.json;
    let host = std::thread::spawn(move || host_others(primary, channel, json));
    let r = serve(paths, opts, true);
    HOST_STOP.store(true, Ordering::SeqCst);
    let _ = host.join();
    r
}

static HOST_STOP: AtomicBool = AtomicBool::new(false);

/// The other registered vaults' services: started, and stopped when they leave the registry or
/// the primary stops.
fn host_others(primary: PathBuf, channel: Option<Channel>, json: bool) {
    use std::collections::HashMap;
    let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let primary = canon(&primary);
    let mut running: HashMap<PathBuf, (std::thread::JoinHandle<()>, Arc<AtomicBool>, String)> = HashMap::new();
    let mut next_check = Instant::now();
    let mut seen: Option<String> = None;
    let file = thc_core::vault::global_config_path();
    while !HOST_STOP.load(Ordering::SeqCst) && !SIGNALLED.load(Ordering::SeqCst) {
        // The file's text, compared: a `thc vault new` elsewhere is served within half a second.
        // The 30 s pass stays, for a vault whose marker appears later.
        let text = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_default();
        let changed = seen.as_deref() != Some(text.as_str());
        if changed || Instant::now() >= next_check {
            seen = Some(text);
            next_check = Instant::now() + Duration::from_secs(30);
            let reg = thc_core::registry::Registry::load();
            // (Compared canonical, served as the registry spells it: the cache and socket are
            // keyed by that spelling, which is how clients find them.)
            let want: Vec<(PathBuf, String)> = reg
                .vaults
                .iter()
                .filter(|e| e.path.join(thc_core::vault::VAULT_MARKER).exists() && canon(&e.path) != primary)
                .map(|e| (e.path.clone(), e.name.clone()))
                .collect();
            // Gone from the registry: stop.
            // (A service that ended on its own, say because that vault already has its own daemon,
            // isn't retried until the registry changes.)
            // Renamed (same path, new name): restart, so its label is the new name.
            let named: HashMap<PathBuf, String> = want.iter().cloned().collect();
            running.retain(|p, (_, stop, name)| {
                if named.get(p) == Some(name) {
                    return true;
                }
                stop.store(true, Ordering::SeqCst);
                false
            });
            for (path, name) in want {
                if running.contains_key(&path) {
                    continue;
                }
                let full = name.clone();
                let stop = Arc::new(AtomicBool::new(false));
                let st = stop.clone();
                let label = (reg.home.as_deref() != Some(name.as_str())).then_some(name);
                let paths = Paths { vault: path.clone(), cache: thc_core::vault::default_cache(&path) };
                let channel = channel.clone();
                let h = std::thread::spawn(move || {
                    let opts = RunOpts { json, channel, label, host: false };
                    let _ = serve_until(paths, opts, false, Some(st));
                });
                running.insert(path, (h, stop, full));
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    for (_, (h, stop, _)) in running {
        stop.store(true, Ordering::SeqCst);
        let _ = h.join();
    }
}

fn serve(paths: Paths, opts: RunOpts, primary: bool) -> Result<()> {
    serve_until(paths, opts, primary, None)
}

/// One vault's service. `primary`: the one launchd started (signals, update checks); the others
/// stop when `stop` is set.
fn serve_until(paths: Paths, opts: RunOpts, primary: bool, stop: Option<Arc<AtomicBool>>) -> Result<()> {
    let socket = proto::socket_path(&paths);
    // Already running? Anything accepting on the socket is a daemon for this vault (maybe a slow
    // one): never remove a socket that answers. Only a refused connection means a stale file.
    if socket.exists() {
        if let Ok(mut c) = proto::Client::connect_path(&socket, Duration::from_millis(2000)) {
            let pid = c.call("status", Value::Null).ok().and_then(|st| st.get("pid").and_then(|p| p.as_u64()));
            // One daemon per vault: a second launcher (a login item and the app, say) finds this
            // one and leaves quietly. Exit 0, so launchd's KeepAlive doesn't respawn it.
            eprintln!(
                "thc daemon: already running for this vault{} · nothing to do",
                pid.map(|p| format!(" · pid {p}")).unwrap_or_default()
            );
            return Ok(());
        }
        // Stale socket: remove without saying so.
        let _ = std::fs::remove_file(&socket);
    }
    if let Some(dir) = socket.parent() {
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    }
    let listener = UnixListener::bind(&socket).with_context(|| format!("binding {}", socket.display()))?;
    std::fs::set_permissions(&socket, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;

    let vault = Vault::open(paths.clone(), Actor { kind: "human".into(), name: None }, "bar")?;
    let started_ms = chrono::Utc::now().timestamp_millis();
    let info = DaemonInfo { pid: std::process::id(), socket: socket.clone(), version: VERSION.into(), proto: PROTO_VERSION, started_ms, vault: paths.vault.clone() };
    std::fs::write(proto::info_path(&paths), serde_json::to_string_pretty(&info)?)?;

    let shared = Arc::new(Shared {
        vault_label: opts.label.clone(),
        vault: Mutex::new(vault),
        clients: Mutex::new(vec![]),
        delivered: Mutex::new(HashSet::new()),
        shutdown: AtomicBool::new(false),
        next_client: AtomicU64::new(1),
        started: Instant::now(),
        started_ms,
        socket: socket.clone(),
        channel: opts.channel.unwrap_or_else(notifier::detect),
        json_log: opts.json,
        color: !opts.json && std::io::stdout().is_terminal(),
        dirty: AtomicBool::new(false),
        watching: AtomicBool::new(false),
        last_change: Mutex::new(None),
    });
    if primary {
        install_signal_handlers();
    }
    if let Some(st) = stop {
        // The host's stop reaches this vault's loop through its own shutdown flag.
        let sh = shared.clone();
        std::thread::spawn(move || {
            while !st.load(Ordering::SeqCst) && !sh.shutdown.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(250));
            }
            sh.shutdown.store(true, Ordering::SeqCst);
        });
    }

    let device = shared.vault.lock().unwrap().device.clone();
    shared.log("start", &format!("thc {VERSION} · pid {} · device {device} · watching {}", std::process::id(), paths.vault.display()));
    // Today's attachment lines get their nodes, once per vault (FORMAT.md "Attachments").
    match thc_core::attach::upgrade(&mut shared.vault.lock().unwrap(), false) {
        Ok(0) => {}
        Ok(n) => shared.log("attachments", &format!("{n} note{} given attachment nodes · thc undo reverts it", if n == 1 { "" } else { "s" })),
        Err(e) => shared.log("error", &format!("attachments: {e:#}")),
    }

    // Socket server first: clients are answered at once, even when the file watcher is slow to
    // start (FSEvents once took 24 s: every client waited that long).
    {
        let sh = shared.clone();
        std::thread::spawn(move || server::accept_loop(sh, listener));
    }
    // File watcher, on its own thread (it lives there until shutdown). Once it's watching, a
    // catch-up takes whatever was written while it started, so nothing slips between them.
    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    for dir in ["log", "drop"] {
        std::fs::create_dir_all(paths.vault.join(dir))?;
    }
    {
        let sh = shared.clone();
        let vault = paths.vault.clone();
        std::thread::spawn(move || {
            let mut watcher = match notify::recommended_watcher(move |res| {
                let _ = tx.send(res);
            }) {
                Ok(w) => w,
                Err(e) => return sh.log("error", &format!("file watcher: {e}")),
            };
            for dir in ["log", "drop"] {
                if let Err(e) = watcher.watch(&vault.join(dir), RecursiveMode::Recursive) {
                    sh.log("error", &format!("watching {dir}: {e}"));
                }
            }
            sh.dirty.store(true, Ordering::SeqCst);
            sh.watching.store(true, Ordering::SeqCst);
            while !sh.shutdown.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(250));
            }
            drop(watcher);
        });
    }
    // Release checks: a minute after start, then every 6 h, by running this binary's
    // `thc update --check` (or `thc update` with update = "auto"). Never in CI or when off.
    if primary && thc_core::release::checks_allowed() {
        let sh = shared.clone();
        std::thread::spawn(move || update_checks(sh));
    }
    let result = main_loop(&shared, &paths, rx);
    // Tell every client before the socket goes, so panels go offline at once.
    let reason = match &result {
        Err(_) => "error",
        Ok(()) if SIGNALLED.load(Ordering::SeqCst) => "signal",
        Ok(()) => "requested",
    };
    shared.broadcast_all("shutdown", json!({ "reason": reason }));
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_file(proto::info_path(&paths));
    shared.log("stop", "socket removed");
    result
}

fn update_checks(shared: Arc<Shared>) {
    std::thread::sleep(std::time::Duration::from_secs(60));
    loop {
        if thc_core::release::checks_allowed() {
            let auto = thc_core::release::mode() == "auto";
            if let Ok(exe) = std::env::current_exe() {
                let args: &[&str] = if auto { &["--json", "update"] } else { &["--json", "update", "--check"] };
                match std::process::Command::new(exe).args(args).env("THC_ACTOR", "human").output() {
                    Ok(o) if o.status.success() => {
                        if let Some(v) = thc_core::release::available(env!("CARGO_PKG_VERSION")) {
                            shared.log("update", &format!("thc {v} available"));
                        }
                    }
                    Ok(o) => shared.log("update", &format!("check failed: {}", String::from_utf8_lossy(&o.stderr).trim())),
                    Err(e) => shared.log("update", &format!("check failed: {e}")),
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(6 * 3600));
    }
}

static SIGNALLED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    SIGNALLED.store(true, Ordering::SeqCst);
}

fn install_signal_handlers() {
    // SAFETY: the handler only stores to an atomic.
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

struct LoopState {
    open_conflicts: HashSet<(String, String)>,
    fs_dirty_at: Option<Instant>,
    drop_dirty: bool,
    export_due: Option<Instant>,
    last_rescan: Instant,
    pending: Vec<PendingDelivery>,
    /// alert id -> fire_at it was delivered for (to withdraw if acted on elsewhere)
    fired_here: HashMap<String, String>,
    /// Log file sizes at the last check: a 1 s safety net for dropped FS events.
    last_sizes: Vec<(String, u64)>,
    last_size_check: Instant,
}

fn main_loop(shared: &Arc<Shared>, paths: &Paths, rx: mpsc::Receiver<notify::Result<notify::Event>>) -> Result<()> {
    let open_conflicts = {
        let mut v = shared.vault.lock().unwrap();
        // What was in the store at start isn't news to push.
        v.take_news();
        v.store.open_conflicts()?.into_iter().map(|(_, n, f, _)| (n, f)).collect()
    };
    let mut st = LoopState {
        open_conflicts,
        fs_dirty_at: None,
        drop_dirty: true, // ingest anything waiting at startup
        export_due: Some(Instant::now()),
        last_rescan: Instant::now(),
        pending: vec![],
        fired_here: HashMap::new(),
        last_sizes: shared.vault.lock().unwrap().log.files().unwrap_or_default(),
        last_size_check: Instant::now(),
    };
    loop {
        if shared.shutdown.load(Ordering::SeqCst) || SIGNALLED.load(Ordering::SeqCst) {
            return Ok(());
        }
        // Drain watcher events (wait up to 100 ms for the first one).
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(ev) => {
                note_fs_event(&mut st, paths, ev);
                while let Ok(ev) = rx.try_recv() {
                    note_fs_event(&mut st, paths, ev);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            // Shutting down: the watcher's thread let it go, which is the normal end.
            Err(mpsc::RecvTimeoutError::Disconnected) if shared.shutdown.load(Ordering::SeqCst) || SIGNALLED.load(Ordering::SeqCst) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Disconnected) => bail!("file watcher stopped"),
        }
        if st.last_size_check.elapsed() >= Duration::from_secs(1) {
            st.last_size_check = Instant::now();
            let sizes = shared.vault.lock().unwrap().log.files().unwrap_or_default();
            if sizes != st.last_sizes {
                st.last_sizes = sizes;
                st.fs_dirty_at.get_or_insert(Instant::now() - DEBOUNCE);
            }
        }
        let quiet = st.fs_dirty_at.is_some_and(|t| t.elapsed() >= DEBOUNCE);
        let rescan = st.last_rescan.elapsed() >= RESCAN_EVERY;
        if quiet || rescan || shared.dirty.swap(false, Ordering::SeqCst) {
            st.fs_dirty_at = None;
            if rescan {
                st.last_rescan = Instant::now();
            }
            if let Err(e) = sync_step(shared, &mut st, rescan) {
                shared.log("error", &format!("{e:#}"));
            }
            // Once a day, on a quiet rescan: compact the store (store.rs vacuum_if_due).
            if rescan && st.fs_dirty_at.is_none() {
                let now = chrono::Utc::now().timestamp_millis();
                match shared.vault.lock().unwrap().store.vacuum_if_due(now, 24 * 3600 * 1000) {
                    Ok(true) => shared.log("store", "compacted"),
                    Ok(false) => {}
                    Err(e) => shared.log("error", &format!("compact: {e:#}")),
                }
            }
        }
        if st.drop_dirty {
            st.drop_dirty = false;
            if let Err(e) = ingest_drop(shared, paths) {
                shared.log("error", &format!("ingest: {e:#}"));
            }
        }
        if let Err(e) = alert_step(shared, &mut st) {
            shared.log("error", &format!("alerts: {e:#}"));
        }
        if st.export_due.is_some_and(|t| Instant::now() >= t) {
            st.export_due = None;
            let v = shared.vault.lock().unwrap();
            match thc_core::export::markdown(&v.store, &paths.vault.join("export"), thc_core::dates::today()) {
                Ok(s) => drop(s),
                Err(e) => shared.log("error", &format!("export: {e:#}")),
            }
        }
    }
}

fn note_fs_event(st: &mut LoopState, paths: &Paths, ev: notify::Result<notify::Event>) {
    let Ok(ev) = ev else { return };
    let drop_dir = paths.vault.join("drop");
    for p in &ev.paths {
        if p.starts_with(&drop_dir) {
            if !p.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.')) {
                st.drop_dirty = true;
            }
        } else {
            st.fs_dirty_at = Some(Instant::now());
        }
    }
}

/// Catch up with the log, then push what changed (from any device, including this one's CLI).
fn sync_step(shared: &Arc<Shared>, st: &mut LoopState, rescan: bool) -> Result<()> {
    let (fresh, conflicts, device) = {
        let mut v = shared.vault.lock().unwrap();
        let replayed = v.catch_up()?;
        // Every event applied since the last step: clients' writes here, other processes' and
        // other devices'. Not a watermark: another device's events arrive with older times, and
        // a rebuild renumbers rows (two-device soak: those were never pushed).
        let news = v.take_news();
        let fresh = if news.overflow {
            // (A first sync or a long time away: the newest transactions stand for the rest.)
            let mut f = v.store.history_where("1 ORDER BY okey DESC LIMIT 500", &[])?;
            f.reverse();
            f
        } else {
            let eids: Vec<&str> = news.events.iter().map(|(e, _)| e.as_str()).collect();
            v.store.history_where("eid IN (SELECT value FROM json_each(?1)) ORDER BY okey", &[&serde_json::to_string(&eids)?])?
        };
        let conflicts: Vec<(String, String)> = v.store.open_conflicts()?.into_iter().map(|(_, n, f, _)| (n, f)).collect();
        if rescan {
            shared.log("rescan", &format!("ok · {replayed} new events"));
        }
        (fresh, conflicts, v.device.clone())
    };
    if !fresh.is_empty() {
        // Group by transaction for both the log line and the pushed events.
        let mut by_tx: Vec<(String, Vec<&thc_core::model::HistoryEntry>)> = Vec::new();
        for e in &fresh {
            match by_tx.last_mut() {
                Some((tx, v)) if *tx == e.tx => v.push(e),
                _ => by_tx.push((e.tx.clone(), vec![e])),
            }
        }
        let mut per_dev: HashMap<String, (usize, usize, HashSet<String>)> = HashMap::new();
        for (tx, evs) in &by_tx {
            let first = evs[0];
            let mut ids: Vec<String> = evs.iter().filter(|e| e.entity.len() == 12).map(|e| e.entity.clone()).collect();
            ids.dedup();
            shared.broadcast("changed", "changed", json!({ "ids": ids, "tx": tx, "actor": first.actor, "dev": first.dev }));
            let entry = per_dev.entry(first.dev.clone()).or_default();
            entry.0 += evs.len();
            entry.1 += 1;
            if first.actor.starts_with("agent") {
                entry.2.insert(first.actor.trim_start_matches("agent:").to_string());
            }
        }
        let last = fresh.last().unwrap();
        *shared.last_change.lock().unwrap() =
            Some((chrono::DateTime::from_timestamp_millis(last.ms).map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%dT%H:%M:%S").to_string()).unwrap_or_default(), last.dev.clone()));
        for (dev, (n, txs, agents)) in per_dev {
            if dev == device {
                continue; // our own CLI/TUI writes: pushed, but not worth a log line
            }
            let mut detail = format!("{dev}  {n} events · {txs} tx");
            if !agents.is_empty() {
                let mut a: Vec<_> = agents.into_iter().collect();
                a.sort();
                detail.push_str(&format!(" by {}", a.join(", ")));
            }
            shared.log("replay", &detail);
        }
        st.export_due = Some(Instant::now() + EXPORT_AFTER);
    }
    // New conflicts.
    let now_set: HashSet<(String, String)> = conflicts.into_iter().collect();
    for (node, field) in now_set.difference(&st.open_conflicts) {
        let kind = if field == "parent" { "move" } else { "text" };
        let short = shared.vault.lock().unwrap().store.short(node);
        shared.broadcast("changed", "conflict", json!({ "id": node, "kind": kind }));
        shared.log("conflict", &format!("{short} {kind} · {}", if kind == "text" { "kept both versions" } else { "move skipped (would loop)" }));
    }
    st.open_conflicts = now_set;
    // Withdraw notifications for alerts that were acted on (here or on another device).
    withdraw_handled(shared, st)?;
    Ok(())
}

fn withdraw_handled(shared: &Arc<Shared>, st: &mut LoopState) -> Result<()> {
    if st.fired_here.is_empty() {
        return Ok(());
    }
    let mut gone = Vec::new();
    {
        let v = shared.vault.lock().unwrap();
        for (id, fired_for) in &st.fired_here {
            let alert = v.store.alerts_where("id=?1", &[id])?.into_iter().next();
            let node_done = alert
                .as_ref()
                .and_then(|a| v.store.node(&a.node).ok().flatten())
                .map(|n| n.deleted || matches!(n.status.as_deref(), Some("done" | "cancelled")))
                .unwrap_or(true);
            let handled = match &alert {
                None => true,
                Some(a) => a.state == "acked" || a.fire_at.as_deref() != Some(fired_for.as_str()) || node_done,
            };
            if handled {
                gone.push(id.clone());
            }
        }
    }
    if gone.is_empty() {
        return Ok(());
    }
    for id in &gone {
        st.fired_here.remove(id);
        shared.channel.withdraw(id);
    }
    shared.broadcast("alerts", "alert.withdraw", json!({ "alerts": gone }));
    Ok(())
}

/// Fire due alerts: offer them to delivering clients, fall back to the OS after the grace.
fn alert_step(shared: &Arc<Shared>, st: &mut LoopState) -> Result<()> {
    let now = alerts::now();
    // Fallbacks whose grace expired without a claim.
    let mut still = Vec::new();
    for p in std::mem::take(&mut st.pending) {
        let claimed = {
            let d = shared.delivered.lock().unwrap();
            p.alerts.iter().all(|a| d.contains(a))
        };
        if claimed {
            continue;
        }
        if Instant::now() >= p.deadline {
            let via = shared.channel.deliver(&p.delivery);
            shared.log("alert", &format!("{} → {via} (no client claimed it)", delivery_label(&p.delivery)));
        } else {
            still.push(p);
        }
    }
    st.pending = still;

    let due = {
        let v = shared.vault.lock().unwrap();
        v.store.due_alerts(now)?
    };
    if due.is_empty() {
        return Ok(());
    }
    let plan = {
        let v = shared.vault.lock().unwrap();
        alerts::plan(&v.store, &due, now)
    };
    {
        let v = shared.vault.lock().unwrap();
        for (a, _) in &due {
            v.store.mark_fired(a, "daemon", now)?;
            if let Some(f) = &a.fire_at {
                st.fired_here.insert(a.id.clone(), f.clone());
            }
        }
    }
    for mut d in plan {
        // Not home: the vault leads (`acme · Book the venue · due 17:00`, vaults.md §3.4).
        if let Some(label) = &shared.vault_label {
            match &mut d {
                Delivery::Single { notification } => notification.title = format!("{label} · {}", notification.title),
                Delivery::Summary { title, .. } => *title = format!("{label} · {title}"),
                Delivery::Silent { .. } => {}
            }
        }
        let ids = alerts::delivery_alerts(&d);
        if matches!(d, Delivery::Silent { .. }) {
            shared.log("alert", &format!("{} marked fired (over 12 h late)", ids.len()));
            continue;
        }
        shared.broadcast("alerts", "alert.fire", serde_json::to_value(&d)?);
        if shared.has_deliverer() {
            shared.log("alert", &format!("{} → client", delivery_label(&d)));
            st.pending.push(PendingDelivery { delivery: d, alerts: ids, deadline: Instant::now() + DELIVERY_GRACE });
        } else {
            let via = shared.channel.deliver(&d);
            shared.log("alert", &format!("{} → {via}", delivery_label(&d)));
        }
    }
    Ok(())
}

fn delivery_label(d: &Delivery) -> String {
    match d {
        Delivery::Single { notification } => format!("{} {}", &notification.alert[..5.min(notification.alert.len())], notification.title),
        Delivery::Summary { title, .. } => title.clone(),
        Delivery::Silent { alerts } => format!("{} silent", alerts.len()),
    }
}

fn ingest_drop(shared: &Arc<Shared>, paths: &Paths) -> Result<()> {
    let drop_dir = paths.vault.join("drop");
    let mut v = shared.vault.lock().unwrap();
    let device = v.device.clone();
    for f in thc_core::ingest::claim(&drop_dir, &device)? {
        let path = f.clone();
        let today = thc_core::dates::today();
        let mut kept = Vec::new();
        let ((), n) = {
            let mut count = 0;
            v.transact(|s| {
                let mut b = thc_core::builder::TxBuilder::new(s, today);
                let got = thc_core::ingest::ingest_file(&mut b, &path)?;
                count = got.ids.len();
                kept = got.kept;
                Ok((b.finish(), ()))
            })?;
            ((), count)
        };
        thc_core::ingest::record_notices(&paths.cache, &kept)?;
        for k in &kept {
            shared.log("ingest", k);
        }
        thc_core::ingest::archive(&f, &drop_dir)?;
        let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let name = name.split_once("--").map(|(_, n)| n.to_string()).unwrap_or(name);
        shared.log("ingest", &format!("drop/{name} → {n} inbox items"));
        shared.dirty.store(true, Ordering::SeqCst);
    }
    Ok(())
}

/// Status object (daemon.md §3.1).
pub fn status_json(shared: &Shared) -> Result<Value> {
    let v = shared.vault.lock().unwrap();
    let devices = v.log.files()?.iter().map(|(f, _)| f.split('/').next().unwrap_or("").to_string()).collect::<HashSet<_>>().len();
    let now = alerts::now();
    let pending = v.store.alerts_where("deleted=0 AND state IN ('pending','snoozed')", &[])?;
    let fired = pending.iter().filter(|a| v.store.fired_at(a).is_some()).count();
    let next = pending
        .iter()
        .filter(|a| v.store.fired_at(a).is_none() && a.fire_at.as_deref().is_some_and(|f| f > now.format("%Y-%m-%dT%H:%M").to_string().as_str()))
        .min_by_key(|a| a.fire_at.clone());
    let clients: Vec<Value> = shared.clients.lock().unwrap().iter().map(|c| json!({ "kind": c.kind })).collect();
    let last = shared.last_change.lock().unwrap().clone();
    Ok(json!({
        "state": "live",
        "pid": std::process::id(),
        "uptime_s": shared.started.elapsed().as_secs(),
        "watching": shared.watching.load(Ordering::SeqCst),
        "version": VERSION,
        "exe": std::env::current_exe().ok().map(|p| p.display().to_string()),
        "proto": PROTO_VERSION,
        "socket": shared.socket,
        "vault": v.paths.vault,
        "device": v.device,
        "devices": devices,
        "last_change": last.map(|(at, dev)| json!({ "at": at, "dev": dev })),
        "alerts": {
            "pending": pending.len() - fired,
            "fired": fired,
            "next": next.map(|a| json!({ "id": a.id, "node": a.node, "at": a.fire_at, "title": v.store.node(&a.node).ok().flatten().map(|n| alerts::plain_title(&v.store, &n)) })),
        },
        "notify": if shared.has_deliverer() { "thoughtbar".to_string() } else { shared.channel.name().to_string() },
        "clients": clients,
    }))
}
