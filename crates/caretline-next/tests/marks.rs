//! Block marks: ids at line starts, mapped through every edit, restored exactly by undo and
//! redo, carried by cut and paste, and kept in the serialized state.
//!
//! Marks here are added by hand on plain-text documents, so these tests cover the mapping
//! alone. The outline layer (which adds a mark to every block) has its own goldens.

mod common;

use caretline_next::helix::Selection;
use caretline_next::marks::{BlockAttrs, Mark, MarkId};
use caretline_next::{update, Msg, State, Viewport};
use common::*;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

/// A state from notation with a mark on each of `lines` (ids 0, 1, … in that order).
fn marked(notation: &str, lines: &[usize]) -> State {
    let mut s = state(notation);
    for &l in lines {
        let pos = s.text.line_to_char(l);
        s.marks.mint(pos);
    }
    s
}

/// The marks as (line, id).
fn marks(s: &State) -> Vec<(usize, u64)> {
    s.marks.iter().map(|m| (s.text.char_to_line(m.pos), m.id.0)).collect()
}

#[test]
fn typing_at_a_line_start_keeps_the_line_its_mark() {
    let mut s = marked("ab\n▮cd", &[0, 1]);
    keys(&mut s, "x");
    assert_eq!(show(&s), "ab\nx▮cd");
    assert_eq!(marks(&s), [(0, 0), (1, 1)]);
}

#[test]
fn a_break_typed_at_a_mark_moves_the_mark_down_with_its_text() {
    let mut s = marked("ab\n▮cd", &[0, 1]);
    keys(&mut s, "<cr>");
    assert_eq!(show(&s), "ab\n\n▮cd");
    assert_eq!(marks(&s), [(0, 0), (2, 1)]);
}

#[test]
fn a_break_typed_mid_line_moves_no_mark() {
    let mut s = marked("a▮b\ncd", &[0, 1]);
    keys(&mut s, "<cr>");
    assert_eq!(show(&s), "a\n▮b\ncd");
    assert_eq!(marks(&s), [(0, 0), (2, 1)]);
}

#[test]
fn joining_a_line_into_the_one_above_removes_its_mark_and_undo_brings_it_back() {
    let mut s = marked("ab\n▮cd", &[0, 1]);
    keys(&mut s, "<bs>");
    assert_eq!(show(&s), "ab▮cd");
    assert_eq!(marks(&s), [(0, 0)]);
    keys(&mut s, "<c-z>");
    assert_eq!(show(&s), "ab\n▮cd");
    assert_eq!(marks(&s), [(0, 0), (1, 1)]);
    keys(&mut s, "<c-y>");
    assert_eq!(marks(&s), [(0, 0)]);
}

#[test]
fn deleting_across_lines_keeps_the_first_mark() {
    // Typing over a selection that spans lines: one line, with the first line's mark.
    let mut s = marked("One ⟦two\nthree fo▮⟧ur", &[0, 1]);
    keys(&mut s, "X");
    assert_eq!(show(&s), "One X▮ur");
    assert_eq!(marks(&s), [(0, 0)]);
    keys(&mut s, "<c-z>");
    assert_eq!(show(&s), "One ⟦two\nthree fo▮⟧ur");
    assert_eq!(marks(&s), [(0, 0), (1, 1)]);

    let mut s = marked("A⟦a\nBb\nC▮⟧c", &[0, 1, 2]);
    keys(&mut s, "<bs>");
    assert_eq!(show(&s), "A▮c");
    assert_eq!(marks(&s), [(0, 0)]);
    keys(&mut s, "<c-z>");
    assert_eq!(marks(&s), [(0, 0), (1, 1), (2, 2)]);
}

#[test]
fn deleting_the_start_of_a_line_keeps_its_mark() {
    let mut s = marked("ab\n⟦- ▮⟧cd", &[0, 1]);
    keys(&mut s, "<bs>");
    assert_eq!(show(&s), "ab\n▮cd");
    assert_eq!(marks(&s), [(0, 0), (1, 1)]);
}

#[test]
fn deleting_whole_lines_takes_their_marks_and_the_next_line_keeps_its_own() {
    let mut s = marked("a\n⟦b\nc\n▮⟧d", &[0, 1, 2, 3]);
    keys(&mut s, "<bs>");
    assert_eq!(show(&s), "a\n▮d");
    assert_eq!(marks(&s), [(0, 0), (1, 3)]);
}

#[test]
fn cut_then_paste_in_place_keeps_the_ids() {
    let mut s = marked("One ⟦two\nth▮⟧ree", &[0, 1]);
    keys(&mut s, "<c-x>");
    assert_eq!(show(&s), "One ▮ree");
    assert_eq!(marks(&s), [(0, 0)]);
    assert_eq!(s.clipboard.marks.len(), 1, "the register carries the cut mark");
    keys(&mut s, "<c-v>");
    assert_eq!(show(&s), "One two\nth▮ree");
    assert_eq!(marks(&s), [(0, 0), (1, 1)]);
}

#[test]
fn a_paste_of_the_registers_own_text_from_outside_keeps_the_ids_too() {
    // A front end pastes the system clipboard, which holds what the cut put there.
    let mut s = marked("One ⟦two\nth▮⟧ree", &[0, 1]);
    let fx = send(&mut s, [Msg::Cut]);
    let text = match &fx[0] {
        caretline_next::Effect::ClipboardSet { text } => text.clone(),
        other => panic!("{other:?}"),
    };
    send(&mut s, [Msg::Paste { text: Some(text) }]);
    assert_eq!(marks(&s), [(0, 0), (1, 1)]);
    // Other text is new: it brings no marks.
    send(&mut s, [Msg::Paste { text: Some("x\ny".into()) }]);
    assert_eq!(marks(&s), [(0, 0), (1, 1)]);
}

#[test]
fn whole_lines_cut_and_pasted_elsewhere_take_their_ids_along() {
    let mut s = marked("⟦a\nb\n▮⟧c\n", &[0, 1, 2]);
    keys(&mut s, "<c-x>");
    assert_eq!(show(&s), "▮c\n");
    assert_eq!(marks(&s), [(0, 2)]);
    keys(&mut s, "<d-down><c-v>");
    assert_eq!(show(&s), "c\na\nb\n▮");
    assert_eq!(marks(&s), [(0, 2), (1, 0), (2, 1)]);
}

#[test]
fn a_second_paste_never_duplicates_an_id() {
    let mut s = marked("x\n⟦a\n▮⟧", &[0, 1]);
    keys(&mut s, "<c-x><c-v><c-v>");
    assert_eq!(s.text.to_string(), "x\na\na\n");
    let ids: Vec<u64> = s.marks.iter().map(|m| m.id.0).collect();
    assert_eq!(ids, [0, 1], "the second copy brings no mark: {:?}", marks(&s));
}

#[test]
fn a_typing_run_undoes_to_its_start_marks_included() {
    let mut s = marked("ab\n▮cd\nef", &[0, 1, 2]);
    keys(&mut s, "xy<cr>z");
    // The text typed at the line start belongs to that line, and so does its mark.
    assert_eq!(show(&s), "ab\nxy\nz▮cd\nef");
    assert_eq!(marks(&s), [(0, 0), (1, 1), (3, 2)]);
    keys(&mut s, "<c-z>");
    assert_eq!(show(&s), "ab\n▮cd\nef");
    assert_eq!(marks(&s), [(0, 0), (1, 1), (2, 2)]);
    keys(&mut s, "<c-y>");
    assert_eq!(marks(&s), [(0, 0), (1, 1), (3, 2)]);
}

#[test]
fn attributes_travel_with_their_mark() {
    let mut s = marked("ab\n▮cd", &[0, 1]);
    s.marks.set_attrs(MarkId(1), BlockAttrs { gap: Some(false) });
    keys(&mut s, "<bs>");
    keys(&mut s, "<c-z>");
    assert_eq!(s.marks.attrs(MarkId(1)), BlockAttrs { gap: Some(false) });
}

#[test]
fn marks_and_their_log_survive_serialization() {
    let mut s = marked("ab\n▮cd", &[0, 1]);
    keys(&mut s, "<bs>x");
    let json = s.to_json();
    let mut back = State::from_json(&json).unwrap();
    assert_eq!(back, s);
    keys(&mut back, "<c-z><c-z>");
    assert_eq!(marks(&back), [(0, 0), (1, 1)], "undo after a round trip restores the marks");
    // A state with no marks serializes as before: no new keys.
    let plain = state("abc▮");
    assert!(!plain.to_json().contains("marks"));
}

#[test]
fn a_state_file_without_marks_or_with_a_string_clipboard_loads() {
    let s = State::from_json(r#"{"text":"a\nb","clipboard":"kept"}"#).unwrap();
    assert!(s.marks.is_empty());
    assert_eq!(s.clipboard, "kept");
    assert_eq!(s.mark_log.len(), s.history.len());
}

#[test]
fn marks_off_line_starts_are_repaired_on_load() {
    let mut s = state("ab\ncd▮");
    s.marks.mint(0);
    let mut v: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
    v["marks"]["marks"].as_array_mut().unwrap().push(serde_json::json!({"pos": 1, "id": 7}));
    v["marks"]["marks"].as_array_mut().unwrap().push(serde_json::json!({"pos": 3, "id": 0}));
    let back = State::from_json(&v.to_string()).unwrap();
    assert_eq!(marks(&back), [(0, 0)], "off a line start, and a repeated id, are dropped");
    assert!(back.marks.next_id().0 >= 8, "the counter stays past every id seen");
}

#[test]
fn an_edit_of_only_marks_is_an_undo_step_of_its_own() {
    // Inserting a mark the way a rule does, through the history.
    let mut s = marked("ab\n▮cd", &[0]);
    caretline_next::update::mark_only_edit(&mut s, |m, _| {
        m.insert(Mark::new(3, MarkId(40))).unwrap();
    });
    assert_eq!(marks(&s), [(0, 0), (1, 40)]);
    keys(&mut s, "<c-z>");
    assert_eq!(marks(&s), [(0, 0)]);
    keys(&mut s, "<c-y>");
    assert_eq!(marks(&s), [(0, 0), (1, 40)]);
}

// ---------------------------------------------------------------------------------------
// Properties over random sessions

fn check_marks(s: &State, ctx: &str) {
    let text = s.text.slice(..);
    let mut ids = std::collections::HashSet::new();
    let mut last: Option<usize> = None;
    for m in s.marks.iter() {
        assert!(m.pos <= text.len_chars(), "{ctx}: mark {m:?} past the end");
        assert!(caretline_next::marks::is_line_start(text, m.pos), "{ctx}: mark {m:?} not at a line start");
        assert!(last.is_none_or(|l| l < m.pos), "{ctx}: marks out of order or two on a line");
        assert!(ids.insert(m.id), "{ctx}: id {:?} twice", m.id);
        last = Some(m.pos);
    }
}

fn random_marked(rng: &mut StdRng) -> State {
    let text = gen::text(rng);
    let (width, height) = gen::size(rng);
    let mut s = State::new(&text, Some("fuzz.md".into()), Viewport { width, height });
    for line in 0..s.text.len_lines() {
        if rng.random_bool(0.6) {
            let pos = s.text.line_to_char(line);
            s.marks.mint(pos);
        }
    }
    s
}

fn is_edit(msg: &Msg) -> bool {
    matches!(
        msg,
        Msg::InsertText { .. }
            | Msg::InsertNewline
            | Msg::DeleteBackward
            | Msg::DeleteForward
            | Msg::DeleteWordBackward
            | Msg::DeleteWordForward
            | Msg::DeleteToLineStart
            | Msg::DeleteToLineEnd
            | Msg::KillLine
            | Msg::Cut
            | Msg::Paste { .. }
    )
}

/// Every mark stays at a line start with a unique id; an edit's undo restores the marks
/// exactly and its redo re-applies them; undoing everything gives the first marks back.
#[test]
fn marks_keep_their_invariants_in_random_sessions() {
    for seed in 0..24u64 {
        let mut rng = StdRng::seed_from_u64(0x3a7c ^ seed);
        let mut s = random_marked(&mut rng);
        let original = (s.text.to_string(), s.marks.clone());
        for step in 0..400 {
            let msg = gen::msg(&mut rng, &s);
            let before = s.clone();
            update(&mut s, msg.clone());
            let ctx = format!("seed {seed} step {step} {msg:?}");
            check_marks(&s, &ctx);
            if is_edit(&msg) && s.history.len() == before.history.len() + 1 {
                let mut undone = s.clone();
                update(&mut undone, Msg::Undo);
                assert_eq!(undone.marks, before.marks, "{ctx}: undo restores the marks");
                update(&mut undone, Msg::Redo);
                assert_eq!(undone.marks, s.marks, "{ctx}: redo re-applies them");
            }
            if rng.random_range(0..30) == 0 {
                let back = State::from_json(&s.to_json()).unwrap();
                assert_eq!(back, s, "{ctx}: round trip");
            }
        }
        let mut guard = 0;
        while s.history.current_revision() != 0 {
            update(&mut s, Msg::Undo);
            guard += 1;
            assert!(guard < 10_000);
        }
        assert_eq!(s.text.to_string(), original.0, "seed {seed}: text after full undo");
        assert_eq!(s.marks, original.1, "seed {seed}: marks after full undo");
    }
}

/// Cut then paste at the same place gives back the same text and the same marks.
#[test]
fn cut_and_paste_in_place_is_the_identity_for_marks() {
    let mut rng = StdRng::seed_from_u64(40);
    for case in 0..400 {
        let s0 = random_marked(&mut rng);
        let len = s0.text.len_chars();
        let t = s0.text.slice(..);
        use caretline_next::helix::graphemes::ensure_grapheme_boundary_prev;
        let a = ensure_grapheme_boundary_prev(t, rng.random_range(0..=len));
        let b = ensure_grapheme_boundary_prev(t, rng.random_range(0..=len));
        if a == b {
            continue;
        }
        let mut s = s0.clone();
        s.selection = Selection::single(a, b);
        update(&mut s, Msg::Cut);
        update(&mut s, Msg::Paste { text: None });
        assert_eq!(s.text, s0.text, "case {case}: text");
        assert_eq!(s.marks, s0.marks, "case {case}: marks for {:?} {a}..{b}", s0.text.to_string());
    }
}

/// The example in docs/caretline/api.md.
#[test]
fn the_api_example_runs() {
    use caretline_next::{BlockAttrs, By, Dir};
    let mut s = State::new("Groceries\nmilk\n", None, Viewport { width: 40, height: 5 });
    let list = s.marks.mint(0);
    let milk = s.marks.mint(s.text.line_to_char(1));
    s.marks.set_attrs(milk, BlockAttrs { gap: Some(false) });
    update(&mut s, Msg::InsertText { text: "Weekly ".into() });
    assert_eq!(s.marks.pos(list), Some(0));
    update(&mut s, Msg::Move { dir: Dir::Forward, by: By::DocEnd, extend: false });
    update(&mut s, Msg::Undo);
    assert_eq!(s.marks.pos(milk), Some(s.text.line_to_char(1)));
    assert_eq!(s.marks.attrs(milk), BlockAttrs { gap: Some(false) });
}
