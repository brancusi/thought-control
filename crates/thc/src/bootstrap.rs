//! Machine setup (docs/design/setup.md): the vault, the `thc` command on PATH, the daemon's login
//! item and the agent skills, each recorded in `~/.config/thought/setup.json` so `--undo` removes
//! exactly what setup added and nothing else. The vault is never removed.
//!
//! Every step reports one line of setup.md §2.2 copy (done, already in place, or failed with thc's
//! own error), so the Mac app's first-launch window, `install.sh` and the first run of `thc` show
//! the same thing. The app runs single steps (`thc setup --step path --json`) to tick its lines.

use crate::out::Out;
use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use thc_core::vault::{self, tilde};

const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The marker on the PATH line setup adds to a shell profile (removed by `--undo`).
pub const PROFILE_MARK: &str = "# added by Thought Central (remove with: thc setup --undo)";
pub const STEPS: [&str; 4] = ["vault", "path", "login", "agents"];

// ---- the record ------------------------------------------------------------------------------

/// One thing setup did. `--undo` reverses each, only if it's still what setup left there.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Item {
    /// vault | link | profile | agent | login
    pub piece: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// link: what it points at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<PathBuf>,
    /// agent: claude | codex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// agent: file | block · login: app | launchd | systemd.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Setup created the file (so undo may remove it once it's empty again).
    #[serde(default)]
    pub created: bool,
    pub at: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Record {
    #[serde(default)]
    pub version: String,
    /// app | cli
    #[serde(default)]
    pub by: String,
    #[serde(default)]
    pub items: Vec<Item>,
}

fn now() -> String {
    thc_core::dates::now_local().format("%Y-%m-%dT%H:%M:%S").to_string()
}

impl Record {
    pub fn path(env: &Env) -> PathBuf {
        env.home.join(".config/thought/setup.json")
    }

    pub fn load(env: &Env) -> Record {
        std::fs::read_to_string(Self::path(env)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    fn save(&mut self, env: &Env) -> Result<()> {
        self.version = VERSION.into();
        if self.by.is_empty() {
            self.by = env.by.clone();
        }
        let p = Self::path(env);
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(&p, serde_json::to_string_pretty(self)? + "\n")?;
        Ok(())
    }

    /// Replace the item for the same piece and path (a re-run doesn't duplicate it).
    fn put(&mut self, item: Item) {
        self.items.retain(|i| !(i.piece == item.piece && i.path == item.path && i.agent == item.agent));
        self.items.push(item);
    }
}

// ---- the environment -------------------------------------------------------------------------

/// What setup reads from the process: HOME, the login shell and this binary.
pub struct Env {
    pub home: PathBuf,
    pub shell: PathBuf,
    /// The binary the `thc` link points at (this one, normally the app's bundled copy).
    pub exe: PathBuf,
    /// app | cli, for the record.
    pub by: String,
}

impl Env {
    pub fn from_process() -> Result<Env> {
        let home = std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| anyhow!("HOME isn't set"))?;
        thc_core::sandbox::check(&home);
        let shell = std::env::var_os("SHELL").map(PathBuf::from).filter(|s| s.is_absolute()).unwrap_or_else(|| PathBuf::from("/bin/zsh"));
        let exe = std::env::current_exe()?.canonicalize()?;
        let by = if std::env::var("THC_SETUP_BY").is_ok_and(|v| v == "app") { "app" } else { "cli" }.to_string();
        Ok(Env { home, shell, exe, by })
    }

    fn local_bin(&self) -> PathBuf {
        self.home.join(".local/bin")
    }

    fn shell_name(&self) -> String {
        self.shell.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
    }

    /// The Thought Central app to own the login item: the one this binary is inside, else an
    /// installed copy. `THC_SETUP_APP=none` (tests, CLI-only machines) means there isn't one.
    pub fn app(&self) -> Option<PathBuf> {
        match std::env::var("THC_SETUP_APP") {
            Ok(v) if v == "none" => return None,
            Ok(v) if !v.is_empty() => return Some(PathBuf::from(v)),
            _ => {}
        }
        // Only the app's own thc defers to the app. A standalone thc (install.sh) owns its login
        // item even while an old app is still in /Applications.
        self.exe.ancestors().find(|a| a.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
    }
}

// ---- steps -----------------------------------------------------------------------------------

/// One checklist line (setup.md §2.2).
#[derive(Debug, Serialize)]
pub struct Step {
    pub step: &'static str,
    /// done | already | failed | skipped | none
    pub state: &'static str,
    pub text: String,
    /// Extra muted text after the line (`open a new terminal window to use it`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// thc's own error, verbatim (failed only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The terminal checklist's terser form (setup.md §3.1): `~/thought (new)`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short: Option<String>,
}

impl Step {
    fn new(step: &'static str, state: &'static str, text: impl Into<String>) -> Step {
        Step { step, state, text: text.into(), detail: None, error: None, path: None, short: None }
    }

    fn short(mut self, s: impl Into<String>) -> Step {
        self.short = Some(s.into());
        self
    }

    fn failed(step: &'static str, text: impl Into<String>, err: impl std::fmt::Display) -> Step {
        Step { error: Some(format!("{err:#}")), ..Step::new(step, "failed", text) }
    }
}

pub fn run_step(name: &str, env: &Env, rec: &mut Record, agents: Option<&[String]>) -> Step {
    match name {
        "vault" => vault_step(env, rec),
        "path" => path_step(env, rec),
        "login" => login_step(env, rec),
        "agents" => agents_step(env, rec, agents),
        _ => Step::failed("vault", format!("unknown step {name}"), "steps are vault, path, login, agents"),
    }
}

/// The global vault (config.toml or THC_VAULT), if one is configured. A repo's .thc.toml doesn't
/// count: setup is about the machine.
fn configured_vault(env: &Env) -> Option<PathBuf> {
    if let Some(v) = std::env::var_os("THC_VAULT") {
        return Some(PathBuf::from(v));
    }
    let cfg = std::fs::read_to_string(env.home.join(".config/thought/config.toml")).ok()?;
    // The registry's home vault (or a pre-vaults `vault = …`, which reads the same way).
    let reg = thc_core::registry::Registry::parse(&cfg);
    let p = reg.home_entry()?.path.clone();
    Some(match p.strip_prefix("~") {
        Ok(rest) if p.to_string_lossy().starts_with("~/") => env.home.join(rest),
        _ => p,
    })
}

fn vault_step(env: &Env, rec: &mut Record) -> Step {
    let s = vault_step_inner(env, rec);
    if s.state != "failed" {
        ensure_tui_block(env);
    }
    s
}

/// Settings are config-driven and visible: config.toml gets the commented `[tui]` and
/// `[tui.focus]` blocks (thc_core::tui_config) it doesn't have yet. Existing settings are
/// never touched.
fn ensure_tui_block(env: &Env) {
    let p = env.home.join(".config/thought/config.toml");
    let have = std::fs::read_to_string(&p).unwrap_or_default();
    let add = thc_core::tui_config::missing_blocks(&have);
    if add.is_empty() {
        return;
    }
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let sep = if have.is_empty() || have.ends_with('\n') { "" } else { "\n" };
    let _ = std::fs::write(&p, format!("{have}{sep}{add}"));
}

fn vault_step_inner(env: &Env, rec: &mut Record) -> Step {
    if let Some(v) = configured_vault(env) {
        let mut s = Step::new("vault", "already", format!("Using your vault in {}", tilde(&v))).short(tilde(&v));
        s.path = Some(v.display().to_string());
        return s;
    }
    let v = env.home.join("thought");
    let shown = tilde(&v);
    let is_vault = v.join(vault::VAULT_MARKER).exists();
    if !is_vault && v.exists() {
        let empty = std::fs::read_dir(&v).map(|d| d.filter_map(|e| e.ok()).all(|e| e.file_name().to_string_lossy().starts_with('.'))).unwrap_or(false);
        if !empty {
            return Step::failed("vault", format!("Couldn't create {shown}"), anyhow!("{shown} already exists and isn't a vault · thc init <folder> --global to use another folder"));
        }
    }
    let r = (|| -> Result<()> {
        vault::init(&v, None, None)?;
        // set_global_vault reads HOME from the environment; Env.home came from it too.
        vault::set_global_vault(&v)?;
        Ok(())
    })();
    match r {
        Err(e) => Step::failed("vault", format!("Couldn't create {shown}"), e),
        Ok(()) => {
            rec.put(Item { piece: "vault".into(), path: Some(v.clone()), target: None, agent: None, kind: None, created: !is_vault, at: now() });
            let mut s = if is_vault {
                Step::new("vault", "already", format!("Using your vault in {shown}")).short(shown.clone())
            } else {
                Step::new("vault", "done", format!("Your notes live in {shown}")).short(format!("{shown} (new)"))
            };
            s.path = Some(v.display().to_string());
            s
        }
    }
}

/// Run the user's login shell (interactive, so ~/.zshrc counts too) and ask where `thc` is and
/// what PATH is. Nil on a shell that hangs or fails (5 s).
pub fn login_shell(env: &Env) -> Option<(Option<PathBuf>, Vec<PathBuf>)> {
    let script = "command -v thc 2>/dev/null; echo \"__PATH__$PATH\"";
    let mut child = Command::new(&env.shell)
        .args(if env.shell_name() == "fish" { vec!["-l", "-i", "-c", script] } else { vec!["-l", "-i", "-c", script] })
        .env("HOME", &env.home)
        .env_remove("PATH") // what a new terminal window gets, not this process's PATH
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut thc = None;
    let mut path = vec![];
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("__PATH__") {
            path = p.split(':').filter(|s| !s.is_empty()).map(PathBuf::from).collect();
        } else if line.starts_with('/') && line.ends_with("/thc") {
            thc = Some(PathBuf::from(line.trim()));
        }
    }
    Some((thc, path))
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// A link Thought Central (or this setup) made: it points into some app's bundled thc.
fn is_our_link(p: &Path, target: &Path) -> bool {
    std::fs::read_link(p).is_ok_and(|d| d == target || d.to_string_lossy().ends_with(".app/Contents/Helpers/thc"))
}

/// `thc --version` of another copy ("0.2.1"), within 3 s.
fn version_of(p: &Path) -> Option<String> {
    let out = Command::new(p).arg("--version").stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    String::from_utf8_lossy(&out.stdout).split_whitespace().last().map(str::to_string)
}

fn older(a: &str, b: &str) -> bool {
    let parse = |s: &str| s.split(['.', '-']).take(3).map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    parse(a) < parse(b)
}

/// The login profile for this shell, and the PATH line in its syntax (setup.md §2.3).
fn profile_for(env: &Env) -> (PathBuf, String) {
    let sh_line = format!("{PROFILE_MARK}\nexport PATH=\"$HOME/.local/bin:$PATH\"\n");
    match env.shell_name().as_str() {
        "zsh" => (env.home.join(".zprofile"), sh_line),
        "fish" => (env.home.join(".config/fish/conf.d/thought-central.fish"), format!("{PROFILE_MARK}\nset -gx PATH $HOME/.local/bin $PATH\n")),
        "bash" => {
            // bash reads the first of these; writing .bash_profile would hide an existing .profile.
            let p = [".bash_profile", ".bash_login", ".profile"].iter().map(|f| env.home.join(f)).find(|p| p.exists()).unwrap_or_else(|| env.home.join(".bash_profile"));
            (p, sh_line)
        }
        _ => (env.home.join(".profile"), sh_line),
    }
}

fn path_step(env: &Env, rec: &mut Record) -> Step {
    let target = env.exe.clone();
    let link = env.local_bin().join("thc");
    let shell = login_shell(env);
    if let Some((Some(found), _)) = &shell {
        if same_file(found, &target) {
            let mut s = Step::new("path", "already", "thc already works in your terminal").short(format!("{} (already on PATH)", tilde(found)));
            s.path = Some(found.display().to_string());
            return s;
        }
        // Another thc comes first on PATH (Homebrew, cargo): never shadow or replace it.
        if !is_our_link(found, &target) {
            let v = version_of(found);
            let mut text = format!("thc already works in your terminal ({}{})", tilde(found), v.as_ref().map(|v| format!(" {v}")).unwrap_or_default());
            if v.as_deref().is_some_and(|v| older(v, VERSION)) {
                text.push_str(&format!(" · older than Thought Central's {VERSION}"));
            }
            let short = text.trim_start_matches("thc already works in your terminal (").replacen(')', "", 1);
            let mut s = Step::new("path", "already", text).short(format!("{short} · already on PATH, left as it is"));
            s.path = Some(found.display().to_string());
            return s;
        }
    }
    // install.sh puts the binary itself at ~/.local/bin/thc: nothing to link, only PATH.
    let standalone = std::fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_file()) && same_file(&link, &target);
    let r = (|| -> Result<Option<Item>> {
        std::fs::create_dir_all(env.local_bin())?;
        if standalone {
            // fall through to the PATH check below
        } else if std::fs::symlink_metadata(&link).is_ok() {
            if !is_our_link(&link, &target) {
                return Err(anyhow!("{} is another program, not a link to Thought Central · remove it first", tilde(&link)));
            }
            std::fs::remove_file(&link)?;
        }
        if !standalone {
            std::os::unix::fs::symlink(&target, &link).with_context(|| format!("linking {}", tilde(&link)))?;
        }
        let on_path = shell.as_ref().is_some_and(|(_, dirs)| dirs.iter().any(|d| same_file(d, &env.local_bin()) || *d == env.local_bin()));
        if on_path {
            return Ok(None);
        }
        let (profile, line) = profile_for(env);
        let existing = std::fs::read_to_string(&profile).unwrap_or_default();
        let created = !profile.exists();
        if !existing.contains(PROFILE_MARK) {
            if let Some(dir) = profile.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let sep = if existing.is_empty() || existing.ends_with('\n') { "" } else { "\n" };
            std::fs::write(&profile, format!("{existing}{sep}{line}")).with_context(|| format!("writing {}", tilde(&profile)))?;
        }
        Ok(Some(Item { piece: "profile".into(), path: Some(profile), target: None, agent: None, kind: None, created, at: now() }))
    })();
    let profile = match r {
        Err(e) => return Step::failed("path", "Couldn't add the thc command", e),
        Ok(p) => p,
    };
    if !standalone {
        rec.put(Item { piece: "link".into(), path: Some(link.clone()), target: Some(target.clone()), agent: None, kind: None, created: true, at: now() });
    }
    if let Some(p) = &profile {
        rec.put(p.clone());
    }
    // The line tells the truth: a fresh login shell must find this link.
    match login_shell(env) {
        Some((Some(found), _)) if same_file(&found, &link) || same_file(&found, &target) => {
            // The new-window reminder is in the closing block.
            let mut s = Step::new("path", "done", "thc command ready").short(tilde(&link));
            s.detail = Some("open a new terminal window to use it".into());
            s.path = Some(link.display().to_string());
            s
        }
        Some((found, _)) => Step::failed(
            "path",
            "Couldn't add the thc command",
            anyhow!(
                "linked {} but a new {} login shell finds {}",
                tilde(&link),
                env.shell_name(),
                found.map(|f| tilde(&f)).unwrap_or_else(|| "no thc".into())
            ),
        ),
        None => {
            // A shell we can't ask (it hung or failed): the link and the profile line are in place.
            let mut s = Step::new("path", "done", "thc command ready").short(tilde(&link));
            s.detail = Some(format!("open a new terminal window to use it · couldn't check with {}", env.shell_name()));
            s.path = Some(link.display().to_string());
            s
        }
    }
}

/// `THC_SETUP_LOGIN=skip` turns the login step off (tests: never touch launchd for real).
fn login_skipped() -> bool {
    std::env::var("THC_SETUP_LOGIN").is_ok_and(|v| v == "skip")
}

fn login_step(env: &Env, rec: &mut Record) -> Step {
    if login_skipped() {
        return Step::new("login", "skipped", "Login item skipped (THC_SETUP_LOGIN=skip)").short("not set up (THC_SETUP_LOGIN=skip)");
    }
    // The app owns the login item whenever it's installed (SMAppService); it migrates a
    // LaunchAgent made by `thc daemon install`. Ask it, headless.
    if let Some(app) = env.app() {
        let exe = app.join("Contents/MacOS/ThoughtBar");
        // Registered before (recorded): a re-check is "already in place", not new work.
        let had = rec.items.iter().any(|i| i.piece == "login" && i.kind.as_deref() == Some("app"));
        return match Command::new(&exe).arg("--register-login-item").stdin(Stdio::null()).output() {
            Ok(o) if o.status.success() => {
                rec.put(Item { piece: "login".into(), path: Some(app.clone()), target: None, agent: None, kind: Some("app".into()), created: true, at: now() });
                let said = String::from_utf8_lossy(&o.stdout).trim().to_string();
                let mut s = Step::new("login", if had { "already" } else { "done" }, "Running in the background · starts at login").short("running · starts at login");
                if !said.is_empty() && said != "starts at login" {
                    s.detail = Some(said);
                }
                s
            }
            Ok(o) => Step::failed("login", "Couldn't start thc", anyhow!("{}", String::from_utf8_lossy(&o.stderr).trim())),
            Err(e) => Step::failed("login", "Couldn't start thc", anyhow!("can't run {}: {e}", exe.display())),
        };
    }
    // CLI-only: thc's own LaunchAgent (or systemd unit), which also starts the daemon now.
    let was = crate::daemon_cmd::install_info();
    // Installed before, `daemon install` also points the login item at this binary and restarts
    // a daemon still running the one this install replaced (through launchd or systemd), so a
    // reinstall or update never leaves the old version running.
    let o = Command::new(&env.exe).args(["--json", "daemon", "install"]).current_dir("/").stdin(Stdio::null()).output();
    match o {
        Ok(o) if o.status.success() => {
            let said: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_default();
            let info = crate::daemon_cmd::install_info();
            let kind = info["kind"].as_str().unwrap_or("none").to_string();
            let path = info["path"].as_str().map(PathBuf::from);
            // Installed before: make sure it's running too (a stopped daemon isn't "already").
            let _ = Command::new(&env.exe).args(["daemon", "start"]).current_dir("/").stdin(Stdio::null()).output();
            let mut s = if was["kind"] == "none" {
                rec.put(Item { piece: "login".into(), path, target: None, agent: None, kind: Some(kind), created: true, at: now() });
                Step::new("login", "done", "Running in the background · starts at login").short("running · starts at login")
            } else if said["note"].is_string() {
                // A restart onto this binary is work done, not "already".
                Step::new("login", "done", "Running in the background · starts at login").short("running · starts at login")
            } else {
                Step::new("login", "already", "Running in the background · starts at login").short("running · starts at login")
            };
            if let Some(note) = said["note"].as_str() {
                s.short = Some(format!("running · starts at login · {note}"));
                s.detail = Some(note.to_string());
            }
            s
        }
        Ok(o) => Step::failed("login", "Couldn't start thc", anyhow!("{}", String::from_utf8_lossy(&o.stderr).trim().trim_start_matches("thc: "))),
        Err(e) => Step::failed("login", "Couldn't start thc", e),
    }
}

/// Every agent setup knows: (id, display name, config dir), whether installed or not.
fn all_agents(env: &Env) -> Vec<(&'static str, &'static str, PathBuf)> {
    let claude = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| env.home.join(".claude"));
    let codex = std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| env.home.join(".codex"));
    vec![("claude", "Claude Code", claude), ("codex", "Codex", codex)]
}

/// Agents on this machine (setup.md §2.4): the ones whose folder exists.
pub fn detected_agents(env: &Env) -> Vec<(&'static str, &'static str, PathBuf)> {
    all_agents(env).into_iter().filter(|(_, _, dir)| dir.is_dir()).collect()
}

/// The one question (setup.md §3.2): `Teach Claude Code to use thc? [Y/n]`. Some(ids) to set up,
/// Some([]) for no, None when there are no agents to ask about.
pub fn ask_agents() -> Result<Option<Vec<String>>> {
    use std::io::Write;
    let env = Env::from_process()?;
    let found = detected_agents(&env);
    if found.is_empty() {
        return Ok(None);
    }
    let list: Vec<&str> = found.iter().map(|(_, n, _)| *n).collect();
    eprint!("Teach {} to use thc? [Y/n] ", names(&list));
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    // No answer at all (EOF) is a no: only Enter or "y" means yes. Control characters (a ^D a
    // terminal echoed) don't count as an answer.
    let read = std::io::stdin().read_line(&mut answer).unwrap_or(0);
    let a = answer.trim_matches(|c: char| c.is_control() || c.is_whitespace()).to_lowercase();
    let yes = read > 0 && (a.is_empty() || a.starts_with('y')) && answer.contains('\n');
    if !yes && read == 0 {
        eprintln!();
    }
    Ok(Some(if yes { found.iter().map(|(id, _, _)| id.to_string()).collect() } else { vec![] }))
}

/// For `thc prime`: user-level agent files setup wrote that an older thc wrote (never a nudge to
/// install them: that's the person's call).
pub fn stale_user_lines() -> Vec<String> {
    let Ok(env) = Env::from_process() else { return vec![] };
    detected_agents(&env)
        .into_iter()
        .filter_map(|(id, _, dir)| {
            let (path, want, _) = agent_file(id, &dir).ok()?;
            let have = std::fs::read_to_string(&path).ok()?;
            let ours = have.contains("generated by `thc setup");
            (ours && have != want).then(|| format!("{} is from an older thc · thc setup {id} --user to refresh", tilde(&path)))
        })
        .collect()
}

/// `thc setup <agent> --user --check`: exit 1 if the user-level file is missing or stale.
pub fn check_user(out: &mut Out, agent: &str) -> Result<()> {
    let env = Env::from_process()?;
    let Some((_, name, dir)) = all_agents(&env).into_iter().find(|(id, _, _)| *id == agent) else {
        return Err(thc_core::error::usage(format!("setup supports claude or codex, not {agent}")));
    };
    let (path, want, _) = agent_file(agent, &dir)?;
    let have = std::fs::read_to_string(&path).ok();
    let msg = match &have {
        None => Some(format!("{name} doesn't know thc yet · thc setup {agent} --user")),
        Some(h) if *h != want => Some(format!("{} is from an older thc · thc setup {agent} --user to refresh", tilde(&path))),
        _ => None,
    };
    if out.json {
        out.json(&json!({ "stale": msg.is_some(), "path": path, "messages": msg.iter().collect::<Vec<_>>() }));
    } else {
        out.line(msg.clone().unwrap_or_else(|| format!("{} is current (thc {VERSION})", tilde(&path))));
    }
    if msg.is_none() { Ok(()) } else { Err(anyhow!(crate::daemon_cmd::Quiet)) }
}

/// The user-level file for an agent and its wanted content: Claude's skill, or Codex's AGENTS.md
/// with thc's short block merged in.
pub fn agent_file(agent: &str, dir: &Path) -> Result<(PathBuf, String, &'static str)> {
    Ok(match agent {
        "claude" => (dir.join("skills/thc/SKILL.md"), crate::setup::skill_text(), "file"),
        "codex" => {
            let p = dir.join("AGENTS.md");
            let have = std::fs::read_to_string(&p).unwrap_or_default();
            (p, crate::setup::merge_block(&have, &crate::setup::user_block()), "block")
        }
        other => return Err(thc_core::error::usage(format!("setup supports claude or codex, not {other}"))),
    })
}

fn names(list: &[&str]) -> String {
    match list {
        [] => String::new(),
        [a] => a.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn agents_step(env: &Env, rec: &mut Record, only: Option<&[String]>) -> Step {
    let mut found = detected_agents(env);
    // Named explicitly (`thc setup codex --user`): set it up even before its folder exists.
    for a in only.unwrap_or_default() {
        if !found.iter().any(|(id, _, _)| id == a) {
            if let Some(known) = all_agents(env).into_iter().find(|(id, _, _)| id == a) {
                found.push(known);
            }
        }
    }
    let chosen: Vec<_> = found.iter().filter(|(id, _, _)| only.is_none_or(|o| o.iter().any(|x| x == id))).collect();
    if chosen.is_empty() {
        let mut s = Step::new("agents", "none", "No agents found").short("none found · thc setup claude --user to add one later");
        s.detail = Some("add one later in Settings › Agents".into());
        if !found.is_empty() {
            s.text = "Agents not set up".into();
            s.detail = Some("you said no · thc setup claude --user to add later".into());
            s.short = Some("not set up · thc setup claude --user to add later".into());
        }
        return s;
    }
    let mut done = vec![];
    let mut already = vec![];
    let mut errors = vec![];
    for (id, name, dir) in chosen {
        let r = (|| -> Result<bool> {
            let (path, want, kind) = agent_file(id, dir)?;
            let have = std::fs::read_to_string(&path).ok();
            if have.as_deref() == Some(want.as_str()) {
                return Ok(false);
            }
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(&path, &want).with_context(|| format!("writing {}", tilde(&path)))?;
            let created = have.is_none() || rec.items.iter().any(|i| i.piece == "agent" && i.path.as_ref() == Some(&path) && i.created);
            rec.put(Item { piece: "agent".into(), path: Some(path), target: None, agent: Some(id.to_string()), kind: Some(kind.into()), created, at: now() });
            Ok(true)
        })();
        match r {
            Ok(true) => done.push(*name),
            Ok(false) => already.push(*name),
            Err(e) => errors.push(format!("{name}: {e:#}")),
        }
    }
    if !errors.is_empty() {
        let first = errors[0].split(':').next().unwrap_or("").to_string();
        return Step::failed("agents", format!("Couldn't set up {first}"), anyhow!("{}", errors.join("\n")));
    }
    let all: Vec<&str> = done.iter().chain(already.iter()).copied().collect();
    if done.is_empty() {
        return Step::new("agents", "already", "Your agents already know thc").short(all.join(" · "));
    }
    Step::new("agents", "done", format!("{} know{} thc", names(&all), if all.len() == 1 { "s" } else { "" })).short(all.join(" · "))
}

// ---- commands --------------------------------------------------------------------------------

/// One line in the terminal checklist (setup.md §3.1).
fn print_step(out: &mut Out, s: &Step) {
    let (mark, label) = match s.state {
        "done" | "already" => (out.green("✓"), s.step),
        "failed" => (out.red("✕"), s.step),
        _ => (out.dim("·"), s.step),
    };
    let label = match label {
        "path" => "thc",
        "login" => "background",
        l => l,
    };
    // The terminal's terser line when there is one (setup.md §3.1), else the window's copy.
    let line = match &s.short {
        Some(short) if s.state != "failed" => short.clone(),
        _ => format!("{}{}", s.text, s.detail.as_ref().map(|d| out.dim(&format!(" · {d}"))).unwrap_or_default()),
    };
    out.line(format!("  {mark} {label:<11} {line}"));
    if let Some(e) = &s.error {
        for l in e.lines() {
            out.line(format!("    {}", out.dim(l)));
        }
    }
}

/// `thc setup [--all] [--step S] [--agents a,b]`: run the steps, record them, print the checklist.
/// `agents` = None asks the question (interactive), Some(list) uses it as given.
pub fn run(out: &mut Out, steps: &[&str], agents: Option<Vec<String>>) -> Result<Vec<Step>> {
    let env = Env::from_process()?;
    let mut rec = Record::load(&env);
    let mut results = vec![];
    if !out.json && steps.len() > 1 {
        out.line("Setting up thought-central");
    }
    for name in steps {
        let s = run_step(name, &env, &mut rec, agents.as_deref());
        if !out.json && steps.len() > 1 {
            print_step(out, &s);
        }
        results.push(s);
    }
    if !rec.items.is_empty() {
        rec.save(&env)?;
    }
    if out.json {
        if results.len() == 1 {
            out.json(&serde_json::to_value(&results[0])?);
        } else {
            out.json(&json!({ "steps": results }));
        }
    } else if steps.len() == 1 {
        print_step(out, &results[0]);
    }
    // The app asks for notification permission itself; a CLI-only install has no app step.
    if steps.contains(&"login") && !out.json && Env::from_process().is_ok_and(|e| e.app().is_some()) {
        out.line(format!("  {} {:<11} {}", out.dim("·"), "reminders", out.dim("open Thought Central once to allow notifications")));
    }
    if !out.json && steps.len() > 1 {
        closing(out, &env, &results);
    }
    Ok(results)
}

/// The ending after the checklist: where the notes live and the first command.
/// A re-run that changed nothing gets one line.
fn closing(out: &mut Out, env: &Env, results: &[Step]) {
    if results.iter().any(|s| s.state == "failed") {
        return;
    }
    if !results.iter().any(|s| s.state == "done") {
        out.line(String::new());
        out.line(format!("  Up to date. {} to write.", out.bold("thc j")));
        return;
    }
    let vault = configured_vault(env).map(|v| tilde(&v)).unwrap_or_else(|| "~/thought".into());
    out.line(String::new());
    out.line(format!("  Ready. Your notes live in {vault}."));
    out.line(String::new());
    for (cmd, what) in [("thc j", "write in today's journal"), ("thc", "see what's due"), ("thc --help", "everything else")] {
        out.line(format!("    {} {}", out.bold(&format!("{cmd:<10}")), out.dim(what)));
    }
    // Only when this shell can't find thc yet (a fresh install, before a new window).
    let link_dir = env.local_bin();
    let live = std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d == link_dir));
    if !live {
        out.line(String::new());
        out.line(out.dim("  (new terminal window first, so thc is on your PATH)"));
    }
}

/// `thc setup --status`: each piece, where it is, and whether it's current.
pub fn status(out: &mut Out) -> Result<()> {
    let env = Env::from_process()?;
    let rec = Record::load(&env);
    let vault = configured_vault(&env);
    let link = env.local_bin().join("thc");
    let link_target = std::fs::read_link(&link).ok();
    let shell = login_shell(&env);
    let login = crate::daemon_cmd::install_info();
    // The app's login item counts only while thc has none of its own (after a migration the
    // LaunchAgent is the truth even if an old record says app).
    let app_login = rec.items.iter().any(|i| i.piece == "login" && i.kind.as_deref() == Some("app")) && login["kind"] == "none";
    let agents: Vec<_> = detected_agents(&env)
        .into_iter()
        .map(|(id, name, dir)| {
            let (path, want, _) = agent_file(id, &dir).unwrap();
            let have = std::fs::read_to_string(&path).ok();
            let state = match &have {
                None => "missing",
                Some(h) if *h == want => "current",
                Some(_) => "stale",
            };
            json!({ "agent": id, "name": name, "path": path, "state": state })
        })
        .collect();
    let v = json!({
        "version": VERSION,
        "record": Record::path(&env),
        "recorded": rec.items,
        "vault": vault,
        "link": link_target.as_ref().map(|t| json!({ "path": link, "target": t })),
        "thc_on_path": shell.as_ref().and_then(|(t, _)| t.clone()),
        "login": if app_login { json!({ "kind": "app" }) } else { login },
        "agents": agents,
    });
    if out.json {
        out.json(&v);
        return Ok(());
    }
    let key = |out: &Out, k: &str| out.dim(&format!("{k:<12}"));
    out.line(format!("{}{}", key(out, "vault"), vault.as_ref().map(|v| tilde(v)).unwrap_or_else(|| "none · thc setup to create ~/thought".into())));
    let on_path = shell.as_ref().and_then(|(t, _)| t.clone());
    let thc_line = match (&link_target, &on_path) {
        (Some(t), Some(p)) if same_file(p, &link) => format!("{} → {}", tilde(&link), tilde(t)),
        (Some(t), Some(p)) => format!("{} → {} · but {} comes first on PATH", tilde(&link), tilde(t), tilde(p)),
        (Some(t), None) => format!("{} → {} · not on PATH in a new shell", tilde(&link), tilde(t)),
        (None, Some(p)) if same_file(p, &link) && std::fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_file()) => format!("{} (standalone · thc update keeps it current)", tilde(&link)),
        (None, Some(p)) => format!("{} (not set up by thc)", tilde(p)),
        (None, None) => "not on PATH · thc setup --step path".into(),
    };
    out.line(format!("{}{thc_line}", key(out, "thc")));
    for i in rec.items.iter().filter(|i| i.piece == "profile") {
        out.line(format!("{}PATH line in {}", key(out, ""), i.path.as_ref().map(|p| tilde(p)).unwrap_or_default()));
    }
    let bg = if app_login {
        "login item · Thought Central (SMAppService)".to_string()
    } else {
        match v["login"]["kind"].as_str() {
            Some("none") | None => "no login item · thc setup --step login".into(),
            Some(k) => format!("login item · {k} · {}", v["login"]["path"].as_str().map(|p| tilde(Path::new(p))).unwrap_or_default()),
        }
    };
    out.line(format!("{}{bg}", key(out, "background")));
    if v["agents"].as_array().is_some_and(|a| a.is_empty()) {
        out.line(format!("{}{}", key(out, "agents"), out.dim("none found (~/.claude, ~/.codex)")));
    }
    for (n, a) in v["agents"].as_array().unwrap().iter().enumerate() {
        out.line(format!(
            "{}{:<12} {} · {}",
            key(out, if n == 0 { "agents" } else { "" }),
            a["name"].as_str().unwrap_or(""),
            tilde(Path::new(a["path"].as_str().unwrap_or(""))),
            a["state"].as_str().unwrap_or("")
        ));
    }
    Ok(())
}

/// `--record-link <path>`: a link to this binary made outside setup (the app's admin install of
/// /usr/local/bin/thc), recorded so --undo removes it (or says it needs a password).
pub fn record_link(out: &mut Out, link: &Path) -> Result<()> {
    let env = Env::from_process()?;
    let target = std::fs::read_link(link).with_context(|| format!("{} isn't a link", tilde(link)))?;
    if !is_our_link(link, &env.exe) && !same_file(&target, &env.exe) {
        return Err(thc_core::error::ThcError::Validation(format!("{} doesn't point at Thought Central's thc", tilde(link))).into());
    }
    let mut rec = Record::load(&env);
    rec.put(Item { piece: "link".into(), path: Some(link.to_path_buf()), target: Some(target), agent: None, kind: Some("system".into()), created: true, at: now() });
    rec.save(&env)?;
    if out.json {
        out.json(&json!({ "ok": true, "recorded": link }));
    } else {
        out.line(format!("recorded {}", tilde(link)));
    }
    Ok(())
}

/// Remove the marked PATH line (the mark and the line after it) from a profile.
fn strip_profile(text: &str) -> String {
    let mut out = vec![];
    let mut skip_next = false;
    for l in text.lines() {
        if skip_next {
            skip_next = false;
            continue;
        }
        if l == PROFILE_MARK {
            skip_next = true;
            continue;
        }
        out.push(l);
    }
    let mut s = out.join("\n");
    if !s.is_empty() && text.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// `thc setup --undo`: reverse each recorded item if it's still what setup left; never the vault.
/// With `only_agent`, just that agent's files, and the record keeps everything else.
pub fn undo(out: &mut Out, dry_run: bool, only_agent: Option<&str>) -> Result<()> {
    let env = Env::from_process()?;
    let mut rec = Record::load(&env);
    if let Some(a) = only_agent {
        if !all_agents(&env).iter().any(|(id, _, _)| *id == a) {
            return Err(thc_core::error::usage(format!("setup supports claude or codex, not {a}")));
        }
    }
    let targets: Vec<Item> = rec.items.iter().filter(|i| only_agent.is_none_or(|a| i.piece == "agent" && i.agent.as_deref() == Some(a))).cloned().collect();
    let mut lines: Vec<String> = vec![];
    let mut errors: Vec<String> = vec![];
    let mut vault_path = configured_vault(&env);
    for i in targets.iter().rev() {
        let Some(path) = i.path.clone() else { continue };
        let r: Result<Option<String>> = (|| {
            match i.piece.as_str() {
                "vault" => {
                    vault_path = Some(path.clone());
                    Ok(None)
                }
                "link" => {
                    let target = i.target.clone().unwrap_or_default();
                    if !is_our_link(&path, &target) {
                        return Ok(Some(format!("left {} (it isn't Thought Central's link any more)", tilde(&path))));
                    }
                    if !dry_run {
                        if let Err(e) = std::fs::remove_file(&path) {
                            if e.kind() == std::io::ErrorKind::PermissionDenied {
                                return Ok(Some(format!("left {} (it needs your password: sudo rm {})", tilde(&path), path.display())));
                            }
                            return Err(e.into());
                        }
                    }
                    Ok(Some(format!("removed the thc link {}", tilde(&path))))
                }
                "profile" => {
                    let Ok(text) = std::fs::read_to_string(&path) else { return Ok(None) };
                    if !text.contains(PROFILE_MARK) {
                        return Ok(None);
                    }
                    let stripped = strip_profile(&text);
                    if !dry_run {
                        if i.created && stripped.trim().is_empty() {
                            std::fs::remove_file(&path)?;
                        } else {
                            std::fs::write(&path, stripped)?;
                        }
                    }
                    Ok(Some(format!("removed the PATH line from {}", tilde(&path))))
                }
                "agent" => {
                    let Ok(text) = std::fs::read_to_string(&path) else { return Ok(None) };
                    let name = i.agent.as_deref().unwrap_or("agent");
                    if i.kind.as_deref() == Some("block") {
                        let stripped = crate::setup::strip_block(&text);
                        if stripped == text {
                            return Ok(None);
                        }
                        if !dry_run {
                            if i.created && stripped.trim().is_empty() {
                                std::fs::remove_file(&path)?;
                            } else {
                                std::fs::write(&path, stripped)?;
                            }
                        }
                        Ok(Some(format!("removed thc's block from {} ({name})", tilde(&path))))
                    } else {
                        if !text.contains("generated by `thc setup") {
                            return Ok(Some(format!("left {} (edited by hand)", tilde(&path))));
                        }
                        if !dry_run {
                            std::fs::remove_file(&path)?;
                            if let Some(dir) = path.parent() {
                                let _ = std::fs::remove_dir(dir); // only if empty
                            }
                        }
                        Ok(Some(format!("removed the {name} skill {}", tilde(&path))))
                    }
                }
                "login" => {
                    if login_skipped() {
                        return Ok(None);
                    }
                    match i.kind.as_deref() {
                        Some("app") => {
                            let exe = path.join("Contents/MacOS/ThoughtBar");
                            if !exe.exists() {
                                return Ok(Some("the app is gone, so is its login item".into()));
                            }
                            if !dry_run {
                                let o = Command::new(&exe).arg("--unregister-login-item").stdin(Stdio::null()).output()?;
                                if !o.status.success() {
                                    return Err(anyhow!("{}", String::from_utf8_lossy(&o.stderr).trim()));
                                }
                                let _ = Command::new(&env.exe).args(["daemon", "stop"]).current_dir("/").output();
                            }
                            Ok(Some("removed the login item and stopped the daemon".into()))
                        }
                        _ => {
                            if !dry_run {
                                let o = Command::new(&env.exe).args(["daemon", "uninstall"]).current_dir("/").stdin(Stdio::null()).output()?;
                                if !o.status.success() {
                                    return Err(anyhow!("{}", String::from_utf8_lossy(&o.stderr).trim()));
                                }
                            }
                            Ok(Some(format!("removed the login item {} and stopped the daemon", tilde(&path))))
                        }
                    }
                }
                _ => Ok(None),
            }
        })();
        match r {
            Ok(Some(l)) => lines.push(l),
            Ok(None) => {}
            Err(e) => errors.push(format!("{}: {e:#}", tilde(&path))),
        }
    }
    if !dry_run && errors.is_empty() {
        if only_agent.is_some() {
            rec.items.retain(|i| !targets.contains(i));
            rec.save(&env)?;
        } else {
            let _ = std::fs::remove_file(Record::path(&env));
        }
    }
    let notes = vault_path.map(|v| tilde(&v)).unwrap_or_else(|| "your vault".into());
    if out.json {
        out.json(&json!({ "dry_run": dry_run, "removed": lines, "errors": errors, "vault": notes }));
    } else {
        if lines.is_empty() && errors.is_empty() {
            out.line("nothing to undo: setup hasn't added anything here");
        }
        for l in &lines {
            out.line(format!("{}{l}", if dry_run { "would have " } else { "" }));
        }
        for e in &errors {
            out.line(out.red(&format!("✕ {e}")));
        }
        out.line(out.dim(&format!("your notes in {notes} were not touched")));
    }
    if errors.is_empty() { Ok(()) } else { Err(anyhow!(crate::daemon_cmd::Quiet)) }
}

// ---- the first run of thc --------------------------------------------------------------------

/// Whether a command with no vault may set the machine up (setup.md §3.2): a person at a
/// terminal, not an agent, CI, a pipe or a machine reading JSON.
pub fn first_run_allowed(json: bool) -> bool {
    use std::io::IsTerminal;
    let actor_ok = std::env::var("THC_ACTOR").map(|a| a.is_empty() || a == "human").unwrap_or(true);
    std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && actor_ok
        && std::env::var_os("CI").is_none()
        && std::env::var("THC_NO_SETUP").map(|v| v != "1").unwrap_or(true)
        && !json
}

/// Bootstrap once: vault, login item, and (after one question) the agent skills. Prints one
/// summary line to stderr so the command's own output stays clean.
pub fn first_run() -> Result<()> {
    let env = Env::from_process()?;
    let mut rec = Record::load(&env);
    let vault = run_step("vault", &env, &mut rec, None);
    if vault.state == "failed" {
        rec.save(&env).ok();
        return Err(anyhow!("{}: {}", vault.text, vault.error.unwrap_or_default()));
    }
    let login = run_step("login", &env, &mut rec, None);
    let chosen: Vec<String> = ask_agents()?.unwrap_or_default();
    let agents = if chosen.is_empty() { None } else { Some(run_step("agents", &env, &mut rec, Some(&chosen))) };
    rec.save(&env)?;
    let mut bits = vec![format!("vault {}", vault.path.as_deref().map(|p| tilde(Path::new(p))).unwrap_or_default())];
    bits.push(match login.state {
        "done" | "already" => "daemon running".into(),
        "skipped" => "daemon not started".into(),
        _ => format!("daemon didn't start ({})", login.error.as_deref().unwrap_or("")),
    });
    if let Some(a) = &agents {
        let ids: Vec<&str> = chosen.iter().map(String::as_str).collect();
        bits.push(match a.state {
            "failed" => format!("skill not installed ({})", a.error.as_deref().unwrap_or("")),
            _ => format!("skill installed for {}", ids.join(", ")),
        });
    }
    bits.push("undo: thc setup --undo".into());
    eprintln!("set up: {}", bits.join(" · "));
    // In WezTerm: the editing keys, offered once (never written without a yes).
    let mut o = crate::out::Out::new(false);
    let _ = crate::wezterm::offer(&mut o);
    o.flush();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_line_round_trips() {
        let before = "export FOO=1\n";
        let with = format!("{before}{PROFILE_MARK}\nexport PATH=\"$HOME/.local/bin:$PATH\"\n");
        assert_eq!(strip_profile(&with), before);
        assert_eq!(strip_profile(before), before);
    }

    #[test]
    fn versions_compare() {
        assert!(older("0.2.1", "0.3.0"));
        assert!(!older("0.3.0", "0.3.0"));
        assert!(!older("1.0.0", "0.3.0"));
    }

    #[test]
    fn agent_names_read_naturally() {
        assert_eq!(names(&["Claude Code"]), "Claude Code");
        assert_eq!(names(&["Claude Code", "Codex"]), "Claude Code and Codex");
    }
}

// ---- migrating from Thought Central.app (the CLI takes over) ------------------------------------

/// The app's SMAppService agent label (its bundled daemon).
const APP_DAEMON_LABEL: &str = "io.github.brancusi.thought-central.thc";

/// `thc setup --migrate-from-app`: this binary becomes ~/.local/bin/thc (replacing the link into
/// Thought Central.app), the app's login item goes, and thc's own LaunchAgent runs the daemon.
/// Ends with one daemon, on the new binary. Never deletes the app: it says how.
pub fn migrate_from_app(out: &mut Out) -> Result<()> {
    let env = Env::from_process()?;
    let dest = env.local_bin().join("thc");
    let me = std::env::current_exe()?;
    if me.to_string_lossy().contains(".app/Contents/") {
        return Err(anyhow!("run this with the standalone thc (install.sh puts it next to the link), not the app's copy"));
    }
    // 1. The app: THC_SETUP_APP when set ("none" = no app, and nothing else is looked at, so a
    //    test can never reach the real one), else the link's target, else /Applications.
    let app: Option<PathBuf> = match std::env::var("THC_SETUP_APP") {
        Ok(v) if v == "none" => None,
        Ok(v) if !v.is_empty() => Some(PathBuf::from(v)),
        _ => std::fs::read_link(&dest)
            .ok()
            .and_then(|t| t.ancestors().find(|a| a.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf))
            .or_else(|| [PathBuf::from("/Applications/Thought Central.app")].into_iter().find(|p| p.exists())),
    };
    // 2. This binary at ~/.local/bin/thc: a copy then a rename, which replaces the link itself.
    std::fs::create_dir_all(env.local_bin())?;
    if !same_file(&me, &dest) || std::fs::symlink_metadata(&dest).is_ok_and(|m| m.file_type().is_symlink()) {
        let tmp = env.local_bin().join(".thc.migrating");
        std::fs::copy(&me, &tmp).with_context(|| format!("copying to {}", tilde(&tmp)))?;
        std::fs::rename(&tmp, &dest).with_context(|| format!("replacing {}", tilde(&dest)))?;
    }
    out.line(format!("{} thc {VERSION} is now {}", out.green("●"), tilde(&dest)));
    // 3. The app's login item (only the app can unregister its own), then stop its daemon now.
    let mut app_note = None;
    if let Some(app) = &app {
        thc_core::sandbox::check(app);
        let bin = app.join("Contents/MacOS/ThoughtBar");
        if bin.exists() {
            match Command::new(&bin).arg("--unregister-login-item").stdin(Stdio::null()).output() {
                Ok(o) if o.status.success() => out.line(format!("{} the app's login item is off", out.green("●"))),
                Ok(o) => out.line(out.yellow(&format!("! couldn't turn off the app's login item: {} · quit Thought Central from its menu", String::from_utf8_lossy(&o.stderr).trim()))),
                Err(e) => out.line(out.yellow(&format!("! couldn't run the app ({e}) · quit Thought Central from its menu"))),
            }
        }
        app_note = Some(app.clone());
    }
    if cfg!(target_os = "macos") && !login_skipped() {
        let uid = unsafe { libc::getuid() };
        let _ = thc_core::sandbox::tool("launchctl").args(["bootout", &format!("gui/{uid}/{APP_DAEMON_LABEL}")]).output();
    }
    // The app's records go: its login item is off and the link is now this binary.
    let mut rec = Record::load(&env);
    rec.items.retain(|i| !(i.piece == "login" && i.kind.as_deref() == Some("app")) && !(i.piece == "link" && i.path.as_deref() == Some(dest.as_path())));
    let _ = rec.save(&env);
    // 4. A daemon still on another binary (the app's) stops; ours keeps running. Setup's login
    //    step installs and starts ours.
    if daemons().iter().any(|c| !c.starts_with(&dest.display().to_string())) {
        let _ = Command::new(&dest).args(["daemon", "stop"]).current_dir("/").stdin(Stdio::null()).output();
    }
    let st = Command::new(&dest).args(["setup", "--all", "--yes"]).current_dir("/").env("THC_SETUP_APP", "none").status()?;
    if !st.success() {
        return Err(anyhow!("setup didn't finish · thc setup --status shows what's left"));
    }
    // 5. Exactly one daemon, on the new binary.
    if !login_skipped() {
        std::thread::sleep(std::time::Duration::from_millis(800));
        let daemons = daemons();
        let ours = daemons.iter().filter(|c| c.starts_with(&dest.display().to_string())).count();
        if daemons.len() == 1 && ours == 1 {
            out.line(format!("{} one daemon, on {}", out.green("●"), tilde(&dest)));
        } else {
            out.line(out.yellow(&format!("! {} daemons running: {}", daemons.len(), daemons.join(" | "))));
        }
    }
    if let Some(app) = app_note {
        out.line(String::new());
        let mut apps = vec![app.display().to_string()];
        if Path::new("/Applications/ThoughtBar.app").exists() {
            apps.push("/Applications/ThoughtBar.app".into());
        }
        out.line(format!("You can delete {} now (drag to the Trash). thc no longer uses it.", apps.join(" and ")));
    }
    Ok(())
}

/// Running `thc daemon run` processes, by command line.
fn daemons() -> Vec<String> {
    let Ok(o) = Command::new("ps").args(["-axo", "command="]).output() else { return vec![] };
    // Only real ones: `<path>/thc daemon run` exactly (a shell whose command line mentions it isn't).
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            w.len() == 3 && w[0].ends_with("/thc") && w[1] == "daemon" && w[2] == "run"
        })
        .map(str::to_string)
        .collect()
}
