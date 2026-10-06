//! Caret motion (caretline SPEC §5, from thc's motion.md): one map of the rows the caret can
//! stop on, one owner per position, a goal column on the screen, and every motion a pure
//! function over the map. A host builds the map from its own wrap (the renderer's), so what's
//! drawn and where a key goes can't disagree.

use crate::Pos;
use unicode_segmentation::UnicodeSegmentation;

/// One row the caret can stop on: a visual row of a visible note (motion.md §1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stop {
    /// The note (the document's line index).
    pub line: usize,
    /// The row's first position: where its text starts (after a heading's or number's marker).
    pub start: usize,
    /// Its last position (inclusive): the greatest position before the next row's start, or the
    /// note's end on its last row (§2).
    pub end: usize,
    /// The cell where the row's text starts, in the document column (indent and hang counted).
    pub x0: usize,
}

/// The stop rows and the notes' text they index.
pub struct Layout<'a> {
    pub texts: Vec<&'a str>,
    pub stops: Vec<Stop>,
}

/// A note's stops from its wrap (`rows`: the byte ranges the renderer draws), the marker drawn
/// in the hang (`marker` bytes), and where its text starts (`x0`).
pub fn note_stops(
    line: usize,
    text: &str,
    rows: &[(usize, usize)],
    marker: usize,
    x0: usize,
) -> Vec<Stop> {
    let mut out = Vec::with_capacity(rows.len());
    for (k, &(s, _)) in rows.iter().enumerate() {
        let start = if k == 0 {
            s.max(marker).min(text.len())
        } else {
            s
        };
        let end = match rows.get(k + 1) {
            Some(&(next, _)) => crate::prev_char(text, next).max(start),
            None => text.len(),
        };
        out.push(Stop {
            line,
            start,
            end: end.max(start),
            x0,
        });
    }
    out
}

impl Layout<'_> {
    /// The stop row that owns a position: the last of its note's rows with start ≤ position
    /// (§2). A position inside a marker belongs to the note's first row. None: a hidden note.
    pub fn owner(&self, p: Pos) -> Option<usize> {
        let first = self.stops.partition_point(|s| s.line < p.line);
        let mut found = None;
        for (k, s) in self.stops[first..].iter().enumerate() {
            if s.line != p.line {
                break;
            }
            if found.is_none() || s.start <= p.byte {
                found = Some(first + k);
            }
        }
        found
    }

    /// The cell a position is drawn at, in the document column.
    pub fn x(&self, p: Pos) -> usize {
        let Some(r) = self.owner(p) else { return 0 };
        let s = self.stops[r];
        let t = self.texts[s.line];
        if p.byte <= s.start {
            return s.x0;
        }
        s.x0 + crate::width(&t[s.start..p.byte.min(t.len())])
    }

    /// The position on stop row `r` for goal column `goal` (§3): the greatest boundary with
    /// x ≤ goal; the row's start when its text begins right of the goal; its end when shorter.
    pub fn land(&self, r: usize, goal: usize) -> Pos {
        let s = self.stops[r];
        let t = self.texts[s.line];
        let (mut b, mut x) = (s.start, s.x0);
        while b < s.end {
            let Some(g) = t[b..s.end].graphemes(true).next() else {
                break;
            };
            let w = if g == "\n" {
                0
            } else {
                unicode_width::UnicodeWidthStr::width(g)
            };
            if x + w > goal {
                break;
            }
            x += w;
            b += g.len();
        }
        Pos {
            line: s.line,
            byte: b,
        }
    }

    pub fn start(&self) -> Pos {
        self.stops.first().map_or(Pos::default(), |s| Pos {
            line: s.line,
            byte: s.start,
        })
    }

    pub fn end(&self) -> Pos {
        self.stops.last().map_or(Pos::default(), |s| Pos {
            line: s.line,
            byte: s.end,
        })
    }

    /// The first and last stop rows of a note.
    fn note_rows(&self, line: usize) -> Option<(usize, usize)> {
        let a = self.stops.partition_point(|s| s.line < line);
        let b = self.stops.partition_point(|s| s.line <= line);
        (a < b).then(|| (a, b - 1))
    }

    /// A position that's a stop: a hidden or out-of-range caret goes to the nearest visible
    /// note's start, a byte past the end to the note's end, and a byte inside a marker to the
    /// text's start.
    pub fn valid(&self, p: Pos) -> Pos {
        let Some((a, b)) = self.note_rows(p.line) else {
            let k = self.stops.partition_point(|s| s.line < p.line);
            return match self.stops.get(k).or(self.stops.last()) {
                Some(s) => Pos {
                    line: s.line,
                    byte: s.start,
                },
                None => p,
            };
        };
        let t = self.texts[p.line];
        let mut byte = p
            .byte
            .min(t.len())
            .clamp(self.stops[a].start, self.stops[b].end);
        while !t.is_char_boundary(byte) {
            byte -= 1;
        }
        // Inside a cluster (e + ◌́, a family emoji): its start.
        if byte < t.len() && !t.grapheme_indices(true).any(|(i, _)| i == byte) {
            byte = t[..byte]
                .grapheme_indices(true)
                .next_back()
                .map_or(0, |(i, _)| i);
        }
        Pos {
            line: p.line,
            byte: byte.max(self.stops[a].start),
        }
    }

    /// ↑ / ↓, by `n` stop rows, keeping the goal (§4). Past the first row, the document's start;
    /// past the last, its end.
    pub fn vertical(&self, p: Pos, goal: Option<usize>, n: isize) -> (Pos, Option<usize>) {
        let p = self.valid(p);
        let goal = goal.unwrap_or_else(|| self.x(p));
        let Some(r) = self.owner(p) else {
            return (p, Some(goal));
        };
        let to = r as isize + n;
        if to < 0 {
            return (self.start(), Some(goal));
        }
        if to as usize >= self.stops.len() {
            return (self.end(), Some(goal));
        }
        (self.land(to as usize, goal), Some(goal))
    }

    /// → by one grapheme; from a note's end, the next visible note's start (§4).
    pub fn right(&self, p: Pos) -> Pos {
        let p = self.valid(p);
        let Some((_, b)) = self.note_rows(p.line) else {
            return p;
        };
        if p.byte < self.stops[b].end {
            return Pos {
                line: p.line,
                byte: crate::next_char(self.texts[p.line], p.byte),
            };
        }
        match self.stops.get(b + 1) {
            Some(s) => Pos {
                line: s.line,
                byte: s.start,
            },
            None => p,
        }
    }

    /// ← by one grapheme; from a note's start, the previous visible note's end.
    pub fn left(&self, p: Pos) -> Pos {
        let p = self.valid(p);
        let Some((a, _)) = self.note_rows(p.line) else {
            return p;
        };
        if p.byte > self.stops[a].start {
            return Pos {
                line: p.line,
                byte: crate::prev_char(self.texts[p.line], p.byte).max(self.stops[a].start),
            };
        }
        match a.checked_sub(1).map(|k| self.stops[k]) {
            Some(s) => Pos {
                line: s.line,
                byte: s.end,
            },
            None => p,
        }
    }

    /// Home / End: the visual row's start or end; at that edge already, the note's (§4).
    pub fn home_end(&self, p: Pos, end: bool) -> Pos {
        let p = self.valid(p);
        let (Some(r), Some((a, b))) = (self.owner(p), self.note_rows(p.line)) else {
            return p;
        };
        let s = self.stops[r];
        let edge = if end { s.end } else { s.start };
        let byte = if edge != p.byte {
            edge
        } else if end {
            self.stops[b].end
        } else {
            self.stops[a].start
        };
        Pos { line: p.line, byte }
    }

    /// The words of a note (UAX #29 segments with a letter or digit), as byte ranges.
    fn words(&self, line: usize) -> Vec<(usize, usize)> {
        let t = self.texts[line];
        let from = self.note_rows(line).map_or(0, |(a, _)| self.stops[a].start);
        t.split_word_bound_indices()
            .filter(|(i, w)| *i >= from && w.chars().any(char::is_alphanumeric))
            .map(|(i, w)| (i, i + w.len()))
            .collect()
    }

    /// ⌥→: the next word's end, across notes.
    pub fn word_right(&self, p: Pos) -> Pos {
        let p = self.valid(p);
        if let Some(&(_, e)) = self.words(p.line).iter().find(|(_, e)| *e > p.byte) {
            return Pos {
                line: p.line,
                byte: e,
            };
        }
        let Some((_, b)) = self.note_rows(p.line) else {
            return p;
        };
        let mut k = b + 1;
        while let Some(s) = self.stops.get(k) {
            if let Some(&(_, e)) = self.words(s.line).first() {
                return Pos {
                    line: s.line,
                    byte: e,
                };
            }
            k = self.note_rows(s.line).map_or(k + 1, |(_, b)| b + 1);
        }
        self.end()
    }

    /// ⌥←: the previous word's start, across notes.
    pub fn word_left(&self, p: Pos) -> Pos {
        let p = self.valid(p);
        if let Some(&(s, _)) = self.words(p.line).iter().rev().find(|(s, _)| *s < p.byte) {
            return Pos {
                line: p.line,
                byte: s,
            };
        }
        let Some((a, _)) = self.note_rows(p.line) else {
            return p;
        };
        let mut k = a;
        while k > 0 {
            let s = self.stops[k - 1];
            if let Some(&(st, _)) = self.words(s.line).last() {
                return Pos {
                    line: s.line,
                    byte: st,
                };
            }
            k = self.note_rows(s.line).map_or(k - 1, |(a, _)| a);
        }
        self.start()
    }
}

/// A caret motion (motion.md §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Up,
    Down,
    /// PgUp / PgDn: this many stop rows (negative: up).
    Page(isize),
    Left,
    Right,
    WordLeft,
    WordRight,
    Home,
    End,
    DocStart,
    DocEnd,
    /// ⌃↑ / ⌃↓: the note's start, else the previous / next visible note's.
    NoteUp,
    NoteDown,
}

impl Layout<'_> {
    /// ⌃↑: the note's start, else the previous visible note's; ⌃↓: the next note's start, else
    /// the document's end.
    pub fn note_step(&self, p: Pos, down: bool) -> Pos {
        let p = self.valid(p);
        let Some((a, b)) = self.note_rows(p.line) else {
            return p;
        };
        if down {
            return self.stops.get(b + 1).map_or(self.end(), |s| Pos {
                line: s.line,
                byte: s.start,
            });
        }
        if p.byte > self.stops[a].start {
            return Pos {
                line: p.line,
                byte: self.stops[a].start,
            };
        }
        match a.checked_sub(1) {
            Some(k) => {
                let line = self.stops[k].line;
                let (a2, _) = self.note_rows(line).unwrap_or((k, k));
                Pos {
                    line,
                    byte: self.stops[a2].start,
                }
            }
            None => p,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture as motion.md §7 gives it: notes (x0, text) wrapped at `w` cells.
    struct Fx {
        texts: Vec<String>,
        stops: Vec<Stop>,
    }

    impl Fx {
        fn new(notes: &[(usize, &str)], w: usize) -> Fx {
            let mut stops = Vec::new();
            for (i, (x0, t)) in notes.iter().enumerate() {
                let rows = crate::wrap(t, w - x0);
                stops.extend(note_stops(i, t, &rows, 0, *x0));
            }
            Fx {
                texts: notes.iter().map(|(_, t)| t.to_string()).collect(),
                stops,
            }
        }
        fn l(&self) -> Layout<'_> {
            Layout {
                texts: self.texts.iter().map(String::as_str).collect(),
                stops: self.stops.clone(),
            }
        }
    }

    fn f1() -> Fx {
        Fx::new(
            &[
                (
                    0,
                    "The quick brown fox jumps over the lazy dog and keeps running through the long grass until dusk.",
                ),
                (0, "Short one."),
                (
                    4,
                    "Bullet item that is long enough to wrap onto a second row.",
                ),
                (0, ""),
                (0, "Last note."),
            ],
            40,
        )
    }

    fn p(line: usize, byte: usize) -> Pos {
        Pos { line, byte }
    }

    #[test]
    fn f1_rows_are_the_spec() {
        let f = f1();
        let rows: Vec<(usize, usize, usize)> =
            f.stops.iter().map(|s| (s.line, s.start, s.end)).collect();
        assert_eq!(
            rows,
            [
                (0, 0, 39),
                (0, 40, 78),
                (0, 79, 96),
                (1, 0, 10),
                (2, 0, 34),
                (2, 35, 58),
                (3, 0, 0),
                (4, 0, 10)
            ]
        );
    }

    // ---- golden scenarios (motion.md §7) in the caret notation (engine-docs.md §5) -----------
    //
    // A scenario: `width N` (and `page N`, `start block:byte`), `---`, the document as logical
    // text (blank lines between blocks, `  - ` a bullet two spaces per level, `∅` an empty
    // block, `{folded}` hides a block's children, `{meta}` a meta on its own row, `▮` the
    // caret), `---`, then one step per line: a key and the caret after it, as an excerpt with
    // `▮` or `block:byte`. `anchor <excerpt>` checks a selection's fixed end.

    struct Scenario {
        texts: Vec<String>,
        stops: Vec<Stop>,
        caret: Pos,
        page: isize,
        steps: Vec<(String, String)>,
    }

    fn scenario(src: &str) -> Scenario {
        let mut parts = src.trim().splitn(3, "\n---\n");
        let (head, body, steps) = (
            parts.next().unwrap(),
            parts.next().unwrap(),
            parts.next().unwrap_or(""),
        );
        let (mut width, mut page, mut start) = (40usize, 3isize, None);
        for l in head.lines() {
            let (k, v) = l.split_once(' ').unwrap();
            match k {
                "width" => width = v.parse().unwrap(),
                "page" => page = v.parse().unwrap(),
                "start" => start = Some(at(v)),
                _ => panic!("unknown header {k}"),
            }
        }
        let (mut texts, mut stops, mut caret) = (Vec::new(), Vec::new(), start);
        let mut hidden_below: Option<usize> = None;
        for (i, block) in body.split("\n\n").enumerate() {
            let mut t = block.to_string();
            let folded = t.contains(" {folded}");
            t = t.replace(" {folded}", "").replace(" {meta}", "");
            let spaces = t.len() - t.trim_start_matches(' ').len();
            let mut depth = 0;
            if t[spaces..].starts_with("- ") {
                depth = spaces / 2;
                t = t[spaces + 2..].to_string();
            }
            if t == "∅" {
                t.clear();
            }
            if let Some(b) = t.find('▮') {
                t = t.replacen('▮', "", 1);
                caret = Some(p(i, b));
            }
            let hidden = hidden_below.is_some_and(|d| depth > d);
            if !hidden {
                hidden_below = folded.then_some(depth);
                let x0 = depth * 4;
                let rows = crate::wrap(&t, width - x0);
                stops.extend(note_stops(i, &t, &rows, 0, x0));
            }
            texts.push(t);
        }
        let steps = steps
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                let (k, v) = l.trim().split_once(' ').unwrap();
                (k.to_string(), v.trim().to_string())
            })
            .collect();
        Scenario {
            texts,
            stops,
            caret: caret.expect("a caret: ▮ or start"),
            page,
            steps,
        }
    }

    fn at(v: &str) -> Pos {
        let (a, b) = v.split_once(':').unwrap();
        p(a.parse().unwrap(), b.parse().unwrap())
    }

    /// Run a scenario's steps and check each.
    fn run(src: &str) {
        let sc = scenario(src);
        let l = Layout {
            texts: sc.texts.iter().map(String::as_str).collect(),
            stops: sc.stops.clone(),
        };
        let (mut c, mut goal, mut anchor) = (sc.caret, None, None::<Pos>);
        let shown = |c: Pos| {
            format!(
                "{}▮{}",
                &sc.texts[c.line][..c.byte],
                &sc.texts[c.line][c.byte..]
            )
        };
        let check = |what: &str, c: Pos, want: &str, k: usize| {
            if want
                .split_once(':')
                .is_some_and(|(a, b)| a.parse::<usize>().is_ok() && b.parse::<usize>().is_ok())
            {
                assert_eq!(c, at(want), "step {k} {what}");
            } else {
                assert!(
                    shown(c).contains(want),
                    "step {k} {what}: want {want}, got {c:?} {}",
                    shown(c)
                );
            }
        };
        for (k, (key, want)) in sc.steps.iter().enumerate() {
            if key == "anchor" {
                check("anchor", anchor.expect("a selection"), want, k);
                continue;
            }
            if key.starts_with('⇧') {
                anchor.get_or_insert(c);
            } else {
                anchor = None;
            }
            (c, goal) = match key.trim_start_matches('⇧') {
                "↓" => l.vertical(c, goal, 1),
                "↑" => l.vertical(c, goal, -1),
                "PgDn" => l.vertical(c, goal, sc.page),
                "PgUp" => l.vertical(c, goal, -sc.page),
                "→" => (l.right(c), None),
                "←" => (l.left(c), None),
                "Home" => (l.home_end(c, false), None),
                "End" => (l.home_end(c, true), None),
                "⌥→" => (l.word_right(c), None),
                "⌥←" => (l.word_left(c), None),
                "⌃Home" => (l.start(), None),
                "⌃End" => (l.end(), None),
                other => panic!("unknown key {other}"),
            };
            check(key, c, want, k);
        }
    }

    const F1: &str = "The quick brown fox jumps over the lazy dog and keeps running through the long grass until dusk.

Short one.

  - Bullet item that is long enough to wrap onto a second row.

∅

Last note.";

    fn f1_run(start: &str, steps: &str) {
        run(&format!("width 40\nstart {start}\n---\n{F1}\n---\n{steps}"));
    }

    #[test]
    fn g1_the_users_bug_down_from_a_wrapped_rows_start() {
        run("width 40
---
The quick brown fox jumps over the lazy dog and keeps running through the long ▮grass until dusk.

Short one.

  - Bullet item that is long enough to wrap onto a second row.
---
↓ ▮Short one.
↓ ▮Bullet item");
    }

    #[test]
    fn g2_down_through_everything_and_back() {
        f1_run(
            "0:40",
            "↓ ▮grass\n↓ ▮Short\n↓ ▮Bullet\n↓ ▮wrap onto\n↓ 3:0\n↓ ▮Last\n↓ note.▮\n↓ note.▮\n↑ 3:0",
        );
    }

    #[test]
    fn g3_the_goal_survives_a_short_note_and_an_indent() {
        f1_run("0:91", "↓ one.▮\n↓ Bullet i▮tem\n↑ one.▮\n↑ until ▮dusk");
    }

    #[test]
    fn g4_end_on_a_wrapped_row_then_down() {
        f1_run("0:10", "End lazy▮ dog\n↓ long▮ grass\n↓ dusk.▮\n↓ one.▮");
    }

    #[test]
    fn g5_to_g7_right_and_left() {
        f1_run("0:38", "→ lazy▮ dog\n→ lazy ▮dog\n← lazy▮ dog");
        f1_run("2:57", "→ row.▮\n→ 3:0\n→ ▮Last");
        f1_run("1:9", "→ one.▮\n→ ▮Bullet");
    }

    #[test]
    fn g8_home_and_end_twice() {
        f1_run(
            "0:50",
            "Home ▮dog and\nHome ▮The quick\nEnd lazy▮ dog\nEnd dusk.▮",
        );
    }

    #[test]
    fn g9_to_g11_words() {
        f1_run("0:37", "⌥→ lazy▮ dog\n⌥→ dog▮ and");
        f1_run("1:6", "⌥→ one▮.\n⌥→ Bullet▮ item");
        f1_run("2:2", "⌥← ▮Bullet\n⌥← ▮one.");
    }

    #[test]
    fn g12_to_g14_goal_at_the_ends_and_pages() {
        f1_run("4:3", "↓ note.▮\n↑ 3:0\n↑ ▮wrap onto\n↑ ▮Bullet\n↑ Sho▮rt");
        f1_run("0:5", "↑ ▮The quick\n↓ 0:45");
        f1_run("0:5", "PgDn Short▮ one\nPgDn 3:0\nPgDn note.▮");
    }

    #[test]
    fn g15_select_down() {
        f1_run(
            "0:79",
            "⇧↓ ▮Short\nanchor ▮grass\n⇧↓ ▮Bullet\nanchor ▮grass",
        );
    }

    /// F2: rows that aren't stops. A meta on its own row and a footer never enter the stop map
    /// (they're drawn rows, not text rows), and folded children are left out of it.
    const F2: &str = "The quick brown fox jumps over the lazy dog and keeps running through the long grass until dusk.

- Short one. {meta}

  - Bullet item that is long enough to wrap onto a second row. {folded}

    - hidden child one

    - hidden child two

∅

Last note.";

    #[test]
    fn g16_to_g18_rows_that_arent_stops() {
        let f2 = |start: &str, steps: &str| {
            run(&format!("width 40\nstart {start}\n---\n{F2}\n---\n{steps}"))
        };
        f2("1:4", "↓ ▮Bullet\n↑ Shor▮t");
        f2("2:40", "↓ 5:0");
        f2("6:2", "↓ note.▮\n↓ note.▮");
    }

    #[test]
    fn g20_to_g22_wide_graphemes_and_soft_breaks() {
        let f3 = |start: &str, steps: &str| {
            run(&format!(
                "width 40\nstart {start}\n---\n日本語のテキスト\n\nabcdef\n\na\nbc\n---\n{steps}"
            ))
        };
        f3("1:5", "↑ 日本▮語\n↓ abcde▮f");
        f3("2:1", "→ ▮bc\n← a▮");
        f3("2:2", "↑ ▮a\n↓ ▮bc\n↓ bc▮");
    }

    // ---- invariants (motion.md §6) over random documents and widths ----------------------------

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545F4914F6CDD1D)
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    const WORDS: &[&str] = &[
        "a",
        "the",
        "quick",
        "brown",
        "fox",
        "日本語",
        "テキスト",
        "👨‍👩‍👧",
        "e\u{301}",
        "supercalifragilisticexpialidocious",
        "https://example.com/a/very/long/path/that/never/breaks",
        "x.",
        "—",
        "\n",
        "🇯🇵",
        "",
    ];

    fn random_doc(r: &mut Rng) -> Vec<(usize, String)> {
        (0..1 + r.below(8))
            .map(|_| {
                let x0 = r.below(4) * 4;
                let mut t = String::new();
                for k in 0..r.below(30) {
                    if k > 0 && r.below(4) > 0 {
                        t.push(' ');
                    }
                    t.push_str(WORDS[r.below(WORDS.len())]);
                }
                (x0, t)
            })
            .collect()
    }

    /// Every position in layout order: each stop row's boundaries from start to end, once each.
    fn positions(l: &Layout) -> Vec<Pos> {
        let mut out: Vec<Pos> = Vec::new();
        for s in &l.stops {
            let t = l.texts[s.line];
            let mut b = s.start;
            loop {
                let pos = p(s.line, b);
                if out.last() != Some(&pos) {
                    out.push(pos);
                }
                if b >= s.end {
                    break;
                }
                b = crate::next_char(t, b);
            }
        }
        out
    }

    #[test]
    fn invariants_hold_on_random_documents() {
        let mut r = Rng(0x9E3779B97F4A7C15);
        for case in 0..400 {
            let notes = random_doc(&mut r);
            let w = 20 + r.below(101);
            let refs: Vec<(usize, &str)> = notes.iter().map(|(x, t)| (*x, t.as_str())).collect();
            let f = Fx::new(&refs, w);
            let l = f.l();
            let all = positions(&l);
            let ctx = || format!("case {case} w {w}: {notes:?}");
            // I1: every position is a boundary owned by exactly one stop row, and rows don't overlap.
            for pos in &all {
                let t = l.texts[pos.line];
                assert!(t.is_char_boundary(pos.byte), "{}", ctx());
                let owners = l
                    .stops
                    .iter()
                    .filter(|s| s.line == pos.line && s.start <= pos.byte && pos.byte <= s.end)
                    .count();
                assert_eq!(owners, 1, "{pos:?} has {owners} rows · {}", ctx());
            }
            // I5: → visits every position once and reaches the end; ← reverses it.
            let mut c = l.start();
            let mut seen = vec![c];
            while c != l.end() {
                let n = l.right(c);
                assert_ne!(n, c, "→ stuck at {c:?} · {}", ctx());
                c = n;
                seen.push(c);
            }
            assert_eq!(seen, all, "→ order · {}", ctx());
            let mut back = vec![c];
            while c != l.start() {
                c = l.left(c);
                back.push(c);
            }
            back.reverse();
            assert_eq!(back, all, "← order · {}", ctx());
            for pos in all.iter().step_by(1 + all.len() / 40) {
                let r0 = l.owner(*pos).unwrap();
                // I2: ↓ lands on the next stop row (or the end from the last), ↑ the previous.
                let (d, g) = l.vertical(*pos, None, 1);
                if r0 + 1 < l.stops.len() {
                    assert_eq!(l.owner(d), Some(r0 + 1), "↓ from {pos:?} · {}", ctx());
                    // I3: back up returns when the goal is reachable on both rows.
                    let x = l.x(*pos);
                    if l.x(d) == x {
                        assert_eq!(l.vertical(d, g, -1).0, *pos, "↓↑ from {pos:?} · {}", ctx());
                    }
                } else {
                    assert_eq!(d, l.end());
                }
                let (u, _) = l.vertical(*pos, None, -1);
                if r0 > 0 {
                    assert_eq!(l.owner(u), Some(r0 - 1), "↑ from {pos:?} · {}", ctx());
                } else {
                    assert_eq!(u, l.start());
                }
                // I4: ↓ (number of stop rows) times reaches the end.
                let (mut c, mut g) = (*pos, None);
                for _ in 0..l.stops.len() {
                    (c, g) = l.vertical(c, g, 1);
                }
                assert_eq!(c, l.end(), "↓× from {pos:?} · {}", ctx());
                // Every motion lands on a valid position.
                for q in [
                    l.word_left(*pos),
                    l.word_right(*pos),
                    l.home_end(*pos, true),
                    l.home_end(*pos, false),
                ] {
                    assert_eq!(l.valid(q), q, "{q:?} from {pos:?} · {}", ctx());
                }
            }
        }
    }
}
