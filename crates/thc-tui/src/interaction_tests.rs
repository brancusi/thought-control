//! The writing experience under the pointer (interaction.md): links open beside without moving
//! the page or its caret, a click on a link is one step, a click after the wheel lands where
//! the pointer is, and the sidebar opening or closing leaves the caret's row where it was.
//! Driven through the session with key scripts, as `thc ui send keys` does.

use crate::session::Session;
use thc_core::builder::TxBuilder;
use thc_core::vault::Vault;

/// A page "Plan" with a long paragraph, links and enough notes to scroll; pages "Garden" and
/// "Kitchen" to link to.
fn vault(tag: &str) -> (crate::fuzz::Scratch, Vault) {
    let (s, mut vault) = crate::fuzz::scratch(tag);
    let today = thc_core::dates::today();
    vault
        .transact(|st| {
            let mut b = TxBuilder::new(st, today);
            for (title, body) in [("Garden", "water the beds"), ("Kitchen", "fix the tap")] {
                let p = b.create_page(title, &[])?;
                b.create_from_capture(Some(p), &thc_core::capture::parse(body, today)?, None)?;
            }
            let plan = b.create_page("Plan", &[])?;
            let mut lines = vec![
                "An opening paragraph long enough to wrap over several rows in a narrow column, so a change of width moves the rows under it, the quick brown fox jumps over the lazy dog again.".to_string(),
                "See [[Garden]] and [[Kitchen]] for the rest.".to_string(),
            ];
            lines.extend((0..60).map(|i| format!("note {i} with a few words in it")));
            for t in &lines {
                b.create_from_capture(Some(plan.clone()), &thc_core::capture::parse(t, today)?, None)?;
            }
            Ok((b.finish(), ()))
        })
        .unwrap();
    (s, vault)
}

fn session(vault: Vault, size: (u16, u16)) -> Session {
    crate::SNAPSHOT.with(|s| s.set(true));
    let mut app = crate::app::App::new(vault).unwrap();
    app.daemon_live = false;
    Session::new(app, size)
}

fn keys(s: &mut Session, script: &str) {
    for m in crate::script::parse(script, false).unwrap() {
        s.apply(m).unwrap();
        s.drop_effects();
    }
}

/// The frame as rows, and the caret's cell.
fn frame(s: &mut Session) -> (Vec<String>, Option<[u16; 2]>) {
    let (w, h) = s.size;
    let r = s.render(w, h, "text").unwrap();
    (r.frame.unwrap().lines().map(str::to_string).collect(), r.cursor)
}

/// Where `text` first shows (column, row), in cells (the fixture is ASCII).
fn find(rows: &[String], text: &str) -> (u16, u16) {
    rows.iter().enumerate().find_map(|(y, r)| r.find(text).map(|x| (r[..x].chars().count() as u16, y as u16))).unwrap_or_else(|| panic!("{text:?} not on screen:\n{}", rows.join("\n")))
}

fn open_plan(size: (u16, u16), tag: &str) -> (crate::fuzz::Scratch, Session) {
    let (scratch, v) = vault(tag);
    let mut s = session(v, size);
    keys(&mut s, "<c-o>Plan<cr><c-home>");
    (scratch, s)
}

fn panel_open(s: &Session, title: &str) -> bool {
    s.app.ui.sidebar.open.iter().any(|p| s.app.panel_name(&p.key()).contains(title))
}

#[test]
fn a_link_opens_beside_with_shift_ctrl_cmd_or_middle_and_the_caret_stays() {
    for click in ["sclick", "cclick", "cmdclick", "mclick"] {
        let (_s, mut s) = open_plan((120, 32), &format!("beside-{click}"));
        let (rows, _) = frame(&mut s);
        let (x, y) = find(&rows, "note 3 with");
        keys(&mut s, &format!("<click:{},{y}>", x + 2));
        let caret = s.app.doc.as_ref().unwrap().caret();
        let (rows, _) = frame(&mut s);
        let (x, y) = find(&rows, "[[Garden]]");
        keys(&mut s, &format!("<{click}:{},{y}>", x + 4));
        assert!(panel_open(&s, "Garden"), "{click}: Garden opens beside");
        let d = s.app.doc.as_ref().unwrap();
        assert!(matches!(&d.target, crate::editor::Target::Page { title, .. } if title == "Plan"), "{click}: the main view stays on Plan");
        assert_eq!(d.caret(), caret, "{click}: the caret stays where it was");
    }
}

/// The ⌘-release WezTerm sends (thc_keys.lua): a plain press, then the release with ⌘.
#[test]
fn a_cmd_release_after_a_plain_press_on_a_link_opens_it_beside() {
    let (_s, mut s) = open_plan((120, 32), "cmd-release");
    let (rows, _) = frame(&mut s);
    let caret = s.app.doc.as_ref().unwrap().caret();
    let (x, y) = find(&rows, "[[Kitchen]]");
    let press = serde_json::json!({"msg": "mouse", "kind": "down", "x": x + 4, "y": y});
    let release = serde_json::json!({"msg": "mouse", "kind": "up", "x": x + 4, "y": y, "mods": "d"});
    for m in [press, release] {
        s.apply(serde_json::from_value(m).unwrap()).unwrap();
    }
    assert!(panel_open(&s, "Kitchen"));
    assert_eq!(s.app.doc.as_ref().unwrap().caret(), caret);
}

/// A plain click follows the link, and nothing changes on the press: the caret never shows
/// inside the link for a frame before the page opens.
#[test]
fn a_plain_click_on_a_link_follows_it_and_the_press_draws_nothing() {
    let (_s, mut s) = open_plan((120, 32), "follow");
    let (before, cursor) = frame(&mut s);
    let (x, y) = find(&before, "[[Garden]]");
    let press = serde_json::json!({"msg": "mouse", "kind": "down", "x": x + 4, "y": y});
    s.apply(serde_json::from_value(press).unwrap()).unwrap();
    assert_eq!(frame(&mut s), (before, cursor), "the press changes nothing on screen");
    let release = serde_json::json!({"msg": "mouse", "kind": "up", "x": x + 4, "y": y});
    s.apply(serde_json::from_value(release).unwrap()).unwrap();
    assert!(matches!(&s.app.doc.as_ref().unwrap().target, crate::editor::Target::Page { title, .. } if title == "Garden"));
    // A drag from a link's title selects instead.
    let (_s2, mut s) = open_plan((120, 32), "follow-drag");
    let (rows, _) = frame(&mut s);
    let (x, y) = find(&rows, "[[Garden]]");
    keys(&mut s, &format!("<drag:{},{y},{},{y}>", x + 4, x + 14));
    let d = s.app.doc.as_ref().unwrap();
    assert!(matches!(&d.target, crate::editor::Target::Page { title, .. } if title == "Plan"));
    assert!(d.selection().is_some(), "dragged from the link: a selection");
}

/// After the wheel scrolls the view away from the caret, a click puts the caret under the
/// pointer and the view doesn't move.
#[test]
fn a_click_after_the_wheel_lands_where_the_pointer_is() {
    let (_s, mut s) = open_plan((120, 32), "wheel-click");
    frame(&mut s);
    keys(&mut s, "<wheel:down:20@60,15>");
    let (before, _) = frame(&mut s);
    let (x, y) = find(&before, "note 30 with");
    keys(&mut s, &format!("<click:{},{y}>", x + 2));
    let (after, cursor) = frame(&mut s);
    assert_eq!(before[2..30], after[2..30], "the view didn't move");
    assert_eq!(cursor, Some([x + 2, y]), "the caret is where the pointer clicked");
}

/// A click near the bottom edge doesn't scroll the view either.
#[test]
fn a_click_near_the_edge_doesnt_scroll() {
    let (_s, mut s) = open_plan((120, 32), "edge-click");
    let (before, _) = frame(&mut s);
    keys(&mut s, "<click:40,29>");
    let (after, cursor) = frame(&mut s);
    assert_eq!(before[2..30], after[2..30]);
    assert_eq!(cursor.map(|c| c[1]), Some(29));
}

/// The sidebar opening and closing: the page's title stays on its row (the crumb shares it
/// when the rail goes), the text starts on the same row, and the caret keeps its row where the
/// page can scroll to keep it.
#[test]
fn opening_and_closing_the_sidebar_keeps_the_caret_row() {
    for size in [(120u16, 32u16), (200, 50)] {
        let (_s, mut s) = open_plan(size, &format!("pin-{}", size.0));
        frame(&mut s);
        keys(&mut s, "<wheel:down:6@60,15>");
        let (rows, _) = frame(&mut s);
        let (x, y) = find(&rows, "note 20 with");
        keys(&mut s, &format!("<click:{},{y}>", x + 2));
        let (rows, before) = frame(&mut s);
        let title_row = find(&rows, "Plan").1;
        // The main view's caret, drawn or not (the keyboard goes to the panel :aside opens).
        let row = |s: &Session| s.app.caret_pin.as_ref().map(|p| p.row());
        assert_eq!(row(&s), before.map(|c| c[1]));
        // Open Garden beside with ⌥O's sibling, the palette: a link on screen may be scrolled off.
        keys(&mut s, "<m-:>aside Garden<cr>");
        assert!(panel_open(&s, "Garden"), "{size:?}: the panel opened");
        let (rows, _) = frame(&mut s);
        assert_eq!(before.map(|c| c[1]), row(&s), "{size:?}: the caret's row with the sidebar open\n{}", rows.join("\n"));
        assert_eq!(find(&rows, "Plan").1, title_row, "{size:?}: the title's row");
        keys(&mut s, "<m-:>sidebar close all<cr>");
        let (rows, _) = frame(&mut s);
        assert_eq!(before.map(|c| c[1]), row(&s), "{size:?}: the caret's row after it closes\n{}", rows.join("\n"));
    }
}

/// At 200 columns the text column keeps its width with the sidebar open: the detail pane it
/// replaces doesn't still take room.
#[test]
fn the_text_column_doesnt_reserve_room_for_a_yielded_detail_pane() {
    let (_s, mut s) = open_plan((200, 50), "detail-yields");
    let (rows, _) = frame(&mut s);
    // The paragraph's first row, as wrapped.
    let width = |rows: &[String]| {
        let (_, y) = find(rows, "An opening paragraph");
        let r = &rows[y as usize];
        let from = r.find("An opening").unwrap();
        r[from..].split("  ").next().unwrap().to_string()
    };
    let w0 = width(&rows);
    keys(&mut s, "<m-:>aside Garden<cr>");
    let (rows, _) = frame(&mut s);
    assert!(panel_open(&s, "Garden"));
    assert_eq!(width(&rows), w0, "the paragraph wraps at the same width\n{}", rows.join("\n"));
}

/// A wide character that doesn't fit whole at the end of a row starts the next row: a 72-cell
/// column never draws a 73-cell row (hyrg2: the engine's wrap takes it when its first cell
/// fits, and its right half lands on the scrollbar or the meta).
#[test]
fn a_wide_character_that_doesnt_fit_wraps_whole() {
    use crate::editor::{Doc, DocRow, Target, ViewGeometry};
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 7).unwrap();
    for (depth, pad, column) in [(0usize, 71usize, 72usize), (1, 67, 68)] {
        let text = format!("{}🙂 tail words here", "a".repeat(pad));
        let b: thc_core::outline::Block = serde_json::from_value(serde_json::json!({"id": "n0", "parent": null, "depth": depth, "kind": "bullet", "text": text, "text_rev": "r"})).unwrap();
        let mut d = Doc::new(Target::Journal { date: today }, Some("root".into()), &[b], today);
        d.set_view(&ViewGeometry { width: 100, height: 10, column: 72, extra_rows: vec![], typewriter: false });
        for r in d.frame().rows {
            if let DocRow::Text { start, end, .. } = r {
                let w = unicode_width::UnicodeWidthStr::width(text[start..end].trim_end());
                assert!(w <= column, "depth {depth}: a {w}-cell row in a {column}-cell column: {:?}", &text[start..end]);
            }
        }
    }
}

/// 02pjq: Enter shows what the saved page will show: on an empty paragraph it does nothing.
#[test]
fn enter_makes_only_what_a_save_keeps() {
    use crate::editor::{BlockPos, Doc, Target};
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 7).unwrap();
    let blk = |id: &str, kind: &str, text: &str| -> thc_core::outline::Block { serde_json::from_value(serde_json::json!({"id": id, "parent": null, "depth": 0, "kind": kind, "text": text, "text_rev": "r"})).unwrap() };
    let texts = |d: &Doc| d.blocks().iter().map(|l| l.text.clone()).collect::<Vec<_>>();
    // An empty paragraph (a journal's fresh line): Enter makes nothing.
    let mut d = Doc::new(Target::Journal { date: today }, Some("root".into()), &[blk("a", "para", "notes"), blk("b", "para", "")], today);
    d.fill_ids((0..20).map(|i| format!("id{i:03}")).collect());
    d.set_caret(BlockPos { line: 1, byte: 0 });
    d.run_command("edit.newline");
    assert_eq!(texts(&d), ["notes", ""]);
    // A split: the rest starts with no spaces, and one undo brings it all back (0d61e).
    let mut d = Doc::new(Target::Journal { date: today }, Some("root".into()), &[blk("a", "bullet", "Last line of the plan")], today);
    d.fill_ids((0..20).map(|i| format!("id{i:03}")).collect());
    d.set_caret(BlockPos { line: 0, byte: 9 });
    d.run_command("edit.newline");
    assert_eq!(texts(&d), ["Last line", "of the plan"]);
    assert_eq!(d.caret(), BlockPos { line: 1, byte: 0 });
    d.run_command("history.undo");
    assert_eq!(texts(&d), ["Last line of the plan"]);
    assert_eq!((d.caret(), d.selection()), (BlockPos { line: 0, byte: 9 }, None));
}

/// A saved page of `n` notes (each saved: its parent and the note it follows recorded).
fn saved_page(n: usize) -> crate::editor::Doc {
    use crate::editor::{Doc, Target};
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 7).unwrap();
    let blocks: Vec<thc_core::outline::Block> = (0..n)
        .map(|i| serde_json::from_value(serde_json::json!({"id": format!("s{i:05}"), "parent": null, "depth": 0, "kind": "bullet", "text": format!("saved {i}"), "text_rev": "r"})).unwrap())
        .collect();
    let mut d = Doc::new(Target::Page { id: "root".into(), title: "Big".into() }, Some("root".into()), &blocks, today);
    d.fill_ids((0..5000).map(|i| format!("n{i:05}")).collect());
    d
}

fn op_counts(ops: &[thc_core::outline::BlockOp]) -> (usize, usize) {
    use thc_core::outline::BlockOp;
    (ops.iter().filter(|o| matches!(o, BlockOp::Create { .. })).count(), ops.iter().filter(|o| matches!(o, BlockOp::Move { .. })).count())
}

/// ymh1g: a paste of 1,000 lines into a 5,000-note page creates 1,000 notes and moves next to
/// none: the notes below it keep their order in the vault (it moved all 5,000 of them).
#[test]
fn a_paste_moves_no_note_it_didnt_move() {
    use crate::editor::BlockPos;
    let paste: String = (0..1000).map(|i| format!("- pasted {i}")).collect::<Vec<_>>().join("\n");
    for at in [0usize, 2500, 4999] {
        let mut d = saved_page(5000);
        d.set_caret(BlockPos { line: at, byte: d.blocks()[at].text.len() });
        d.paste(&format!("\n{paste}"), false);
        let plan = d.plan_save(true);
        let (creates, moves) = op_counts(&plan.ops);
        assert!((998..=1001).contains(&creates), "at {at}: {creates} creates");
        assert!(moves <= 2, "at {at}: {moves} moves");
    }
}

/// After pastes in the middle (two of them) and a save, the vault's order of the page's notes is
/// the document's: the save moved only what moved, and every note is where it shows.
#[test]
fn after_pastes_the_vault_order_is_the_documents() {
    let (_s, mut s) = open_plan((120, 32), "paste-order");
    let block = |tag: &str, n: usize| (0..n).map(|i| format!("- {tag} {i}")).collect::<Vec<_>>().join("\\n");
    keys(&mut s, &format!("{}<end><cr><paste:{}>", "<down>".repeat(20), block("first", 30)));
    keys(&mut s, &format!("{}<end><cr><paste:{}>", "<down>".repeat(12), block("second", 20)));
    s.apply(serde_json::from_value(serde_json::json!({"msg": "focus", "gained": false})).unwrap()).unwrap();
    let d = s.app.doc.as_ref().unwrap();
    let root = d.root.clone().unwrap();
    let doc: Vec<String> = d.blocks().iter().map(|l| l.text.clone()).filter(|t| !t.trim().is_empty()).collect();
    let vault: Vec<String> = thc_core::outline::render_for_editor(&s.app.vault.store, &root).unwrap().into_iter().map(|b| b.text).filter(|t| !t.trim().is_empty()).collect();
    assert_eq!(vault, doc);
    assert!(doc.iter().any(|t| t == "first 29") && doc.iter().any(|t| t == "second 0"));
    // Nothing left to send: the notes the pastes landed above know what they follow now (the
    // next save used to move every one of them).
    let ops = s.app.doc.as_mut().unwrap().plan_save(true).ops;
    assert!(ops.is_empty(), "{ops:?}");
}

/// mgmm8: on a page that fits the screen at 120 columns, a panel opening beside it changes no
/// text row: the rail gives way, but the column keeps the width it had (it widened 66 → 72 and
/// the paragraph reflowed, moving the caret's line).
#[test]
fn a_panel_opening_beside_a_short_page_reflows_nothing() {
    let (_s, mut s) = open_plan((120, 32), "short-reflow");
    // A short page: Garden, with a long note.
    keys(&mut s, "<c-o>Garden<cr><c-home><end> and a long tail of words so that this note wraps over more than one row at sixty or seventy columns of text");
    let (before, b) = frame(&mut s);
    // The note's rows, as wrapped: from "water the beds" on, the text before any panel.
    let wrapped = |rows: &[String]| {
        let (_, y) = find(rows, "water the beds");
        rows[y as usize..y as usize + 2].iter().map(|r| {
            let r = r.split('│').find(|p| p.contains("water") || p.contains("than one")).unwrap_or(r);
            r.trim().trim_start_matches('·').trim().to_string()
        }).collect::<Vec<_>>()
    };
    keys(&mut s, "<m-:>aside Kitchen<cr><m-s>");
    assert!(panel_open(&s, "Kitchen"));
    let (after, _) = frame(&mut s);
    let row = s.app.caret_pin.as_ref().map(|p| p.row());
    assert_eq!(b.map(|c| c[1]), row, "the caret's row");
    assert_eq!(wrapped(&before), wrapped(&after), "the note wraps the same\n{}\n---\n{}", before.join("\n"), after.join("\n"));
}

/// 5jzx9: a space typed at a row's end hangs in the margin, and the caret after it stays on that
/// row: no row of its own (the save drops the space, so a reopened page was a row shorter).
#[test]
fn the_caret_after_a_space_that_ends_a_row_stays_on_the_row() {
    use crate::editor::{BlockPos, Doc, DocRow, Target, ViewGeometry};
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 7).unwrap();
    let text = format!("{} ", "a".repeat(72));
    let b: thc_core::outline::Block = serde_json::from_value(serde_json::json!({"id": "n0", "parent": null, "depth": 0, "kind": "bullet", "text": text, "text_rev": "r"})).unwrap();
    let mut d = Doc::new(Target::Journal { date: today }, Some("root".into()), &[b], today);
    d.set_caret(BlockPos { line: 0, byte: text.len() });
    d.set_view(&ViewGeometry { width: 100, height: 10, column: 72, extra_rows: vec![], typewriter: false });
    let f = d.frame();
    let rows = f.rows.iter().filter(|r| matches!(r, DocRow::Text { .. })).count();
    assert_eq!(rows, 1, "one row, the space and the caret in its margin");
    assert_eq!(f.cursor.map(|c| c.1), Some(0));
}

/// The meta never runs under the scrollbar: its column is kept whether or not one shows (the
/// caret's `↗ open` chip read `↗ ope` on a long page at 120 columns beside the rail).
#[test]
fn the_meta_keeps_clear_of_the_scrollbar() {
    for size in [(120u16, 32u16), (100, 30), (200, 50)] {
        let (_s, mut s) = open_plan(size, &format!("meta-bar-{}", size.0));
        let (rows, _) = frame(&mut s);
        let (x, y) = find(&rows, "[[Garden]]");
        keys(&mut s, &format!("<aclick:{},{y}>", x + 4));
        let (rows, _) = frame(&mut s);
        let row = &rows[y as usize];
        assert!(row.contains("↗ open"), "{size:?}: the chip whole: {row}");
        let last: String = row.chars().last().into_iter().collect();
        assert!(matches!(last.as_str(), "│" | "┃" | " "), "{size:?}: the scrollbar's column: {row}");
    }
}

/// The calendar beside a day counts in English: `1 entry · 1 task`, not `1 entries · 1 tasks`.
#[test]
fn the_day_summary_says_one_entry() {
    let (_scratch, v) = vault("one-entry");
    let mut s = session(v, (200, 50));
    keys(&mut s, "5[ ] call the plumber<cr><esc>5");
    let (rows, _) = frame(&mut s);
    let line = rows.iter().find(|r| r.contains(" done")).cloned().unwrap_or_default();
    assert!(line.contains("1 entry ·") && line.contains("1 task ·"), "{line}");
}

/// Just arrived on a page (parked, navigation.md §6.1), the bar says how to start and how to
/// move on; the first key that writes puts the writing keys back. Narrow, those go first.
#[test]
fn the_bar_says_type_to_write_on_arrival() {
    let (_v, v) = vault("parked-bar");
    let mut s = session(v, (120, 32));
    keys(&mut s, "<c-o>Plan<cr>");
    let bar = |s: &mut Session| frame(s).0.last().cloned().unwrap_or_default();
    let b = bar(&mut s);
    assert!(b.contains("type to write") && b.contains("Tab next view") && b.contains("⌃T task"), "{b}");
    keys(&mut s, "x");
    let b = bar(&mut s);
    assert!(!b.contains("type to write") && b.contains("⌃T task"), "{b}");
    let (_v2, v) = vault("parked-bar-80");
    let mut s = session(v, (80, 24));
    keys(&mut s, "<c-o>Plan<cr>");
    let b = bar(&mut s);
    assert!(b.contains("⌃T task") && !b.contains("type to write"), "{b}");
}

/// The `[[` popup ranks what you'd pick: a title starting with what's typed first, then a word
/// in it, then anywhere; with nothing typed, the pages you've been on; never the page you're on.
#[test]
fn the_link_popup_ranks_by_how_the_title_matches() {
    let (_scratch, mut v) = vault("link-rank");
    let today = thc_core::dates::today();
    v.transact(|st| {
        let mut b = TxBuilder::new(st, today);
        for t in ["Ungarden", "Rose garden", "Gardening tips"] {
            b.create_page(t, &[])?;
        }
        Ok((b.finish(), ()))
    })
    .unwrap();
    let mut s = session(v, (120, 32));
    keys(&mut s, "<c-o>Kitchen<cr><c-o>Plan<cr>");
    let titles = |s: &Session, q: &str| s.app.link_matches(q).0.into_iter().map(|(_, t)| t).collect::<Vec<_>>();
    let g = titles(&s, "gard");
    let mut first: Vec<String> = g[..2].to_vec();
    first.sort();
    assert_eq!(first, ["Garden", "Gardening tips"], "titles starting so first: {g:?}");
    assert_eq!(g[2], "Rose garden", "{g:?}");
    assert_eq!(g[3], "Ungarden", "{g:?}");
    let empty = titles(&s, "");
    assert_eq!(empty.first().map(String::as_str), Some("Kitchen"), "the page you came from first: {empty:?}");
    assert!(!empty.contains(&"Plan".to_string()), "never the page you're on: {empty:?}");
}

/// caretline 3743263 (host edits as the least change): leaving the empty line a page arrives
/// with, and the save that follows, keep every other note's id and write nothing: no create,
/// delete or move (dropping that one line used to touch every block's mark).
#[test]
fn leaving_the_arrival_line_changes_no_other_note() {
    let (_scratch, v) = vault("arrival-ids");
    let mut s = session(v, (120, 32));
    keys(&mut s, "<c-o>Plan<cr>");
    let ids = |s: &Session| s.app.doc.as_ref().unwrap().blocks().iter().filter(|l| !l.is_new).map(|l| l.id.clone()).collect::<Vec<_>>();
    let events = |s: &Session| -> i64 { s.app.vault.store.conn.query_row("SELECT count(*) FROM events", [], |r| r.get(0)).unwrap() };
    let before = (ids(&s), events(&s));
    keys(&mut s, "<c-home><down><down>");
    s.apply(serde_json::from_value(serde_json::json!({"msg": "focus", "gained": false})).unwrap()).unwrap();
    assert_eq!(ids(&s), before.0, "every note keeps its id");
    assert_eq!(events(&s), before.1, "nothing written");
    assert!(s.app.doc.as_mut().unwrap().plan_save(true).ops.is_empty());
}
