//! Structural queries (research item 8, views.md §3): `ancestor:(…)`, `has:child(…)`,
//! `happens<=…`.

mod common;

use serde_json::Value;
use std::path::Path;
use std::process::Command;

fn run(root: &Path, args: &[&str]) -> String {
    let o = common::thc()
        .args(args)
        .current_dir(root)
        .env("THC_VAULT", root.join("vault"))
        .env("THC_CACHE_DIR", root.join("cache"))
        .env("THC_NOW", "2026-10-03T08:00")
        .env_remove("THC_ACTOR")
        .env_remove("THC_CONTEXT")
        .output()
        .unwrap();
    assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into()
}

#[test]
fn structural_queries_walk_the_tree() {
    let root = std::env::temp_dir().join(format!("thc-struct-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    assert!(common::thc().current_dir(&root).args(["init", "vault"]).output().unwrap().status.success());
    let r = &root;
    let id = |out: String| serde_json::from_str::<Value>(&out).unwrap()["nodes"][0]["id"].as_str().unwrap().to_string();
    let p = id(run(r, &["--json", "add", "Acme #project", "--inbox"]));
    let a = id(run(r, &["--json", "todo", "Kickoff", "--under", &p]));
    let b = id(run(r, &["--json", "todo", "Book room", "--due", "tomorrow", "--under", &a]));
    let c = id(run(r, &["--json", "todo", "Old thing", "--due", "+10d", "--under", &p]));
    run(r, &["done", &c]);
    let x = id(run(r, &["--json", "add", "Call the bank"]));
    run(r, &["alert", "add", &x, "--at", "today 5pm"]);

    let q = |s: &str| -> Vec<String> {
        let v: Value = serde_json::from_str(&run(r, &["--json", "q", s])).unwrap();
        let mut ids: Vec<String> = v["items"].as_array().unwrap().iter().map(|n| n["id"].as_str().unwrap().to_string()).collect();
        ids.sort();
        ids
    };
    let set = |v: &[&String]| -> Vec<String> {
        let mut s: Vec<String> = v.iter().map(|x| x.to_string()).collect();
        s.sort();
        s
    };
    // Descendants of anything matching, at any depth, not the match itself.
    assert_eq!(q("ancestor:(#project)"), set(&[&a, &b, &c]));
    assert_eq!(q("ancestor:(#project) status:open"), set(&[&a, &b]));
    // Parents of anything matching.
    assert_eq!(q("has:child(status:open)"), set(&[&p, &a]));
    assert_eq!(q("is:task -has:child(status:open)"), set(&[&b, &c]));
    assert_eq!(q("has:child is:task"), set(&[&a]));
    // Nesting: under something that has a done child.
    assert_eq!(q("ancestor:(has:child(status:done))"), set(&[&a, &b, &c]));
    // Any date: scheduled, due, or an alert.
    assert_eq!(q("happens<=+2d"), set(&[&b, &x]));
    assert_eq!(q("happens:none is:task"), set(&[&a]));

    // Day nodes read as days in human output, not `Journal 2026-10-03`.
    let out = run(r, &["q", "has:child"]);
    assert!(out.contains("§ today") && !out.contains("Journal 20"), "{out}");
    let out = run(r, &["q", "ancestor:(#project) has:child(status:open)", "--explain"]);
    assert!(out.contains("under something that is: tagged #project"), "{out}");
    assert!(out.contains("and with a child that is: open tasks"), "{out}");
    let _ = std::fs::remove_dir_all(&root);
}
