//! `thc doctor --fix` (data-model-review.md §1, §5): days and tags made twice before ids were
//! keyed merge into the oldest, empty tags go; a preview by default, one transaction with
//! `--yes`, and `thc undo` reverses it.

mod common;

use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use thc_core::event::{Actor, Op};
use thc_core::vault::{self, Paths, Vault};

fn thc(root: &Path, args: &[&str]) -> (i32, String, String) {
    let mut c = common::thc();
    c.args(args).current_dir(root).env("THC_VAULT", root.join("vault")).env("THC_CACHE_DIR", root.join("cache")).env_remove("THC_ACTOR");
    let o = c.output().unwrap();
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into(), String::from_utf8_lossy(&o.stderr).into())
}

/// A vault with the duplicates old versions could make: two roots for one day (each with a
/// line), two #lisbon tags (each tagging a line), and an empty tag.
fn seeded(name: &str) -> PathBuf {
    let root = thc_core::scratch::dir(&format!("thc-doctor-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    vault::init(&root.join("vault"), None, None).unwrap();
    let paths = Paths { vault: root.join("vault"), cache: root.join("cache") };
    let mut v = Vault::open(paths, Actor { kind: "human".into(), name: None }, "test").unwrap();
    let node = |id: &str, parent: Option<&str>, ord: &str, text: &str, title: Option<&str>, props: Value| Op::NodeCreate {
        id: id.into(),
        parent: parent.map(str::to_string),
        order: ord.into(),
        text: text.into(),
        title: title.map(str::to_string),
        props: props.as_object().cloned().unwrap_or_else(Map::new),
    };
    let ops = vec![
        node("aaaaaaaaaaa1", None, "a", "", None, json!({ "journal": "2026-10-04" })),
        node("aaaaaaaaaaa2", Some("aaaaaaaaaaa1"), "a", "from the first device #lisbon", None, json!({})),
        node("tttttttttt01", None, "b", "", Some("lisbon"), json!({ "tag": true })),
        Op::EdgeAdd { src: "aaaaaaaaaaa2".into(), rel: "tag".into(), dst: "tttttttttt01".into() },
        node("tttttttttt03", None, "d", "", Some(""), json!({ "tag": true })),
        Op::EdgeAdd { src: "aaaaaaaaaaa2".into(), rel: "tag".into(), dst: "tttttttttt03".into() },
    ];
    v.transact(|_| Ok((ops, ()))).unwrap();
    // The second device, later: its own root for the same day, and its own #lisbon.
    let ops = vec![
        node("bbbbbbbbbbb1", None, "c", "", None, json!({ "journal": "2026-10-04" })),
        node("bbbbbbbbbbb2", Some("bbbbbbbbbbb1"), "a", "from the second device #lisbon", None, json!({})),
        node("tttttttttt02", None, "e", "", Some("Lisbon"), json!({ "tag": true })),
        Op::EdgeAdd { src: "bbbbbbbbbbb2".into(), rel: "tag".into(), dst: "tttttttttt02".into() },
    ];
    std::thread::sleep(std::time::Duration::from_millis(5));
    v.transact(|_| Ok((ops, ()))).unwrap();
    root
}

#[test]
fn doctor_fix_previews_then_merges_in_one_undoable_transaction() {
    let root = seeded("fix");
    let r = &root;
    // The preview: what it would do, nothing written.
    let (code, out, err) = thc(r, &["doctor", "--fix"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("! 1 day appears twice (Sun Oct 4) · merge into the oldest"), "{out}");
    assert!(out.contains("! 1 tag appears twice (#lisbon, #Lisbon) · merge into the oldest"), "{out}");
    assert!(out.contains("! 1 empty tag (left by old heading markers) · remove"), "{out}");
    assert!(out.contains("checking ") && out.contains(" nodes"), "{out}");
    assert!(out.contains("would write 1 transaction · 2 merges · 1 removal\nnothing you wrote is lost · history keeps both ids"), "{out}");
    assert!(out.contains("thc doctor --fix --yes to apply · thc undo reverses it"), "{out}");

    // --yes: one transaction.
    let (code, out, err) = thc(r, &["doctor", "--fix", "--yes"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("merged 1 day and 1 tag, removed 1 empty tag in 1 transaction · thc undo --tx "), "{out}");
    let (_, j, _) = thc(r, &["--json", "q", "journal=2026-10-04"]);
    let v: Value = serde_json::from_str(&j).unwrap();
    let texts: Vec<&str> = v["items"].as_array().unwrap().iter().filter_map(|n| n["text"].as_str()).collect();
    assert!(texts.iter().any(|t| t.contains("first device")) && texts.iter().any(|t| t.contains("second device")), "both lines under one day: {j}");
    let (_, j, _) = thc(r, &["--json", "q", "#lisbon"]);
    assert_eq!(serde_json::from_str::<Value>(&j).unwrap()["items"].as_array().unwrap().len(), 2, "both lines carry the one tag: {j}");
    let (_, out, _) = thc(r, &["doctor", "--fix"]);
    assert!(out.contains("is healthy · nothing to fix"), "{out}");

    // One undo puts it all back.
    let (code, _, err) = thc(r, &["undo"]);
    assert_eq!(code, 0, "{err}");
    let (_, out, _) = thc(r, &["doctor", "--fix"]);
    assert!(out.contains("would write 1 transaction"), "undone: {out}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn agents_cant_repair() {
    let root = seeded("agent");
    let mut c = common::thc();
    c.args(["doctor", "--fix", "--yes"]).current_dir(&root).env("THC_VAULT", root.join("vault")).env("THC_CACHE_DIR", root.join("cache")).env("THC_ACTOR", "claude");
    let o = c.output().unwrap();
    assert_ne!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stdout));
    let _ = std::fs::remove_dir_all(&root);
}
