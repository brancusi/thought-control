//! `group:` (views.md §3.2): presentation only. The same matches, laid out in groups.

mod common;

use serde_json::Value;
use std::path::Path;
use std::process::Command;

fn run(root: &Path, env: &[(&str, &str)], args: &[&str]) -> String {
    let mut c = common::thc();
    c.args(args).current_dir(root).env("THC_VAULT", root.join("vault")).env("THC_CACHE_DIR", root.join("cache")).env_remove("THC_ACTOR").env_remove("THC_CONTEXT");
    for (k, v) in env {
        c.env(k, v);
    }
    let o = c.output().unwrap();
    assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into()
}

fn labels(v: &Value) -> Vec<(String, usize)> {
    v["groups"].as_array().unwrap().iter().map(|g| (g["label"].as_str().unwrap().to_string(), g["count"].as_u64().unwrap() as usize)).collect()
}

#[test]
fn groups_lay_out_the_same_matches() {
    let root = std::env::temp_dir().join(format!("thc-group-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    assert!(common::thc().current_dir(&root).args(["init", "vault"]).output().unwrap().status.success());
    let r = &root;
    let now = [("THC_NOW", "2026-10-03T09:00")];
    let id = |out: String| -> String { serde_json::from_str::<Value>(&out).unwrap()["nodes"][0]["id"].as_str().unwrap().to_string() };
    let page = id(run(r, &now, &["--json", "page", "new", "Q4 Planning"]));
    let okrs = id(run(r, &now, &["--json", "todo", "Draft OKRs #work #q4", "--due", "-2d", "--under", &page]));
    run(r, &now, &["todo", "Collect metrics", "--due", "today", "--under", &okrs]);
    run(r, &now, &["todo", "Book flights", "--due", "tomorrow"]);
    run(r, &now, &["todo", "Plan offsite", "--due", "+4d"]);
    run(r, &now, &["todo", "Someday thing"]);
    let alpha = id(run(r, &now, &["--json", "page", "new", "Alpha"]));
    run(r, &now, &["todo", "Far off", "--due", "+60d", "--under", &alpha]);
    run(r, &[("THC_NOW", "2026-10-03T09:00"), ("THC_ACTOR", "claude")], &["todo", "Agent task", "--due", "+30d"]);

    let all = |q: &str| serde_json::from_str::<Value>(&run(r, &now, &["--json", "q", q])).unwrap();
    let v = all("status:open group:due");
    assert_eq!(v["count"], 7);
    assert_eq!(
        labels(&v),
        [("Overdue", 1), ("Today", 1), ("Tomorrow", 1), ("This week", 1), ("Later", 2), ("No due date", 1)].map(|(a, b)| (a.to_string(), b)).to_vec()
    );
    assert_eq!(v["groups"][0]["key"], "overdue");

    let v = all("status:open group:parent");
    let l = labels(&v);
    assert!(l.contains(&("¶ Q4 Planning".into(), 2)), "{l:?}");
    assert!(l.contains(&("§ today".into(), 4)), "{l:?}");
    assert_eq!(v["groups"].as_array().unwrap().iter().find(|g| g["label"] == "¶ Q4 Planning").unwrap()["key"], page);
    // No sort: pages A–Z, then journal days. With sort:due, the group with the soonest item first.
    let order = |v: &Value| labels(v).into_iter().map(|(l, _)| l).collect::<Vec<_>>();
    assert_eq!(order(&v), ["¶ Alpha", "¶ Q4 Planning", "§ today"]);
    assert_eq!(order(&all("status:open group:parent sort:due")), ["¶ Q4 Planning", "§ today", "¶ Alpha"]);

    let v = all("status:open group:actor");
    assert_eq!(labels(&v)[0], ("human".into(), 6));
    assert_eq!(labels(&v)[1], ("◆ claude".into(), 1));

    let v = all("status:open group:tag");
    assert_eq!(v["repeated"], true);
    assert_eq!(labels(&v), vec![("#q4".into(), 1), ("#work".into(), 1), ("no tag".into(), 6)]);

    // Human output: headings with counts, the repeated-tags note, and no `in ¶ Q4 Planning`
    // under its own heading (but `in Draft OKRs` stays).
    let out = run(r, &now, &["q", "status:open group:parent"]);
    assert!(out.contains("¶ Q4 Planning  2"), "{out}");
    assert!(!out.contains("in Q4 Planning"), "{out}");
    assert!(out.contains("in Draft OKRs"), "{out}");
    let out = run(r, &now, &["q", "status:open group:tag"]);
    assert!(out.trim_end().ends_with("nodes with several tags appear once per tag"), "{out}");
    let out = run(r, &now, &["q", "status:open group:status"]);
    assert!(!out.contains("several tags"), "{out}");
    let _ = std::fs::remove_dir_all(&root);
}
