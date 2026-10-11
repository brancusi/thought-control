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
    assert_eq!(host.feed("frame", &json!({})).unwrap(), json!({"inbox": "1", "date": "Sun Oct 11", "live": false, "conflicts": 0}));
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
    serde_json::to_value(&fictty::state::field_view(st, "pages_doc").expect("the page editor").1.selection).unwrap()
}

#[test]
fn typing_into_a_page_writes_the_vault_and_keeps_ids() {
    let (root, v) = seeded("type");
    let (home, water, can, wifi) = (node_id(&root, "Home"), node_id(&root, "Water plants #garden"), node_id(&root, "Buy a watering can"), node_id(&root, "Wifi password is on the fridge"));
    let host = ThcHost::new(v).at(now());
    let mut s = page(&host, &home);
    let text = s.screen();
    assert!(text.contains("¶ Home") && text.contains("- [ ] Water plants #garden due:2026-10-11") && text.contains("  - [ ] Buy a watering can"), "{text}");
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
    s.keys(&["d-up", "d-right"]);
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
    s.keys(&["d-up", "c-t"]);
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
    assert!(text.contains("§ Sunday, October 11 2026") && text.contains("- [ ] Call the bank due:2026-10-08 !high"), "focus {focus:?} → {:?}, status {:?}\n{text}", st.focus, st.status);
    s.keys(&["d-down", "bs"]);
    type_in(&mut s, "\nLunch with Sam");
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
    assert!(text.contains("¶ Q4 Planning") && text.contains("- Theme for the quarter"), "Q4 Planning to write in: {text}");
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
    assert!(s.screen().contains("¶ Health") && s.screen().contains("write here"), "Health to write in: {}", s.screen());
    s.keys(&["esc"]);
    let back = s.screen();
    assert!(!back.contains("write here") && back.contains("¶ Health"), "the Pages list again: {back}");
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

// ---- outline keys in a page: Tab indents lines, ⌃T cycles a line's task state (fictty) ------

/// The texts and ids of a node's children, in order.
fn kids(root: &Path, parent: &str) -> Vec<(String, String)> {
    open(root, human()).store.children(parent).unwrap().into_iter().map(|n| (n.text, n.id)).collect()
}

fn kid_texts(root: &Path, parent: &str) -> Vec<String> {
    kids(root, parent).into_iter().map(|k| k.0).collect()
}

/// Q4 Planning with two more items typed at its end, the caret at the end of the last:
/// `- Theme…`, `- one`, `- two▮`.
fn q4_with_two(name: &str) -> (std::path::PathBuf, ThcHost, fictty::Headless, String) {
    let (root, v) = seeded(name);
    let q4 = node_id(&root, "Q4 Planning");
    let host = ThcHost::new(v).at(now());
    let mut s = page(&host, &q4);
    s.keys(&["d-down"]);
    type_in(&mut s, "- one\ntwo");
    (root, host, s, q4)
}

/// E22: a selection over two list items, `- o⟦ne ‖ - tw▮⟧o`, Tab: both nest under the item
/// before them, the selection stays, and the vault has them as children with their ids.
#[test]
fn e22_tab_nests_the_selected_lines_and_keeps_the_selection() {
    let (root, _host, mut s, q4) = q4_with_two("e22");
    let before = kids(&root, &q4);
    assert_eq!(kid_texts(&root, &q4), vec!["Theme for the quarter: fewer, deeper bets", "one", "two"], "{}", s.screen());
    let (one, two) = (before[1].1.clone(), before[2].1.clone());
    // The caret inside `two` (tw▮o), the anchor inside `one` (o⟦ne).
    s.keys(&["left", "s-up", "s-left"]);
    let sel = caret(&mut s);
    s.keys(&["tab"]);
    let text = s.screen();
    assert!(text.contains("  - one") && text.contains("  - two"), "both nested: {text}");
    assert_eq!(s.state().focus.as_deref(), Some("pages_doc"), "Tab stayed in the editor");
    let after = caret(&mut s);
    assert_ne!(after, sel, "the selection moved with the text");
    assert_eq!(after.to_string().replace(|c: char| c.is_ascii_digit(), ""), sel.to_string().replace(|c: char| c.is_ascii_digit(), ""), "still a selection: {sel} → {after}");
    let top = kids(&root, &q4);
    assert_eq!(top.len(), 1, "one top item left: {top:?}");
    assert_eq!(kids(&root, &top[0].1), vec![("one".to_string(), one), ("two".to_string(), two)], "nested, the same nodes");
    let _ = std::fs::remove_dir_all(root);
}

/// Writing a child: Enter for a new item, type it, Tab: it's a child of the item above.
#[test]
fn enter_type_tab_writes_a_nested_child() {
    let (root, _host, mut s, q4) = q4_with_two("child");
    type_in(&mut s, "\nthree");
    s.keys(&["tab"]);
    assert!(s.screen().contains("  - three"), "{}", s.screen());
    let top = kids(&root, &q4);
    assert_eq!(kid_texts(&root, &q4), vec!["Theme for the quarter: fewer, deeper bets", "one", "two"]);
    assert_eq!(kid_texts(&root, &top[2].1), vec!["three"], "a child of two");
    let _ = std::fs::remove_dir_all(root);
}

/// A paragraph written over two lines is one node (Markdown: prose lines join), its id kept
/// and out of its text. Tab on its later line makes that line a child paragraph (editing.md E22,
/// owner decision 2026-10-11: an indented line is a child, whatever the kind): the paragraph
/// keeps its id and its first line, the child is a new paragraph under it.
#[test]
fn a_two_line_paragraph_is_one_node_and_tab_makes_its_later_line_a_child() {
    let (root, v) = seeded("para");
    let q4 = node_id(&root, "Q4 Planning");
    let host = ThcHost::new(v).at(now());
    let mut s = page(&host, &q4);
    s.keys(&["d-down"]);
    type_in(&mut s, "A plain paragraph
its later line");
    let before = kids(&root, &q4);
    assert_eq!(before[1].0, "A plain paragraph its later line", "the id stays out of the text: {before:?}");
    s.keys(&["tab"]);
    assert!(s.screen().contains("   its later line"), "{}", s.screen());
    let after = kids(&root, &q4);
    assert_eq!(after[1], ("A plain paragraph".to_string(), before[1].1.clone()), "the paragraph keeps its id");
    let child = kids(&root, &after[1].1);
    assert_eq!(child.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), ["its later line"], "a child paragraph");
    let store = open(&root, human()).store;
    assert!(thc_core::edit::has_para_style(&store, &child[0].1));
    let _ = std::fs::remove_dir_all(root);
}

/// Open a page and type at once: the keys land in the page's text and the vault.
#[test]
fn typed_ahead_keys_land_in_the_page() {
    let (root, v) = seeded("ahead");
    let health = node_id(&root, "Health");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Pages, 120, 32);
    let mut keys = vec!["down".to_string(), "enter".to_string()];
    keys.extend(typed("- Stretch"));
    let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
    s.keys(&refs);
    assert!(s.screen().contains("- Stretch"), "{}", s.screen());
    assert_eq!(kid_texts(&root, &health), vec!["Stretch"]);
    let _ = std::fs::remove_dir_all(root);
}

/// T04: open a page that has text and type at once: the caret starts at the end (fictty's
/// `"caret": "end"`), so the words are appended, not put before the first line.
#[test]
fn t04_typing_right_after_opening_a_page_appends() {
    let (root, v) = seeded("t04");
    let home = node_id(&root, "Home");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Pages, 120, 32);
    let mut keys = vec!["down".to_string(), "down".to_string(), "enter".to_string()];
    keys.extend(typed("Quick words"));
    let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
    s.keys(&refs);
    let texts = kid_texts(&root, &home);
    assert_eq!(texts, vec!["Water plants #garden", "Wifi password is on the fridge", "Quick words"], "{}", s.screen());
    let _ = std::fs::remove_dir_all(root);
}

/// ⌃T cycles a line plain → task → done → plain; each time the vault's node changes kind (a
/// task is a node with a status) and keeps its id.
#[test]
fn ctrl_t_cycles_a_line_and_the_node_keeps_its_id() {
    let (root, v) = seeded("cycle");
    let (home, wifi) = (node_id(&root, "Home"), node_id(&root, "Wifi password is on the fridge"));
    let host = ThcHost::new(v).at(now());
    let mut s = page(&host, &home);
    s.keys(&["d-down", "up"]);
    // (fictty's cycle ends on a plain line without its list marker: `- [x] Wifi` → `Wifi`.)
    for (shown, status) in [("- [ ] Wifi", Some("todo")), ("- [x] Wifi", Some("done")), ("\n Wifi password", None)] {
        s.keys(&["c-t"]);
        assert!(s.screen().contains(shown), "{shown}: {}", s.screen());
        let n = open(&root, human()).store.node(&wifi).unwrap().expect("the same node");
        assert_eq!((n.text.as_str(), n.status.as_deref()), ("Wifi password is on the fridge", status), "{shown}");
    }
    assert_eq!(kids(&root, &home).len(), 2, "no new nodes");
    let _ = std::fs::remove_dir_all(root);
}

// ---- watching an agent work: the toast, live rows, review and undo -----------------------------

/// Whether a node is deleted (undone) in the vault.
fn deleted(root: &Path, id: &str) -> bool {
    !open(root, human()).store.nodes_where("n.id = ?1 AND n.deleted = 1", &[&id]).unwrap().is_empty()
}

/// The screen's toasts' texts.
fn toasts(s: &mut fictty::Headless) -> Vec<String> {
    s.state().toasts.iter().map(|t| t.text.clone()).collect()
}

/// An agent beside the screen adds a task: the toast says who and what on the agent's tint for
/// 4 s of the screen's clock, the row's gutter carries `•`, and `u` is set to undo it.
#[test]
fn an_agents_change_toasts_and_marks_its_row_live() {
    let (root, v) = seeded("agent");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 120, 32);
    outside_add(&root, None, "[ ] Repot the basil due:2026-10-11");
    let id = node_id(&root, "Repot the basil");
    let short = open(&root, human()).store.short(&id);
    assert!(host.poll(false));
    let text = s.screen();
    let said = format!("◆ claude added {short} \"Repot the basil\"   L review  u undo");
    // The toast owns the bar row (§3.3), daemon state and all; the corner has none.
    let bar = text.lines().nth(31).unwrap();
    assert!(bar.starts_with(&format!(" {said}")) && !bar.contains("○ local"), "the bar: {bar:?}\n{text}");
    assert_eq!(text.matches("◆ claude added").count(), 1, "only in the bar: {text}");
    assert_eq!(s.state().toasts[0].tone.as_deref(), Some("agent"));
    let gutter = |s: &mut fictty::Headless| {
        let text = s.screen();
        let row = text.lines().find(|l| l.contains("Repot the basil") && !l.contains("added")).unwrap_or_else(|| panic!("{text}")).to_string();
        row.find('•').is_some_and(|g| g < row.find(&short).unwrap())
    };
    assert!(gutter(&mut s), "the gutter has the live mark");
    assert_eq!(s.state().store["live"].read()["value"], json!({id.clone(): true}));
    // 3 s on the screen's clock: the mark has gone, the toast is still there.
    s.msg(fictty::state::Msg::Tick { now_ms: 3_001 });
    assert_eq!(s.state().store["live"].read()["value"], json!({}), "the clock took the mark away");
    assert!(!gutter(&mut s), "the live mark went at 3 s");
    assert!(s.screen().lines().nth(31).unwrap().contains("◆ claude added"));
    // 4 s: the toast has gone, and the daemon state is back in the bar.
    s.msg(fictty::state::Msg::Tick { now_ms: 4_001 });
    let bar = s.screen().lines().nth(31).unwrap().to_string();
    assert!(!bar.contains("◆ claude added") && bar.contains("○ local"), "{bar:?}");
    // With it gone, u has nothing to undo.
    s.keys(&["u"]);
    assert!(!deleted(&root, &id), "u after the toast does nothing");
    assert!(toasts(&mut s).is_empty() && s.state().status.is_none(), "{:?} {:?}", toasts(&mut s), s.state().status);
    let _ = std::fs::remove_dir_all(root);
}

/// `u` undoes the agent's change from the toast, as a new transaction; `U` redoes it.
#[test]
fn u_undoes_the_agents_change_and_capital_u_redoes_it() {
    let (root, v) = seeded("undo");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 120, 32);
    outside_add(&root, None, "[ ] Repot the basil due:2026-10-11");
    let id = node_id(&root, "Repot the basil");
    host.poll(false);
    s.keys(&["u"]);
    assert!(deleted(&root, &id), "undone: {:?}", toasts(&mut s));
    assert!(toasts(&mut s).iter().any(|t| t.starts_with("undone  tx ") && t.contains("◆ claude · 1 node") && t.ends_with("U redo")), "{:?}", toasts(&mut s));
    assert!(!s.screen().contains("Repot the basil\t"), "the row went");
    let check = open(&root, human());
    let h = check.store.history(&id, 5).unwrap();
    assert_eq!(h[0].actor, "human", "the undo is the person's own transaction");
    // U redoes it, and the bar says so, offering the undo again (§8.3); the tx it names is the
    // agent's change each time.
    let undone = toasts(&mut s).last().unwrap().clone();
    let tx = undone.split_whitespace().nth(2).unwrap().to_string();
    s.keys(&["U"]);
    assert!(!deleted(&root, &id), "redone");
    let redone = toasts(&mut s).last().unwrap().clone();
    assert_eq!(redone, format!("redone  tx {tx} · ◆ claude · 1 node   U undo"));
    s.keys(&["U"]);
    assert!(deleted(&root, &id), "U again undoes it again");
    assert_eq!(toasts(&mut s).last().unwrap(), &format!("undone  tx {tx} · ◆ claude · 1 node   U redo"));
    let _ = std::fs::remove_dir_all(root);
}

/// The Log: `u` undoes the selected transaction; `L` anywhere is the Log of agents' changes, `@`
/// everyone's again.
#[test]
fn the_log_undoes_a_transaction_and_l_reviews_the_agents() {
    let (root, v) = seeded("log-undo");
    let summarize = node_id(&root, "Summarize newsletters");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 120, 32);
    s.keys(&["L"]);
    let text = s.screen();
    assert!(text.contains("last 24h · agents") && text.contains("◆ claude") && !text.contains(" you "), "{text}");
    assert_eq!(s.state().focus.as_deref(), Some("log"));
    // The agent's transaction is the only row: u undoes it.
    s.keys(&["u"]);
    assert!(deleted(&root, &summarize), "{:?}", s.state().status);
    assert!(toasts(&mut s).iter().any(|t| t.starts_with("undone  tx ")), "{:?}", toasts(&mut s));
    s.keys(&["@"]);
    let text = s.screen();
    assert!(text.contains("last 24h · everyone") && text.contains(" you "), "{text}");
    let _ = std::fs::remove_dir_all(root);
}

/// The bar's daemon state (§3.3), as thc_core::live reports it.
#[test]
fn the_bar_says_live_or_local() {
    let (root, v) = seeded("live");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 120, 32);
    assert!(s.screen().contains("○ local"));
    host.connected(true);
    assert!(s.screen().contains("● live") && !s.screen().contains("○ local"), "{}", s.screen());
    host.connected(false);
    assert!(s.screen().contains("○ local"));
    let _ = std::fs::remove_dir_all(root);
}

// ---- help, the palette, conflicts, the minimum size ---------------------------------------------

/// `?`: the view's keys, from its bindings, by group; Esc closes it.
#[test]
fn question_mark_shows_the_views_keys() {
    let (root, v) = seeded("help");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Journal, 120, 32);
    s.keys(&["?"]);
    let text = s.screen();
    for want in ["help", "Journal", "[             the day before", "on a row", "x             done", "p h           priority high"] {
        assert!(text.contains(want), "{want:?}\n{text}");
    }
    // The rest is further down the list: everywhere's keys.
    let rows = s.state().store["keys_journal"].read()["value"].clone();
    let words: Vec<String> = rows.as_array().unwrap().iter().map(|r| format!("{} {}", r["label"].as_str().or(r["title"].as_str()).unwrap_or(""), r["words"].as_str().unwrap_or(""))).collect();
    for want in ["everywhere ", "a capture to today", "u undo the agent's change", "L review agents' changes", "? help", ": commands", "1 Today"] {
        assert!(words.iter().any(|w| w == want), "{want:?} in {words:?}");
    }
    // Enter in the Journal writes in the day; the row's Enter (open) isn't listed there.
    assert!(!text.contains("Enter         open"), "{text}");
    s.keys(&["esc"]);
    assert!(!s.screen().contains("the day before"));
    assert_eq!(s.state().focus.as_deref(), Some("journal"));
    let _ = std::fs::remove_dir_all(root);
}

/// `:`: the view's commands, filtered by what's typed; Enter runs one as its key does.
#[test]
fn colon_runs_a_command_from_the_palette() {
    let (root, v) = seeded("palette");
    let bank = node_id(&root, "Call the bank");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 120, 32);
    s.keys(&[":"]);
    assert!(s.screen().contains("commands"), "{}", s.screen());
    type_in(&mut s, "done");
    let text = s.screen();
    assert!(text.contains("done") && text.contains("on a row") && !text.contains("capture to today"), "{text}");
    s.keys(&["enter"]);
    assert!(!s.screen().contains(" commands "), "closed: {}", s.screen());
    assert_eq!(open(&root, human()).store.node(&bank).unwrap().unwrap().status.as_deref(), Some("done"), "x on the selected row");
    let _ = std::fs::remove_dir_all(root);
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let p = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &p);
        } else {
            std::fs::copy(e.path(), p).unwrap();
        }
    }
}

/// A text conflict on `Call the bank`, as scripts/fixture-conflict.sh makes one: another device
/// (studio-mini, claude) edits it, this one edits the same text after, then studio-mini's log
/// arrives.
fn with_a_conflict(name: &str) -> (std::path::PathBuf, Vault, String) {
    let (root, v) = seeded(name);
    let bank = node_id(&root, "Call the bank");
    copy_dir(&root.join("v"), &root.join("v2"));
    std::fs::create_dir_all(root.join("c2")).unwrap();
    std::fs::write(root.join("c2/device"), "studio-mini").unwrap();
    let edit = |v: &mut Vault, text: &str| {
        v.transact(|s| {
            let mut b = TxBuilder::new(s, today());
            b.set_text(&bank, text)?;
            Ok((b.finish(), ()))
        })
        .unwrap();
    };
    let mut studio = Vault::open(Paths { vault: root.join("v2"), cache: root.join("c2") }, Actor { kind: "agent".into(), name: Some("claude".into()) }, "test").unwrap();
    edit(&mut studio, "Call the bank about the card");
    std::thread::sleep(std::time::Duration::from_millis(20));
    let mut me = open(&root, human());
    edit(&mut me, "Call the bank before noon");
    copy_dir(&root.join("v2/log/studio-mini"), &root.join("v/log/studio-mini"));
    (root, v, bank)
}

/// The conflict banner (§7.4) under the tabs, `· 1 conflict` in the bar, `≠` in the row's gutter.
#[test]
fn an_open_conflict_shows_its_banner_and_gutter() {
    let (root, v, bank) = with_a_conflict("conflict");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 120, 32);
    host.poll(true);
    // studio-mini's edit was claude's: its toast owns the bar for 4 s first.
    s.msg(fictty::state::Msg::Tick { now_ms: 4_001 });
    let short = open(&root, human()).store.short(&bank);
    let text = s.screen();
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[2].contains(&format!("≠ 1 conflict  {short} text was edited on two devices")), "row 2 is the banner:\n{text}");
    assert!(lines[31].contains("· 1 conflict"), "the bar: {:?}", lines[31]);
    let row = lines.iter().find(|l| l.contains(&short) && l.contains("[ ]")).unwrap();
    assert!(row.trim_start().starts_with('≠'), "the gutter: {row:?}");
    let _ = std::fs::remove_dir_all(root);
}

/// Below 60×24 only the one centred line shows (§3).
#[test]
fn below_60_by_24_only_one_line_shows() {
    let (root, v) = seeded("small");
    let host = ThcHost::new(v).at(now());
    for (w, h) in [(52, 20), (59, 30), (100, 23)] {
        let mut s = screen(&host, View::Today, w, h);
        let text = s.screen();
        let shown: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(shown.len(), 1, "{w}x{h}:\n{text}");
        assert_eq!(shown[0].trim(), format!("thc needs at least 60×24 · now {w}×{h}"));
        assert_eq!(text.lines().position(|l| !l.trim().is_empty()), Some((h as usize - 1) / 2), "centred");
    }
    let mut s = screen(&host, View::Today, 60, 24);
    assert!(s.screen().contains("Today"));
    let _ = std::fs::remove_dir_all(root);
}

// ---- harness round 3 -------------------------------------------------------------------------

/// E12: Esc with a selection in a page drops the selection and keeps writing; Esc again closes
/// the page, back on its row in Pages.
#[test]
fn e12_esc_drops_a_selection_first_then_closes_the_page() {
    let (root, v) = seeded("e12");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Pages, 120, 32);
    s.keys(&["down", "down", "enter"]);
    assert_eq!(s.state().focus.as_deref(), Some("pages_doc"));
    s.keys(&["d-up", "s-right", "s-right"]);
    let sel = caret(&mut s);
    assert_ne!(sel["ranges"][0]["anchor"], sel["ranges"][0]["head"], "a selection: {sel}");
    s.keys(&["esc"]);
    assert_eq!(s.state().focus.as_deref(), Some("pages_doc"), "still writing");
    let after = caret(&mut s);
    assert_eq!(after["ranges"][0]["anchor"], after["ranges"][0]["head"], "collapsed: {after}");
    s.keys(&["esc"]);
    assert_eq!(s.state().focus.as_deref(), Some("pages"), "closed");
    assert!(!s.screen().contains("write here"));
    let _ = std::fs::remove_dir_all(root);
}

/// N04 at 120 wide: found with the finder, opened, Esc: the list has the focus on that page.
#[test]
fn n04_esc_from_a_page_opened_through_the_finder_lands_on_it() {
    let (root, v) = seeded("n04-wide");
    let health = node_id(&root, "Health");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 120, 32);
    s.keys(&["4"]);
    type_in(&mut s, "Health");
    s.keys(&["enter", "esc"]);
    let st = s.state();
    assert_eq!(st.focus.as_deref(), Some("pages"), "the list, not the prompt");
    let sel = st.local["pages"].key.clone();
    assert_eq!(sel.as_deref(), Some(health.as_str()), "on Health");
    let _ = std::fs::remove_dir_all(root);
}

/// T02 (fictty ec2fb64 continues lists): Enter at the end of `- Parent item`, type, Tab: a child
/// note under it in the vault.
#[test]
fn t02_enter_continues_the_list_and_tab_makes_a_child() {
    let (root, v) = seeded("t02");
    let q4 = node_id(&root, "Q4 Planning");
    let host = ThcHost::new(v).at(now());
    let mut s = page(&host, &q4);
    s.keys(&["d-down"]);
    type_in(&mut s, "- Parent item");
    s.keys(&["enter"]);
    type_in(&mut s, "Child note");
    s.keys(&["tab"]);
    let text = s.screen();
    assert!(text.contains("- Parent item") && text.contains("  - Child note"), "{text}");
    let top = kids(&root, &q4);
    let parent = top.iter().find(|k| k.0 == "Parent item").unwrap_or_else(|| panic!("{top:?}"));
    assert_eq!(kid_texts(&root, &parent.1), vec!["Child note"]);
    let _ = std::fs::remove_dir_all(root);
}

// ---- Inbox triage, Agenda, panes, Journal, Search, themes --------------------------------------

/// The Inbox with four notes, oldest first: desk, a, b, c.
fn inbox_of_four(name: &str) -> (std::path::PathBuf, ThcHost, fictty::Headless) {
    let (root, v) = seeded(name);
    let host = ThcHost::new(v).at(now());
    for t in ["Note a", "Note b", "Note c"] {
        host.perform("add", &json!({"text": t, "inbox": true})).unwrap();
    }
    let s = screen(&host, View::Inbox, 120, 32);
    (root, host, s)
}

/// The selected row's id in a list.
fn selected(s: &mut fictty::Headless, list: &str) -> String {
    s.state().local[list].key.clone().unwrap_or_default()
}

/// §5.2: `t` a task and `x` done, each moving the cursor to the next row; `m` the move picker
/// (fuzzy: `q4` finds Q4 Planning), Enter moves it; `D` asks first, `n` keeps it, `y` deletes it.
/// After each the cursor is on the next row.
#[test]
fn inbox_triage_moves_on_after_each_action() {
    let (root, _host, mut s) = inbox_of_four("triage");
    let (desk, a, b, c) = (node_id(&root, "Look into a standing desk"), node_id(&root, "Note a"), node_id(&root, "Note b"), node_id(&root, "Note c"));
    let status = |id: &str| open(&root, human()).store.node(id).unwrap().unwrap().status;
    assert_eq!(selected(&mut s, "inbox"), desk);
    s.keys(&["t"]);
    assert_eq!(status(&desk).as_deref(), Some("todo"));
    assert_eq!(selected(&mut s, "inbox"), a, "t: on to the next row");
    s.keys(&["x"]);
    assert_eq!(status(&a).as_deref(), Some("done"));
    assert_eq!(selected(&mut s, "inbox"), b, "x: on to the next row");
    // m: the picker, fuzzy.
    s.keys(&["m"]);
    assert_eq!(s.state().focus.as_deref(), Some("inbox_move_q"));
    type_in(&mut s, "q4");
    let text = s.screen();
    assert!(text.contains("› q4") && text.contains("move to") && text.contains("¶ Q4 Planning") && !text.contains("¶ Atomic Habits"), "{text}");
    s.keys(&["enter"]);
    let q4 = node_id(&root, "Q4 Planning");
    assert_eq!(open(&root, human()).store.node(&b).unwrap().unwrap().parent.as_deref(), Some(q4.as_str()), "moved under Q4 Planning");
    assert_eq!(s.state().focus.as_deref(), Some("inbox"));
    assert_eq!(selected(&mut s, "inbox"), c, "m: the next row is under the cursor");
    // D: n keeps it, y deletes it.
    s.keys(&["D"]);
    assert!(s.screen().contains("Delete “Note c”?"), "{}", s.screen());
    s.keys(&["n"]);
    assert!(!deleted(&root, &c));
    s.keys(&["D", "y"]);
    assert!(deleted(&root, &c), "deleted after y");
    assert!(toasts(&mut s).iter().any(|t| t.starts_with("deleted ")), "{:?}", toasts(&mut s));
    // At 120 the detail pane lists the Triage keys, from the rows' own bindings.
    let text = s.screen();
    for want in ["Triage", "m    move to a page", "t    task on / off", "D    delete"] {
        assert!(text.contains(want), "{want:?}\n{text}");
    }
    let _ = std::fs::remove_dir_all(root);
}

/// §5.1 `w`: the agenda (a heading, days in bold with their rows, empty days as one muted line,
/// Later), and `w` again the sections.
#[test]
fn w_swaps_today_for_the_agenda_and_back() {
    let (root, v) = seeded("agenda");
    let host = ThcHost::new(v).at(now());
    outside_add(&root, None, "[ ] Book the dentist due:2026-10-20");
    host.poll(false);
    let mut s = screen(&host, View::Today, 120, 32);
    s.keys(&["w"]);
    let text = s.screen();
    for want in ["Agenda", "7 days   w sections", "Sun Oct 11 · today", "Mon Oct 12 · tomorrow", "Tue Oct 13 · Wed Oct 14 · Thu Oct 15", "nothing scheduled", "Later", "Book the dentist", "agenda · 7 days"] {
        assert!(text.contains(want), "{want:?}\n{text}");
    }
    // Under its day a row's meta drops the date: `due`, not `due today`.
    let water = text.lines().find(|l| l.contains("Water plants")).unwrap();
    assert!(water.contains(" due ") && !water.contains("due today"), "{water}");
    s.keys(&["w"]);
    assert!(s.screen().contains("Overdue  1") && !s.screen().contains("Agenda"), "{}", s.screen());
    let _ = std::fs::remove_dir_all(root);
}

/// §3.4: `\` takes the detail pane away and back; ⌃W moves the focus to it and back. The other
/// pane's cursor row shows only `▌` (no selection background).
#[test]
fn backslash_and_ctrl_w_work_the_detail_pane() {
    let (root, v) = seeded("panes");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 120, 32);
    let has_pane = |s: &mut fictty::Headless| s.screen().lines().nth(4).is_some_and(|l| l.contains('│'));
    assert!(has_pane(&mut s));
    s.keys(&["\\"]);
    assert!(!has_pane(&mut s), "{}", s.screen());
    s.keys(&["\\"]);
    assert!(has_pane(&mut s));
    let sel_bg = |s: &mut fictty::Headless| {
        let ansi = s.ansi();
        let line = ansi.lines().find(|l| l.contains("Call the bank") && l.contains('▌')).unwrap().to_string();
        line.contains(&format!("48;2;{}", rgb(palette_hex("ember-dark", thc_core::palette::Token::Selection))))
    };
    assert!(sel_bg(&mut s), "the focused list's row on `selection`");
    s.keys(&["c-w"]);
    assert_eq!(s.state().focus.as_deref(), Some("today_pane"));
    assert!(!sel_bg(&mut s), "the other pane: only ▌");
    s.keys(&["c-w"]);
    assert_eq!(s.state().focus.as_deref(), Some("today"));
    let _ = std::fs::remove_dir_all(root);
}

fn palette_hex(theme: &str, t: thc_core::palette::Token) -> String {
    thc_screen::theme::theme(theme).unwrap().tokens[t.name()].clone()
}

/// `#1B1916` as `27;25;22`, the way an SGR says it.
fn rgb(hex: String) -> String {
    let n = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap();
    format!("{};{};{}", n(1), n(3), n(5))
}

/// §5.5: the title on row 2, the strip on row 3 from column 3, the `━` under the shown day on
/// row 4, entries from row 6; the calendar with today as `[11•`; `[` `]` `T` move the day.
#[test]
fn the_journal_matches_its_spec() {
    let (root, v) = seeded("journal-spec");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Journal, 120, 32);
    let text = s.screen();
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[2].starts_with("§ Sunday, October 11 2026 · today"), "{}", lines[2]);
    assert!(lines[3].starts_with("   Thu 8"), "{:?}", lines[3]);
    let sun = lines[3].find("Sun 11").unwrap();
    let rule = lines[4].find('━').unwrap();
    assert_eq!(lines[4][..rule].chars().count() + 1, lines[3][..sun].chars().count(), "a cell before the label: {:?}", lines[4]);
    assert_eq!(lines[4].matches('━').count(), "Sun 11".len() + 2);
    assert!(lines[5].trim().is_empty() && lines[6].contains("Call the bank"), "{text}");
    assert!(text.contains("October 2026") && text.contains("Mo Tu We Th Fr Sa Su") && text.contains("[11•"), "{text}");
    s.keys(&["["]);
    assert!(s.screen().contains("§ Saturday, October 10 2026"));
    s.keys(&["T"]);
    assert!(s.screen().contains("§ Sunday, October 11 2026"));
    let _ = std::fs::remove_dir_all(root);
}

/// §5.6: matches in bold on `accent-tint`, the preview with the match among its siblings, and
/// ⌃Q the search as a Tasks filter.
#[test]
fn search_previews_in_context_and_ctrl_q_makes_a_tasks_filter() {
    let (root, v) = seeded("search-spec");
    let host = ThcHost::new(v).at(now());
    let mut s = host.app(&Route { view: View::Search, query: Some("bank".into()), ..Route::default() }, "ember-dark").headless(120, 32).unwrap();
    let text = s.screen();
    // The preview: today's journal, the match marked among the day's other entries.
    assert!(text.contains("│ § Sun Oct 11") && text.contains("▌ Call the b") && text.contains("Morning run"), "{text}");
    let ansi = s.ansi();
    let tint = rgb(palette_hex("ember-dark", thc_core::palette::Token::AccentTint));
    assert!(ansi.contains(&format!("1;38;2;{};48;2;{tint}m", rgb(palette_hex("ember-dark", thc_core::palette::Token::Text)))) || ansi.contains(&format!("48;2;{tint}mbank")), "bold on accent-tint");
    s.keys(&["c-q"]);
    let st = s.state();
    assert_eq!(st.store["tasks_q"].read()["text"], "text:bank");
    assert!(s.screen().contains("q› text:bank") && s.screen().contains("Call the bank"), "{}", s.screen());
    let _ = std::fs::remove_dir_all(root);
}

/// `~` cycles the four themes (in fictty's order: by name), each drawn in its own palette.
#[test]
fn tilde_cycles_the_four_themes() {
    let (root, v) = seeded("themes");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 120, 32);
    for name in ["ember-dark", "ember-light", "newsprint", "redacted", "ember-dark"] {
        assert_eq!(s.state().theme.as_deref(), Some(name));
        let bg = rgb(palette_hex(name, thc_core::palette::Token::Bg));
        assert!(s.ansi().contains(&format!("48;2;{bg}m")), "{name}'s background");
        s.keys(&["~"]);
    }
    let _ = std::fs::remove_dir_all(root);
}

// ---- the sidebar (sidebar.md §15) ----------------------------------------------------------------

/// The sidebar's panel keys, top to bottom.
fn panels(s: &mut fictty::Headless) -> Vec<String> {
    let v = s.state().store["sidebar"].read()["value"].clone();
    v["open"].as_array().unwrap().iter().map(|p| format!("{}:{}", p["kind"].as_str().unwrap(), p["ref"].as_str().unwrap())).collect()
}

/// The screen's column where the sidebar's `│` stands on a row, if it does.
fn divider(s: &mut fictty::Headless, row: usize) -> Option<usize> {
    let text = s.screen();
    let line: Vec<char> = text.lines().nth(row)?.chars().collect();
    line.iter().rposition(|c| *c == '│')
}

/// S1, S2: `o` on a row opens its page or day aside, on top; focus stays in the list; opening
/// one already there moves it up, never twice. S19: the detail pane yields to it at 140, and
/// comes back when `⌥\` hides it.
#[test]
fn s1_s2_s19_o_opens_a_rows_page_or_day_aside_newest_on_top() {
    let (root, v) = seeded("aside");
    let home = node_id(&root, "Home");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 140, 40);
    // Call the bank lives in today's journal; Water plants on ¶ Home.
    s.keys(&["o"]);
    assert_eq!(panels(&mut s), vec!["day:2026-10-11"]);
    assert_eq!(s.state().focus.as_deref(), Some("today"), "focus stays in main");
    let text = s.screen();
    assert!(text.contains("§ Sun 11 Oct · today"), "{text}");
    assert!(!text.contains("status     todo"), "the detail pane yields (§6.2): {text}");
    s.keys(&["down", "o"]);
    assert_eq!(panels(&mut s), vec![format!("page:{home}"), "day:2026-10-11".into()]);
    s.keys(&["up", "o"]);
    assert_eq!(panels(&mut s), vec!["day:2026-10-11".to_string(), format!("page:{home}")], "moved up, not twice");
    s.keys(&["a-\\"]);
    assert!(s.screen().contains("status     todo") && !s.screen().contains("§ Sun 11 Oct · today"), "{}", s.screen());
    s.keys(&["a-\\"]);
    assert!(s.screen().contains("§ Sun 11 Oct · today"));
    let _ = std::fs::remove_dir_all(root);
}

/// S18: a third of the width, 40 to 64: 46 at 140, 40 at 120, 64 at 200.
#[test]
fn s18_the_column_is_a_third_between_40_and_64() {
    let (root, v) = seeded("width");
    let host = ThcHost::new(v).at(now());
    for (w, side) in [(140u16, 46usize), (120, 40), (200, 64)] {
        let mut s = screen(&host, View::Today, w, 40);
        s.keys(&["o"]);
        let at = divider(&mut s, 4).unwrap_or_else(|| panic!("{w}: {}", s.screen()));
        assert_eq!(w as usize - at - 1, side, "{w} wide: the sidebar\n{}", s.screen());
    }
    let _ = std::fs::remove_dir_all(root);
}

/// S3, S7, S8, S10, S12: ⌥S focuses the stack, Esc gives the focus back; ⌥C folds (the count
/// shows), ⌥P pins, ⌥W closes, ⌥⇧T reopens, ⌥M opens it in the main view and closes it.
#[test]
fn s3_to_s12_the_sidebar_keys() {
    let (root, v) = seeded("sidebar-keys");
    let home = node_id(&root, "Home");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 140, 40);
    s.keys(&["o", "down", "o"]);
    // The newest panel is the active one (§2 rule 1): ⌥S lands in ¶ Home's text.
    let home_doc = format!("sidebar_today_panel-page-{home}");
    s.keys(&["a-s"]);
    assert_eq!(s.state().focus.as_deref(), Some(home_doc.as_str()));
    s.keys(&["esc"]);
    assert_eq!(s.state().focus.as_deref(), Some("today"), "back to main");
    s.keys(&["a-s", "a-c"]);
    assert!(s.screen().contains("2 open"), "folded ¶ Home shows its count: {}", s.screen());
    assert_eq!(s.state().focus.as_deref(), Some(format!("sidebar_today_head-page-{home}").as_str()), "folded: its header");
    s.keys(&["a-c", "a-p"]);
    assert!(s.screen().contains("pinned"));
    // The day under it: close it, reopen it.
    s.keys(&["a-j"]);
    assert_eq!(s.state().focus.as_deref(), Some("sidebar_today_panel-day-2026-10-11"));
    s.keys(&["a-w"]);
    assert_eq!(panels(&mut s), vec![format!("page:{home}")]);
    s.keys(&["a-s-t"]);
    assert_eq!(panels(&mut s), vec![format!("page:{home}"), "day:2026-10-11".into()], "reopened under the pinned one");
    // ⌥M on Home: the main view is ¶ Home to write in, and the panel is gone.
    assert_eq!(s.state().focus.as_deref(), Some(home_doc.as_str()), "the panel that took its place");
    s.keys(&["a-m"]);
    assert_eq!(panels(&mut s), vec!["day:2026-10-11"]);
    assert_eq!(s.state().focus.as_deref(), Some("pages_doc"));
    assert!(s.screen().contains("¶ Home") && s.screen().contains("Water plants"), "{}", s.screen());
    let _ = std::fs::remove_dir_all(root);
}

/// S16, S17: narrower, the same stack is a drawer (90–119: 60 wide on the right) or replaces
/// the view (< 90, with a back row); Esc closes it and keeps the stack.
#[test]
fn s16_s17_the_drawer_and_replace() {
    let (root, v) = seeded("drawer");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 100, 30);
    s.keys(&["o"]);
    let text = s.screen();
    assert!(text.lines().nth(2).unwrap().contains("§ Sun 11 Oct · today"), "the drawer, over the right: {text}");
    // The open put the focus in the drawer.
    assert!(s.state().focus.as_deref().is_some_and(|f| f.starts_with("drawer_today_panel-")), "{:?}", s.state().focus);
    s.keys(&["esc"]);
    assert!(!s.screen().contains("§ Sun 11 Oct"), "closed");
    assert_eq!(panels(&mut s), vec!["day:2026-10-11"], "the stack stays");
    let mut s = screen(&host, View::Today, 80, 24);
    s.keys(&["o"]);
    let text = s.screen();
    assert!(text.contains("‹ main   beside it: 1 panels") && text.contains("§ Sun 11 Oct · today"), "{text}");
    s.keys(&["a-s", "esc"]);
    assert!(s.screen().contains("Call the bank") && !s.screen().contains("beside it"), "{}", s.screen());
    let _ = std::fs::remove_dir_all(root);
}

/// S21's half that's the screen's: an agent writes the stack (as `thc ui aside` does over the
/// socket); the panel is on top, says who opened it, and the focus doesn't move.
#[test]
fn s21_an_agents_aside_shows_who_and_leaves_the_focus() {
    let (root, v) = seeded("agent-aside");
    let health = node_id(&root, "Health");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 140, 40);
    // `thc ui aside Health` as claude, over the screen's socket: read, then the writes.
    let read = s.request(json!({"op": "read", "key": "sidebar"}));
    let store = open(&root, human());
    let (key, _, reqs) = thc_screen::sidebar::aside_requests(&store.store, &read, "Health", Some("claude"), false, false, false, today()).unwrap();
    assert_eq!(key, format!("page:{health}"));
    for r in &reqs {
        let reply = s.request(r.clone());
        assert_ne!(reply["ok"], json!(false), "{r} → {reply}");
    }
    // A stale write (the person changed the stack since the read) is refused.
    let stale = s.request(reqs[0].clone());
    assert_eq!(stale["ok"], json!(false), "{stale}");
    // It shows at once, with who opened it, and its body; the focus didn't move.
    let text = s.screen();
    assert!(text.contains("¶ Health") && text.contains("◆ claude"), "{text}");
    assert_eq!(s.state().focus.as_deref(), Some("today"));
    let _ = std::fs::remove_dir_all(root);
}

/// sidebar.md §7.1, checked in fictty: two editors on one hosted entry (`page/<id>`) are one
/// text. Typing in one shows in the other at once, the vault gets it, and an undo in the other
/// takes it back.
#[test]
fn one_page_two_editors_one_text_one_undo() {
    use fictty::model::{Kind, Node, Ui};
    let (root, v) = seeded("two-views");
    let home = node_id(&root, "Home");
    let host = ThcHost::new(v).at(now());
    let editor = |id: &str| Node {
        id: Some(id.into()),
        kind: Kind::Editor { text: String::new(), doc: Some(format!("page/{home}")), placeholder: None, syntax: Some("markdown".into()), readonly: false, tab: None, tasks: None, caret: Some("end".into()), links: vec![] },
        ..Node::default()
    };
    let ui = Ui { root: Node { kind: Kind::Col { children: vec![editor("main"), editor("panel")], gap: None }, ..Node::default() }, focus: Some("main".into()), status_bar: Some(false), ..Ui::default() };
    let mut s = fictty::App::new(ui).themes(thc_screen::theme::themes()).theme("ember-dark").host(host.clone()).headless(100, 20).unwrap();
    type_in(&mut s, "Repot the fern");
    let text = s.screen();
    assert_eq!(text.matches("Repot the fern").count(), 2, "both editors show it: {text}");
    assert_eq!(kid_texts(&root, &home).last().map(String::as_str), Some("Repot the fern"));
    s.msg(fictty::state::Msg::Click { id: "panel".into(), row: None, tab: None });
    s.keys(&["d-z"]);
    assert_eq!(s.screen().matches("Repot the fern").count(), 0, "one undo history: {}", s.screen());
    assert!(!kid_texts(&root, &home).contains(&"Repot the fern".to_string()));
    let _ = std::fs::remove_dir_all(root);
}

/// §2 rule 1 and §6.4: narrow, an open opens the drawer and focuses it (wide, the focus stays);
/// the drawer has its `│` left edge, 60 columns from the right.
#[test]
fn narrow_an_open_focuses_the_drawer_which_has_its_edge() {
    let (root, v) = seeded("drawer-focus");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 100, 30);
    s.keys(&["o"]);
    assert_eq!(s.state().focus.as_deref(), Some("drawer_today_panel-day-2026-10-11"), "the drawer takes it");
    let text = s.screen();
    let edge: Vec<char> = text.lines().nth(4).unwrap().chars().collect();
    assert_eq!(edge.get(40), Some(&'│'), "the edge at 100 - 60: {text}");
    let mut s = screen(&host, View::Today, 140, 40);
    s.keys(&["o"]);
    assert_eq!(s.state().focus.as_deref(), Some("today"), "beside, the focus stays");
    let _ = std::fs::remove_dir_all(root);
}

/// S14: a day panel changes day with `[` `]`, and `T` makes it the relative today.
#[test]
fn s14_a_day_panel_changes_day() {
    let (root, v) = seeded("day-panel");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 140, 40);
    s.keys(&["o", "a-s", "c-p"]);
    assert_eq!(panels(&mut s), vec!["day:2026-10-10"]);
    assert!(s.screen().contains("§ Sat 10 Oct"), "{}", s.screen());
    s.keys(&["c-n", "a-]"]);
    assert_eq!(panels(&mut s), vec!["day:2026-10-12"]);
    s.keys(&["a-c", "T"]);
    assert_eq!(panels(&mut s), vec!["day:today"]);
    assert!(s.screen().contains("§ Sun 11 Oct · today"));
    let _ = std::fs::remove_dir_all(root);
}

/// S20: the stack is kept in the vault's cache (per vault, per device) and comes back on the
/// next run; a panel whose page was deleted is dropped, and the bar says so once.
#[test]
fn s20_the_stack_comes_back_and_a_deleted_page_goes() {
    let (root, v) = seeded("persist");
    let home = node_id(&root, "Home");
    let host = ThcHost::new(v).at(now()).persisting();
    let mut s = screen(&host, View::Today, 140, 40);
    s.keys(&["o", "down", "o", "a-s", "a-p"]);
    let before = panels(&mut s);
    assert_eq!(before.len(), 2);
    drop(s);
    drop(host);
    assert!(thc_screen::sidebar::file(&root.join("c")).exists(), "kept in the vault's cache");
    let host = ThcHost::new(open(&root, human())).at(now()).persisting();
    let mut s = screen(&host, View::Today, 140, 40);
    assert_eq!(panels(&mut s), before, "the same stack");
    assert!(s.screen().contains("¶ Home") && s.screen().contains("pinned"), "{}", s.screen());
    drop(s);
    drop(host);
    // Home deleted elsewhere: dropped on the next run, said once.
    let mut other = open(&root, human());
    other
        .transact(|st| {
            let mut b = TxBuilder::new(st, today());
            b.delete(&home)?;
            Ok((b.finish(), ()))
        })
        .unwrap();
    drop(other);
    let host = ThcHost::new(open(&root, human())).at(now()).persisting();
    let mut s = screen(&host, View::Today, 140, 40);
    assert_eq!(panels(&mut s), vec!["day:2026-10-11"]);
    assert!(toasts(&mut s).iter().any(|t| t == "¶ Home was deleted · removed from the sidebar"), "{:?}", toasts(&mut s));
    // Snapshots and tests that don't persist neither read nor write it.
    let host = ThcHost::new(open(&root, human())).at(now());
    let mut s = screen(&host, View::Today, 140, 40);
    assert!(panels(&mut s).is_empty());
    let _ = std::fs::remove_dir_all(root);
}

/// S12: ⌥M is a step in the main view: Esc from the page comes back to the view ⌥M was
/// pressed in, its list focused.
#[test]
fn s12_esc_after_open_in_main_comes_back() {
    let (root, v) = seeded("to-main-back");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 140, 40);
    s.keys(&["down", "o", "a-s", "a-m"]);
    assert_eq!(s.state().focus.as_deref(), Some("pages_doc"));
    s.keys(&["esc"]);
    let st = s.state();
    assert_eq!(st.focus.as_deref(), Some("today"), "back in Today");
    assert_eq!(st.local["views"].tab, 0);
    let _ = std::fs::remove_dir_all(root);
}

/// §0 frame B: with the column showing, the bar still spans the screen under it.
#[test]
fn the_bar_spans_the_width_beside_the_column() {
    let (root, v) = seeded("bar-width");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 140, 40);
    s.keys(&["o"]);
    let text = s.screen();
    let bar = text.lines().nth(39).unwrap();
    let right = bar.chars().count();
    assert!(bar.starts_with(" today ·") && right >= 138, "to the right edge: {bar:?}");
    assert!(!bar.contains('│'), "{bar:?}");
    let _ = std::fs::remove_dir_all(root);
}

/// N04 exactly as the harness runs it (scripts/e2e/scenarios/N04): a vault with only ¶ Health
/// (and a note under it); `4`, type `Health`, Enter (the page to write in), Esc: the focus has
/// left the page, and the Pages list's selected row is Health.
#[test]
fn n04_harness_repro_esc_lands_on_the_page_found() {
    let root = thc_core::scratch::dir(&format!("thc-screen-n04-exact-{}", thc_core::id::new_id()));
    vault::init(&root.join("v"), None, None).unwrap();
    let mut v = open(&root, human());
    v.transact(|s| {
        let mut b = TxBuilder::new(s, today());
        let health = b.create_page("Health", &[])?;
        b.create_from_capture(Some(health), &thc_core::capture::parse("Schedule annual physical", today()).unwrap(), None)?;
        Ok((b.finish(), ()))
    })
    .unwrap();
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 120, 32);
    s.keys(&["4"]);
    type_in(&mut s, "Health");
    s.keys(&["enter"]);
    assert_eq!(s.state().focus.as_deref(), Some("pages_doc"));
    s.keys(&["esc"]);
    assert_ne!(s.state().focus.as_deref(), Some("pages_doc"));
    let row = fictty::state::selected_item(s.state()).unwrap_or_default();
    assert_eq!(row["text"], "Health", "the selected row: {row}");
    let _ = std::fs::remove_dir_all(root);
}

// ---- container queries, panel bodies, height sharing, links ------------------------------------

/// With the sidebar beside it at 140 columns, the main pane is 93 wide: Tasks lays itself out
/// narrow (a list, not the table), by the pane's width, not the screen's.
#[test]
fn beside_the_sidebar_tasks_lays_out_by_its_pane() {
    let (root, v) = seeded("container");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Tasks, 140, 40);
    assert!(s.screen().contains("  id        task"), "the table at 140: {}", s.screen());
    s.keys(&["o"]);
    let text = s.screen();
    assert!(!text.contains("  id        task") && text.contains("Call the bank"), "narrow beside the sidebar: {text}");
    let _ = std::fs::remove_dir_all(root);
}

/// Panels have bodies: a page panel is an editor on the page's own entry, so typing there
/// writes the vault, and a list panel (`@today`) shows the view's rows, where `x` completes in
/// place (S15).
#[test]
fn panels_show_their_bodies_and_write() {
    let (root, v) = seeded("bodies");
    let home = node_id(&root, "Home");
    let bank = node_id(&root, "Call the bank");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 140, 40);
    s.keys(&["down", "o"]);
    let text = s.screen();
    assert!(text.contains("- [ ] Water plants #garden due:2026-10-11") && text.contains("- Wifi password is on the fridge"), "¶ Home's text in its panel: {text}");
    s.keys(&["a-s", "d-down"]);
    type_in(&mut s, "Fix the gate");
    assert!(kid_texts(&root, &home).contains(&"Fix the gate".to_string()), "{:?}", kid_texts(&root, &home));
    // A list panel: Today's rows; x on one completes it.
    let read = s.request(json!({"op": "read", "key": "sidebar"}));
    let store = open(&root, human());
    let (_, _, reqs) = thc_screen::sidebar::aside_requests(&store.store, &read, "@today", None, false, false, false, today()).unwrap();
    for r in &reqs {
        s.request(r.clone());
    }
    s.keys(&["esc", "a-s"]);
    assert_eq!(s.state().focus.as_deref(), Some("sidebar_today_rows-view-today"));
    s.keys(&["x"]);
    assert_eq!(open(&root, human()).store.node(&bank).unwrap().unwrap().status.as_deref(), Some("done"));
    let _ = std::fs::remove_dir_all(root);
}

/// S11 (§6.3): when everything fits, each body is its natural height; when not, the panels other
/// than the active one take at most 6 rows, the active one the rest (at least 3), and what's
/// still too tall folds from the bottom.
#[test]
fn s11_panels_share_the_height() {
    use thc_screen::feeds::share_height;
    let p = |id: &str, h: usize| (id.to_string(), h);
    assert_eq!(share_height(&[p("a", 4), p("b", 5)], "a", 40), vec![(4, 0), (5, 0)], "it fits");
    assert_eq!(share_height(&[p("a", 30), p("b", 20), p("c", 10)], "b", 40), vec![(6, 24), (20, 0), (6, 4)]);
    assert_eq!(share_height(&[p("a", 30), p("b", 40)], "b", 24), vec![(6, 24), (12, 28)], "the active takes what's left");
    assert_eq!(share_height(&[p("a", 9), p("b", 9), p("c", 9), p("d", 9)], "a", 14).iter().filter(|(b, _)| *b == 0).count(), 3, "folded for room from the bottom");
}

/// Where `needle` is in an editor's text, as a click there sends it: the cell relative to the
/// editor (`first` is the editor's first line, to find where it starts on screen).
fn cell_of(s: &mut fictty::Headless, first: &str, needle: &str) -> (u16, u16) {
    let text = s.screen();
    let lines: Vec<&str> = text.lines().collect();
    let top = lines.iter().position(|l| l.contains(first)).expect("the editor's first line");
    let left = lines[top].chars().collect::<String>().find(first).map(|b| lines[top][..b].chars().count()).unwrap();
    let row = lines.iter().position(|l| l.contains(needle)).expect("the needle");
    let col = lines[row][..lines[row].find(needle).unwrap()].chars().count();
    ((col - left) as u16 + 1, (row - top) as u16)
}

/// Today's journal day to write in, with `Morning run, see [[Home]]` in it.
fn day_doc(name: &str) -> (std::path::PathBuf, ThcHost, fictty::Headless) {
    let (root, v) = seeded(name);
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Journal, 140, 40);
    s.keys(&["enter"]);
    assert_eq!(s.state().focus.as_deref(), Some("journal_doc"));
    (root, host, s)
}

fn point(s: &mut fictty::Headless, id: &str, (col, row): (u16, u16), shift: bool) {
    s.msg(fictty::state::Msg::Point { id: id.into(), col, row, extend: shift });
}

/// E63: a click on a link's title follows it in the main view, and Esc comes back to the day.
#[test]
fn e63_a_click_on_a_link_follows_it() {
    let (root, _host, mut s) = day_doc("e63");
    let at = cell_of(&mut s, "- [ ] Call the bank", "Home]]");
    point(&mut s, "journal_doc", at, false);
    let st = s.state();
    assert_eq!(st.focus.as_deref(), Some("pages_doc"), "followed: {:?}", st.status);
    assert!(s.screen().contains("¶ Home") && s.screen().contains("Water plants"), "{}", s.screen());
    s.keys(&["esc"]);
    assert_eq!(s.state().focus.as_deref(), Some("journal_doc"), "back to the day");
    assert_eq!(s.state().local["views"].tab, 4);
    let _ = std::fs::remove_dir_all(root);
}

/// E64: a click on the `[[` places the caret there, with no navigation.
#[test]
fn e64_a_click_on_the_brackets_places_the_caret() {
    let (root, _host, mut s) = day_doc("e64");
    let at = cell_of(&mut s, "- [ ] Call the bank", "[[Home]]");
    point(&mut s, "journal_doc", at, false);
    assert_eq!(s.state().focus.as_deref(), Some("journal_doc"), "no navigation");
    let _ = std::fs::remove_dir_all(root);
}

/// E66: arrows into a link never navigate; ⌃O follows it; ⌥O opens it aside.
#[test]
fn e66_arrows_into_a_link_then_ctrl_o_and_alt_o() {
    let (root, _host, mut s) = day_doc("e66");
    let home = node_id(&root, "Home");
    // To the start of the Morning run line, then along it into the link.
    let at = cell_of(&mut s, "- [ ] Call the bank", "Morning run");
    point(&mut s, "journal_doc", at, false);
    for _ in 0.."Morning run, see [[H".len() {
        s.keys(&["right"]);
    }
    assert_eq!(s.state().focus.as_deref(), Some("journal_doc"), "no navigation on arrows");
    s.keys(&["a-o"]);
    assert_eq!(panels(&mut s), vec![format!("page:{home}")], "⌥O: aside");
    assert_eq!(s.state().focus.as_deref(), Some("journal_doc"), "the focus stays");
    s.keys(&["c-o"]);
    assert_eq!(s.state().focus.as_deref(), Some("pages_doc"), "⌃O follows");
    let _ = std::fs::remove_dir_all(root);
}

/// E67: a ⇧-click on a link's title opens it aside; the main page and its caret stay.
#[test]
fn e67_a_shift_click_on_a_link_opens_it_aside() {
    let (root, _host, mut s) = day_doc("e67");
    let home = node_id(&root, "Home");
    let at = cell_of(&mut s, "- [ ] Call the bank", "Home]]");
    point(&mut s, "journal_doc", at, true);
    assert_eq!(panels(&mut s), vec![format!("page:{home}")]);
    assert_eq!(s.state().focus.as_deref(), Some("journal_doc"), "main stays");
    let _ = std::fs::remove_dir_all(root);
}

/// E69: a click on a tag places the caret (tags don't navigate); a ⇧-click opens its list aside.
#[test]
fn e69_a_tag_click_places_the_caret_and_a_shift_click_opens_its_list() {
    let (root, v) = seeded("e69");
    let home = node_id(&root, "Home");
    let host = ThcHost::new(v).at(now());
    let mut s = page(&host, &home);
    let at = cell_of(&mut s, "- [ ] Water plants", "#garden");
    point(&mut s, "pages_doc", at, false);
    assert_eq!(s.state().focus.as_deref(), Some("pages_doc"), "no navigation");
    assert!(panels(&mut s).is_empty());
    point(&mut s, "pages_doc", at, true);
    assert_eq!(panels(&mut s), vec!["query:#garden"], "the tag's list aside");
    let _ = std::fs::remove_dir_all(root);
}

/// In a sidebar panel, Esc with a selection drops it and keeps writing there; a second Esc
/// returns to the main view (sidebar.md §5.1, editing.md E12).
#[test]
fn esc_in_a_panel_drops_a_selection_then_returns_to_main() {
    let (root, v) = seeded("panel-esc");
    let home = node_id(&root, "Home");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 140, 40);
    s.keys(&["down", "o", "a-s"]);
    let panel = format!("sidebar_today_panel-page-{home}");
    assert_eq!(s.state().focus.as_deref(), Some(panel.as_str()));
    s.keys(&["d-up", "s-right", "s-right"]);
    let sel = serde_json::to_value(&fictty::state::field_view(s.state(), &panel).unwrap().1.selection).unwrap();
    assert_ne!(sel["ranges"][0]["anchor"], sel["ranges"][0]["head"], "a selection");
    s.keys(&["esc"]);
    assert_eq!(s.state().focus.as_deref(), Some(panel.as_str()), "still in the panel");
    let after = serde_json::to_value(&fictty::state::field_view(s.state(), &panel).unwrap().1.selection).unwrap();
    assert_eq!(after["ranges"][0]["anchor"], after["ranges"][0]["head"], "collapsed");
    s.keys(&["esc"]);
    assert_eq!(s.state().focus.as_deref(), Some("today"), "back to main");
    let _ = std::fs::remove_dir_all(root);
}

/// §12: a click on a panel's header folds it (and again unfolds it).
#[test]
fn a_click_on_a_panels_header_folds_it() {
    let (root, v) = seeded("head-click");
    let home = node_id(&root, "Home");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 140, 40);
    s.keys(&["down", "o"]);
    let head = format!("sidebar_today_head-page-{home}");
    s.msg(fictty::state::Msg::Click { id: head.clone(), row: Some(0), tab: None });
    assert!(s.screen().contains("▸ ¶ Home") && s.screen().contains("2 open"), "{}", s.screen());
    s.msg(fictty::state::Msg::Click { id: head, row: Some(0), tab: None });
    assert!(s.screen().contains("▾ ¶ Home"), "{}", s.screen());
    let _ = std::fs::remove_dir_all(root);
}
