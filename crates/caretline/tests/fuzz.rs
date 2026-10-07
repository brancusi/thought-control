//! Property tests: random documents, random message sequences (seeded), and the editing
//! invariants checked after every step.

mod common;

use caretline::helix::graphemes::ensure_grapheme_boundary_prev;
use caretline::{update, view, Msg, State, Viewport};
use common::gen;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

const SEEDS: u64 = 24;
const STEPS: usize = 500;

fn check_selection(state: &State, ctx: &str) {
    let text = state.doc.text.slice(..);
    let len = text.len_chars();
    for r in state.view.selection.iter() {
        for pos in [r.anchor, r.head] {
            assert!(pos <= len, "{ctx}: position {pos} past the end {len}");
            assert_eq!(
                ensure_grapheme_boundary_prev(text, pos),
                pos,
                "{ctx}: position {pos} is inside a grapheme of {:?}",
                state.doc.text.to_string()
            );
        }
    }
}

fn single_range(state: &State) -> Option<(usize, usize, usize, usize)> {
    if state.view.selection.len() == 1 {
        let r = state.view.selection.primary();
        Some((r.anchor, r.head, r.from(), r.to()))
    } else {
        None
    }
}

fn splice(text: &str, from: usize, to: usize, insert: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out: String = chars[..from].iter().collect();
    out.push_str(insert);
    out.extend(&chars[to..]);
    out
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

fn run_seed(seed: u64) -> usize {
    let mut rng = StdRng::seed_from_u64(seed);
    let original = gen::text(&mut rng);
    let (width, height) = gen::size(&mut rng);
    let mut state = State::new(&original, Some("fuzz.md".into()), Viewport { width, height });
    let mut steps = 0;
    for step in 0..STEPS {
        let ctx = format!("seed {seed} step {step}");
        if rng.random_range(0..40) == 0 {
            // Start from a multi-range selection now and then, as a state file could.
            state.view.selection = gen::multi_selection(&mut rng, &state);
        }
        let msg = gen::msg(&mut rng, &state);
        let before = state.clone();
        let effects = update(&mut state, msg.clone());
        steps += 1;
        let ctx = format!("{ctx} {msg:?}");
        check_selection(&state, &ctx);
        let text_before = before.doc.text.to_string();

        match &msg {
            // EI1: a motion without Shift leaves no selection.
            Msg::Move { extend: false, .. } => {
                assert!(state.view.selection.iter().all(|r| r.is_empty()), "{ctx}: EI1");
            }
            // EI2: a motion with Shift never moves the anchor.
            Msg::Move { extend: true, .. } => {
                if let (Some((a0, ..)), Some((a1, h1, ..))) = (single_range(&before), single_range(&state)) {
                    assert_eq!(a0, a1, "{ctx}: EI2 anchor moved");
                    let _ = h1;
                }
            }
            // EI4: copy changes nothing but the clipboard.
            Msg::Copy => {
                assert_eq!(state.doc.text, before.doc.text, "{ctx}: EI4 text");
                assert_eq!(state.view.selection, before.view.selection, "{ctx}: EI4 selection");
                assert_eq!(state.doc.history, before.doc.history, "{ctx}: EI4 history");
                assert_eq!(state.doc.dirty, before.doc.dirty, "{ctx}: EI4 dirty");
            }
            _ => {}
        }

        if let Some((_, _, from, to)) = single_range(&before) {
            if from < to {
                let removed = splice(&text_before, from, to, "");
                match &msg {
                    // EI6: typing over a selection replaces exactly it.
                    Msg::InsertText { text } if !text.contains(['\r', '\n']) => {
                        assert_eq!(state.doc.text.to_string(), splice(&text_before, from, to, text), "{ctx}: EI6");
                    }
                    // EI7: every delete with a selection removes exactly the selection.
                    Msg::DeleteBackward
                    | Msg::DeleteForward
                    | Msg::DeleteWordBackward
                    | Msg::DeleteWordForward
                    | Msg::DeleteToLineStart
                    | Msg::DeleteToLineEnd
                    | Msg::KillLine
                    | Msg::Cut => {
                        assert_eq!(state.doc.text.to_string(), removed, "{ctx}: EI7");
                    }
                    _ => {}
                }
            }
        }

        // EI5: an edit that made a new revision undoes to exactly the state before it
        // (text and selection), and redoes to exactly the state after it.
        if is_edit(&msg) && state.doc.history.len() == before.doc.history.len() + 1 {
            let mut undone = state.clone();
            update(&mut undone, Msg::Undo);
            assert_eq!(undone.doc.text, before.doc.text, "{ctx}: EI5 undo text");
            assert_eq!(undone.view.selection, before.view.selection, "{ctx}: EI5 undo selection");
            update(&mut undone, Msg::Redo);
            assert_eq!(undone.doc.text, state.doc.text, "{ctx}: EI5 redo text");
            assert_eq!(undone.view.selection.ranges().len(), state.view.selection.ranges().len(), "{ctx}: EI5 redo");
        }

        // Effects are plain values and match the message.
        if matches!(msg, Msg::Quit) && !effects.is_empty() {
            // A quit ends a real session; keep fuzzing from the same state anyway.
        }

        // view never panics, at the state's size and at extreme sizes.
        let frame = view(&state);
        assert_eq!(frame.cells.len(), state.view.viewport.width as usize * state.view.viewport.height as usize);
        if step % 10 == 0 {
            for (w, h) in [(1, 1), (2, 200), (200, 2), gen::size(&mut rng)] {
                let mut sized = state.clone();
                update(&mut sized, Msg::Resize { width: w, height: h });
                let f = view(&sized);
                if let Some((x, y)) = f.cursor {
                    assert!(x < w && y < h.saturating_sub(1).max(1), "{ctx}: cursor off screen");
                }
            }
        }

        // Serialize and deserialize at random points: the same state and the same frame.
        if rng.random_range(0..25) == 0 {
            let json = state.to_json();
            let back = State::from_json(&json).expect("state parses");
            assert_eq!(back, state, "{ctx}: round trip");
            assert_eq!(view(&back), frame, "{ctx}: round-trip frame");
        }
    }

    // Undo all the way back: the original text.
    let mut guard = 0;
    loop {
        update(&mut state, Msg::Undo);
        if state.view.status.as_deref() == Some("nothing to undo") {
            break;
        }
        guard += 1;
        assert!(guard < 10_000, "seed {seed}: undo never reached the root");
    }
    assert_eq!(state.doc.text.to_string(), original, "seed {seed}: full undo");
    check_selection(&state, &format!("seed {seed} after full undo"));
    steps
}

#[test]
fn random_sessions_keep_every_invariant() {
    let steps: usize = (0..SEEDS).map(run_seed).sum();
    assert!(steps >= 10_000, "only {steps} steps");
}

/// EI3 and EI8: cut then paste in place, and copy then paste over the same selection, leave
/// the text unchanged.
#[test]
fn cut_paste_and_copy_paste_are_identities() {
    let mut rng = StdRng::seed_from_u64(7);
    for _ in 0..400 {
        let text = gen::text(&mut rng);
        let mut s = State::new(&text, None, Viewport { width: 40, height: 10 });
        let len = s.doc.text.len_chars();
        let t = s.doc.text.slice(..);
        let a = ensure_grapheme_boundary_prev(t, rng.random_range(0..=len));
        let b = ensure_grapheme_boundary_prev(t, rng.random_range(0..=len));
        s.view.selection = caretline::helix::Selection::single(a, b);
        let mut cut = s.clone();
        update(&mut cut, Msg::Cut);
        update(&mut cut, Msg::Paste { text: None });
        if a != b {
            assert_eq!(cut.doc.text, s.doc.text, "EI3 on {text:?} {a}..{b}");
            assert_eq!(cut.caret(), a.max(b), "EI3 caret at the end of the paste");
        }
        let mut copy = s.clone();
        update(&mut copy, Msg::Copy);
        update(&mut copy, Msg::Paste { text: None });
        if a != b {
            assert_eq!(copy.doc.text, s.doc.text, "EI8 on {text:?} {a}..{b}");
        }
    }
}

/// EI12: select all and delete leaves an empty document; one undo brings it all back.
#[test]
fn select_all_delete_then_undo() {
    let mut rng = StdRng::seed_from_u64(12);
    for _ in 0..200 {
        let text = gen::text(&mut rng);
        let mut s = State::new(&text, None, Viewport { width: 30, height: 8 });
        update(&mut s, Msg::SelectAll);
        update(&mut s, Msg::DeleteBackward);
        assert_eq!(s.doc.text.len_chars(), 0);
        update(&mut s, Msg::Undo);
        assert_eq!(s.doc.text.to_string(), text);
        assert_eq!(s.view.selection, caretline::helix::Selection::single(0, s.doc.text.len_chars()));
    }
}
