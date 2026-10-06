//! Batch attach and area capture, always in scratch vaults with a fixture seam.
mod common;
use serde_json::Value;
use std::{
    path::Path,
    process::{Command, Output},
};

fn command(root: &Path) -> Command {
    let mut c = common::thc();
    c.current_dir(root)
        .env("THC_VAULT", root.join("vault"))
        .env("THC_CACHE_DIR", root.join("cache"))
        .env("THC_ACTOR", "codex-engineer-4");
    c
}
fn json(o: Output) -> Value {
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_slice(&o.stdout).unwrap()
}
fn run(root: &Path, args: &[&str]) -> Value {
    json(command(root).arg("--json").args(args).output().unwrap())
}
fn setup(name: &str) -> std::path::PathBuf {
    let root = common::root().join(name);
    std::fs::create_dir_all(&root).unwrap();
    run(&root, &["init", "vault"]);
    // Legacy dimensions-only PNG fixture: preparation must still accept existing attachments.
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend(4u32.to_be_bytes());
    png.extend(3u32.to_be_bytes());
    png.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
    std::fs::write(root.join("shot.png"), png).unwrap();
    root
}
fn task(root: &Path) -> String {
    run(root, &["todo", "Screenshot repro"])["nodes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn batch_attach_is_one_transaction_with_embeds_and_one_undo() {
    let root = setup("shot-batch");
    let parent = task(&root);
    std::fs::write(root.join("log.txt"), "trace").unwrap();
    let a = run(&root, &["attach", &parent, "shot.png", "log.txt"]);
    let items = a["attachments"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    let s = run(&root, &["show", &parent]);
    assert_eq!(s["attachments"].as_array().unwrap().len(), 2);
    for item in items {
        let file = run(&root, &["show", item["attachment"].as_str().unwrap()]);
        assert_eq!(file["embeds"][0]["id"], item["id"]);
        assert!(Path::new(item["abs"].as_str().unwrap()).is_file());
    }
    run(&root, &["undo", "--tx", a["tx"].as_str().unwrap()]);
    assert!(
        run(&root, &["show", &parent])["children"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        run(&root, &["q", "is:attachment"])["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn batch_validation_policy_and_preconditions_happen_before_file_storage() {
    let root = setup("shot-validation");
    let parent = task(&root);
    let o = command(&root)
        .args(["attach", &parent, "shot.png", "missing.txt"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(6));
    assert!(!root.join("vault/files").exists());
    let o = command(&root)
        .args(["--readonly", "attach", &parent, "shot.png"])
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(!root.join("vault/files").exists());
    let o = command(&root)
        .args(["--expect", "status=done", "attach", &parent, "shot.png"])
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(2),
        "a create cannot guard an existing node"
    );
    assert!(!root.join("vault/files").exists());
}

#[test]
fn screenshot_uses_the_attachment_path_and_journal_and_never_opens_capture_ui() {
    let root = setup("shot-capture");
    let parent = task(&root);
    let fixture = root.join("shot.png");
    let a = json(
        command(&root)
            .env("THC_SHOT_IMAGE", &fixture)
            .args([
                "--json",
                "shot",
                "--under",
                &parent,
                "--caption",
                "screen #literal",
            ])
            .output()
            .unwrap(),
    );
    assert_eq!((a["w"].as_u64(), a["h"].as_u64()), (Some(4), Some(3)));
    assert!(
        run(&root, &["show", &parent])["children"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("![screen #literal]")
    );
    let again = json(
        command(&root)
            .env("THC_SHOT_IMAGE", &fixture)
            .args(["--json", "shot", "--under", &parent])
            .output()
            .unwrap(),
    );
    assert_eq!(
        a["attachment"], again["attachment"],
        "same stored bytes/name produce one file node"
    );
    let j = json(
        command(&root)
            .env("THC_SHOT_IMAGE", &fixture)
            .args(["--json", "shot", "--journal", "2026-10-04"])
            .output()
            .unwrap(),
    );
    let note = run(&root, &["show", j["id"].as_str().unwrap()]);
    let day = run(&root, &["show", note["parent"].as_str().unwrap()]);
    assert_eq!(day["journal"], "2026-10-04");
    let o = command(&root)
        .args(["shot"])
        .env_remove("THC_SHOT_IMAGE")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(6));
    assert!(String::from_utf8_lossy(&o.stderr).contains("capture UI is disabled"));
}

#[test]
fn screenshot_cancel_dry_run_bad_destination_and_readonly_write_nothing() {
    let root = setup("shot-no-write");
    let cancelled = json(
        command(&root)
            .env("THC_SHOT_IMAGE", "")
            .args(["--json", "shot"])
            .output()
            .unwrap(),
    );
    assert_eq!(cancelled["cancelled"], true);
    run(&root, &["--dry-run", "shot"]);
    for args in [
        vec!["shot", "--under", "zzzzzzzzzzzz"],
        vec!["shot", "--journal", "fryday"],
        vec!["--readonly", "shot"],
        vec!["shot", "--under", "zzzz", "--journal", "today"],
    ] {
        assert!(!command(&root).args(args).output().unwrap().status.success());
    }
    assert!(!root.join("vault/files").exists());
    assert!(
        run(&root, &["q", "status:any"])["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn pdf_thumbnail_is_optional_cached_and_visible_in_show() {
    use std::os::unix::fs::PermissionsExt;
    let root = setup("shot-pdf");
    let parent = task(&root);
    std::fs::write(root.join("spec.pdf"), b"%PDF-fixture").unwrap();
    let renderer = root.join("pdftoppm-shim");
    let script = format!(
        "#!/bin/sh\nfor arg do last=\"$arg\"; done\nprintf '%s\\n' \"$*\" > '{}'\ncp '{}' \"$last.png\"\n",
        root.join("renderer.log").display(),
        root.join("shot.png").display()
    );
    std::fs::write(&renderer, script).unwrap();
    std::fs::set_permissions(&renderer, std::fs::Permissions::from_mode(0o700)).unwrap();
    let a = json(
        command(&root)
            .env("THC_PDF_RENDERER", &renderer)
            .args(["--json", "attach", &parent, "spec.pdf"])
            .output()
            .unwrap(),
    );
    let thumb = Path::new(a["thumbnail"].as_str().unwrap());
    assert!(thumb.is_file());
    assert!(thumb.starts_with(root.join("cache")));
    assert!(
        std::fs::read_to_string(root.join("renderer.log"))
            .unwrap()
            .starts_with("-f 1 -l 1 -singlefile -scale-to 640 -png")
    );
    assert_eq!(
        run(&root, &["show", &parent])["attachments"][0]["thumbnail"],
        a["thumbnail"]
    );
    assert_eq!(
        run(&root, &["show", a["attachment"].as_str().unwrap()])["file"]["thumbnail"],
        a["thumbnail"]
    );
    std::fs::remove_file(thumb).unwrap();
    let again = run(&root, &["attach", &parent, "spec.pdf"]);
    assert!(
        again.get("thumbnail").is_none(),
        "missing renderer never prevents attach"
    );
}
