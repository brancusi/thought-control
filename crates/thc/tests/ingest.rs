//! drop/ ingest is lenient: nobody reads an exit code there, so a bad token is kept as text,
//! reported, and shown by `thc doctor`. No item is ever dropped.

mod common;

use serde_json::Value;
use std::path::Path;
use std::process::Command;

fn thc(root: &Path, args: &[&str]) -> (Value, String) {
    let out = common::thc()
        .args(args)
        .env("THC_VAULT", root.join("vault"))
        .env("THC_CACHE_DIR", root.join("cache"))
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    (serde_json::from_str(s.lines().last().unwrap_or("null")).unwrap_or(Value::Null), String::from_utf8_lossy(&out.stderr).into())
}

#[test]
fn bad_tokens_in_dropped_files_are_kept_and_reported() {
    let root = thc_core::scratch::dir(&format!("thc-ingest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    assert!(common::thc().current_dir(&root).args(["init", "vault"]).output().unwrap().status.success());
    std::fs::create_dir_all(root.join("vault/drop")).unwrap();
    std::fs::write(root.join("vault/drop/notes.md"), "- Call mom due:fryday\n- Buy milk due:fri\n\nA plain paragraph.\n").unwrap();
    let (v, err) = thc(&root, &["--json", "ingest"]);
    assert_eq!(v["created"].as_array().map(|a| a.len()), Some(3), "every item becomes a node: {v} {err}");
    assert!(err.contains("kept \"due:fryday\" as text"), "{err}");
    let (inbox, _) = thc(&root, &["--json", "inbox"]);
    let texts: Vec<&str> = inbox["items"].as_array().unwrap().iter().filter_map(|n| n["text"].as_str()).collect();
    assert!(texts.contains(&"Call mom due:fryday"), "{texts:?}");
    let (doc, _) = thc(&root, &["--json", "doctor"]);
    let notices: Vec<&str> = doc["notices"].as_array().unwrap().iter().filter_map(|n| n.as_str()).collect();
    assert!(notices.iter().any(|n| n.contains("drop/notes.md: kept \"due:fryday\" as text")), "{notices:?}");
    let _ = std::fs::remove_dir_all(&root);
}
