//! Outline properties: random outline documents and random messages (seeded), with the block
//! invariants checked after every step.
//!
//! - Every block start has a mark, every mark starts a block, and ids are unique.
//! - No caret is inside a marker, and no caret is strictly inside an atomic (image) block.
//! - Undo after an edit restores text, selection and marks exactly; redo re-applies them;
//!   undoing everything gives back the first document and marks.
//! - The task cycle, Tab and Shift-Tab never change another block's blank row, and undo
//!   restores every blank row.
//! - Cut then paste in place gives back the same text and the same ids.
//! - The state survives JSON, and `view` never panics.

mod common;

use std::collections::{HashMap, HashSet};

use caretline::helix::graphemes::ensure_grapheme_boundary_prev;
use caretline::helix::Selection;
use caretline::outline::markdown;
use caretline::{update, view, By, Dir, Kind, MarkId, Msg, NewBlock, OutlineConfig, State, Viewport};
use common::gen;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

const WORDS: &[&str] = &["one", "two", "日本", "é", "👨‍👩‍👧", "x.", "longer words here", "a"];

/// A random outline document, written as Markdown and loaded.
fn random_doc(rng: &mut StdRng) -> State {
    let mut md = String::new();
    let mut depth = 0usize;
    for i in 0..rng.random_range(1..9) {
        if i > 0 && rng.random_bool(0.4) {
            md.push('\n');
        }
        let words: Vec<&str> = (0..rng.random_range(0..4)).map(|_| WORDS[rng.random_range(0..WORDS.len())]).collect();
        let text = words.join(" ");
        depth = match rng.random_range(0..3) {
            0 => 0,
            1 => depth,
            _ => depth + 1,
        }
        .min(2);
        let pad = "  ".repeat(depth);
        let line = match rng.random_range(0..11) {
            0..=2 => {
                let more = if rng.random_bool(0.3) { format!("\n{}", WORDS[rng.random_range(0..WORDS.len())]) } else { String::new() };
                format!("{text}{more}")
            }
            3 | 4 => format!("{pad}- {text}"),
            5 | 6 => format!("{pad}- [{}] {text}", [' ', 'x', '/'][rng.random_range(0..3)]),
            7 => format!("{pad}{}. {text}", rng.random_range(1..20)),
            8 => format!("## {text}"),
            9 => "![shot](files/a.png)".to_string(),
            _ => format!("```\n{text}\n- in a fence\n```"),
        };
        md.push_str(&line);
        md.push('\n');
    }
    let (w, h) = match rng.random_range(0..4) {
        0 => (rng.random_range(4..20), rng.random_range(2..6)),
        _ => (rng.random_range(20..100), rng.random_range(4..30)),
    };
    let mut s = markdown::load(&md, Some("fuzz.md".into()), Viewport { width: w, height: h }, OutlineConfig::default());
    let len = s.doc.text.len_chars();
    let t = s.doc.text.slice(..);
    let a = ensure_grapheme_boundary_prev(t, rng.random_range(0..=len));
    let b = if rng.random_bool(0.5) { a } else { ensure_grapheme_boundary_prev(t, rng.random_range(0..=len)) };
    s.view.selection = Selection::single(a, b);
    update(&mut s, Msg::Resize { width: w, height: h });
    s
}

const PIECES: &[&str] = &["- ", "[ ] ", "# ", "```", "1. ", "x", "word ", "  ", "\n", "日", "👍🏽", "![a](b)"];

fn outline_msg(rng: &mut StdRng, s: &State) -> Msg {
    let blocks = s.blocks().unwrap();
    let id = blocks.blocks[rng.random_range(0..blocks.blocks.len())].id;
    let dir = if rng.random_bool(0.5) { Dir::Forward } else { Dir::Backward };
    match rng.random_range(0..40) {
        0..=3 => Msg::InsertText { text: PIECES[rng.random_range(0..PIECES.len())].into() },
        4..=5 => Msg::SoftBreak,
        6..=8 => Msg::Indent,
        9..=10 => Msg::Outdent,
        11..=14 => Msg::TaskCycle,
        15..=16 => Msg::MoveBlock { dir },
        17 => Msg::SetStatus { id, ch: ['x', ' ', '/'][rng.random_range(0..3)] },
        18 => Msg::SelectBlock { id },
        19 => Msg::SelectWordAt { pos: rng.random_range(0..=s.doc.text.len_chars()) },
        20 => Msg::InsertBlocks {
            after: rng.random_bool(0.8).then_some(id),
            blocks: vec![NewBlock { depth: 0, kind: Kind::Bullet, status: None, tag: None, text: "new".into(), gap: None, mark: None }],
        },
        21 => Msg::Paste { text: Some("- a\n  - [ ] b\n\npara".into()) },
        22 => Msg::PastePlain { text: Some("one\ntwo\n\nthree".into()) },
        23..=25 => Msg::Move { dir, by: By::Block, extend: rng.random_bool(0.3) },
        26..=29 => Msg::InsertNewline,
        _ => Msg::DeleteBackward,
    }
}

fn check(s: &State, ctx: &str) {
    let o = s.blocks().expect("outline");
    let text = s.doc.text.slice(..);
    // Marks and blocks agree.
    let block_ids: Vec<(usize, MarkId)> = o.blocks.iter().map(|b| (b.start, b.id)).collect();
    let mark_ids: Vec<(usize, MarkId)> = s.doc.marks.iter().map(|m| (m.pos, m.id)).collect();
    assert_eq!(block_ids, mark_ids, "{ctx}: blocks and marks differ in {:?}", s.doc.text.to_string());
    let mut seen = HashSet::new();
    for (_, id) in &mark_ids {
        assert!(seen.insert(*id), "{ctx}: id {id:?} twice");
    }
    // The caret invariant.
    for r in s.view.selection.iter() {
        for (end, pos) in [("anchor", r.anchor), ("head", r.head)] {
            assert!(pos <= text.len_chars(), "{ctx}: {end} past the end");
            let b = o.block_at(text, pos);
            assert!(
                !(b.prefix_len > 0 && pos >= b.start && pos < b.content_start()),
                "{ctx}: {end} {pos} inside the marker of {:?} in {:?}",
                b,
                s.doc.text.to_string()
            );
        }
        if r.is_empty() {
            let b = o.block_at(text, r.head);
            assert!(
                !(b.atomic && r.head >= b.content_start() && r.head <= b.end),
                "{ctx}: a caret on an image in {:?}",
                s.doc.text.to_string()
            );
        }
    }
}

fn gaps(s: &State) -> HashMap<MarkId, bool> {
    s.blocks().unwrap().blocks.iter().map(|b| (b.id, b.gap)).collect()
}

fn run_seed(seed: u64) -> usize {
    let mut rng = StdRng::seed_from_u64(0x0b1e ^ seed.wrapping_mul(0x9e37_79b9));
    let mut s = random_doc(&mut rng);
    let original = (s.doc.text.to_string(), s.doc.marks.clone());
    check(&s, &format!("seed {seed} start"));
    let mut steps = 0;
    for step in 0..300 {
        let msg = if rng.random_bool(0.5) { outline_msg(&mut rng, &s) } else { gen::msg(&mut rng, &s) };
        // Quitting and saving have nothing outline-specific to check.
        if matches!(msg, Msg::Quit | Msg::Save) {
            continue;
        }
        let before = s.clone();
        update(&mut s, msg.clone());
        steps += 1;
        let ctx = format!("seed {seed} step {step} {msg:?}");
        check(&s, &ctx);

        // A motion without Shift leaves no selection, except a selected image (one unit).
        if let Msg::Move { extend: false, .. } = msg {
            let o = s.blocks().unwrap();
            for r in s.view.selection.iter().filter(|r| !r.is_empty()) {
                let b = o.block_at(s.doc.text.slice(..), r.from());
                assert!(b.atomic && r.from() == b.content_start() && r.to() == b.end, "{ctx}: a motion left a selection");
            }
        }
        let new_revision = s.doc.history.len() == before.doc.history.len() + 1;
        if msg.edits() && new_revision && !matches!(msg, Msg::Undo | Msg::Redo) {
            let mut undone = s.clone();
            update(&mut undone, Msg::Undo);
            assert_eq!(undone.doc.text, before.doc.text, "{ctx}: undo text");
            assert_eq!(undone.doc.marks.as_slice(), before.doc.marks.as_slice(), "{ctx}: undo marks");
            assert_eq!(undone.view.selection, before.view.selection, "{ctx}: undo selection");
            update(&mut undone, Msg::Redo);
            assert_eq!(undone.doc.text, s.doc.text, "{ctx}: redo text");
            assert_eq!(undone.doc.marks.as_slice(), s.doc.marks.as_slice(), "{ctx}: redo marks");
        }
        // A kind or depth change moves no other block.
        if matches!(msg, Msg::TaskCycle | Msg::Indent | Msg::Outdent) && before.view.selection.primary().is_empty() {
            let (g0, g1) = (gaps(&before), gaps(&s));
            let caret_block = before.blocks().unwrap().block_at(before.doc.text.slice(..), before.caret()).id;
            let first = s.blocks().unwrap().blocks[0].id;
            for (id, g) in &g1 {
                if *id != caret_block && *id != first {
                    if let Some(was) = g0.get(id) {
                        assert_eq!(was, g, "{ctx}: block {id:?}'s blank row moved in {:?}", s.doc.text.to_string());
                    }
                }
            }
        }
        if rng.random_range(0..25) == 0 {
            let back = State::from_json(&s.to_json()).unwrap();
            assert_eq!(back, s, "{ctx}: round trip");
            assert_eq!(view(&back), view(&s), "{ctx}: round-trip frame");
        }
        let frame = view(&s);
        assert_eq!(frame.cells.len(), s.view.viewport.width as usize * s.view.viewport.height as usize);
    }
    let mut guard = 0;
    while s.doc.history.current_revision() != 0 {
        update(&mut s, Msg::Undo);
        guard += 1;
        assert!(guard < 10_000, "seed {seed}: undo never reached the root");
    }
    assert_eq!(s.doc.text.to_string(), original.0, "seed {seed}: full undo text");
    assert_eq!(s.doc.marks.as_slice(), original.1.as_slice(), "seed {seed}: full undo marks");
    check(&s, &format!("seed {seed} after full undo"));
    steps
}

#[test]
fn random_outline_sessions_keep_every_invariant() {
    let seeds: u64 = std::env::var("CARETLINE_OUTLINE_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(40);
    let steps: usize = (0..seeds).map(run_seed).sum();
    assert!(steps > 1000);
}

/// Cut then paste at the same place: the same text and the same ids.
#[test]
fn cut_and_paste_in_place_keeps_text_and_ids() {
    let mut rng = StdRng::seed_from_u64(77);
    let mut tested = 0;
    for case in 0..300 {
        let mut s = random_doc(&mut rng);
        if s.view.selection.primary().is_empty() {
            continue;
        }
        let before = s.clone();
        let r = before.view.selection.primary();
        update(&mut s, Msg::Cut);
        if s.caret() != r.from() {
            // The cut left the caret's place inside a marker (the text after it starts with
            // spaces or a marker), so the caret moved: there is no "same place" to paste at.
            continue;
        }
        if s.doc.marks.iter().any(|m| !before.doc.marks.contains(m.id)) {
            // What remained has new blocks (the cut closed a fence, and marker lines after it
            // started blocks): they keep their ids after the paste.
            continue;
        }
        update(&mut s, Msg::Paste { text: None });
        assert_eq!(s.doc.text, before.doc.text, "case {case}: text after cutting {}..{} of {:?}", r.from(), r.to(), before.doc.text.to_string());
        assert_eq!(s.doc.marks.as_slice(), before.doc.marks.as_slice(), "case {case}: ids after cutting {}..{} of {:?}", r.from(), r.to(), before.doc.text.to_string());
        tested += 1;
    }
    assert!(tested > 60, "only {tested} cases");
}

/// Select all, delete: one empty block; undo: everything back, ids too.
#[test]
fn select_all_delete_then_undo_restores_the_ids() {
    let mut rng = StdRng::seed_from_u64(12);
    for _ in 0..200 {
        let mut s = random_doc(&mut rng);
        let before = s.clone();
        update(&mut s, Msg::SelectAll);
        let selected = s.view.selection.clone();
        update(&mut s, Msg::DeleteBackward);
        assert_eq!(s.blocks().unwrap().blocks.len(), 1, "{:?}", s.doc.text.to_string());
        update(&mut s, Msg::Undo);
        assert_eq!(s.doc.text, before.doc.text);
        assert_eq!(s.doc.marks.as_slice(), before.doc.marks.as_slice());
        assert_eq!(s.view.selection, selected);
    }
}

/// A document written to Markdown and read back has the same blocks and blank rows.
#[test]
fn markdown_files_round_trip() {
    let mut rng = StdRng::seed_from_u64(80);
    for _ in 0..300 {
        let s = random_doc(&mut rng);
        let md = markdown::to_file(&s);
        let back = markdown::load(&md, None, Viewport { width: 80, height: 24 }, OutlineConfig::default());
        let shape = |s: &State| -> Vec<(Kind, u16, bool, String)> {
            let o = s.blocks().unwrap();
            o.blocks
                .iter()
                .filter(|b| !(b.kind == Kind::Para && b.is_empty() && b.line_count == 1 && b.hang == caretline::outline::Hang::None))
                .map(|b| (b.kind, b.depth, b.gap, s.doc.text.slice(b.content_start()..b.end).to_string()))
                .collect()
        };
        assert_eq!(shape(&back), shape(&s), "{md:?}");
        assert_eq!(markdown::to_file(&back), md);
    }
}
