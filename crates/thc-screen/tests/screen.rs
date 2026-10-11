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
    // The detail pane's lines: title, where, fields, subtasks.
    let lines = row(1, 0)["lines"].clone();
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
    assert!(home["md"].as_str().unwrap().contains("- [ ] Water plants"), "{}", home["md"]);
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
    assert!(log["rows"].as_array().unwrap().iter().any(|r| r["who_m"].as_str().is_some_and(|w| w.contains("◆ claude"))), "{log:#}");
    // The frame: the inbox count in the tabs, the accent under the lit one.
    let frame = host.feed("frame", &json!({"tab": 1})).unwrap();
    assert!(frame["narrow"].as_str().unwrap().contains("<text+b>Inbox 1</>"), "{frame:#}");
    assert_eq!(frame["rule_narrow"], format!("<line>{}</><accent>{}</>", "─".repeat(14), "━".repeat(9)));
    assert!(host.feed("nope", &json!({})).is_err());
    let _ = std::fs::remove_dir_all(root);
}

fn screen(host: &ThcHost, view: View, w: u16, h: u16) -> fictty::Headless {
    fictty::App::new(host.screen(&Route::to(view))).themes(thc_screen::theme::themes()).theme("ember-dark").host(host.clone()).headless(w, h).unwrap()
}

#[test]
fn x_on_a_today_row_marks_it_done_in_the_vault() {
    let (root, v) = seeded("done");
    let host = ThcHost::new(v).at(now());
    let mut s = screen(&host, View::Today, 120, 32);
    let frame = s.screen();
    assert!(frame.contains("Today   Inbox 1   Tasks"), "{frame}");
    assert!(frame.contains("[ ] Call the bank"), "{frame}");
    assert!(frame.contains("│ [ ] Call the bank") || frame.contains("Overdue  1"), "{frame}");
    // The cursor starts on the Overdue section; j is its first row.
    s.keys(&["j", "x"]);
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
    // The rule lights the new tab: one cell either side of `Inbox 1` (tabs from column 7).
    assert!(inbox.lines().nth(1).unwrap().starts_with(&format!("{}{}─", "─".repeat(14), "━".repeat(9))), "{inbox}");
    s.keys(&["4"]);
    assert!(s.screen().contains("¶ Home"), "pages");
    let mut wide = screen(&host, View::Inbox, 120, 32);
    let frame = wide.screen();
    assert!(frame.contains("┬"), "the rule joins the split: {frame}");
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
    // On screen, a failed action says why on the status line.
    let mut s = screen(&host, View::Detail, 80, 24);
    s.keys(&["x"]);
    assert!(s.state().status.as_deref().is_some_and(|st| st.contains("done needs an id")), "{:?}", s.state().status);
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
