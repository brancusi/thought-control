//! `thc ui`: headless render and replay are byte-identical across runs (THC_NOW pinned), and a
//! running TUI answers the JSON-lines protocol (docs/ui-protocol.md).

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

struct Root(PathBuf);

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A vault with a few tasks and notes, for one test.
fn vault(tag: &str) -> Root {
    let root = std::env::temp_dir().join(format!("thc-ui-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let r = Root(root);
    let o = thc(&r.0).args(["init", "vault"]).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    for text in ["[ ] write the plan due:today #work", "[ ] call the bank !high", "[ ] water the plants #home", "an idea about [[Garden]]"] {
        let o = thc(&r.0).args(["add", text]).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }
    r
}

fn thc(root: &Path) -> Command {
    let mut c = common::thc();
    c.current_dir(root)
        .env("THC_VAULT", root.join("vault"))
        .env("THC_CACHE_DIR", root.join("cache"))
        .env("THC_NOW", "2026-10-07T10:00")
        .env("THC_FIXTURE_IDS", "1")
        .env("THC_DEVICE", "test")
        .env("THC_ACTOR", "human");
    c
}

fn ok(c: &mut Command) -> String {
    let o = c.output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into()
}

#[test]
fn render_from_a_state_file_is_the_same_every_time() {
    let r = vault("render");
    let state = r.0.join("state.json");
    std::fs::write(&state, r#"{"view": "tasks", "tasks_filter": "status:open #work"}"#).unwrap();
    let a = ok(thc(&r.0).args(["ui", "render", "100x24", "--state"]).arg(&state));
    let b = ok(thc(&r.0).args(["ui", "render", "100x24", "--state"]).arg(&state));
    assert_eq!(a, b);
    assert!(a.contains("write the plan") && !a.contains("water the plants"), "{a}");
    assert_eq!(a.lines().count(), 24);
    // The default state round-trips: what `state --default` prints renders as Today.
    let d = ok(thc(&r.0).args(["ui", "state", "--default"]));
    std::fs::write(&state, &d).unwrap();
    let today = ok(thc(&r.0).args(["ui", "render", "100x24", "--state"]).arg(&state));
    assert!(today.contains("Today"), "{today}");
    // Cells: JSON rows.
    let cells = ok(thc(&r.0).args(["ui", "render", "60x24", "--format", "cells", "--state"]).arg(&state));
    let v: serde_json::Value = serde_json::from_str(&cells).unwrap();
    assert_eq!(v["rows"].as_array().unwrap().len(), 24);
    // A bad state is a validation error (exit 6) with a suggestion.
    std::fs::write(&state, r#"{"veiw": "tasks"}"#).unwrap();
    let o = thc(&r.0).args(["ui", "render", "--state"]).arg(&state).output().unwrap();
    assert_eq!(o.status.code(), Some(6), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8_lossy(&o.stderr).contains("did you mean `view`"));
}

#[test]
fn a_recorded_trace_replays_byte_for_byte() {
    let r = vault("replay");
    let trace = r.0.join("trace.jsonl");
    let live = ok(thc(&r.0)
        .args(["tui", "--trace"])
        .arg(&trace)
        .env("THC_TUI_SNAPSHOT", "100x24")
        .env("THC_TUI_KEYS", "3jj<tab>k/plan<cr><esc>?<esc>2<s-tab>"));
    let lines = std::fs::read_to_string(&trace).unwrap();
    assert!(lines.lines().next().unwrap().starts_with("{\"state\":"), "{lines}");
    let a = ok(thc(&r.0).args(["ui", "replay"]).arg(&trace));
    let b = ok(thc(&r.0).args(["ui", "replay"]).arg(&trace));
    assert_eq!(a, b);
    assert_eq!(a, live, "the replay's last frame is the session's");
    let every = ok(thc(&r.0).args(["ui", "replay", "--every"]).arg(&trace));
    assert_eq!(every.matches('\u{c}').count(), lines.lines().count() - 1);
    assert_eq!(every, ok(thc(&r.0).args(["ui", "replay", "--every"]).arg(&trace)));
}
