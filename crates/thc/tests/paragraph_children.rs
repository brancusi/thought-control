//! Tab nests any note under the note above, paragraphs too (like Logseq): typed in the
//! editor, saved as a child of the paragraph's node, still nested after reopening, and
//! indented under it in the Markdown export. On both editor engines.

mod common;

use serde_json::Value;
use std::path::PathBuf;

struct V {
    root: PathBuf,
}

impl V {
    fn new(name: &str) -> V {
        let root = std::env::temp_dir().join(format!("thc-pchild-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let v = V { root };
        v.cli(&["init", "vault"]);
        v
    }

    fn cmd(&self) -> std::process::Command {
        let mut c = common::thc();
        c.current_dir(&self.root).env("THC_VAULT", self.root.join("vault")).env("THC_CACHE_DIR", self.root.join("cache")).env_remove("THC_ACTOR").env("THC_NOW", "");
        c
    }

    fn cli(&self, args: &[&str]) -> String {
        let o = self.cmd().args(args).output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into()
    }

    fn node(&self, text: &str) -> Value {
        let v: Value = serde_json::from_str(&self.cli(&["--json", "q", text])).unwrap();
        v["items"][0].clone()
    }

    fn snap(&self, engine: &str, keys: &str) -> String {
        let o = self
            .cmd()
            .env("THC_EDITOR", engine)
            .env("THC_TUI_SNAPSHOT", "100x24")
            .env("THC_TUI_KEYS", keys)
            .env("THC_TUI_SNAPSHOT_WRITE", "1")
            .args(["j", "--no-focus"])
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).to_string()
    }
}

impl Drop for V {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn column(frame: &str, text: &str) -> usize {
    let line = frame.lines().find(|l| l.contains(text)).unwrap_or_else(|| panic!("{text:?} in {frame}"));
    line[..line.find(text).unwrap()].chars().count()
}

fn tab_under_a_paragraph_saves_a_child(engine: &str) {
    let v = V::new(engine);
    v.snap(engine, "Para line<cr>first subtask<tab><esc>");
    let para = v.node("Para line");
    let child = v.node("first subtask");
    assert_eq!(child["parent"], para["id"], "[{engine}] the line is the paragraph's child: {child}");
    assert_eq!(para["text"], "Para line", "[{engine}] the paragraph keeps only its own line");
    // Reopened: still nested, drawn one level in.
    let f = v.snap(engine, "");
    assert_eq!(column(&f, "first subtask"), column(&f, "Para line") + 4, "[{engine}] {f}");
    // Shift-Tab takes it back to the top level.
    v.snap(engine, "<up><s-tab><esc>");
    let child = v.node("first subtask");
    assert_eq!(child["parent"], para["parent"], "[{engine}] back beside the paragraph: {child}");
}

#[test]
fn tab_under_a_paragraph_saves_a_child_on_the_old_engine() {
    tab_under_a_paragraph_saves_a_child("old");
}

#[test]
fn tab_under_a_paragraph_saves_a_child_on_caretline_next() {
    tab_under_a_paragraph_saves_a_child("next");
}

#[test]
fn the_markdown_export_indents_a_paragraphs_children() {
    let v = V::new("export");
    v.snap("next", "Para line<cr>first subtask<tab><cr><cr>[ ] a task under it<esc>");
    v.cli(&["export"]);
    let dir = v.root.join("vault/export");
    let md = walk(&dir).into_iter().map(|p| std::fs::read_to_string(p).unwrap()).find(|s| s.contains("Para line")).expect("the day exported");
    let line = |t: &str| md.lines().find(|l| l.contains(t)).unwrap_or_else(|| panic!("{t:?} in {md}")).to_string();
    let indent = |l: &str| l.len() - l.trim_start().len();
    assert!(indent(&line("first subtask")) > indent(&line("Para line")), "{md}");
    assert_eq!(indent(&line("a task under it")), indent(&line("first subtask")), "{md}");
}

fn walk(d: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}
