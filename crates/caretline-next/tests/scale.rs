//! Long typing runs stay linear: one `insert_text` per character, all in one run (no clock
//! gap), as an agent streaming text into an editor sends them.

use std::time::{Duration, Instant};

use caretline_next::state::{RUN_MAX_CHARS, RUN_WORD_BREAK_CHARS};
use caretline_next::{view, Msg, Session, State, Viewport};

/// Types `n` characters of `pattern` one message at a time, rendering every `render_every`
/// characters (0: never). Returns the session and the time it took.
fn type_run(n: usize, pattern: &str, render_every: usize) -> (Session, Duration) {
    let mut s = Session::new(State::new("", None, Viewport { width: 80, height: 24 }));
    let chars: Vec<char> = pattern.chars().collect();
    let t = Instant::now();
    for i in 0..n {
        s.apply(Msg::InsertText { text: chars[i % chars.len()].to_string() });
        if render_every > 0 && i % render_every == 0 {
            std::hint::black_box(view(s.state()));
        }
    }
    (s, t.elapsed())
}

const PROSE: &str = "The quick brown fox jumps over the lazy dog, again and again. ";

#[test]
fn a_long_run_splits_into_bounded_undo_steps() {
    for pattern in ["x", PROSE] {
        let (mut s, _) = type_run(4000, pattern, 0);
        let typed = s.state().doc.text.len_chars();
        assert_eq!(typed, 4000);
        // Every step holds at most RUN_MAX_CHARS, so there are at least n / max steps.
        let steps = s.state().doc.history.len() - 1;
        assert!(steps >= 4000 / RUN_MAX_CHARS, "{pattern:?}: {steps} steps");
        // Each undo removes one step: never more than RUN_MAX_CHARS characters.
        let mut prev = typed;
        while prev > 0 {
            s.apply(Msg::Undo);
            let now = s.state().doc.text.len_chars();
            assert!(now < prev && prev - now <= RUN_MAX_CHARS, "{pattern:?}: undo {prev} -> {now}");
            prev = now;
        }
    }
}

#[test]
fn prose_runs_break_at_word_boundaries() {
    let (s, _) = type_run(2000, PROSE, 0);
    let mut state = s.state().clone();
    let full = state.doc.text.to_string();
    caretline_next::update(&mut state, Msg::Undo);
    let kept = state.doc.text.len_chars();
    let removed: String = full.chars().skip(kept).collect();
    assert!(removed.starts_with(' '), "the last step starts at a word boundary: {removed:?}");
    assert!(removed.chars().count() <= RUN_MAX_CHARS);
    let (s, _) = type_run(RUN_WORD_BREAK_CHARS - 1, PROSE, 0);
    assert_eq!(s.state().doc.history.len(), 2, "a short run is one step");
}

/// 16,000 characters in one run, on one line and as prose, finish well under a second in a
/// release build, and four times the characters take about four times as long (not
/// sixteen), also when every keystroke is rendered as a live editor does. Debug builds run a
/// shorter run without timing it. `cargo test --release --test scale -- --nocapture` prints
/// the numbers.
#[test]
fn a_16k_character_run_is_linear() {
    if cfg!(debug_assertions) {
        let (s, _) = type_run(3000, "x", 1);
        assert_eq!(s.state().doc.text.len_chars(), 3000);
        return;
    }
    for (pattern, render_every) in [("x", 0), (PROSE, 0), ("x", 1), (PROSE, 1)] {
        let (_, small) = type_run(4000, pattern, render_every);
        let (s, big) = type_run(16000, pattern, render_every);
        assert_eq!(s.state().doc.text.len_chars(), 16000);
        let ratio = big.as_secs_f64() / small.as_secs_f64().max(1e-6);
        eprintln!("{pattern:.1?} render {render_every}: 4k {small:?}, 16k {big:?}, ratio {ratio:.1}");
        if render_every == 0 {
            assert!(big < Duration::from_millis(500), "{pattern:?}: 16k chars took {big:?}");
        }
        assert!(ratio < 8.0, "{pattern:?}: 4x the chars took {ratio:.1}x the time");
    }
}

/// Typing in a 5,000-block outline stays well inside an interactive budget in a release
/// build (each key re-derives the blocks; see docs/caretline/outline.md). Debug builds type a
/// few keys without timing them.
#[test]
fn typing_in_a_5000_block_outline() {
    use caretline_next::outline::markdown;
    use caretline_next::OutlineConfig;
    let mut md = String::new();
    for i in 0..5000 {
        md.push_str(&format!("- [ ] item number {i} with some words\n"));
    }
    let mut s = markdown::load(&md, None, Viewport { width: 100, height: 40 }, OutlineConfig::default());
    let keys = if cfg!(debug_assertions) { 20 } else { 400 };
    for pos in [10, s.doc.text.len_chars() - 3] {
        s.view.selection = caretline_next::helix::Selection::point(pos);
        let t = Instant::now();
        for k in 0..keys {
            caretline_next::update(&mut s, Msg::InsertText { text: "x".into() });
            if k % 50 == 49 {
                caretline_next::update(&mut s, Msg::InsertNewline);
            }
            std::hint::black_box(view(&s));
        }
        let per_key = t.elapsed() / keys as u32;
        eprintln!("outline typing at {pos}: {per_key:?} per key");
        if !cfg!(debug_assertions) {
            assert!(per_key < Duration::from_millis(4), "{per_key:?} per key");
        }
    }
}
