//! thc's views on fictty, against a seeded scratch vault at a pinned time: the feeds' records,
//! and screens driven headlessly (keys in, frames and vault writes out).

use chrono::{NaiveDate, NaiveDateTime};
use serde_json::{Value, json};
use std::path::Path;
use thc_core::builder::TxBuilder;
use thc_core::event::Actor;
use thc_core::glyphs::UNICODE;
use thc_core::vault::{self, Paths, Vault};
use thc_screen::feeds::{self, Cx};
use thc_screen::{Route, ThcHost, View};

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 11).unwrap()
}

fn now() -> NaiveDateTime {
    today().and_hms_opt(9, 0, 0).unwrap()
}

fn open(root: &Path, actor: Actor) -> Vault {
    Vault::open(Paths { vault: root.join("v"), cache: root.join("c") }, actor, "test").unwrap()
}

fn human() -> Actor {
    Actor { kind: "human".into(), name: None }
}

/// A small vault: a page with a task and a subtask, today's journal with a task due today and
/// a note, one overdue task, an inbox note, an agent's task, and a task done today.
fn seeded(name: &str) -> (std::path::PathBuf, Vault) {
    let root = thc_core::scratch::dir(&format!("thc-screen-{name}-{}", thc_core::id::new_id()));
    vault::init(&root.join("v"), None, None).unwrap();
    let mut v = open(&root, human());
    let d = today();
    v.transact(|s| {
        let mut b = TxBuilder::new(s, d);
        let cap = |t: &str| thc_core::capture::parse(t, d).unwrap();
        // The sample vault's pages, in its order: Atomic Habits, Health, Home, Q4 Planning.
        b.create_page("Atomic Habits", &[])?;
        b.create_page("Health", &[])?;
        let q4 = b.create_page("Q4 Planning", &[])?;
        b.create_from_capture(Some(q4), &cap("Theme for the quarter: fewer, deeper bets"), None)?;
        let home = b.create_page("Home", &[])?;
        let water = b.create_from_capture(Some(home.clone()), &cap("[ ] Water plants due:2026-10-11 #garden"), None)?;
        b.create_from_capture(Some(water), &cap("[ ] Buy a watering can"), None)?;
        b.create_from_capture(Some(home), &cap("Wifi password is on the fridge"), None)?;
        let j = b.journal(d)?;
        b.create_from_capture(Some(j.clone()), &cap("[ ] Call the bank due:2026-10-08 !high"), None)?;
        b.create_from_capture(Some(j.clone()), &cap("Morning run, see [[Home]]"), None)?;
        let done = b.create_from_capture(Some(j), &cap("[ ] Renew passport"), None)?;
        b.create_from_capture(None, &cap("Look into a standing desk"), None)?;
        Ok((b.finish(), done))
    })
    .map(|(_, done)| done)
    .and_then(|done| {
        v.transact(|s| {
            let mut b = TxBuilder::new(s, d);
            b.complete(&done)?;
            Ok((b.finish(), ()))
        })
    })
    .unwrap();
    let mut agent = open(&root, Actor { kind: "agent".into(), name: Some("claude".into()) });
    agent
        .transact(|s| {
            let mut b = TxBuilder::new(s, d);
            let j = b.journal(d)?;
            b.create_from_capture(Some(j), &thc_core::capture::parse("[ ] Summarize newsletters due:2026-10-12", d).unwrap(), None)?;
            Ok((b.finish(), ()))
        })
        .unwrap();
    v.catch_up().unwrap();
    (root, v)
}

fn texts(rows: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for r in rows.as_array().unwrap() {
        out.push(r["text"].as_str().or(r["title"].as_str()).unwrap_or("").to_string());
        if let Some(c) = r.get("children") {
            out.extend(texts(c).into_iter().map(|t| format!("  {t}")));
        }
    }
    out
}

#[test]
fn today_has_its_sections_rows_and_meta() {
    let (root, v) = seeded("today");
    let cx = Cx { store: &v.store, g: &UNICODE, now: now() };
    let t = feeds::today(&cx).unwrap();
    assert_eq!(
        texts(&t["rows"]),
        vec!["Overdue", "  Call the bank", "Today", "  Water plants #garden", "Next 7 days", "  Summarize newsletters", "Done today", "  Renew passport"]
    );
    let row = |sec: usize, i: usize| t["rows"][sec]["children"][i].clone();
    // Overdue: the date in `overdue`, priority bold; today's journal isn't named.
    assert_eq!(row(0, 0)["meta_m"], "<overdue>due Oct 8 (3d ago)</><muted> · </><text+b>!high</>");
    assert_eq!(row(0, 0)["status"], "todo");
    // A tag in its own colour; the page it lives on as context; a variant without it (§4.2).
    assert_eq!(row(1, 0)["text_m"], "Water plants <tag>#garden</>");
    assert_eq!(row(1, 0)["metas"][0], "<today>due today</><muted> · </><muted>¶ Home</>");
    assert_eq!(row(1, 0)["metas"][1], "<today>due today</>");
    // An agent's task says whose it is.
    assert!(row(2, 0)["meta_m"].as_str().unwrap().contains("<agent>◆ claude</>"), "{}", row(2, 0)["meta_m"]);
    // Done rows: muted text and the time they were done.
    assert_eq!(row(3, 0)["kind"], "done");
    assert_eq!(row(3, 0)["text_m"], "<muted>Renew passport</>");
    assert_eq!(t["bar_m"], "<text>today</><muted> · </><muted>3 open</><muted> · </><overdue>1 overdue</><muted> · </><muted>1 inbox</>");
    // The detail pane's lines (the `detail` feed, for the selected row's id): title, where,
    // fields, subtasks.
    let lines = feeds::detail(&cx, row(1, 0)["id"].as_str().unwrap()).unwrap()["lines"].clone();
    assert_eq!(lines[0]["k"], "title");
    assert!(lines[1]["m"].as_str().unwrap().contains("¶ Home ›"), "{}", lines[1]);
    assert!(lines.as_array().unwrap().iter().any(|l| l["k"] == "sub" && l["text_m"] == "Buy a watering can"), "{lines:#}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn every_feed_answers_for_the_seeded_vault() {
    let (root, v) = seeded("feeds");
    let host = ThcHost::new(v).at(now());
    let inbox = host.feed("inbox", &json!({})).unwrap();
    assert_eq!(inbox["rows"][1]["text"], "Look into a standing desk");
    let tasks = host.feed("tasks", &json!({"query": "status:open"})).unwrap();
    assert_eq!(tasks["rows"].as_array().unwrap().len(), 4, "{tasks:#}");
    assert!(tasks["filter_m"].as_str().unwrap().starts_with("<muted>status:</><text>open</>"));
    let pages = host.feed("pages", &json!({})).unwrap();
    let home = pages["rows"].as_array().unwrap().iter().find(|r| r["text"] == "Home").unwrap().clone();
    assert_eq!(home["doc"], format!("page/{}", home["id"].as_str().unwrap()), "the row names its document");
    assert_eq!(home["meta_m"], "<muted>2 children · 2 open · ← 1</>");
    let outline = host.feed("outline", &json!({"id": home["id"]})).unwrap();
    assert_eq!(texts(&outline["rows"]), vec!["Water plants #garden", "  Buy a watering can", "Wifi password is on the fridge", "← linked from", "  Morning run, see [[Home]]"]);
    let journal = host.feed("journal", &json!({})).unwrap();
    assert!(journal["title_m"].as_str().unwrap().starts_with("<text+b>§ Sunday, October 11 2026</>"));
    assert_eq!(journal["calendar"][0]["m"], "<text+b>October 2026</>");
    let search = host.feed("search", &json!({"query": "bank"})).unwrap();
    assert_eq!(texts(&search["rows"]), vec!["Nodes", "  Call the bank"]);
    assert!(search["rows"][0]["children"][0]["text_m"].as_str().unwrap().contains("<b+on-accent-tint>bank</>"));
    let log = host.feed("log", &json!({})).unwrap();
    let agent = log["rows"].as_array().unwrap().iter().find(|r| r["kind"] == "agent").expect("the agent's transaction").clone();
    assert_eq!(agent["who"], "◆ claude");
    let tx = host.feed("tx", &json!({"id": agent["id"]})).unwrap();
    assert!(tx["lines"][0]["m"].as_str().unwrap().contains(agent["tx"].as_str().unwrap()), "{tx:#}");
    // The frame: what the tab titles and the date read.
    assert_eq!(host.feed("frame", &json!({})).unwrap(), json!({"inbox": "1", "date": "Sun Oct 11", "live": false}));
    assert!(host.feed("nope", &json!({})).is_err());
    let _ = std::fs::remove_dir_all(root);
}

fn screen(host: &ThcHost, view: View, w: u16, h: u16) -> fictty::Headless {
    host.app(&Route::to(view), "ember-dark").headless(w, h).unwrap()
}

#[test]
fn x_on_a_today_row_marks_it_done_in_the_vault() {
    let (root, v) = seeded("done");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 120, 32);
    let frame = s.screen();
    assert!(frame.contains("Today   Inbox 1   Tasks"), "{frame}");
    assert!(frame.contains("[ ] Call the bank"), "{frame}");
    // The cursor starts on the first row (sections take none), and the pane shows it.
    assert!(frame.contains("│ [ ] Call the bank"), "{frame}");
    s.keys(&["x"]);
    let frame = s.screen();
    assert!(frame.contains("Done today  2"), "the row moved to Done today: {frame}");
    assert!(!frame.contains("Overdue"), "{frame}");
    // In the vault, as the person.
    let check = open(&root, human());
    let done = check.store.nodes_where("n.text = 'Call the bank'", &[]).unwrap();
    assert_eq!(done[0].status.as_deref(), Some("done"));
    let h = check.store.history(&done[0].id, 10).unwrap();
    assert_eq!(h[0].op, "node.complete");
    assert_eq!(h[0].actor, "human");
    assert_eq!(h[0].via, "test");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn keys_switch_views_and_the_split_follows_the_width() {
    let (root, v) = seeded("views");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 80, 24);
    let narrow = s.screen();
    assert!(!narrow.contains('│'), "no detail pane below 120: {narrow}");
    s.keys(&["2"]);
    let inbox = s.screen();
    assert!(inbox.contains("Inbox  1") && inbox.contains("Look into a standing desk"), "{inbox}");
    // The rule lights the new tab: one cell either side of `Inbox 1` (after ` [•] thc ` and Today).
    assert!(inbox.lines().nth(1).unwrap().starts_with(&format!("{}{}─", "─".repeat(18), "━".repeat(9))), "{inbox}");
    s.keys(&["4"]);
    assert!(s.screen().contains("¶ Home"), "pages");
    // A click on a tab title (what the runtime makes of the mouse there) switches too.
    s.msg(fictty::state::Msg::Click { id: "views".into(), row: Some(1002), tab: Some(2) });
    assert!(s.screen().contains("q› "), "tasks by click: {}", s.screen());
    // In Tasks the rows have the focus, so 1 goes back to Today (it isn't typed in the filter).
    s.keys(&["1"]);
    assert!(s.screen().lines().nth(1).unwrap().starts_with(&format!("{}{}─", "─".repeat(10), "━".repeat(7))), "{}", s.screen());
    // `f` edits the filter; the rows follow what's typed; Enter goes back to them.
    s.keys(&["3", "f", "d-right"]);
    type_in(&mut s, " #garden");
    s.keys(&["enter"]);
    let tasks = s.screen();
    assert!(tasks.contains("Water plants") && !tasks.contains("Call the bank") && tasks.contains("1 task"), "{tasks}");
    assert_eq!(s.state().focus.as_deref(), Some("tasks-list"), "back on the rows");
    let mut wide = screen(&host, View::Inbox, 120, 32);
    let frame = wide.screen();
    assert!(frame.contains("│  ·  Look into a standing desk"), "the detail pane: {frame}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn actions_write_as_the_person_and_say_why_when_they_cant() {
    let (root, v) = seeded("acts");
    let host = ThcHost::new(v).at(now());
    let id = |text: &str| {
        let v = open(&root, human());
        v.store.nodes_where("(n.text = ?1 OR n.title = ?1)", &[&text]).unwrap()[0].id.clone()
    };
    let call = id("Call the bank");
    host.perform("set", &json!({"id": call, "props": {"priority": "low", "due": "2026-10-20"}})).unwrap();
    host.perform("add", &json!({"text": "Book flu shot due:2026-10-13 #health"})).unwrap();
    host.perform("mv", &json!({"id": id("Look into a standing desk"), "under": id("Home")})).unwrap();
    let check = open(&root, human());
    let n = check.store.node(&call).unwrap().unwrap();
    assert_eq!((n.priority.as_deref(), n.due.as_deref()), (Some("low"), Some("2026-10-20")));
    let flu = &check.store.nodes_where("n.text LIKE 'Book flu shot%'", &[]).unwrap()[0];
    assert_eq!(flu.due.as_deref(), Some("2026-10-13"));
    assert_eq!(check.store.node(flu.parent.as_deref().unwrap()).unwrap().unwrap().journal.as_deref(), Some("2026-10-11"));
    assert!(host.perform("done", &json!({})).unwrap_err().to_string().contains("done needs an id"));
    // On screen: `d` asks for a date in the bar; one thc can't read says why, in a toast.
    let mut s = screen(&host, View::Today, 80, 24);
    s.keys(&["d"]);
    assert!(s.screen().contains("due ›"), "the prompt: {}", s.screen());
    type_in(&mut s, "fryday");
    s.keys(&["enter"]);
    let said: Vec<String> = s.state().toasts.iter().map(|t| t.text.clone()).collect();
    assert!(said.iter().any(|t| t.contains("fryday")), "{said:?}");
    assert!(!s.screen().contains("due ›"), "the prompt closed");
    // And one it can read sets the date on the row the prompt was asked about (the first row:
    // Call the bank moved out of Today above).
    let first = host.feed("today", &json!({})).unwrap()["rows"][0]["children"][0]["id"].as_str().unwrap().to_string();
    s.keys(&["d"]);
    type_in(&mut s, "2026-10-30");
    s.keys(&["enter"]);
    assert_eq!(open(&root, human()).store.node(&first).unwrap().unwrap().due.as_deref(), Some("2026-10-30"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_change_behind_the_screen_shows_when_the_host_says_so() {
    let (root, v) = seeded("changed");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Inbox, 80, 24);
    assert!(!s.screen().contains("Gift idea"));
    host.perform("add", &json!({"text": "Gift idea: a chef's knife", "inbox": true})).unwrap();
    assert!(!s.screen().contains("Gift idea"), "nothing refetches by itself");
    s.msg(fictty::state::Msg::Changed { host: "inbox".into() });
    assert!(s.screen().contains("Gift idea"));
    let _ = std::fs::remove_dir_all(root);
}

// ---- the vault changing outside the screen --------------------------------------------------------

/// Another writer on the same vault, as another process would be (its own connection to the
/// store, its own log appends): `thc todo` from an agent beside the screen.
fn outside_add(root: &Path, parent: Option<String>, text: &str) {
    let mut other = open(root, Actor { kind: "agent".into(), name: Some("claude".into()) });
    let d = today();
    other
        .transact(|s| {
            let mut b = TxBuilder::new(s, d);
            let parent = match parent {
                Some(p) => Some(p),
                None => Some(b.journal(d)?),
            };
            b.create_from_capture(parent, &thc_core::capture::parse(text, d).unwrap(), None)?;
            Ok((b.finish(), ()))
        })
        .unwrap();
}

/// Wait (up to 5 s) for the screen to show `text`.
fn shows(s: &mut fictty::Headless, text: &str) -> bool {
    for _ in 0..50 {
        if s.screen().contains(text) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    false
}

#[test]
fn the_watcher_shows_an_outside_write_by_itself() {
    let (root, v) = seeded("watch");
    // The watcher runs on its own thread, as `thc screen` runs it: no daemon here, so it looks
    // at the vault every second.
    let host = ThcHost::new(v).at(now()).watching();
    let mut s = screen(&host, View::Today, 120, 32);
    assert!(s.screen().contains("Call the bank"));
    outside_add(&root, None, "[ ] Book the furnace service visit due:2026-10-12");
    assert!(shows(&mut s, "Book the furnace service visit"), "{}", s.screen());
    let _ = std::fs::remove_dir_all(root);
}

// ---- pages and days as text --------------------------------------------------------------------

fn node_id(root: &Path, text: &str) -> String {
    let v = open(root, human());
    v.store.nodes_where("(n.text = ?1 OR n.title = ?1)", &[&text]).unwrap()[0].id.clone()
}

fn page(host: &ThcHost, id: &str) -> fictty::Headless {
    let route = Route { view: View::Page, id: Some(id.into()), ..Route::default() };
    host.app(&route, "ember-dark").headless(120, 32).unwrap()
}

/// Typed text as key names.
fn typed(s: &str) -> Vec<String> {
    s.chars()
        .map(|c| match c {
            ' ' => "space".to_string(),
            '\n' => "enter".to_string(),
            c => c.to_string(),
        })
        .collect()
}

fn type_in(s: &mut fictty::Headless, text: &str) {
    let ks = typed(text);
    let refs: Vec<&str> = ks.iter().map(String::as_str).collect();
    s.keys(&refs);
}

/// The caret (the page editor's selection), as JSON.
fn caret(s: &mut fictty::Headless) -> Value {
    let st = s.state();
    serde_json::to_value(&fictty::state::field_view(st, "doc").expect("the page editor").1.selection).unwrap()
}

#[test]
fn typing_into_a_page_writes_the_vault_and_keeps_ids() {
    let (root, v) = seeded("type");
    let (home, water, can, wifi) = (node_id(&root, "Home"), node_id(&root, "Water plants #garden"), node_id(&root, "Buy a watering can"), node_id(&root, "Wifi password is on the fridge"));
    let host = ThcHost::new(v).at(now());
    let mut s = page(&host, &home);
    let text = s.screen();
    assert!(text.contains("# Home") && text.contains("- [ ] Water plants #garden due:2026-10-11") && text.contains("  - [ ] Buy a watering can"), "{text}");
    assert!(!text.contains('^'), "no ids on show: {text}");
    // To the end of the last line, and type: an edit to that line, then a new one.
    s.keys(&["d-down", "bs"]);
    type_in(&mut s, " now\n");
    type_in(&mut s, "Call the plumber");
    let check = open(&root, human());
    let kids: Vec<(String, String)> = check.store.children(&home).unwrap().into_iter().map(|n| (n.id, n.text)).collect();
    assert_eq!(kids[0], (water.clone(), "Water plants #garden".into()), "{kids:?}");
    assert_eq!(kids[1], (wifi, "Wifi password is on the fridge now".into()), "the edited line keeps its node: {kids:?}");
    assert_eq!(kids[2].1, "Call the plumber", "{kids:?}");
    assert_eq!(check.store.children(&water).unwrap()[0].id, can, "the nested line keeps its node");
    let h = check.store.history(&kids[2].0, 5).unwrap();
    assert_eq!((h[0].actor.as_str(), h[0].via.as_str()), ("human", "test"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn an_outside_add_shows_in_the_page_without_moving_the_caret() {
    let (root, v) = seeded("reload");
    let home = node_id(&root, "Home");
    let host = ThcHost::new(v).at(now());
    let mut s = page(&host, &home);
    // The caret at the end of the Water plants line.
    s.keys(&["d-up", "down", "down", "d-right"]);
    let before = caret(&mut s);
    outside_add(&root, Some(home.clone()), "Fix the garden gate");
    assert!(!s.screen().contains("Fix the garden gate"));
    assert!(host.poll(false), "the screen is there");
    let text = s.screen();
    assert!(text.contains("- Fix the garden gate"), "{text}");
    assert_eq!(caret(&mut s), before, "the caret stayed where the person left it");
    assert_eq!(s.state().store[&format!("page/{home}")].by, "host", "it came in as the host's edit");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ticking_a_task_box_marks_the_node_done() {
    let (root, v) = seeded("tick");
    let (home, water) = (node_id(&root, "Home"), node_id(&root, "Water plants #garden"));
    let host = ThcHost::new(v).at(now());
    let mut s = page(&host, &home);
    s.keys(&["d-up", "down", "down", "c-t"]);
    assert!(s.screen().contains("- [x] Water plants"), "{}", s.screen());
    let check = open(&root, human());
    assert_eq!(check.store.node(&water).unwrap().unwrap().status.as_deref(), Some("done"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_journal_day_opens_as_text_and_a_new_line_lands_in_the_day() {
    let (root, v) = seeded("day");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Journal, 120, 32);
    let focus = s.state().focus.clone();
    s.keys(&["enter"]);
    let text = s.screen();
    let st = s.state();
    assert!(text.contains("# 2026-10-11 (journal)") && text.contains("- [ ] Call the bank due:2026-10-08 !high"), "focus {focus:?} → {:?}, status {:?}\n{text}", st.focus, st.status);
    s.keys(&["d-down", "bs"]);
    type_in(&mut s, "\n- Lunch with Sam");
    let check = open(&root, human());
    let day = check.store.journal_node("2026-10-11").unwrap().unwrap();
    assert!(check.store.children(&day).unwrap().iter().any(|n| n.text == "Lunch with Sam"));
    // Esc goes back to the Journal on that day.
    s.keys(&["esc"]);
    assert!(s.screen().contains("§ Sunday, October 11 2026"), "{}", s.screen());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn the_mac_editing_keys_work_in_a_page() {
    let (root, v) = seeded("mac");
    let home = node_id(&root, "Home");
    let host = ThcHost::new(v).at(now());
    let ids_before = vec![node_id(&root, "Water plants #garden"), node_id(&root, "Wifi password is on the fridge")];
    let mut s = page(&host, &home);
    let whole = s.screen();
    // ⌘A ⌫ empties the page; ⌘Z brings it all back.
    s.keys(&["d-a", "bs"]);
    assert!(!s.screen().contains("Water plants"));
    s.keys(&["d-z"]);
    assert_eq!(s.screen(), whole, "undo restores the page");
    let check = open(&root, human());
    let texts: Vec<String> = check.store.children(&home).unwrap().into_iter().map(|n| n.text).collect();
    assert_eq!(texts, vec!["Water plants #garden", "Wifi password is on the fridge"], "and the vault");
    let ids: Vec<String> = check.store.children(&home).unwrap().into_iter().map(|n| n.id).collect();
    assert_eq!(ids, ids_before, "the same nodes, not new ones");
    // ⌥⌫ deletes the word before the caret.
    s.keys(&["d-down", "bs", "a-bs"]);
    let text = s.screen();
    assert!(text.contains("- Wifi password is on the") && !text.contains("fridge"), "{text}");
    // ⇧⌘← selects to the start of the row; ⌘C copies it.
    s.keys(&["d-s-left", "d-c"]);
    assert_eq!(s.copied().last().map(String::as_str), Some("- Wifi password is on the "));
    let _ = std::fs::remove_dir_all(root);
}

/// The person's path, as driven live: a click on the Pages tab, ↓ ↓ ↓ to Q4 Planning, Enter, ⌘↓
/// to the end of the page, a new task typed: it's in the vault under the page.
#[test]
fn a_click_on_pages_then_keys_write_a_task_into_a_page() {
    let (root, v) = seeded("click-write");
    let q4 = node_id(&root, "Q4 Planning");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 120, 32);
    // The click on the tab title, as the runtime reports it; the focus goes into the view.
    s.msg(fictty::state::Msg::Click { id: "views".into(), row: Some(1003), tab: Some(3) });
    assert_eq!(s.state().focus.as_deref(), Some("pages"), "the list: arriving never takes the caret (N14)");
    s.keys(&["down", "down", "down"]);
    assert!(s.screen().contains("▌ "), "{}", s.screen());
    s.keys(&["enter"]);
    let text = s.screen();
    assert!(text.contains("# Q4 Planning") && text.contains("- Theme for the quarter"), "Q4 Planning to write in: {text}");
    s.keys(&["d-down"]);
    type_in(&mut s, "- [ ] X");
    let check = open(&root, human());
    let kids = check.store.children(&q4).unwrap();
    let x = kids.iter().find(|n| n.text == "X").unwrap_or_else(|| panic!("no X under Q4 Planning: {kids:?}"));
    assert_eq!(x.status.as_deref(), Some("todo"));
    assert_eq!(check.store.history(&x.id, 5).unwrap()[0].actor, "human");
    let _ = std::fs::remove_dir_all(root);
}

// ---- the e2e harness's scenarios (scripts/e2e on e2e/keys-mouse-harness), headless -------------

/// Whether the accent rule (row 2) is under `label` in the header (row 1), as the harness checks.
fn rule_under(screen: &str, label: &str) -> bool {
    let rows: Vec<Vec<char>> = screen.lines().take(2).map(|l| l.chars().collect()).collect();
    let head: String = rows[0].iter().collect();
    let Some(at) = head.find(label) else { return false };
    let col = head[..at].chars().count();
    (col..col + label.chars().count()).all(|c| rows[1].get(c) == Some(&'━'))
}

/// N01, N14: digits switch views from every view, Pages included (arriving never takes the
/// caret); with a query typed on Pages, a digit types.
#[test]
fn n01_n14_digits_switch_views_and_type_in_a_query() {
    let (root, v) = seeded("n01");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 120, 32);
    for (key, label) in [("3", "Tasks"), ("2", "Inbox"), ("4", "Pages"), ("7", "Log"), ("4", "Pages"), ("1", "Today")] {
        s.keys(&[key]);
        assert!(rule_under(&s.screen(), label), "{key} → {label}: {}", s.screen());
    }
    // N14: a letter on Pages starts the find; then a digit types into it.
    s.keys(&["4", "q", "3"]);
    assert_eq!(s.state().focus.as_deref(), Some("pages_q"));
    assert!(s.screen().contains("› q3"), "{}", s.screen());
    assert!(rule_under(&s.screen(), "Pages"));
    // Esc clears the query and gives the keys back to the list: 3 switches again.
    s.keys(&["esc", "3"]);
    assert!(rule_under(&s.screen(), "Tasks"), "{}", s.screen());
    let _ = std::fs::remove_dir_all(root);
}

/// N01 on a vault with no pages (the harness's): Pages has no row to select, and its digits still
/// switch views; the preview never takes the keys.
#[test]
fn n01_digits_switch_views_from_an_empty_pages() {
    let root = thc_core::scratch::dir(&format!("thc-screen-n01e-{}", thc_core::id::new_id()));
    vault::init(&root.join("v"), None, None).unwrap();
    let mut v = open(&root, human());
    v.transact(|s| {
        let mut b = TxBuilder::new(s, today());
        b.create_from_capture(None, &thc_core::capture::parse("[ ] Alpha task due:2026-10-11", today()).unwrap(), None)?;
        Ok((b.finish(), ()))
    })
    .unwrap();
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 120, 32);
    s.keys(&["4"]);
    let focus = s.state().focus.clone();
    s.keys(&["7", "1"]);
    assert!(rule_under(&s.screen(), "Today"), "focus on Pages was {focus:?}: {}", s.screen());
    let _ = std::fs::remove_dir_all(root);
}

/// N02, N16: Tab and ⇧Tab cycle the views from anywhere, Pages and Search included.
#[test]
fn n02_n16_tab_and_shift_tab_cycle_views() {
    let (root, v) = seeded("n02");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 120, 32);
    for label in ["Inbox", "Tasks", "Pages", "Journal", "Search", "Log", "Today"] {
        s.keys(&["tab"]);
        assert!(rule_under(&s.screen(), label), "tab → {label}: {}", s.screen());
    }
    for label in ["Log", "Search", "Journal"] {
        s.keys(&["s-tab"]);
        assert!(rule_under(&s.screen(), label), "s-tab → {label}: {}", s.screen());
    }
    let _ = std::fs::remove_dir_all(root);
}

/// N04, N11, B02: on Pages type a name (the find), Enter opens the top match; Esc comes back to
/// the Pages list with that page selected, the query as it was.
#[test]
fn n04_b02_enter_opens_a_page_and_esc_comes_back_to_it() {
    let (root, v) = seeded("n04");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 80, 24);
    s.keys(&["4"]);
    type_in(&mut s, "Health");
    s.keys(&["enter"]);
    assert!(s.screen().contains("# Health"), "Health to write in: {}", s.screen());
    s.keys(&["esc"]);
    let back = s.screen();
    assert!(!back.contains("# Health") && back.contains("¶ Health"), "the Pages list again: {back}");
    // The page you came from is selected (its row has the ▌).
    assert!(back.lines().any(|l| l.starts_with('▌') && l.contains("¶ Health")), "{back}");
    // Esc on the list clears the query; every page shows again.
    s.keys(&["esc"]);
    assert!(s.screen().contains("¶ Q4 Planning"), "{}", s.screen());
    // B02, fresh: Pages, down to Q4 Planning, open, write at the end, Esc: Q4 Planning is selected.
    let mut s = screen(&host, View::Today, 80, 24);
    s.keys(&["4", "down", "down", "down"]);
    s.keys(&["enter", "d-down", "enter"]);
    type_in(&mut s, "[ ] shipped");
    s.keys(&["esc"]);
    let back = s.screen();
    assert!(back.lines().any(|l| l.starts_with('▌') && l.contains("¶ Q4 Planning")), "{back}");
    let check = open(&root, human());
    let shipped = check.store.nodes_where("n.text = 'shipped'", &[]).unwrap();
    assert_eq!(shipped.len(), 1);
    assert_eq!(shipped[0].status.as_deref(), Some("todo"));
    let _ = std::fs::remove_dir_all(root);
}

/// W01: `a` captures, in capture syntax, into today's journal.
#[test]
fn w01_a_captures_a_task() {
    let (root, v) = seeded("w01");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 120, 32);
    s.keys(&["a"]);
    assert!(s.screen().contains("capture ›"), "{}", s.screen());
    type_in(&mut s, "Buy oat milk due:fri");
    s.keys(&["enter"]);
    let check = open(&root, human());
    let n = check.store.nodes_where("n.text = 'Buy oat milk'", &[]).unwrap();
    assert_eq!(n.len(), 1, "captured once");
    assert_eq!(n[0].due.as_deref(), Some("2026-10-16"), "capture syntax read");
    assert_eq!(check.store.node(n[0].parent.as_deref().unwrap()).unwrap().unwrap().journal.as_deref(), Some("2026-10-11"));
    assert!(s.screen().contains("Buy oat milk"), "it shows: {}", s.screen());
    let _ = std::fs::remove_dir_all(root);
}

/// W02, N24: on Tasks, x marks the row done and it stays where it was, so X reopens it; leaving
/// the view lets it go.
#[test]
fn w02_done_stays_until_you_leave_so_it_can_be_reopened() {
    let (root, v) = seeded("w02");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Tasks, 120, 32);
    let first = s.screen().lines().find(|l| l.starts_with('▸') || l.starts_with('▌')).unwrap_or_default().to_string();
    s.keys(&["x"]);
    let after = s.screen();
    assert!(after.contains("[x]"), "the row stays, done: {after}");
    let id = |text: &str| open(&root, human()).store.nodes_where("n.text LIKE ?1", &[&format!("{text}%")]).unwrap()[0].clone();
    let done = after.lines().find(|l| l.contains("[x]")).unwrap().to_string();
    let name = done.split("[x]").nth(1).unwrap().trim().split("  ").next().unwrap().to_string();
    assert_eq!(id(&name).status.as_deref(), Some("done"), "{first} / {done}");
    s.keys(&["X"]);
    assert_eq!(id(&name).status.as_deref(), Some("todo"), "reopened");
    // Done again, then leave and come back: the done row has gone.
    s.keys(&["x", "1", "3"]);
    assert!(!s.screen().contains("[x]"), "{}", s.screen());
    let _ = std::fs::remove_dir_all(root);
}
