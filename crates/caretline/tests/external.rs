//! Changes from elsewhere (`Msg::External`) and the history transform behind them.
//!
//! - `ChangeSet::map` converges: `A ∘ B.map(A) == B ∘ A.map(B, before)` on random edits.
//! - Undo after a change from elsewhere takes back only local edits: on random interleavings,
//!   undoing everything gives the first document with every change from elsewhere applied,
//!   marks included, and at every change `undo(after it) == it applied to undo(before it)`.
//! - Every view is mapped through it, keeps the editing invariants, and the save point and
//!   typing run stay on their revisions.

use caretline::helix::graphemes::ensure_grapheme_boundary_prev;
use caretline::helix::{ChangeSet, Rope, Selection, Tendril, Transaction};
use caretline::outline::markdown;
use caretline::{
    update, update_doc, view, By, Dir, Document, Effect, ExtChange, ExternalUndo, Kind, MarkId, Msg, NewBlock,
    OutlineConfig, State, View, Viewport,
};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

// ---------------------------------------------------------------------------------------
// ChangeSet::map

const PIECES: &[&str] = &["a", "bc", "日本", "👨‍👩‍👧", "\n", "xyz ", "é"];

fn random_changes(rng: &mut StdRng, doc: &Rope) -> ChangeSet {
    let len = doc.len_chars();
    let mut points: Vec<usize> = (0..rng.random_range(0..5)).map(|_| rng.random_range(0..=len)).collect();
    points.sort();
    let mut changes = Vec::new();
    let mut last = 0;
    for p in points.chunks(2) {
        let from = p[0].max(last);
        let to = if p.len() == 2 && rng.random_bool(0.5) { p[1].max(from) } else { from };
        let ins = rng.random_bool(0.7).then(|| Tendril::from(PIECES[rng.random_range(0..PIECES.len())]));
        changes.push((from, to, ins));
        last = to;
    }
    Transaction::change(doc, changes.into_iter()).changes().clone()
}

fn applied(doc: &Rope, cs: &ChangeSet) -> Rope {
    let mut d = doc.clone();
    assert!(cs.apply(&mut d));
    d
}

#[test]
fn map_converges_on_random_edits() {
    let mut rng = StdRng::seed_from_u64(7);
    for case in 0..3000 {
        let text: String = (0..rng.random_range(0..12)).map(|_| PIECES[rng.random_range(0..PIECES.len())]).collect();
        let doc = Rope::from(text.as_str());
        let a = random_changes(&mut rng, &doc);
        let b = random_changes(&mut rng, &doc);
        let b_after_a = b.clone().map(a.clone());
        let a_after_b = a.map_ordered(&b, true);
        assert_eq!(b_after_a.len(), a.len_after(), "case {case}");
        assert_eq!(a_after_b.len(), b.len_after(), "case {case}");
        let one = applied(&applied(&doc, &a), &b_after_a);
        let two = applied(&applied(&doc, &b), &a_after_b);
        assert_eq!(one, two, "case {case}: {text:?} a {a:?} b {b:?}");
        // Composed either way it is the same change.
        assert_eq!(applied(&doc, &a.clone().compose(b_after_a)), one);
    }
}

#[test]
fn map_of_far_apart_edits_shifts_positions() {
    let doc = Rope::from("hello world");
    let a = Transaction::change(&doc, [(0, 0, Some(Tendril::from(">> ")))].into_iter()).changes().clone();
    let b = Transaction::change(&doc, [(6, 11, Some(Tendril::from("there")))].into_iter()).changes().clone();
    assert_eq!(applied(&applied(&doc, &a), &b.clone().map(a.clone())).to_string(), ">> hello there");
    // Text deleted by one side stays deleted; text inserted inside it is kept.
    let del = Transaction::change(&doc, [(2, 9, None)].into_iter()).changes().clone();
    let ins = Transaction::change(&doc, [(5, 5, Some(Tendril::from("!")))].into_iter()).changes().clone();
    assert_eq!(applied(&applied(&doc, &del), &ins.clone().map(del.clone())).to_string(), "he!ld");
}

// ---------------------------------------------------------------------------------------
// Undo across changes from elsewhere

/// Blocks with even ids are edited here, odd ones from elsewhere.
fn local(id: MarkId) -> bool {
    id.0.is_multiple_of(2)
}

fn outline_doc(rng: &mut StdRng) -> State {
    let mut md = String::new();
    for i in 0..rng.random_range(2..10) {
        let line = match rng.random_range(0..4) {
            0 => format!("para {i} with words\n\n"),
            1 => format!("- item {i}\n"),
            2 => format!("  - [ ] task {i}\n"),
            _ => format!("1. number {i}\n"),
        };
        md.push_str(&line);
    }
    // A paragraph first, so nested items always have a parent.
    let md = format!("start here\n\n{md}");
    markdown::load(&md, None, Viewport { width: 40, height: 12 }, OutlineConfig::default())
}

/// A local edit inside a local block's content: typing, or deleting a span of it.
fn local_edit(rng: &mut StdRng, s: &mut State) -> bool {
    let o = s.blocks().unwrap();
    let mine: Vec<_> = o.blocks.iter().filter(|b| local(b.id) && b.line_count == 1).cloned().collect();
    if mine.is_empty() {
        return false;
    }
    let b = &mine[rng.random_range(0..mine.len())];
    let rope = s.doc.text.clone();
    let text = rope.slice(..);
    let a = ensure_grapheme_boundary_prev(text, rng.random_range(b.content_start()..=b.end));
    let now = s.doc.now_ms + if rng.random_bool(0.5) { 100 } else { 3000 };
    update(s, Msg::Tick { now_ms: now });
    let e = ensure_grapheme_boundary_prev(text, rng.random_range(a..=b.end));
    if rng.random_bool(0.6) || e <= a {
        s.view.selection = Selection::point(a);
        update(s, Msg::InsertText { text: ["x", "yz", "漢", " w"][rng.random_range(0..4)].into() });
    } else {
        // A selection inside the block's content: Backspace deletes just it.
        s.view.selection = Selection::single(a, e);
        update(s, Msg::DeleteBackward);
    }
    true
}

fn remote_change(rng: &mut StdRng, s: &State, next_id: &mut u64) -> Option<ExtChange> {
    let o = s.blocks().unwrap();
    let theirs: Vec<_> = o.blocks.iter().enumerate().filter(|(i, b)| *i > 0 && !local(b.id)).map(|(_, b)| b.clone()).collect();
    if theirs.is_empty() {
        *next_id += 2;
        return Some(ExtChange::InsertBlock {
            after: None,
            block: NewBlock { depth: 0, kind: Kind::Bullet, status: None, tag: None, text: "from elsewhere".into(), gap: None, mark: Some(MarkId(*next_id)) },
        });
    }
    let b = &theirs[rng.random_range(0..theirs.len())];
    Some(match rng.random_range(0..5) {
        0 | 1 => ExtChange::ReplaceContent { id: b.id, text: ["remote", "new words", "日本語", "a\nsecond line"][rng.random_range(0..4)].into() },
        2 => {
            *next_id += 2;
            ExtChange::InsertBlock {
                after: Some(b.id),
                block: NewBlock {
                    depth: b.depth,
                    kind: [Kind::Bullet, Kind::Task][rng.random_range(0..2)],
                    status: Some(' '),
                    tag: None,
                    text: "inserted".into(),
                    gap: None,
                    mark: Some(MarkId(*next_id)),
                },
            }
        }
        3 if b.kind != Kind::Para => ExtChange::SetShape { id: b.id, depth: b.depth, kind: Kind::Task, status: Some('x'), tag: None },
        _ if o.blocks.last().map(|l| l.id) != Some(b.id) => ExtChange::RemoveBlock { id: b.id },
        _ => ExtChange::SetGap { id: b.id, gap: Some(true) },
    })
}

fn external(s: &mut State, change: ExtChange) -> Vec<Effect> {
    update(s, Msg::External { changes: vec![change] })
}

fn undo_all(s: &mut State) {
    let mut guard = 0;
    while s.doc.history.current_revision() != 0 {
        update(s, Msg::Undo);
        guard += 1;
        assert!(guard < 10_000);
    }
}

#[test]
fn undo_after_changes_from_elsewhere_takes_back_only_local_edits() {
    for seed in 0..seeds(120) {
        let mut rng = StdRng::seed_from_u64(0xe7_0000 + seed);
        let mut s = outline_doc(&mut rng);
        // Ids from elsewhere are odd and above the document's own.
        let mut next_id = (s.doc.marks.next_id().0 + 101) | 1;
        let mut reference = s.clone();
        for step in 0..40 {
            let ctx = format!("seed {seed} step {step}");
            if rng.random_bool(0.6) {
                local_edit(&mut rng, &mut s);
                continue;
            }
            let Some(change) = remote_change(&mut rng, &s, &mut next_id) else { continue };
            // undo(after it) == it applied to undo(before it), on one step.
            if s.doc.history.current_revision() > 0 {
                let mut a = s.clone();
                external(&mut a, change.clone());
                update(&mut a, Msg::Undo);
                let mut b = s.clone();
                update(&mut b, Msg::Undo);
                external(&mut b, change.clone());
                assert_eq!(a.doc.text, b.doc.text, "{ctx}: {change:?} on {:?} last step {:?}", s.doc.text.to_string(), s.doc.history.current_transaction());
                assert_eq!(a.doc.marks.as_slice(), b.doc.marks.as_slice(), "{ctx}: marks after {change:?}");
            }
            let fx = external(&mut s, change.clone());
            assert!(fx.is_empty(), "{ctx}: {fx:?}");
            let fx = external(&mut reference, change.clone());
            assert!(fx.is_empty(), "{ctx}: reference {fx:?}");
            check(&s.doc, &s.view, &ctx);
        }
        undo_all(&mut s);
        assert_eq!(s.doc.text, reference.doc.text, "seed {seed}: undoing everything leaves exactly the changes from elsewhere");
        assert_eq!(s.doc.marks.as_slice(), reference.doc.marks.as_slice(), "seed {seed}: marks");
        // Redo goes forward again over the same local edits.
        while s.doc.history.can_redo() {
            update(&mut s, Msg::Redo);
            check(&s.doc, &s.view, &format!("seed {seed} redo"));
        }
    }
}

/// Every block start has a mark, ids are unique, and no selection end is inside a marker.
fn check(doc: &Document, v: &View, ctx: &str) {
    let o = doc.blocks().unwrap();
    let blocks: Vec<_> = o.blocks.iter().map(|b| (b.start, b.id)).collect();
    let marks: Vec<_> = doc.marks.iter().map(|m| (m.pos, m.id)).collect();
    assert_eq!(blocks, marks, "{ctx}: blocks and marks in {:?}", doc.text.to_string());
    let text = doc.text.slice(..);
    for r in v.selection.iter() {
        for pos in [r.anchor, r.head] {
            assert!(pos <= text.len_chars(), "{ctx}: past the end");
            assert_eq!(ensure_grapheme_boundary_prev(text, pos), pos, "{ctx}: inside a grapheme");
            let b = o.block_at(text, pos);
            assert!(!(b.prefix_len > 0 && pos >= b.start && pos < b.content_start()), "{ctx}: {pos} in a marker of {:?}", doc.text.to_string());
        }
    }
}

#[test]
fn two_views_and_random_changes_from_elsewhere_keep_the_invariants() {
    for seed in 0..seeds(80) {
        let mut rng = StdRng::seed_from_u64(0x2_0000 + seed);
        let base = outline_doc(&mut rng);
        let mut doc = base.doc.clone();
        let mut views = [base.view.clone(), View::new(Viewport { width: 20, height: 6 })];
        views[1].selection = Selection::point(doc.text.len_chars());
        let mut next_id = (doc.marks.next_id().0 + 101) | 1;
        for step in 0..60 {
            let ctx = format!("seed {seed} step {step}");
            let i = rng.random_range(0..2);
            let msg = match rng.random_range(0..10) {
                0..=3 => {
                    let st = State::from_parts(doc.clone(), views[0].clone());
                    match remote_change(&mut rng, &st, &mut next_id) {
                        Some(c) => Msg::External { changes: vec![c] },
                        None => continue,
                    }
                }
                4 => Msg::InsertText { text: "typed".into() },
                5 => Msg::InsertNewline,
                6 => Msg::DeleteBackward,
                7 => Msg::Undo,
                8 => Msg::Redo,
                _ => Msg::Move { dir: if rng.random_bool(0.5) { Dir::Forward } else { Dir::Backward }, by: By::VisualLine, extend: rng.random_bool(0.3) },
            };
            update_doc(&mut doc, &mut views, i, msg.clone());
            for (k, v) in views.iter().enumerate() {
                check(&doc, v, &format!("{ctx} {msg:?} view {k}"));
                let f = view(&State::from_parts(doc.clone(), v.clone()));
                assert_eq!(f.cells.len(), v.viewport.width as usize * v.viewport.height as usize);
            }
            if rng.random_range(0..10) == 0 {
                let s = State::from_parts(doc.clone(), views[0].clone());
                let back = State::from_json(&s.to_json()).unwrap();
                assert_eq!(back, s, "{ctx}: round trip");
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Behaviour

fn outline(md: &str) -> State {
    markdown::load(md, Some("x.md".into()), Viewport { width: 40, height: 10 }, OutlineConfig::default())
}

#[test]
fn a_change_from_elsewhere_maps_every_view() {
    let mut s = outline("- one\n- two three\n");
    let ids: Vec<MarkId> = s.doc.marks.iter().map(|m| m.id).collect();
    let mut doc = s.doc.clone();
    let mut views = [s.view.clone(), View::new(Viewport { width: 30, height: 5 })];
    views[0].selection = Selection::point(doc.text.line_to_char(1) + 6); // before "three"
    views[1].selection = Selection::point(doc.text.line_to_char(1) + 6);
    update_doc(
        &mut doc,
        &mut views,
        0,
        Msg::External { changes: vec![ExtChange::RemoveBlock { id: ids[0] }, ExtChange::ReplaceContent { id: ids[1], text: "two, and three".into() }] },
    );
    assert_eq!(doc.text.to_string(), "- two, and three");
    for v in &views {
        // Same block, still before "three": a new content changes only what differs.
        assert_eq!(doc.text.slice(v.caret()..).to_string(), "three");
    }
    s.doc = doc;
    assert_eq!(s.doc.marks.iter().map(|m| m.id).collect::<Vec<_>>(), vec![ids[1]]);
}

#[test]
fn a_view_before_the_change_stays_on_its_text() {
    let mut s = State::new("hello world", None, Viewport { width: 40, height: 5 });
    s.view.selection = Selection::point(6); // before "world"
    update(&mut s, Msg::External { changes: vec![ExtChange::Replace { from: 0, to: 0, text: "say ".into() }] });
    assert_eq!(s.doc.text.to_string(), "say hello world");
    assert_eq!(s.caret(), 10, "still before world");
}

#[test]
fn undo_leaves_a_change_from_elsewhere() {
    let mut s = outline("- one\n- two\n");
    let two = s.doc.marks.iter().nth(1).unwrap().id;
    s.view.selection = Selection::point(5);
    update(&mut s, Msg::InsertText { text: "!".into() });
    assert_eq!(s.doc.text.to_string(), "- one!\n- two");
    let fx = update(&mut s, Msg::External { changes: vec![ExtChange::ReplaceContent { id: two, text: "TWO".into() }] });
    assert!(fx.is_empty());
    assert_eq!(s.doc.text.to_string(), "- one!\n- TWO");
    assert!(s.doc.dirty, "the local edit is still unsaved");
    update(&mut s, Msg::Undo);
    assert_eq!(s.doc.text.to_string(), "- one\n- TWO", "undo took back only the local edit");
    assert!(!s.doc.dirty, "back at the save point: the change from elsewhere isn't ours to save");
    update(&mut s, Msg::Redo);
    assert_eq!(s.doc.text.to_string(), "- one!\n- TWO");
}

#[test]
fn a_typing_run_continues_across_a_change_from_elsewhere() {
    let mut s = State::new("abc\nxyz", None, Viewport { width: 40, height: 5 });
    s.view.selection = Selection::point(3);
    update(&mut s, Msg::InsertText { text: "d".into() });
    update(&mut s, Msg::External { changes: vec![ExtChange::Replace { from: 5, to: 8, text: "XYZ".into() }] });
    update(&mut s, Msg::InsertText { text: "e".into() });
    assert_eq!(s.doc.text.to_string(), "abcde\nXYZ");
    assert_eq!(s.doc.history.len(), 2, "one typing run, one revision");
    update(&mut s, Msg::Undo);
    assert_eq!(s.doc.text.to_string(), "abc\nXYZ");
}

#[test]
fn an_overlapping_change_from_elsewhere_wins() {
    let mut s = State::new("one two", None, Viewport { width: 40, height: 5 });
    s.view.selection = Selection::point(7);
    update(&mut s, Msg::InsertText { text: " three".into() });
    // Elsewhere, the whole line was replaced (our words included).
    update(&mut s, Msg::External { changes: vec![ExtChange::Replace { from: 0, to: 13, text: "replaced".into() }] });
    update(&mut s, Msg::Undo);
    assert_eq!(s.doc.text.to_string(), "replaced", "undo can't bring back what elsewhere deleted");
}

#[test]
fn redo_past_a_change_from_elsewhere_is_gone() {
    let mut s = State::new("abc", None, Viewport { width: 40, height: 5 });
    s.view.selection = Selection::point(3);
    update(&mut s, Msg::InsertText { text: "d".into() });
    update(&mut s, Msg::Undo);
    assert!(s.doc.history.can_redo());
    update(&mut s, Msg::External { changes: vec![ExtChange::Replace { from: 0, to: 0, text: ">".into() }] });
    assert!(!s.doc.history.can_redo());
    update(&mut s, Msg::Redo);
    assert_eq!(s.view.status.as_deref(), Some("nothing to redo"));
}

#[test]
fn the_barrier_fallback_stops_undo_at_the_change() {
    let mut s = State::new("abc", None, Viewport { width: 40, height: 5 });
    s.doc.config.external_undo = ExternalUndo::Barrier;
    s.view.selection = Selection::point(3);
    update(&mut s, Msg::InsertText { text: "d".into() });
    update(&mut s, Msg::External { changes: vec![ExtChange::Replace { from: 0, to: 0, text: ">".into() }] });
    update(&mut s, Msg::Undo);
    assert_eq!(s.doc.text.to_string(), ">abcd");
    assert_eq!(s.view.status.as_deref(), Some("undo stops at a change from elsewhere"));
    // The setting and the floor survive JSON.
    let back = State::from_json(&s.to_json()).unwrap();
    assert_eq!(back, s);
    assert!(s.to_json().contains("\"external_undo\": \"barrier\""));
}

#[test]
fn set_shape_rewrites_the_marker_and_keeps_the_content() {
    let mut s = outline("- one\n1. two\n");
    let ids: Vec<MarkId> = s.doc.marks.iter().map(|m| m.id).collect();
    update(
        &mut s,
        Msg::External {
            changes: vec![
                ExtChange::SetShape { id: ids[0], depth: 0, kind: Kind::Task, status: Some('x'), tag: None },
                ExtChange::SetShape { id: ids[1], depth: 1, kind: Kind::Bullet, status: None, tag: None },
            ],
        },
    );
    assert_eq!(s.doc.text.to_string(), "- [x] one\n  1. two");
    assert_eq!(s.doc.marks.iter().map(|m| m.id).collect::<Vec<_>>(), ids);
}

#[test]
fn an_inserted_block_takes_the_hosts_id() {
    let mut s = outline("- one\n- two\n");
    let one = s.doc.marks.iter().next().unwrap().id;
    let nb = NewBlock { depth: 1, kind: Kind::Task, status: Some(' '), tag: None, text: "new".into(), gap: None, mark: Some(MarkId(500)) };
    update(&mut s, Msg::External { changes: vec![ExtChange::InsertBlock { after: Some(one), block: nb }] });
    assert_eq!(s.doc.text.to_string(), "- one\n  - [ ] new\n- two");
    assert_eq!(s.doc.marks.iter().nth(1).unwrap().id, MarkId(500));
    // A change to a block that isn't there is skipped and said.
    let fx = update(&mut s, Msg::External { changes: vec![ExtChange::RemoveBlock { id: MarkId(9) }] });
    assert_eq!(fx, vec![Effect::Notice { text: "a change from elsewhere was skipped: no block 9".into() }]);
}

#[test]
fn external_changes_are_plain_json() {
    let msg: Msg = serde_json::from_str(
        r#"{"msg":"external","changes":[{"change":"replace_content","id":3,"text":"hi"},{"change":"replace","from":0,"to":2,"text":"x"},{"change":"insert_block","after":3,"block":{"kind":"task","status":" ","text":"new","mark":12}}]}"#,
    )
    .unwrap();
    let Msg::External { changes } = &msg else { panic!() };
    assert_eq!(changes.len(), 3);
    assert!(msg.is_passive());
    assert!(!msg.edits(), "a read-only view still takes changes from elsewhere");
    let back: Msg = serde_json::from_str(&serde_json::to_string(&msg).unwrap()).unwrap();
    assert_eq!(back, msg);
}

#[test]
fn a_read_only_view_takes_changes_from_elsewhere() {
    let mut s = State::new("abc", None, Viewport { width: 40, height: 5 });
    s.view.read_only = true;
    s.view.selection = Selection::point(3);
    update(&mut s, Msg::External { changes: vec![ExtChange::Replace { from: 0, to: 0, text: ">".into() }] });
    assert_eq!(s.doc.text.to_string(), ">abc");
    assert_eq!(s.caret(), 4);
}

/// A transform walks the whole undo history; a long one stays cheap (release build).
#[test]
fn a_change_from_elsewhere_over_a_long_history() {
    let mut s = State::new("", None, Viewport { width: 80, height: 24 });
    let steps = if cfg!(debug_assertions) { 300 } else { 5000 };
    for i in 0..steps {
        update(&mut s, Msg::Tick { now_ms: i * 2000 });
        update(&mut s, Msg::InsertText { text: format!("word {i} ") });
    }
    assert_eq!(s.doc.history.len(), steps as usize + 1);
    let t = std::time::Instant::now();
    update(&mut s, Msg::External { changes: vec![ExtChange::Replace { from: 0, to: 0, text: "start ".into() }] });
    let took = t.elapsed();
    eprintln!("transform over {steps} revisions: {took:?}");
    undo_all(&mut s);
    assert_eq!(s.doc.text.to_string(), "start ");
    if !cfg!(debug_assertions) {
        assert!(took < std::time::Duration::from_millis(100), "{took:?}");
    }
}

/// `CARETLINE_EXT_SEEDS` runs more seeds than the default.
fn seeds(default: u64) -> u64 {
    std::env::var("CARETLINE_EXT_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

#[test]
fn a_change_from_elsewhere_at_a_caret_goes_after_it() {
    let mut doc = State::new("Hey there, \nnext\n", None, Viewport { width: 60, height: 5 }).doc;
    let mut views = [View::new(Viewport { width: 60, height: 5 }), View::new(Viewport { width: 60, height: 5 })];
    views[0].selection = Selection::point(11);
    views[1].selection = Selection::single(4, 11); // "there, "
    let ins = ExtChange::Replace { from: 11, to: 11, text: "\n- Agent note: safely.".into() };
    update_doc(&mut doc, &mut views, 1, Msg::External { changes: vec![ins] });
    assert_eq!(views[0].caret(), 11, "a caret at the insertion point stays before it");
    let r = views[1].selection.primary();
    assert_eq!((r.from(), r.to()), (4, 11), "a selection ending there keeps what it covered");
    update_doc(&mut doc, &mut views, 0, Msg::InsertText { text: "w".into() });
    assert_eq!(doc.text.to_string(), "Hey there, w\n- Agent note: safely.\nnext\n");
    // Undo takes back only the person's typing.
    update_doc(&mut doc, &mut views, 0, Msg::Undo);
    assert_eq!(doc.text.to_string(), "Hey there, \n- Agent note: safely.\nnext\n");
}

/// A change from elsewhere inside one block's lines is that block's: removing the break
/// between its two empty lines (`"\n\n"` → `"\n"`) leaves it its mark, though that break,
/// read alone, is a whole line.
#[test]
fn a_change_inside_a_block_keeps_its_mark() {
    let mut s = State::new("\n\n", None, Viewport { width: 40, height: 10 });
    s.doc.marks.insert(caretline::Mark { pos: 0, id: MarkId(1), attrs: Default::default() }).unwrap();
    s.doc.marks.insert(caretline::Mark { pos: 1, id: MarkId(0), attrs: Default::default() }).unwrap();
    s.doc.outline = Some(OutlineConfig::default());
    s.outline_changed();
    let ids = |s: &State| s.blocks().unwrap().blocks.iter().map(|b| (b.id.0, b.start, b.end)).collect::<Vec<_>>();
    assert_eq!(ids(&s), [(1, 0, 0), (0, 1, 2)]);
    s.view.selection = Selection::point(2);
    external(&mut s, ExtChange::Replace { from: 1, to: 2, text: String::new() });
    assert_eq!(ids(&s), [(1, 0, 0), (0, 1, 1)]);
    // The same through the block's own change.
    let mut s = State::new("\n\n", None, Viewport { width: 40, height: 10 });
    s.doc.marks.insert(caretline::Mark { pos: 0, id: MarkId(1), attrs: Default::default() }).unwrap();
    s.doc.marks.insert(caretline::Mark { pos: 1, id: MarkId(0), attrs: Default::default() }).unwrap();
    s.doc.outline = Some(OutlineConfig::default());
    s.outline_changed();
    external(&mut s, ExtChange::ReplaceContent { id: MarkId(0), text: String::new() });
    assert_eq!(ids(&s), [(1, 0, 0), (0, 1, 1)]);
}
