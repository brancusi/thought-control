//! About (docs/design/about.md): what's new since the version you last looked at,
//! the update toast and the footer's `· new` once, the sections, and searching the changelog.

mod common;

use std::path::PathBuf;

struct S {
    root: PathBuf,
}

impl S {
    /// A scratch vault and a device cache whose about.json says `seen` / `launched`.
    fn new(name: &str, seen: Option<&str>) -> S {
        let root = thc_core::scratch::dir(&format!("thc-about-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("xdg/thc")).unwrap();
        if let Some(v) = seen {
            std::fs::write(root.join("xdg/thc/about.json"), format!(r#"{{"seen":"{v}","launched":"{v}"}}"#)).unwrap();
        }
        let s = S { root };
        let o = s.cmd().args(["init", "vault"]).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        s
    }

    fn cmd(&self) -> std::process::Command {
        let mut c = common::thc();
        c.current_dir(&self.root)
            .env("XDG_CACHE_HOME", self.root.join("xdg"))
            .env("THC_VAULT", self.root.join("vault"))
            .env("THC_CACHE_DIR", self.root.join("cache"))
            .env("THC_TUI_VERSION", "0.9.55")
            .env_remove("THC_TUI_SNAPSHOT_WRITE")
            .env_remove("THC_TUI_SNAPSHOT_ABOUT");
        c
    }

    /// One frame after `keys`. `write`: what it marks as seen is kept, as in a real session.
    fn snap(&self, keys: &str, write: bool) -> String {
        let mut c = self.cmd();
        c.arg("tui").env("THC_TUI_SNAPSHOT", "110x30").env("THC_TUI_KEYS", keys);
        if write {
            c.env("THC_TUI_SNAPSHOT_ABOUT", "1");
        }
        let o = c.output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into()
    }

    fn state(&self) -> String {
        std::fs::read_to_string(self.root.join("xdg/thc/about.json")).unwrap_or_default()
    }
}

fn footer(frame: &str) -> &str {
    frame.lines().last().unwrap_or("")
}

/// The What's new section: from its heading up to This thc.
fn whats_new(frame: &str) -> String {
    let lines: Vec<&str> = frame.lines().collect();
    let a = lines.iter().position(|l| l.contains("What's new since") || l.contains("You're up to date")).expect(frame);
    let b = lines.iter().position(|l| l.trim_start().starts_with("This thc")).unwrap_or(lines.len());
    lines[a..b].join("\n")
}

#[test]
fn a1_a2_whats_new_since_you_last_looked_then_up_to_date() {
    let s = S::new("a1", Some("0.9.52"));
    // First launch of 0.9.55 after 0.9.52: the toast once, and the footer says new.
    let first = s.snap("", true);
    assert!(footer(&first).contains("updated to 0.9.55"), "the toast (over the footer): {first}");
    assert!(s.state().contains(r#""launched":"0.9.55""#), "{}", s.state());
    let again = s.snap("", true);
    assert!(!again.contains("updated to"), "the toast is once: {again}");
    assert!(footer(&again).contains("thc 0.9.55 · new"), "new until About is opened: {}", footer(&again));
    // About lists the three releases since 0.9.52, newest first, and nothing older.
    let about = s.snap("<space>a", true);
    let new = whats_new(&about);
    assert!(new.contains("What's new since 0.9.52 (3 releases)") && new.contains("new since you last looked"), "{new}");
    let (p55, p54, p53) = (new.find("0.9.55").unwrap(), new.find("0.9.54").unwrap(), new.find("0.9.53").unwrap());
    assert!(p55 < p54 && p54 < p53, "{new}");
    assert!(!new.contains("0.9.52  "), "{new}");
    // Notes read as text: no Markdown marks.
    assert!(!new.contains("**") && !new.contains('`'), "{new}");
    assert!(s.state().contains(r#""seen":"0.9.55""#), "{}", s.state());
    // Opened once: up to date, and the footer is plain again.
    let later = s.snap("", true);
    assert!(!footer(&later).contains("· new") && footer(&later).contains("thc 0.9.55"), "{}", footer(&later));
    let about = s.snap("<space>a", true);
    assert!(whats_new(&about).contains("You're up to date · 0.9.55"), "{about}");
}

#[test]
fn a_first_install_has_nothing_new_and_a_snapshot_marks_nothing() {
    let s = S::new("fresh", None);
    let f = s.snap("", false);
    assert!(!f.contains("updated to") && !footer(&f).contains("· new"), "{f}");
    // A snapshot that doesn't write leaves the device's state alone.
    assert_eq!(s.state(), "");
    let s = S::new("look", Some("0.9.52"));
    s.snap("<space>a", false);
    assert!(s.state().contains(r#""seen":"0.9.52""#), "{}", s.state());
}

#[test]
fn sections_jump_and_this_thc_never_shows_the_home_path() {
    let s = S::new("sections", Some("0.9.55"));
    let two = s.snap("<space>a2", false);
    let body: Vec<&str> = two.lines().skip(2).collect();
    assert!(body[0].trim_start().starts_with("This thc"), "{two}");
    for label in ["version", "vault", "daemon", "terminal", "doctor", "config"] {
        assert!(body.iter().any(|l| l.trim_start().starts_with(label)), "{label}: {two}");
    }
    let home = common::root().join("home");
    assert!(!two.contains(home.to_str().unwrap()), "a full home path: {two}");
    assert!(two.contains("~/.config/thought/config.toml") || two.contains("config.toml"), "{two}");
    let three = s.snap("<space>a3", false);
    let body: Vec<&str> = three.lines().skip(2).collect();
    assert!(body[0].trim_start().starts_with("Changelog ·"), "{three}");
    // `:changes` is About (at What's new), and Esc closes it.
    let changes = s.snap(":changes<cr>", false);
    assert!(changes.contains("About thc · What's new"), "{changes}");
    let closed = s.snap("<space>a<esc>", false);
    assert!(!closed.contains("About thc ·"), "{closed}");
}

#[test]
fn a4_slash_searches_the_changelog() {
    let s = S::new("search", Some("0.9.55"));
    let mut c = s.cmd();
    c.arg("tui").env("THC_TUI_SNAPSHOT", "110x30").env("THC_TUI_KEYS", "<space>a/kitty<cr>").env("THC_TUI_SNAPSHOT_FORMAT", "ansi");
    let o = c.output().unwrap();
    let ansi = String::from_utf8_lossy(&o.stdout);
    let plain = s.snap("<space>a/kitty<cr>", false);
    assert!(plain.lines().next().unwrap().contains("/kitty"), "{plain}");
    // The view went to the first match, highlighted (reversed).
    assert!(plain.lines().skip(2).any(|l| l.to_lowercase().contains("kitty")), "{plain}");
    // Reverse video: an SGR sequence with a 7 among its parameters (`\e[7m`, `\e[0;1;7;38;…m`).
    let reversed = ansi.split("\u{1b}[").skip(1).filter_map(|s| s.split_once('m').map(|(p, _)| p)).any(|p| p.split(';').any(|x| x == "7"));
    assert!(reversed, "no highlight: {ansi}");
    // n steps to the next match: the view moves.
    let next = s.snap("<space>a/kitty<cr>n", false);
    assert_ne!(next.lines().nth(3), plain.lines().nth(3), "n didn't move");
}
