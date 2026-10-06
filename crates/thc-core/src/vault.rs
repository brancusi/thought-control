//! A vault: the synced folder (log/, drop/, export/) plus this device's local cache.

use crate::error::usage;
use crate::event::{Actor, Event, FORMAT_VERSION, Op};
use crate::hlc::Hlc;
use crate::log::Log;
use crate::store::Store;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::fs::{self, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

pub const VAULT_MARKER: &str = "thc-vault.toml";
pub const PROJECT_CONFIG: &str = ".thc.toml";

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    vault: Option<String>,
    cache: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Paths {
    pub vault: PathBuf,
    pub cache: PathBuf,
}

/// Why a vault was chosen: shown by `thc doctor` and `thc daemon status`, so a person with a
/// repo `.thc.toml` and a global vault can tell which one a command is using.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VaultSource {
    /// `--vault <path>`.
    Flag,
    /// The `THC_VAULT` environment variable.
    Env,
    /// A `.thc.toml` (the path of the file).
    Project(PathBuf),
    /// Inside a vault directory (cwd at or below a vault marker).
    Inside,
    /// `vault = …` in the global config (the path of the file).
    Global(PathBuf),
    /// `thc vault use <name>` (device-local; the config file's path).
    Use(PathBuf),
}

impl VaultSource {
    pub fn kind(&self) -> &'static str {
        match self {
            VaultSource::Flag => "flag",
            VaultSource::Env => "env",
            VaultSource::Project(_) => "project",
            VaultSource::Inside => "inside",
            VaultSource::Global(_) => "global",
            VaultSource::Use(_) => "use",
        }
    }

    pub fn file(&self) -> Option<&Path> {
        match self {
            VaultSource::Project(p) | VaultSource::Global(p) | VaultSource::Use(p) => Some(p),
            _ => None,
        }
    }

    /// "from ~/code/x/.thc.toml", "from ~/.config/thought/config.toml", "from THC_VAULT".
    pub fn describe(&self) -> String {
        match self {
            VaultSource::Flag => "from --vault".into(),
            VaultSource::Env => "from THC_VAULT".into(),
            // The full path: a stray .thc.toml up the tree (/tmp) must be easy to find.
            VaultSource::Project(p) => format!("from {}", tilde(p)),
            VaultSource::Inside => "inside the vault folder".into(),
            VaultSource::Global(p) => format!("from {}", tilde(p)),
            VaultSource::Use(_) => "from thc vault use".into(),
        }
    }
}

/// `~/…` for paths under HOME.
pub fn tilde(p: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        if let Ok(rest) = p.strip_prefix(&home) {
            return if rest.as_os_str().is_empty() { "~".into() } else { format!("~/{}", rest.display()) };
        }
    }
    p.display().to_string()
}

/// The global config: `~/.config/thought/config.toml`.
pub fn global_config_path() -> Option<PathBuf> {
    // THC_CONFIG_DIR moves it with policy's config (tests; a portable setup).
    let p = match std::env::var_os("THC_CONFIG_DIR").filter(|d| !d.is_empty()) {
        Some(d) => Some(PathBuf::from(d).join("config.toml")),
        None => std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/thought/config.toml")),
    };
    p.inspect(|p| {
        crate::sandbox::check(p);
    })
}

/// The suggested place for a first vault (never a sync folder by default).
pub const DEFAULT_VAULT: &str = "~/thought";

impl Paths {
    /// Resolution: explicit/`THC_VAULT` > nearest `.thc.toml` upward from cwd > `~/.config/thought/config.toml`.
    pub fn resolve(explicit: Option<&Path>) -> Result<Paths> {
        Self::resolve_with_source(explicit).map(|(p, _)| p)
    }

    /// [`Paths::resolve`], plus why that vault was chosen. `explicit` comes from `--vault` or
    /// `THC_VAULT` (clap merges them); the env var is checked to tell the two apart.
    pub fn resolve_with_source(explicit: Option<&Path>) -> Result<(Paths, VaultSource)> {
        let env_cache = std::env::var_os("THC_CACHE_DIR").map(PathBuf::from);
        if let Some(v) = explicit {
            // A registered name (`--vault acme`, `THC_VAULT=acme`) unless it's a folder here.
            let vault = match by_name(v) {
                Some(p) => p,
                None => absolute(v)?,
            };
            let cache = env_cache.unwrap_or_else(|| default_cache(&vault));
            let from_env = std::env::var_os("THC_VAULT").is_some_and(|e| Path::new(&e) == v);
            return Ok((Paths { vault, cache }, if from_env { VaultSource::Env } else { VaultSource::Flag }));
        }
        let mut dir = std::env::current_dir()?;
        loop {
            let cfg_path = dir.join(PROJECT_CONFIG);
            if cfg_path.exists() {
                let cfg: ConfigFile = toml::from_str(&fs::read_to_string(&cfg_path)?)
                    .with_context(|| format!("parsing {}", cfg_path.display()))?;
                let mut vault = dir.join(cfg.vault.as_deref().unwrap_or("vault"));
                // `vault = "acme"`: a registered name, when it isn't a vault folder beside the file.
                if !vault.join(VAULT_MARKER).exists() {
                    if let Some(p) = cfg.vault.as_deref().and_then(|n| by_name(Path::new(n))) {
                        vault = p;
                    }
                }
                if !vault.join(VAULT_MARKER).exists() {
                    return Err(usage(format!(
                        "{} points at {}, which isn't a vault · remove that file, or `thc init {}`",
                        tilde(&cfg_path),
                        tilde(&vault),
                        tilde(&vault)
                    )));
                }
                let cache = env_cache
                    .or_else(|| cfg.cache.as_deref().map(|c| dir.join(c)))
                    .unwrap_or_else(|| default_cache(&vault));
                return Ok((Paths { vault, cache }, VaultSource::Project(cfg_path)));
            }
            if dir.join(VAULT_MARKER).exists() {
                let cache = env_cache.unwrap_or_else(|| default_cache(&dir));
                return Ok((Paths { vault: dir, cache }, VaultSource::Inside));
            }
            if !dir.pop() {
                break;
            }
        }
        if let Some(global) = global_config_path() {
            if global.exists() {
                let cfg: ConfigFile = toml::from_str(&fs::read_to_string(&global)?)
                    .with_context(|| format!("parsing {}", global.display()))?;
                let reg = crate::registry::Registry::load();
                // `thc vault use <name>`, then the home vault (or a pre-vaults `vault = …`).
                if let Some(e) = reg.current.as_deref().and_then(|c| reg.find(c)) {
                    let vault = e.path.clone();
                    let cache = env_cache.clone().unwrap_or_else(|| default_cache(&vault));
                    return Ok((Paths { vault, cache }, VaultSource::Use(global)));
                }
                if let Some(e) = reg.home_entry() {
                    let vault = e.path.clone();
                    let cache = env_cache.or(cfg.cache.map(|c| expand_home(&c))).unwrap_or_else(|| default_cache(&vault));
                    return Ok((Paths { vault, cache }, VaultSource::Global(global)));
                }
            }
        }
        Err(usage(format!(
            "no vault found: run `thc init {DEFAULT_VAULT} --global` to create one, or set THC_VAULT, or pass --vault <path>"
        )))
    }
}

/// A registered vault's folder for a bare name (`acme`), unless a folder of that name exists here.
fn by_name(v: &Path) -> Option<PathBuf> {
    let s = v.to_str()?;
    if !crate::registry::valid_name(s) || Path::new(s).join(VAULT_MARKER).exists() {
        return None;
    }
    crate::registry::Registry::load().find(s).map(|e| e.path.clone())
}

fn absolute(p: &Path) -> Result<PathBuf> {
    Ok(if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir()?.join(p) })
}

fn expand_home(p: &str) -> PathBuf {
    match (p.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(p),
    }
}

/// This device's cache folder for a vault (keyed by its path until V6 moves the socket).
pub fn default_cache(vault: &Path) -> PathBuf {
    let hash = {
        // FNV-1a: stable, dependency-free.
        let mut h: u64 = 0xcbf29ce484222325;
        for b in vault.to_string_lossy().bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        format!("{h:016x}")
    };
    let base = if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Caches/thought"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .map(|p| p.join("thought"))
    };
    base.unwrap_or_else(|| PathBuf::from(".thc-cache")).join(hash)
}

/// Create a vault at `vault` (and a project `.thc.toml` in `config_dir` when given).
pub fn init(vault: &Path, config_dir: Option<&Path>, cache_rel: Option<&str>) -> Result<()> {
    crate::sandbox::check(vault);
    if let Some(dir) = config_dir.filter(|d| !d.join(PROJECT_CONFIG).exists()) {
        crate::sandbox::check(&dir.join(PROJECT_CONFIG));
        if shared_dir(dir) {
            return Err(usage(format!(
                "won't write .thc.toml in {}: every command run anywhere under it would use this vault · \
                 run `thc init` in a project folder, or pass --global",
                dir.display()
            )));
        }
    }
    for d in ["log", "drop", "export"] {
        fs::create_dir_all(vault.join(d))?;
    }
    let marker = vault.join(VAULT_MARKER);
    if !marker.exists() {
        fs::write(&marker, format!("# thought-central vault\nformat = {FORMAT_VERSION}\n"))?;
    }
    if let Some(dir) = config_dir {
        let cfg = dir.join(PROJECT_CONFIG);
        if !cfg.exists() {
            let rel = pathdiff(vault, dir);
            let mut s = format!("vault = {:?}\n", rel);
            if let Some(c) = cache_rel {
                s.push_str(&format!("cache = {c:?}\n"));
            }
            fs::write(cfg, s)?;
        }
    }
    Ok(())
}

/// A folder a project config must never go in: the filesystem root, the shared temp folders and
/// the home folder (a stray /tmp/.thc.toml once captured every scratch command under /tmp).
fn shared_dir(dir: &Path) -> bool {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let d = canon(dir);
    let mut shared: Vec<PathBuf> = ["/", "/tmp", "/var/tmp", "/private/tmp", "/private/var/tmp"].iter().map(PathBuf::from).collect();
    shared.push(std::env::temp_dir());
    shared.extend(std::env::var_os("HOME").map(PathBuf::from));
    shared.iter().any(|s| canon(s) == d)
}

/// Make `vault` the default for this user: the home vault in the registry
/// (~/.config/thought/config.toml; other keys are kept). It's named from its identity, else
/// `personal` (vaults.md §1). Returns the config path.
pub fn set_global_vault(vault: &Path) -> Result<PathBuf> {
    use crate::registry::{Entry, Registry, ensure_identity, read_identity, valid_name};
    let mut reg = Registry::load();
    if reg.file.is_none() {
        let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME isn't set"))?;
        reg.file = Some(PathBuf::from(home).join(".config/thought/config.toml"));
    }
    let name = reg.by_path(vault).map(|e| e.name.clone()).or_else(|| read_identity(vault).name.filter(|n| valid_name(n))).unwrap_or_else(|| "personal".into());
    let id = ensure_identity(vault, &name).ok().and_then(|i| i.id);
    reg.vaults.retain(|e| e.name != name);
    reg.vaults.insert(0, Entry { name: name.clone(), path: vault.to_path_buf(), id });
    reg.home = Some(name);
    reg.save()
}

fn pathdiff(target: &Path, base: &Path) -> String {
    target.strip_prefix(base).map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| target.to_string_lossy().to_string())
}

pub struct Vault {
    pub paths: Paths,
    pub device: String,
    pub actor: Actor,
    pub via: String,
    pub store: Store,
    pub log: Log,
    pub warnings: Vec<String>,
    /// `--readonly` / `THC_READONLY` (policy.md §2.2): every write is refused, whoever writes.
    pub readonly: Option<&'static str>,
    /// When this is a scratch copy (TUI snapshots), the vault it was copied from: the daemon
    /// socket is looked up there.
    pub origin: Option<Paths>,
    /// "from /path/.thc.toml" when a project config chose this vault (the TUI says so on open).
    pub source_note: Option<String>,
    /// Events newly applied to the store since the last `take_news`, by catch-up, rebuild or
    /// this vault's own writes. For views that show "what changed": no watermark works, since
    /// another device's events arrive with older times and a rebuild renumbers every row
    /// (two-device soak).
    news: News,
    /// The store's last event row this vault has accounted for: rows past it were applied by
    /// another process sharing the store (the CLI beside the daemon), which this vault's
    /// catch-up never sees.
    seen_row: i64,
}

/// What `Vault::take_news` hands back.
#[derive(Debug, Default, Clone)]
pub struct News {
    /// (event id, written by this vault) in the order applied.
    pub events: Vec<(String, bool)>,
    /// More arrived than is kept (a first sync, a long time away): treat everything as changed.
    pub overflow: bool,
}

impl News {
    const CAP: usize = 20_000;

    fn push(&mut self, eid: &str, own: bool) {
        if self.overflow {
            return;
        }
        if self.events.len() >= Self::CAP {
            self.events.clear();
            self.overflow = true;
            return;
        }
        self.events.push((eid.to_string(), own));
    }

    /// Anything from elsewhere (another device, another process here)?
    pub fn foreign(&self) -> bool {
        self.overflow || self.events.iter().any(|(_, own)| !own)
    }
}

/// `BEGIN IMMEDIATE` so a writer waits on the busy timeout instead of failing to upgrade
/// a read transaction (SQLITE_BUSY) when another process commits first.
struct Immediate<'a> {
    conn: &'a rusqlite::Connection,
    done: bool,
}

impl<'a> Immediate<'a> {
    fn begin(conn: &'a rusqlite::Connection) -> Result<Immediate<'a>> {
        conn.execute_batch("BEGIN IMMEDIATE")?;
        Ok(Immediate { conn, done: false })
    }

    fn commit(mut self) -> Result<()> {
        self.conn.execute_batch("COMMIT")?;
        self.done = true;
        Ok(())
    }
}

impl Drop for Immediate<'_> {
    fn drop(&mut self) {
        if !self.done {
            let _ = self.conn.execute_batch("ROLLBACK");
        }
    }
}

/// Recorded in the store's meta: the format version of the reader that built it.
const READER: &str = "2";
const _: () = assert!(FORMAT_VERSION == 2, "bump READER with FORMAT_VERSION");

struct WriteLock(File);

impl WriteLock {
    fn acquire(path: &Path) -> Result<WriteLock> {
        let f = OpenOptions::new().create(true).truncate(false).write(true).open(path)?;
        // SAFETY: flock on a valid fd; released when the file is dropped.
        let rc = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) };
        if rc != 0 {
            bail!("could not lock {}", path.display());
        }
        Ok(WriteLock(f))
    }
}

impl Drop for WriteLock {
    fn drop(&mut self) {
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

impl Vault {
    pub fn open(paths: Paths, actor: Actor, via: &str) -> Result<Vault> {
        crate::sandbox::check(&paths.vault);
        crate::sandbox::check(&paths.cache);
        if !paths.vault.join(VAULT_MARKER).exists() {
            bail!(usage(format!("{} is not a vault (missing {VAULT_MARKER}); run `thc init`", paths.vault.display())));
        }
        // A vault from before vaults gets its identity on first open (vaults-architecture §1).
        if crate::policy::readonly_source(false).is_none() && crate::registry::read_identity(&paths.vault).id.is_none() {
            let name = crate::registry::Registry::load().name_for(&paths.vault);
            let _ = crate::registry::ensure_identity(&paths.vault, &name);
        }
        fs::create_dir_all(&paths.cache)?;
        let device = device_id(&paths.cache)?;
        let store = Store::open(&paths.cache.join("store.db"))?;
        let log = Log::new(paths.vault.join("log"));
        let readonly = crate::policy::readonly_source(false);
        let mut v = Vault { paths, device, actor, via: via.to_string(), store, log, warnings: vec![], readonly, origin: None, source_note: None, news: News::default(), seen_row: 0 };
        // What the store holds at open is the starting state, not news: start past it. (From 0,
        // the first `take_news` read every event ever applied: 40 ms at 6,000, on every TUI
        // open, and growing with the vault.)
        v.seen_row = v.max_event_row();
        // A store built by an older reader may have skipped ops it didn't know: replay once.
        if v.store.meta("reader")?.as_deref() != Some(READER) {
            let _lock = v.lock()?;
            let has_events = v.store.conn.query_row("SELECT EXISTS(SELECT 1 FROM events)", [], |r| r.get::<_, bool>(0))?;
            if has_events {
                v.rebuild_locked()?;
            } else {
                v.store.set_meta("reader", READER)?;
            }
        }
        v.catch_up()?;
        // A store from before re-homing: one full pass (cheap: an indexed join over deleted parents).
        if v.store.meta("rehome")?.is_none() {
            let _lock = v.lock()?;
            v.store.rehome_all()?;
        }
        v.store.ensure_stats();
        Ok(v)
    }

    /// Where the daemon for this vault listens: the original vault's, for a scratch copy.
    pub fn daemon_paths(&self) -> &Paths {
        self.origin.as_ref().unwrap_or(&self.paths)
    }

    fn lock(&self) -> Result<WriteLock> {
        WriteLock::acquire(&self.paths.cache.join("write.lock"))
    }

    /// Apply new complete lines from every device's log. Falls back to a full rebuild when an
    /// event arrives that sorts before what is already applied (late sync delivery).
    /// Takes the device write lock: every store writer on this device is serialized.
    pub fn catch_up(&mut self) -> Result<usize> {
        // Fast path: nothing new on disk, no lock needed.
        let mut changed = false;
        for (rel, size) in self.log.files()? {
            if self.store.cursor(&rel)? != size {
                changed = true;
                break;
            }
        }
        if !changed {
            return Ok(0);
        }
        let _lock = self.lock()?;
        self.catch_up_locked()
    }

    fn catch_up_locked(&mut self) -> Result<usize> {
        let mut chunks = Vec::new();
        let mut needs_rebuild = false;
        for (rel, size) in self.log.files()? {
            let cur = self.store.cursor(&rel)?;
            if size < cur {
                needs_rebuild = true;
                break;
            }
            if size > cur {
                chunks.push(self.log.read_from(&rel, cur)?);
            }
        }
        if needs_rebuild {
            return self.rebuild_locked();
        }
        let mut events: Vec<Event> = Vec::new();
        for c in &chunks {
            self.warnings.extend(c.bad_lines.iter().cloned());
            events.extend(c.events.iter().cloned());
        }
        if events.is_empty() {
            let tx = Immediate::begin(&self.store.conn)?;
            for c in &chunks {
                self.store.set_cursor(&c.rel, c.new_offset)?;
            }
            tx.commit()?;
            return Ok(0);
        }
        events.sort_by_key(|e| e.order_key());
        if let Some(max) = self.store.max_okey()? {
            if events.iter().any(|e| e.order_key() < max && !self.store.has_event(&e.eid).unwrap_or(false)) {
                return self.rebuild_locked();
            }
        }
        let tx = Immediate::begin(&self.store.conn)?;
        let mut fresh = Vec::new();
        for e in &events {
            if !self.store.has_event(&e.eid)? {
                fresh.push(e.eid.as_str());
            }
            self.store.apply(e)?;
        }
        let cands = self.store.rehome_candidates(&events);
        self.store.rehome(&cands)?;
        for c in &chunks {
            self.store.set_cursor(&c.rel, c.new_offset)?;
        }
        tx.commit()?;
        for eid in fresh {
            self.news.push(eid, false);
        }
        // A big batch (a sync after days away) can change table sizes enough to matter.
        if events.len() >= 500 {
            self.store.refresh_stats();
        }
        Ok(events.len())
    }

    /// The events applied since the last call (see `News`), and forget them. Includes what other
    /// processes applied to the shared store. (After another process's rebuild the rows are
    /// renumbered: some old events may count as news again, which only repeats a push.)
    pub fn take_news(&mut self) -> News {
        let mut news = std::mem::take(&mut self.news);
        let max = self.max_event_row();
        if max > self.seen_row && !news.overflow {
            let known: std::collections::HashSet<String> = news.events.iter().map(|(e, _)| e.clone()).collect();
            if let Ok(mut st) = self.store.conn.prepare("SELECT eid FROM events WHERE rowid > ?1 ORDER BY rowid") {
                let rows: Vec<String> = st.query_map([self.seen_row], |r| r.get(0)).map(|it| it.flatten().collect()).unwrap_or_default();
                for eid in rows {
                    if !known.contains(&eid) {
                        news.push(&eid, false);
                    }
                }
            }
        }
        self.seen_row = max;
        news
    }

    fn max_event_row(&self) -> i64 {
        self.store.conn.query_row("SELECT coalesce(max(rowid), 0) FROM events", [], |r| r.get(0)).unwrap_or(0)
    }

    /// Drop the materialized view and replay every log line in total order.
    pub fn rebuild(&mut self) -> Result<usize> {
        let _lock = self.lock()?;
        self.rebuild_locked()
    }

    fn rebuild_locked(&mut self) -> Result<usize> {
        let mut events = Vec::new();
        let mut cursors = Vec::new();
        for (rel, _) in self.log.files()? {
            let c = self.log.read_from(&rel, 0)?;
            self.warnings.extend(c.bad_lines);
            cursors.push((rel, c.new_offset));
            events.extend(c.events);
        }
        events.sort_by_key(|e| e.order_key());
        // What the store didn't have before: news (read before the reset forgets it).
        let fresh: Vec<&str> = events.iter().filter(|e| !self.store.has_event(&e.eid).unwrap_or(false)).map(|e| e.eid.as_str()).collect();
        let tx = Immediate::begin(&self.store.conn)?;
        self.store.reset()?;
        for e in &events {
            self.store.apply(e)?;
        }
        for (rel, off) in cursors {
            self.store.set_cursor(&rel, off)?;
        }
        self.store.rehome_all()?;
        self.store.set_meta("reader", READER)?;
        tx.commit()?;
        for eid in fresh {
            self.news.push(eid, false);
        }
        self.seen_row = self.max_event_row();
        self.store.analyze();
        Ok(events.len())
    }

    /// Stamp, append (fsync) and apply a transaction of ops. `build` sees the caught-up store.
    pub fn transact<T>(&mut self, build: impl FnOnce(&Store) -> Result<(Vec<Op>, T)>) -> Result<(Vec<Event>, T)> {
        if let Some(src) = self.readonly {
            return Err(crate::error::ThcError::Refused {
                kind: "readonly",
                message: format!("read-only ({src}) · nothing written"),
                hint: String::new(),
                actor: self.actor.label(),
                tier: String::new(),
                verb: String::new(),
                config: None,
            }
            .into());
        }
        let _lock = self.lock()?;
        self.catch_up_locked()?;
        let (ops, out) = build(&self.store)?;
        if ops.is_empty() {
            return Ok((vec![], out));
        }
        let events = self.stamp(ops)?;
        self.commit(&events)?;
        Ok((events, out))
    }

    pub fn stamp(&self, mut ops: Vec<Op>) -> Result<Vec<Event>> {
        // Team attribution is writer input, never replay state. Only newly created nodes
        // receive it; edits keep their attribution in the event log without extra node props.
        if self.actor.kind == "agent" {
            let role = std::env::var("THC_ROLE").ok().filter(|s| !s.is_empty());
            let session = std::env::var("THC_SESSION_ID").ok().filter(|s| !s.is_empty());
            for op in &mut ops {
                if let Op::NodeCreate { props, .. } = op {
                    if let Some(role) = &role { props.insert("agent_role".into(), serde_json::json!(role)); }
                    if let Some(session) = &session { props.insert("session_id".into(), serde_json::json!(session)); }
                }
            }
        }
        let mut hlc = self.store.max_hlc()?;
        // Fixtures (THC_FIXTURE_IDS=1 with THC_NOW): the pinned clock and ids derived from it and
        // the device, so a seeded vault is the same on every run (scripts/guide-renders.sh).
        let fixed = fixture_clock();
        // Derived ids belong to fixtures only: without a pinned clock (or the test sandbox) a
        // leftover THC_FIXTURE_IDS refuses to write rather than guess.
        if fixed.is_none() && std::env::var("THC_FIXTURE_IDS").is_ok_and(|v| !v.is_empty()) && std::env::var_os("THC_TEST").is_none() {
            anyhow::bail!("THC_FIXTURE_IDS is set without THC_NOW: it is for fixtures only · unset THC_FIXTURE_IDS");
        }
        let tick = |h: Hlc| match fixed {
            Some(ms) => Hlc::tick_at(h, ms),
            None => Hlc::tick(h),
        };
        let derived = |h: Hlc, salt: &str| {
            // FNV-1a: stable across builds, unlike std's hasher.
            let mut x: u64 = 0xcbf29ce484222325;
            for b in format!("{}|{}|{}|{salt}", self.device, h.0, h.1).bytes() {
                x ^= b as u64;
                x = x.wrapping_mul(0x100000001b3);
            }
            ulid::Ulid::from_parts(h.0, x as u128).to_string()
        };
        let tx = match fixed {
            Some(_) => derived(tick(hlc), "tx"),
            None => ulid::Ulid::new().to_string(),
        };
        let mut events = Vec::with_capacity(ops.len());
        for op in ops {
            hlc = tick(hlc);
            events.push(Event {
                v: op.min_version(),
                eid: if fixed.is_some() { derived(hlc, "eid") } else { ulid::Ulid::new().to_string() },
                hlc,
                dev: self.device.clone(),
                actor: self.actor.clone(),
                via: self.via.clone(),
                tx: tx.clone(),
                op,
            });
        }
        Ok(events)
    }

    fn commit(&mut self, events: &[Event]) -> Result<()> {
        let written = self.log.append(&self.device, events)?;
        let tx = Immediate::begin(&self.store.conn)?;
        for e in events {
            self.store.apply(e)?;
        }
        let cands = self.store.rehome_candidates(events);
        self.store.rehome(&cands)?;
        for e in events {
            self.news.push(&e.eid, true);
        }
        for (rel, before, after) in written {
            // Only advance if nothing unread sat before our append (always true for our own files).
            if self.store.cursor(&rel)? == before {
                self.store.set_cursor(&rel, after)?;
            }
        }
        tx.commit()?;
        // Imports and apply batches grow the cache through commit, not catch_up_locked.
        if events.len() >= 500 {
            self.store.refresh_stats();
        }
        Ok(())
    }

    /// A throwaway store replayed up to `ms` (inclusive), for `--as-of` queries.
    pub fn as_of(&self, ms: u64) -> Result<Store> {
        let mem = Store::open_memory()?;
        let bodies: Vec<String> = {
            let mut st = self.store.conn.prepare("SELECT body FROM events WHERE ms <= ?1 ORDER BY okey")?;
            st.query_map([ms as i64], |r| r.get(0))?.collect::<Result<_, _>>()?
        };
        let tx = mem.conn.unchecked_transaction()?;
        for b in bodies {
            let e: Event = serde_json::from_str(&b)?;
            mem.apply(&e)?;
        }
        tx.commit()?;
        Ok(mem)
    }
}

/// THC_FIXTURE_IDS=1 with THC_NOW: the pinned time in ms, for `stamp`. Never set in real use.
fn fixture_clock() -> Option<u64> {
    if std::env::var("THC_FIXTURE_IDS").ok().as_deref() != Some("1") {
        return None;
    }
    let v = std::env::var("THC_NOW").ok()?;
    let n = chrono::NaiveDateTime::parse_from_str(&v, "%Y-%m-%dT%H:%M").ok()?;
    use chrono::TimeZone;
    chrono::Local.from_local_datetime(&n).earliest().map(|t| t.timestamp_millis().max(0) as u64)
}

fn device_id(cache: &Path) -> Result<String> {
    if let Ok(d) = std::env::var("THC_DEVICE") {
        if !d.is_empty() {
            return Ok(d);
        }
    }
    let path = cache.join("device");
    if let Ok(s) = fs::read_to_string(&path) {
        let s = s.trim().to_string();
        if !s.is_empty() {
            return Ok(s);
        }
    }
    let host = hostname().unwrap_or_else(|| "device".into());
    let short: String = host
        .split('.')
        .next()
        .unwrap_or("device")
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(24)
        .collect();
    let id = format!("{short}-{}", &crate::id::new_id()[..4]);
    fs::write(&path, &id)?;
    Ok(id)
}

fn hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: buffer is valid for its length; gethostname NUL-terminates on success.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if rc != 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    Some(String::from_utf8_lossy(&buf[..end]).to_string())
}

/// A throwaway copy of a vault and its cache under the system temp dir, for TUI snapshots
/// that replay keys: writes land in the copy, never the real vault. On APFS `fs::copy`
/// clones, so even a large store copies in milliseconds. Returns the copy's paths.
pub fn scratch_copy(paths: &Paths) -> Result<Paths> {
    let root = std::env::temp_dir().join(format!("thc-snapshot-{}-{}", std::process::id(), crate::id::new_id()));
    fn copy_dir(from: &Path, to: &Path) -> Result<()> {
        fs::create_dir_all(to)?;
        for e in fs::read_dir(from)? {
            let e = e?;
            let t = e.file_type()?;
            let dest = to.join(e.file_name());
            if t.is_dir() {
                copy_dir(&e.path(), &dest)?;
            } else if t.is_file() {
                fs::copy(e.path(), dest)?;
            }
            // Sockets and other special files are skipped.
        }
        Ok(())
    }
    let copy = Paths { vault: root.join("vault"), cache: root.join("cache") };
    copy_dir(&paths.vault, &copy.vault)?;
    if paths.cache.exists() {
        copy_dir(&paths.cache, &copy.cache)?;
    }
    Ok(copy)
}
