//! Several views of one document: they share the text, marks and undo; an edit through one
//! rebases the others; a read-only view is refused every edit; folds belong to a view.

use caretline_next::helix::graphemes::ensure_grapheme_boundary_prev;
use caretline_next::helix::Selection;
use caretline_next::outline::markdown;
use caretline_next::{
    update, update_doc, view, By, Dir, Document, Effect, Msg, OutlineConfig, State, View, Viewport,
};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

const SAMPLE: &str = "First line of a paragraph\nand its second line\n\n- a bullet\n  - [ ] a task under it\n\nLast, with ünïcode 漢字 and 🙂\n";

/// The sample outline as one document, and a view of it at `w`x`h`.
fn sample() -> Document {
    markdown::load(SAMPLE, None, Viewport { width: 60, height: 20 }, OutlineConfig::default()).doc
}

fn at(w: u16, h: u16) -> View {
    View::new(Viewport { width: w, height: h })
}

/// Applies `msg` through view `i`.
fn send(doc: &mut Document, views: &mut [View], i: usize, msg: Msg) -> Vec<Effect> {
    update_doc(doc, views, i, msg)
}

/// A view seen as a single-view state (for rendering and checks).
fn state(doc: &Document, v: &View) -> State {
    State::from_parts(doc.clone(), v.clone())
}

/// Every selection end is inside the text, on a grapheme boundary and out of block markers.
fn valid(doc: &Document, v: &View) -> Result<(), String> {
    let text = doc.text.slice(..);
    let o = doc.blocks();
    for r in v.selection.iter() {
        for pos in [r.anchor, r.head] {
            if pos > text.len_chars() {
                return Err(format!("{pos} past the end"));
            }
            if ensure_grapheme_boundary_prev(text, pos) != pos {
                return Err(format!("{pos} inside a grapheme"));
            }
            if let Some(o) = &o {
                let b = o.block_at(text, pos);
                if b.prefix_len > 0 && pos >= b.start && pos < b.content_start() {
                    return Err(format!("{pos} inside the marker of block {:?}", b.id));
                }
            }
        }
    }
    for id in &v.folds {
        if !doc.marks.contains(*id) {
            return Err(format!("fold on a gone block {id:?}"));
        }
    }
    Ok(())
}

const EDITS: &[&str] = &["xé", "word ", "- ", "漢", "🙂"];

fn random_msg(rng: &mut StdRng, doc: &Document) -> Msg {
    let dir = if rng.random_bool(0.5) { Dir::Forward } else { Dir::Backward };
    let id = {
        let o = doc.blocks().unwrap();
        o.blocks[rng.random_range(0..o.blocks.len())].id
    };
    match rng.random_range(0..22) {
        0..=3 => Msg::InsertText { text: EDITS[rng.random_range(0..EDITS.len())].into() },
        4 => Msg::InsertNewline,
        5 => Msg::SoftBreak,
        6 => Msg::DeleteBackward,
        7 => Msg::DeleteForward,
        8 => Msg::DeleteWordBackward,
        9 => Msg::KillLine,
        10 => Msg::Indent,
        11 => Msg::Outdent,
        12 => Msg::TaskCycle,
        13 => Msg::MoveBlock { dir },
        14 => Msg::Move { dir, by: By::VisualLine, extend: false },
        15 => Msg::Move { dir, by: By::Word, extend: true },
        16 => Msg::Move { dir, by: By::LineEnd, extend: false },
        17 => Msg::Move { dir, by: By::DocEnd, extend: false },
        18 => Msg::Click { col: rng.random_range(0..40), row: rng.random_range(0..12), extend: false },
        19 => Msg::ToggleFold { id },
        20 => Msg::ScrollView { rows: rng.random_range(-5..6) },
        _ => Msg::Cut,
    }
}

/// The old engine's two-view check, ported: random edits through either view keep both
/// views valid, and undo from the other view goes back to an earlier state of the one
/// document (typing runs undo as one step, so maybe further than one edit).
#[test]
fn two_views_on_one_doc_share_text_and_undo() {
    let mut rng = StdRng::seed_from_u64(0x2545_F491_4F6C_DD1D);
    for case in 0..150 {
        let mut doc = sample();
        let mut views = [at(60, 20), at(24, 8)];
        views[1].selection = Selection::point(doc.text.len_chars() - 3);
        let mut seen = vec![doc.text.to_string()];
        for step in 0..30 {
            let i = rng.random_range(0..2);
            let msg = random_msg(&mut rng, &doc);
            let before = doc.text.to_string();
            send(&mut doc, &mut views, i, msg.clone());
            for (k, v) in views.iter().enumerate() {
                if let Err(e) = valid(&doc, v) {
                    panic!("case {case} step {step} {msg:?}: view {k}: {e} in {:?}", doc.text.to_string());
                }
            }
            if doc.text != before.as_str() && rng.random_range(0..4) == 0 {
                send(&mut doc, &mut views, 1 - i, Msg::Undo);
                assert!(seen.contains(&doc.text.to_string()), "case {case} step {step}: undo from the other view gave {:?}", doc.text.to_string());
                for v in &views {
                    valid(&doc, v).unwrap();
                }
            }
            seen.push(doc.text.to_string());
            // Both views render without panicking, at their own sizes.
            for v in &views {
                let f = view(&state(&doc, v));
                assert_eq!(f.cells.len(), v.viewport.width as usize * v.viewport.height as usize);
            }
        }
    }
}

#[test]
fn a_view_follows_an_edit_made_through_another() {
    let mut doc = State::new("hello world", None, Viewport { width: 40, height: 5 }).doc;
    let mut views = [at(40, 5), at(40, 5)];
    views[1].selection = Selection::point(6); // before "world"
    // Typing before it moves it right; typing after it doesn't.
    send(&mut doc, &mut views, 0, Msg::InsertText { text: "say ".into() });
    assert_eq!(doc.text.to_string(), "say hello world");
    assert_eq!(views[1].caret(), 10, "still before world");
    views[0].selection = Selection::point(15);
    send(&mut doc, &mut views, 0, Msg::InsertText { text: "!".into() });
    assert_eq!(views[1].caret(), 10);
    // A new line above it: it stays on its text, one line down.
    views[0].selection = Selection::point(0);
    send(&mut doc, &mut views, 0, Msg::InsertNewline);
    assert_eq!(doc.text.to_string(), "\nsay hello world!");
    assert_eq!(views[1].caret(), 11);
    // A selection in the other view grows with text typed inside it.
    views[1].selection = Selection::single(1, 4); // "say"
    views[0].selection = Selection::point(2);
    send(&mut doc, &mut views, 0, Msg::InsertText { text: "ee".into() });
    assert_eq!(views[1].selection.primary().from(), 1);
    assert_eq!(views[1].selection.primary().to(), 6, "{:?}", doc.text.to_string());
}

#[test]
fn a_read_only_view_is_refused_every_edit_and_still_moves() {
    let edits = [
        Msg::InsertNewline,
        Msg::SoftBreak,
        Msg::DeleteBackward,
        Msg::DeleteForward,
        Msg::DeleteWordBackward,
        Msg::KillLine,
        Msg::DeleteToLineStart,
        Msg::Indent,
        Msg::Outdent,
        Msg::TaskCycle,
        Msg::MoveBlock { dir: Dir::Forward },
        Msg::Undo,
        Msg::Redo,
        Msg::Cut,
        Msg::Paste { text: Some("pasted".into()) },
        Msg::InsertText { text: "x".into() },
    ];
    let mut doc = sample();
    let mut views = [at(60, 20), at(30, 10).read_only(true)];
    send(&mut doc, &mut views, 0, Msg::InsertText { text: "seed ".into() });
    views[1].selection = Selection::point(doc.text.line_to_char(2) + 3);
    let before = doc.clone();
    for msg in edits {
        let fx = send(&mut doc, &mut views, 1, msg.clone());
        assert_eq!(fx, vec![Effect::Refused], "{msg:?}");
        assert_eq!(doc, before, "{msg:?} changed nothing");
        assert_eq!(doc.history.len(), before.history.len(), "{msg:?} left undo alone");
    }
    // Motion, selection and copy still work.
    let c0 = views[1].caret();
    send(&mut doc, &mut views, 1, Msg::Move { dir: Dir::Forward, by: By::Grapheme, extend: false });
    assert_ne!(views[1].caret(), c0, "→ moves");
    send(&mut doc, &mut views, 1, Msg::SelectAll);
    assert!(!views[1].selection.primary().is_empty(), "select all selects");
    let fx = send(&mut doc, &mut views, 1, Msg::Copy);
    assert!(fx.iter().any(|e| matches!(e, Effect::ClipboardSet { .. })), "copy works");
    assert_eq!(doc.text, before.text);
    // The same through the single-view facade.
    let mut s = State::from_parts(doc.clone(), at(30, 10).read_only(true));
    assert_eq!(update(&mut s, Msg::InsertText { text: "no".into() }), vec![Effect::Refused]);
    assert_eq!(s.doc.text, doc.text);
}

#[test]
fn every_view_keeps_the_editing_invariants() {
    // A motion without Shift leaves no selection, and every end stays valid, at every width.
    let motions = [By::Grapheme, By::Word, By::VisualLine, By::Line, By::LineStart, By::LineEnd, By::DocStart, By::DocEnd, By::Block, By::Page];
    let mut rng = StdRng::seed_from_u64(0x9E37_79B9_7F4A_7C15);
    for w in [8u16, 20, 60] {
        let mut doc = sample();
        let mut views = [at(w, 10), at(60, 20)];
        for _ in 0..300 {
            let by = motions[rng.random_range(0..motions.len())];
            let dir = if rng.random_bool(0.5) { Dir::Forward } else { Dir::Backward };
            let extend = rng.random_range(0..3) == 0;
            send(&mut doc, &mut views, 0, Msg::Move { dir, by, extend });
            if !extend {
                assert!(views[0].selection.primary().is_empty(), "EI1 at width {w}: {by:?}");
            }
            valid(&doc, &views[0]).unwrap_or_else(|e| panic!("width {w}: {e}"));
        }
    }
}

#[test]
fn a_motion_rebases_nothing() {
    let mut doc = sample();
    let mut views = [at(60, 20), at(60, 20)];
    views[1].selection = Selection::single(doc.text.line_to_char(3) + 8, doc.text.line_to_char(3) + 10);
    let b = views[1].clone();
    send(&mut doc, &mut views, 0, Msg::Move { dir: Dir::Forward, by: By::DocEnd, extend: false });
    send(&mut doc, &mut views, 0, Msg::Click { col: 3, row: 1, extend: true });
    assert_eq!(views[1], b, "the other view is untouched");
}

#[test]
fn undo_is_the_documents_and_lands_in_the_acting_view() {
    let mut doc = State::new("one\ntwo\nthree", None, Viewport { width: 40, height: 5 }).doc;
    let mut views = [at(40, 5), at(40, 5)];
    views[0].selection = Selection::point(3);
    send(&mut doc, &mut views, 0, Msg::InsertText { text: "!".into() });
    views[1].selection = Selection::point(doc.text.len_chars());
    // Undo through the other view takes back view 0's edit and puts view 1's caret there.
    send(&mut doc, &mut views, 1, Msg::Undo);
    assert_eq!(doc.text.to_string(), "one\ntwo\nthree");
    assert_eq!(views[1].caret(), 3, "the acting view goes where the step happened");
    assert_eq!(views[0].caret(), 3, "the other view is mapped back");
    send(&mut doc, &mut views, 0, Msg::Redo);
    assert_eq!(doc.text.to_string(), "one!\ntwo\nthree");
    assert_eq!(views[1].caret(), 4);
}

#[test]
fn a_view_keeps_its_scroll_on_its_text() {
    let text: String = (0..60).map(|i| format!("line {i}\n")).collect();
    let mut doc = State::new(&text, None, Viewport { width: 40, height: 6 }).doc;
    let mut views = [at(40, 6), at(40, 6)];
    // View 1 looks at line 30.
    views[1].selection = Selection::point(doc.text.line_to_char(30));
    send(&mut doc, &mut views, 1, Msg::Tick { now_ms: 0 });
    let top = views[1].scroll.line;
    assert!(top > 20 && top <= 30, "{top}");
    // Three lines inserted at the top through view 0: view 1 still shows the same lines.
    send(&mut doc, &mut views, 0, Msg::InsertText { text: "a\nb\nc\n".into() });
    assert_eq!(views[1].scroll.line, top + 3);
    assert_eq!(doc.text.line(views[1].scroll.line).to_string(), format!("line {top}\n"));
    assert_eq!(views[1].caret(), doc.text.line_to_char(33));
}

/// Typing with a second view open costs no more than typing with one: the other view is
/// only rebased (its selection mapped), never laid out. Timed in a release build (`cargo
/// test --release --test views -- --nocapture` prints the numbers); debug builds type a few
/// keys without timing them.
#[test]
fn typing_in_a_big_doc_with_two_views() {
    let mut md = String::new();
    for i in 0..5000 {
        let pad = if i % 7 == 3 { "  " } else { "" };
        let marker = if i % 9 == 0 { "- [ ] " } else { "- " };
        md.push_str(&format!("{pad}{marker}the quick brown fox jumps over a lazy dog\n"));
    }
    let base = markdown::load(&md, None, Viewport { width: 120, height: 40 }, OutlineConfig::default()).doc;
    let keys = if cfg!(debug_assertions) { 20 } else { 300 };
    let mut per_key = Vec::new();
    for n in [1, 2] {
        let mut doc = base.clone();
        let mut views = vec![at(120, 40), at(40, 20)];
        views.truncate(n);
        views[0].selection = Selection::point(doc.text.line_to_char(2500) + 2);
        if n == 2 {
            views[1].selection = Selection::point(doc.text.line_to_char(4001) + 2);
        }
        let t = std::time::Instant::now();
        for k in 0..keys {
            let msg = if k % 3 == 2 { Msg::DeleteBackward } else { Msg::InsertText { text: "x".into() } };
            send(&mut doc, &mut views, 0, msg);
            std::hint::black_box(view(&state(&doc, &views[0])));
        }
        per_key.push(t.elapsed() / keys);
        if n == 2 {
            assert!(doc.text.line(4001).to_string().contains("quick"));
            assert_eq!(views[1].caret(), doc.text.line_to_char(4001) + 2, "the second view kept its place");
        }
    }
    eprintln!("5,000 blocks: one view {:?} per key, two views {:?} per key", per_key[0], per_key[1]);
    if !cfg!(debug_assertions) {
        assert!(per_key[1] < std::time::Duration::from_millis(4), "{:?} per key", per_key[1]);
    }
}

/// Replay is pure: the same messages through the same views give the same document, ids
/// and undo steps every time. Time comes only from `tick`.
#[test]
fn the_same_messages_give_the_same_documents() {
    let run = || {
        let mut doc = markdown::load("- \n", None, Viewport { width: 40, height: 10 }, OutlineConfig::default()).doc;
        let mut views = [at(40, 10), at(30, 6)];
        views[0].selection = Selection::point(2);
        for (now, i, msg) in [
            (0, 0, Msg::InsertText { text: "a".into() }),
            (500, 0, Msg::InsertText { text: "b".into() }),
            (5000, 0, Msg::InsertText { text: "c".into() }),
            (5100, 0, Msg::InsertNewline),
            (5200, 1, Msg::Move { dir: Dir::Forward, by: By::DocEnd, extend: false }),
            (5300, 0, Msg::InsertText { text: "next".into() }),
        ] {
            send(&mut doc, &mut views, i, Msg::Tick { now_ms: now });
            send(&mut doc, &mut views, i, msg);
        }
        let ids: Vec<u64> = doc.marks.iter().map(|m| m.id.0).collect();
        for _ in 0..3 {
            send(&mut doc, &mut views, 1, Msg::Undo);
        }
        (ids, doc.text.to_string(), views.clone())
    };
    let (ids, text, views) = run();
    assert_eq!(ids, vec![0, 1], "the new item got the next id");
    assert_eq!(text, "- ab", "\"ab\" was one run, \"c\" another (by the tick's clock)");
    assert_eq!(run(), (ids, text, views));
}

#[test]
fn a_read_only_view_selects_and_reads_while_another_edits() {
    let mut doc = sample();
    let mut views = [at(60, 20), at(30, 10).read_only(true)];
    let last = doc.text.line_to_char(4);
    views[1].selection = Selection::point(last + 4);
    views[0].selection = Selection::point(last);
    send(&mut doc, &mut views, 0, Msg::InsertText { text: ">> ".into() });
    assert_eq!(views[1].caret(), last + 7, "the panel's caret follows its text");
    assert_eq!(views[0].caret(), last + 3);
    let word = doc.text.line_to_char(2) + 5;
    send(&mut doc, &mut views, 1, Msg::SelectWordAt { pos: word });
    let r = views[1].selection.primary();
    assert_eq!(doc.text.slice(r.from()..r.to()).to_string(), "bullet", "a word selected in a read-only view");
}

#[test]
fn scroll_view_leaves_the_caret_and_the_next_motion_follows_it() {
    let text: String = (0..60).map(|i| format!("line {i}\n")).collect();
    let mut s = State::new(&text, None, Viewport { width: 40, height: 6 });
    update(&mut s, Msg::ScrollView { rows: 20 });
    assert_eq!(s.view.scroll.line, 20);
    assert_eq!(s.caret(), 0, "the caret stays");
    assert!(s.view.free);
    update(&mut s, Msg::Tick { now_ms: 10 });
    assert_eq!(s.view.scroll.line, 20, "a passive message doesn't bring it back");
    update(&mut s, Msg::Move { dir: Dir::Forward, by: By::Grapheme, extend: false });
    assert!(!s.view.free);
    assert_eq!(s.view.scroll.line, 0, "a caret motion follows the caret again");
    // Never past the end.
    update(&mut s, Msg::ScrollView { rows: 500 });
    assert_eq!(s.view.scroll.line, 61 - 5);
}

#[test]
fn typewriter_follow_keeps_the_caret_row() {
    let text: String = (0..60).map(|i| format!("line {i}\n")).collect();
    let mut s = State::new(&text, None, Viewport { width: 40, height: 21 });
    s.view.config.follow = caretline_next::Follow::Typewriter { percent: 50 };
    for _ in 0..30 {
        update(&mut s, Msg::Move { dir: Dir::Forward, by: By::Line, extend: false });
    }
    let f = view(&s);
    assert_eq!(f.cursor.map(|c| c.1), Some(10), "the caret's row is half the text rows");
    // The setting survives JSON.
    let back = State::from_json(&s.to_json()).unwrap();
    assert_eq!(back.view.config.follow, s.view.config.follow);
}
