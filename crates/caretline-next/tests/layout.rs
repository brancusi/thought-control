//! The outline layout: markers in a hang, a column and width per depth, virtual rows, folds,
//! row info and hit-testing.

use caretline_next::helix::Selection;
use caretline_next::outline::markdown;
use caretline_next::view::{hit, render, Hit, Role, RowInfo};
use caretline_next::{update, update_doc, view, By, Dir, MarkId, Msg, OutlineConfig, OutlineLayout, State, View, Viewport};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

const TRIP: &str = "# Lisbon trip\n\nBooked the flat in Lisbon.\nIt faces the river.\n\n- [ ] Pay the deposit\n  - ask Ana about her desk\n- [ ] Book flights\n- [x] Renew passport\n\n![boiler label](files/boiler.png)\n\n1. Pack\n2. Leave\n";

fn laid_out(md: &str, w: u16, h: u16) -> State {
    let mut s = markdown::load(md, None, Viewport { width: w, height: h }, OutlineConfig::default());
    s.view.config.status_bar = false;
    s.view.layout = Some(OutlineLayout { hang_glyphs: true, ..Default::default() });
    update(&mut s, Msg::Resize { width: w, height: h });
    s
}

fn ids(s: &State) -> Vec<MarkId> {
    s.doc.marks.iter().map(|m| m.id).collect()
}

#[test]
fn markers_move_to_the_hang_and_depths_get_columns() {
    let s = laid_out(TRIP, 50, 16);
    let f = view(&s);
    assert_eq!(
        f.to_text(),
        "  #   Lisbon trip\n\n      Booked the flat in Lisbon.\n      It faces the river.\n\n  [ ] Pay the deposit\n      •   ask Ana about her desk\n  [ ] Book flights\n  [x] Renew passport\n\n      ![boiler label](files/boiler.png)\n\n  1.  Pack\n  2.  Leave\n\n\n"
    );
    // The hang's cells say so, and text cells carry their char.
    assert_eq!(f.cell(2, 5).role, Role::Hang);
    assert_eq!(f.cell(1, 5).role, Role::Text, "the marks column is the host's, blank");
    let pay = s.doc.text.line_to_char(3) + 6;
    assert_eq!(f.cell(6, 5).char_idx, Some(pay as u32));
    assert_eq!(f.cell(6, 5).symbol.as_str(), "P");
    assert_eq!(f.cell(5, 5).char_idx, None);
    // The caret starts at the heading's content.
    assert_eq!(f.cursor, Some((6, 0)));
}

#[test]
fn row_info_names_every_row() {
    let s = laid_out(TRIP, 30, 20);
    let f = view(&s);
    let ids = ids(&s);
    assert_eq!(f.rows.len(), 20);
    assert!(matches!(f.rows[0], RowInfo::Text { line: 0, row: 0, first: true, last: true, x: 6, .. }));
    assert_eq!(f.rows[1], RowInfo::Gap { before: ids[1] });
    // "Booked the flat in Lisbon." wraps at 24 columns: two rows of one line.
    match (&f.rows[2], &f.rows[3]) {
        (RowInfo::Text { block, line: 1, row: 0, first: true, last: false, chars, .. }, RowInfo::Text { line: 1, row: 1, first: false, last: false, chars: c2, .. }) => {
            assert_eq!(*block, Some(ids[1]));
            assert_eq!(s.doc.text.slice(chars.clone()).to_string(), "Booked the flat in ");
            assert_eq!(s.doc.text.slice(c2.clone()).to_string(), "Lisbon.");
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(f.rows[4], RowInfo::Text { line: 2, last: true, .. }));
    assert!(matches!(f.rows[19], RowInfo::Past));
}

#[test]
fn extra_rows_follow_a_block_and_are_never_caret_stops() {
    let mut s = laid_out("- one\n- two\n- three\n", 40, 10);
    let ids = ids(&s);
    s.view.layout.as_mut().unwrap().extra_rows.insert(ids[0], 2);
    let f = view(&s);
    assert_eq!(f.rows[1], RowInfo::Extra { block: ids[0], index: 0 });
    assert_eq!(f.rows[2], RowInfo::Extra { block: ids[0], index: 1 });
    assert!(matches!(f.rows[3], RowInfo::Text { line: 1, .. }));
    update(&mut s, Msg::Move { dir: Dir::Forward, by: By::VisualLine, extend: false });
    assert_eq!(s.caret(), s.doc.text.line_to_char(1) + 2, "↓ steps over the extra rows");
    update(&mut s, Msg::Move { dir: Dir::Backward, by: By::VisualLine, extend: false });
    assert_eq!(s.caret(), 2, "↑ too");
    assert_eq!(hit(&s.doc, &s.view, 10, 2), Hit::Extra { block: ids[0], index: 1 });
}

#[test]
fn hit_tells_hang_marks_gap_and_text() {
    let s = laid_out(TRIP, 50, 16);
    let ids = ids(&s);
    assert_eq!(hit(&s.doc, &s.view, 0, 5), Hit::Marks { block: ids[2] });
    assert_eq!(hit(&s.doc, &s.view, 3, 5), Hit::Hang { block: ids[2] });
    assert_eq!(hit(&s.doc, &s.view, 9, 5), Hit::Text { pos: s.doc.text.line_to_char(3) + 9 });
    assert_eq!(hit(&s.doc, &s.view, 9, 1), Hit::Gap { block: ids[1] });
    assert_eq!(hit(&s.doc, &s.view, 9, 15), Hit::Past);
    // A click in the hang puts the caret at the content's start.
    let mut c = s.clone();
    update(&mut c, Msg::Click { col: 3, row: 6, extend: false });
    assert_eq!(c.caret(), c.doc.text.line_to_char(4) + 4);
}

#[test]
fn vertical_motion_keeps_the_screen_column_across_depths() {
    let mut s = laid_out("- [ ] alpha beta\n  - nested text here\n- gamma delta\n", 60, 10);
    s.view.selection = Selection::point(6 + 8); // "alpha be|ta", screen x 14
    update(&mut s, Msg::Move { dir: Dir::Forward, by: By::VisualLine, extend: false });
    let f = view(&s);
    assert_eq!(f.cursor, Some((14, 1)));
    update(&mut s, Msg::Move { dir: Dir::Forward, by: By::VisualLine, extend: false });
    assert_eq!(view(&s).cursor, Some((14, 2)));
}

#[test]
fn folds_belong_to_the_view() {
    let s = laid_out(TRIP, 50, 16);
    let ids = ids(&s);
    let pay = ids[2];
    let mut doc = s.doc.clone();
    let mut views = [s.view.clone(), s.view.clone()];
    // The caret is in the child; folding its parent moves it to the parent's end.
    views[0].selection = Selection::point(doc.text.line_to_char(4) + 6);
    update_doc(&mut doc, &mut views, 0, Msg::Fold { id: pay });
    assert_eq!(views[0].caret(), doc.text.line_to_char(3) + "- [ ] Pay the deposit".len());
    let a = render(&doc, &views[0]).to_text();
    let b = render(&doc, &views[1]).to_text();
    assert!(!a.contains("ask Ana"), "view 0 hides the child:\n{a}");
    assert!(b.contains("ask Ana"), "view 1, on the same document, shows it");
    // ↓ from the folded block skips its children.
    update_doc(&mut doc, &mut views, 0, Msg::Move { dir: Dir::Forward, by: By::VisualLine, extend: false });
    assert_eq!(doc.text.char_to_line(views[0].caret()), 5);
    // → at its end goes past them too.
    views[0].selection = Selection::point(doc.text.line_to_char(3) + 21);
    update_doc(&mut doc, &mut views, 0, Msg::Move { dir: Dir::Forward, by: By::Grapheme, extend: false });
    assert_eq!(views[0].caret(), doc.text.line_to_char(5) + 6);
    // A childless block doesn't fold; toggling unfolds.
    update_doc(&mut doc, &mut views, 0, Msg::Fold { id: ids[4] });
    assert_eq!(views[0].status.as_deref(), Some("nothing to fold: no children"));
    update_doc(&mut doc, &mut views, 0, Msg::ToggleFold { id: pay });
    assert!(views[0].folds.is_empty());
    // A fold goes with its block.
    update_doc(&mut doc, &mut views, 0, Msg::Fold { id: pay });
    views[1].selection = Selection::point(doc.text.line_to_char(3) + 6);
    update_doc(&mut doc, &mut views, 1, Msg::Move { dir: Dir::Forward, by: By::LineEnd, extend: true });
    update_doc(&mut doc, &mut views, 1, Msg::Move { dir: Dir::Backward, by: By::Grapheme, extend: false });
    views[1].selection = Selection::single(doc.text.line_to_char(2), doc.text.line_to_char(5));
    update_doc(&mut doc, &mut views, 1, Msg::DeleteBackward);
    assert!(!doc.marks.contains(pay));
    assert!(views[0].folds.is_empty(), "the fold dropped with its block");
}

#[test]
fn folds_hide_rows_without_a_layout_too() {
    let mut s = markdown::load("- a\n  - b\n  - c\n- d\n", None, Viewport { width: 20, height: 6 }, OutlineConfig::default());
    let a = ids(&s)[0];
    update(&mut s, Msg::Fold { id: a });
    assert_eq!(view(&s).to_text().lines().take(2).collect::<Vec<_>>(), ["- a", "- d"]);
    let back = State::from_json(&s.to_json()).unwrap();
    assert_eq!(back.view.folds, s.view.folds, "folds survive JSON");
}

#[test]
fn a_long_fence_line_scrolls_sideways_on_its_own() {
    let md = "intro\n\n```\nlet a_very_long_line_of_code = 1234567890 + 1234567890;\n```\n";
    let mut s = laid_out(md, 30, 8);
    let line = 2;
    s.view.selection = Selection::point(s.doc.text.line_to_char(line));
    update(&mut s, Msg::Move { dir: Dir::Forward, by: By::LineEnd, extend: false });
    let f = view(&s);
    let (x, y) = f.cursor.expect("the caret is on screen");
    assert!(x < 30, "{x}");
    let row: String = (0..30).map(|c| f.cell(c, y).symbol.to_string()).collect();
    assert!(row.contains("1234567890;"), "{row:?}");
    // Other lines don't move.
    assert!(f.to_text().starts_with("      intro"));
}

#[test]
fn a_view_lays_out_into_any_size() {
    let s0 = laid_out(TRIP, 50, 16);
    for (w, h) in [(1u16, 1u16), (8, 3), (12, 4), (40, 10), (200, 60)] {
        let mut s = s0.clone();
        update(&mut s, Msg::Resize { width: w, height: h });
        s.view.selection = Selection::point(s.doc.text.len_chars());
        update(&mut s, Msg::Tick { now_ms: 1 });
        let f = view(&s);
        assert_eq!(f.rows.len(), h as usize);
        if w > 12 {
            assert!(f.cursor.is_some(), "{w}x{h}: the caret is in view");
        }
    }
}

/// Random messages through a laid-out view with folds and extra rows: the caret is never in
/// a marker or a hidden block, it is drawn where its row says, and every frame has one row
/// info per row.
#[test]
fn random_editing_in_a_laid_out_view() {
    for seed in 0..60u64 {
        let mut rng = StdRng::seed_from_u64(0x1a_0000 + seed);
        let (w, h) = (rng.random_range(14..70), rng.random_range(3..20));
        let mut s = laid_out(TRIP, w, h);
        for step in 0..120 {
            let o = s.blocks().unwrap();
            let id = o.blocks[rng.random_range(0..o.blocks.len())].id;
            let dir = if rng.random_bool(0.5) { Dir::Forward } else { Dir::Backward };
            let msg = match rng.random_range(0..16) {
                0..=2 => Msg::InsertText { text: ["x", "word ", "日本", "- "][rng.random_range(0..4)].into() },
                3 => Msg::InsertNewline,
                4 => Msg::DeleteBackward,
                5 => Msg::Indent,
                6 => Msg::ToggleFold { id },
                7..=9 => Msg::Move { dir, by: [By::VisualLine, By::Page, By::Grapheme, By::LineEnd][rng.random_range(0..4)], extend: false },
                10 => Msg::Click { col: rng.random_range(0..w), row: rng.random_range(0..h), extend: false },
                11 => Msg::Undo,
                12 => Msg::ScrollView { rows: rng.random_range(-4..5) },
                13 => {
                    s.view.layout.as_mut().unwrap().extra_rows.insert(id, rng.random_range(0..3));
                    Msg::Tick { now_ms: step }
                }
                14 => Msg::Resize { width: rng.random_range(14..70), height: rng.random_range(3..20) },
                _ => Msg::TaskCycle,
            };
            update(&mut s, msg.clone());
            let ctx = format!("seed {seed} step {step} {msg:?}");
            let o = s.blocks().unwrap();
            let text = s.doc.text.slice(..);
            let caret = s.caret();
            let b = o.block_at(text, caret);
            assert!(!(b.prefix_len > 0 && caret >= b.start && caret < b.content_start()), "{ctx}: caret in a marker");
            let hidden = caretline_next::views::hidden_lines(&o, &s.view.folds);
            let line = text.char_to_line(caret);
            assert!(!hidden.iter().any(|&(a, z)| a <= line && line < z), "{ctx}: caret in a folded block");
            let f = view(&s);
            assert_eq!(f.rows.len(), s.view.viewport.height as usize, "{ctx}");
            if let Some((_, y)) = f.cursor {
                match &f.rows[y as usize] {
                    RowInfo::Text { chars, line: l, .. } => {
                        assert_eq!(*l, line, "{ctx}: the caret's row is its line");
                        assert!(chars.start <= caret && caret <= chars.end.max(chars.start), "{ctx}: caret {caret} outside {chars:?}");
                    }
                    other => panic!("{ctx}: the caret on {other:?}"),
                }
            } else if !s.view.free && s.view.viewport.width > 20 && b.atomic.eq(&false) && s.view.selection.primary().is_empty() {
                panic!("{ctx}: a following view lost its caret {caret} line {line} scroll {:?} text {:?} rows {:?}\n{}", s.view.scroll, s.doc.text.to_string(), f.rows, f.to_text());
            }
        }
    }
}

#[test]
fn the_layout_survives_json() {
    let mut s = laid_out(TRIP, 50, 16);
    let id = ids(&s)[2];
    s.view.layout.as_mut().unwrap().extra_rows.insert(id, 1);
    let back = State::from_json(&s.to_json()).unwrap();
    assert_eq!(back, s);
    assert_eq!(view(&back), view(&s));
    let v: View = serde_json::from_str(r#"{"layout":{"column":40},"folds":[2]}"#).unwrap();
    assert_eq!(v.layout.unwrap().indent, 4, "defaults fill the rest");
}
