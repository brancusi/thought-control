//! Contexts (views.md §2): people get them, JSON and agents only when they ask, precedence is
//! env > .thc-context > device, and capture defaults can be opted out of.

mod common;

use serde_json::Value;
use std::path::Path;
use std::process::Command;

fn run(root: &Path, dir: &Path, env: &[(&str, &str)], args: &[&str]) -> (i32, String) {
    let mut c = common::thc();
    c.args(args).current_dir(dir).env("THC_VAULT", root.join("vault")).env("THC_CACHE_DIR", root.join("cache")).env_remove("THC_ACTOR").env_remove("THC_CONTEXT");
    for (k, v) in env {
        c.env(k, v);
    }
    let o = c.output().unwrap();
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into())
}

fn json(s: &str) -> Value {
    serde_json::from_str(s.lines().last().unwrap_or("null")).unwrap()
}

#[test]
fn contexts_filter_people_not_agents() {
    let root = thc_core::scratch::dir(&format!("thc-ctx-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    assert!(common::thc().current_dir(&root).args(["init", "vault"]).output().unwrap().status.success());
    let r = &root;
    run(r, r, &[], &["todo", "Ship the deck #work", "--due", "today"]);
    run(r, r, &[], &["todo", "Water plants", "--due", "today"]);
    run(r, r, &[], &["view", "add", "work", "status:open #work", "--capture", "#work"]);
    run(r, r, &[], &["view", "add", "home", "status:open -#work"]);

    let (code, out) = run(r, r, &[], &["context", "work"]);
    assert_eq!(code, 0);
    assert!(out.contains("context @work is on"), "{out}");

    // A person's listing is filtered, and says so on its first line.
    let (_, out) = run(r, r, &[], &["today"]);
    assert!(out.lines().next().unwrap().contains("showing 1 of 2"), "{out}");
    assert!(out.contains("Ship the deck") && !out.contains("Water plants"), "{out}");
    // JSON ignores the device context unless it asks.
    let (_, out) = run(r, r, &[], &["--json", "today"]);
    let v = json(&out);
    assert_eq!(v["context"]["applied"], false);
    assert_eq!(v["today"].as_array().unwrap().len(), 2);
    let (_, out) = run(r, r, &[], &["--json", "--context", "work", "today"]);
    assert_eq!(json(&out)["today"].as_array().unwrap().len(), 1);

    // Capture defaults: added for people, skipped with -#work, ignored for agents.
    let (_, out) = run(r, r, &[], &["add", "Call the printer"]);
    assert!(out.contains("#work") && out.contains("from context @work"), "{out}");
    let (_, out) = run(r, r, &[], &["add", "Buy milk -#work"]);
    assert!(!out.contains("#work") && !out.contains("-#work"), "{out}");
    let (_, out) = run(r, r, &[("THC_ACTOR", "claude")], &["add", "Agent note"]);
    assert!(!out.contains("#work"), "{out}");

    // Precedence: THC_CONTEXT > .thc-context > device.
    let proj = root.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join(".thc-context"), "home\n").unwrap();
    let (_, out) = run(r, &proj, &[], &["context"]);
    assert!(out.starts_with("@home (from .thc-context"), "{out}");
    let (_, out) = run(r, &proj, &[("THC_CONTEXT", "work")], &["context"]);
    assert!(out.starts_with("@work (from THC_CONTEXT)"), "{out}");
    let (_, out) = run(r, &proj, &[("THC_CONTEXT", "none")], &["context"]);
    assert_eq!(out.trim(), "none");

    let (_, out) = run(r, r, &[], &["context", "none"]);
    assert!(out.contains("context off"), "{out}");
    let (_, out) = run(r, r, &[], &["today"]);
    assert!(out.contains("Water plants") && !out.contains("showing"), "{out}");
    let _ = std::fs::remove_dir_all(&root);
}

/// Every JSON listing a context can filter carries `context`; when the context filtered the
/// results, `applied` is true and `hidden` is what it removed. Search and pages never filter.
#[test]
fn every_filterable_listing_says_its_context() {
    let root = thc_core::scratch::dir(&format!("thc-ctx-json-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    assert!(common::thc().current_dir(&root).args(["init", "vault"]).output().unwrap().status.success());
    let r = &root;
    run(r, r, &[], &["todo", "Ship the deck #work", "--due", "today"]);
    run(r, r, &[], &["todo", "Water plants", "--due", "today"]);
    run(r, r, &[], &["todo", "Plan the trip", "--due", "+2d"]);
    run(r, r, &[], &["add", "Note about plants"]);
    run(r, r, &[], &["add", "Loose idea", "--inbox"]);
    run(r, r, &[], &["add", "Loose work idea #work", "--inbox"]);
    run(r, r, &[], &["view", "add", "work", "#work"]);

    fn count(cmd: &str, v: &Value) -> usize {
        let len = |k: &str| v[k].as_array().map_or(0, |a| a.len());
        match cmd {
            "today" => len("overdue") + len("today"),
            "agenda" => len("overdue") + v["days"].as_array().unwrap().iter().map(|d| d["items"].as_array().unwrap().len()).sum::<usize>(),
            _ => len("items"),
        }
    }
    let listings: &[&[&str]] = &[&["today"], &["agenda", "--days", "7"], &["inbox"], &["journal"], &["q", "status:open"], &["q", "is:inbox"]];
    for args in listings {
        let cmd = args[0];
        let all: Vec<&str> = ["--json"].iter().chain(args.iter()).copied().collect();
        let (code, out) = run(r, r, &[("THC_CONTEXT", "work")], &all);
        assert_eq!(code, 0, "{args:?}: {out}");
        let v = json(&out);
        let (_, out) = run(r, r, &[], &all);
        let u = json(&out);
        let (shown, total) = (count(cmd, &v), count(cmd, &u));
        assert!(shown < total, "{args:?} should be filtered: {shown} of {total}");
        assert_eq!(v["context"]["applied"], true, "{args:?}: {v}");
        assert_eq!(v["context"]["name"], "work", "{args:?}");
        assert_eq!(v["context"]["hidden"], total - shown, "{args:?}: {v}");
        // Without a context the key is still there, so agents can rely on it.
        assert_eq!(u["context"]["applied"], false, "{args:?}: {u}");
        assert!(u["context"].get("hidden").is_none(), "{args:?}: {u}");
    }
    for args in [&["search", "plants"][..], &["pages"][..]] {
        let all: Vec<&str> = ["--json"].iter().chain(args.iter()).copied().collect();
        let (_, out) = run(r, r, &[("THC_CONTEXT", "work")], &all);
        let v = json(&out);
        assert_eq!(v["context"]["applied"], false, "{args:?} never filters: {v}");
        assert_eq!(v["context"]["name"], "work", "{args:?}");
    }

    // People: one sentence when the context hides everything, and the search line names no count.
    run(r, r, &[], &["view", "add", "none-match", "#nothing"]);
    let (_, out) = run(r, r, &[("THC_CONTEXT", "none-match")], &["today"]);
    assert!(out.lines().next().unwrap().starts_with("nothing due in @none-match · 2 more without it · thc context none"), "{out}");
    assert!(!out.contains("showing 0") && !out.contains("Nothing due"), "{out}");
    let (_, out) = run(r, r, &[("THC_CONTEXT", "work")], &["search", "plants"]);
    assert!(out.lines().next().unwrap() == "context @work is on · search ignores contexts", "{out}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn capture_echo_names_the_context_parent() {
    let root = thc_core::scratch::dir(&format!("thc-ctx-under-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    assert!(common::thc().current_dir(&root).args(["init", "vault"]).output().unwrap().status.success());
    let r = &root;
    run(r, r, &[], &["page", "new", "Acme"]);
    run(r, r, &[], &["view", "add", "work", "#work", "--capture", "#work under:Acme"]);
    let (_, out) = run(r, r, &[("THC_CONTEXT", "work")], &["add", "Call the printer"]);
    assert!(out.contains("· under ¶ Acme · from context @work"), "{out}");
    let _ = std::fs::remove_dir_all(&root);
}
