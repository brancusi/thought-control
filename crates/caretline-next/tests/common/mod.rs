//! Test helpers: the caret notation and short ways to drive the editor.
//!
//! Notation: `▮` is the caret. `⟦…⟧` is a selection, with `▮` at the end where the caret
//! is: `⟦abc▮⟧` was selected left to right and `⟦▮abc⟧` right to left.
#![allow(dead_code)]

use caretline_next::helix::Selection;
use caretline_next::{script_to_msgs, update, view, Effect, Msg, State, Viewport};

/// Builds a state from notation, laid out at `width`x`height`.
pub fn state_wh(notation: &str, width: u16, height: u16) -> State {
    let (text, anchor, head) = parse(notation);
    let mut state = State::new(&text, Some("test.md".into()), Viewport { width, height });
    state.selection = Selection::single(anchor, head);
    // A resize message settles the view around the caret.
    update(&mut state, Msg::Resize { width, height });
    state
}

/// A state at 80x24.
pub fn state(notation: &str) -> State {
    state_wh(notation, 80, 24)
}

/// Splits notation into text, anchor and head (char indices).
pub fn parse(notation: &str) -> (String, usize, usize) {
    let mut text = String::new();
    let mut n = 0usize;
    let (mut caret, mut open, mut close) = (None, None, None);
    for c in notation.chars() {
        match c {
            '▮' => caret = Some(n),
            '⟦' => open = Some(n),
            '⟧' => close = Some(n),
            c => {
                text.push(c);
                n += 1;
            }
        }
    }
    let head = caret.expect("notation needs a caret ▮");
    match (open, close) {
        (Some(o), Some(c)) => {
            let anchor = if head == o { c } else { o };
            (text, anchor, head)
        }
        (None, None) => (text, head, head),
        _ => panic!("unbalanced ⟦⟧ in {notation:?}"),
    }
}

/// The primary selection in notation.
pub fn show(state: &State) -> String {
    let r = state.selection.primary();
    let text = state.text.to_string();
    let mut out = String::new();
    for (i, c) in text.chars().enumerate() {
        mark(&mut out, i, r.anchor, r.head);
        out.push(c);
    }
    mark(&mut out, text.chars().count(), r.anchor, r.head);
    out
}

fn mark(out: &mut String, i: usize, anchor: usize, head: usize) {
    if anchor == head {
        if i == head {
            out.push('▮');
        }
        return;
    }
    let (from, to) = (anchor.min(head), anchor.max(head));
    if i == from {
        out.push('⟦');
        if head == from {
            out.push('▮');
        }
    }
    if i == to {
        if head == to {
            out.push('▮');
        }
        out.push('⟧');
    }
}

/// Runs a key script through the keymap and update; returns all effects.
pub fn keys(state: &mut State, script: &str) -> Vec<Effect> {
    let msgs = script_to_msgs(script, state.now_ms).expect("key script parses");
    msgs.into_iter().flat_map(|m| update(state, m)).collect()
}

/// Sends messages; returns all effects.
pub fn send(state: &mut State, msgs: impl IntoIterator<Item = Msg>) -> Vec<Effect> {
    msgs.into_iter().flat_map(|m| update(state, m)).collect()
}

/// `before · keys → after`, all in notation.
#[track_caller]
pub fn golden(before: &str, script: &str, after: &str) {
    let mut s = state(before);
    keys(&mut s, script);
    assert_eq!(show(&s), after, "{before:?} · {script:?}");
}

/// The rendered frame as text.
pub fn frame(state: &State) -> String {
    view(state).to_text()
}

/// The caret's screen cell.
pub fn cursor(state: &State) -> Option<(u16, u16)> {
    view(state).cursor
}

/// Random documents and messages for the property tests (seeded, so failures reproduce).
pub mod gen {
    use caretline_next::helix::{Range, Selection, SmallVec};
    use caretline_next::{By, Dir, Msg, State};
    use rand::rngs::StdRng;
    use rand::Rng;

    const PIECES: &[&str] = &[
        "a", "b", "word", "Hello", "the", " ", "  ", "\t", "\n", "\n", "\r\n", ".", ",", "-",
        "_", "e\u{301}", "👍🏽", "👨‍👩‍👧", "漢字", "カナ", "🇫🇷", "❤️", "x\u{200b}y", "\u{7}",
    ];

    pub fn text(rng: &mut StdRng) -> String {
        let n = rng.random_range(0..120);
        (0..n).map(|_| PIECES[rng.random_range(0..PIECES.len())]).collect()
    }

    fn snippet(rng: &mut StdRng) -> String {
        let n = rng.random_range(0..6);
        (0..n).map(|_| PIECES[rng.random_range(0..PIECES.len())]).collect()
    }

    fn dir(rng: &mut StdRng) -> Dir {
        if rng.random_bool(0.5) {
            Dir::Backward
        } else {
            Dir::Forward
        }
    }

    pub const BYS: &[By] = &[
        By::Grapheme,
        By::Word,
        By::Line,
        By::VisualLine,
        By::LineStart,
        By::LineEnd,
        By::Page,
        By::DocStart,
        By::DocEnd,
    ];

    pub fn size(rng: &mut StdRng) -> (u16, u16) {
        match rng.random_range(0..6) {
            0 => (1, 1),
            1 => (2, 200),
            2 => (200, 2),
            3 => (rng.random_range(1..12), rng.random_range(1..6)),
            _ => (rng.random_range(10..120), rng.random_range(2..40)),
        }
    }

    pub fn msg(rng: &mut StdRng, state: &State) -> Msg {
        match rng.random_range(0..100) {
            0..=19 => Msg::InsertText { text: PIECES[rng.random_range(0..PIECES.len())].to_string() },
            20..=24 => Msg::InsertNewline,
            25..=31 => Msg::DeleteBackward,
            32..=35 => Msg::DeleteForward,
            36..=37 => Msg::DeleteWordBackward,
            38..=39 => Msg::DeleteWordForward,
            40 => Msg::DeleteToLineStart,
            41 => Msg::DeleteToLineEnd,
            42 => Msg::KillLine,
            43..=66 => Msg::Move {
                dir: dir(rng),
                by: BYS[rng.random_range(0..BYS.len())],
                extend: rng.random_bool(0.4),
            },
            67..=69 => Msg::Click {
                col: rng.random_range(0..state.viewport.width.saturating_add(3)),
                row: rng.random_range(0..state.viewport.height.saturating_add(3)),
                extend: rng.random_bool(0.3),
            },
            70 => Msg::Scroll { rows: rng.random_range(-30..30) },
            71 => Msg::SelectAll,
            72 => Msg::Collapse,
            73..=74 => Msg::Copy,
            75..=76 => Msg::Cut,
            77..=78 => Msg::Paste { text: if rng.random_bool(0.5) { None } else { Some(snippet(rng)) } },
            79..=83 => Msg::Undo,
            84..=86 => Msg::Redo,
            87 => Msg::Save,
            88 => Msg::Saved,
            89 => Msg::SaveFailed { err: "disk full".into() },
            90 => Msg::Quit,
            91..=93 => {
                let (width, height) = size(rng);
                Msg::Resize { width, height }
            }
            _ => Msg::Tick { now_ms: state.now_ms + rng.random_range(0..3000) },
        }
    }

    /// A random multi-range selection on grapheme boundaries (as a state file could hold).
    pub fn multi_selection(rng: &mut StdRng, state: &State) -> Selection {
        use caretline_next::helix::graphemes::ensure_grapheme_boundary_prev;
        let text = state.text.slice(..);
        let len = text.len_chars();
        let n = rng.random_range(1..4);
        let ranges: SmallVec<[Range; 1]> = (0..n)
            .map(|_| {
                let a = ensure_grapheme_boundary_prev(text, rng.random_range(0..=len));
                let h = if rng.random_bool(0.5) {
                    a
                } else {
                    ensure_grapheme_boundary_prev(text, rng.random_range(0..=len))
                };
                Range::new(a, h)
            })
            .collect();
        Selection::new(ranges, 0)
    }
}
