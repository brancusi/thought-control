//! Write preconditions (agents.md §3): `--if-match <rev>` and `--expect field=value`.

mod common;

use serde_json::Value;
use std::path::Path;
use std::process::Command;

fn thc(root: &Path, actor: &str, args: &[&str]) -> (i32, String, String) {
    let out = common::thc()
        .args(args)
        .env("THC_VAULT", root.join("vault"))
        .env("THC_CACHE_DIR", root.join("cache"))
        .env("THC_ACTOR", actor)
        .output()
        .unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into(), String::from_utf8_lossy(&out.stderr).into())
}

fn json(s: &str) -> Value {
    serde_json::from_str(s.lines().last().unwrap_or("")).unwrap_or_else(|e| panic!("not JSON ({e}): {s}"))
}

#[test]
fn stale_writes_are_refused_and_fresh_ones_go_through() {
    let root = std::env::temp_dir().join(format!("thc-pre-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    assert!(common::thc().current_dir(&root).args(["init", "vault"]).output().unwrap().status.success());

    let (_, out, _) = thc(&root, "claude", &["--json", "todo", "Guarded"]);
    let node = json(&out)["nodes"][0].clone();
    let (id, rev) = (node["id"].as_str().unwrap().to_string(), node["rev"].as_str().unwrap().to_string());

    // Someone else changes it: the stale write is refused with what changed, and nothing is written.
    thc(&root, "human", &["set", &id, "status=waiting"]);
    let (code, _, err) = thc(&root, "claude", &["--json", "set", &id, "status=done", "--if-match", &rev]);
    assert_eq!(code, 4);
    let e = json(&err)["error"].clone();
    assert_eq!(e["kind"], "stale");
    assert_eq!(e["changed"][0]["field"], "status");
    assert_eq!(e["changed"][0]["from"], "todo");
    assert_eq!(e["changed"][0]["to"], "waiting");
    let (_, out, _) = thc(&root, "claude", &["--json", "show", &id]);
    let now = json(&out);
    assert_eq!(now["status"], "waiting", "nothing was written");
    assert_eq!(e["rev"], now["rev"]);

    // With the current rev it goes through.
    let (code, _, _) = thc(&root, "claude", &["set", &id, "priority=high", "--if-match", now["rev"].as_str().unwrap()]);
    assert_eq!(code, 0);

    // --expect
    let (code, _, err) = thc(&root, "claude", &["set", &id, "status=done", "--expect", "status=todo"]);
    assert_eq!(code, 4);
    assert!(err.contains("expected status=todo, found waiting"), "{err}");
    let (code, _, _) = thc(&root, "claude", &["done", &id, "--expect", "status=open"]);
    assert_eq!(code, 0);

    // A rev that isn't this node's, or isn't a rev at all.
    let (_, out, _) = thc(&root, "claude", &["--json", "todo", "Other"]);
    let other_rev = json(&out)["nodes"][0]["rev"].as_str().unwrap().to_string();
    assert_eq!(thc(&root, "claude", &["set", &id, "priority=low", "--if-match", &other_rev]).0, 6);
    assert_eq!(thc(&root, "claude", &["set", &id, "priority=low", "--if-match", "nope"]).0, 6);
    let _ = std::fs::remove_dir_all(&root);
}
