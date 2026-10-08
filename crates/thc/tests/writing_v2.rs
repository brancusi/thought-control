//! Writing v2 acceptance (docs/design/writing.md §7, A1–A21): each test types into `thc j` as a
//! snapshot that writes to its scratch vault, then reads what was saved through the CLI.

mod common;

use serde_json::Value;
use std::path::{Path, PathBuf};

struct V {
    root: PathBuf,
}

impl V {
    fn new(name: &str) -> V {
        let root = thc_core::scratch::dir(&format!("thc-w2-{name}-{}", std::process::id()));
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

    fn json(&self, args: &[&str]) -> Value {
        let mut a = vec!["--json"];
        a.extend_from_slice(args);
        serde_json::from_str(&self.cli(&a)).unwrap()
    }

    /// Type `keys` into today's journal (writes kept), return the frame.
    fn type_in(&self, keys: &str) -> String {
        self.type_args(keys, &["j", "--no-focus"])
    }

    fn type_args(&self, keys: &str, args: &[&str]) -> String {
        let o = self.cmd(&[("THC_TUI_SNAPSHOT", "110x30"), ("THC_TUI_KEYS", keys), ("THC_TUI_SNAPSHOT_WRITE", "1"), ("THC_TUI_SNAPSHOT_CURSOR", "1")], args);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into()
    }

    /// Today's notes, in order: (text, status, id).
    fn day(&self) -> Vec<(String, Option<String>, String)> {
        let v = self.json(&["q", "journal=today sort:date"]);
        v["items"].as_array().unwrap().iter().map(|n| (n["text"].as_str().unwrap().to_string(), n["status"].as_str().map(str::to_string), n["id"].as_str().unwrap().to_string())).collect()
    }

    fn texts(&self) -> Vec<String> {
        self.day().into_iter().map(|d| d.0).collect()
    }

    /// A note written as a paragraph (`style=para`) under today, as the editor makes them.
    fn para(&self, text: &str) -> String {
        let id = self.json(&["add", text])["nodes"][0]["id"].as_str().unwrap().to_string();
        self.cli(&["set", &id, "style=para"]);
        id
    }

    fn events(&self) -> usize {
        self.json(&["log", "--limit", "1000"])["events"].as_array().map(|a| a.len()).unwrap_or(0)
    }
}

impl Drop for V {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn status_of(v: &V, id: &str) -> Value {
    v.json(&["show", id])
}

#[test]
fn a1_a_blank_day_is_ready() {
    let v = V::new("a1");
    // THC_NOW="" unpins the clock: a pinned clock's warning owns the bar.
    let o = v.cmd(&[("THC_TUI_SNAPSHOT", "110x30"), ("THC_TUI_KEYS", ""), ("THC_NOW", ""), ("THC_TUI_SNAPSHOT_CURSOR", "1")], &["j", "--no-focus"]);
    let f = String::from_utf8_lossy(&o.stdout).to_string();
    let bar = f.lines().last().unwrap();
    assert!(bar.contains("⌃T task  ⌃O open  ⌃P ⌃N day  Esc done  F1 keys"), "{bar}");
    assert!(bar.contains("just type"), "{bar}");
    assert!(f.contains("▮"), "the caret is in the day: {f}");
}

#[test]
fn a2_a3_enter_makes_a_note_and_shift_enter_breaks_the_line() {
    // The Logseq model (writing.md §1, 2026-10-08): Enter starts a note, ⇧Enter (⌃J) breaks
    // the line inside one.
    let v = V::new("a2");
    v.type_in("one<c-j>two<esc>");
    assert_eq!(v.texts(), ["one\ntwo"], "A2: one note");
    let v = V::new("a3");
    v.type_in("one<cr>two<esc>");
    assert_eq!(v.texts(), ["one", "two"], "A3: two notes");
    let v = V::new("a3b");
    v.type_in("one<cr><cr>two<esc>");
    assert_eq!(v.texts(), ["one", "two"], "A3: Enter on the empty note does nothing: no empty one");
}

#[test]
fn a4_a5_split_keeps_the_id_and_join_keeps_the_upper() {
    let v = V::new("a4");
    let id = v.para("abcdef");
    v.type_in("<up><home><right><right><right><cr><cr><esc>");
    let day = v.day();
    assert_eq!(day.iter().map(|d| d.0.as_str()).collect::<Vec<_>>(), ["abc", "def"]);
    assert_eq!(day[0].2, id, "A4: the first part keeps the id");
    let h = v.cli(&["history", &id]);
    assert!(h.contains("node.text"), "an edit, not a create: {h}");
    // A5: ⌫ at the start of the second joins it into the first.
    v.type_in("<up><home><bs><esc>");
    let day = v.day();
    assert_eq!(day.iter().map(|d| d.0.as_str()).collect::<Vec<_>>(), ["abc\ndef"]);
    assert_eq!(day[0].2, id, "A5: the upper id survives");
}

#[test]
fn a6_a7_a8_lists_continue_and_end() {
    let v = V::new("a6");
    v.type_in("- milk<cr><cr>after<esc>");
    let day = v.day();
    assert_eq!(day.iter().map(|d| d.0.as_str()).collect::<Vec<_>>(), ["milk", "after"], "A6: the empty item ended the list");
    let v = V::new("a7");
    let f = v.type_in("1. a<cr>b");
    assert!(f.contains("    1. a") && f.contains("    2. b"), "A7: numbers in the hang: {f}");
    let v = V::new("a8");
    v.type_in("[ ] call<cr>pay<esc>");
    let day = v.day();
    assert!(day.iter().all(|d| d.1.as_deref() == Some("todo")), "A8: both tasks: {day:?}");
}

#[test]
fn a9_a10_the_marker_is_the_truth() {
    let v = V::new("a9");
    let id = v.json(&["todo", "call", "--due", "fri"])["nodes"][0]["id"].as_str().unwrap().to_string();
    v.type_in("<up><c-t><c-t><esc>");
    let n = status_of(&v, &id);
    assert!(n["status"].is_null() && n["due"].is_null(), "A9: [x] → text clears the status and its date: {n}");
    let v = V::new("a10");
    let id = v.json(&["todo", "call"])["nodes"][0]["id"].as_str().unwrap().to_string();
    v.type_in("<up><home><bs><bs><esc>");
    let n = status_of(&v, &id);
    assert!(n["status"].is_null(), "A10: plain, no status: {n}");
    assert_eq!(v.texts(), ["call"]);
}

#[test]
fn a11_a_selection_cycles_together() {
    let v = V::new("a11");
    for t in ["a", "b", "c"] {
        v.para(t);
    }
    // The first Esc clears the selection, the second saves and goes to Today.
    v.type_in("<up><up><up><home><s-down><s-down><s-right><c-t><esc><esc>");
    let day = v.day();
    assert!(day.iter().all(|d| d.1.as_deref() == Some("todo")), "every selected line a task: {day:?}");
}

#[test]
fn a12_a13_a14_a15_links() {
    let v = V::new("a12");
    let before = v.events();
    v.type_in("see [[Lisbon]] soon<esc>");
    let pages = v.json(&["q", "is:page"]);
    assert!(pages["items"].as_array().unwrap().iter().any(|p| p["title"] == "Lisbon"), "A12: the page exists");
    assert!(v.events() > before);
    let txs: Vec<String> = v.json(&["log", "--limit", "50"])["events"].as_array().unwrap().iter().filter_map(|e| e["tx"].as_str().map(str::to_string)).collect();
    let mut uniq = txs.clone();
    uniq.dedup();
    assert!(uniq.len() <= 2, "A12: the line and its page in one transaction (plus the day): {uniq:?}");
    // A13: case-insensitive.
    v.type_in("and [[lisbon]] too<esc>");
    let pages = v.json(&["q", "is:page"]);
    assert_eq!(pages["items"].as_array().unwrap().len(), 1, "A13: no second page");
    // A14: a near miss offers the existing page; ⌃O takes it and no stub stays.
    // (each half in its own vault: typing writes, so the first would leave its stub behind)
    let w = V::new("a14a");
    w.cli(&["page", "new", "Lisbon"]);
    let f = w.type_in("go [[Lisbn]] now<cr><cr>x");
    assert!(f.contains("new page \"Lisbn\" · ⌃O Lisbon?"), "A14: the chip: {f}");
    let w = V::new("a14b");
    w.cli(&["page", "new", "Lisbon"]);
    w.type_in("go [[Lisbn]] now<cr><cr>x<c-o><esc>");
    let titles: Vec<String> = w.json(&["q", "is:page"])["items"].as_array().unwrap().iter().filter_map(|p| p["title"].as_str().map(str::to_string)).collect();
    assert_eq!(titles, ["Lisbon"], "A14: no stub");
    assert!(w.texts().iter().any(|t| t.contains("[[Lisbon]]")), "A14: the link rewritten: {:?}", w.texts());
    // A15: ⌃O on a link opens the page.
    let f = v.type_in("visit [[Lisbon]]<left><left><left><c-o>");
    assert!(f.lines().any(|l| { let l = l.trim(); l == "Lisbon" || l.ends_with(" › Lisbon") }), "A15: the page: {f}");
    assert!(v.texts().iter().any(|t| t.starts_with("visit")), "A15: the line was saved first");
}

#[test]
fn a16_a17_a18_keys() {
    let v = V::new("a16");
    let f = v.type_in("<c-p>");
    assert!(f.contains("FRI 02 OCT 2026"), "A16: the day before: {f}");
    let f = v.type_in("<c-n>");
    assert!(f.contains("SUN 04 OCT 2026"), "A16: the day after: {f}");
    let f = v.type_in("hello<esc>");
    assert!(f.contains("Next 7 days") || f.contains("Nothing due"), "A17: Today: {f}");
    assert_eq!(v.texts(), ["hello"], "A17: saved");
    let id = v.day()[0].2.clone();
    v.type_in("<up><m-x><c-s><c-f><esc>");
    let n = status_of(&v, &id);
    assert!(n["status"].is_null() && n["scheduled"].is_null(), "A18: nothing ran: {n}");
    assert_eq!(v.texts(), ["hello"], "A18: nothing typed");
}

#[test]
fn a20_a_legacy_day_loads_with_no_writes() {
    let v = V::new("a20");
    for t in ["first para", "second para", "third para"] {
        v.para(t);
    }
    v.cli(&["add", "plain one"]);
    v.cli(&["add", "plain two"]);
    let before = v.events();
    let f = v.type_in("<esc>");
    assert_eq!(v.events(), before, "A20: nothing saved");
    let open = v.type_args("", &["j", "--no-focus"]);
    let rows: Vec<&str> = open.lines().map(str::trim_end).collect();
    let at = |t: &str| rows.iter().position(|l| l.contains(t)).unwrap();
    assert!(at("second para") >= at("first para") + 2 && at("third para") >= at("second para") + 2, "paragraphs apart: {open}");
    assert!(rows[at("plain one")].trim_start().starts_with('·'), "plain notes as items: {open}");
    let _ = f;
}

#[test]
fn a21_a_pasted_block_is_one_transaction() {
    let v = V::new("a21");
    let before: Vec<String> = Vec::new();
    v.type_in("<paste:Intro line\n\n- one\n- two\n[ ] task><esc>");
    let day = v.day();
    let texts: Vec<&str> = day.iter().map(|d| d.0.as_str()).collect();
    assert!(texts.contains(&"Intro line") && texts.contains(&"one") && texts.contains(&"two") && texts.contains(&"task"), "{texts:?}");
    let creates: Vec<String> = v.json(&["log", "--limit", "100"])["events"].as_array().unwrap().iter().filter(|e| e["op"] == "node.create").filter_map(|e| e["tx"].as_str().map(str::to_string)).collect();
    let mut txs = creates.clone();
    txs.sort();
    txs.dedup();
    assert!(txs.len() <= 3, "the paste is one transaction (plus the day and the typed line): {txs:?}");
    let _ = before;
}

#[allow(dead_code)]
fn unused(_: &Path) {}

#[test]
fn the_near_miss_shows_as_the_link_closes_and_text_has_no_done_time() {
    // Review: the chip shows once `]]` is typed (before any save), ⌃O takes the page with no stub.
    let v = V::new("chip");
    v.cli(&["page", "new", "Health"]);
    let f = v.type_in("see [[Helth]] now");
    assert!(f.contains("new page \"Helth\" · ⌃O Health?"), "{f}");
    v.type_in("see [[Helth]]<c-o> now<esc>");
    let titles: Vec<String> = v.json(&["q", "is:page"])["items"].as_array().unwrap().iter().filter_map(|p| p["title"].as_str().map(str::to_string)).collect();
    assert_eq!(titles, ["Health"]);
    // ⌃T back to text: no status, no done time, in the store and on screen.
    let v = V::new("doneat");
    let id = v.json(&["todo", "call"])["nodes"][0]["id"].as_str().unwrap().to_string();
    v.cli(&["done", &id]);
    let f = v.type_in("<up><c-t><down>");
    let row = f.lines().find(|l| l.contains("call")).unwrap_or_default().to_string();
    assert!(!row.contains("done"), "{row}");
    let n = status_of(&v, &id);
    assert!(n["status"].is_null() && n["done_at"].is_null(), "{n}");
}

#[test]
fn tokens_fold_when_the_caret_leaves_the_line() {
    // writing.md §1 / tui-editor §5: leaving a note (Enter to a new item, an arrow) runs the
    // parsing save for it: the tokens become fields and leave the text; no Esc needed.
    let v = V::new("fold");
    let f = v.type_in("[ ] Call the landlord due:tue !high #lisbon<cr>");
    let row = f.lines().find(|l| l.contains("Call the landlord")).unwrap_or_default().to_string();
    assert!(!row.contains("due:tue") && row.contains("due tue · !high"), "folded on screen: {row}");
    let n = &v.json(&["q", "text:landlord"])["items"][0];
    assert_eq!((n["text"].as_str(), n["due"].as_str(), n["priority"].as_str()), (Some("Call the landlord #lisbon"), Some("2026-10-06"), Some("high")), "{n}");
}

#[test]
fn ctrl_o_finds_pages_and_days_and_creates_a_page() {
    let v = V::new("finder");
    v.cli(&["page", "new", "Lisbon"]);
    v.cli(&["page", "new", "Lighthouse notes"]);
    // Nothing typed: the pages, most recent first.
    let f = v.type_in("<c-o>");
    let (a, b) = (f.find("¶ Lighthouse notes").unwrap(), f.find("¶ Lisbon").unwrap());
    assert!(a < b && f.contains("↑↓ choose  Enter go  Esc close"), "{f}");
    // Fuzzy: `lsb` is Lisbon.
    let f = v.type_in("<c-o>lsb<cr>");
    assert!(f.lines().any(|l| { let l = l.trim(); l == "Lisbon" || l.ends_with(" › Lisbon") }), "{f}");
    // Days, the nearest: `oct 2` and `yesterday` are Fri 02 (THC_NOW is Sat Oct 3); `fri` offers
    // the coming one first and the last one too.
    for q in ["oct 2", "yesterday", "yest"] {
        let f = v.type_in(&format!("<c-o>{q}<cr>"));
        assert!(f.contains("FRI 02 OCT 2026"), "{q}: {f}");
    }
    let f = v.type_in("<c-o>fri");
    assert!(f.contains("§ Fri Oct 9 · in 6 days") && f.contains("§ Fri Oct 2 · yesterday"), "{f}");
    let f = v.type_in("<c-o>fri<down><cr>");
    assert!(f.contains("FRI 02 OCT 2026"), "{f}");
    // A new name: `+ new page`, and Enter makes it (no stray page for one letter).
    let f = v.type_in("<c-o>P");
    assert!(!f.contains("+ new page"), "{f}");
    let f = v.type_in("<c-o>Porto<cr>");
    assert!(f.lines().any(|l| { let l = l.trim(); l == "Porto" || l.ends_with(" › Porto") }), "{f}");
    let titles = v.json(&["q", "is:page"]);
    assert_eq!(titles["count"], 3, "{titles}");
    // A click on a row goes there.
    let f = v.type_in("<c-o>");
    let y = f.lines().position(|l| l.contains("¶ Lisbon")).unwrap();
    let x = f.lines().nth(y).unwrap().chars().position(|c| c == '¶').unwrap();
    let f = v.type_in(&format!("<c-o><click:{x},{y}>"));
    assert!(f.lines().any(|l| { let l = l.trim(); l == "Lisbon" || l.ends_with(" › Lisbon") }), "{f}");
    // From a list too, and Esc closes without going anywhere.
    let f = v.type_args("3<c-o>lis<esc>", &["tui"]);
    assert!(f.contains("status:open") && !f.contains("╭─ open"), "{f}");
}

#[test]
fn the_empty_finder_goes_back_where_you_were() {
    let v = V::new("finderback");
    v.cli(&["page", "new", "Oslo"]);
    // Follow a link out of the day; the empty ⌃O leads with the day (preselected), the page you're
    // on isn't offered, and Enter goes back.
    let f = v.type_in("see [[Lisbon flat]]<left><left><c-o><c-o>");
    let rows: Vec<&str> = f.lines().filter(|l| l.contains("│ ←") || l.contains("│ ¶") || l.contains("│ §")).collect();
    assert!(rows.first().is_some_and(|r| r.contains("← § sat 03 oct")), "{f}");
    assert!(!rows.iter().any(|r| r.contains("¶ Lisbon flat")), "not the page you're on: {f}");
    let f = v.type_in("see [[Lisbon flat]]<left><left><c-o><c-o><cr>");
    assert!(f.contains("SAT 03 OCT 2026"), "back on the day: {f}");
    // From a list with nothing opened yet: § today first.
    let f = v.type_args("1<c-o>", &["tui"]);
    let first = f.lines().find(|l| l.contains("│ §") || l.contains("│ ¶")).unwrap_or_default();
    assert!(first.contains("§ today"), "{f}");
}
