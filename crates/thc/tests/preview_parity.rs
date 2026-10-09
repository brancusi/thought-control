//! The capture preview (`thc parse`, the daemon's `parse`, ThoughtBar's preview line) must match
//! what a capture actually writes. Compared with `thc add --dry-run` on the same inputs.

mod common;

use serde_json::Value;
use std::path::Path;
use std::process::Command;

fn thc(root: &Path, args: &[&str]) -> Value {
    let out = common::thc()
        .args(args)
        .env("THC_VAULT", root.join("vault"))
        .env("THC_CACHE_DIR", root.join("cache"))
        .env_remove("THC_ACTOR")
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(s.lines().last().unwrap_or("null")).unwrap_or(Value::Null)
}

#[test]
fn preview_matches_what_add_writes() {
    let root = thc_core::scratch::dir(&format!("thc-parity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    assert!(common::thc().current_dir(&root).args(["init", "vault"]).output().unwrap().status.success());
    let inputs = [
        "Book flu shot #health sched:mon due:fri",
        "[ ] Pay rent at:\"nov 1 9am\" every:month !high",
        "Read [[Atomic Habits]] #books",
        "Dentist at:\"tue 2pm\"",
        "[/] Draft plan due:+3d every!:2w !low",
        "plain note",
    ];
    for text in inputs {
        let preview = thc(&root, &["--json", "parse", text]);
        let dry = thc(&root, &["--json", "add", "--dry-run", text]);
        let ops = dry["ops"].as_array().unwrap_or_else(|| panic!("no ops for {text}: {dry}"));
        let create = ops.iter().rev().find(|o| o["op"] == "node.create" && o["props"].get("journal").is_none() && o["props"].get("tag").is_none() && o.get("title").is_none()).unwrap_or_else(|| panic!("{text}: {dry}"));
        let props = &create["props"];
        let field = |k: &str| props.get(k).cloned().unwrap_or(Value::Null);
        assert_eq!(preview["status"].clone(), field("status"), "{text}: status");
        assert_eq!(preview["scheduled"].clone(), field("scheduled"), "{text}: scheduled");
        assert_eq!(preview["due"].clone(), field("due"), "{text}: due");
        assert_eq!(preview["priority"].clone(), field("priority"), "{text}: priority");
        assert_eq!(preview["repeat"].clone(), field("repeat").get("text").cloned().unwrap_or(Value::Null), "{text}: repeat");
        assert!(preview["bad"].as_array().unwrap().is_empty(), "{text}: no bad tokens");
        // Stored text keeps tags and turns [[Title]] into an id link; compare the words.
        let words = |s: &str| {
            let mut out = String::new();
            let mut rest = s;
            while let Some(i) = rest.find("[[") {
                out.push_str(&rest[..i]);
                rest = rest[i..].find("]]").map(|j| &rest[i + j + 2..]).unwrap_or("");
            }
            out.push_str(rest);
            out.split_whitespace().map(str::to_string).collect::<Vec<_>>()
        };
        let stored = create["text"].as_str().unwrap();
        assert_eq!(words(preview["text"].as_str().unwrap()), words(stored), "{text}: text");
    }
    // A bad value: the preview keeps it as text and flags it (as the daemon's capture saves it).
    let p = thc(&root, &["--json", "parse", "Call mom due:fryday"]);
    assert_eq!(p["bad"][0], "due:fryday");
    assert_eq!(p["text"], "Call mom due:fryday");
    assert!(p.get("due").is_none());
    let _ = std::fs::remove_dir_all(&root);
}
