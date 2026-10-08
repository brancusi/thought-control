//! Safety tiers and read-only mode (docs/design/policy.md §2).

mod common;

use serde_json::Value;
use std::path::Path;
use std::process::{Command, Stdio};

fn run(root: &Path, actor: &str, env: &[(&str, &str)], args: &[&str], stdin: Option<&str>) -> (i32, String, String) {
    let mut c = common::thc();
    c.args(args)
        .current_dir(root)
        .env("THC_VAULT", root.join("vault"))
        .env("THC_CACHE_DIR", root.join("cache"))
        .env("THC_CONFIG_DIR", root.join("cfg"))
        .env("THC_ACTOR", actor)
        .env_remove("THC_READONLY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        c.env(k, v);
    }
    let mut ch = c.spawn().unwrap();
    if let Some(s) = stdin {
        use std::io::Write;
        ch.stdin.take().unwrap().write_all(s.as_bytes()).unwrap();
    }
    let o = ch.wait_with_output().unwrap();
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into(), String::from_utf8_lossy(&o.stderr).into())
}

#[test]
fn tiers_refuse_loudly_and_write_nothing() {
    let root = thc_core::scratch::dir(&format!("thc-policy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("cfg")).unwrap();
    assert!(common::thc().current_dir(&root).args(["init", "vault"]).output().unwrap().status.success());
    let r = &root;
    let id = |out: &str| serde_json::from_str::<Value>(out).unwrap()["nodes"][0]["id"].as_str().unwrap().to_string();
    let (_, out, _) = run(r, "human", &[], &["--json", "add", "Human note"], None);
    let note = id(&out);
    let human_tx = serde_json::from_str::<Value>(&out).unwrap()["tx"].as_str().unwrap().to_string();
    let events = || std::fs::read_dir(r.join("vault/log")).unwrap().flatten().flat_map(|d| std::fs::read_dir(d.path()).unwrap().flatten()).map(|f| std::fs::read_to_string(f.path()).unwrap().lines().count()).sum::<usize>();

    // Default agent tier is write: adds work, rm doesn't.
    assert_eq!(run(r, "claude", &[], &["add", "Agent note"], None).0, 0);
    let before = events();
    let (code, _, err) = run(r, "claude", &[], &["rm", &note], None);
    assert_eq!(code, 6);
    assert!(err.starts_with("thc: claude can't rm (tier: write) · nothing written\nask the human to do it, or to change [actors.claude] in"), "{err}");
    let (_, _, err) = run(r, "claude", &[], &["--json", "rm", &note], None);
    let e: Value = serde_json::from_str(err.trim()).unwrap();
    assert_eq!(e["error"]["kind"], "denied");
    assert_eq!(e["error"]["verb"], "rm");
    assert!(e["error"]["config"].as_str().unwrap().ends_with("[actors.claude]"));
    // An apply line with rm is the delete class; undo of the human's tx is undo-others.
    let (code, _, err) = run(r, "claude", &[], &["apply", "-"], Some(&format!("{{\"cmd\":\"rm\",\"id\":\"{note}\"}}\n")));
    assert_eq!(code, 6, "{err}");
    assert!(err.contains("claude can't delete (apply line 1, tier: write)"), "{err}");
    let (code, _, err) = run(r, "claude", &[], &["undo", "--tx", &human_tx[human_tx.len() - 6..]], None);
    assert_eq!(code, 6, "{err}");
    assert!(err.contains("claude can't undo someone else's change (tier: write)"), "{err}");
    assert_eq!(events(), before, "refusals write nothing");

    // The human can do it; a dry run is allowed for any tier.
    std::fs::write(r.join("cfg/config.toml"), "[actors.codex]\ntier = \"read\"\n\n[actors.claude]\ntier = \"write\"\nconfirm = [\"done\"]\n").unwrap();
    let (code, _, err) = run(r, "codex", &[], &["add", "x"], None);
    assert_eq!(code, 6);
    assert!(err.contains("codex can't add (tier: read)"), "{err}");
    assert_eq!(run(r, "codex", &[], &["--dry-run", "add", "x"], None).0, 0);
    // Confirm: refused without --yes, done with it.
    let (_, out, _) = run(r, "claude", &[], &["--json", "todo", "Task"], None);
    let task = id(&out);
    let (code, _, err) = run(r, "claude", &[], &["done", &task], None);
    assert_eq!(code, 6);
    assert!(err.contains("done needs confirmation for claude · nothing written\nask the human first, then run it again with --yes"), "{err}");
    assert_eq!(run(r, "claude", &[], &["--yes", "done", &task], None).0, 0);

    // Read-only refuses everyone, the human included.
    let (code, _, err) = run(r, "human", &[("THC_READONLY", "1")], &["add", "x"], None);
    assert_eq!(code, 6);
    assert_eq!(err.trim(), "thc: read-only (THC_READONLY) · nothing written");
    let (_, _, err) = run(r, "human", &[], &["--readonly", "--json", "add", "x"], None);
    assert_eq!(serde_json::from_str::<Value>(err.trim()).unwrap()["error"]["kind"], "readonly");
    assert_eq!(run(r, "human", &[], &["rm", &note], None).0, 0);
    let _ = std::fs::remove_dir_all(&root);
}
