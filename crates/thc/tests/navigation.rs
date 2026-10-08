//! Finding your way (docs/design/navigation.md §4): the Pages tab is the list.

mod common;

use std::path::PathBuf;

struct V {
    root: PathBuf,
}

impl V {
    fn new(name: &str) -> V {
        let root = thc_core::scratch::dir(&format!("thc-nav-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let v = V { root };
        v.run(&[], &["init", "vault"]);
        v
    }

    fn run(&self, env: &[(&str, &str)], args: &[&str]) -> String {
        let mut c = common::thc();
        c.args(args).current_dir(&self.root).env("THC_VAULT", self.root.join("vault")).env("THC_CACHE_DIR", self.root.join("cache")).env("THC_NOW", "2026-10-03T09:00").env_remove("THC_ACTOR");
        for (k, v) in env {
            c.env(k, v);
        }
        let o = c.output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into()
    }

    fn frame(&self, keys: &str) -> String {
        self.run(&[("THC_TUI_SNAPSHOT", "110x30"), ("THC_TUI_KEYS", keys)], &["tui"])
    }
}

/// The row the list's cursor is on.
fn selected(frame: &str) -> String {
    frame.lines().find(|l| l.starts_with('▌')).unwrap_or_else(|| panic!("no selected row:\n{frame}")).to_string()
}

#[test]
fn n1_n2_the_pages_tab_is_always_the_list() {
    let v = V::new("n1");
    v.run(&[], &["add", "see [[Alpha]] and [[Beta]]"]);
    // Open Beta from the list, Esc, then 4: the list, with Beta under the cursor.
    let f = v.frame("4<down><cr><esc>4");
    assert!(f.contains("Pages  2"), "not the list:\n{f}");
    assert!(selected(&f).contains("Beta"), "{f}");
    // After other views: still the list.
    let f = v.frame("4<down><cr><esc>2<esc>3<esc>4");
    assert!(f.contains("Pages  2") && selected(&f).contains("Beta"), "{f}");
    // Enter on it reopens the page.
    let f = v.frame("4<down><cr><esc>4<cr>");
    assert!(!f.contains("Pages  2") && f.contains("Beta"), "{f}");
}

#[test]
fn n3_n4_n9_esc_goes_back_where_you_came_from() {
    let v = V::new("n3");
    v.run(&[], &["add", "see [[Alpha]] and [[Beta]]"]);
    v.run(&[], &["todo", "Water plants", "--due", "today"]);
    v.run(&[], &["todo", "Pay rent", "--due", "today"]);
    // N3: Alpha from the list, ⌃O to another page (the finder: Beta), Esc → the list, Alpha
    // still selected (the chain started there).
    let f = v.frame("4<cr><c-o>Beta<cr><esc>");
    assert!(f.contains("Pages  2"), "not the list:\n{f}");
    assert!(selected(&f).contains("Alpha"), "{f}");
    // N4: a row opened from Today (the second) → Esc → Today, that row selected.
    let f = v.frame("1<down><cr><esc>");
    assert!(f.contains("Today  2"), "not Today:\n{f}");
    let sel = selected(&f);
    assert!(sel.contains("Pay rent") || sel.contains("Water plants"), "{f}");
    let second = f.lines().filter(|l| l.contains("[ ]")).nth(1).unwrap().to_string();
    assert!(second.starts_with('▌'), "the second row should be selected:\n{f}");
    // N9: `thc p Beta`, Esc → Today.
    let f = v.run(&[("THC_TUI_SNAPSHOT", "110x30"), ("THC_TUI_KEYS", "<esc>")], &["p", "Beta"]);
    assert!(f.lines().next().is_some_and(|l| l.contains("Today")) && !f.contains("Pages  2"), "{f}");
}

impl V {
    fn frame_at(&self, size: &str, args: &[&str], keys: &str, write: bool) -> String {
        let mut env = vec![("THC_TUI_SNAPSHOT", size), ("THC_TUI_KEYS", keys)];
        if write {
            env.push(("THC_TUI_SNAPSHOT_WRITE", "1"));
        }
        self.run(&env, args)
    }
}

#[test]
fn n5_n6_n7_n8_the_rail_and_the_crumb() {
    let v = V::new("n5");
    v.run(&[], &["add", "see [[Alpha]] and [[Beta]] and [[Gamma]]"]);
    // N5: at 124 columns a page has the rail, the current page marked.
    let f = v.frame_at("124x30", &["tui"], "4<cr>", false);
    let rail: Vec<&str> = f.lines().skip(2).map(|l| l.split('│').next().unwrap_or("")).collect();
    assert!(rail[0].contains("PAGES"), "{f}");
    assert!(rail.iter().any(|l| l.starts_with('▌') && l.contains("Alpha")), "{f}");
    assert!(rail.iter().any(|l| l.contains("all pages: 3")), "{f}");
    // Clicking another page opens it, and what was typed on the first is saved.
    let gamma_y = f.lines().position(|l| l.split('│').next().is_some_and(|r| r.contains("Gamma"))).unwrap();
    let f = v.frame_at("124x30", &["tui"], &format!("4<cr><down>a note on alpha<click:5,{gamma_y}>"), true);
    assert!(f.lines().any(|l| l.starts_with('▌') && l.contains("Gamma")), "{f}");
    let found = v.run(&[], &["--json", "search", "a note on alpha"]);
    assert!(found.contains("a note on alpha"), "the first page's edit wasn't saved: {found}");
    // N6: at 100 columns no rail, a crumb; clicking `Pages` goes to the list.
    let f = v.frame_at("100x30", &["tui"], "4<cr>", false);
    assert!(!f.contains("PAGES") && f.contains("¶ Pages › Alpha"), "{f}");
    let (y, line) = f.lines().enumerate().find(|(_, l)| l.contains("¶ Pages ›")).unwrap();
    let x = line.chars().position(|c| c == 'P').unwrap();
    let f = v.frame_at("100x30", &["tui"], &format!("4<cr><click:{x},{y}>"), false);
    assert!(f.contains("Pages  3"), "the crumb didn't open the list: {f}");
    // N7: a day at 124 has a rail of days.
    let f = v.frame_at("124x30", &["j", "--no-focus"], "", false);
    assert!(f.contains("DAYS") && f.lines().any(|l| l.starts_with("▌today")), "{f}");
    let (y, _) = f.lines().enumerate().find(|(_, l)| l.contains("any day: ⌃O")).expect("the any-day row");
    let f = v.frame_at("124x30", &["j", "--no-focus"], &format!("<click:5,{y}>"), false);
    assert!(f.contains("╭─ open "), "the finder didn't open: {f}");
    // N8: Focus hides both; `nav` brings them back.
    let f = v.frame_at("124x30", &["j"], "", false);
    assert!(!f.contains("DAYS") && !f.contains("§ Journal ›"), "{f}");
    let f = v.run(&[("THC_TUI_SNAPSHOT", "124x30"), ("THC_TUI_FOCUS", "+nav")], &["j"]);
    assert!(f.contains("DAYS"), "{f}");
}

/// The Pages list opens with its find-as-you-type prompt, which swallowed every
/// click. A page double-clicked open never opened, and typing went into the hidden filter
/// until Escape. Now a click steps the prompt aside and typing lands at once.
#[test]
fn a_page_double_clicked_open_takes_typing_at_once() {
    let v = V::new("dclick");
    v.run(&[], &["add", "see [[Alpha]] and [[Beta]]"]);
    let f = v.frame("4");
    let (y, line) = f.lines().enumerate().find(|(_, l)| l.contains("¶ Beta")).expect("Beta in the list");
    let x = line.find("Beta").unwrap();
    let f = v.run(&[("THC_TUI_SNAPSHOT", "110x30"), ("THC_TUI_KEYS", &format!("4<dclick:{x},{y}>typed at once")), ("THC_TUI_SNAPSHOT_WRITE", "1")], &["tui"]);
    v.run(&[("THC_TUI_SNAPSHOT", "110x30"), ("THC_TUI_KEYS", &format!("4<dclick:{x},{y}>typed at once<esc>")), ("THC_TUI_SNAPSHOT_WRITE", "1")], &["tui"]);
    assert!(f.lines().any(|l| l.contains("typed at once")) && !f.contains("› typed"), "{f}");
    let found = v.run(&[], &["--json", "search", "typed at once"]);
    assert!(found.contains("\"typed at once\""), "saved on Beta: {found}");
}

/// writing.md §1, "the caret remembers": leave a page and come back, by any
/// path, and the caret is where it was. A day never opened here still starts at the end.
#[test]
fn the_caret_comes_back_where_it_was() {
    let v = V::new("caret");
    v.run(&[], &["page", "new", "Plans"]);
    let plans = |keys: &str, write: bool| {
        let mut env = vec![("THC_TUI_SNAPSHOT", "110x30"), ("THC_TUI_KEYS", keys), ("THC_TUI_SNAPSHOT_CURSOR", "1"), ("THC_TUI_SNAPSHOT_CARETS", "1")];
        if write {
            env.push(("THC_TUI_SNAPSHOT_WRITE", "1"));
        }
        v.run(&env, &["p", "Plans", "--no-focus"])
    };
    // Three lines, then the caret into the middle of the second, and away.
    plans("first line<cr>second line<cr>third line<up><home><right><right><right><esc>", true);
    // Back by thc p: the caret is on `second`, after `sec`.
    let f = plans("", false);
    let caret = f.lines().find(|l| l.contains('▮')).unwrap_or_default().to_string();
    assert!(caret.contains("sec▮nd line"), "{f}");
    // Back through the Pages list: the same place.
    let f = v.run(&[("THC_TUI_SNAPSHOT", "110x30"), ("THC_TUI_KEYS", "4<cr>"), ("THC_TUI_SNAPSHOT_CURSOR", "1"), ("THC_TUI_SNAPSHOT_CARETS", "1")], &["tui"]);
    assert!(f.lines().any(|l| l.contains("sec▮nd line")), "{f}");
}

/// navigation.md §6, N10–N15: arriving never takes the cursor, Tab always
/// cycles, and in the finders (Pages, Search) letters find.
#[test]
fn n10_to_n15_arriving_never_takes_the_cursor_and_tab_cycles() {
    let v = V::new("n10");
    v.run(&[], &["add", "see [[Lisbon trip]] and [[Alpha]]"]);
    v.run(&[], &["todo", "a task"]);
    let f = |keys: &str| v.run(&[("THC_TUI_SNAPSHOT", "110x30"), ("THC_TUI_KEYS", keys), ("THC_TUI_SNAPSHOT_CURSOR", "1")], &["tui"]);
    let header = |frame: &str| frame.lines().nth(2).unwrap_or_default().to_string();
    // N10: no caret on arrival at Pages or Search; Tab goes on.
    for (key, next) in [("4", "Journal"), ("6", "Log")] {
        let a = f(key);
        assert!(!header(&a).contains('▮'), "no caret on arrival ({key}): {a}");
        let b = f(&format!("{key}<tab>"));
        let tabs = b.lines().nth(1).unwrap_or_default().to_string();
        let under = b.lines().next().unwrap_or_default().find(next).unwrap();
        assert!(tabs.chars().skip(under).take(3).any(|c| c == '━'), "Tab from {key} goes to {next}: {b}");
    }
    // N11: letters find; Enter opens the top match.
    let a = f("4lis");
    assert!(header(&a).contains("lis▮"), "{a}");
    assert!(f("4lis<cr>").contains("¶ Pages › Lisbon trip"));
    // N12: with a query, Tab moves on; ⇧Tab comes back to the query, no caret.
    let a = f("6lisbon<tab><s-tab>");
    assert!(header(&a).contains("lisbon") && !header(&a).contains('▮'), "{a}");
    // N13: a click selects; the next letters still find, nothing opens.
    let a = f("4<click:10,4>hello");
    assert!(header(&a).contains("hello▮") && !a.contains("¶ Pages ›"), "{a}");
    // N14: with the query empty a digit is a view; with a query it types.
    assert!(f("43").lines().nth(2).unwrap_or_default().contains("q›"), "3 → Tasks");
    assert!(header(&f("4a3")).contains("a3▮"));
    // N15: other lists keep their letters; f opens the filter, Esc closes it.
    let a = f("3f");
    assert!(header(&a).contains('▮'), "{a}");
    assert!(!header(&f("3f<esc>")).contains('▮'));
}

/// The view a frame shows: the tab with the heavy rule under it.
fn current_view(frame: &str) -> String {
    let mut l = frame.lines();
    let (tabs, rule) = (l.next().unwrap_or_default(), l.next().unwrap_or_default());
    let rule: Vec<char> = rule.chars().collect();
    let tabs_chars: Vec<char> = tabs.chars().collect();
    for name in ["Today", "Inbox", "Tasks", "Pages", "Journal", "Search", "Log"] {
        let t: Vec<char> = name.chars().collect();
        if let Some(i) = tabs_chars.windows(t.len()).position(|w| w == t.as_slice()) {
            if rule.get(i..i + t.len()).is_some_and(|r| r.contains(&'━')) {
                return name.to_string();
            }
        }
    }
    String::new()
}

/// navigation.md §6.1, N16–N20: a document arrived at is parked, so Tab keeps
/// changing views; any other key starts writing.
#[test]
fn n16_to_n20_documents_park_on_arrival() {
    let v = V::new("n16");
    v.run(&[], &["page", "new", "Plans"]);
    let f = |keys: &str| v.run(&[("THC_TUI_SNAPSHOT", "110x30"), ("THC_TUI_KEYS", keys)], &["tui"]);
    let log = || v.run(&[], &["--json", "log"]).len();
    let before = log();
    // N16: Tab ×7 round the views and back, ⇧Tab ×7 the other way, nothing written.
    let order = ["Inbox", "Tasks", "Pages", "Journal", "Search", "Log", "Today"];
    for (i, name) in order.iter().enumerate() {
        assert_eq!(current_view(&f(&"<tab>".repeat(i + 1))), *name, "Tab ×{}", i + 1);
    }
    let back = ["Log", "Search", "Journal", "Pages", "Tasks", "Inbox", "Today"];
    for (i, name) in back.iter().enumerate() {
        assert_eq!(current_view(&f(&"<s-tab>".repeat(i + 1))), *name, "⇧Tab ×{}", i + 1);
    }
    assert_eq!(log(), before, "cycling wrote nothing");
    // N17: Tab to Journal, then typing writes there and Tab nests (writing has started).
    let a = f("<tab><tab><tab><tab>- one<cr>two<tab>");
    assert_eq!(current_view(&a), "Journal", "{a}");
    assert!(a.contains("one") && a.contains("two"), "{a}");
    // N18: 5 then 3: the 3 is typed, not Tasks.
    let a = f("53");
    assert_eq!(current_view(&a), "Journal");
    assert!(a.lines().any(|l| l.trim() == "3" || l.trim().ends_with(" 3")), "{a}");
    // N19: 5, x, Esc → Today; then Tab → Inbox.
    assert_eq!(current_view(&f("5x<esc><tab>")), "Inbox");
    // N20: a page opened from Pages: Tab → Journal, ⇧Tab → Tasks.
    assert_eq!(current_view(&f("4plans<cr><tab>")), "Journal");
    assert_eq!(current_view(&f("4plans<cr><s-tab>")), "Tasks");
}

/// keymap.md §6: the footer ends with the version, dim, when it fits; it's the
/// first thing dropped when narrow, before any key hint. Pinned here (THC_TUI_VERSION).
#[test]
fn the_footer_shows_the_version_when_it_fits() {
    let v = V::new("version");
    let bar = |size: &str, args: &[&str]| v.run(&[("THC_TUI_SNAPSHOT", size), ("THC_TUI_VERSION", "9.9.9"), ("THC_TUI_KEYS", ""), ("THC_NOW", "")], args).lines().last().unwrap_or_default().to_string();
    for args in [&["tui"][..], &["j", "--no-focus"][..]] {
        let wide = bar("120x24", args);
        assert!(wide.trim_end().ends_with("thc 9.9.9"), "{args:?} at 120: {wide}");
        let narrow = bar("60x24", args);
        assert!(!narrow.contains("thc 9.9.9"), "{args:?} at 60: {narrow}");
    }
    // At 60 the document's hints are all still there.
    assert!(bar("60x24", &["j", "--no-focus"]).contains("Esc done"));
}
