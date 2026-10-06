//! Doc + View (engine-extraction.md §1a): two views on one doc share text and undo; edits
//! rebase the other views; read-only is refused in the engine; a view lays out into any rect.

use caretline::buffer::BlockLine;
use caretline::doc::{Doc, Rect, Refused, View};
use caretline::{Block, Command, Kind, Motion, Pos};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

/// A host line with nothing beside the block.
#[derive(Clone, Debug, PartialEq)]
struct L {
    b: Block<u64>,
    new: bool,
}

impl std::ops::Deref for L {
    type Target = Block<u64>;
    fn deref(&self) -> &Block<u64> {
        &self.b
    }
}
impl std::ops::DerefMut for L {
    fn deref_mut(&mut self) -> &mut Block<u64> {
        &mut self.b
    }
}

impl BlockLine for L {
    type Id = u64;
    fn fresh(depth: usize, kind: Kind, text: &str) -> Self {
        L { b: Block::new(NEXT.fetch_add(1, Ordering::Relaxed), depth, kind, text), new: true }
    }
    fn is_new(&self) -> bool {
        self.new
    }
    fn keep_host_state(&mut self, _cur: &Self) {}
    fn revive(&mut self) {
        self.new = true;
    }
}

fn doc(texts: &[(Kind, usize, &str)]) -> Doc<L> {
    Doc::new(texts.iter().map(|(k, d, t)| {
        let mut l = L::fresh(*d, *k, t);
        l.new = false;
        l
    }).collect())
}

fn texts(d: &Doc<L>) -> Vec<String> {
    d.lines().iter().map(|l| l.text.clone()).collect()
}

fn valid(d: &Doc<L>, v: &View<u64>) -> bool {
    let ok = |p: Pos| d.lines().get(p.line).is_some_and(|l| p.byte <= l.text.len() && l.text.is_char_boundary(p.byte));
    ok(v.caret) && v.anchor.is_none_or(ok)
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const W: fn(&L) -> usize = |_| 40;

fn sample() -> Doc<L> {
    doc(&[(Kind::Para, 0, "First line of a paragraph\nand its second line"), (Kind::Bullet, 0, "a bullet"), (Kind::Task, 1, "a task under it"), (Kind::Para, 0, "Last, with ünïcode 漢字 and 🙂")])
}

#[test]
fn two_views_on_one_doc_share_text_and_undo() {
    let cmds = [
        Command::Newline,
        Command::SoftBreak,
        Command::Backspace,
        Command::Delete,
        Command::DeleteWordBack,
        Command::KillToEnd,
        Command::Indent,
        Command::Outdent,
        Command::TaskCycle,
        Command::MoveLine(1),
        Command::MoveLine(-1),
        Command::Move { motion: Motion::Down, select: false },
        Command::Move { motion: Motion::Up, select: false },
        Command::Move { motion: Motion::WordRight, select: true },
        Command::Move { motion: Motion::End, select: false },
        Command::Move { motion: Motion::DocEnd, select: false },
    ];
    let mut r = Rng(0x2545F4914F6CDD1D);
    for case in 0..200 {
        let mut d = sample();
        let mut a = View::new(Rect { x: 0, y: 0, width: 60, height: 20 });
        let mut b = View::new(Rect { x: 0, y: 0, width: 24, height: 8 });
        b.caret = Pos { line: 3, byte: 5 };
        let mut history = vec![texts(&d)];
        for step in 0..30 {
            let before = texts(&d);
            let use_a = r.below(2) == 0;
            let (acting, other) = if use_a { (&mut a, &mut b) } else { (&mut b, &mut a) };
            if r.below(5) == 0 {
                d.insert_and_rebase(acting, [&mut *other], "xé").unwrap();
            } else {
                let c = cmds[r.below(cmds.len())];
                d.apply_and_rebase(acting, [&mut *other], c, &W).unwrap();
            }
            assert!(valid(&d, &a) && valid(&d, &b), "case {case} step {step}: a {:?} b {:?} in {:?}", a.caret, b.caret, texts(&d));
            // Undo, from the other view, goes back to an earlier state of the one doc (typing runs
            // undo as one step, so maybe further than this step).
            if texts(&d) != before && r.below(4) == 0 {
                let (undoer, rest) = if use_a { (&mut b, &mut a) } else { (&mut a, &mut b) };
                d.apply_and_rebase(undoer, [&mut *rest], Command::Undo, &W).unwrap();
                assert!(history.contains(&texts(&d)), "case {case} step {step}: undo from the other view gave {:?}", texts(&d));
                assert!(valid(&d, &a) && valid(&d, &b));
            }
            history.push(texts(&d));
        }
    }
}

#[test]
fn a_view_follows_an_edit_made_through_another() {
    let mut d = doc(&[(Kind::Para, 0, "hello world")]);
    let mut a = View::new(Rect { x: 0, y: 0, width: 40, height: 5 });
    let mut b = View::new(Rect { x: 0, y: 0, width: 40, height: 5 });
    b.caret = Pos { line: 0, byte: 6 }; // before "world"
    // Typing before it moves it right; typing after it doesn't.
    d.insert_and_rebase(&mut a, [&mut b], "say ").unwrap();
    assert_eq!(texts(&d), vec!["say hello world"]);
    assert_eq!(b.caret, Pos { line: 0, byte: 10 }, "still before world");
    a.caret = Pos { line: 0, byte: 15 };
    d.insert_and_rebase(&mut a, [&mut b], "!").unwrap();
    assert_eq!(b.caret, Pos { line: 0, byte: 10 });
    // A new block above it: it keeps its block (by identity), one line down.
    a.caret = Pos { line: 0, byte: 0 };
    d.apply_and_rebase(&mut a, [&mut b], Command::Newline, &W).unwrap();
    assert_eq!(d.lines()[b.caret.line].text, "say hello world!");
    assert_eq!(b.caret.byte, 10);
}

#[test]
fn a_read_only_view_is_refused_every_edit_and_still_moves() {
    let edits = [
        Command::Newline,
        Command::SoftBreak,
        Command::Backspace,
        Command::Delete,
        Command::DeleteWordBack,
        Command::KillToEnd,
        Command::KillToStart,
        Command::Indent,
        Command::Outdent,
        Command::TaskCycle,
        Command::MoveLine(1),
        Command::Undo,
        Command::Redo,
    ];
    let mut d = sample();
    let mut w = View::new(Rect { x: 0, y: 0, width: 60, height: 20 });
    d.insert(&mut w, "seed ").unwrap();
    let mut ro = View::new(Rect { x: 0, y: 0, width: 30, height: 10 }).read_only(true);
    ro.caret = Pos { line: 1, byte: 2 };
    let before = texts(&d);
    let depth = d.undo_depth();
    for c in edits {
        assert_eq!(d.apply(&mut ro, c, &W).err(), Some(Refused::ReadOnly), "{c:?}");
        assert_eq!(texts(&d), before, "{c:?} changed nothing");
        assert_eq!(d.undo_depth(), depth, "{c:?} left undo alone");
    }
    assert_eq!(d.insert(&mut ro, "x").err(), Some(Refused::ReadOnly));
    // Motion, selection still work.
    let c0 = ro.caret;
    d.apply(&mut ro, Command::Move { motion: Motion::Right, select: false }, &W).unwrap();
    assert_ne!(ro.caret, c0, "→ moves");
    d.apply(&mut ro, Command::SelectAll, &W).unwrap();
    assert!(ro.anchor.is_some(), "select all selects");
    assert_eq!(texts(&d), before);
}

#[test]
fn a_view_lays_out_into_any_rect() {
    let d = sample();
    for (w, h) in [(0u16, 0u16), (1, 1), (12, 4), (40, 10), (200, 60)] {
        let mut v = View::new(Rect { x: 3, y: 2, width: w, height: h });
        v.caret = Pos { line: 3, byte: d.lines()[3].text.len() };
        let rows = d.layout(&v);
        assert!(!rows.is_empty(), "{w}x{h}");
        for r in &rows {
            let t = &d.lines()[r.line].text[r.start..r.end];
            // (A row of one grapheme wider than the rect can't be helped: 漢 in one column.)
            let one = unicode_segmentation::UnicodeSegmentation::graphemes(t.trim_end(), true).count() <= 1;
            assert!(w == 0 || one || r.x + caretline::width(t.trim_end()) <= w as usize, "{w}x{h}: {t:?} at {} overflows", r.x);
        }
        d.scroll_to_caret(&mut v);
        let row = d.caret_row(&v).unwrap();
        if h > 0 {
            assert!(row >= v.scroll && row < v.scroll + h as usize, "{w}x{h}: caret row {row}, scroll {}", v.scroll);
        }
    }
}

#[test]
fn every_view_keeps_the_editing_invariants() {
    // EI1 (a motion without ⇧ leaves no selection) and valid carets, in views of every width.
    let motions = [Motion::Left, Motion::Right, Motion::Up, Motion::Down, Motion::WordLeft, Motion::WordRight, Motion::Home, Motion::End, Motion::DocStart, Motion::DocEnd];
    let mut r = Rng(0x9E3779B97F4A7C15);
    for w in [8u16, 20, 60] {
        let mut d = sample();
        let mut v = View::new(Rect { x: 0, y: 0, width: w, height: 10 });
        for _ in 0..300 {
            let m = motions[r.below(motions.len())];
            let select = r.below(3) == 0;
            d.apply(&mut v, Command::Move { motion: m, select }, &W).unwrap();
            if !select {
                assert!(v.anchor.is_none(), "EI1 at width {w}: {m:?}");
            }
            assert!(valid(&d, &v), "width {w}: {:?}", v.caret);
        }
    }
}

#[test]
fn folds_belong_to_the_view() {
    let d = sample();
    let mut a = View::new(Rect { x: 0, y: 0, width: 60, height: 20 });
    let b = View::new(Rect { x: 0, y: 0, width: 60, height: 20 });
    a.folds.insert(d.lines()[1].id);
    assert!(d.layout(&a).iter().all(|r| r.line != 2), "a hides the task under its folded bullet");
    assert!(d.layout(&b).iter().any(|r| r.line == 2), "b, on the same doc, shows it");
}

#[test]
fn a_remote_edit_rebases_every_view_the_same_way() {
    let mut d = doc(&[(Kind::Para, 0, "one"), (Kind::Para, 0, "two three")]);
    let mut a = View::new(Rect { x: 0, y: 0, width: 40, height: 5 });
    a.caret = Pos { line: 1, byte: 4 }; // before "three"
    let edit = d.external_edit(|lines| {
        lines.remove(0);
        lines[0].text = "two, and three".into();
    });
    d.rebase(&mut a, &edit);
    assert_eq!(a.caret, Pos { line: 0, byte: 9 }, "same block, still before three");
}

#[test]
fn anchors_survive_edits_and_find_their_block() {
    let mut d = doc(&[(Kind::Para, 0, "one 漢字 two"), (Kind::Para, 0, "three")]);
    let mut v = View::new(Rect { x: 0, y: 0, width: 40, height: 5 });
    let a = d.anchor(Pos { line: 1, byte: 2 }).unwrap();
    // A block inserted above: the anchor still finds "three".
    v.caret = Pos { line: 0, byte: 0 };
    d.apply(&mut v, Command::Newline, &W).unwrap();
    assert_eq!(d.resolve(&a), Some(Pos { line: 2, byte: 2 }));
    // A byte inside a wide character snaps back to its start.
    let mid = caretline::doc::Anchor { id: d.lines()[1].id, byte: 5 };
    assert_eq!(d.resolve(&mid).unwrap().byte, 4);
}

/// Typing doesn't scan the document: 200 keys into 5,000 blocks stay well inside a second even
/// unoptimized (a per-key whole-document check once made it quadratic: 60 ms a key at 5,000).
#[test]
fn typing_in_a_big_doc_is_not_proportional_to_its_size() {
    let blocks: Vec<(Kind, usize, &str)> = (0..5000).map(|i| (if i % 9 == 0 { Kind::Task } else { Kind::Bullet }, (i % 7 == 3) as usize, "the quick brown fox jumps over a lazy dog")).collect();
    let mut d = doc(&blocks);
    let mut v = View::new(Rect { x: 0, y: 0, width: 120, height: 40 });
    v.caret = Pos { line: 2500, byte: 0 };
    let t = std::time::Instant::now();
    for _ in 0..200 {
        d.insert(&mut v, "x").unwrap();
    }
    for _ in 0..100 {
        d.apply(&mut v, Command::Backspace, &W).unwrap();
    }
    let ms = t.elapsed().as_millis();
    assert!(ms < 1500, "300 keys took {ms} ms at 5,000 blocks");
}

/// Replay is pure. The engine reads time and makes lines only through the host's clock
/// and minter, so the same messages give the same document, ids and undo steps every time.
#[test]
fn the_host_supplies_the_clock_and_the_new_lines() {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    let run = || {
        let t0 = Instant::now();
        let at = Arc::new(Mutex::new(t0));
        let clock = at.clone();
        let mut next = 1000u64;
        // A list item: Enter makes the next item, a new block from the host's minter.
        let mut d = doc(&[(Kind::Bullet, 0, "")]).with_clock(move || *clock.lock().unwrap()).with_lines(move |depth, kind, text| {
            next += 1;
            L { b: Block::new(next, depth, kind, text), new: true }
        });
        let mut v = View::new(Rect { x: 0, y: 0, width: 40, height: 10 });
        let w = |_: &L| 40usize;
        // "ab" typed within a second is one typing run; "c" five seconds later (by the host's
        // clock, not the wall's) is another.
        d.insert(&mut v, "a").unwrap();
        *at.lock().unwrap() = t0 + Duration::from_millis(500);
        d.insert(&mut v, "b").unwrap();
        *at.lock().unwrap() = t0 + Duration::from_secs(5);
        d.insert(&mut v, "c").unwrap();
        d.apply(&mut v, Command::Newline, &w).unwrap();
        d.insert(&mut v, "next").unwrap();
        // (The first line is the test's own, made before the minter: only the engine's count.)
        let ids: Vec<u64> = d.lines().iter().skip(1).map(|l| l.id).collect();
        let changed = d.changed_at();
        // Undo: "next", the new item, then "c" on its own (five seconds after "ab" by the host's
        // clock, though the wall clock saw it all within a millisecond).
        for _ in 0..3 {
            d.apply(&mut v, Command::Undo, &w).unwrap();
        }
        let after_undo = texts(&d);
        (ids, changed.map(|c| c.duration_since(t0)), after_undo)
    };
    let (ids, changed, after_undo) = run();
    // The new line came from the host's minter.
    assert_eq!(ids, vec![1001], "{ids:?}");
    // changed_at is the host's time.
    assert_eq!(changed, Some(Duration::from_secs(5)));
    // "ab" was one run, "c" another.
    assert_eq!(after_undo, vec!["ab".to_string()]);
    // And it's the same every time.
    assert_eq!(run(), (ids, changed, after_undo));
}
