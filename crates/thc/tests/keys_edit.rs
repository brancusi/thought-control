//! `thc keys --edit` (keymap.md §8.2a): a generated, commented keys block in
//! config.toml; later runs refresh only the comments; the result is checked, problems named
//! with their line.

mod common;

use std::path::PathBuf;

struct H {
    home: PathBuf,
}

impl H {
    fn new(name: &str) -> H {
        let home = thc_core::scratch::dir(&format!("thc-keysedit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("cfg")).unwrap();
        H { home }
    }

    fn run(&self, args: &[&str], editor: Option<&str>) -> std::process::Output {
        let mut c = common::thc();
        c.args(args).current_dir(&self.home).env("HOME", &self.home).env("THC_CONFIG_DIR", self.home.join("cfg"));
        if let Some(e) = editor {
            c.env("VISUAL", e);
        }
        c.output().unwrap()
    }

    fn config(&self) -> String {
        std::fs::read_to_string(self.home.join("cfg/config.toml")).unwrap_or_default()
    }

    fn script(&self, name: &str, body: &str) -> String {
        let p = self.home.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        p.display().to_string()
    }
}

impl Drop for H {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

#[test]
fn the_block_is_made_kept_and_checked() {
    let h = H::new("main");
    std::fs::write(h.home.join("cfg/config.toml"), "home = \"personal\"\n").unwrap();
    // --print: the block, nothing written.
    let o = h.run(&["keys", "--edit", "--print"], None);
    let block = String::from_utf8_lossy(&o.stdout).to_string();
    assert!(block.contains("# [keys.write]") && block.contains("# \"C-t\" = \"doc.task_cycle\""), "{block}");
    assert_eq!(h.config(), "home = \"personal\"\n");
    // --edit with an editor that changes nothing: the block is appended, the rest kept.
    let noop = h.script("noop.sh", "true");
    let o = h.run(&["keys", "--edit"], Some(&noop));
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8_lossy(&o.stdout).contains("keys ok · 0 remapped"));
    let cfg = h.config();
    assert!(cfg.starts_with("home = \"personal\"\n") && cfg.contains("# [keys.list]"), "{cfg}");
    // Uncomment a remap (the table and one line): ok, 1 remapped.
    let un = h.script("un.sh", "perl -0pi -e 's/^# \\[keys.list\\]$/[keys.list]/m; s/^# \"x\" = \"node.done\" .*$/\"d\" = \"node.done\"/m' \"$1\"");
    let o = h.run(&["keys", "--edit"], Some(&un));
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8_lossy(&o.stdout).contains("keys ok · 1 remapped"), "{}", String::from_utf8_lossy(&o.stdout));
    // A later run refreshes the comments and keeps that line where it was.
    let o = h.run(&["keys", "--edit"], Some(&noop));
    assert!(o.status.success());
    let cfg = h.config();
    assert_eq!(cfg.matches("[keys.list]").count(), 1, "{cfg}");
    let list_at = cfg.find("\n[keys.list]").unwrap();
    let mine_at = cfg.find("\n\"d\" = \"node.done\"").expect("kept");
    assert!(mine_at > list_at && cfg[list_at..mine_at].lines().count() <= 2, "under its table: {cfg}");
    assert_eq!(cfg.matches(super_marker()).count(), 1, "one block");
    // A bad action: named, with its line; exit 6.
    let bad = h.script("bad.sh", "sed -i '' -e 's/^\"d\" = \"node.done\"$/\"d\" = \"node.donee\"/' \"$1\"");
    let o = h.run(&["keys", "--edit"], Some(&bad));
    assert_eq!(o.status.code(), Some(6), "{}", String::from_utf8_lossy(&o.stderr));
    let err = String::from_utf8_lossy(&o.stderr);
    let line = h.config().lines().position(|l| l.contains("node.donee")).unwrap() + 1;
    assert!(err.contains(&format!("line {line}:")) && err.contains("node.donee"), "{err}");
    // --context opens at its table (vi-style +LINE): the editor sees the line number.
    std::fs::create_dir_all(h.home.join("bin")).unwrap();
    let vim = h.home.join("bin/vim");
    std::fs::write(&vim, format!("#!/bin/sh\necho \"$@\" > '{}'\n", h.home.join("args").display())).unwrap();
    std::fs::set_permissions(&vim, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let fix = h.script("fix.sh", "sed -i '' -e 's/node.donee/node.done/' \"$1\"");
    assert!(h.run(&["keys", "--edit"], Some(&fix)).status.success());
    let _ = h.run(&["keys", "--edit", "--context", "write"], Some(vim.to_str().unwrap()));
    let args = std::fs::read_to_string(h.home.join("args")).unwrap();
    let want = h.config().lines().position(|l| l.trim() == "# [keys.write]").unwrap() + 1;
    assert!(args.starts_with(&format!("+{want} ")), "{args}");
}

fn super_marker() -> &'static str {
    "# ── Keys (thc keys --edit)"
}
