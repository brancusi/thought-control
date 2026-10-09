//! A bare `m` (`30m`, `+2m`, `every 1m`) is ambiguous: every input surface rejects it with the
//! same message (exit 6). `min` and `mo` work.

mod common;

use std::path::Path;
use std::process::{Command, Stdio};

fn run(root: &Path, args: &[&str], stdin: Option<&str>) -> (i32, String, String) {
    let mut c = common::thc();
    c.args(args)
        .current_dir(root)
        .env("THC_VAULT", root.join("vault"))
        .env("THC_CACHE_DIR", root.join("cache"))
        .env("THC_NOW", "2026-10-03T08:00")
        .env_remove("THC_ACTOR")
        .env_remove("THC_CONTEXT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut ch = c.spawn().unwrap();
    if let Some(s) = stdin {
        use std::io::Write;
        ch.stdin.take().unwrap().write_all(s.as_bytes()).unwrap();
    }
    let o = ch.wait_with_output().unwrap();
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into(), String::from_utf8_lossy(&o.stderr).into())
}

#[test]
fn bare_m_is_rejected_everywhere() {
    let root = thc_core::scratch::dir(&format!("thc-barem-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    assert!(common::thc().current_dir(&root).args(["init", "vault"]).output().unwrap().status.success());
    let r = &root;
    let (_, out, _) = run(r, &["--json", "todo", "Base task", "--due", "fri"], None);
    let id = serde_json::from_str::<serde_json::Value>(&out).unwrap()["nodes"][0]["id"].as_str().unwrap().to_string();
    let rejects: Vec<(Vec<&str>, Option<&str>, &str)> = vec![
        (vec!["add", "Call back due:+30m"], None, "30m"),
        (vec!["todo", "Thing", "--due", "+2m"], None, "2m"),
        (vec!["remind", "Stretch", "--at", "+30m"], None, "30m"),
        (vec!["set", &id, "due=+30m"], None, "30m"),

        (vec!["alert", "add", &id, "--before", "15m"], None, "15m"),
        (vec!["apply", "-"], Some("{\"cmd\":\"todo\",\"text\":\"X\",\"due\":\"+30m\"}\n"), "30m"),
        (vec!["import", "-"], Some("- Outline item due:+30m\n"), "30m"),
        (vec!["q", "due<=+2m"], None, "2m"),
    ];
    for (args, stdin, tok) in &rejects {
        let (code, out, err) = run(r, args, *stdin);
        assert_eq!(code, 6, "{args:?} should exit 6: {out}{err}");
        let n = &tok[..tok.len() - 1];
        let all = format!("{out}{err}");
        assert!(all.contains(&format!("\"{tok}\" is ambiguous here · use {n}min or {n}mo")), "{args:?}: {all}");
    }
    // Repeats: a per-minute repeat isn't a thing, so the hint is just months.
    for args in [vec!["todo", "Rent", "--repeat", "every 1m"], vec!["add", "Rent every!:1m"]] {
        let (code, out, err) = run(r, &args, None);
        assert_eq!(code, 6, "{args:?}: {out}{err}");
        assert!(err.contains("\"1m\" is ambiguous here · use 1mo for months"), "{args:?}: {err}");
    }
    // Shorthand repeats read as words.
    let (_, out, _) = run(r, &["add", "Rent3 every!:1mo"], None);
    assert!(out.contains("↻ every! month"), "{out}");
    let (_, out, _) = run(r, &["add", "Water every:3d"], None);
    assert!(out.contains("↻ every 3 days"), "{out}");
    for args in [
        vec!["add", "Call back due:+30min"],
        vec!["todo", "Thing", "--due", "+2mo"],
        vec!["todo", "Rent", "--repeat", "every 1mo"],
        vec!["add", "Rent2 every!:1mo"],
        vec!["alert", "add", &id, "--before", "15min"],
    ] {
        let (code, out, err) = run(r, &args, None);
        assert_eq!(code, 0, "{args:?}: {out}{err}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// A date token whose value doesn't parse is a validation error (exit 6, kind validation), as
/// AGENTS.md and `thc instructions` say: in capture text and in --due.
#[test]
fn an_unreadable_date_exits_6() {
    let dir = thc_core::scratch::dir(&format!("thc-fryday-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let run = |args: &[&str]| common::thc().args(args).current_dir(&dir).env("THC_VAULT", dir.join("v")).env("THC_CACHE_DIR", dir.join("c")).env_remove("THC_ACTOR").output().unwrap();
    assert!(run(&["init", "v"]).status.success());
    for args in [&["--json", "add", "call due:fryday"][..], &["--json", "todo", "x", "--due", "fryday"][..]] {
        let o = run(args);
        let err = String::from_utf8_lossy(&o.stderr);
        assert_eq!(o.status.code(), Some(6), "{args:?}: {err}");
        assert!(err.contains("\"kind\":\"validation\"") && err.contains("try fri, +3d"), "{err}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
