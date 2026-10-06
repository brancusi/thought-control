//! A kind change never moves text: ⌃T on a note changes that note, and
//! every other line stays on its screen row; no blank line between notes appears or goes.
//! Random documents are typed into a real frame, ⌃T is pressed on every row of every note, and
//! the rows of all other lines are compared.

use crate::app::{App, View};
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use std::collections::HashMap;

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

fn key(app: &mut App, code: KeyCode, m: KeyModifiers) {
    crate::input::handle_key(app, KeyEvent::new(code, m));
}

fn typed(app: &mut App, s: &str) {
    for c in s.chars() {
        if c == '\n' {
            key(app, KeyCode::Enter, KeyModifiers::NONE);
        } else {
            key(app, KeyCode::Char(c), KeyModifiers::NONE);
        }
    }
}

/// A random document as a person types it: paragraphs (some with line breaks), blank lines,
/// bullets and tasks, in any order.
fn source(r: &mut Rng) -> String {
    let mut s = String::new();
    for k in 0..(2 + r.below(5)) {
        if k > 0 {
            // After a list item, Enter twice leaves the list; after a paragraph one more
            // Enter is a blank line (a new note).
            s.push_str(if r.below(2) == 0 { "\n\n" } else { "\n" });
        }
        match r.below(4) {
            0 => s.push_str(&format!("para {k} one\nline two of {k}")),
            1 => s.push_str(&format!("para {k} alone")),
            2 => s.push_str(&format!("- bullet {k}")),
            _ => s.push_str(&format!("[ ] task {k}")),
        }
    }
    s
}

/// Each line's first screen row, by note id; the rows of the frame that hold text.
fn rows(app: &App) -> HashMap<String, u16> {
    let d = app.doc.as_ref().unwrap();
    let mut m = HashMap::new();
    for h in &app.render.doc_hits {
        if let Some(l) = d.lines().get(h.line) {
            m.entry(l.id.clone()).and_modify(|y: &mut u16| *y = (*y).min(h.y)).or_insert(h.y);
        }
    }
    m
}

/// Each note's own height: the rows it draws (text and meta), not the blank line before it.
fn heights(app: &App) -> HashMap<String, u16> {
    let d = app.doc.as_ref().unwrap();
    let mut m: HashMap<String, std::collections::BTreeSet<u16>> = HashMap::new();
    for h in &app.render.doc_hits {
        if let Some(l) = d.lines().get(h.line) {
            m.entry(l.id.clone()).or_default().insert(h.y);
        }
    }
    m.into_iter().map(|(k, v)| (k, v.len() as u16)).collect()
}

fn picture(term: &Terminal<TestBackend>) -> String {
    let b = term.backend().buffer();
    (0..b.area.height).map(|y| (0..b.area.width).map(|x| b[(x, y)].symbol().to_string()).collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n")
}

#[test]
fn a_kind_change_never_moves_another_line() {
    let mut r = Rng(0x9E3779B97F4A7C15);
    let mut fails: Vec<String> = vec![];
    for case in 0..60 {
        let src = source(&mut r);
        let (_s, vault) = crate::fuzz::scratch(&format!("kinds-{case}"));
        crate::SNAPSHOT.with(|s| s.set(true));
        let mut app = App::new(vault).unwrap();
        app.daemon_live = false;
        app.journal_date = app.today;
        app.set_view(View::Journal);
        let mut term = Terminal::new(TestBackend::new(100, 40)).unwrap();
        term.draw(|f| crate::ui::draw_app(f, &mut app)).unwrap();
        typed(&mut app, &src);
        term.draw(|f| crate::ui::draw_app(f, &mut app)).unwrap();
        // Every row of every note: put the caret there, ⌃T, compare, undo.
        let targets: Vec<(usize, usize)> = app.render.doc_hits.iter().map(|h| (h.line, h.start)).collect();
        for (line, byte) in targets {
            {
                let d = app.doc.as_mut().unwrap();
                if line >= d.lines().len() || byte > d.lines()[line].text.len() {
                    continue;
                }
                d.view.anchor = None;
                d.view.caret = crate::doc::Pos { line, byte };
            }
            term.draw(|f| crate::ui::draw_app(f, &mut app)).unwrap();
            let before = rows(&app);
            let heights0 = heights(&app);
            let pic0 = picture(&term);
            let d0 = app.doc.as_ref().unwrap();
            let ids0: Vec<String> = d0.lines().iter().map(|l| l.id.clone()).collect();
            let gaps0 = d0.gaps();
            key(&mut app, KeyCode::Char('t'), KeyModifiers::CONTROL);
            term.draw(|f| crate::ui::draw_app(f, &mut app)).unwrap();
            let after = rows(&app);
            let heights1 = heights(&app);
            let d1 = app.doc.as_ref().unwrap();
            let ids1: Vec<String> = d1.lines().iter().map(|l| l.id.clone()).collect();
            // The changed notes: this line, notes made by a split, notes gone in a join.
            let me = &ids0[line];
            let changed0: Vec<&String> = ids0.iter().filter(|id| *id == me || !ids1.contains(id)).collect();
            let changed1: Vec<&String> = ids1.iter().filter(|id| *id == me || !ids0.contains(id)).collect();
            let start = ids0.iter().position(|id| changed0.contains(&id)).unwrap();
            let own = |h: &HashMap<String, u16>, ids: &[&String]| ids.iter().map(|id| h.get(*id).copied().unwrap_or(0) as i32).sum::<i32>();
            let delta = own(&heights1, &changed1) - own(&heights0, &changed0);
            let mut bad = vec![];
            for (k, id) in ids0.iter().enumerate() {
                if changed0.contains(&id) {
                    continue;
                }
                let (Some(&y0), Some(&y1)) = (before.get(id), after.get(id)) else { continue };
                let want = if k < start { y0 as i32 } else { y0 as i32 + delta };
                if y1 as i32 != want {
                    bad.push(format!("line {k}: row {y0}→{y1}, want {want}"));
                }
                if let (Some(g0), Some(g1)) = (gaps0.get(id), d1.gaps().get(id)) {
                    if g0 != g1 {
                        bad.push(format!("line {k}: gap {g0}→{g1}"));
                    }
                }
            }
            if !bad.is_empty() {
                fails.push(format!("case {case}, ⌃T on line {line} byte {byte}: {bad:?}\n--- before\n{pic0}\n--- after\n{}", picture(&term)));
            }
            key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
            // EI14: and its undo restores every gap.
            let back = app.doc.as_ref().unwrap().gaps();
            if back != gaps0 {
                fails.push(format!("case {case}, ⌃T then undo on line {line}: gaps {gaps0:?} → {back:?}"));
            }
        }
    }
    assert!(fails.is_empty(), "{} kind changes moved other lines; first 3:\n{}", fails.len(), fails.iter().take(3).cloned().collect::<Vec<_>>().join("\n\n"));
}
