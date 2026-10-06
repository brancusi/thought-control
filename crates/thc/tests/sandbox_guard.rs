//! The guard works: with THC_TEST=1, pointing thc at the real home or the real launchctl stops it
//! with exit 70 before anything is read or written.

mod common;

use std::process::Command;

fn real_home() -> String {
    let o = Command::new("sh").args(["-c", "eval echo ~$(id -un)"]).output().unwrap();
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

#[test]
fn a_real_home_is_refused() {
    let mut c = common::thc();
    // Undo the sandbox's HOME and config dir: thc must refuse on its own.
    let o = c.env("HOME", real_home()).env_remove("THC_CONFIG_DIR").env_remove("XDG_CACHE_HOME").env_remove("THC_TEST_ALLOW").env_remove("THC_VAULT").current_dir(common::root()).args(["--json", "today"]).output().unwrap();
    assert_eq!(o.status.code(), Some(70), "stderr: {}", String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8_lossy(&o.stderr).contains("THC_TEST: refusing"));
}

#[test]
fn the_real_launchctl_is_refused() {
    let root = common::root().join("guard-launchctl");
    std::fs::create_dir_all(&root).unwrap();
    assert!(common::thc().current_dir(&root).args(["init", "vault"]).output().unwrap().status.success());
    // No shims on PATH: launchctl would be the real one.
    let o = common::thc().env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin").env("THC_VAULT", root.join("vault")).env("THC_CACHE_DIR", root.join("cache")).current_dir(&root).args(["daemon", "install"]).output().unwrap();
    if cfg!(target_os = "macos") {
        assert_eq!(o.status.code(), Some(70), "stderr: {}", String::from_utf8_lossy(&o.stderr));
        assert!(String::from_utf8_lossy(&o.stderr).contains("the real launchctl"));
    }
}
