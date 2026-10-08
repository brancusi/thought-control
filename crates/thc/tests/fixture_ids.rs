//! THC_FIXTURE_IDS=1 with THC_NOW: two vaults built the same way hold the same log (tx ids, event
//! ids and times), so the guide renders only change when the UI does (scripts/guide-renders.sh).

mod common;

use serde_json::Value;

fn build(name: &str, fixed: bool) -> Value {
    let root = thc_core::scratch::dir(&format!("thc-fixture-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let run = |args: &[&str]| {
        let mut c = common::thc();
        c.args(args).current_dir(&root).env("THC_VAULT", root.join("vault")).env("THC_CACHE_DIR", root.join("cache")).env("THC_NOW", "2026-10-03T10:41").env("THC_DEVICE", "guide").env_remove("THC_ACTOR");
        if fixed {
            c.env("THC_FIXTURE_IDS", "1");
        }
        let o = c.output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).to_string()
    };
    run(&["init", "vault"]);
    run(&["todo", "Pay rent", "--due", "fri", "--key", "rent"]);
    let log: Value = serde_json::from_str(&run(&["--json", "log", "--limit", "50"])).unwrap();
    let _ = std::fs::remove_dir_all(&root);
    log
}

#[test]
fn fixture_ids_make_the_log_the_same_on_every_run() {
    let strip = |v: &Value| -> Vec<(String, String)> { v["events"].as_array().unwrap().iter().map(|e| (e["tx"].as_str().unwrap_or_default().to_string(), e["ms"].to_string())).collect() };
    let (a, b) = (build("a", true), build("b", true));
    assert!(!strip(&a).is_empty());
    assert_eq!(strip(&a), strip(&b), "the same tx ids and times");
    // Without it, tx ids are random as always.
    let (c, d) = (build("c", false), build("d", false));
    assert_ne!(strip(&c), strip(&d));
}

#[test]
fn a_leftover_fixture_ids_warns_and_refuses_writes_without_a_pinned_clock() {
    let root = thc_core::scratch::dir(&format!("thc-fixture-guard-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let cmd = |args: &[&str], now: bool, test: bool| {
        let mut c = common::thc();
        c.args(args).current_dir(&root).env("THC_VAULT", root.join("vault")).env("THC_CACHE_DIR", root.join("cache")).env("THC_FIXTURE_IDS", "1").env_remove("THC_NOW");
        if now {
            c.env("THC_NOW", "2026-10-03T10:41");
        }
        if !test {
            // Outside the sandbox flag only (PATH shims and the scratch HOME still apply).
            c.env_remove("THC_TEST");
        }
        c.output().unwrap()
    };
    assert!(cmd(&["init", "vault"], true, true).status.success());
    // Warned wherever THC_NOW is: prime and doctor.
    for a in [&["prime"][..], &["doctor"][..]] {
        let o = cmd(a, true, true);
        let out = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        assert!(out.contains("THC_FIXTURE_IDS is set"), "{a:?}: {out}");
    }
    // No pinned clock, not a test: the write is refused and nothing lands.
    let o = cmd(&["todo", "Pay rent"], false, false);
    assert!(!o.status.success() && String::from_utf8_lossy(&o.stderr).contains("THC_FIXTURE_IDS is set without THC_NOW"), "{}", String::from_utf8_lossy(&o.stderr));
    // With THC_NOW it writes.
    assert!(cmd(&["todo", "Pay rent"], true, false).status.success());
    let _ = std::fs::remove_dir_all(&root);
}
