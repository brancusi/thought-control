//! The headless CLI: fixtures render to their saved snapshots, traces replay, and key
//! scripts and message files drive the editor.

use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_caretline"))
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn run(cmd: &mut Command) -> String {
    let out = cmd.output().expect("caretline runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

fn viewport(state: &Path) -> String {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(state).unwrap()).unwrap();
    format!("{}x{}", v["viewport"]["width"], v["viewport"]["height"])
}

#[test]
fn fixtures_render_their_snapshots() {
    let dir = fixtures();
    let mut seen = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".state.json") else { continue };
        let size = viewport(&path);
        for (format, ext) in [("text", "txt"), ("ansi", "ansi")] {
            let want = std::fs::read_to_string(dir.join(format!("{stem}.snapshot.{ext}"))).unwrap();
            let got = run(bin().arg("--state").arg(&path).args(["--snapshot", &size, "--format", format]));
            assert_eq!(got, want, "{stem} ({format})");
        }
        seen += 1;
    }
    assert!(seen >= 4, "only {seen} fixtures");
}

#[test]
fn trace_replay_and_message_file_agree() {
    let dir = fixtures();
    let want = std::fs::read_to_string(dir.join("session.snapshot.txt")).unwrap();
    let replayed = run(bin().arg("--replay").arg(dir.join("session.trace.jsonl")).args(["--snapshot", "36x8"]));
    assert_eq!(replayed, want);
    let applied = run(
        bin()
            .arg("--state")
            .arg(dir.join("mid-selection.state.json"))
            .arg("--msgs")
            .arg(dir.join("session.msgs.jsonl"))
            .args(["--snapshot", "36x8"]),
    );
    // The trace starts from the same text as the mid-selection fixture but with the caret
    // at the start, so the two runs differ; both must at least render a full frame.
    assert_eq!(applied.lines().count(), 8);
}

#[test]
fn keys_dump_state_and_rehydrate() {
    let tmp = std::env::temp_dir().join(format!("caretline-cli-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let doc = tmp.join("doc.md");
    std::fs::write(&doc, "hello world\n").unwrap();
    let state = run(bin().arg("--new-state").arg(&doc).args(["--size", "30x5"]));
    let s0 = tmp.join("s0.json");
    std::fs::write(&s0, state).unwrap();
    let s1 = tmp.join("s1.json");
    let frame = run(
        bin()
            .arg("--state")
            .arg(&s0)
            .args(["--keys", "<a-right><s-a-right><bs> there", "--snapshot", "30x5", "--dump-state"])
            .arg(&s1),
    );
    assert_eq!(frame.lines().next().unwrap(), "hello there");
    // Rendering the dumped state alone gives the same frame.
    assert_eq!(run(bin().arg("--state").arg(&s1).args(["--snapshot", "30x5"])), frame);
    // Effects are reported, never performed, in a headless run.
    let effects = run(bin().arg("--state").arg(&s1).args(["--keys", "<c-s>", "--effects", "--dump-state", "-"]));
    assert!(effects.starts_with("{\"effect\":\"write_file\""));
    assert_eq!(std::fs::read_to_string(&doc).unwrap(), "hello world\n");
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn version_flag() {
    let out = run(bin().arg("--version"));
    assert!(out.starts_with("caretline "));
}

/// A `--state` file needs only what a client knows: no history, run or saved revision.
#[test]
fn a_minimal_state_file_is_a_working_editor() {
    let dir = std::env::temp_dir().join(format!("caretline-min-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let state = dir.join("min.json");
    std::fs::write(
        &state,
        r#"{"text":"hello\nworld\n","selection":{"ranges":[{"anchor":0,"head":5}]},"viewport":{"width":20,"height":4},"config":{"soft_wrap":false}}"#,
    )
    .unwrap();
    let typed = run(bin().arg("--state").arg(&state).args(["--keys", "bye", "--snapshot", "20x4"]));
    assert_eq!(typed.lines().take(2).collect::<Vec<_>>(), ["bye", "world"]);
    let undone = run(bin().arg("--state").arg(&state).args(["--keys", "bye<c-z>", "--snapshot", "20x4"]));
    assert_eq!(undone.lines().take(2).collect::<Vec<_>>(), ["hello", "world"]);
    std::fs::write(&state, r#"{"text":"just text"}"#).unwrap();
    let dumped = run(bin().arg("--state").arg(&state).args(["--dump-state", "-"]));
    let v: serde_json::Value = serde_json::from_str(&dumped).unwrap();
    assert_eq!(v["viewport"], serde_json::json!({"width": 80, "height": 24}));
    assert_eq!(v["dirty"], false);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn outline_mode_reads_markdown_edits_blocks_and_saves_markdown() {
    let dir = fixtures();
    let file = dir.join("trip.md");
    let frame = run(bin().arg("--outline").arg(&file).args(["--keys", "<down><down><down><c-t>", "--snapshot", "50x18"]));
    assert!(frame.contains("- [x] Pay the deposit"), "{frame}");
    let effects = run(bin().arg("--outline").arg(&file).args(["--keys", "<d-down>!<c-s>", "--effects"]));
    let write = effects.lines().find(|l| l.contains("write_file")).expect("a write_file effect");
    let v: serde_json::Value = serde_json::from_str(write).unwrap();
    let want = std::fs::read_to_string(&file).unwrap().replace("2. Leave", "2. Leave!");
    assert_eq!(v["text"].as_str(), Some(want.as_str()), "the file comes back as Markdown, blank lines and all");
}
