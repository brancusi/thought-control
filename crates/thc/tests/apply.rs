//! `thc apply` (agents.md §4): one transaction or nothing; `$name` references; `thc import -`;
//! `is:ready` (§5).

mod common;

use serde_json::Value;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn thc(root: &Path, args: &[&str], stdin: &str) -> (i32, Value) {
    let mut child = common::thc()
        .args(["--json"])
        .args(args)
        .env("THC_VAULT", root.join("vault"))
        .env("THC_CACHE_DIR", root.join("cache"))
        .env("THC_ACTOR", "claude")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let v = serde_json::from_str(text.lines().last().unwrap_or("null")).unwrap_or(Value::Null);
    (out.status.code().unwrap_or(-1), v)
}

fn count(root: &Path, q: &str) -> usize {
    thc(root, &["q", q], "").1["items"].as_array().map(|a| a.len()).unwrap_or(0)
}

#[test]
fn a_batch_is_one_transaction_or_nothing() {
    let root = std::env::temp_dir().join(format!("thc-apply-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    assert!(common::thc().current_dir(&root).args(["init", "vault"]).output().unwrap().status.success());

    // One bad line: nothing at all is written, and every bad line is reported.
    let bad = "{\"cmd\":\"todo\",\"text\":\"Ok task\"}\n{\"cmd\":\"set\",\"id\":\"zzzzz\",\"props\":{\"due\":\"fri\"}}\n{\"cmd\":\"todo\",\"text\":\"x\",\"due\":\"fryday\"}\n";
    let (code, v) = thc(&root, &["apply", "-"], bad);
    assert_eq!(code, 6);
    assert_eq!(v["invalid"], 2);
    assert_eq!(v["errors"][0]["line"], 2);
    assert_eq!(count(&root, "Ok task"), 0, "all or nothing");

    // References between lines, and a single transaction.
    let plan = "{\"cmd\":\"add\",\"text\":\"Offsite\",\"inbox\":true,\"as\":\"off\"}\n\
                {\"cmd\":\"todo\",\"text\":\"Book the venue\",\"under\":\"$off\",\"as\":\"venue\"}\n\
                {\"cmd\":\"todo\",\"text\":\"Send invites\",\"under\":\"$off\",\"as\":\"invites\"}\n\
                {\"cmd\":\"link\",\"a\":\"$venue\",\"b\":\"$invites\",\"rel\":\"blocks\"}\n\
                {\"cmd\":\"set\",\"id\":\"$venue\",\"props\":{\"priority\":\"high\"}}\n";
    let (code, dry) = thc(&root, &["apply", "-", "--dry-run"], plan);
    assert_eq!(code, 0);
    assert_eq!(dry["ops"], 5);
    assert_eq!(count(&root, "Offsite"), 0, "dry run writes nothing");
    let (code, v) = thc(&root, &["apply", "-"], plan);
    assert_eq!(code, 0, "{v}");
    let tx = v["tx"].as_str().unwrap().to_string();
    let (_, log) = thc(&root, &["log"], "");
    let txs: std::collections::HashSet<&str> = log["events"].as_array().unwrap().iter().filter_map(|e| e["tx"].as_str()).collect();
    assert_eq!(txs.len(), 1, "one transaction");
    let venue = v["lines"][1]["nodes"][0].as_str().unwrap().to_string();
    let (_, show) = thc(&root, &["show", &venue], "");
    assert_eq!(show["priority"], "high");

    // is:ready: the venue is ready, the invites are blocked by it.
    let (_, ready) = thc(&root, &["q", "is:ready"], "");
    let texts: Vec<&str> = ready["items"].as_array().unwrap().iter().filter_map(|n| n["text"].as_str()).collect();
    assert!(texts.contains(&"Book the venue") && !texts.contains(&"Send invites"), "{texts:?}");

    // One review item, one undo.
    let (_, review) = thc(&root, &["review"], "");
    assert_eq!(review["pending"], 1);
    let short = &tx[tx.len() - 6..];
    assert_eq!(thc(&root, &["undo", "--tx", short], "").0, 0);
    assert_eq!(count(&root, "Book the venue"), 0);

    // Outline import.
    let (code, v) = thc(&root, &["import", "-", "--page", "Lisbon"], "- Trip prep\n    - [ ] Renew passport\n- Packing\n");
    assert_eq!(code, 0);
    assert_eq!(v["created"], 3);
    let _ = std::fs::remove_dir_all(&root);
}
