//! Tags added outside the text (`thc tag`, `todo -t`) survive text edits; a `#tag` removed from
//! the text goes.

mod common;

use serde_json::Value;

fn thc(root: &std::path::Path, args: &[&str]) -> String {
    let o = common::thc().args(args).current_dir(root).env("THC_VAULT", root.join("vault")).env("THC_CACHE_DIR", root.join("cache")).env_remove("THC_ACTOR").output().unwrap();
    assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into()
}

#[test]
fn explicit_tags_survive_text_edits() {
    let root = thc_core::scratch::dir(&format!("thc-tags-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    thc(&root, &["init", "vault"]);
    let v: Value = serde_json::from_str(&thc(&root, &["--json", "todo", "Draft plan #home", "-t", "work"])).unwrap();
    let id = v["nodes"][0]["id"].as_str().unwrap().to_string();
    thc(&root, &["tag", &id, "urgent"]);
    thc(&root, &["text", &id, "Draft the plan #focus"]);
    let n: Value = serde_json::from_str(&thc(&root, &["--json", "show", &id])).unwrap();
    let mut tags: Vec<&str> = n["tags"].as_array().unwrap().iter().map(|t| t.as_str().unwrap()).collect();
    tags.sort();
    assert_eq!(tags, ["focus", "urgent", "work"], "#home left with the text; -t and thc tag stay");
    // Removing an explicit tag still works.
    thc(&root, &["tag", &id, "-urgent"]);
    let n: Value = serde_json::from_str(&thc(&root, &["--json", "show", &id])).unwrap();
    assert!(!n["tags"].as_array().unwrap().iter().any(|t| t == "urgent"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn reserved_vault_links_stay_text() {
    // [[vault:…]] is reserved for links between vaults (FORMAT.md): no page, no edge.
    let root = thc_core::scratch::dir(&format!("thc-vaultlink-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let run = |args: &[&str]| {
        let mut c = common::thc();
        c.args(args).current_dir(&root).env("THC_VAULT", root.join("v")).env("THC_CACHE_DIR", root.join("c"));
        let o = c.output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).to_string()
    };
    run(&["init", root.join("v").to_str().unwrap()]);
    let out = run(&["--json", "add", "see [[vault:abc123/def456]] and [[Real Page]]"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(v["nodes"][0]["text"].as_str().unwrap().contains("[[vault:abc123/def456]]"), "{out}");
    let pages = run(&["--json", "q", "is:page"]);
    assert!(pages.contains("Real Page") && !pages.contains("vault:"), "{pages}");
    let _ = std::fs::remove_dir_all(&root);
}

/// `thc add --plain`: the text exactly as written, so a `#word`
/// in a finding is text, not a tag.
#[test]
fn plain_adds_take_no_tags_from_the_text() {
    let root = thc_core::scratch::dir(&format!("thc-plaintags-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    thc(&root, &["init", "vault"]);
    let v: Value = serde_json::from_str(&thc(&root, &["--json", "add", "--plain", "a finding about #x and !high due:fri"])).unwrap();
    let id = v["nodes"][0]["id"].as_str().unwrap().to_string();
    let n: Value = serde_json::from_str(&thc(&root, &["--json", "show", &id])).unwrap();
    assert!(n["tags"].as_array().is_none_or(|t| t.is_empty()), "{n}");
    assert_eq!(n["text"], "a finding about #x and !high due:fri");
    assert!(n.get("due").is_none() && n.get("priority").is_none(), "{n}");
    // Links too: `[[…]]` stays text, makes no stub page, and an empty one isn't refused.
    let v: Value = serde_json::from_str(&thc(&root, &["--json", "add", "--plain", "a click on [[ ]] or [[Lisbon]] places the caret"])).unwrap();
    assert_eq!(v["nodes"][0]["text"], "a click on [[ ]] or [[Lisbon]] places the caret");
    let pages: Value = serde_json::from_str(&thc(&root, &["--json", "q", "is:page"])).unwrap();
    assert!(pages["items"].as_array().unwrap().is_empty(), "{pages}");
    let _ = std::fs::remove_dir_all(&root);
}
