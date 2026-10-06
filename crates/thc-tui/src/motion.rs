//! thc's side of caret motion: the stop map built from the document's own wrap (the
//! renderer's), and `Doc::motion`. The rules live in caretline (the editor engine).

use crate::doc::{Doc, Line};
pub use caretline::motion::{Layout, Motion, Stop, note_stops};

/// The document's stops at the current width: every row of every note not hidden by a fold.
pub fn stops(d: &mut Doc, width_of: &dyn Fn(&Line) -> usize) -> Vec<Stop> {
    let mut out = Vec::new();
    for i in 0..d.lines.len() {
        if d.lines[i].folded_hidden(&d.lines[..i]) {
            continue;
        }
        let w = width_of(&d.lines[i]);
        let rows = d.rows_of(i, w);
        let l = &d.lines[i];
        out.extend(note_stops(i, &l.text, &rows, crate::doc_ui::marker_len(l), l.depth * 4));
    }
    out
}

/// The layout of a document at the current width.
pub fn layout<'a>(d: &'a mut Doc, width_of: &dyn Fn(&Line) -> usize) -> Layout<'a> {
    let stops = stops(d, width_of);
    Layout { texts: d.lines.iter().map(|l| l.text.as_str()).collect(), stops }
}

impl Doc {
    /// Move the caret (⇧: extend the selection from where it began). Pure over the layout: it
    /// never edits, saves or marks anything dirty. ↑ ↓ and pages keep the goal column; every
    /// other motion resets it (§3).
    pub fn motion(&mut self, m: Motion, select: bool, width_of: &dyn Fn(&Line) -> usize) {
        // A motion without ⇧ and with a selection collapses it, as a macOS text field does
        // (editing.md §2): ← and → stop at its start or end; the others move from
        // the start (up, left) or the end (down, right).
        let collapse = if select { None } else { self.selection() };
        self.select(select);
        if let Some((s, e)) = collapse {
            match m {
                Motion::Left => {
                    self.caret = s;
                    self.goal_col = None;
                    return;
                }
                Motion::Right => {
                    self.caret = e;
                    self.goal_col = None;
                    return;
                }
                Motion::Up | Motion::WordLeft | Motion::Home | Motion::DocStart | Motion::NoteUp => self.caret = s,
                Motion::Page(n) if n < 0 => self.caret = s,
                _ => self.caret = e,
            }
            self.goal_col = None;
        }
        let (caret, goal) = (self.caret, self.goal_col);
        let l = layout(self, width_of);
        if l.stops.is_empty() {
            return;
        }
        let (c, g) = match m {
            Motion::Up => l.vertical(caret, goal, -1),
            Motion::Down => l.vertical(caret, goal, 1),
            Motion::Page(n) => l.vertical(caret, goal, n),
            Motion::Left => (l.left(caret), None),
            Motion::Right => (l.right(caret), None),
            Motion::WordLeft => (l.word_left(caret), None),
            Motion::WordRight => (l.word_right(caret), None),
            Motion::Home => (l.home_end(caret, false), None),
            Motion::End => (l.home_end(caret, true), None),
            Motion::DocStart => (l.start(), None),
            Motion::DocEnd => (l.end(), None),
            Motion::NoteUp | Motion::NoteDown => (l.note_step(caret, m == Motion::NoteDown), None),
        };
        self.caret = c;
        self.goal_col = g;
    }
}


impl Doc {
    /// Apply an editing or motion command (caretline's `Command`) to the document. The rules are
    /// still thc's editing code; this is the seam they move behind (engine-extraction.md §3).
    pub fn apply(&mut self, cmd: caretline::Command, width_of: &dyn Fn(&Line) -> usize) -> caretline::Outcome {
        // A kind or depth change (⌃T, a marker typed or deleted, Tab) never moves another
        // line: every note keeps the blank line it had (writing.md §1).
        // (Only those: Enter on an empty item leaves the list, and the paragraph it makes takes
        // the blank line a paragraph has, as it always did.) ⌫ / Delete run on every key: only
        // the caret's neighbourhood is checked, O(1). ⌃T and Tab may touch a whole selection:
        // every block, O(n), on a rare key. Motions check nothing (a whole-document scan per
        // key cost 60 ms at 5,000 lines).
        use caretline::Command as C;
        match cmd {
            C::Backspace | C::Delete => {
                let before = self.near_caret();
                let out = self.apply_inner(cmd, width_of);
                self.pin_near(&before);
                out
            }
            C::TaskCycle | C::Indent | C::Outdent => {
                let gaps = self.gaps();
                let out = self.apply_inner(cmd, width_of);
                self.pin_gaps(&gaps);
                out
            }
            _ => self.apply_inner(cmd, width_of),
        }
    }

    fn apply_inner(&mut self, cmd: caretline::Command, width_of: &dyn Fn(&Line) -> usize) -> caretline::Outcome {
        use caretline::{Command as C, Outcome as O};
        match cmd {
            C::Newline => self.newline(),
            C::SoftBreak => self.soft_break(),
            C::Backspace => self.backspace(),
            C::Delete => self.delete_forward(),
            C::DeleteWordBack => self.delete_word_back(),
            C::KillToEnd => self.kill_to_end(),
            C::KillToStart => self.kill_to_start(),
            C::Indent => {
                if !self.nest(1) {
                    return O::Nothing("paragraphs don't nest · - makes a bullet");
                }
            }
            C::Outdent => {
                self.nest(-1);
            }
            C::TaskCycle => {
                if self.task_cycle() == "done" {
                    return O::Completed;
                }
            }
            C::MoveLine(n) => {
                if let Err(m) = self.move_line(n) {
                    return O::Nothing(m);
                }
            }
            C::SelectAll => {
                self.anchor = Some(crate::doc::Pos { line: 0, byte: 0 });
                let last = self.lines.len() - 1;
                self.caret = crate::doc::Pos { line: last, byte: self.lines[last].text.len() };
            }
            C::Undo => return if self.undo() { O::Restored } else { O::Nothing("nothing to undo") },
            C::Redo => return if self.redo() { O::Restored } else { O::Nothing("nothing to redo") },
            C::Move { motion, select } => self.motion(motion, select, width_of),
        }
        O::Done
    }
}
