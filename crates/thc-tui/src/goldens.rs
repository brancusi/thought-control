//! The editing goldens (docs/design/editing.md): each row `before · keys → after` in the caret
//! notation, run against the document exactly as the keys drive it (Doc::apply, insert, the
//! clipboard as copy and paste do it), then rendered back in the notation and compared.
//!
//! Notation (editing.md §0): `▮` the caret; `⟦…⟧` a selection with `▮` at the caret's end;
//! ` ‖ ` between notes; `⏎` a soft break; `- `, `- [ ] `, `- [x] ` markers, two spaces per depth.

use crate::editor::{BlockPos, Doc, Line, Target};
use thc_core::outline::Kind;

/// A document from the notation, with its caret and anchor.
fn parse(src: &str) -> (Vec<Line>, BlockPos, Option<BlockPos>) {
    let mut lines = Vec::new();
    let (mut caret, mut anchor_mark, mut sel_open, mut sel_close) = (None, None, None, None);
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
                '▮' => caret = Some(BlockPos { line: i, byte: text.len() }),
                '⟦' => sel_open = Some(BlockPos { line: i, byte: text.len() }),
                '⟧' => sel_close = Some(BlockPos { line: i, byte: text.len() }),
                '⏎' => text.push('\n'),
                c => text.push(c),
            }
        }
        let mut l = Line::new(depth, kind, &text);
        l.status = status.map(str::to_string);
        l.is_new = false;
        l.saved = Some(text.clone());
        lines.push(l);
    }
    let caret = caret.expect("a ▮");
    if let (Some(a), Some(b)) = (sel_open, sel_close) {
        // ▮ is at one end; the other end is the anchor.
        anchor_mark = Some(if caret == a { b } else { a });
    }
    (lines, caret, anchor_mark)
}

/// The document back in the notation.
fn render(d: &Doc) -> String {
    let sel = d.selection();
    let mut out = Vec::new();
    for (i, l) in d.blocks().iter().enumerate() {
        let marker = match (l.kind(), l.status.as_deref()) {
            (Kind::Task, Some("done")) => "- [x] ",
            (Kind::Task, _) => "- [ ] ",
            (Kind::Bullet, _) => "- ",
            _ => "",
        };
        let mut s = format!("{}{marker}", "  ".repeat(l.depth));
        let mark = |b: usize| -> String {
            let p = BlockPos { line: i, byte: b };
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
                // ▮ goes inside ⟦…⟧: at the end, before ⟧; at the start, after ⟦.
                return m;
            }
            if p == d.caret() {
                m.push('▮');
            }
            m
        };
        let t = &l.text;
        for (b, c) in t.char_indices() {
            s.push_str(&mark(b));
            s.push(if c == '\n' { '⏎' } else { c });
        }
        s.push_str(&mark(t.len()));
        out.push(s);
    }
    out.join(" ‖ ").replace("▮⟧", "▮⟧").replace("⟦▮", "⟦▮")
}

/// Run keys against a document; the clipboard is a string.
fn run(before: &str, keys: &[&str], clip: &mut String) -> Doc {
    let (lines, caret, anchor) = parse(before);
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap();
    let mut d = Doc::new(Target::Journal { date: today }, None, &[], today);
    d.set_blocks(lines);
    d.select_range(anchor, caret);
    for k in keys {
        let cmd = match *k {
            "←" => Some("move.left"),
            "→" => Some("move.right"),
            "↑" => Some("move.up"),
            "↓" => Some("move.down"),
            "⌥←" => Some("move.word_left"),
            "⌥→" => Some("move.word_right"),
            "⇧←" => Some("select.left"),
            "⇧→" => Some("select.right"),
            "⌫" => Some("edit.backspace"),
            "Del" => Some("edit.delete_forward"),
            "⌥⌫" => Some("edit.delete_word"),
            "⌘⌫" => Some("edit.delete_to_line_start"),
            "⌃K" => Some("edit.kill_line"),
            "Enter" => Some("edit.newline"),
            "Tab" => Some("structure.indent"),
            "⌃T" => Some(crate::editor::TASK_CYCLE),
            "⌘A" => Some("select.all"),
            "⌘Z" => Some("history.undo"),
            "⇧⌘Z" => Some("history.redo"),
            _ => None,
        };
        if let Some(c) = cmd {
            d.run_command(c);
            continue;
        }
        match *k {
            "⌘C" | "⌘X" => {
                let text = d.copy_text();
                if !text.is_empty() {
                    *clip = text;
                    if *k == "⌘X" {
                        d.delete_selection();
                    }
                }
            }
            "⌘V" => {
                let text = clip.clone();
                if !text.contains('\n') {
                    d.paste_line(&text);
                } else {
                    d.paste(&text, false);
                }
            }
            "Esc" => d.clear_selection(),
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

/// Every mismatch in a test, at once.
fn done() {
    let fails = FAILS.with(|f| std::mem::take(&mut *f.borrow_mut()));
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
}

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

fn e38_e39_e40_paste() {
    let mut clip = "X".to_string();
    let d = run("Hello ⟦wor▮⟧ld", &["⌘V"], &mut clip);
    assert_eq!(render(&d), "Hello X▮ld", "E38");
    let mut clip = "- a\n- b".to_string();
    let d = run("▮", &["⌘V"], &mut clip);
    assert_eq!(render(&d), "- a ‖ - b▮", "E39");
}


// ---- invariants (editing.md §10) over random documents and key sequences --------------------

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

/// A random document: notes of random kinds and depths, random text, a random caret (and maybe
/// an anchor) on grapheme boundaries.
fn random_doc(r: &mut Rng) -> Doc {
    use unicode_segmentation::UnicodeSegmentation;
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap();
    let mut d = Doc::new(Target::Journal { date: today }, None, &[], today);
    let mut lines: Vec<Line> = (0..1 + r.below(5))
        .map(|_| {
            let kind = [Kind::Para, Kind::Bullet, Kind::Task][r.below(3)];
            let text: Vec<&str> = (0..r.below(5)).map(|_| WORDS[r.below(WORDS.len())]).collect();
            let mut l = Line::new(if kind == Kind::Para { 0 } else { r.below(2) }, kind, &text.join(" "));
            l.is_new = false;
            l
        })
        .collect();
    // A first line at depth 0 (a list can't start indented).
    lines[0].depth = 0;
    d.set_blocks(lines);
    let pos = |r: &mut Rng, d: &Doc| {
        let line = r.below(d.blocks().len());
        let t = &d.blocks()[line].text;
        let bounds: Vec<usize> = t.grapheme_indices(true).map(|(i, _)| i).chain(std::iter::once(t.len())).collect();
        BlockPos { line, byte: bounds[r.below(bounds.len())] }
    };
    let caret = pos(r, &d);
    let anchor = (r.below(2) == 0).then(|| pos(r, &d));
    d.select_range(anchor, caret);
    d
}

/// What the document is: each note's (depth, kind, status, text), the caret and the selection.
fn state(d: &Doc) -> (Vec<(usize, Kind, Option<String>, String)>, BlockPos, Option<(BlockPos, BlockPos)>) {
    (d.blocks().iter().map(|l| (l.depth, l.kind(), l.status.clone(), l.text.clone())).collect(), d.caret(), d.selection())
}

fn texts(d: &Doc) -> Vec<String> {
    d.blocks().iter().map(|l| l.text.clone()).collect()
}

fn editing_invariants_hold_on_random_documents() {
    let mut r = Rng(0xD1B54A32D192ED03);
    let motions = ["left", "right", "up", "down", "word_left", "word_right", "line_start", "line_end", "doc_start", "doc_end"];
    for case in 0..400 {
        // EI1: a motion without ⇧ leaves no selection. EI2: with ⇧ the anchor never moves.
        let mut d = random_doc(&mut r);
        let m = motions[r.below(motions.len())];
        d.run_command(&format!("move.{m}"));
        assert!(d.selection().is_none(), "EI1 case {case}: {m:?} left a selection: {}", render(&d));
        let mut d = random_doc(&mut r);
        let before_anchor = d.anchor().unwrap_or(d.caret());
        let had = d.selection().is_some();
        d.run_command(&format!("select.{m}"));
        assert_eq!(d.anchor().unwrap_or(before_anchor), before_anchor, "EI2 case {case}: {m:?}");
        let _ = had;
        // EI4: copy changes nothing.
        let mut d = random_doc(&mut r);
        let s0 = state(&d);
        let _ = d.copy_text();
        assert_eq!(state(&d), s0, "EI4 case {case}");
        // EI7: with a selection, every delete gives the same document.
        let d0 = random_doc(&mut r);
        if d0.selection().is_some() {
            let results: Vec<Vec<String>> = ["edit.backspace", "edit.delete_forward", "edit.delete_word", "edit.kill_line", "edit.delete_to_line_start"]
                .iter()
                .map(|c| {
                    let mut d = random_doc(&mut Rng(0));
                    d.set_blocks(d0.blocks().to_vec());
                    d.select_range(d0.anchor(), d0.caret());
                    d.run_command(c);
                    texts(&d)
                })
                .collect();
            for (i, t) in results.iter().enumerate() {
                assert_eq!(t, &results[0], "EI7 case {case}: delete #{i} differs from ⌫: {}", render(&d0));
            }
            // EI6: typing over it is before[..start] + c + before[end..] (notes joined).
            let mut d = random_doc(&mut Rng(0));
            d.set_blocks(d0.blocks().to_vec());
            d.select_range(d0.anchor(), d0.caret());
            let (s, e) = d.selection().unwrap();
            let want = format!("{}c{}", &d0.blocks()[s.line].text[..s.byte], &d0.blocks()[e.line].text[e.byte..]);
            d.insert("c");
            assert_eq!(d.blocks()[s.line].text, want, "EI6 case {case}: {}", render(&d0));
            // EI8: copy, then paste over the same selection, changes nothing in the text.
            let mut d = random_doc(&mut Rng(0));
            d.set_blocks(d0.blocks().to_vec());
            d.select_range(d0.anchor(), d0.caret());
            let clip = d.copy_text();
            if !clip.contains('\n') {
                d.paste_line(&clip);
                assert_eq!(texts(&d), texts(&d0), "EI8 case {case}: {}", render(&d0));
            }
        }
        // EI5: N edits then N undos is the start again: text, caret and selection.
        let mut d = random_doc(&mut r);
        let start = state(&d);
        let n = 1 + r.below(6);
        for _ in 0..n {
            match r.below(6) {
                0 => d.insert("z"),
                1 => {
                    d.run_command("edit.backspace");
                }
                2 => {
                    d.run_command("edit.newline");
                }
                3 => {
                    d.run_command("edit.delete_forward");
                }
                4 => {
                    d.run_command("edit.delete_word");
                }
                _ => d.insert("yy"),
            }
        }
        for _ in 0..n {
            d.run_command("history.undo");
        }
        let (lines0, caret0, sel0) = start.clone();
        let (lines1, caret1, sel1) = state(&d);
        assert_eq!(lines1, lines0, "EI5 case {case}: text");
        assert_eq!((caret1, sel1), (caret0, sel0), "EI5 case {case}: caret and selection");
        // EI12: ⌘A then ⌫ leaves one empty note, and ⌘Z restores everything.
        let mut d = random_doc(&mut r);
        let start = state(&d);
        d.run_command("select.all");
        let selected = state(&d);
        d.run_command("edit.backspace");
        assert!(d.blocks().len() == 1 && d.blocks()[0].text.is_empty(), "EI12 case {case}: {}", render(&d));
        d.run_command("history.undo");
        assert_eq!(state(&d).0, start.0, "EI12 case {case}: ⌘Z restores the text");
        assert_eq!(state(&d), selected, "EI12 case {case}: and the selection");
    }
}

// ---- §7b: kind changes per line, and nothing moves -----------------------------------------
// ` ‖ ` is a boundary with a blank line, ` ¦ ` one without (rows adjacent); tasks as `[ ] `.

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

fn run_g(before: &str, keys: &[&str]) -> Doc {
    let segs = segments(before);
    let joined = segs.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>().join(" ‖ ");
    let mut clip = String::new();
    let mut d = run(&joined, &[], &mut clip);
    // The blank lines as written: explicit only where the kinds wouldn't give them (the
    // engine's default, read before any is set).
    let default: Vec<bool> = (0..segs.len()).map(|i| d.effective_gap(i)).collect();
    for (i, (_, gap)) in segs.iter().enumerate().skip(1) {
        if default[i] != *gap || (d.blocks()[i].kind() == Kind::Para && d.blocks()[i - 1].kind() == Kind::Para) {
            d.set_saved_gap(i, (default[i] != *gap).then_some(*gap));
        }
    }
    for k in keys {
        let c = match *k {
            "⌃T" => crate::editor::TASK_CYCLE,
            "⌘Z" => "history.undo",
            "Tab" => "structure.indent",
            "⇧Tab" => "structure.outdent",
            "⌥↑" => "structure.move_up",
            "⌥↓" => "structure.move_down",
            "Enter" => "edit.newline",
            "⌫" => "edit.backspace",
            t if t.starts_with("type ") => {
                d.insert(&t["type ".len()..]);
                continue;
            }
            other => panic!("unknown key {other}"),
        };
        d.run_command(c);
    }
    d
}

fn render_g(d: &Doc) -> String {
    let plain = render(d);
    let parts: Vec<&str> = plain.split(" ‖ ").collect();
    let mut out = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            out.push_str(if d.effective_gap(i) { " ‖ " } else { " ¦ " });
        }
        let ind = p.len() - p.trim_start_matches(' ').len();
        let body = &p[ind..];
        let body = body.strip_prefix("- [ ] ").map(|r| format!("[ ] {r}")).or_else(|| body.strip_prefix("- [x] ").map(|r| format!("[x] {r}"))).unwrap_or_else(|| body.to_string());
        out.push_str(&" ".repeat(ind));
        out.push_str(&body);
    }
    out
}

fn check_g(id: &str, before: &str, keys: &[&str], after: &str) -> Doc {
    let d = run_g(before, keys);
    let got = render_g(&d);
    if got != after {
        FAILS.with(|f| f.borrow_mut().push(format!("{id}: {before} · {keys:?}\n   want {after}\n   got  {got}")));
    }
    d
}

fn e70_to_e81_kind_changes_per_line_and_nothing_moves() {
    // E70: a line inside a paragraph: three notes, adjacent, the first keeps the id.
    let src70 = "First line⏎sec▮ond line⏎third line";
    check_g("E70", src70, &["⌃T"], "First line ¦ [ ] sec▮ond line ¦ third line");
    let mut d70 = run_g(src70, &[]);
    let id0 = d70.blocks()[0].id.clone();
    d70.run_command("thc.task_cycle");
    assert_eq!(d70.blocks()[0].id, id0, "E70: the first piece keeps the id");
    assert!(d70.blocks()[1].is_new && d70.blocks()[2].is_new, "E70: the other pieces are new notes");
    // E71: the first line: the task keeps the id.
    let mut d71 = run_g("fir▮st⏎second", &[]);
    let id = d71.blocks()[0].id.clone();
    d71.run_command("thc.task_cycle");
    assert_eq!(render_g(&d71), "[ ] fir▮st ¦ second", "E71");
    assert_eq!(d71.blocks()[0].id, id, "E71: the task keeps the id");
    // E72: each selected line its own task.
    check_g("E72", "a⏎⟦b⏎c▮⟧⏎d", &["⌃T"], "a ¦ [ ] ⟦b ¦ [ ] c▮⟧ ¦ d");
    // E73: a list item with a soft break is one item.
    check_g("E73", "- item one⏎more of it▮", &["⌃T"], "[ ] item one⏎more of it▮");
    // E74: a reported case: the gap stays.
    check_g("E74", "[ ] Buy milk ‖ Call ▮the bank", &["⌃T"], "[ ] Buy milk ‖ [ ] Call ▮the bank");
    // E75: B to text: no gap appears.
    check_g("E75", "[ ] A ¦ [ ] B▮", &["⌃T", "⌃T"], "[ ] A ¦ - B▮");
    // E76: to text after a paragraph with a gap: two notes, never joined; back to text is a
    // bullet note (the Logseq model).
    check_g("E76", "Para one ‖ [ ] Ta▮sk", &["⌃T", "⌃T"], "Para one ‖ - Ta▮sk");
    // E77: Tab nests the note right under the one above: the blank row between them goes
    // (tta6t, decided 2026-10-08; caretline nest_joins). ⇧Tab adds none back.
    check_g("E77", "[ ] A ‖ [ ] B▮", &["Tab"], "[ ] A ¦   [ ] B▮");
    check_g("E77b", "[ ] A ¦   [ ] B▮", &["⇧Tab"], "[ ] A ¦ [ ] B▮");
    // E78: undo puts the paragraph back, with its id and the caret.
    let mut d78 = run_g(src70, &[]);
    let id = d78.blocks()[0].id.clone();
    d78.run_command("thc.task_cycle");
    d78.run_command("history.undo");
    assert_eq!(render_g(&d78), src70, "E78");
    assert_eq!(d78.blocks()[0].id, id, "E78: the id");
    // E79: deleting the marker: a bullet note (never a paragraph: the Logseq model), no gap
    // added; the first note stays.
    check_g("E79", "[ ] ▮A ¦ [ ] B", &["⌫", "⌫", "⌫", "⌫"], "- ▮A ¦ [ ] B");
    // E81: back to text joins the neighbours it touches: the reverse of E70.
    let mut d81 = run_g(src70, &[]);
    let id = d81.blocks()[0].id.clone();
    d81.run_command("thc.task_cycle");
    d81.run_command("thc.task_cycle");
    d81.run_command("thc.task_cycle");
    assert_eq!(render_g(&d81), "First line⏎sec▮ond line⏎third line", "E81");
    assert_eq!(d81.blocks()[0].id, id, "E81: the upper note's id");
    done();
}

/// Tab nests any note under the note above, paragraphs too (like Logseq): one level deeper
/// than the note above, whatever its kind. Shift-Tab takes it back.
fn paragraph_children() {
    // `Para line`, Enter, `first subtask`, Tab: the line under the paragraph is its child.
    // (The Logseq model: the typed line is a bullet, and Tab joins it to the paragraph.)
    check_g("PC1", "Para line▮", &["Enter", "type first subtask", "Tab"], "Para line ¦   - first subtask▮");
    check_g("PC2", "Para line ‖   first subtask▮", &["⇧Tab"], "Para line ‖ first subtask▮");
    // Under a paragraph, a bullet, a task; an item under a paragraph.
    check_g("PC3", "One ‖ ▮Two", &["Tab"], "One ¦   ▮Two");
    check_g("PC4", "- one ‖ ▮Two", &["Tab"], "- one ¦   ▮Two");
    check_g("PC5", "[ ] one ‖ ▮Two", &["Tab"], "[ ] one ¦   ▮Two");
    check_g("PC6", "One ¦ - ▮two", &["Tab"], "One ¦   - ▮two");
    // One level at a time, and never under nothing.
    check_g("PC7", "One ‖   ▮Two", &["Tab"], "One ‖   ▮Two");
    check_g("PC8", "▮One", &["Tab"], "▮One");
    // A paragraph moves with its children (⌥↑ / ⌥↓).
    check_g("PC9", "A ‖ B▮ ‖   b1 ‖   - b2", &["⌥↑"], "B▮ ‖   b1 ‖   - b2 ‖ A");
    check_g("PC10", "B▮ ‖   b1 ‖   - b2 ‖ A", &["⌥↓"], "A ‖ B▮ ‖   b1 ‖   - b2");
    // Enter, Enter under a child paragraph: a new note, then (empty) out a level, as Logseq.
    check_g("PC11", "Para ‖   child▮", &["Enter", "Enter", "type next"], "Para ‖   child ‖ - next▮");
    check_g("PC13", "Para▮", &["Enter", "type child", "Tab", "Enter", "Enter", "type [", "type  ", "type ]", "type  ", "type t"], "Para ¦   - child ¦ [ ] t▮");
    // A task box typed on it: a task.
    check_g("PC12", "Para ‖   child▮", &["Enter", "Enter", "type [", "type  ", "type ]", "type  ", "type next"], "Para ‖   child ‖ [ ] next▮");
    done();
}

/// Every test here.
mod run {
    #[test]
    fn paragraph_children() {
        super::paragraph_children();
    }

    #[test]
    fn e1_to_e14_selection_collapse() {
        super::e1_to_e14_selection_collapse();
    }

    #[test]
    fn e15_to_e24_edits_with_a_selection() {
        super::e15_to_e24_edits_with_a_selection();
    }

    #[test]
    fn e25_to_e32_word_and_line_deletes() {
        super::e25_to_e32_word_and_line_deletes();
    }

    #[test]
    fn e33_to_e43_the_clipboard() {
        super::e33_to_e43_the_clipboard();
    }

    #[test]
    fn e38_e39_e40_paste() {
        super::e38_e39_e40_paste();
    }

    #[test]
    fn editing_invariants_hold_on_random_documents() {
        super::editing_invariants_hold_on_random_documents();
    }

    #[test]
    fn e70_to_e81_kind_changes_per_line_and_nothing_moves() {
        super::e70_to_e81_kind_changes_per_line_and_nothing_moves();
    }
}
