//! The editing goldens (editing.md), run against caretline's Doc and View: the same tables
//! thc-tui runs against its own document (crates/thc-tui/src/goldens.rs), so the engine and
//! thc agree rule for rule before thc moves onto it (engine-extraction.md §1a).
//!
//! Notation (editing.md §0): `▮` the caret; `⟦…⟧` a selection with `▮` at the caret's end;
//! ` ‖ ` between notes; `⏎` a soft break; `- `, `- [ ] `, `- [x] ` markers, two spaces per depth.

use caretline::buffer::BlockLine;
use caretline::doc::{Doc, Rect, View};
use caretline::{Block, Command, Kind, Motion, Pos};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq)]
struct Line {
    b: Block<u64>,
    is_new: bool,
}
impl std::ops::Deref for Line {
    type Target = Block<u64>;
    fn deref(&self) -> &Block<u64> {
        &self.b
    }
}
impl std::ops::DerefMut for Line {
    fn deref_mut(&mut self) -> &mut Block<u64> {
        &mut self.b
    }
}
impl BlockLine for Line {
    type Id = u64;
    fn fresh(depth: usize, kind: Kind, text: &str) -> Self {
        Line { b: Block::new(NEXT.fetch_add(1, Ordering::Relaxed), depth, kind, text), is_new: true }
    }
    fn is_new(&self) -> bool {
        self.is_new
    }
    fn keep_host_state(&mut self, _: &Self) {}
    fn revive(&mut self) {
        self.is_new = true;
    }
}

/// One document and the one view the keys go through.
struct D {
    doc: Doc<Line>,
    v: View<u64>,
}

const W: fn(&Line) -> usize = |_| 72;

impl D {
    fn new(lines: Vec<Line>, caret: Pos, anchor: Option<Pos>) -> D {
        let mut v = View::new(Rect { x: 0, y: 0, width: 80, height: 40 });
        v.caret = caret;
        v.anchor = anchor;
        D { doc: Doc::new(lines), v }
    }
    fn lines(&self) -> &[Line] {
        self.doc.lines()
    }
    fn caret(&self) -> Pos {
        self.v.caret
    }
    fn selection(&self) -> Option<(Pos, Pos)> {
        let a = self.v.anchor?;
        let c = self.v.caret;
        (a != c).then(|| if a < c { (a, c) } else { (c, a) })
    }
    fn apply(&mut self, c: Command) {
        let _ = self.doc.apply(&mut self.v, c, &W);
    }
    fn insert(&mut self, s: &str) {
        let _ = self.doc.insert(&mut self.v, s);
    }
}

/// A document from the notation, with its caret and anchor.
fn parse(src: &str) -> (Vec<Line>, Pos, Option<Pos>) {
    let mut lines = Vec::new();
    let (mut caret, mut sel_open, mut sel_close) = (None, None, None);
    for (i, raw) in src.split(" ‖ ").enumerate() {
        let spaces = raw.len() - raw.trim_start_matches(' ').len();
        let mut t = &raw[spaces..];
        let (mut kind, mut status, depth) = (Kind::Para, None, spaces / 2);
        if let Some(r) = t.strip_prefix("- [ ] ") {
            (kind, status, t) = (Kind::Task, Some("todo"), r);
        } else if let Some(r) = t.strip_prefix("- [x] ") {
            (kind, status, t) = (Kind::Task, Some("done"), r);
        } else if let Some(r) = t.strip_prefix("[ ] ") {
            (kind, status, t) = (Kind::Task, Some("todo"), r);
        } else if let Some(r) = t.strip_prefix("- ") {
            (kind, t) = (Kind::Bullet, r);
        }
        let mut text = String::new();
        for c in t.chars() {
            match c {
                '▮' => caret = Some(Pos { line: i, byte: text.len() }),
                '⟦' => sel_open = Some(Pos { line: i, byte: text.len() }),
                '⟧' => sel_close = Some(Pos { line: i, byte: text.len() }),
                '⏎' => text.push('\n'),
                c => text.push(c),
            }
        }
        let mut l = Line::fresh(if kind == Kind::Para { 0 } else { depth }, kind, &text);
        l.status = status.map(str::to_string);
        l.is_new = false;
        lines.push(l);
    }
    let caret = caret.expect("a ▮");
    let anchor = match (sel_open, sel_close) {
        (Some(a), Some(b)) => Some(if caret == a { b } else { a }),
        _ => None,
    };
    (lines, caret, anchor)
}

/// The document back in the notation.
fn render(d: &D) -> String {
    let sel = d.selection();
    let mut out = Vec::new();
    for (i, l) in d.lines().iter().enumerate() {
        let marker = match (l.kind, l.status.as_deref()) {
            (Kind::Task, Some("done")) => "- [x] ",
            (Kind::Task, _) => "- [ ] ",
            (Kind::Bullet, _) => "- ",
            _ => "",
        };
        let mut s = format!("{}{marker}", "  ".repeat(if l.kind == Kind::Para { 0 } else { l.depth }));
        let mark = |b: usize| -> String {
            let p = Pos { line: i, byte: b };
            let mut m = String::new();
            if let Some((a, e)) = sel {
                if p == a {
                    m.push('⟦');
                }
                if p == d.caret() {
                    m.push('▮');
                }
                if p == e {
                    m.push('⟧');
                }
                return m;
            }
            if p == d.caret() {
                m.push('▮');
            }
            m
        };
        for (b, c) in l.text.char_indices() {
            s.push_str(&mark(b));
            s.push(if c == '\n' { '⏎' } else { c });
        }
        s.push_str(&mark(l.text.len()));
        out.push(s);
    }
    out.join(" ‖ ")
}

/// Run keys against a document; the clipboard is a string.
fn run(before: &str, keys: &[&str], clip: &mut String) -> D {
    let (lines, caret, anchor) = parse(before);
    let mut d = D::new(lines, caret, anchor);
    for k in keys {
        let mv = |m| Command::Move { motion: m, select: false };
        let sh = |m| Command::Move { motion: m, select: true };
        let cmd = match *k {
            "←" => Some(mv(Motion::Left)),
            "→" => Some(mv(Motion::Right)),
            "↑" => Some(mv(Motion::Up)),
            "↓" => Some(mv(Motion::Down)),
            "⌥←" => Some(mv(Motion::WordLeft)),
            "⌥→" => Some(mv(Motion::WordRight)),
            "⇧←" => Some(sh(Motion::Left)),
            "⇧→" => Some(sh(Motion::Right)),
            "⌫" => Some(Command::Backspace),
            "Del" => Some(Command::Delete),
            "⌥⌫" => Some(Command::DeleteWordBack),
            "⌘⌫" => Some(Command::KillToStart),
            "⌃K" => Some(Command::KillToEnd),
            "Enter" => Some(Command::Newline),
            "Tab" => Some(Command::Indent),
            "⌃T" => Some(Command::TaskCycle),
            "⌘A" => Some(Command::SelectAll),
            "⌘Z" => Some(Command::Undo),
            "⇧⌘Z" => Some(Command::Redo),
            _ => None,
        };
        if let Some(c) = cmd {
            d.apply(c);
            continue;
        }
        match *k {
            "⌘C" => {
                let t = d.doc.copy(&d.v);
                if !t.is_empty() {
                    *clip = t;
                }
            }
            "⌘X" => {
                let (t, _) = d.doc.cut(&mut d.v).unwrap();
                if !t.is_empty() {
                    *clip = t;
                }
            }
            "⌘V" => {
                let t = clip.clone();
                d.doc.paste(&mut d.v, &t, false).unwrap();
            }
            "Esc" => d.v.anchor = None,
            t if t.starts_with("type ") => d.insert(&t["type ".len()..]),
            other => panic!("unknown key {other}"),
        }
    }
    d
}

thread_local! {
    static FAILS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn check(id: &str, before: &str, keys: &[&str], after: &str) {
    check_clip_opt(id, before, keys, after, None);
}

fn check_clip(id: &str, before: &str, keys: &[&str], after: &str, want_clip: &str) {
    check_clip_opt(id, before, keys, after, Some(want_clip));
}

fn check_clip_opt(id: &str, before: &str, keys: &[&str], after: &str, want_clip: Option<&str>) {
    let mut clip = String::new();
    let d = run(before, keys, &mut clip);
    let got = render(&d);
    if got != after {
        FAILS.with(|f| f.borrow_mut().push(format!("{id}: {before} · {keys:?}\n   want {after}\n   got  {got}")));
    }
    if let Some(w) = want_clip {
        if clip != w {
            FAILS.with(|f| f.borrow_mut().push(format!("{id}: clip want {w:?} got {clip:?}")));
        }
    }
}

fn done() {
    let fails = FAILS.with(|f| std::mem::take(&mut *f.borrow_mut()));
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
}

#[test]
fn e1_to_e14_selection_collapse() {
    check("E1", "Hello ⟦wor▮⟧ld", &["←"], "Hello ▮world");
    check("E2", "Hello ⟦wor▮⟧ld", &["→"], "Hello wor▮ld");
    check("E3", "Hello ⟦▮wor⟧ld", &["→"], "Hello wor▮ld");
    check("E4", "Hello ⟦wor▮⟧ld", &["⇧←"], "Hello ⟦wo▮⟧rld");
    check("E5", "Hello ⟦wor▮⟧ld", &["⇧→"], "Hello ⟦worl▮⟧d");
    check("E6", "ab ⟦▮c⟧ d", &["⇧→"], "ab c▮ d");
    check("E7", "First note ‖ Sec⟦ond no▮⟧te", &["←"], "First note ‖ Sec▮ond note");
    check("E8", "Line one ‖ Line ⟦two and▮⟧ more", &["↑"], "Line ▮one ‖ Line two and more");
    check("E9", "Line ⟦one▮⟧ ‖ Line two", &["↓"], "Line one ‖ Line two▮");
    check("E10", "one two ⟦thr▮⟧ee", &["⌥←"], "one ▮two three");
    check("E11", "one ⟦tw▮⟧o three", &["⌥→"], "one two▮ three");
    check("E12", "ab ⟦cd▮⟧ ef", &["Esc"], "ab cd▮ ef");
    done();
}

#[test]
fn e15_to_e24_edits_with_a_selection() {
    check("E15", "Hello ⟦wor▮⟧ld", &["type X"], "Hello X▮ld");
    check("E16", "Hello ⟦wor▮⟧ld", &["⌫"], "Hello ▮ld");
    check("E17", "Hello ⟦wor▮⟧ld", &["Del"], "Hello ▮ld");
    check("E18", "one ⟦two thr▮⟧ee", &["⌥⌫"], "one ▮ee");
    check("E19", "One ⟦two ‖ three fo▮⟧ur", &["type X"], "One X▮ur");
    check("E20", "A⟦a ‖ Bb ‖ C▮⟧c", &["⌫"], "A▮c");
    check("E21", "ab⟦cd▮⟧ef", &["Enter"], "ab⏎▮ef");
    check("E22", "- o⟦ne ‖ - tw▮⟧o", &["Tab"], "- o⟦ne ‖   - tw▮⟧o");
    check("E23", "o⟦ne ‖ tw▮⟧o", &["⌃T"], "- [ ] o⟦ne ‖ - [ ] tw▮⟧o");
    check("E24", "Hello ⟦wor▮⟧ld", &["type X", "type Y", "⌘Z"], "Hello ⟦wor▮⟧ld");
    done();
}

#[test]
fn e25_to_e32_word_and_line_deletes() {
    check("E25", "one two thr▮ee", &["⌥⌫"], "one two ▮ee");
    check("E26", "one two ▮three", &["⌥⌫"], "one ▮three");
    // "Joins like ⌫": ⌫ at a paragraph's start joins with a line break (writing.md §1).
    check("E27", "First ‖ ▮Second", &["⌥⌫"], "First⏎▮Second");
    check("E29", "one two thr▮ee", &["⌘⌫"], "▮ee");
    check("E30", "one ▮two⏎three", &["⌃K"], "one ▮⏎three");
    check("E31", "one▮⏎three", &["⌃K"], "one▮three");
    done();
}

#[test]
fn e33_to_e43_the_clipboard() {
    check_clip("E33", "Hello ⟦wor▮⟧ld", &["⌘C"], "Hello ⟦wor▮⟧ld", "wor");
    check_clip("E34", "- [ ] Buy ⟦milk ‖ - [ ] Call ▮⟧Sam", &["⌘C"], "- [ ] Buy ⟦milk ‖ - [ ] Call ▮⟧Sam", "milk\n- [ ] Call ");
    check_clip("E35", "⟦One ‖ - [x] Two▮⟧", &["⌘C"], "⟦One ‖ - [x] Two▮⟧", "One\n\n- [x] Two");
    // From the very end of a note: the copy starts with the boundary (the empty end, then the
    // paragraph's blank line), so pasting it back over the same selection changes nothing (EI8).
    check_clip("E35b", "Plan the trip⟦ ‖ - [ ] Book the flat▮⟧", &["⌘C"], "Plan the trip⟦ ‖ - [ ] Book the flat▮⟧", "\n\n- [ ] Book the flat");
    check_clip("E36", "Hello wor▮ld", &["⌘C"], "Hello wor▮ld", "");
    check_clip("E37", "Hello ⟦wor▮⟧ld", &["⌘X"], "Hello ▮ld", "wor");
    check_clip("E37b", "Hello ⟦wor▮⟧ld", &["⌘X", "⌘Z"], "Hello ⟦wor▮⟧ld", "wor");
    check("E41", "ab ‖ cd▮", &["⌘A"], "⟦ab ‖ cd▮⟧");
    check("E41b", "ab ‖ cd▮", &["⌘A", "←"], "▮ab ‖ cd");
    check("E42", "⟦ab ‖ cd▮⟧", &["⌫"], "▮");
    check("E42b", "⟦ab ‖ cd▮⟧", &["⌫", "⌘Z"], "⟦ab ‖ cd▮⟧");
    check("E43", "Hello X▮ld", &["⇧⌘Z"], "Hello X▮ld");
    done();
}

#[test]
fn e38_e39_e40_paste() {
    let mut clip = "X".to_string();
    let d = run("Hello ⟦wor▮⟧ld", &["⌘V"], &mut clip);
    assert_eq!(render(&d), "Hello X▮ld", "E38");
    let mut clip = "- a\n- b".to_string();
    let d = run("▮", &["⌘V"], &mut clip);
    assert_eq!(render(&d), "- a ‖ - b▮", "E39");
}


// ---- §7b: kind changes per line, and nothing moves -----------------------------------------

/// Split on both boundaries: the segments and whether a blank line comes before each.
fn segments(src: &str) -> Vec<(String, bool)> {
    let mut out = vec![];
    let mut rest = src;
    let mut gap = false;
    loop {
        let a = rest.find(" ‖ ");
        let b = rest.find(" ¦ ");
        let (at, sep_gap, len) = match (a, b) {
            (Some(x), Some(y)) if x < y => (x, true, " ‖ ".len()),
            (Some(_), Some(y)) => (y, false, " ¦ ".len()),
            (Some(x), None) => (x, true, " ‖ ".len()),
            (None, Some(y)) => (y, false, " ¦ ".len()),
            (None, None) => break,
        };
        out.push((rest[..at].to_string(), gap));
        gap = sep_gap;
        rest = &rest[at + len..];
    }
    out.push((rest.to_string(), gap));
    out
}

fn run_g(before: &str, keys: &[&str]) -> D {
    let segs = segments(before);
    let joined = segs.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>().join(" ‖ ");
    let mut clip = String::new();
    let mut d = run(&joined, &[], &mut clip);
    for (i, (_, gap)) in segs.iter().enumerate().skip(1) {
        let def = d.doc.default_gap(i);
        if def != *gap {
            d.doc.set_gap(i, Some(*gap));
        }
    }
    for k in keys {
        let c = match *k {
            "⌃T" => Command::TaskCycle,
            "⌘Z" => Command::Undo,
            "Tab" => Command::Indent,
            "⌫" => Command::Backspace,
            other => panic!("unknown key {other}"),
        };
        d.apply(c);
    }
    d
}

fn render_g(d: &D) -> String {
    let plain = render(d);
    let parts: Vec<&str> = plain.split(" ‖ ").collect();
    let mut out = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            out.push_str(if d.doc.effective_gap(i) { " ‖ " } else { " ¦ " });
        }
        let ind = p.len() - p.trim_start_matches(' ').len();
        let body = &p[ind..];
        let body = body.strip_prefix("- [ ] ").map(|r| format!("[ ] {r}")).or_else(|| body.strip_prefix("- [x] ").map(|r| format!("[x] {r}"))).unwrap_or_else(|| body.to_string());
        out.push_str(&" ".repeat(ind));
        out.push_str(&body);
    }
    out
}

fn check_g(id: &str, before: &str, keys: &[&str], after: &str) -> D {
    let d = run_g(before, keys);
    let got = render_g(&d);
    if got != after {
        FAILS.with(|f| f.borrow_mut().push(format!("{id}: {before} · {keys:?}\n   want {after}\n   got  {got}")));
    }
    d
}

#[test]
fn e70_to_e81_kind_changes_per_line_and_nothing_moves() {
    // E70: a line inside a paragraph: three notes, adjacent, the first keeps the id.
    let src70 = "First line⏎sec▮ond line⏎third line";
    check_g("E70", src70, &["⌃T"], "First line ¦ [ ] sec▮ond line ¦ third line");
    let mut d70 = run_g(src70, &[]);
    let id0 = d70.lines()[0].id;
    d70.apply(Command::TaskCycle);
    assert_eq!(d70.lines()[0].id, id0, "E70: the first piece keeps the id");
    assert!(d70.lines()[1].is_new && d70.lines()[2].is_new, "E70: the other pieces are new notes");
    // E71: the first line: the task keeps the id.
    let mut d71 = run_g("fir▮st⏎second", &[]);
    let id = d71.lines()[0].id;
    d71.apply(Command::TaskCycle);
    assert_eq!(render_g(&d71), "[ ] fir▮st ¦ second", "E71");
    assert_eq!(d71.lines()[0].id, id, "E71: the task keeps the id");
    // E72: each selected line its own task.
    check_g("E72", "a⏎⟦b⏎c▮⟧⏎d", &["⌃T"], "a ¦ [ ] ⟦b ¦ [ ] c▮⟧ ¦ d");
    // E73: a list item with a soft break is one item.
    check_g("E73", "- item one⏎more of it▮", &["⌃T"], "[ ] item one⏎more of it▮");
    // E74: a reported case: the gap stays.
    check_g("E74", "[ ] Buy milk ‖ Call ▮the bank", &["⌃T"], "[ ] Buy milk ‖ [ ] Call ▮the bank");
    // E75: B to text: no gap appears.
    check_g("E75", "[ ] A ¦ [ ] B▮", &["⌃T", "⌃T"], "[ ] A ¦ B▮");
    // E76: to text after a paragraph with a gap: two notes, never joined.
    check_g("E76", "Para one ‖ [ ] Ta▮sk", &["⌃T", "⌃T"], "Para one ‖ Ta▮sk");
    // E77: Tab keeps the gap.
    check_g("E77", "[ ] A ‖ [ ] B▮", &["Tab"], "[ ] A ‖   [ ] B▮");
    // E78: undo puts the paragraph back, with its id and the caret.
    let mut d78 = run_g(src70, &[]);
    let id = d78.lines()[0].id;
    d78.apply(Command::TaskCycle);
    d78.apply(Command::Undo);
    assert_eq!(render_g(&d78), src70, "E78");
    assert_eq!(d78.lines()[0].id, id, "E78: the id");
    // E79: deleting the marker: a paragraph, no gap added.
    check_g("E79", "[ ] ▮A ¦ [ ] B", &["⌫", "⌫", "⌫", "⌫"], "▮A ¦ [ ] B");
    // E81: back to text joins the neighbours it touches: the reverse of E70.
    let mut d81 = run_g(src70, &[]);
    let id = d81.lines()[0].id;
    d81.apply(Command::TaskCycle);
    d81.apply(Command::TaskCycle);
    d81.apply(Command::TaskCycle);
    assert_eq!(render_g(&d81), "First line⏎sec▮ond line⏎third line", "E81");
    assert_eq!(d81.lines()[0].id, id, "E81: the upper note's id");
    done();
}

// ---- invariants (editing.md §10) over random documents, through a View ------------------------

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const WORDS: &[&str] = &["one", "two", "日本", "é", "👨‍👩‍👧", "x.", "longer", "a"];

fn random_doc(r: &mut Rng) -> D {
    use unicode_segmentation::UnicodeSegmentation;
    let mut lines: Vec<Line> = (0..1 + r.below(5))
        .map(|_| {
            let kind = [Kind::Para, Kind::Bullet, Kind::Task][r.below(3)];
            let text: Vec<&str> = (0..r.below(5)).map(|_| WORDS[r.below(WORDS.len())]).collect();
            let mut l = Line::fresh(if kind == Kind::Para { 0 } else { r.below(2) }, kind, &text.join(if r.below(4) == 0 { "\n" } else { " " }));
            l.is_new = false;
            l
        })
        .collect();
    lines[0].depth = 0;
    let pos = |r: &mut Rng, lines: &[Line]| {
        let line = r.below(lines.len());
        let t = &lines[line].text;
        let bounds: Vec<usize> = t.grapheme_indices(true).map(|(i, _)| i).chain(std::iter::once(t.len())).collect();
        Pos { line, byte: bounds[r.below(bounds.len())] }
    };
    let caret = pos(r, &lines);
    let anchor = (r.below(2) == 0).then(|| pos(r, &lines));
    D::new(lines, caret, anchor)
}

type State = (Vec<(usize, Kind, Option<String>, String)>, Pos, Option<(Pos, Pos)>);

fn state(d: &D) -> State {
    (d.lines().iter().map(|l| (l.depth, l.kind, l.status.clone(), l.text.clone())).collect(), d.caret(), d.selection())
}

fn texts(d: &D) -> Vec<String> {
    d.lines().iter().map(|l| l.text.clone()).collect()
}

#[test]
fn editing_invariants_hold_through_a_view() {
    let mut r = Rng(0xD1B54A32D192ED03);
    let motions = [Motion::Left, Motion::Right, Motion::Up, Motion::Down, Motion::WordLeft, Motion::WordRight, Motion::Home, Motion::End, Motion::DocStart, Motion::DocEnd];
    for case in 0..400 {
        // EI1: a motion without ⇧ leaves no selection. EI2: with ⇧ the anchor never moves.
        let mut d = random_doc(&mut r);
        let m = motions[r.below(motions.len())];
        d.apply(Command::Move { motion: m, select: false });
        assert!(d.selection().is_none(), "EI1 case {case}: {m:?}: {}", render(&d));
        let mut d = random_doc(&mut r);
        let before_anchor = d.v.anchor.unwrap_or(d.v.caret);
        d.apply(Command::Move { motion: m, select: true });
        assert_eq!(d.v.anchor.unwrap_or(before_anchor), before_anchor, "EI2 case {case}: {m:?}");
        // EI4: copy changes nothing.
        let mut d = random_doc(&mut r);
        let s0 = state(&d);
        let _ = d.doc.copy(&d.v);
        assert_eq!(state(&d), s0, "EI4 case {case}");
        // EI7: with a selection, every delete gives the same document.
        let d0 = random_doc(&mut r);
        if let Some((s, e)) = d0.selection() {
            let src = render(&d0);
            let results: Vec<Vec<String>> = [Command::Backspace, Command::Delete, Command::DeleteWordBack, Command::KillToEnd, Command::KillToStart]
                .iter()
                .map(|c| {
                    let (lines, caret, anchor) = parse(&src);
                    let mut d = D::new(lines, caret, anchor);
                    d.apply(*c);
                    texts(&d)
                })
                .collect();
            for (i, t) in results.iter().enumerate() {
                assert_eq!(t, &results[0], "EI7 case {case}: delete #{i} differs from ⌫: {src}");
            }
            // EI6: typing over it is before[..start] + c + before[end..].
            let (lines, caret, anchor) = parse(&src);
            let mut d = D::new(lines, caret, anchor);
            let want = format!("{}c{}", &d0.lines()[s.line].text[..s.byte], &d0.lines()[e.line].text[e.byte..]);
            d.insert("c");
            assert_eq!(d.lines()[s.line].text, want, "EI6 case {case}: {src}");
        }
        // EI5: N edits then N undos is the start again: text, caret and selection.
        let mut d = random_doc(&mut r);
        let start = state(&d);
        let n = 1 + r.below(6);
        for _ in 0..n {
            match r.below(6) {
                0 => d.insert("z"),
                1 => d.apply(Command::Backspace),
                2 => d.apply(Command::Newline),
                3 => d.apply(Command::Delete),
                4 => d.apply(Command::DeleteWordBack),
                _ => d.insert("yy"),
            }
        }
        for _ in 0..n {
            d.apply(Command::Undo);
        }
        assert_eq!(state(&d).0, start.0, "EI5 case {case}: text");
        assert_eq!((state(&d).1, state(&d).2), (start.1, start.2), "EI5 case {case}: caret and selection");
        // EI12: ⌘A then ⌫ leaves one empty note, and ⌘Z restores everything.
        let mut d = random_doc(&mut r);
        let start = state(&d);
        d.apply(Command::SelectAll);
        let selected = state(&d);
        d.apply(Command::Backspace);
        assert!(d.lines().len() == 1 && d.lines()[0].text.is_empty(), "EI12 case {case}: {}", render(&d));
        d.apply(Command::Undo);
        assert_eq!(state(&d).0, start.0, "EI12 case {case}");
        assert_eq!(state(&d), selected, "EI12 case {case}: and the selection");
        // EI13 (document side): a kind change never changes another block's blank line.
        // EI14: and its undo restores every gap.
        let mut d = random_doc(&mut r);
        d.v.anchor = None;
        let gaps0 = d.doc.gaps();
        let me = d.lines()[d.caret().line].id;
        let ids0: Vec<u64> = d.lines().iter().map(|l| l.id).collect();
        let c = [Command::TaskCycle, Command::Indent, Command::Outdent][r.below(3)];
        d.apply(c);
        let gaps1 = d.doc.gaps();
        for id in &ids0 {
            if *id == me {
                continue;
            }
            if let (Some(a), Some(b)) = (gaps0.get(id), gaps1.get(id)) {
                // (A block the change joined into another isn't there to compare.)
                assert_eq!(a, b, "EI13 case {case}: {c:?} moved block {id}: {}", render(&d));
            }
        }
        d.apply(Command::Undo);
        assert_eq!(d.doc.gaps(), gaps0, "EI14 case {case}: {c:?} then undo");
    }
}
