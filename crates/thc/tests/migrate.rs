//! `thc setup --migrate-from-app` on a scratch HOME: the link into the app becomes the standalone
//! binary, the app is asked to drop its login item, and setup finishes. launchd is never touched
//! (THC_SETUP_LOGIN=skip); the real /Applications is never looked at (THC_SETUP_APP).

mod common;

use std::path::Path;
use std::process::Command;

#[test]
fn migrating_from_the_app_replaces_the_link() {
    let real = Path::new(env!("CARGO_BIN_EXE_thc"));
    let home = std::env::temp_dir().join(format!("thc-migrate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let app = home.join("Applications/Thought Central.app");
    std::fs::create_dir_all(app.join("Contents/Helpers")).unwrap();
    std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
    std::fs::copy(real, app.join("Contents/Helpers/thc")).unwrap();
    let log = home.join("app.log");
    std::fs::write(app.join("Contents/MacOS/ThoughtBar"), format!("#!/bin/sh\necho \"$@\" >> '{}'\n", log.display())).unwrap();
    Command::new("chmod").arg("+x").arg(app.join("Contents/MacOS/ThoughtBar")).status().unwrap();
    std::fs::create_dir_all(home.join(".local/bin")).unwrap();
    std::os::unix::fs::symlink(app.join("Contents/Helpers/thc"), home.join(".local/bin/thc")).unwrap();
    // The app recorded its login item and the link in setup.json.
    std::fs::create_dir_all(home.join(".config/thought")).unwrap();
    std::fs::write(home.join(".config/thought/setup.json"), format!(r#"{{"version":"0.6.6","by":"app","items":[{{"piece":"login","kind":"app","path":"{}","created":true,"at":"x"}},{{"piece":"link","path":"{}","target":"{}","created":true,"at":"x"}}]}}"#, app.display(), home.join(".local/bin/thc").display(), app.join("Contents/Helpers/thc").display())).unwrap();
    // The downloaded standalone binary (install.sh unpacks it somewhere temporary).
    std::fs::create_dir_all(home.join("dl")).unwrap();
    std::fs::copy(real, home.join("dl/thc")).unwrap();

    let o = common::sandbox(&mut Command::new(home.join("dl/thc")))
        .args(["setup", "--migrate-from-app"])
        .env("HOME", &home)
        .env("THC_SETUP_LOGIN", "skip")
        .env("THC_SETUP_APP", &app)
        .env("THC_NO_UPDATE_CHECK", "1")
        .env("SHELL", "/bin/zsh")
        .env_remove("THC_VAULT")
        .env_remove("THC_CACHE_DIR")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        .env_remove("THC_ACTOR")
        .current_dir(&home)
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(o.status.success(), "{out}\n{err}");
    let link = home.join(".local/bin/thc");
    let meta = std::fs::symlink_metadata(&link).unwrap();
    assert!(meta.file_type().is_file(), "the link became the binary");
    let v = common::sandbox(&mut Command::new(&link)).arg("--version").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&v.stdout).trim(), format!("thc {}", env!("CARGO_PKG_VERSION")));
    assert!(std::fs::read_to_string(&log).unwrap_or_default().contains("--unregister-login-item"), "the app was asked to drop its login item");
    assert!(out.contains("delete"), "it says the app can go: {out}");
    assert!(home.join("thought").is_dir(), "setup made the vault");
    let rec = std::fs::read_to_string(home.join(".config/thought/setup.json")).unwrap();
    assert!(!rec.contains(r#""kind":"app""#), "the app's login record is gone: {rec}");
    // Running it again is harmless: already standalone. (THC_SETUP_APP stays on the fake app:
    // never let a test fall back to the real /Applications.)
    std::fs::remove_file(&log).unwrap();
    let again = common::sandbox(&mut Command::new(&link)).args(["setup", "--migrate-from-app"]).env("HOME", &home).env("THC_SETUP_LOGIN", "skip").env("THC_SETUP_APP", &app).env("THC_NO_UPDATE_CHECK", "1").env_remove("THC_VAULT").env_remove("CLAUDE_CONFIG_DIR").env_remove("CODEX_HOME").current_dir(&home).output().unwrap();
    assert!(again.status.success(), "{}", String::from_utf8_lossy(&again.stderr));
    let _ = std::fs::remove_dir_all(&home);
}
