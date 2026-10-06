//! TUI snapshots replay keys, and keys write. By default the writes land in a scratch copy,
//! so `THC_TUI_KEYS=…` never changes a real vault; THC_TUI_SNAPSHOT_WRITE=1 writes for real.

mod common;

use std::path::Path;
use std::process::Command;

fn log_lines(root: &Path) -> usize {
    let mut n = 0;
    for d in std::fs::read_dir(root.join("vault/log")).unwrap().flatten() {
        for f in std::fs::read_dir(d.path()).unwrap().flatten() {
            n += std::fs::read_to_string(f.path()).unwrap().lines().count();
        }
    }
    n
}

fn snap(root: &Path, keys: &str, write: bool) -> String {
    let mut c = common::thc();
    c.arg("tui")
        .current_dir(root)
        .env("THC_VAULT", root.join("vault"))
        .env("THC_CACHE_DIR", root.join("cache"))
        .env("THC_TUI_SNAPSHOT", "100x24")
        .env("THC_TUI_KEYS", keys)
        .env_remove("THC_ACTOR")
        .env_remove("THC_TUI_SNAPSHOT_WRITE");
    if write {
        c.env("THC_TUI_SNAPSHOT_WRITE", "1");
    }
    let o = c.output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into()
}

#[test]
fn snapshots_never_write_to_the_real_vault() {
    let root = std::env::temp_dir().join(format!("thc-snap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    assert!(common::thc().current_dir(&root).args(["init", "vault"]).output().unwrap().status.success());
    let o = common::thc().current_dir(&root).env("THC_VAULT", root.join("vault")).env("THC_CACHE_DIR", root.join("cache")).args(["add", "First"]).output().unwrap();
    assert!(o.status.success());
    let before = log_lines(&root);
    // Capture a note and make a page: the frame shows them, the vault doesn't get them.
    let frame = snap(&root, "aCheck budget<cr>", false);
    assert!(frame.contains("Check budget"), "{frame}");
    snap(&root, "4q4<cr>", false);
    assert_eq!(log_lines(&root), before, "a snapshot changed the vault's log");
    // Opt in, and it writes.
    snap(&root, "aReally write<cr>", true);
    assert!(log_lines(&root) > before);
    let _ = std::fs::remove_dir_all(&root);
}

/// After an in-place update the new binary opens where the old one was, with the toast.
#[test]
fn an_update_resumes_where_it_was() {
    let root = std::env::temp_dir().join(format!("thc-resume-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    assert!(common::thc().current_dir(&root).args(["init", "vault"]).output().unwrap().status.success());
    let state = root.join("resume.json");
    std::fs::write(&state, r#"{"view":"Journal","journal_date":"2026-10-02","updated_to":"9.9.9"}"#).unwrap();
    let o = common::thc()
        .arg("tui")
        .current_dir(&root)
        .env("THC_VAULT", root.join("vault"))
        .env("THC_CACHE_DIR", root.join("cache"))
        .env("THC_TUI_SNAPSHOT", "100x24")
        .env("THC_TUI_KEYS", "")
        .env("THC_TUI_RESUME", &state)
        .output()
        .unwrap();
    let screen = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(screen.contains("updated to 9.9.9"), "{screen}");
    assert!(screen.contains("FRI 02 OCT 2026"), "back on the same journal day: {screen}");
    assert!(!state.exists(), "the handoff file is used once");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_256_colour_terminal_gets_the_256_colour_theme() {
    // tui-handoff.md §1.2: no COLORTERM, TERM says 256 colours -> ember-dark-256 (indexed).
    let root = std::env::temp_dir().join(format!("thc-256-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let run = |env: &[(&str, &str)], args: &[&str]| {
        let mut c = common::thc();
        c.args(args).current_dir(&root).env("THC_VAULT", root.join("v")).env("THC_CACHE_DIR", root.join("c")).env_remove("COLORTERM").env_remove("THC_THEME").env_remove("NO_COLOR");
        for (k, v) in env {
            c.env(k, v);
        }
        let o = c.output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).to_string()
    };
    run(&[], &["init", root.join("v").to_str().unwrap()]);
    let snap = |env: &[(&str, &str)]| {
        let mut e = vec![("THC_TUI_SNAPSHOT", "100x24"), ("THC_TUI_SNAPSHOT_FORMAT", "ansi")];
        e.extend_from_slice(env);
        run(&e, &["tui"])
    };
    let f = snap(&[("TERM", "xterm-256color")]);
    assert!(f.contains("38;5;254"), "256 colours: text is index 254");
    let f = snap(&[("TERM", "xterm-256color"), ("COLORTERM", "truecolor")]);
    assert!(!f.contains(";5;") && f.contains("38;2;"), "truecolor wins");
    let f = snap(&[("TERM", "xterm")]);
    assert!(!f.contains(";5;") && !f.contains("38;2;"), "16 colours");
    let f = snap(&[("THC_THEME", "ember-light-256")]);
    assert!(f.contains("38;5;235"), "forced light: text is index 235");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn first_run_and_review_3_copy() {
    let root = std::env::temp_dir().join(format!("thc-first-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let run = |env: &[(&str, &str)], args: &[&str]| {
        let mut c = common::thc();
        c.args(args).current_dir(&root).env("THC_VAULT", root.join("v")).env("THC_CACHE_DIR", root.join("c")).env_remove("THC_ACTOR");
        for (k, v) in env {
            c.env(k, v);
        }
        let o = c.output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).to_string()
    };
    let snap = |keys: &str, args: &[&str]| run(&[("THC_TUI_SNAPSHOT", "120x30"), ("THC_TUI_KEYS", keys)], args);
    run(&[], &["init", root.join("v").to_str().unwrap()]);
    // An empty vault: the welcome, the first journal's `just type`, prime's note for agents.
    let f = snap("", &["tui"]);
    assert!(f.contains("Nothing here yet.") && f.contains("5 or thc j   write in today's journal"), "{f}");
    let f = snap("", &["j", "--no-focus"]);
    assert!(f.lines().last().unwrap().contains("just type"), "{f}");
    assert!(run(&[], &["prime"]).contains("empty     a new vault"));
    assert!(run(&[], &["today"]).contains("Nothing due. thc j to write in today's journal"));
    // After the first line: the ordinary empty state; `you`, not `human`.
    run(&[], &["todo", "Call the bank every!:3mo"]);
    run(&[], &["add", "A thought"]);
    let f = snap("1", &["tui"]);
    assert!(!f.contains("Nothing here yet."), "{f}");
    let f = run(&[("THC_TUI_SNAPSHOT", "100x30"), ("THC_TUI_KEYS", "3")], &["tui"]);
    assert!(!f.contains("human") && f.contains("↻ 3mo!") && !f.contains(" ms"), "short repeat, no timings: {f}");
    let f = snap("7", &["tui"]);
    assert!(f.contains("you") && !f.contains("human"), "{f}");
    // The palette: sentence case, no internal keys.
    let f = snap(":upd", &["tui"]);
    assert!(f.contains("Update thc") && !f.contains("!update"), "{f}");
    let _ = std::fs::remove_dir_all(&root);
}
