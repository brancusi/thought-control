//! thc's side of caret motion: the stop map built from the document's own wrap (the
//! renderer's), handed to caretline (the editor engine), whose rules move the caret.

use super::doc::{Doc, Line};
use caretline::motion::{Stop, note_stops};
#[cfg(test)]
use caretline::motion::Layout;
use std::collections::{HashMap, HashSet};

/// The stops of `lines` at the current width: every row of every note not hidden by a fold.
fn stops_of(lines: &[Line], folds: &HashSet<String>, wraps: &mut HashMap<(u64, usize), Vec<(usize, usize)>>, width_of: &dyn Fn(&Line) -> usize) -> Vec<Stop> {
    let mut out = Vec::new();
    for i in 0..lines.len() {
        if crate::doc_ui::folded_hidden(lines, i, folds) {
            continue;
        }
        let l = &lines[i];
        let rows = super::doc::rows(wraps, l, width_of(l));
        out.extend(note_stops(i, &l.text, &rows, crate::doc_ui::marker_len(l), l.depth * 4));
    }
    out
}

/// The document's stops at the current width.
#[cfg(test)]
pub(super) fn stops(d: &mut Doc, width_of: &dyn Fn(&Line) -> usize) -> Vec<Stop> {
    let Doc { engine, view, wraps, .. } = d;
    stops_of(engine.lines(), &view.folds, wraps, width_of)
}

/// The layout of a document at the current width.
#[cfg(test)]
pub(super) fn layout<'a>(d: &'a mut Doc, width_of: &dyn Fn(&Line) -> usize) -> Layout<'a> {
    let stops = stops(d, width_of);
    Layout { texts: d.lines().iter().map(|l| l.text.as_str()).collect(), stops }
}

impl Doc {
    /// Apply an editing or motion command (caretline's `Command`) at the caret: caretline's
    /// rules, with thc's own layout for motion. Undo and redo keep thc's save state (Doc::undo).
    pub(super) fn apply_command(&mut self, cmd: caretline::Command, width_of: &dyn Fn(&Line) -> usize) -> caretline::Outcome {
        use caretline::{Command as C, Outcome as O};
        match cmd {
            C::Undo => return if self.undo() { O::Restored } else { O::Nothing("nothing to undo") },
            C::Redo => return if self.redo() { O::Restored } else { O::Nothing("nothing to redo") },
            _ => {}
        }
        let Doc { engine, view, wraps, .. } = self;
        let mut host = |lines: &[Line], v: &caretline::View<String>| stops_of(lines, &v.folds, wraps, width_of);
        let out = engine.apply_and_rebase_with(view, [], cmd, &mut host).expect("the main view edits");
        match (cmd, out) {
            (C::Indent, O::Nothing(_)) => O::Nothing("paragraphs don't nest · - makes a bullet"),
            _ => out,
        }
    }
}
