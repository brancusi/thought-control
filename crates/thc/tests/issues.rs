//! Issues (docs/design/issues.md): an agent's `done` waits in To review until a person
//! accepts it (`is:to-review`, derived, not a status).

mod common;

use serde_json::Value;

fn thc(root: &std::path::Path, actor: &str, args: &[&str]) -> String {
    let o = common::thc().args(args).current_dir(root).env("THC_VAULT", root.join("vault")).env("THC_CACHE_DIR", root.join("cache")).env("THC_ACTOR", actor).output().unwrap();
    assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into()
}

fn texts(root: &std::path::Path, q: &str) -> Vec<String> {
    let v: Value = serde_json::from_str(&thc(root, "human", &["--json", "q", q])).unwrap();
    v["items"].as_array().unwrap().iter().map(|i| i["text"].as_str().unwrap().to_string()).collect()
}

#[test]
fn i5_an_agents_done_waits_in_to_review_until_accepted() {
    let root = thc_core::scratch::dir(&format!("thc-issues-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    thc(&root, "human", &["init", "vault"]);
    let id = |out: String| serde_json::from_str::<Value>(&out).unwrap()["nodes"][0]["id"].as_str().unwrap().to_string();
    let a = id(thc(&root, "human", &["--json", "todo", "fixed by claude"]));
    let b = id(thc(&root, "human", &["--json", "todo", "fixed by me"]));
    thc(&root, "claude", &["done", &a]);
    thc(&root, "human", &["done", &b]);
    assert_eq!(texts(&root, "is:to-review"), ["fixed by claude"]);
    // A person accepts it in the review lane: it's plain done now.
    thc(&root, "human", &["review"]);
    thc(&root, "human", &["review", "accept", "1"]);
    assert!(texts(&root, "is:to-review").is_empty());
    assert_eq!(texts(&root, "status:done").len(), 2);
    // Reopened and done again by the agent: back in To review.
    thc(&root, "claude", &["reopen", &a]);
    thc(&root, "claude", &["done", &a]);
    assert_eq!(texts(&root, "is:to-review"), ["fixed by claude"]);
    let _ = std::fs::remove_dir_all(&root);
}

/// I2 (issues.md §1): an issue opens as its own document, its task line the header and
/// its children the body; writing there adds children; ⌃O on its line in a page opens it too,
/// and Esc goes back to that page.
#[test]
fn i2_an_issue_opens_as_a_document() {
    let root = thc_core::scratch::dir(&format!("thc-issues-doc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    thc(&root, "human", &["init", "vault"]);
    let id = |out: String| serde_json::from_str::<Value>(&out).unwrap()["nodes"][0]["id"].as_str().unwrap().to_string();
    let page = id(thc(&root, "human", &["--json", "page", "new", "Issues"]));
    let issue = id(thc(&root, "human", &["--json", "todo", "Shift drops capitals", "--under", &page, "-p", "high"]));
    thc(&root, "human", &["set", &issue, "owner=claude"]);
    thc(&root, "human", &["add", "--plain", "--under", &issue, "Steps: type Shift-A"]);
    let snap = |args: &[&str], keys: &str, write: bool| {
        let mut c = common::thc();
        c.args(args).current_dir(&root).env("THC_VAULT", root.join("vault")).env("THC_CACHE_DIR", root.join("cache")).env("THC_ACTOR", "human").env("THC_TUI_SNAPSHOT", "110x30").env("THC_TUI_KEYS", keys);
        if write {
            c.env("THC_TUI_SNAPSHOT_WRITE", "1");
        }
        String::from_utf8_lossy(&c.output().unwrap().stdout).to_string()
    };
    // From Tasks, Enter on the issue: the header is the task line with its meta.
    let f = snap(&["tui"], "3<cr>", false);
    assert!(f.contains("¶ Issues › Shift drops capitals") || f.contains("[ ] Shift drops capitals"), "{f}");
    assert!(f.contains("◆ claude · !high · opened"), "{f}");
    assert!(f.contains("Steps: type Shift-A"), "{f}");
    // Writing in it adds a child of the issue.
    snap(&["tui"], "3<cr><c-end><cr>Second finding<esc>", true);
    let kids: Value = serde_json::from_str(&thc(&root, "human", &["--json", "q", &format!("parent:{issue}")])).unwrap();
    let texts: Vec<&str> = kids["items"].as_array().unwrap().iter().map(|i| i["text"].as_str().unwrap()).collect();
    assert!(texts.contains(&"Second finding"), "{texts:?}");
    // ⌃O on its line in ¶ Issues opens it; Esc comes back to ¶ Issues.
    let f = snap(&["p", "Issues", "--no-focus"], "<c-home><c-o>", false);
    assert!(f.contains("◆ claude · !high"), "{f}");
    let f = snap(&["p", "Issues", "--no-focus"], "<c-home><c-o><esc>", false);
    assert!(f.contains("¶ Pages › Issues") || f.lines().any(|l| l.trim() == "Issues"), "{f}");
    let _ = std::fs::remove_dir_all(&root);
}
