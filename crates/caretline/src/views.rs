//! Several views of one document: [`update_doc`], rebasing views through edits made in
//! another, and folds (which belong to a view).
//!
//! A [`Document`] holds what every view shares: the text, marks, undo history, outline and
//! clipboard. A [`View`] holds what one window on it has: its selection, scroll, viewport,
//! folds, status line and whether it may edit. A message acts through one view (the
//! *acting* view); when it changes the text, every other view's selection is mapped through
//! the same changes, so each keeps its place in the text it was on.

use crate::helix::{Assoc, ChangeSet, Range, Selection, SmallVec};
use crate::marks::MarkId;
use crate::msg::{Effect, Msg};
use crate::outline::Outline;
use crate::state::{Document, State, View};
use crate::update::step;

/// Applies one message through `views[acting]` to `doc`, then rebases every other view
/// through the changes it made to the text. Returns the effects for the runtime.
///
/// - Editing messages ([`Msg::edits`]) on a read-only view change nothing and return
///   [`Effect::Refused`].
/// - Undo and redo are the document's: they take back the last step whichever view made it,
///   and put the acting view's selection where the step happened.
/// - A view's scroll stays on the text it showed; its folds drop when their blocks go.
///
/// `update(state, msg)` is `update_doc(&mut state.doc, [&mut state.view], 0, msg)`.
pub fn update_doc(doc: &mut Document, views: &mut [View], acting: usize, msg: Msg) -> Vec<Effect> {
    doc.journal.0.clear();
    if msg.is_external() {
        return crate::external::apply(doc, views, msg);
    }
    let Some(view) = views.get(acting) else { return Vec::new() };
    if view.read_only && msg.edits() {
        return vec![Effect::Refused];
    }
    let tops = tops(doc, views);
    let edits = doc.edits.0;
    // A typing run belongs to one view.
    let run_view = doc.run.map(|r| r.view);
    if run_view.is_some_and(|v| v != acting) {
        doc.run = None;
    }
    let mut state = State { doc: std::mem::take(doc), view: std::mem::take(&mut views[acting]) };
    let effects = step(&mut state, msg);
    *doc = state.doc;
    views[acting] = state.view;
    if let Some(r) = &mut doc.run {
        r.view = acting;
    }
    if doc.edits.0 != edits {
        // The acting view's folds drop with their blocks too (whatever path the edit took).
        views[acting].folds.retain(|id| doc.marks.contains(*id));
        let journal = std::mem::take(&mut doc.journal.0);
        for (i, v) in views.iter_mut().enumerate() {
            if i != acting {
                rebase(doc, v, &journal, tops[i]);
            }
        }
    }
    doc.journal.0.clear();
    effects
}

/// Where each view's top line starts, as a char position (mapped through an edit, it keeps
/// the view on the same text).
pub(crate) fn tops(doc: &Document, views: &[View]) -> Vec<usize> {
    let last = doc.text.len_lines().saturating_sub(1);
    views.iter().map(|v| doc.text.line_to_char(v.scroll.line.min(last))).collect()
}

/// Maps a view through text changes made elsewhere (`journal`, in order), then fits it to
/// the document: selections inside the text and out of block markers, out of folded
/// blocks, folds on live blocks, the scroll on the line it showed (`top`, before).
pub(crate) fn rebase(doc: &Document, view: &mut View, journal: &[ChangeSet], top: usize) {
    let mut top = top;
    let mut selection = view.selection.clone();
    for cs in journal {
        selection = selection.map(cs);
        view.wrap.edited(cs);
        top = cs.map_pos(top, Assoc::Before);
    }
    view.selection = selection;
    if !journal.is_empty() {
        view.word_drag = None;
        view.scroll.line = doc.text.char_to_line(top.min(doc.text.len_chars()));
    }
    view.fit(doc);
    if doc.outline.is_some() {
        let calm = Msg::Tick { now_ms: doc.now_ms };
        if let Some(sel) = unhidden(doc, view, &view.selection, false) {
            view.selection = sel;
        }
        let prev = view.selection.clone();
        if let Some(sel) = crate::outline::rules::normalized(doc, &view.selection, &prev, &calm) {
            view.selection = sel;
        }
    }
}

// ---------------------------------------------------------------------------------------
// Folds

/// Folds (`Some(true)`), unfolds (`Some(false)`) or toggles (`None`) block `id` in the acting
/// view. Only a block with children folds.
pub(crate) fn fold(state: &mut State, id: MarkId, set: Option<bool>) {
    let Some(o) = state.doc.blocks() else {
        state.view.status = Some("only in outline documents".into());
        return;
    };
    let Some(i) = o.index_of(id) else {
        state.view.status = Some("no such block".into());
        return;
    };
    let on = set.unwrap_or(!state.view.folds.contains(&id));
    if !on {
        state.view.folds.remove(&id);
    } else if o.subtree_end(i) > i + 1 {
        state.view.folds.insert(id);
    } else {
        state.view.status = Some("nothing to fold: no children".into());
    }
}

/// The lines a view hides, as sorted, disjoint `[from, to)` line ranges: the children of
/// every folded block.
pub fn hidden_lines(o: &Outline, folds: &std::collections::BTreeSet<MarkId>) -> Vec<(usize, usize)> {
    if folds.is_empty() {
        return Vec::new();
    }
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for &id in folds {
        let Some(i) = o.index_of(id) else { continue };
        let end = o.subtree_end(i);
        if end > i + 1 {
            ranges.push((o.blocks[i + 1].first_line, o.blocks[end - 1].last_line() + 1));
        }
    }
    ranges.sort();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (a, b) in ranges {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    merged
}

/// The hidden range holding `line`, if any.
pub(crate) fn hidden_range(hidden: &[(usize, usize)], line: usize) -> Option<(usize, usize)> {
    let i = hidden.partition_point(|&(_, b)| b <= line);
    hidden.get(i).copied().filter(|&(a, _)| a <= line)
}

/// After a message, moves selection ends out of folded blocks: forward motion to the first
/// line after them, anything else to the fold's own end.
pub(crate) fn unhide(state: &mut State, msg: &Msg) {
    let forward = matches!(msg, Msg::Move { dir: crate::msg::Dir::Forward, .. });
    if let Some(sel) = unhidden(&state.doc, &state.view, &state.view.selection, forward) {
        state.view.selection = sel;
    }
}

fn unhidden(doc: &Document, view: &View, selection: &Selection, forward: bool) -> Option<Selection> {
    if view.folds.is_empty() {
        return None;
    }
    let o = doc.blocks()?;
    let hidden = hidden_lines(&o, &view.folds);
    if hidden.is_empty() {
        return None;
    }
    let text = doc.text.slice(..);
    let lines = text.len_lines();
    let out = |pos: usize| -> usize {
        let line = text.char_to_line(pos.min(text.len_chars()));
        match hidden_range(&hidden, line) {
            None => pos,
            Some((a, b)) => {
                if forward && b < lines {
                    text.line_to_char(b)
                } else {
                    // The end of the line before the hidden lines: the folded block's end.
                    crate::helix::line_ending::line_end_char_index(&text, a.saturating_sub(1))
                }
            }
        }
    };
    let ranges: SmallVec<[Range; 1]> = selection
        .iter()
        .map(|r| Range { anchor: out(r.anchor), head: out(r.head), old_visual_position: r.old_visual_position })
        .collect();
    let fixed = Selection::new(ranges, selection.primary_index());
    (fixed != *selection).then_some(fixed)
}
