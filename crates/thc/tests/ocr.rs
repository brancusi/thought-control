//! Explicit attachment/backfill OCR, with scratch-only files and the normal write policy.
mod common;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};
fn command(root: &Path) -> Command {
    let mut c = common::thc();
    c.current_dir(root)
        .env("THC_VAULT", root.join("vault"))
        .env("THC_CACHE_DIR", root.join("cache"))
        .env("THC_ACTOR", "codex-engineer-2");
    c
}
fn json(o: Output) -> Value {
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_slice(&o.stdout).unwrap()
}
fn run(root: &Path, args: &[&str]) -> Value {
    json(command(root).arg("--json").args(args).output().unwrap())
}
fn setup(name: &str) -> PathBuf {
    let root = common::root().join(name);
    std::fs::create_dir_all(&root).unwrap();
    run(&root, &["init", "vault"]);
    std::fs::write(
        root.join("screenshot.png"),
        include_bytes!("../../thc-core/tests/fixtures/ocr-screenshot.png"),
    )
    .unwrap();
    root
}
fn prop(root: &Path, id: &str, value: Value) {
    // Custom JSON objects are log values; the CLI's generic set accepts scalar text.
    let mut vault = thc_core::vault::Vault::open(
        thc_core::vault::Paths {
            vault: root.join("vault"),
            cache: root.join("cache"),
        },
        thc_core::event::Actor {
            kind: "agent".into(),
            name: Some("codex-engineer-2".into()),
        },
        "ocr-fixture",
    )
    .unwrap();
    vault
        .transact(|_| {
            Ok((
                vec![thc_core::event::Op::NodeSet {
                    id: id.into(),
                    props: json!({"ocr":value}).as_object().unwrap().clone(),
                }],
                (),
            ))
        })
        .unwrap();
}

#[test]
fn attach_json_search_and_duplicate_content() {
    let root = setup("ocr-attach");
    let parent = run(&root, &["add", "ordinary context"])["nodes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    run(&root, &["--dry-run", "attach", &parent, "screenshot.png"]);
    assert!(!root.join("vault/files").exists());
    let first = run(
        &root,
        &[
            "attach",
            &parent,
            "screenshot.png",
            "--caption",
            "Sprint screenshot",
        ],
    );
    let aid = first["attachment"].as_str().unwrap();
    let note = first["id"].as_str().unwrap();
    if thc_core::ocr::available() {
        assert!(first["ocr"]["text"].as_str().unwrap().contains("CGTN-2048"));
        let image = run(&root, &["show", aid]);
        assert_eq!(image["ocr"], first["ocr"]);
        let parent_json = run(&root, &["show", &parent]);
        assert_eq!(parent_json["attachments"][0]["ocr"], first["ocr"]);
        let found = run(&root, &["search", "CGTN-2048"]);
        assert_eq!(found["count"], 1);
        assert_eq!(found["items"][0]["id"], note);
        assert!(
            found["items"][0]["image_matches"][0]["snippet"]
                .as_str()
                .unwrap()
                .contains("CGTN-2048")
        );
        let human = command(&root)
            .args(["search", "CGTN-2048"])
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&human.stdout).contains("in image: Sprint screenshot"));
        assert_eq!(run(&root, &["q", "text:CGTN-2048"])["count"], 1);
        assert_eq!(
            run(&root, &["q", "is:image text:CGTN-2048"])["items"][0]["id"],
            aid
        );
        std::fs::copy(root.join("screenshot.png"), root.join("different-name.png")).unwrap();
        let second = run(&root, &["attach", &parent, "different-name.png"]);
        assert_ne!(second["attachment"], first["attachment"]);
        assert_eq!(second["ocr"], first["ocr"]);
    } else {
        assert!(first.get("ocr").is_none());
    }
    // Invalid image data still attaches; no success marker means backfill can retry.
    std::fs::write(root.join("broken.png"), b"broken").unwrap();
    let broken = run(&root, &["attach", &parent, "broken.png"]);
    assert!(broken.get("ocr").is_none());
    assert!(
        root.join("vault")
            .join(broken["path"].as_str().unwrap())
            .exists()
    );
}
#[test]
fn backfill_preview_reuse_policy_and_idempotence() {
    let root = setup("ocr-backfill");
    let parent = run(&root, &["add", "note"])["nodes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let first = run(&root, &["attach", &parent, "screenshot.png"]);
    let aid = first["attachment"].as_str().unwrap();
    std::fs::copy(root.join("screenshot.png"), root.join("copy.png")).unwrap();
    let second = run(&root, &["attach", &parent, "copy.png"]);
    let other = second["attachment"].as_str().unwrap();
    // A known stored result lets the content reuse/backfill contract run on every platform.
    let bytes = std::fs::read(root.join("vault").join(first["path"].as_str().unwrap())).unwrap();
    let value = json!({"hash":thc_core::ocr::hash(&bytes),"engine":thc_core::ocr::ENGINE,"text":"Quasar synthetic marker"});
    prop(&root, other, value.clone());
    prop(&root, aid, Value::Null);
    assert_eq!(run(&root, &["ocr"])["pending"], 1);
    assert_eq!(run(&root, &["--yes", "--dry-run", "ocr"])["pending"], 1);
    assert!(run(&root, &["show", aid]).get("ocr").is_none());
    let denied = command(&root)
        .args(["--readonly", "--yes", "ocr"])
        .output()
        .unwrap();
    assert!(!denied.status.success());
    assert!(run(&root, &["show", aid]).get("ocr").is_none());
    let backfill = run(&root, &["--yes", "ocr"]);
    assert_eq!(backfill["recognized"], 1);
    assert_eq!(backfill["skipped"], json!([]));
    assert_eq!(run(&root, &["show", aid])["ocr"], value);
    let rev = run(&root, &["show", aid])["rev"].clone();
    let repeat = run(&root, &["--yes", "ocr"]);
    assert_eq!(repeat["recognized"], 0);
    assert!(repeat["tx"].is_null());
    assert_eq!(run(&root, &["show", aid])["rev"], rev);
    assert_eq!(run(&root, &["search", "quasar"])["count"], 2);
    let tx = backfill["tx"].as_str().unwrap();
    run(&root, &["undo", "--tx", tx]);
    assert!(run(&root, &["show", aid]).get("ocr").is_none());
    assert_eq!(run(&root, &["search", "quasar"])["count"], 1);
}
