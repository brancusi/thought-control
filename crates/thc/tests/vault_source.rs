//! Which vault a command uses, and why (doctor and daemon status), and the first-run hint.

mod common;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("thc-vs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    // /var is a link to /private/var, and thc sees the resolved cwd.
    d.canonicalize().unwrap()
}

/// thc with a clean environment: a temp HOME and no THC_* variables.
fn thc(home: &Path, cwd: &Path) -> Command {
    let mut c = common::thc();
    c.current_dir(cwd).env("HOME", home).env("THC_CACHE_DIR", home.join("cache"));
    for (k, _) in std::env::vars() {
        if k.starts_with("THC_") && k != "THC_CACHE_DIR" {
            c.env_remove(k);
        }
    }
    c
}

fn doctor_source(home: &Path, cwd: &Path) -> Value {
    let o = thc(home, cwd).args(["doctor", "--json"]).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_slice::<Value>(&o.stdout).unwrap()["vault_source"].clone()
}

#[test]
fn no_vault_suggests_the_default() {
    let home = tmp("none");
    let o = thc(&home, Path::new("/")).arg("today").output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("thc init ~/thought --global"), "{err}");
}

#[test]
fn doctor_says_where_the_vault_came_from() {
    let home = tmp("src");
    let o = thc(&home, &home).args(["init", "notes", "--global"]).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let s = doctor_source(&home, Path::new("/"));
    assert_eq!(s["kind"], "global");
    assert!(s["text"].as_str().unwrap().contains("~/.config/thought/config.toml"), "{s}");

    // A project .thc.toml wins over the global vault, and says so.
    let repo = home.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    assert!(thc(&home, &repo).arg("init").output().unwrap().status.success());
    let s = doctor_source(&home, &repo);
    assert_eq!(s["kind"], "project");
    assert_eq!(s["text"], "from ~/repo/.thc.toml");

    let o = thc(&home, Path::new("/")).env("THC_VAULT", home.join("notes")).args(["doctor", "--json"]).output().unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&o.stdout).unwrap()["vault_source"]["kind"], "env");
    let o = thc(&home, Path::new("/")).args(["--vault"]).arg(home.join("notes")).args(["doctor", "--json"]).output().unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&o.stdout).unwrap()["vault_source"]["kind"], "flag");
}

#[test]
fn daemon_status_offline_names_the_vault() {
    let home = tmp("dst");
    assert!(thc(&home, &home).args(["init", "notes", "--global"]).output().unwrap().status.success());
    let o = thc(&home, Path::new("/")).args(["daemon", "status", "--json"]).output().unwrap();
    assert_eq!(o.status.code(), Some(3));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["state"], "offline");
    assert_eq!(v["vault_source"]["kind"], "global");
    assert!(v["vault"].as_str().unwrap().ends_with("notes"));
}

#[test]
fn parse_needs_no_vault() {
    // The panel and the editor preview capture text before a vault may exist, or with a stale
    // .thc.toml nearby: parse is pure.
    let home = tmp("parse");
    let o = thc(&home, Path::new("/")).env("THC_VAULT", home.join("nope")).args(["parse", "--json", "Call mom due:fri"]).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["text"], "Call mom");
    assert!(v["due"].is_string());
}

#[test]
fn init_never_writes_a_project_config_in_a_shared_folder() {
    // The home folder stands in for /tmp (same rule; a test must never try the real /tmp).
    let home = tmp("shared");
    let o = thc(&home, &home).arg("init").output().unwrap();
    assert_eq!(o.status.code(), Some(2), "{}", String::from_utf8_lossy(&o.stdout));
    assert!(String::from_utf8_lossy(&o.stderr).contains("won't write .thc.toml"));
    assert!(!home.join(".thc.toml").exists());
}

#[test]
fn a_project_config_is_named_wherever_it_chooses_the_vault() {
    let home = tmp("named");
    let repo = home.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    assert!(thc(&home, &repo).arg("init").output().unwrap().status.success());
    let sub = repo.join("src");
    std::fs::create_dir_all(&sub).unwrap();

    let o = thc(&home, &sub).arg("prime").output().unwrap();
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(s.contains("vault repo ~/repo/vault (from ~/repo/.thc.toml)"), "{s}");
    let o = thc(&home, &sub).arg("today").output().unwrap();
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(s.contains("(from ~/repo/.thc.toml)"), "{s}");
    let o = thc(&home, &sub).args(["today", "--json"]).output().unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&o.stdout).unwrap()["vault_source"]["kind"], "project");
    let o = thc(&home, &sub).env("THC_TUI_SNAPSHOT", "100x30").arg("tui").output().unwrap();
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(s.contains("(from ~/repo/.thc.toml)"), "{s}");

    // Pointing at a vault that's gone is an error that names the file.
    std::fs::remove_dir_all(repo.join("vault")).unwrap();
    let o = thc(&home, &sub).arg("today").output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("~/repo/.thc.toml points at ~/repo/vault, which isn't a vault"), "{err}");
}
