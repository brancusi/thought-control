//! A line whose parent was deleted while it lived on (daemon.md §4.0a): re-homed, flagged, and
//! the person keeps it here or deletes it.

mod common;

use serde_json::Value;
use std::path::PathBuf;

struct V {
    root: PathBuf,
}

impl V {
    fn new(name: &str) -> V {
        let root = thc_core::scratch::dir(&format!("thc-rehome-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let v = V { root };
        v.ok(&["init", "vault"]);
        v
    }

    fn cmd(&self, args: &[&str]) -> std::process::Output {
        let mut c = common::thc();
        c.args(args).current_dir(&self.root).env("THC_VAULT", self.root.join("vault")).env("THC_CACHE_DIR", self.root.join("cache")).env("THC_NOW", "2026-10-03T09:00");
        c.output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let o = self.cmd(args);
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into()
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut a = vec!["--json"];
        a.extend_from_slice(args);
        serde_json::from_str(&self.ok(&a)).unwrap()
    }

    fn id(&self, args: &[&str]) -> String {
        self.json(args)["nodes"][0]["id"].as_str().unwrap().to_string()
    }

    /// A page, a note on it, a line under the note; the note deleted, the line restored alone:
    /// the line lives on under a deleted parent. (page, note, line)
    fn orphan(&self) -> (String, String, String) {
        let page = self.id(&["page", "new", "Offsite notes"]);
        let note = self.id(&["add", "Plan the offsite", "--under", &page]);
        let line = self.id(&["add", "Book the venue", "--under", &note]);
        self.ok(&["rm", &note]);
        self.ok(&["restore", &line]);
        (page, note, line)
    }
}

#[test]
fn a_line_under_a_deleted_parent_is_rehomed_flagged_and_decided() {
    let v = V::new("keep");
    let (page, note, line) = v.orphan();
    let c = v.json(&["conflict", "ls"]);
    let k = &c["conflicts"][0];
    assert_eq!(k["kind"], "rehomed");
    assert_eq!(k["node"], line.as_str());
    assert_eq!(k["deleted_parent"]["id"], note.as_str());
    assert_eq!(k["deleted_parent"]["title"], "Plan the offsite");
    assert_eq!(k["now_under"]["id"], page.as_str());
    // It shows on the page.
    let show = v.json(&["show", &page, "--depth", "1"]);
    assert!(show["children"].as_array().unwrap().iter().any(|c| c["id"] == line.as_str()), "{show}");
    // Plain resolve says what to do; --keep here makes it real.
    assert_eq!(v.cmd(&["conflict", "resolve", &line]).status.code(), Some(2));
    let out = v.ok(&["conflict", "resolve", &line, "--keep", "here"]);
    assert!(out.starts_with("kept ") && out.contains("flag cleared"), "{out}");
    assert_eq!(v.json(&["show", &line])["parent"], page.as_str());
    assert_eq!(v.json(&["conflict", "ls"])["conflicts"].as_array().unwrap().len(), 0);
}

#[test]
fn delete_does_what_the_other_device_meant_and_restore_brings_it_back() {
    let v = V::new("delete");
    let (_page, note, line) = v.orphan();
    let out = v.ok(&["conflict", "resolve", &line, "--delete"]);
    assert!(out.contains("as this device meant") && out.contains("thc restore"), "{out}");
    assert_eq!(v.json(&["conflict", "ls"])["conflicts"].as_array().unwrap().len(), 0);
    // Restoring the parent brings both back where they were, with no flag.
    v.ok(&["restore", &note]);
    assert_eq!(v.json(&["show", &line])["parent"], note.as_str());
    assert_eq!(v.json(&["conflict", "ls"])["conflicts"].as_array().unwrap().len(), 0);
}
