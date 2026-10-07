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
    keys(&mut s, "<c-o>Plan<cr>");
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
