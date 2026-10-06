//! The blank lines you type are kept (writing.md §1): a note's `gap` is an
//! ordinary prop, saved by the editor when a kind change pins it, and round-tripped through
//! `thc import`, `thc edit` and the Markdown export (editing.md E74, E80, EI14).

mod common;

use serde_json::Value;
use std::path::PathBuf;

struct V {
    root: PathBuf,
}

impl V {
    fn new(name: &str) -> V {
        let root = std::env::temp_dir().join(format!("thc-gaps-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let v = V { root };
        v.cli(&["init", "vault"], &[]);
        v
    }

    fn cmd(&self, env: &[(&str, &str)]) -> std::process::Command {
        let mut c = common::thc();
        c.current_dir(&self.root).env("THC_VAULT", self.root.join("vault")).env("THC_CACHE_DIR", self.root.join("cache")).env_remove("THC_ACTOR").env("THC_NOW", "");
        for (k, v) in env {
            c.env(k, v);
        }
        c
    }

    fn cli(&self, args: &[&str], env: &[(&str, &str)]) -> String {
        let o = self.cmd(env).args(args).output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into()
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut a = vec!["--json"];
        a.extend_from_slice(args);
        serde_json::from_str(&self.cli(&a, &[])).unwrap()
    }

    fn gap(&self, text: &str) -> Option<String> {
        let n = &self.json(&["q", &format!("text:\"{text}\" status:any")])["items"][0];
        n["props"]["gap"].as_str().map(str::to_string)
    }
}

impl Drop for V {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn e80_import_edit_and_export_keep_the_blank_line() {
    let v = V::new("rt");
    // Import: the blank line before B is its gap; A has none.
    let f = v.root.join("in.md");
    std::fs::write(&f, "- [ ] Alpha task\n\n- [ ] Bravo task\n- [ ] Charlie task\n").unwrap();
    v.cli(&["import", f.to_str().unwrap(), "--page", "Plan"], &[]);
    assert_eq!(v.gap("Bravo task").as_deref(), Some("1"), "import reads the blank line");
    assert_eq!(v.gap("Charlie task"), None);
    // Export writes it back.
    v.cli(&["export"], &[]);
    let md = std::fs::read_to_string(v.root.join("vault/export/pages/plan.md")).unwrap();
    let a = md.find("Alpha task").unwrap();
    let b = md.find("Bravo task").unwrap();
    let c = md.find("Charlie task").unwrap();
    assert!(md[a..b].contains("\n\n"), "a blank line before Bravo: {md}");
    assert!(!md[b..c].contains("\n\n"), "none before Charlie: {md}");
    // thc edit, unchanged: the gap stays.
    let page = v.json(&["q", "is:page text:Plan"])["items"][0]["id"].as_str().unwrap().to_string();
    v.cli(&["edit", &page], &[("VISUAL", "/usr/bin/true")]);
    assert_eq!(v.gap("Bravo task").as_deref(), Some("1"), "thc edit keeps it");
    // thc edit, blank line removed: the gap goes (back to the default for a task).
    let script = v.root.join("noblank.sh");
    std::fs::write(&script, "#!/bin/sh\nsed -i '' -e '/Alpha task/{n;/^$/d;}' \"$1\"\n").unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    v.cli(&["edit", &page], &[("VISUAL", script.to_str().unwrap())]);
    assert_ne!(v.gap("Bravo task").as_deref(), Some("1"), "the blank line was deleted in the editor");
    // And a blank line added in the editor is a gap.
    let script2 = v.root.join("blank.sh");
    std::fs::write(&script2, "#!/bin/sh\nsed -i '' -e 's/^\\(- \\[ \\] Charlie task.*\\)$/\\n\\1/' \"$1\"\n").unwrap();
    std::fs::set_permissions(&script2, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    v.cli(&["edit", &page], &[("VISUAL", script2.to_str().unwrap())]);
    assert_eq!(v.gap("Charlie task").as_deref(), Some("1"), "a blank line typed in the editor");
}

#[test]
fn e74_the_users_case_saves_the_gap_and_it_shows_after_reopening() {
    let v = V::new("tui");
    let snap = |keys: &str| {
        let o = v.cmd(&[("THC_TUI_SNAPSHOT", "100x24"), ("THC_TUI_KEYS", keys), ("THC_TUI_SNAPSHOT_WRITE", "1")]).args(["j", "--no-focus"]).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).to_string()
    };
    // A task, a blank line, a paragraph; ⌃T on the paragraph.
    snap("[ ] Buy milk<cr><cr>Call the bank<esc>");
    let f = snap("");
    let rows = |f: &str| -> (usize, usize) {
        let ls: Vec<&str> = f.lines().collect();
        (ls.iter().position(|l| l.contains("Buy milk")).unwrap(), ls.iter().position(|l| l.contains("Call the bank")).unwrap())
    };
    let (a, b) = rows(&f);
    assert_eq!(b, a + 2, "a blank row between them: {f}");
    let line = f.lines().nth(b).unwrap();
    let x = line[..line.find("Call").unwrap()].chars().count() + 2;
    // (A snapshot without writes keeps nothing: the frame first, then the same keys and Esc.)
    let o = v.cmd(&[("THC_TUI_SNAPSHOT", "100x24"), ("THC_TUI_KEYS", &format!("<click:{x},{b}><c-t>"))]).args(["j", "--no-focus"]).output().unwrap();
    let f = String::from_utf8_lossy(&o.stdout).to_string();
    assert!(f.contains("[ ] Call the bank"), "{f}");
    assert_eq!(rows(&f), (a, b), "nothing moved: {f}");
    snap(&format!("<click:{x},{b}><c-t><esc>"));
    assert_eq!(v.gap("Call the bank").as_deref(), Some("1"), "the gap is saved with the kind change");
    // Reopened: still there.
    assert_eq!(rows(&snap("")), (a, b));
}
