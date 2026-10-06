//! The login daemon under launchd: start and restart go through launchctl, and a restart ends
//! with exactly one daemon, on the new binary. `launchctl` here is a shim that behaves like
//! launchd for one job (kickstart starts it, kickstart -k stops it first), so the test can
//! count processes.
#![cfg(target_os = "macos")]

mod common;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

struct Rig {
    root: PathBuf,
    home: PathBuf,
    vault: PathBuf,
    shims: PathBuf,
}

impl Rig {
    fn new(name: &str) -> Rig {
        let root = std::env::temp_dir().join(format!("thc-launchd-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (home, shims, state) = (root.join("home"), root.join("shims"), root.join("state"));
        for d in [&home, &shims, &state] {
            std::fs::create_dir_all(d).unwrap();
        }
        let root = root.canonicalize().unwrap();
        let (home, shims, state) = (root.join("home"), root.join("shims"), root.join("state"));
        let vault = root.join("vault");
        let cache = root.join("cache");
        let bin = env!("CARGO_BIN_EXE_thc");
        // launchd for one job: its pid in state/pid, every pid it ever started in state/all.
        let shim = format!(
            r#"#!/bin/sh
S="{state}"
echo "launchctl $*" >> "$S/calls"
case "$1" in
  kickstart)
    pid=$(cat "$S/pid" 2>/dev/null)
    if [ "$2" = "-k" ]; then
      if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        kill "$pid"; n=0
        while kill -0 "$pid" 2>/dev/null && [ $n -lt 100 ]; do sleep 0.05; n=$((n+1)); done
      fi
    elif [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
      exit 0
    fi
    THC_VAULT="{vault}" THC_CACHE_DIR="{cache}" nohup "{bin}" daemon run < /dev/null >> "$S/daemon.log" 2>&1 &
    echo $! > "$S/pid"; echo $! >> "$S/all"
    ;;
esac
exit 0
"#,
            state = state.display(),
            vault = vault.display(),
            cache = cache.display(),
        );
        std::fs::write(shims.join("launchctl"), shim).unwrap();
        Command::new("chmod").arg("+x").arg(shims.join("launchctl")).status().unwrap();
        let rig = Rig { root, home, vault, shims };
        let o = rig.thc(&rig.root).args(["init"]).arg(&rig.vault).arg("--global").output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        // thc's LaunchAgent, as `thc daemon install` writes it.
        let agents = rig.home.join("Library/LaunchAgents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("dev.thought.thc.plist"),
            format!(
                "<plist><dict><key>Label</key><string>dev.thought.thc</string><key>EnvironmentVariables</key><dict><key>THC_VAULT</key><string>{}</string><key>THC_CACHE_DIR</key><string>{}</string></dict></dict></plist>",
                rig.vault.display(),
                cache.display()
            ),
        )
        .unwrap();
        rig
    }

    fn thc(&self, cwd: &Path) -> Command {
        let mut c = common::thc();
        let path = format!("{}:{}", self.shims.display(), std::env::var("PATH").unwrap_or_default());
        // The cache the plist names (a real install writes the one thc resolves).
        c.current_dir(cwd).env("HOME", &self.home).env("PATH", path).env("THC_TEST_ROOT", &self.root).env("THC_CACHE_DIR", self.root.join("cache")).env_remove("THC_VAULT");
        c
    }

    /// Every daemon the fake launchd started that's still alive.
    fn alive(&self) -> Vec<u32> {
        let all = std::fs::read_to_string(self.root.join("state/all")).unwrap_or_default();
        all.lines().filter_map(|l| l.trim().parse().ok()).filter(|p: &u32| unsafe { libc::kill(*p as i32, 0) } == 0).collect()
    }

    fn status(&self) -> Value {
        let o = self.thc(&self.root).args(["--json", "daemon", "status"]).output().unwrap();
        serde_json::from_slice(&o.stdout).unwrap_or(Value::Null)
    }

    fn wait_one(&self) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let a = self.alive();
            if a.len() == 1 && self.status()["pid"].as_u64() == Some(a[0] as u64) {
                return a[0];
            }
            assert!(Instant::now() < deadline, "expected one daemon, alive: {a:?}, status: {}", self.status());
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

#[test]
fn restart_under_launchd_leaves_exactly_one_daemon() {
    let rig = Rig::new("restart");
    let o = rig.thc(&rig.root).args(["daemon", "start"]).output().unwrap();
    assert!(o.status.success(), "{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    let first = rig.wait_one();

    // Restart: launchd (kickstart -k) replaces the process; thc never spawns one beside it.
    let o = rig.thc(&rig.root).args(["daemon", "restart"]).output().unwrap();
    let said = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success() && said.contains("daemon restarted"), "{said}{}", String::from_utf8_lossy(&o.stderr));
    let second = rig.wait_one();
    assert_ne!(first, second);
    let calls = std::fs::read_to_string(rig.root.join("state/calls")).unwrap();
    assert!(calls.contains("kickstart -k gui/"), "{calls}");

    // The update's second half, run by the new binary from a folder whose .thc.toml points at
    // another vault: it still restarts the login daemon, and checks the version it came back on.
    let other = rig.root.join("elsewhere");
    std::fs::create_dir_all(&other).unwrap();
    assert!(rig.thc(&other).args(["init", "v2"]).output().unwrap().status.success());
    let o = rig.thc(&other).args(["update", "--finish", env!("CARGO_PKG_VERSION")]).output().unwrap();
    let v: Value = serde_json::from_slice(&o.stdout).unwrap_or(Value::Null);
    assert_eq!(v["daemon"], "daemon restarted on the new thc", "{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    let third = rig.wait_one();
    assert_ne!(second, third);

    // A version that isn't what came back is said loudly.
    let o = rig.thc(&other).args(["update", "--finish", "99.0.0"]).output().unwrap();
    let v: Value = serde_json::from_slice(&o.stdout).unwrap_or(Value::Null);
    assert!(v["daemon"].as_str().is_some_and(|d| d.starts_with("! the daemon still runs thc")), "{v}");
    rig.wait_one();
}
