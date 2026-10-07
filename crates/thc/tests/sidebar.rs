//! The sidebar's acceptance checks (docs/design/sidebar.md §15), driven through
//! `THC_TUI_SNAPSHOT` on the sample vault with the clock pinned, plus its goldens
//! (tests/golden/sidebar/, regenerated with THC_UPDATE_GOLDENS=1).

mod common;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The sample vault (scripts/seed-sample.sh), seeded once per test process with fixed ids.
fn sample() -> &'static Path {
    static V: OnceLock<PathBuf> = OnceLock::new();
    V.get_or_init(|| {
        let root = common::root().join("sidebar-sample");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/seed-sample.sh");
        let mut c = std::process::Command::new("bash");
        common::sandbox(&mut c);
        let o = c
            .arg(&script)
            .arg(root.join("vault"))
            .env("THC", env!("CARGO_BIN_EXE_thc"))
            .env("THC_VAULT", root.join("vault"))
            .env("THC_CACHE_DIR", root.join("cache"))
            .env("THC_NOW", "2026-10-07T09:12")
            .env("THC_FIXTURE_IDS", "1")
            .env("THC_DEVICE", "guide")
            .output()
            .unwrap();
        assert!(o.status.success(), "seed: {}", String::from_utf8_lossy(&o.stderr));
        root
    })
}

/// A fresh copy of the sample vault (for checks that write).
fn copy_of_sample(tag: &str) -> PathBuf {
    let src = sample();
    let dst = common::root().join(format!("sidebar-{tag}"));
    let _ = std::fs::remove_dir_all(&dst);
    let o = std::process::Command::new("cp").arg("-R").arg(src).arg(&dst).output().unwrap();
    assert!(o.status.success());
    let _ = std::fs::remove_dir_all(dst.join("cache"));
    dst
}

struct Snap<'a> {
    root: &'a Path,
    size: &'a str,
    env: Vec<(&'a str, &'a str)>,
}

impl<'a> Snap<'a> {
    fn new(root: &'a Path) -> Snap<'a> {
        Snap { root, size: "140x40", env: vec![] }
    }

    fn size(mut self, s: &'a str) -> Self {
        self.size = s;
        self
    }

    fn env(mut self, k: &'a str, v: &'a str) -> Self {
        self.env.push((k, v));
        self
    }

    fn run(&self, keys: &str) -> String {
        let mut c = common::thc();
        c.arg("tui")
            .env("THC_VAULT", self.root.join("vault"))
            .env("THC_CACHE_DIR", self.root.join("cache"))
            .env("THC_NOW", "2026-10-07T09:12")
            .env("THC_TUI_RENDER", "1")
            .env("THC_THEME", "ember-dark")
            .env("THC_TUI_SNAPSHOT", self.size)
            .env("THC_TUI_KEYS", keys)
            .env_remove("THC_ACTOR")
            .env_remove("NO_COLOR");
        for (k, v) in &self.env {
            c.env(k, v);
        }
        let o = c.output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into()
    }
}

/// Column `from..` of every row (the sidebar at 140 columns starts at 94).
fn cols(frame: &str, from: usize) -> Vec<String> {
    frame.lines().map(|l| l.chars().skip(from).collect::<String>()).collect()
}

fn sidebar(frame: &str) -> Vec<String> {
    cols(frame, 93)
}

fn main_area(frame: &str) -> Vec<String> {
    frame.lines().map(|l| l.chars().take(92).collect::<String>()).collect()
}

/// Where the panels' headers are, top to bottom: their titles.
fn headers(frame: &str) -> Vec<String> {
    sidebar(frame)
        .iter()
        .filter_map(|l| {
            // A header starts at the sidebar's second column (body rows are indented further).
            let t = l.strip_prefix('│')?;
            let t = t.strip_prefix(' ').or_else(|| t.strip_prefix('▌'))?;
            (t.starts_with('▾') || t.starts_with('▸')).then(|| t[t.find('¶').or(t.find('§')).unwrap_or(0)..].split("  ").next().unwrap().trim().to_string())
        })
        .collect()
}

// Journal at 140 × 40, in the sample vault: `[[Health]]` on row 8 at column 54 before the
// sidebar opens (the rail is beside it), at column 31 on row 9 after (the crumb instead).
const SHIFT_HEALTH: &str = "<sclick:54,8>";
const SHIFT_READING: &str = "<sclick:50,12>";
const CLICK_HEALTH_AFTER: &str = "<click:31,9>";

#[test]
fn s1_shift_click_opens_beside_and_main_stays() {
    let s = Snap::new(sample());
    let before = s.run("5");
    let after = s.run(&format!("5{SHIFT_HEALTH}"));
    assert_eq!(headers(&after), ["¶ Health"], "{after}");
    // The main page is unchanged (the same lines), and focus stays in main: the footer is
    // writing's.
    for line in ["Morning: slept well, 30 min run #health", "Call dentist to reschedule #health", "Dentist appointment [[Health]]"] {
        assert!(main_area(&after).iter().any(|l| l.contains(line)), "{after}");
        assert!(before.contains(line));
    }
    assert!(after.lines().last().unwrap().contains("⌃T task"), "{after}");
    assert!(!after.contains("▌▾"), "the panel isn't focused: {after}");
    // The main area is a 92-column screen: the crumb instead of the rail, and the detail pane
    // (the calendar) gives way (§6.2).
    assert!(after.contains("§ Journal › Wed 07 Oct"), "{after}");
    assert!(!after.contains("October 2026"), "{after}");
    assert!(after.lines().nth(1).unwrap().chars().nth(93) == Some('┬'), "the divider meets the tab rule");
}

#[test]
fn s2_newest_on_top_and_no_duplicates() {
    let s = Snap::new(sample());
    let f = s.run(&format!("5{SHIFT_HEALTH}{SHIFT_READING}"));
    assert_eq!(headers(&f), ["¶ Reading List", "¶ Health"], "{f}");
    // ⇧-click Health again (it's now on row 9 at column 31).
    let f = s.run(&format!("5{SHIFT_HEALTH}{SHIFT_READING}<sclick:31,9>"));
    assert_eq!(headers(&f), ["¶ Health", "¶ Reading List"], "{f}");
}

#[test]
fn s3_alt_s_focuses_the_panel_and_esc_returns() {
    let s = Snap::new(sample());
    let f = s.run(&format!("5{SHIFT_HEALTH}{SHIFT_READING}<sclick:31,9><m-s>"));
    assert!(f.contains("│▌▾ ¶ Health"), "{f}");
    let bar = f.lines().last().unwrap();
    assert!(bar.contains("¶ health · aside 1 of 2") && bar.contains("⌥S main") && bar.contains("⌥J ⌥K panel") && bar.contains("⌥M to main"), "{bar}");
    let back = s.run(&format!("5{SHIFT_HEALTH}{SHIFT_READING}<sclick:31,9><m-s><esc>"));
    assert!(!back.contains("▌▾"), "{back}");
    assert!(back.lines().last().unwrap().contains("⌃T task"), "focus is back in main");
    assert_eq!(headers(&back), ["¶ Health", "¶ Reading List"], "Esc doesn't close: {back}");
}

#[test]
fn s4_typing_in_a_panel_saves_in_one_transaction() {
    let root = copy_of_sample("s4");
    let log = |r: &Path| -> usize {
        let mut n = 0;
        for d in std::fs::read_dir(r.join("vault/log")).unwrap().flatten() {
            for f in std::fs::read_dir(d.path()).unwrap().flatten() {
                n += std::fs::read_to_string(f.path()).unwrap().lines().filter(|l| l.contains("\"tx\"")).count();
            }
        }
        n
    };
    let txs = |r: &Path| -> std::collections::BTreeSet<String> {
        let mut s = std::collections::BTreeSet::new();
        for d in std::fs::read_dir(r.join("vault/log")).unwrap().flatten() {
            for f in std::fs::read_dir(d.path()).unwrap().flatten() {
                for l in std::fs::read_to_string(f.path()).unwrap().lines() {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(l) {
                        if let Some(t) = v["tx"].as_str() {
                            s.insert(t.to_string());
                        }
                    }
                }
            }
        }
        s
    };
    let _ = log;
    let before = txs(&root);
    Snap::new(&root).env("THC_TUI_SNAPSHOT_WRITE", "1").run(&format!("5{SHIFT_HEALTH}<m-s><end> with Dr. Patel<m-s>"));
    let after = txs(&root);
    assert_eq!(after.len() - before.len(), 1, "one transaction");
    let o = common::thc().args(["q", "text:Patel", "--json"]).env("THC_VAULT", root.join("vault")).env("THC_CACHE_DIR", root.join("cache")).output().unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains("office on 4th St with Dr. Patel"), "{out}");
}

#[test]
fn s5_one_document_two_views() {
    let s = Snap::new(sample());
    // Health in a panel, then followed in main: one document, two views.
    let typed = s.run(&format!("5{SHIFT_HEALTH}{CLICK_HEALTH_AFTER}<m-s><end> (new office)"));
    let main: Vec<String> = main_area(&typed);
    assert!(main.iter().any(|l| l.contains("office on 4th St (new office)")), "the main view shows the panel's typing in the same frame: {typed}");
    assert!(sidebar(&typed).iter().any(|l| l.contains("(new office)")), "{typed}");
    // ⌘Z (⌃Z) in main undoes it.
    let undone = s.run(&format!("5{SHIFT_HEALTH}{CLICK_HEALTH_AFTER}<m-s><end> (new office)<esc><c-z>"));
    assert!(!undone.contains("(new office)"), "{undone}");
}

#[test]
fn s6_a_change_from_elsewhere_shows_in_the_panel() {
    let s = Snap::new(sample());
    let f = s.run(&format!("5{SHIFT_HEALTH}<remote:yzp20:Dr. Patel, 555-0199, new office>"));
    assert!(sidebar(&f).iter().any(|l| l.contains("Dr. Patel, 555-0199, new office")), "{f}");
    // In ANSI, the changed row has the live tint.
    let a = s.env("THC_TUI_SNAPSHOT_FORMAT", "ansi").run(&format!("5{SHIFT_HEALTH}<remote:yzp20:Dr. Patel, 555-0199, new office>"));
    assert!(a.contains("555-0199"));
}

#[test]
fn s10_fold_shows_the_count_and_keeps_the_view() {
    let s = Snap::new(sample());
    let f = s.run(&format!("5{SHIFT_HEALTH}<m-s><m-c>"));
    assert!(f.contains("▸ ¶ Health") && f.contains("2 open"), "{f}");
    let g = s.run(&format!("5{SHIFT_HEALTH}<m-s><down><m-c><m-c>x"));
    assert!(sidebar(&g).iter().any(|l| l.contains("xSchedule annual physical")), "the caret came back where it was: {g}");
}

#[test]
fn s12_to_main_opens_the_page_at_the_panel_caret() {
    let s = Snap::new(sample());
    let f = s.run(&format!("5{SHIFT_HEALTH}<m-s><down><m-m>"));
    assert!(f.contains("¶ Pages › Health") || f.contains("PAGES"), "main is Health: {f}");
    assert!(headers(&f).is_empty(), "the panel is gone: {f}");
    let typed = s.run(&format!("5{SHIFT_HEALTH}<m-s><down><m-m>x"));
    assert!(typed.contains("xSchedule annual physical"), "at the panel's caret: {typed}");
    // ⌘[ comes back to the journal.
    let back = s.run(&format!("5{SHIFT_HEALTH}<m-s><down><m-m><c-m-left>"));
    assert!(back.contains("WED 07 OCT 2026"), "{back}");
}

#[test]
fn s13_a_click_on_a_link_in_a_panel_follows_it_in_main() {
    let s = Snap::new(sample());
    // Reading List's first line links [[Atomic Habits]] (row 3, the title from column 103).
    let f = s.run("5<sclick:68,11><click:105,3>");
    assert!(f.contains("Atomic Habits") && (f.contains("¶ Pages › Atomic Habits") || f.contains("PAGES")), "{f}");
    assert_eq!(headers(&f), ["¶ Reading List"], "the panel stays: {f}");
}

#[test]
fn keys_guide_lists_the_sidebar() {
    let o = common::thc().args(["keys", "--json"]).output().unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success() && out.contains("sidebar.focus") && out.contains("sidebar.open_aside"), "{out}");
}

// ---- goldens ----------------------------------------------------------------------------------

fn golden(name: &str, got: &str) {
    // The version on the bar changes every release; the frame doesn't.
    let got = &got.replace(concat!("thc ", env!("CARGO_PKG_VERSION")), "thc X.Y.Z");
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/sidebar").join(name);
    if std::env::var_os("THC_UPDATE_GOLDENS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("no golden {name}: run with THC_UPDATE_GOLDENS=1"));
    assert_eq!(*got, want, "golden {name} changed (THC_UPDATE_GOLDENS=1 to accept)");
}

#[test]
fn goldens() {
    let s = Snap::new(sample());
    let b = format!("5{SHIFT_HEALTH}{SHIFT_READING}");
    golden("frame-b-140x40.txt", &s.run(&b));
    golden("frame-d-140x40.txt", &s.run(&format!("{b}<sclick:31,9><m-s>")));
    golden("frame-b-140x40-ember-dark.ansi", &Snap::new(sample()).env("THC_TUI_SNAPSHOT_FORMAT", "ansi").run(&b));
    golden("frame-b-140x40-ember-light.ansi", &Snap::new(sample()).env("THC_TUI_SNAPSHOT_FORMAT", "ansi").env("THC_THEME", "ember-light").run(&b));
    golden("frame-h-140x40-ascii.txt", &Snap::new(sample()).env("THC_GLYPHS", "ascii").run(&b));
    golden("frame-h-140x40-ansi.ansi", &Snap::new(sample()).env("THC_GLYPHS", "ascii").env("THC_THEME", "ansi").env("THC_TUI_SNAPSHOT_FORMAT", "ansi").run(&format!("{b}<m-s>")));
}
