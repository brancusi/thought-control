//! Navigation history, ⌘[ back and ⌘] forward (navigation.md §7.6): the goldens
//! H1–H14 as snapshots against a scratch vault.

mod common;

use std::path::PathBuf;

struct V {
    root: PathBuf,
}

impl V {
    fn new(name: &str) -> V {
        let root = thc_core::scratch::dir(&format!("thc-history-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let v = V { root };
        v.cli(&["init", "vault"]);
        let pages: Vec<(String, &str)> = [("Alpha", "see [[Beta]] here"), ("Beta", "on to [[Gamma]]"), ("Gamma", "the end"), ("Delta", "elsewhere")]
            .into_iter()
            .map(|(t, body)| (v.json(&["page", "new", t])["nodes"][0]["id"].as_str().unwrap().to_string(), body))
            .collect();
        for (id, body) in pages {
            v.cli(&["add", body, "--under", &id]);
        }
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

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let mut a = vec!["--json"];
        a.extend_from_slice(args);
        serde_json::from_str(&self.cli(&a)).unwrap()
    }

    /// One session (writes kept, so history persists across runs as it does for a person).
    fn tui(&self, keys: &str) -> String {
        let o = self.cmd().args(["tui"]).env("THC_TUI_SNAPSHOT", "100x30").env("THC_TUI_KEYS", keys).env("THC_TUI_SNAPSHOT_WRITE", "1").output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into()
    }

    fn reset(&self) {
        let _ = std::fs::remove_file(self.root.join("history.json"));
    }
}

impl Drop for V {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The page open, from the crumb (`¶ Pages › Alpha`), or the view the tab rule is under.
fn at(f: &str) -> String {
    if let Some(l) = f.lines().find(|l| l.contains("¶ Pages › ")) {
        return l.split("¶ Pages › ").nth(1).unwrap().trim().to_string();
    }
    if let Some(l) = f.lines().find(|l| l.contains("§ Journal › ")) {
        return format!("journal {}", l.split("§ Journal › ").nth(1).unwrap().trim());
    }
    let (tabs, rule): (Vec<char>, Vec<char>) = (f.lines().next().unwrap_or("").chars().collect(), f.lines().nth(1).unwrap_or("").chars().collect());
    for name in ["Today", "Inbox", "Tasks", "Pages", "Journal", "Search", "Log"] {
        let t: Vec<char> = name.chars().collect();
        if let Some(i) = tabs.windows(t.len()).position(|w| w == t.as_slice()) {
            if rule.get(i..i + t.len()).is_some_and(|r| r.contains(&'━')) {
                return name.to_string();
            }
        }
    }
    String::new()
}

const OPEN: fn(&str) -> String = |t| format!("<c-o>{t}<cr>");

#[test]
fn h1_h3_h4_h5_back_forward_dropping_and_coalescing() {
    let v = V::new("basic");
    // H1: Pages, open A, back: the Pages list; forward: A.
    let f = v.tui(&format!("4{}<d-[>", OPEN("Alpha")));
    assert_eq!(at(&f), "Pages", "H1 back: {f}");
    v.reset();
    let f = v.tui(&format!("4{}<d-[><d-]>", OPEN("Alpha")));
    assert_eq!(at(&f), "Alpha", "H1 forward: {f}");
    // H3: A → B → C, back (B), open D: nothing forward (C dropped).
    v.reset();
    let f = v.tui(&format!("{}{}{}<d-[>{}<d-]>", OPEN("Alpha"), OPEN("Beta"), OPEN("Gamma"), OPEN("Delta")));
    assert_eq!(at(&f), "Delta", "{f}");
    assert!(f.contains("nothing forward"), "H3: {f}");
    // H4: A, ⌃O to A again: no new entry; back goes before A.
    v.reset();
    let f = v.tui(&format!("4{}{}<d-[>", OPEN("Alpha"), OPEN("Alpha")));
    assert_eq!(at(&f), "Pages", "H4: {f}");
    // H5: Today, Tab Tab Tab, back: Today in one step.
    v.reset();
    let f = v.tui("1<tab><tab><tab><d-[>");
    assert_eq!(at(&f), "Today", "H5: {f}");
    // At the start: nothing back.
    v.reset();
    let f = v.tui("1<d-[>");
    assert!(f.contains("nothing back"), "{f}");
}

#[test]
fn h2_h9_h10_carets_and_esc() {
    let v = V::new("carets");
    // H2: in A on the link line, follow it, back: A with the caret on that line (type there).
    let f = v.tui(&format!("{}", OPEN("Alpha")));
    let (y, line) = f.lines().enumerate().find(|(_, l)| l.contains("see [[Beta]]")).unwrap();
    let x = line[..line.find("Beta").unwrap()].chars().count() + 1;
    v.reset();
    let f = v.tui(&format!("{}<click:{x},{y}><d-[>", OPEN("Alpha")));
    assert_eq!(at(&f), "Alpha", "H2: {f}");
    v.tui(&format!("{}<click:{x},{y}><d-[><end>X<esc>", OPEN("Alpha")));
    assert!(v.cli(&["q", "text:here"]).contains("here X") || v.cli(&["q", "text:hereX"]).contains("hereX"), "H2: the caret came back on the link's line");
    // H10: Pages → A → link B → link C, Esc: Pages. Then back: C.
    v.reset();
    let f = v.tui(&format!("4{}<click:{x},{y}>", OPEN("Alpha")));
    let (y2, line2) = f.lines().enumerate().find(|(_, l)| l.contains("on to [[Gamma]]")).unwrap();
    let x2 = line2[..line2.find("Gamma").unwrap()].chars().count() + 1;
    v.reset();
    let keys = format!("4{}<click:{x},{y}><click:{x2},{y2}>", OPEN("Alpha"));
    let f = v.tui(&format!("{keys}<esc>"));
    assert_eq!(at(&f), "Pages", "H10 Esc: {f}");
    v.reset();
    let f = v.tui(&format!("{keys}<esc><d-[>"));
    assert_eq!(at(&f), "Gamma", "H10 back: {f}");
    // H9: a long page, a note low down, ⌘↓, back: the same note.
    let id = v.json(&["page", "new", "Long"])["nodes"][0]["id"].as_str().unwrap().to_string();
    for i in 1..=40 {
        v.cli(&["add", &format!("note {i:02}"), "--under", &id]);
    }
    v.reset();
    v.tui(&format!("{}<c-home><down><down><down><end><d-down><d-[>X<esc>", OPEN("Long")));
    assert!(v.cli(&["q", "text:\"note 04X\""]).contains("note 04X") || v.cli(&["q", "text:note"]).contains("note 04X"), "H9: back to note 04: {}", v.cli(&["q", "text:X"]));
}

#[test]
fn h6_days_h11_relaunch_h8_deleted_h12_list_h14_twin() {
    let v = V::new("more");
    // H6: Journal, ⌃P ⌃P, back: the day before; back again: the first.
    let today = v.tui("5");
    let first = at(&today);
    v.reset();
    let f = v.tui("5<c-p><c-p><d-[>");
    let mid = at(&v.tui("5<c-p>"));
    v.reset();
    assert_eq!(at(&f), mid, "H6 one back");
    let f = v.tui("5<c-p><c-p><d-[><d-[>");
    assert_eq!(at(&f), first, "H6 two back");
    // H11: open A, quit; relaunch, back: A.
    v.reset();
    v.tui(&OPEN("Alpha"));
    let f = v.tui("1<d-[>");
    assert_eq!(at(&f), "Alpha", "H11: {f}");
    // H8: A → B, A deleted, back from B: A is skipped, with the note.
    v.reset();
    v.tui(&format!("1{}{}", OPEN("Delta"), OPEN("Beta")));
    let delta = v.json(&["q", "is:page text:Delta"])["items"][0]["id"].as_str().unwrap().to_string();
    v.cli(&["rm", &delta, "--yes"]);
    let f = v.tui("<d-[><d-[>");
    assert!(f.contains("skipped a deleted page"), "H8: {f}");
    assert_eq!(at(&f), "Today", "H8 lands before it: {f}");
    // H12: the list: the third entry, then forward goes to the second (nothing dropped).
    v.reset();
    let keys = format!("1{}{}{}", OPEN("Alpha"), OPEN("Beta"), OPEN("Gamma"));
    let f = v.tui(&format!("{keys}<m-:>history<cr>"));
    let rows: Vec<(usize, &str)> = f.lines().enumerate().filter(|(_, l)| l.contains('│') && l.contains("¶ ") && (l.contains("Alpha") || l.contains("Beta") || l.contains("Gamma"))).collect();
    assert!(rows.len() >= 3, "the list: {f}");
    let (y, l) = rows[2];
    let x = l.find('¶').map(|b| l[..b].chars().count()).unwrap() + 2;
    let f = v.tui(&format!("{keys}<m-:>history<cr><click:{x},{y}>"));
    assert_eq!(at(&f), "Alpha", "H12 jump: {f}");
    let f = v.tui(&format!("{keys}<m-:>history<cr><click:{x},{y}><d-]>"));
    assert_eq!(at(&f), "Beta", "H12 forward: {f}");
    // H14: ⌃⌥← is ⌘['s twin.
    v.reset();
    let f = v.tui(&format!("4{}<c-m-left>", OPEN("Alpha")));
    assert_eq!(at(&f), "Pages", "H14: {f}");
}
