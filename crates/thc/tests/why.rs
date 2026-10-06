//! `reasons[]` on today/agenda rows, and `--why` (views.md §3.3).

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

fn reasons(v: &Value, text: &str) -> Vec<String> {
    let rows: Vec<&Value> = ["overdue", "today", "done"].iter().flat_map(|k| v[*k].as_array().into_iter().flatten()).collect();
    let n = rows.iter().find(|n| n["text"].as_str().unwrap_or("").starts_with(text)).unwrap_or_else(|| panic!("no row {text}: {v}"));
    n["reasons"].as_array().unwrap().iter().map(|r| r.as_str().unwrap().to_string()).collect()
}

#[test]
fn rows_say_why_they_are_here() {
    let root = std::env::temp_dir().join(format!("thc-why-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    assert!(common::thc().current_dir(&root).args(["init", "vault"]).output().unwrap().status.success());
    let r = &root;
    run(r, &["todo", "Late thing", "--due", "-2d"]);
    run(r, &["todo", "Due now", "--due", "today"]);
    run(r, &["add", "Water plants sched:today every:3d"]);
    run(r, &["add", "[ ] Started earlier sched:-2d due:+5d"]);
    let id = |out: String| serde_json::from_str::<Value>(&out).unwrap()["nodes"][0]["id"].as_str().unwrap().to_string();
    let doing = id(run(r, &["--json", "todo", "In progress", "--due", "today"]));
    run(r, &["set", &doing, "status=doing"]);
    let alerted = id(run(r, &["--json", "add", "[ ] Call the bank sched:today"]));
    run(r, &["alert", "add", &alerted, "--at", "today 9am"]);
    let done = id(run(r, &["--json", "todo", "Finished", "--due", "today"]));
    run(r, &["done", &done]);
    run(r, &["todo", "Next week", "--due", "+3d"]);
    // Due tomorrow, alert this afternoon: a Today row only because of the alert (§3.3).
    let reg = id(run(r, &["--json", "todo", "Renew registration", "--due", "tomorrow"]));
    run(r, &["alert", "add", &reg, "--at", "today 5pm"]);

    let v: Value = serde_json::from_str(&run(r, &["--json", "today"])).unwrap();
    assert_eq!(reasons(&v, "Late thing"), ["overdue"]);
    assert_eq!(reasons(&v, "Due now"), ["due-today"]);
    assert_eq!(reasons(&v, "Water plants"), ["scheduled", "repeating"]);
    // Scheduled earlier, still open: it stays in Today until it's done.
    assert_eq!(reasons(&v, "Started earlier"), ["scheduled"]);
    assert_eq!(reasons(&v, "In progress"), ["due-today", "doing"]);
    assert_eq!(reasons(&v, "Call the bank"), ["scheduled", "alert"]);
    assert_eq!(reasons(&v, "Finished"), ["due-today", "done-today"]);
    assert_eq!(reasons(&v, "Renew registration"), ["alert"]);
    assert_eq!(v["alerts"].as_array().unwrap().len(), 2, "alerts[] stays (additive)");

    // Agenda: each day's rows carry that day's reasons.
    let v: Value = serde_json::from_str(&run(r, &["--json", "agenda", "--days", "7"])).unwrap();
    let day = v["days"].as_array().unwrap().iter().find(|d| d["date"] == "2026-10-06").unwrap();
    assert_eq!(day["items"][0]["reasons"], serde_json::json!(["due-today"]));

    // Human output: only with --why.
    let out = run(r, &["today"]);
    assert!(!out.contains('←'), "{out}");
    assert!(out.contains("Renew registration  ◎ 17:00 · due tomorrow"), "{out}");
    assert!(!out.lines().any(|l| l == "Alerts"), "no separate Alerts section: {out}");
    let out = run(r, &["today", "--why"]);
    assert!(out.contains("Water plants") && out.contains("← scheduled today · repeating"), "{out}");
    assert!(out.contains("← scheduled today · alert 09:00"), "{out}");
    assert!(out.contains("← due today · doing"), "{out}");
    assert!(out.contains("← scheduled 2d ago"), "{out}");
    let _ = std::fs::remove_dir_all(&root);
}
