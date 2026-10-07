//! `thc daemon status|start|stop|restart|install|uninstall` (docs/design/daemon.md §3).

use crate::out::Out;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use thc_core::error::ThcError;
use thc_core::proto::{self, Client, VERSION};
use thc_core::vault::Paths;

const LABEL: &str = "dev.thought.thc";

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

fn tilde(p: &std::path::Path) -> String {
    let h = home();
    match p.strip_prefix(&h) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

fn plist_path() -> PathBuf {
    let p = home().join("Library/LaunchAgents").join(format!("{LABEL}.plist"));
    thc_core::sandbox::check(&p);
    p
}

fn unit_path() -> PathBuf {
    let p = home().join(".config/systemd/user/thc.service");
    thc_core::sandbox::check(&p);
    p
}

/// What runs a daemon: launchd (thc's LaunchAgent), systemd (thc's `--user` unit), or nothing
/// (a plain background process `thc daemon start` spawned).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Manager {
    Launchd,
    Systemd,
    Plain,
}

impl Manager {
    pub fn name(self) -> &'static str {
        match self {
            Manager::Launchd => "launchd",
            Manager::Systemd => "systemd",
            Manager::Plain => "plain",
        }
    }
}

/// This OS's service manager for the login item. Tests choose one with THC_SERVICE_MANAGER
/// (honoured only under THC_TEST, with the tool shimmed on PATH).
fn os_manager() -> Manager {
    if thc_core::sandbox::active() {
        match std::env::var("THC_SERVICE_MANAGER").as_deref() {
            Ok("launchd") => return Manager::Launchd,
            Ok("systemd") => return Manager::Systemd,
            Ok("none") => return Manager::Plain,
            _ => {}
        }
    }
    if cfg!(target_os = "macos") {
        Manager::Launchd
    } else if cfg!(target_os = "linux") {
        Manager::Systemd
    } else {
        Manager::Plain
    }
}

/// thc's login item file (plist or unit), whether or not it exists.
fn item_path() -> Option<PathBuf> {
    match os_manager() {
        Manager::Launchd => Some(plist_path()),
        Manager::Systemd => Some(unit_path()),
        Manager::Plain => None,
    }
}

fn item_text() -> Option<String> {
    std::fs::read_to_string(item_path()?).ok()
}

/// A value from the login item's environment (`THC_VAULT`, `THC_CACHE_DIR`): which vault the
/// login daemon serves, whatever the caller's directory resolves to.
pub(crate) fn login_env(key: &str) -> Option<String> {
    let s = item_text()?;
    match os_manager() {
        Manager::Launchd => {
            let rest = &s[s.find(&format!("<key>{key}</key>"))?..];
            let a = rest.find("<string>")? + "<string>".len();
            let b = rest[a..].find("</string>")?;
            Some(rest[a..a + b].to_string())
        }
        Manager::Systemd => s.lines().find_map(|l| l.trim().strip_prefix("Environment=")?.strip_prefix(&format!("{key}=")).map(str::to_string)),
        Manager::Plain => None,
    }
}

/// The binary the login item runs (ProgramArguments[0] / ExecStart's program).
pub(crate) fn login_exe() -> Option<PathBuf> {
    let s = item_text()?;
    match os_manager() {
        Manager::Launchd => {
            let rest = &s[s.find("<key>ProgramArguments</key>")?..];
            let a = rest.find("<string>")? + "<string>".len();
            let b = rest[a..].find("</string>")?;
            Some(PathBuf::from(&rest[a..a + b]))
        }
        Manager::Systemd => s.lines().find_map(|l| l.trim().strip_prefix("ExecStart=")?.split_whitespace().next().map(PathBuf::from)),
        Manager::Plain => None,
    }
}

/// The login daemon's vault and cache, from its login item.
pub fn login_paths() -> Option<Paths> {
    let vault = PathBuf::from(login_env("THC_VAULT")?);
    let cache = login_env("THC_CACHE_DIR").map(PathBuf::from).unwrap_or_else(|| thc_core::vault::default_cache(&vault));
    Some(Paths { vault, cache })
}

/// Who runs the daemon for this vault: the login item's manager when it serves this vault (so
/// starts and restarts go through it and thc never spawns a second one beside it), else plain.
pub fn manager_for(paths: &Paths) -> Manager {
    match os_manager() {
        Manager::Plain => Manager::Plain,
        m => {
            if login_env("THC_VAULT").is_some_and(|v| Path::new(&v) == paths.vault) {
                m
            } else {
                Manager::Plain
            }
        }
    }
}

fn canon(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// The binary a daemon for `paths` should be running: its login item's, or `exe` (the caller)
/// for a plain one.
pub fn want_exe(m: Manager, exe: &Path) -> PathBuf {
    match m {
        Manager::Plain => exe.to_path_buf(),
        _ => login_exe().unwrap_or_else(|| exe.to_path_buf()),
    }
}

/// Why the daemon that answered `st` isn't running `want` as that file is now: another binary,
/// or an older copy of it (an install renamed a new file over it since the daemon started,
/// which keeps the path, and the version string too when only the build changed). None when
/// it's current.
pub fn stale_reason(st: &Value, want: &Path) -> Option<String> {
    let ver = st["version"].as_str().unwrap_or("?");
    let raw = st["exe"].as_str()?;
    // Linux names a replaced binary "<path> (deleted)".
    let deleted = raw.ends_with(" (deleted)");
    let exe = PathBuf::from(raw.trim_end_matches(" (deleted)"));
    if canon(&exe) != canon(want) {
        return Some(format!("it runs {} (thc {ver}), not {}", tilde(&exe), tilde(want)));
    }
    let older = || Some(format!("it runs an older copy of {} (thc {ver}) that was replaced after it started", tilde(want)));
    if deleted {
        return older();
    }
    let want_id = proto::exe_identity(want)?;
    match st["exe_id"].as_str() {
        Some(id) => (id != want_id).then(older).flatten(),
        // A daemon from before exe_id doesn't say which file it started from: compare when the
        // file last changed (a rename sets its ctime) with when the daemon started.
        None => {
            use std::os::unix::fs::MetadataExt;
            let m = std::fs::metadata(want).ok()?;
            let changed_ms = m.ctime().max(m.mtime()) * 1000;
            let now_ms = chrono::Utc::now().timestamp_millis();
            let started_ms = st["started_ms"].as_i64().unwrap_or_else(|| now_ms - st["uptime_s"].as_i64().unwrap_or(0) * 1000);
            (changed_ms > started_ms + 2000).then(older).flatten()
        }
    }
}

/// `launchctl kickstart [-k]` on thc's job, loading the plist first when launchd doesn't know it
/// yet. Returns whether launchctl accepted it.
fn kickstart(kill: bool) -> bool {
    let uid = unsafe { libc::getuid() };
    let target = format!("gui/{uid}/{LABEL}");
    let run = || {
        let mut c = thc_core::sandbox::tool("launchctl");
        c.arg("kickstart");
        if kill {
            c.arg("-k");
        }
        c.arg(&target).output().is_ok_and(|o| o.status.success())
    };
    if run() {
        return true;
    }
    let _ = thc_core::sandbox::tool("launchctl").args(["bootstrap", &format!("gui/{uid}"), &plist_path().to_string_lossy()]).output();
    run()
}

/// `systemctl --user <verb> thc.service` (or `daemon-reload`); whether it succeeded.
fn systemctl(args: &[&str]) -> Result<()> {
    let o = thc_core::sandbox::tool("systemctl").arg("--user").args(args).output()?;
    if o.status.success() {
        Ok(())
    } else {
        Err(anyhow!("systemctl --user {} failed: {}", args.join(" "), String::from_utf8_lossy(&o.stderr).trim()))
    }
}

/// Wait until a daemon other than `old` answers for `paths` and `old` has gone.
fn wait_replaced(paths: &Paths, old: Option<u32>, secs: u64) -> Option<Value> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if let Some(st) = status(paths) {
            let pid = st["pid"].as_u64().map(|p| p as u32);
            if pid != old && old.is_none_or(|o| !proto::pid_alive(o)) {
                return Some(st);
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

fn not_replaced(paths: &Paths, old: Option<u32>, who: &str) -> anyhow::Error {
    let now = status(paths).and_then(|s| s["pid"].as_u64());
    let why = match (old, now) {
        (Some(o), Some(n)) if n as u32 == o => format!("pid {o} is still the one answering"),
        (Some(o), _) if proto::pid_alive(o) => format!("the old daemon (pid {o}) is still running"),
        (_, None) => "no daemon answers".into(),
        _ => "it didn't settle".into(),
    };
    anyhow!("{who} didn't restart the daemon: {why} · log: {}", tilde(&paths.cache.join("daemon.log")))
}

/// Restart the daemon for `paths` through what runs it, and wait for the new process: launchd
/// `kickstart -k` (it stops the old process and starts the new one), `systemctl --user restart`,
/// or for a plain one a stop and a fresh spawn of `exe`. Checks that exactly that happened: a
/// new pid answers and the old one is gone.
fn restart_via(m: Manager, paths: &Paths, exe: &Path) -> Result<Value> {
    let old = status(paths).and_then(|s| s["pid"].as_u64()).map(|p| p as u32);
    match m {
        Manager::Launchd => {
            if !kickstart(true) {
                return Err(anyhow!("launchctl couldn't restart {LABEL} · launchctl print gui/$(id -u)/{LABEL}"));
            }
        }
        Manager::Systemd => systemctl(&["restart", "thc.service"])?,
        Manager::Plain => {
            stop(paths)?;
            return spawn_plain(paths, exe);
        }
    }
    wait_replaced(paths, old, 10).ok_or_else(|| not_replaced(paths, old, m.name()))
}

/// What [`ensure`] did.
#[derive(Debug)]
pub struct Ensured {
    /// offline | current | restarted
    pub action: &'static str,
    pub manager: Manager,
    pub before: Option<Value>,
    pub after: Option<Value>,
    /// Why it was restarted (stale), when it was.
    pub reason: Option<String>,
}

impl Ensured {
    pub fn json(&self) -> Value {
        let brief = |v: &Option<Value>| v.as_ref().map(|s| json!({ "pid": s["pid"], "version": s["version"], "exe": s["exe"], "exe_id": s["exe_id"] }));
        json!({ "action": self.action, "manager": self.manager.name(), "before": brief(&self.before), "after": brief(&self.after), "reason": self.reason, "line": self.line() })
    }

    pub fn line(&self) -> String {
        let v = |s: &Option<Value>, k: &str| s.as_ref().map(|s| s[k].to_string().trim_matches('"').to_string()).unwrap_or_else(|| "?".into());
        match self.action {
            "offline" => "no daemon running".into(),
            "current" => format!("daemon current · pid {} · thc {}", v(&self.before, "pid"), v(&self.before, "version")),
            _ => format!(
                "daemon restarted on thc {} · pid {} → {} ({}){}",
                v(&self.after, "version"),
                v(&self.before, "pid"),
                v(&self.after, "pid"),
                self.manager.name(),
                self.reason.as_ref().map(|r| format!(" · {r}")).unwrap_or_default()
            ),
        }
    }
}

/// Make the daemon for `paths` run the binary it should (its login item's, or `exe` for a plain
/// one): one on another binary, or an older copy of it, restarts through whatever runs it
/// (any running one does with `force`), then must come back current. Never starts one that
/// isn't running.
pub fn ensure(paths: &Paths, exe: &Path, force: bool) -> Result<Ensured> {
    let m = manager_for(paths);
    let want = want_exe(m, exe);
    let Some(before) = status(paths) else {
        return Ok(Ensured { action: "offline", manager: m, before: None, after: None, reason: None });
    };
    let reason = stale_reason(&before, &want);
    if reason.is_none() && !force {
        return Ok(Ensured { action: "current", manager: m, before: Some(before), after: None, reason: None });
    }
    if m == Manager::Plain && test_guarded() {
        return Err(anyhow!("daemon not restarted: THC_TEST is set (THC_TEST_DAEMON=1 to start one)"));
    }
    let after = restart_via(m, paths, &want)?;
    if let Some(r) = stale_reason(&after, &want) {
        return Err(anyhow!("the daemon came back stale: {r} · thc daemon restart"));
    }
    Ok(Ensured { action: "restarted", manager: m, before: Some(before), after: Some(after), reason })
}

/// Under THC_TEST a plain daemon outlives the test that started it (a first run in a scratch
/// HOME left one running): only tests that ask for one get it.
fn test_guarded() -> bool {
    thc_core::sandbox::active() && std::env::var("THC_TEST_DAEMON").map_or(true, |v| v != "1")
}

/// For `thc doctor` and `thc daemon status`: the vault's daemon, and an issue line when it isn't
/// on the binary it should be (or isn't this thc's version).
pub fn health(paths: &Paths) -> (Value, Option<String>) {
    let Some(st) = status(paths) else {
        return (json!({ "state": "offline" }), None);
    };
    let me = std::env::current_exe().unwrap_or_default();
    let m = manager_for(paths);
    let want = want_exe(m, &me);
    let reason = stale_reason(&st, &want);
    let ver = st["version"].as_str().unwrap_or("?").to_string();
    let v = json!({ "state": "live", "pid": st["pid"], "version": ver, "exe": st["exe"], "manager": m.name(), "stale": reason });
    let issue = match reason {
        Some(r) => Some(format!("the daemon (pid {}) is stale: {r} · thc daemon restart", st["pid"])),
        None if ver != VERSION => {
            let hint = if m != Manager::Plain && canon(&want) != canon(&me) { "thc daemon install points its login item at this thc" } else { "thc daemon restart" };
            Some(format!("the daemon is thc {ver} ({}), this is thc {VERSION} ({}) · {hint}", tilde(&want), tilde(&me)))
        }
        None => None,
    };
    (v, issue)
}

pub fn install_info() -> Value {
    match (os_manager(), item_path()) {
        (Manager::Launchd, Some(p)) if p.exists() => json!({ "kind": "launchd", "label": LABEL, "loaded": true, "path": p, "exe": login_exe() }),
        (Manager::Systemd, Some(p)) if p.exists() => json!({ "kind": "systemd", "unit": "thc.service", "loaded": true, "path": p, "exe": login_exe() }),
        _ => json!({ "kind": "none" }),
    }
}

pub fn status(paths: &Paths) -> Option<Value> {
    let mut c = Client::connect(paths)?;
    c.call("status", Value::Null).ok()
}

fn fmt_uptime(s: u64) -> String {
    match s {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s => format!("{}h {}m", s / 3600, (s % 3600) / 60),
    }
}

pub fn run(out: &mut Out, paths: &Paths, cmd: &crate::cli::DaemonCmd) -> Result<()> {
    use crate::cli::DaemonCmd::*;
    match cmd {
        Run => unreachable!(),
        Status => {
            let install = install_info();
            match status(paths) {
                Some(mut st) => {
                    let me = std::env::current_exe().unwrap_or_default();
                    if let Some(r) = stale_reason(&st, &want_exe(manager_for(paths), &me)) {
                        st["stale"] = json!(r);
                    }
                    st["install"] = install.clone();
                    st["vault_source"] = crate::vault_source_json();
                    if out.json {
                        out.json(&st);
                    } else {
                        print_status(out, &st, &install);
                    }
                    Ok(())
                }
                None => {
                    if out.json {
                        out.json(&json!({ "state": "offline", "install": install, "vault": paths.vault, "vault_source": crate::vault_source_json() }));
                    } else {
                        let k = out.dim(&format!("{:<12}", "○ local"));
                        out.line(format!("{k}daemon offline"));
                        let why = crate::VAULT_SOURCE.get().map(|s| out.dim(&format!(" · {}", s.describe()))).unwrap_or_default();
                        out.line(format!("{}{}{why}", out.dim(&format!("{:<12}", "vault")), tilde(&paths.vault)));
                        for l in [
                            "Everything works. Alerts won't fire, and changes from other".to_string(),
                            "devices show up on your next command.".to_string(),
                            format!("thc daemon start       {}", out.dim("run it now")),
                            format!("thc daemon install     {}", out.dim("run it at every login")),
                        ] {
                            out.line(format!("{:<12}{l}", ""));
                        }
                    }
                    out.flush();
                    Err(ThcError::NotFound("daemon offline".into()).into()).map_err(|e: anyhow::Error| e.context(Quiet))
                }
            }
        }
        Start => start(out, paths, false),
        Stop => {
            let was = stop(paths)?;
            if out.json {
                out.json(&json!({ "ok": true, "was_running": was, "install": install_info() }));
                return Ok(());
            }
            if was {
                out.line(format!("{} daemon stopped · alerts are paused until it runs again", out.dim("○")));
                if install_info()["kind"] != "none" {
                    out.line(out.dim("  it will start again at login · thc daemon uninstall to stop that"));
                }
            } else {
                out.line(format!("{} not running", out.dim("○")));
            }
            Ok(())
        }
        // Under launchd, a stop then a start could race it (a plain kickstart is a no-op while
        // the old process is still exiting) and end with two daemons: kickstart -k instead.
        Restart => match manager_for(paths) {
            Manager::Plain => {
                stop(paths)?;
                start(out, paths, true)
            }
            m => {
                let st = restart_via(m, paths, &want_exe(m, &std::env::current_exe()?))?;
                if out.json {
                    out.json(&st);
                } else {
                    out.line(format!("{} daemon restarted · pid {} · thc {}", out.green("●"), st["pid"], st["version"].as_str().unwrap_or("?")));
                }
                Ok(())
            }
        },
        Install => install(out, paths),
        Uninstall => {
            let _ = stop(paths);
            if os_manager() == Manager::Launchd {
                let p = plist_path();
                if p.exists() {
                    let uid = unsafe { libc::getuid() };
                    let _ = thc_core::sandbox::tool("launchctl").args(["bootout", &format!("gui/{uid}"), &p.to_string_lossy()]).output();
                    std::fs::remove_file(&p)?;
                }
            } else if os_manager() == Manager::Systemd && unit_path().exists() {
                let _ = thc_core::sandbox::tool("systemctl").args(["--user", "disable", "--now", "thc.service"]).output();
                std::fs::remove_file(unit_path())?;
            }
            if out.json {
                out.json(&json!({ "ok": true }));
            } else {
                out.line(format!("{} removed the login item · the daemon is stopped", out.dim("○")));
            }
            Ok(())
        }
    }
}

/// Marker context: the message was already printed (status offline), only the exit code matters.
#[derive(Debug)]
pub struct Quiet;
impl std::fmt::Display for Quiet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "")
    }
}

fn print_status(out: &mut Out, st: &Value, install: &Value) {
    let key = |out: &Out, k: &str| out.dim(&format!("{k:<12}"));
    let dot = out.green("●");
    out.line(format!(
        "{dot} {}pid {} · up {} · thc {}",
        out.dim(&format!("{:<10}", "live")),
        st["pid"],
        fmt_uptime(st["uptime_s"].as_u64().unwrap_or(0)),
        st["version"].as_str().unwrap_or("?")
    ));
    let vault = st["vault"].as_str().map(|v| tilde(std::path::Path::new(v))).unwrap_or_default();
    let why = crate::VAULT_SOURCE.get().map(|s| format!(" ({})", s.describe())).unwrap_or_default();
    let mut vline = format!("{vault}{why} · {} device{}", st["devices"], if st["devices"] == 1 { "" } else { "s" });
    if let Some(lc) = st["last_change"].as_object() {
        let at = lc.get("at").and_then(|v| v.as_str()).unwrap_or("");
        vline.push_str(&format!(" · last change {} from {}", at.get(11..16).unwrap_or(at), lc.get("dev").and_then(|v| v.as_str()).unwrap_or("?")));
    }
    out.line(format!("{}{vline}", key(out, "vault")));
    let a = &st["alerts"];
    let next = match a["next"].as_object() {
        Some(n) => {
            let at = n.get("at").and_then(|v| v.as_str()).unwrap_or("");
            let now = thc_core::dates::now_local();
            let when = thc_core::dates::DateVal::from_stored(at)
                .map(|d| {
                    let t = d.time().map(|t| t.format(" %H:%M").to_string()).unwrap_or_default();
                    let rel = thc_core::dates::relative(d.date(), now.date());
                    let at_dt = d.time().map(|t| d.date().and_time(t)).unwrap_or_else(|| d.date().and_hms_opt(9, 0, 0).unwrap());
                    let mins = (at_dt - now).num_minutes();
                    let delta = match mins {
                        m if m < 0 => "now".to_string(),
                        m if m < 60 => format!("in {m}m"),
                        m if m < 24 * 60 => format!("in {}h {}m", m / 60, m % 60),
                        m => format!("in {}d {}h", m / 1440, (m % 1440) / 60),
                    };
                    format!("{rel}{t} ({delta})")
                })
                .unwrap_or_default();
            format!("next {} · {when} · ", n.get("title").and_then(|v| v.as_str()).unwrap_or("?"))
        }
        None => String::new(),
    };
    out.line(format!("{}{next}{} pending · {} fired", key(out, "alerts"), a["pending"], a["fired"]));
    let notify = match st["notify"].as_str().unwrap_or("") {
        "thoughtbar" => "Thought Central".to_string(),
        "terminal-notifier" => "terminal-notifier".to_string(),
        "osascript" => "osascript (no actions)".to_string(),
        "notify-send" => "notify-send".to_string(),
        other => other.to_string(),
    };
    out.line(format!("{}{notify}", key(out, "notify")));
    let clients: Vec<String> = st["clients"].as_array().map(|a| a.iter().filter_map(|c| c["kind"].as_str().map(str::to_string)).collect()).unwrap_or_default();
    out.line(format!("{}{}", key(out, "clients"), if clients.is_empty() { "none".into() } else { clients.join(" · ") }));
    let inst = match install["kind"].as_str() {
        Some("launchd") => format!("login item (launchd, {LABEL})"),
        Some("systemd") => "systemd --user unit".to_string(),
        _ => format!("not installed · {}", out.dim("thc daemon install keeps it running")),
    };
    out.line(format!("{}{inst}", key(out, "install")));
    if let Some(r) = st["stale"].as_str() {
        out.line(format!("{}{}", key(out, "update"), out.yellow(&format!("stale: {r} · thc daemon restart"))));
    } else if st["version"].as_str().is_some_and(|v| v != VERSION) {
        out.line(format!("{}the daemon is thc {}, this is {VERSION} · thc daemon restart", key(out, "update"), st["version"].as_str().unwrap_or("?")));
    }
}

fn start(out: &mut Out, paths: &Paths, restarted: bool) -> Result<()> {
    let say = |out: &mut Out, st: &Value, restarted: bool| {
        if out.json {
            out.json(st);
        } else if restarted {
            out.line(format!("{} daemon restarted · pid {} · thc {}", out.green("●"), st["pid"], st["version"].as_str().unwrap_or("?")));
        } else {
            out.line(format!("{} daemon started · pid {} · watching {}", out.green("●"), st["pid"], tilde(&paths.vault)));
        }
    };
    // Running already: on the binary it should be (an install may have replaced it since), or
    // restarted onto it through whatever runs it.
    if status(paths).is_some() {
        let e = ensure(paths, &std::env::current_exe()?, false)?;
        match (e.action, &e.after, &e.before) {
            ("restarted", Some(st), _) => {
                if out.json {
                    out.json(st);
                } else {
                    out.line(format!("{} {}", out.green("●"), e.line()));
                }
            }
            (_, _, Some(st)) => {
                if out.json {
                    out.json(st);
                } else {
                    out.line(format!("{} already running · pid {}", out.green("●"), st["pid"]));
                }
            }
            // It stopped between the two looks: start it below.
            _ => return start(out, paths, restarted),
        }
        return Ok(());
    }
    // thc's login item serves this vault: ask its manager, so it owns the process (restarts,
    // login) instead of a child of this shell. If the manager accepts but nothing answers,
    // that's an error: a direct spawn beside its job would make two daemons.
    match manager_for(paths) {
        Manager::Launchd => {
            let accepted = kickstart(false);
            if let Some(st) = wait_live(paths, if accepted { 8 } else { 0 }) {
                say(out, &st, restarted);
                return Ok(());
            }
            if accepted {
                return Err(anyhow!("launchd didn't start the daemon · log: {} · launchctl print gui/$(id -u)/{LABEL}", tilde(&paths.cache.join("daemon.log"))));
            }
            // launchctl itself failed (no launchd here): start it directly, below.
        }
        Manager::Systemd => {
            systemctl(&["start", "thc.service"])?;
            let st = wait_live(paths, 8).ok_or_else(|| anyhow!("systemd didn't start the daemon · systemctl --user status thc.service"))?;
            say(out, &st, restarted);
            return Ok(());
        }
        Manager::Plain => {}
    }
    if test_guarded() {
        out.line("daemon not started: THC_TEST is set (THC_TEST_DAEMON=1 to start one)");
        return Ok(());
    }
    let st = spawn_plain(paths, &std::env::current_exe()?)?;
    say(out, &st, restarted);
    Ok(())
}

fn wait_live(paths: &Paths, secs: u64) -> Option<Value> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(st) = status(paths) {
            return Some(st);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Start `exe daemon run` for `paths` as a plain background process and wait for it to answer.
fn spawn_plain(paths: &Paths, exe: &Path) -> Result<Value> {
    std::fs::create_dir_all(&paths.cache)?;
    let log_path = paths.cache.join("daemon.log");
    let log = std::fs::OpenOptions::new().create(true).append(true).open(&log_path)?;
    let log_start = log.metadata().map(|m| m.len()).unwrap_or(0);
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["daemon", "run"])
        .env("THC_VAULT", &paths.vault)
        .env("THC_CACHE_DIR", &paths.cache)
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd.spawn()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(st) = status(paths) {
            return Ok(st);
        }
        // It died: say why, in its own words (the app shows this verbatim, mac-app.md §3).
        if let Ok(Some(code)) = child.try_wait() {
            let said = log_since(&log_path, log_start);
            let why = if said.is_empty() { format!("it exited ({code})") } else { said };
            return Err(anyhow!("the daemon didn't start: {why}"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let said = log_since(&log_path, log_start);
    let tail = if said.is_empty() { String::new() } else { format!(": {said}") };
    Err(anyhow!("the daemon didn't answer in 5 s{tail} · log: {}", tilde(&log_path)))
}

/// The last few lines the daemon wrote to its log after `from` (its startup error, usually).
fn log_since(path: &std::path::Path, from: u64) -> String {
    let bytes = std::fs::read(path).unwrap_or_default();
    let new = String::from_utf8_lossy(bytes.get(from as usize..).unwrap_or_default()).to_string();
    let lines: Vec<&str> = new.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines[lines.len().saturating_sub(4)..].join("\n")
}

/// Stop if running; returns whether it was.
fn stop(paths: &Paths) -> Result<bool> {
    let Some(mut c) = Client::connect(paths) else {
        // A stale socket/info file is cleaned up silently.
        let _ = std::fs::remove_file(proto::socket_path(paths));
        return Ok(false);
    };
    let pid = c.call("status", Value::Null).ok().and_then(|s| s["pid"].as_u64()).unwrap_or(0) as u32;
    let _ = c.call("shutdown", Value::Null);
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if !proto::socket_path(paths).exists() && (pid == 0 || !proto::pid_alive(pid)) {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    if pid != 0 && proto::pid_alive(pid) {
        unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    }
    Ok(true)
}

fn plist_xml(exe: &Path, vault: &Path, cache: &Path) -> String {
    let log = cache.join("daemon.log");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key><array><string>{}</string><string>daemon</string><string>run</string></array>
  <key>EnvironmentVariables</key><dict>
    <key>THC_VAULT</key><string>{}</string>
    <key>THC_CACHE_DIR</key><string>{}</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>StandardOutPath</key><string>{}</string>
  <key>StandardErrorPath</key><string>{}</string>
</dict>
</plist>
"#,
        exe.display(),
        vault.display(),
        cache.display(),
        log.display(),
        log.display()
    )
}

fn unit_text(exe: &Path, vault: &Path, cache: &Path) -> String {
    format!(
        "[Unit]\nDescription=thc daemon (thought-central)\n\n[Service]\nExecStart={} daemon run\nEnvironment=THC_VAULT={}\nEnvironment=THC_CACHE_DIR={}\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n",
        exe.display(),
        vault.display(),
        cache.display()
    )
}

fn install(out: &mut Out, paths: &Paths) -> Result<()> {
    let exe = std::env::current_exe()?;
    let m = os_manager();
    let Some(p) = item_path() else {
        return Err(anyhow!("no service manager to run it at login here · thc daemon start runs it now"));
    };
    if p.exists() {
        return refresh_item(out, paths, m, &p, &exe);
    }
    let _ = stop(paths);
    std::fs::create_dir_all(p.parent().unwrap())?;
    if m == Manager::Launchd {
        std::fs::write(&p, plist_xml(&exe, &paths.vault, &paths.cache))?;
        let uid = unsafe { libc::getuid() };
        let r = thc_core::sandbox::tool("launchctl").args(["bootstrap", &format!("gui/{uid}"), &p.to_string_lossy()]).output()?;
        if !r.status.success() {
            return Err(anyhow!("launchctl bootstrap failed: {}", String::from_utf8_lossy(&r.stderr).trim()));
        }
    } else {
        std::fs::write(&p, unit_text(&exe, &paths.vault, &paths.cache))?;
        systemctl(&["enable", "--now", "thc.service"])?;
    }
    let pid = wait_status(paths).map(|s| s["pid"].clone()).unwrap_or(Value::Null);
    if out.json {
        out.json(&json!({ "ok": true, "install": install_info(), "pid": pid }));
    } else if m == Manager::Launchd {
        out.line(format!("{} installed as a login item (launchd, {LABEL})", out.green("●")));
        out.line(format!("  {}   {}", out.dim("plist"), tilde(&p)));
        out.line(format!("  {} pid {pid}", out.dim("started")));
    } else {
        out.line(format!("{} installed as a systemd --user unit (thc.service)", out.green("●")));
        out.line(format!("  {}    {}", out.dim("unit"), tilde(&p)));
        out.line(format!("  {} pid {pid}", out.dim("started")));
    }
    Ok(())
}

/// `thc daemon install` with the login item already there (every reinstall and update runs
/// this through `thc setup`): point it at this binary if it names another, and make sure its
/// daemon runs and runs the binary as it is now. An install that renamed a new thc over the
/// old one leaves the old process running until something restarts it; this does, through
/// the manager that runs it, and waits for the new one to answer.
fn refresh_item(out: &mut Out, paths: &Paths, m: Manager, p: &Path, exe: &Path) -> Result<()> {
    let lp = login_paths().unwrap_or_else(|| paths.clone());
    let have = login_exe();
    let (daemon, note) = if have.as_deref().map(canon) != Some(canon(exe)) {
        // Another binary (an old path, the app's copy): this one runs at login from now on.
        let before = status(&lp);
        let old = before.as_ref().and_then(|s| s["pid"].as_u64()).map(|p| p as u32);
        if m == Manager::Launchd {
            std::fs::write(p, plist_xml(exe, &lp.vault, &lp.cache))?;
            let uid = unsafe { libc::getuid() };
            let _ = thc_core::sandbox::tool("launchctl").args(["bootout", &format!("gui/{uid}/{LABEL}")]).output();
            let r = thc_core::sandbox::tool("launchctl").args(["bootstrap", &format!("gui/{uid}"), &p.to_string_lossy()]).output()?;
            if !r.status.success() {
                return Err(anyhow!("launchctl bootstrap failed: {}", String::from_utf8_lossy(&r.stderr).trim()));
            }
        } else {
            std::fs::write(p, unit_text(exe, &lp.vault, &lp.cache))?;
            systemctl(&["daemon-reload"])?;
            systemctl(&["restart", "thc.service"])?;
        }
        let st = wait_replaced(&lp, old, 10).ok_or_else(|| not_replaced(&lp, old, m.name()))?;
        if let Some(r) = stale_reason(&st, exe) {
            return Err(anyhow!("the daemon came back stale: {r} · thc daemon restart"));
        }
        let was = have.as_deref().map(tilde).unwrap_or_else(|| "nothing".into());
        let e = Ensured { action: "restarted", manager: m, before, after: Some(st), reason: Some(format!("the login item ran {was}")) };
        let note = format!("login item now runs {} (was {was}) · {}", tilde(exe), e.line());
        (e.json(), Some(note))
    } else if status(&lp).is_none() {
        // Installed but stopped: start it through its manager.
        if m == Manager::Launchd {
            kickstart(false);
        } else {
            systemctl(&["start", "thc.service"])?;
        }
        let st = wait_live(&lp, 8).ok_or_else(|| anyhow!("{} didn't start the daemon · log: {}", m.name(), tilde(&lp.cache.join("daemon.log"))))?;
        let note = format!("daemon started · pid {} · thc {}", st["pid"], st["version"].as_str().unwrap_or("?"));
        (json!({ "action": "started", "manager": m.name(), "after": { "pid": st["pid"], "version": st["version"], "exe": st["exe"] }, "line": note }), Some(note))
    } else {
        let e = ensure(&lp, exe, false)?;
        let note = (e.action == "restarted").then(|| e.line());
        (e.json(), note)
    };
    if out.json {
        out.json(&json!({ "ok": true, "already": true, "install": install_info(), "daemon": daemon, "note": note }));
    } else {
        out.line(format!("{} already a login item ({})", out.green("●"), tilde(p)));
        if let Some(n) = note {
            out.line(format!("  {n}"));
        }
    }
    Ok(())
}

fn wait_status(paths: &Paths) -> Option<Value> {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Some(s) = status(paths) {
            return Some(s);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

/// True when no daemon answers for this vault (for the "alerts are paused" hint).
pub fn offline(paths: &Paths) -> bool {
    match proto::read_info(paths) {
        Some(info) => !proto::pid_alive(info.pid) || !proto::socket_path(paths).exists(),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("thc-stale-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("thc");
        std::fs::write(&p, "x").unwrap();
        p
    }

    #[test]
    fn a_daemon_is_stale_when_its_binary_was_replaced() {
        let p = file("replaced");
        let id = proto::exe_identity(&p).unwrap();
        let st = |exe: &Path, id: Option<&str>| json!({ "version": "1.0.0", "exe": exe, "exe_id": id, "started_ms": 0 });
        assert_eq!(stale_reason(&st(&p, Some(&id)), &p), None);
        // Renamed over (install.sh, thc update): same path, another file.
        let tmp = p.with_file_name(".new");
        std::fs::write(&tmp, "y").unwrap();
        std::fs::rename(&tmp, &p).unwrap();
        assert!(stale_reason(&st(&p, Some(&id)), &p).is_some_and(|r| r.contains("older copy")));
        // Another binary altogether.
        let other = file("other");
        assert!(stale_reason(&st(&other, proto::exe_identity(&other).as_deref()), &p).is_some_and(|r| r.contains("it runs")));
        // Linux's name for a replaced binary.
        let deleted = json!({ "version": "1.0.0", "exe": format!("{} (deleted)", p.display()) });
        assert!(stale_reason(&deleted, &p).is_some());
    }

    #[test]
    fn a_daemon_without_exe_id_is_judged_by_when_it_started() {
        // Daemons from before exe_id: the file changed after the daemon started → stale.
        let p = file("old");
        let now = chrono::Utc::now().timestamp_millis();
        let st = |started: i64| json!({ "version": "0.10.1", "exe": p, "started_ms": started });
        assert!(stale_reason(&st(now - 3_600_000), &p).is_some());
        assert_eq!(stale_reason(&st(now + 60_000), &p), None);
        // No started_ms either: uptime says when.
        let st = json!({ "version": "0.10.1", "exe": p, "uptime_s": 3600 });
        assert!(stale_reason(&st, &p).is_some());
    }
}
