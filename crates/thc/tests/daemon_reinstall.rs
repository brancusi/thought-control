//! Reinstalling or updating thc never leaves the old daemon running (j2zgw). An install renames
//! a new binary over the old one, which keeps the path (and the version, for a rebuild), so the
//! running daemon looked current while it ran the replaced file. `thc setup` (install.sh's last
//! step), `thc daemon install` and `thc daemon start` now notice and restart it through whatever
//! runs it: launchd, systemd --user, or a plain process. `thc doctor` flags it.
//!
//! The service managers are shims (fake launchd / systemd for one job) that run the binary the
//! login item names, so each test can count processes. Nothing touches the real machine.

mod common;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Launchd,
    Systemd,
    Plain,
}

struct Rig {
    kind: Kind,
    root: PathBuf,
    home: PathBuf,
    vault: PathBuf,
    cache: PathBuf,
    shims: PathBuf,
    /// The installed thc (a copy of the build, replaced by `reinstall`).
    bin: PathBuf,
}

const SHIM_COMMON: &str = r#"
alive() { pid=$(cat "$S/pid" 2>/dev/null); [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; }
stopit() {
  if alive; then
    kill "$pid"; n=0
    while kill -0 "$pid" 2>/dev/null && [ $n -lt 100 ]; do sleep 0.05; n=$((n+1)); done
  fi
}
startit() {
  THC_VAULT="$VAULT" THC_CACHE_DIR="$CACHE" nohup "$BIN" daemon run < /dev/null >> "$S/daemon.log" 2>&1 &
  echo $! > "$S/pid"; echo $! >> "$S/all"
}
"#;

impl Rig {
    fn new(name: &str, kind: Kind, item_exe: Option<&str>) -> Rig {
        let root = std::env::temp_dir().join(format!("thc-reinstall-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let (home, shims, state, bindir) = (root.join("home"), root.join("shims"), root.join("state"), root.join("bin"));
        for d in [&home, &shims, &state, &bindir] {
            std::fs::create_dir_all(d).unwrap();
        }
        let (vault, cache) = (root.join("vault"), root.join("cache"));
        let bin = bindir.join("thc");
        std::fs::copy(env!("CARGO_BIN_EXE_thc"), &bin).unwrap();
        let head = format!("#!/bin/sh\nS=\"{}\"\nVAULT=\"{}\"\nCACHE=\"{}\"\n{SHIM_COMMON}", state.display(), vault.display(), cache.display());
        let plist = home.join("Library/LaunchAgents/dev.thought.thc.plist");
        let unit = home.join(".config/systemd/user/thc.service");
        // launchd for one job, running the plist's ProgramArguments[0].
        let launchctl = format!(
            r#"{head}
echo "launchctl $*" >> "$S/calls"
BIN=$(tr -d '\n' < "{plist}" | sed -E 's|.*<key>ProgramArguments</key>[[:space:]]*<array>[[:space:]]*<string>([^<]*)</string>.*|\1|')
case "$1" in
  kickstart) if [ "$2" = "-k" ]; then stopit; startit; elif ! alive; then startit; fi ;;
  bootout) stopit ;;
  bootstrap) alive || startit ;;
esac
exit 0
"#,
            plist = plist.display()
        );
        // systemd --user for one unit, running its ExecStart program.
        let systemctl = format!(
            r#"{head}
echo "systemctl $*" >> "$S/calls"
BIN=$(sed -n 's/^ExecStart=\([^ ]*\).*/\1/p' "{unit}")
case "$2" in
  restart) stopit; startit ;;
  start) alive || startit ;;
  enable) alive || startit ;;
  disable|stop) stopit ;;
esac
exit 0
"#,
            unit = unit.display()
        );
        for (n, body) in [("launchctl", launchctl), ("systemctl", systemctl)] {
            std::fs::write(shims.join(n), body).unwrap();
            Command::new("chmod").arg("+x").arg(shims.join(n)).status().unwrap();
        }
        let rig = Rig { kind, root, home, vault, cache, shims, bin };
        let o = rig.thc().args(["init"]).arg(&rig.vault).arg("--global").output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let exe = item_exe.map(|n| rig.root.join("bin").join(n)).unwrap_or_else(|| rig.bin.clone());
        if exe != rig.bin {
            std::fs::copy(env!("CARGO_BIN_EXE_thc"), &exe).unwrap();
        }
        // The login item, as `thc daemon install` writes it.
        match kind {
            Kind::Launchd => {
                std::fs::create_dir_all(plist.parent().unwrap()).unwrap();
                std::fs::write(
                    &plist,
                    format!(
                        "<?xml version=\"1.0\"?>\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key><string>dev.thought.thc</string>\n  <key>ProgramArguments</key><array><string>{}</string><string>daemon</string><string>run</string></array>\n  <key>EnvironmentVariables</key><dict>\n    <key>THC_VAULT</key><string>{}</string>\n    <key>THC_CACHE_DIR</key><string>{}</string>\n  </dict>\n</dict>\n</plist>\n",
                        exe.display(),
                        rig.vault.display(),
                        rig.cache.display()
                    ),
                )
                .unwrap();
            }
            Kind::Systemd => {
                std::fs::create_dir_all(unit.parent().unwrap()).unwrap();
                std::fs::write(
                    &unit,
                    format!(
                        "[Service]\nExecStart={} daemon run\nEnvironment=THC_VAULT={}\nEnvironment=THC_CACHE_DIR={}\nRestart=on-failure\n",
                        exe.display(),
                        rig.vault.display(),
                        rig.cache.display()
                    ),
                )
                .unwrap();
            }
            Kind::Plain => {}
        }
        rig
    }

    /// The installed thc, sandboxed, with this rig's HOME, shims and service manager.
    fn thc(&self) -> Command {
        let mut c = Command::new(&self.bin);
        common::sandbox(&mut c);
        let path = format!("{}:{}", self.shims.display(), std::env::var("PATH").unwrap_or_default());
        let manager = match self.kind {
            Kind::Launchd => "launchd",
            Kind::Systemd => "systemd",
            Kind::Plain => "none",
        };
        c.current_dir(&self.root)
            .env("HOME", &self.home)
            .env("PATH", path)
            .env("THC_TEST_ROOT", &self.root)
            .env("THC_CACHE_DIR", &self.cache)
            .env("THC_SERVICE_MANAGER", manager)
            .env("THC_TEST_DAEMON", "1")
            .env_remove("THC_VAULT");
        c
    }

    fn json(&self, args: &[&str]) -> Value {
        let o = self.thc().arg("--json").args(args).output().unwrap();
        serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{args:?}: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
    }

    fn status(&self) -> Value {
        let o = self.thc().args(["--json", "daemon", "status"]).output().unwrap();
        serde_json::from_slice(&o.stdout).unwrap_or(Value::Null)
    }

    /// install.sh's swap: a fresh copy renamed over the installed binary (same path and
    /// version, another file).
    fn reinstall(&self) {
        let tmp = self.bin.with_file_name(".thc.installing");
        std::fs::copy(env!("CARGO_BIN_EXE_thc"), &tmp).unwrap();
        std::fs::rename(&tmp, &self.bin).unwrap();
    }

    fn identity(&self, p: &Path) -> String {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::metadata(p).unwrap();
        format!("{}:{}", m.dev(), m.ino())
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.root.join("state/calls")).unwrap_or_default()
    }

    /// Every daemon still alive: those the fake manager started, and the one answering.
    fn alive(&self) -> Vec<u32> {
        let all = std::fs::read_to_string(self.root.join("state/all")).unwrap_or_default();
        let mut pids: Vec<u32> = all.lines().filter_map(|l| l.trim().parse().ok()).collect();
        if let Some(p) = self.status()["pid"].as_u64() {
            pids.push(p as u32);
        }
        pids.sort();
        pids.dedup();
        pids.into_iter().filter(|p| unsafe { libc::kill(*p as i32, 0) } == 0).collect()
    }

    /// Exactly one daemon alive, and it's the one answering: its status.
    fn wait_one(&self) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let a = self.alive();
            let st = self.status();
            if a.len() == 1 && st["pid"].as_u64() == Some(a[0] as u64) {
                return st;
            }
            assert!(Instant::now() < deadline, "expected one daemon, alive: {a:?}, status: {st}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        for p in self.alive() {
            unsafe { libc::kill(p as i32, libc::SIGTERM) };
        }
        std::thread::sleep(Duration::from_millis(200));
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The whole bug, for one service manager: a daemon on the installed thc, a reinstall, and
/// install.sh's `thc setup` step that must restart it onto the new file through the manager.
fn reinstall_restarts(kind: Kind, name: &str, restart_call: &str) {
    let rig = Rig::new(name, kind, None);
    let o = rig.thc().args(["daemon", "start"]).output().unwrap();
    assert!(o.status.success(), "{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    let first = rig.wait_one();
    assert_eq!(first["exe_id"].as_str(), Some(rig.identity(&rig.bin).as_str()), "{first}");
    assert!(first["stale"].is_null(), "{first}");
    let doctor = rig.json(&["doctor"]);
    assert_eq!(doctor["daemon"]["state"], "live", "{doctor}");
    assert!(doctor["daemon"]["stale"].is_null(), "{doctor}");

    rig.reinstall();

    // Before anything restarts it: status and doctor both say so, with the fix.
    let st = rig.status();
    assert!(st["stale"].as_str().is_some_and(|s| s.contains("older copy")), "{st}");
    let doctor = rig.json(&["doctor"]);
    let issues = doctor["issues"].to_string();
    assert!(issues.contains("stale") && issues.contains("thc daemon restart"), "{doctor}");

    // install.sh's last step: setup's login step restarts it, through the manager.
    let mut c = rig.thc();
    let o = c.env_remove("THC_SETUP_LOGIN").args(["--json", "setup", "--step", "login"]).output().unwrap();
    let step: Value = serde_json::from_slice(&o.stdout).unwrap_or(Value::Null);
    assert_eq!(step["state"], "done", "{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    assert!(step["detail"].as_str().is_some_and(|d| d.contains("daemon restarted")), "{step}");
    let second = rig.wait_one();
    assert_ne!(first["pid"], second["pid"]);
    assert_eq!(second["exe_id"].as_str(), Some(rig.identity(&rig.bin).as_str()), "{second}");
    assert!(second["stale"].is_null(), "{second}");
    if !restart_call.is_empty() {
        assert!(rig.calls().contains(restart_call), "{}", rig.calls());
    }
    let doctor = rig.json(&["doctor"]);
    assert!(!doctor["issues"].to_string().contains("daemon"), "{doctor}");

    // Again with nothing to do: no restart.
    let mut c = rig.thc();
    let o = c.env_remove("THC_SETUP_LOGIN").args(["--json", "setup", "--step", "login"]).output().unwrap();
    let step: Value = serde_json::from_slice(&o.stdout).unwrap_or(Value::Null);
    assert_eq!(step["state"], "already", "{step}");
    assert_eq!(rig.wait_one()["pid"], second["pid"]);
}

#[test]
fn reinstall_under_launchd_restarts_the_old_daemon() {
    reinstall_restarts(Kind::Launchd, "launchd", "kickstart -k gui/");
}

#[test]
fn reinstall_under_systemd_restarts_the_old_daemon() {
    reinstall_restarts(Kind::Systemd, "systemd", "--user restart thc.service");
}

/// No login item: `thc daemon start` (setup's follow-up, or a person) restarts a plain daemon
/// that runs a replaced binary, instead of saying "already running".
#[test]
fn plain_daemon_on_a_replaced_binary_restarts_on_start() {
    let rig = Rig::new("plain", Kind::Plain, None);
    let o = rig.thc().args(["daemon", "start"]).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let first = rig.wait_one();
    let o = rig.thc().args(["daemon", "start"]).output().unwrap();
    assert!(String::from_utf8_lossy(&o.stdout).contains("already running"), "{}", String::from_utf8_lossy(&o.stdout));

    rig.reinstall();
    let o = rig.thc().args(["daemon", "start"]).output().unwrap();
    let said = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success() && said.contains("daemon restarted"), "{said}{}", String::from_utf8_lossy(&o.stderr));
    let second = rig.wait_one();
    assert_ne!(first["pid"], second["pid"]);
    assert_eq!(second["exe_id"].as_str(), Some(rig.identity(&rig.bin).as_str()), "{second}");
}

/// A login item that names another binary (an old install path) is pointed at this one, and
/// its daemon comes back on it, through launchd (bootout + bootstrap).
#[test]
fn install_points_the_login_item_at_this_binary() {
    let rig = Rig::new("repoint", Kind::Launchd, Some("old-thc"));
    let o = rig.thc().args(["daemon", "start"]).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let first = rig.wait_one();
    assert!(first["exe"].as_str().is_some_and(|e| e.ends_with("/old-thc")), "{first}");
    // This thc isn't the one the login item runs, but the daemon is current for its item.
    assert!(first["stale"].is_null(), "{first}");

    let v = rig.json(&["daemon", "install"]);
    assert!(v["note"].as_str().is_some_and(|n| n.contains("login item now runs")), "{v}");
    let second = rig.wait_one();
    assert_ne!(first["pid"], second["pid"]);
    assert_eq!(second["exe"].as_str().map(|e| Path::new(e).canonicalize().unwrap()), Some(rig.bin.clone()), "{second}");
    let plist = std::fs::read_to_string(rig.home.join("Library/LaunchAgents/dev.thought.thc.plist")).unwrap();
    assert!(plist.contains(&format!("<string>{}</string>", rig.bin.display())) && plist.contains(&rig.vault.display().to_string()), "{plist}");
    let calls = rig.calls();
    assert!(calls.contains("bootout") && calls.contains("bootstrap"), "{calls}");
}

/// Another registered vault with a daemon of its own (a plain `thc daemon start` there, which
/// the login daemon then leaves alone) is restarted onto the new binary by the same setup step.
#[test]
fn reinstall_restarts_other_vaults_own_daemons() {
    let rig = Rig::new("others", Kind::Launchd, None);
    let side = rig.root.join("side");
    let o = rig.thc().args(["vault", "new", "side"]).arg(&side).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    // Its own cache (the default for its path), as a plain start there would have.
    let side_thc = || {
        let mut c = rig.thc();
        c.env_remove("THC_CACHE_DIR").env("THC_SERVICE_MANAGER", "launchd").args(["--vault", "side"]);
        c
    };
    let side_status = || -> Value { serde_json::from_slice(&side_thc().args(["--json", "daemon", "status"]).output().unwrap().stdout).unwrap_or(Value::Null) };
    // The side daemon first, so the login daemon's thread for that vault finds it taken.
    let o = side_thc().args(["daemon", "start"]).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let side_first = side_status();
    let o = rig.thc().args(["daemon", "start"]).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let login_first = rig.wait_one();
    assert_ne!(side_first["pid"], login_first["pid"], "{side_first}");

    rig.reinstall();
    let doctor: Value = serde_json::from_slice(&side_thc().args(["--json", "doctor"]).output().unwrap().stdout).unwrap();
    assert!(doctor["issues"].to_string().contains("stale"), "{doctor}");

    let mut c = rig.thc();
    let o = c.env_remove("THC_SETUP_LOGIN").args(["--json", "setup", "--step", "login"]).output().unwrap();
    let step: Value = serde_json::from_slice(&o.stdout).unwrap_or(Value::Null);
    assert!(step["detail"].as_str().is_some_and(|d| d.contains("side: daemon restarted")), "{step}{}", String::from_utf8_lossy(&o.stderr));
    let side_second = side_status();
    assert_ne!(side_first["pid"], side_second["pid"]);
    assert_eq!(side_second["exe_id"].as_str(), Some(rig.identity(&rig.bin).as_str()), "{side_second}");
    let old = side_first["pid"].as_u64().unwrap() as i32;
    assert!(unsafe { libc::kill(old, 0) } != 0, "the old side daemon is gone");
    rig.wait_one();
    if let Some(p) = side_second["pid"].as_u64() {
        unsafe { libc::kill(p as i32, libc::SIGTERM) };
    }
}
