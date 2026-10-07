//! The mouse in documents (docs/design/mouse.md §10, M1–M6): clicks, drags and the wheel as
//! snapshot tokens against a scratch vault, then what was saved.

mod common;

use serde_json::Value;
use std::path::PathBuf;

struct V {
    root: PathBuf,
}

impl V {
    fn new(name: &str) -> V {
        let root = std::env::temp_dir().join(format!("thc-mouse-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let v = V { root };
        v.cli(&["init", "vault"]);
        v
    }

    fn cmd(&self, env: &[(&str, &str)], args: &[&str]) -> std::process::Output {
        let mut c = common::thc();
        c.args(args).current_dir(&self.root).env("THC_VAULT", self.root.join("vault")).env("THC_CACHE_DIR", self.root.join("cache")).env("THC_NOW", "2026-10-03T09:00").env_remove("THC_ACTOR");
        for (k, v) in env {
            c.env(k, v);
        }
        c.output().unwrap()
    }

    fn cli(&self, args: &[&str]) -> String {
        let o = self.cmd(&[], args);
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into()
    }

    /// Type and click into today's journal at 60 columns (writes kept); the frame. (Row 2 is the
    /// crumb, navigation.md §3: the day starts a row lower.)
    fn run(&self, keys: &str) -> String {
        let o = self.cmd(&[("THC_TUI_SNAPSHOT", "60x24"), ("THC_TUI_KEYS", keys), ("THC_TUI_SNAPSHOT_WRITE", "1"), ("THC_TUI_SNAPSHOT_CURSOR", "1")], &["j", "--no-focus"]);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into()
    }

    fn texts(&self) -> Vec<(String, Option<String>)> {
        let v: Value = serde_json::from_str(&self.cli(&["--json", "q", "journal=today sort:date"])).unwrap();
        v["items"].as_array().unwrap().iter().map(|n| (n["text"].as_str().unwrap().to_string(), n["status"].as_str().map(str::to_string))).collect()
    }
}

impl Drop for V {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

const PARA: &str = "The quick brown fox jumps over the lazy dog and keeps running far past the old wooden fence.";

#[test]
fn m1_a_click_on_a_wrapped_row_places_the_caret() {
    // At 60 columns the paragraph wraps after "and"; its second row starts at column 7.
    let v = V::new("m1");
    v.run(&format!("{PARA}<click:20,8>X<esc>"));
    assert_eq!(v.texts()[0].0, PARA.replace("keeps running far", "keeps runningX far"));
}

#[test]
fn m2_the_right_half_of_a_wide_character_goes_after_it() {
    // "a漢字b" at column 7: a=7, 漢=8-9, 字=10-11. The right half of 漢 (9) is after it.
    let v = V::new("m2");
    v.run("a漢字b<click:9,7>X<esc>");
    assert_eq!(v.texts()[0].0, "a漢X字b");
    let v = V::new("m2l");
    v.run("a漢字b<click:8,7>X<esc>");
    assert_eq!(v.texts()[0].0, "aX漢字b", "the left half: before it");
}

#[test]
fn m3_the_wheel_never_moves_the_caret() {
    let v = V::new("m3");
    let many: String = (1..=30).map(|i| format!("- line {i}<cr>")).collect();
    let f = v.run(&format!("{many}<wheel:up:20>"));
    assert!(!f.contains('▮') || f.contains("line 1"), "scrolled up, the caret off-screen: {f}");
    let v = V::new("m3b");
    let f = v.run(&format!("{many}<wheel:up:20>end"));
    assert!(f.contains("end▮"), "typing brings the view back to the caret: {f}");
    let v = V::new("m3c");
    v.run(&format!("{many}<wheel:up:20>end<esc>"));
    let texts = v.texts();
    assert_eq!(texts.last().map(|t| t.0.as_str()), Some("end"), "the text went at the caret (the last line): {texts:?}");
}

#[test]
fn m4_the_task_box_toggles_open_and_done_only() {
    let v = V::new("m4");
    v.run("[ ] call<click:4,7><esc>");
    assert_eq!(v.texts()[0].1.as_deref(), Some("done"));
    v.run("<up><click:4,7><esc>");
    assert_eq!(v.texts()[0].1.as_deref(), Some("todo"), "a second click reopens, never text");
}

#[test]
fn m5_a_plain_click_on_a_link_writes_and_ctrl_click_opens() {
    let v = V::new("m5");
    v.cli(&["page", "new", "Lisbon"]);
    let f = v.run("see [[Lisbon]] soon<click:12,7>");
    assert!(f.contains("SAT 03 OCT"), "a plain click stays in the day: {f}");
    let f = v.run("see [[Lisbon]] soon<cclick:12,7>");
    assert!(f.lines().any(|l| l.trim() == "Lisbon"), "⌃-click opens the page: {f}");
    let f = v.run("see [[Lisbon]] soon<mclick:12,7>");
    assert!(f.lines().any(|l| l.trim() == "Lisbon"), "middle-click opens the page: {f}");
}

#[test]
fn m6_drag_double_and_triple_click_select() {
    // A drag across two paragraphs, then ⌃X: both pieces go.
    let v = V::new("m6");
    v.run("first para<cr><cr>second para<esc>");
    // first para at row 6, blank 7, second para at row 8 (text from column 7).
    v.run("<drag:13,7,13,9><c-x><esc>");
    let t: Vec<String> = v.texts().into_iter().map(|t| t.0).collect();
    assert_eq!(t, ["first  para"], "the selection from `first |` to `second|` went: {t:?}");
    // A double-click selects the word; typing replaces it.
    let v = V::new("m6w");
    v.run("hello world<dclick:15,7>there<esc>");
    assert_eq!(v.texts()[0].0, "hello there");
    // A triple-click selects the note.
    let v = V::new("m6n");
    v.run("hello world<tclick:9,7>new<esc>");
    assert_eq!(v.texts()[0].0, "new");
    // ⇧-click extends from the caret.
    let v = V::new("m6s");
    v.run("hello world<click:7,7><sclick:13,7>X<esc>");
    assert_eq!(v.texts()[0].0, "Xworld");
}

/// Where `needle` is drawn in a frame: (column, row), counting cells (the frame is single-width
/// there).
fn pos(frame: &str, needle: &str) -> (usize, usize) {
    for (y, l) in frame.lines().enumerate() {
        if let Some(b) = l.find(needle) {
            return (l[..b].chars().count(), y);
        }
    }
    panic!("{needle:?} not on screen: {frame}");
}

impl V {
    fn tui(&self, keys: &str) -> String {
        let o = self.cmd(&[("THC_TUI_SNAPSHOT", "100x24"), ("THC_TUI_KEYS", keys), ("THC_TUI_SNAPSHOT_WRITE", "1"), ("THC_NOW", "")], &["tui"]);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into()
    }
}

#[test]
fn m7_m8_chrome_clicks() {
    let v = V::new("chrome");
    v.cli(&["todo", "Buy milk", "--due", "fri"]);
    v.cli(&["page", "new", "Lisbon"]);
    // Tabs switch views.
    let (x, y) = pos(&v.tui(""), "Tasks");
    let f = v.tui(&format!("<click:{x},{y}>"));
    assert!(f.contains("status:open"), "Tasks: {f}");
    // M7: a footer key's click runs its key (F1 opens help, as F1 does).
    let f = v.tui("5");
    let (x, y) = pos(&f, "F1 keys");
    let f = v.tui(&format!("5<click:{x},{y}>"));
    assert!(f.contains("Writing") && f.contains("text → [ ] → [x] → text"), "F1 by click: {f}");
    // M8: a click outside the palette closes it and does nothing underneath.
    let (tx, ty) = pos(&v.tui(""), "Pages");
    let f = v.tui(&format!(":<click:{tx},{ty}>"));
    assert!(!f.contains("Enter run") && f.contains("Next 7 days") || f.contains("Nothing due"), "closed, still Today: {f}");
    // A list row's box toggles it.
    let (x, y) = pos(&v.tui("3"), "[ ] Buy milk");
    v.tui(&format!("3<click:{},{y}>", x + 1));
    let n: serde_json::Value = serde_json::from_str(&v.cli(&["--json", "q", "text:milk", "status:any"])).unwrap();
    assert_eq!(n["items"][0]["status"], "done");
    // The day strip goes to that day.
    // (The real clock here: the strip shows today ± 3 days, so click the one 3 days back.)
    let back = chrono::Local::now().date_naive() - chrono::Duration::days(3);
    let label = back.format("%a %d").to_string().to_uppercase();
    let f = v.tui("5");
    let (x, y) = pos(&f, &label);
    let f = v.tui(&format!("5<click:{x},{y}>"));
    assert!(f.contains(&back.format("%a %d %b %Y").to_string().to_uppercase()), "{f}");
    // The ↗ open chip opens the link; a date chip opens its editor.
    let f = v.tui("5see [[Lisbon]]<left><left>");
    let (x, y) = pos(&f, "↗ open");
    let f = v.tui(&format!("5see [[Lisbon]]<left><left><click:{},{y}>", x + 2));
    assert!(f.lines().any(|l| l.trim() == "Lisbon"), "{f}");
    // (today's journal on the unpinned clock, as the snapshots here run)
    assert!(v.cmd(&[("THC_NOW", "")], &["todo", "Pay rent", "--due", "fri"]).status.success());
    let f = v.tui("5");
    // Its due chip, whatever it reads today (`due fri`, or `due tomorrow` on a Thursday: the
    // snapshot runs on the real clock, as a pinned one would hide the footer behind its warning).
    let (_, y) = pos(&f, "Pay rent");
    let line = f.lines().nth(y).unwrap();
    let x = line[..line.rfind("due ").expect("a due chip on Pay rent's row")].chars().count();
    let f = v.tui(&format!("5<click:{},{y}>", x + 2));
    assert!(f.lines().last().unwrap().contains("due ›"), "{f}");
}

#[test]
fn help_rows_focus_rows_scrollbar_and_hover() {
    let v = V::new("more");
    // A help row runs its key and closes help: ⌃T on the caret's line.
    let f = v.tui("5call<f1>");
    let (x, y) = pos(&f, "⌃T         text");
    let f = v.tui(&format!("5call<f1><click:{x},{y}>"));
    assert!(f.contains("[ ] call") && !f.contains("Writing"), "{f}");
    // A :focus row toggles its element (h: the header, off in bare).
    let f = v.tui("5<m-z><m-:>focus<cr>");
    let (x, y) = pos(&f, "h  [x] header");
    let f = v.tui(&format!("5<m-z><m-:>focus<cr><click:{x},{y}>"));
    assert!(f.contains("h  [ ] header"), "{f}");
    // The scrollbar: a long day overflows; a click low on the track pages down, the caret stays.
    let many: String = (1..=60).map(|i| format!("- line {i}<cr>")).collect();
    let f = v.tui(&format!("5{many}<wheel:up:60>"));
    assert!(f.contains('┃') && f.contains("line 1"), "a scrollbar at the top: {f}");
    let (_, bottom) = pos(&f, "Esc done");
    let f = v.tui(&format!("5{many}<wheel:up:60><click:99,{}>", bottom - 2));
    assert!(!f.contains("- line 1\n") && !f.contains("line 1 "), "paged down: {f}");
    // Hover lights a footer key (colour only: the text is the same). Found, not a fixed column:
    // the version sits at the bar's right edge and moves the keys.
    let (hx, hy) = pos(&v.tui("5"), "F1 keys");
    let plain = v.cmd(&[("THC_TUI_SNAPSHOT", "100x24"), ("THC_TUI_KEYS", "5"), ("THC_TUI_SNAPSHOT_FORMAT", "ansi"), ("THC_NOW", ""), ("THC_THEME", "ember-dark")], &["tui"]);
    let lit = v.cmd(&[("THC_TUI_SNAPSHOT", "100x24"), ("THC_TUI_KEYS", &format!("5<hover:{hx},{hy}>") as &str), ("THC_TUI_SNAPSHOT_FORMAT", "ansi"), ("THC_NOW", ""), ("THC_THEME", "ember-dark")], &["tui"]);
    let last = |o: &std::process::Output| String::from_utf8_lossy(&o.stdout).lines().last().unwrap_or_default().to_string();
    assert_ne!(last(&plain), last(&lit), "hover changes the colour");
}

#[test]
fn colon_mouse_off_stops_clicks_and_on_brings_them_back() {
    let v = V::new("toggle");
    let f = v.tui("5hello world<m-:>mouse off<cr>");
    assert!(f.contains("mouse off · :mouse on to click again"), "{f}");
    // Off: a click on a tab does nothing; on again, it switches.
    let (x, y) = pos(&v.tui(""), "Tasks");
    let f = v.tui(&format!("1:mouse off<cr><click:{x},{y}>"));
    assert!(!f.contains("status:open"), "{f}");
    let f = v.tui(&format!("1:mouse off<cr>:mouse on<cr><click:{x},{y}>"));
    assert!(f.contains("status:open"), "{f}");
}

impl V {
    fn wide(&self, keys: &str) -> String {
        let o = self.cmd(&[("THC_TUI_SNAPSHOT", "140x30"), ("THC_TUI_KEYS", keys), ("THC_TUI_SNAPSHOT_WRITE", "1"), ("THC_TUI_RENDER", "1")], &["tui"]);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into()
    }
}

#[test]
fn the_detail_pane_links_and_fields_click() {
    let v = V::new("detail");
    let page: Value = serde_json::from_str(&v.cli(&["--json", "page", "new", "Lisbon"])).unwrap();
    let pid = page["nodes"][0]["id"].as_str().unwrap().to_string();
    v.cli(&["todo", "Call the landlord", "--due", "fri", "-p", "high", "-t", "lisbon", "--under", &pid]);
    // Today, the task selected: its detail pane at the right.
    let f = v.wide("1");
    assert!(f.contains("¶ Lisbon ›"), "{f}");
    // The path's page opens it.
    let (x, y) = pos(&f, "¶ Lisbon ›");
    let f = v.wide(&format!("1<click:{},{y}>", x + 2));
    assert!(f.lines().any(|l| l.contains("Lisbon") && !l.contains('›') && !l.contains("Today")), "the page: {f}");
    // The due value opens its prompt, prefilled as a person writes it.
    let f = v.wide("1");
    let (x, y) = pos(&f, "Fri Oct 9");
    let f = v.wide(&format!("1<click:{x},{y}>"));
    assert!(f.lines().last().unwrap().contains("due ›") && f.contains("fri"), "{f}");
    // priority asks h / m / l; tags open the tags prompt.
    let f = v.wide("1");
    let (x, y) = pos(&f, "priority   ");
    let (tx, ty) = pos(&f, "tags       ");
    let pf = v.wide(&format!("1<click:{},{y}>", x + 12));
    assert!(pf.lines().last().unwrap().contains("p…") && pf.contains("h high  m med  l low  - none"), "{pf}");
    let tf = v.wide(&format!("1<click:{},{ty}>", tx + 12));
    assert!(tf.lines().last().unwrap().contains("lisbon"), "{tf}");
    // The key hints at the bottom are buttons: `x done` completes it.
    let (x, y) = pos(&f, "x done");
    v.wide(&format!("1<click:{x},{y}>"));
    let n: Value = serde_json::from_str(&v.cli(&["--json", "q", "text:landlord", "status:any"])).unwrap();
    assert_eq!(n["items"][0]["status"], "done");
}

#[test]
fn a_long_list_has_a_scrollbar_that_pages() {
    let v = V::new("listbar");
    let plan: String = (1..=40).map(|i| format!("{{\"cmd\":\"todo\",\"text\":\"task {i}\",\"due\":\"+{i}d\"}}\n")).collect();
    let mut c = common::thc();
    c.args(["apply", "-"]).current_dir(&v.root).env("THC_VAULT", v.root.join("vault")).env("THC_CACHE_DIR", v.root.join("cache")).env("THC_NOW", "2026-10-03T09:00").stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null());
    let mut ch = c.spawn().unwrap();
    use std::io::Write;
    ch.stdin.take().unwrap().write_all(plan.as_bytes()).unwrap();
    assert!(ch.wait().unwrap().success());
    let f = v.wide("3");
    assert!(f.contains('┃') && f.contains("task 1 "), "{f}");
    // A click low on the track pages down: the cursor moves a page.
    let (_, last) = pos(&f, "saved");
    let f = v.wide(&format!("3<click:139,{}>", last - 1));
    assert!(!f.contains("task 1 "), "paged: {f}");
}

#[test]
fn list_date_chips_and_task_columns_open_their_prompts() {
    let v = V::new("chips");
    v.cli(&["todo", "Pay rent", "--due", "fri"]);
    v.cli(&["todo", "Buy milk", "--due", "mon"]);
    // Today: the second row's `due mon` chip selects it and opens its due prompt.
    let f = v.wide("1");
    let (x, y) = pos(&f, "due mon");
    let f = v.wide(&format!("1<click:{},{y}>", x + 4));
    let bar = f.lines().last().unwrap();
    assert!(bar.contains("due ›") && bar.contains("mon"), "{f}");
    // Tasks: the sched column opens the schedule prompt (empty: none set).
    let f = v.wide("3");
    let (x, _) = pos(&f, "sched");
    let (_, y) = pos(&f, "Pay rent");
    let f = v.wide(&format!("3<click:{},{y}>", x + 1));
    assert!(f.lines().last().unwrap().contains("sched"), "{f}");
}

#[test]
fn a_history_row_opens_the_full_history_on_that_change() {
    let v = V::new("hist");
    v.cli(&["todo", "Pay rent", "--due", "fri"]);
    let id: Value = serde_json::from_str(&v.cli(&["--json", "q", "text:rent"])).unwrap();
    let id = id["items"][0]["id"].as_str().unwrap().to_string();
    v.cli(&["tag", &id, "home"]);
    let f = v.wide("1");
    // History, newest first: `tagged #home`, then `created`. Click `created`.
    let y = f.lines().position(|l| l.trim_end().ends_with("you     created")).expect("the created row");
    let f = v.wide(&format!("1<click:90,{y}>"));
    let cur = f.lines().find(|l| l.starts_with('▌')).unwrap_or_default().to_string();
    assert!(cur.contains("created"), "the cursor on the created change: {f}");
    let f = v.wide("1");
    let y = f.lines().position(|l| l.trim_end().ends_with("you     tagged #home")).expect("the tagged row");
    let f = v.wide(&format!("1<click:90,{y}>"));
    let cur = f.lines().find(|l| l.starts_with('▌')).unwrap_or_default().to_string();
    assert!(cur.contains("tagged"), "the cursor on the tag change: {f}");
}

#[test]
fn a_click_on_a_link_follows_it() {
    // editing.md E63–E66, E69. E67/E68 (⇧-click → the sidebar) come with the sidebar.
    let v = V::new("follow");
    v.tui("5see [[Lisbon flat]] today #lisbon<esc>");
    let f = v.tui("5");
    let (x, y) = pos(&f, "Lisbon flat");
    let page = |f: &str| f.lines().any(|l| l.trim() == "Lisbon flat");
    let journal = |f: &str| f.contains("· TODAY") && !page(f);
    // E63: a click on the title follows it, and Esc comes back.
    let f = v.tui(&format!("5<click:{},{y}>", x + 2));
    assert!(page(&f) && !journal(&f), "followed: {f}");
    let f = v.tui(&format!("5<click:{},{y}><esc>", x + 2));
    assert!(journal(&f), "Esc comes back: {f}");
    // A drag that starts on the title selects instead.
    let f = v.tui(&format!("5<drag:{},{y},{},{y}>", x + 1, x + 6));
    assert!(journal(&f), "a drag doesn't follow: {f}");
    // Hover underlines the title, and only the title.
    let ansi = |keys: String| String::from_utf8_lossy(&v.cmd(&[("THC_TUI_SNAPSHOT", "100x24"), ("THC_TUI_KEYS", &keys), ("THC_TUI_SNAPSHOT_FORMAT", "ansi"), ("THC_NOW", ""), ("THC_THEME", "ember-dark")], &["tui"]).stdout).lines().nth(y).unwrap_or_default().to_string();
    let lit = ansi(format!("5<hover:{},{y}>", x + 2));
    assert_ne!(ansi("5".into()), lit, "hover changes the link");
    assert_eq!(ansi("5".into()), ansi(format!("5<hover:{},{y}>", x - 3)), "hover on plain text changes nothing");
    // E66: the keyboard enters a link without following it; ⌃O follows.
    let f = v.tui(&format!("5<aclick:{},{y}><right><right><right>", x - 3));
    assert!(journal(&f), "{f}");
    let f = v.tui(&format!("5<aclick:{},{y}><right><right><right><c-o>", x - 3));
    assert!(page(&f), "⌃O follows: {f}");
    // E69: a tag only places the caret.
    let (tx, _) = pos(&f_journal(&v), "#lisbon");
    let f = v.tui(&format!("5<click:{},{y}>", tx + 2));
    assert!(journal(&f), "a tag doesn't navigate: {f}");
    // E64: on the `[[` the caret is placed; E65: ⌥-click on the title too. (Writes: last.)
    v.tui(&format!("5<click:{},{y}>X<esc>", x - 2));
    v.tui(&format!("5<aclick:{},{y}>Y<esc>", x + 4));
    let t: Value = serde_json::from_str(&v.cli(&["--json", "q", "text:see"])).unwrap();
    assert_eq!(t["items"][0]["text"], "see X[[LisYbon flat]] today #lisbon");
}

fn f_journal(v: &V) -> String {
    v.tui("5")
}
