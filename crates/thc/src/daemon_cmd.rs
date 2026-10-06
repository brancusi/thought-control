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

/// A value from thc's LaunchAgent's EnvironmentVariables (`THC_VAULT`, `THC_CACHE_DIR`): which
/// vault the login daemon serves, whatever the caller's directory resolves to.
pub(crate) fn plist_env(key: &str) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let s = std::fs::read_to_string(plist_path()).ok()?;
    let rest = &s[s.find(&format!("<key>{key}</key>"))? ..];
    let a = rest.find("<string>")? + "<string>".len();
    let b = rest[a..].find("</string>")?;
    Some(rest[a..a + b].to_string())
}

/// thc's LaunchAgent serves this vault: launchd owns its daemon, so starts and restarts go
/// through launchctl and thc never spawns a second one beside it.
fn launchd_serves(paths: &Paths) -> bool {
    plist_env("THC_VAULT").is_some_and(|v| Path::new(&v) == paths.vault)
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

/// Restart a launchd-owned daemon: `kickstart -k` (launchd stops the old process and starts the
/// new one), then check that exactly that happened: a new pid answers and the old one is gone.
fn restart_launchd(out: &mut Out, paths: &Paths) -> Result<()> {
    let old = status(paths).and_then(|s| s["pid"].as_u64()).map(|p| p as u32);
    let log_path = paths.cache.join("daemon.log");
    if !kickstart(true) {
        return Err(anyhow!("launchctl couldn't restart {LABEL} · launchctl print gui/$(id -u)/{LABEL}"));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Some(st) = status(paths) {
            let pid = st["pid"].as_u64().map(|p| p as u32);
            if pid != old && old.is_none_or(|o| !proto::pid_alive(o)) {
                if out.json {
                    out.json(&st);
                } else {
                    out.line(format!("{} daemon restarted · pid {} · thc {}", out.green("●"), st["pid"], st["version"].as_str().unwrap_or("?")));
                }
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let now = status(paths).and_then(|s| s["pid"].as_u64());
    let why = match (old, now) {
        (Some(o), Some(n)) if n as u32 == o => format!("pid {o} is still the one answering"),
        (Some(o), _) if proto::pid_alive(o) => format!("the old daemon (pid {o}) is still running"),
        (_, None) => "no daemon answers".into(),
        _ => "it didn't settle".into(),
    };
    Err(anyhow!("launchd didn't restart the daemon: {why} · log: {}", tilde(&log_path)))
}

fn unit_path() -> PathBuf {
    let p = home().join(".config/systemd/user/thc.service");
    thc_core::sandbox::check(&p);
    p
}

pub fn install_info() -> Value {
    if cfg!(target_os = "macos") {
        let p = plist_path();
        if p.exists() {
            return json!({ "kind": "launchd", "label": LABEL, "loaded": true, "path": p });
        }
    } else if unit_path().exists() {
        return json!({ "kind": "systemd", "unit": "thc.service", "loaded": true, "path": unit_path() });
    }
    json!({ "kind": "none" })
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
        Restart if launchd_serves(paths) => restart_launchd(out, paths),
        Restart => {
            stop(paths)?;
            start(out, paths, true)
        }
        Install => install(out, paths),
        Uninstall => {
            let _ = stop(paths);
            if cfg!(target_os = "macos") {
                let p = plist_path();
                if p.exists() {
                    let uid = unsafe { libc::getuid() };
                    let _ = thc_core::sandbox::tool("launchctl").args(["bootout", &format!("gui/{uid}"), &p.to_string_lossy()]).output();
                    std::fs::remove_file(&p)?;
                }
            } else if unit_path().exists() {
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
    if st["version"].as_str().is_some_and(|v| v != VERSION) {
        out.line(format!("{}the daemon is thc {}, this is {VERSION} · thc daemon restart", key(out, "update"), st["version"].as_str().unwrap_or("?")));
    }
}

fn start(out: &mut Out, paths: &Paths, restarted: bool) -> Result<()> {
    if let Some(st) = status(paths) {
        if out.json {
            out.json(&st);
        } else {
            out.line(format!("{} already running · pid {}", out.green("●"), st["pid"]));
        }
        return Ok(());
    }
    // thc's LaunchAgent serves this vault: ask launchd, so it owns the process (restarts,
    // login) instead of a child of this shell. If launchctl accepts but nothing answers, that's
    // an error: a direct spawn beside launchd's job would make two daemons.
    if launchd_serves(paths) {
        let accepted = kickstart(false);
        {
            let deadline = Instant::now() + Duration::from_secs(if accepted { 8 } else { 0 });
            while Instant::now() < deadline {
                if let Some(st) = status(paths) {
                    if out.json {
                        out.json(&st);
                    } else if restarted {
                        out.line(format!("{} daemon restarted · pid {} · thc {}", out.green("●"), st["pid"], st["version"].as_str().unwrap_or("?")));
                    } else {
                        out.line(format!("{} daemon started · pid {} · watching {}", out.green("●"), st["pid"], tilde(&paths.vault)));
                    }
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            if accepted {
                return Err(anyhow!("launchd didn't start the daemon · log: {} · launchctl print gui/$(id -u)/{LABEL}", tilde(&paths.cache.join("daemon.log"))));
            }
            // launchctl itself failed (no launchd here): start it directly, below.
        }
    }
    // Under THC_TEST a daemon outlives the test that started it (a first run in a scratch HOME
    // left one running): only tests that ask for one get it.
    if thc_core::sandbox::active() && std::env::var("THC_TEST_DAEMON").map_or(true, |v| v != "1") {
        out.line("daemon not started: THC_TEST is set (THC_TEST_DAEMON=1 to start one)");
        return Ok(());
    }
    let exe = std::env::current_exe()?;
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
            if out.json {
                out.json(&st);
            } else if restarted {
                out.line(format!("{} daemon restarted · pid {} · thc {}", out.green("●"), st["pid"], st["version"].as_str().unwrap_or("?")));
            } else {
                out.line(format!("{} daemon started · pid {} · watching {}", out.green("●"), st["pid"], tilde(&paths.vault)));
            }
            return Ok(());
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

fn install(out: &mut Out, paths: &Paths) -> Result<()> {
    let exe = std::env::current_exe()?;
    let log = paths.cache.join("daemon.log");
    if cfg!(target_os = "macos") {
        let p = plist_path();
        if p.exists() {
            if out.json {
                out.json(&json!({ "ok": true, "already": true, "install": install_info() }));
            } else {
                out.line(format!("{} already a login item ({})", out.green("●"), tilde(&p)));
            }
            return Ok(());
        }
        let _ = stop(paths);
        std::fs::create_dir_all(p.parent().unwrap())?;
        let xml = format!(
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
            paths.vault.display(),
            paths.cache.display(),
            log.display(),
            log.display()
        );
        std::fs::write(&p, xml)?;
        let uid = unsafe { libc::getuid() };
        let r = thc_core::sandbox::tool("launchctl").args(["bootstrap", &format!("gui/{uid}"), &p.to_string_lossy()]).output()?;
        if !r.status.success() {
            return Err(anyhow!("launchctl bootstrap failed: {}", String::from_utf8_lossy(&r.stderr).trim()));
        }
        let pid = wait_status(paths).map(|s| s["pid"].clone()).unwrap_or(Value::Null);
        if out.json {
            out.json(&json!({ "ok": true, "install": install_info(), "pid": pid }));
        } else {
            out.line(format!("{} installed as a login item (launchd, {LABEL})", out.green("●")));
            out.line(format!("  {}   {}", out.dim("plist"), tilde(&p)));
            out.line(format!("  {} pid {pid}", out.dim("started")));
        }
    } else {
        let p = unit_path();
        std::fs::create_dir_all(p.parent().unwrap())?;
        let unit = format!(
            "[Unit]\nDescription=thc daemon (thought-central)\n\n[Service]\nExecStart={} daemon run\nEnvironment=THC_VAULT={}\nEnvironment=THC_CACHE_DIR={}\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n",
            exe.display(),
            paths.vault.display(),
            paths.cache.display()
        );
        let _ = stop(paths);
        std::fs::write(&p, unit)?;
        let r = thc_core::sandbox::tool("systemctl").args(["--user", "enable", "--now", "thc.service"]).output()?;
        if !r.status.success() {
            return Err(anyhow!("systemctl failed: {}", String::from_utf8_lossy(&r.stderr).trim()));
        }
        let pid = wait_status(paths).map(|s| s["pid"].clone()).unwrap_or(Value::Null);
        if out.json {
            out.json(&json!({ "ok": true, "install": install_info(), "pid": pid }));
        } else {
            out.line(format!("{} installed as a systemd --user unit (thc.service)", out.green("●")));
            out.line(format!("  {}    {}", out.dim("unit"), tilde(&p)));
            out.line(format!("  {} pid {pid}", out.dim("started")));
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
