//! `update`: the one place state changes. Pure and deterministic: no clock, no randomness,
//! no I/O. Time arrives in `Msg::Tick`; work for the outside world leaves as `Effect`s.

use crate::helix::chars::{char_is_line_ending, char_is_word};
use crate::helix::graphemes::{next_grapheme_boundary, prev_grapheme_boundary};
use crate::helix::history::State as HistoryState;
use crate::helix::line_ending::line_end_char_index;
use crate::helix::{Range, RopeSlice, Selection, SmallVec, Tendril, Transaction};
use crate::layout::{ensure_caret_visible, Layout};
use crate::msg::{By, Dir, Effect, Msg};
use crate::helix::transaction::Operation;
use crate::marks::{ClipMark, Clipboard, Mark, MarkDelta, Marks};
use crate::state::{EditRun, RunKind, Scroll, State, RUN_GAP_MS, RUN_MAX_CHARS, RUN_WORD_BREAK_CHARS};

/// Applies one message to a single-view state (one document, one view). Returns the effects
/// for the runtime to perform. For several views of one document, see
/// [`crate::update_doc`].
pub fn update(state: &mut State, msg: Msg) -> Vec<Effect> {
    if msg.is_external() {
        return crate::views::update_doc(&mut state.doc, std::slice::from_mut(&mut state.view), 0, msg);
    }
    if state.view.read_only && msg.edits() {
        return vec![Effect::Refused];
    }
    state.doc.journal.0.clear();
    let effects = step(state, msg);
    state.doc.journal.0.clear();
    effects
}

/// One message through one view: everything `update` does, without the read-only check.
/// The text changes it made are left in the document's journal for the caller to rebase
/// other views with.
pub(crate) fn step(state: &mut State, msg: Msg) -> Vec<Effect> {
    let mut effects = Vec::new();
    let passive = msg.is_passive();
    if !passive {
        state.view.status = None;
        if !matches!(msg, Msg::Quit) {
            state.view.quit_armed = false;
        }
    }
    // Anything but an edit of the same kind (or a passive message) ends an edit run.
    let keeps_run = passive
        || matches!(
            msg,
            Msg::InsertText { .. } | Msg::InsertNewline | Msg::DeleteBackward | Msg::DeleteForward
        );
    if !keeps_run {
        state.doc.run = None;
    }
    let outline_on = state.doc.outline.is_some();
    let before = outline_on.then(|| (state.view.selection.clone(), caret_block(state), msg.clone(), state.view.status.clone()));
    if !passive && !matches!(msg, Msg::Click { extend: true, .. }) {
        state.view.word_drag = None;
    }
    let selection_before = state.view.selection.clone();
    let scrolls_freely = matches!(msg, Msg::ScrollView { .. });

    let pins = if outline_on { crate::outline::rules::pins_for(state, &msg) } else { None };
    let edits_before = state.doc.edits.0;
    let handled = if outline_on { crate::outline::rules::update(state, &msg) } else { None };
    if let Some(fx) = handled {
        effects.extend(fx);
    } else {
        plain(state, msg, &mut effects);
    }
    if let Some(pins) = pins.filter(|_| state.doc.edits.0 != edits_before) {
        crate::outline::rules::pin(state, pins);
    }

    if let Some((prev, prev_block, msg, status)) = before {
        crate::views::unhide(state, &msg);
        crate::outline::rules::normalize(state, &prev, &msg);
        let now = caret_block(state);
        if now != prev_block {
            effects.push(Effect::BlockLeft { from: prev_block, to: now });
        }
        if matches!(msg, Msg::Undo | Msg::Redo) && state.view.status.is_none() {
            effects.push(Effect::Restored);
        }
        if !state.view.config.status_bar && state.view.status != status {
            if let Some(text) = state.view.status.clone() {
                effects.push(Effect::Notice { text });
            }
        }
    }

    let edited = state.doc.edits.0 != edits_before;
    if edited {
        state.doc.rev += 1;
    }
    state.doc.dirty = state.compute_dirty();
    if state.doc.run.is_some_and(|r| r.revision != state.doc.history.current_revision()) {
        state.doc.run = None;
    }
    // A view scrolled freely stays put until the caret moves or the text changes.
    if state.view.free && !scrolls_freely && (edited || state.view.selection != selection_before) {
        state.view.free = false;
    }
    if state.view.free {
        crate::layout::clamp_scroll(state);
    } else {
        ensure_caret_visible(state);
    }
    effects
}

/// The block holding the primary caret, in an outline document.
fn caret_block(state: &State) -> Option<crate::marks::MarkId> {
    let o = state.blocks()?;
    Some(o.block_at(state.doc.text.slice(..), state.caret()).id)
}

/// A message as the plain-text editor handles it.
fn plain(state: &mut State, msg: Msg, effects: &mut Vec<Effect>) {
    match msg {
        Msg::InsertText { text } => {
            let text = normalize_line_endings(&text, state.doc.config.line_ending.as_str());
            if !text.is_empty() {
                insert(state, &text, Some(RunKind::Typing));
            }
        }
        Msg::InsertNewline | Msg::SoftBreak => {
            let le = state.doc.config.line_ending.as_str().to_string();
            insert(state, &le, Some(RunKind::Typing));
        }
        Msg::DeleteBackward => delete(state, Some(RunKind::DeleteBackward), |text, _, head| {
            (prev_grapheme_boundary(text, head), head)
        }),
        Msg::DeleteForward => delete(state, Some(RunKind::DeleteForward), |text, _, head| {
            (head, next_grapheme_boundary(text, head))
        }),
        // At a line's start (or end) a word delete removes only the line break, joining the
        // lines exactly as Backspace (or Delete) would.
        Msg::DeleteWordBackward => delete(state, None, |text, _, head| {
            let prev = prev_grapheme_boundary(text, head);
            if prev < head && char_is_line_ending(text.char(prev)) {
                (prev, head)
            } else {
                (word_left(text, head), head)
            }
        }),
        Msg::DeleteWordForward => delete(state, None, |text, _, head| {
            if head < text.len_chars() && char_is_line_ending(text.char(head)) {
                (head, next_grapheme_boundary(text, head))
            } else {
                (head, word_right(text, head))
            }
        }),
        Msg::DeleteToLineStart => {
            let layout = Layout::new(state);
            delete(state, None, |text, _, head| {
                let start = row_start(&layout, head);
                if start == head {
                    (prev_grapheme_boundary(text, head), head)
                } else {
                    (start, head)
                }
            })
        }
        Msg::DeleteToLineEnd => {
            let layout = Layout::new(state);
            delete(state, None, |text, _, head| {
                let end = row_end(&layout, head);
                if end == head {
                    (head, next_grapheme_boundary(text, head))
                } else {
                    (head, end)
                }
            })
        }
        Msg::KillLine => delete(state, None, |text, _, head| {
            let end = line_end_char_index(&text, text.char_to_line(head));
            if end == head {
                (head, next_grapheme_boundary(text, head))
            } else {
                (head, end)
            }
        }),
        Msg::Move { dir, by, extend } => motion(state, dir, by, extend),
        Msg::Click { col, row, extend } => {
            let layout = Layout::new(state);
            let pos = layout.pos_at_screen(&state.view.scroll, col, row);
            let primary = state.view.selection.primary();
            let range = match (extend, state.view.word_drag) {
                // After a double-click, extending goes by whole words.
                (true, Some((a, b))) => {
                    let (wa, wb) = word_at(state.doc.text.slice(..), pos);
                    if pos >= b { Range::new(a, wb.max(b)) } else { Range::new(b, wa.min(a)) }
                }
                (true, None) => Range::new(primary.anchor, pos),
                (false, _) => Range::point(pos),
            };
            state.view.selection = Selection::single(range.anchor, range.head);
        }
        Msg::SelectWordAt { pos } => {
            let pos = pos.min(state.doc.text.len_chars());
            let (a, b) = word_at(state.doc.text.slice(..), pos);
            state.view.selection = Selection::single(a, b);
            state.view.word_drag = Some((a, b));
        }
        Msg::PastePlain { text } => plain(state, Msg::Paste { text }, effects),
        Msg::Indent
        | Msg::Outdent
        | Msg::TaskCycle
        | Msg::SetStatus { .. }
        | Msg::MoveBlock { .. }
        | Msg::SelectBlock { .. }
        | Msg::InsertBlocks { .. } => {
            state.view.status = Some("only in outline documents".into());
        }
        Msg::Scroll { rows } => scroll(state, rows),
        Msg::ScrollView { rows } => {
            scroll_view(state, rows);
            state.view.free = true;
        }
        Msg::Fold { id } => crate::views::fold(state, id, Some(true)),
        Msg::Unfold { id } => crate::views::fold(state, id, Some(false)),
        Msg::ToggleFold { id } => crate::views::fold(state, id, None),
        Msg::SelectAll => {
            state.view.selection = Selection::single(0, state.doc.text.len_chars());
        }
        Msg::Collapse => {
            state.view.selection = state
                .view
                .selection
                .clone()
                .transform(|r| Range::point(r.head));
        }
        Msg::Copy => {
            if let Some(text) = selected_text(state) {
                state.view.status = Some(format!("copied {}", count_label(&text)));
                state.doc.clipboard = Clipboard::from(text.clone());
                effects.push(Effect::ClipboardSet { text });
            } else {
                state.view.status = Some("nothing selected".into());
            }
        }
        Msg::Cut => {
            if let Some(text) = selected_text(state) {
                state.view.status = Some(format!("cut {}", count_label(&text)));
                // One range: the register keeps the marks the cut takes, so pasting it
                // back re-creates the same ids.
                let single = (state.view.selection.len() == 1).then(|| state.view.selection.primary());
                // Only the non-empty ranges are cut; carets elsewhere stay as they are.
                let removed = delete_marks(state, None, |_, _, head| (head, head));
                let marks = match single {
                    Some(r) => carried(&removed, r.from(), r.to()),
                    None => Vec::new(),
                };
                state.doc.clipboard = Clipboard { text: text.clone(), external: None, marks };
                effects.push(Effect::ClipboardSet { text });
            } else {
                state.view.status = Some("nothing selected".into());
            }
        }
        Msg::Paste { text } => {
            // Outside text takes the document's line ending; the register came from this
            // document, so it goes back exactly as it was copied (with any marks it holds).
            let (text, marks) = match text {
                Some(text) => {
                    let text = normalize_line_endings(&text, state.doc.config.line_ending.as_str());
                    if state.doc.clipboard.is_own(&text) {
                        (state.doc.clipboard.text.clone(), state.doc.clipboard.marks.clone())
                    } else {
                        (text, Vec::new())
                    }
                }
                None => (state.doc.clipboard.text.clone(), state.doc.clipboard.marks.clone()),
            };
            if text.is_empty() {
                state.view.status = Some("the clipboard is empty".into());
            } else {
                paste_text(state, &text, &marks);
            }
        }
        Msg::Undo => {
            let rev = state.doc.history.current_revision();
            let txn = state.doc.history.undo().cloned();
            match txn {
                Some(txn) => apply_history(state, &txn, rev, true),
                None => state.view.status = Some("nothing to undo".into()),
            }
        }
        Msg::Redo => {
            let txn = state.doc.history.redo().cloned();
            let rev = state.doc.history.current_revision();
            match txn {
                Some(txn) => apply_history(state, &txn, rev, false),
                None => state.view.status = Some("nothing to redo".into()),
            }
        }
        Msg::Save => match &state.doc.path {
            Some(path) => {
                state.doc.saving = Some(state.doc.history.current_revision());
                let text = if state.doc.outline.is_some() {
                    crate::outline::markdown::to_file(state)
                } else {
                    state.doc.text.to_string()
                };
                effects.push(Effect::WriteFile { path: path.clone(), text });
            }
            None => state.view.status = Some("no file name: start caretline with a path".into()),
        },
        Msg::Saved => {
            if let Some(rev) = state.doc.saving.take() {
                state.doc.saved_revision = Some(rev);
                state.view.status = Some(format!("saved {}", state.name()));
            }
        }
        Msg::SaveFailed { err } => {
            state.doc.saving = None;
            state.view.status = Some(format!("save failed: {err}"));
        }
        Msg::Quit => {
            if state.doc.dirty && !state.view.quit_armed {
                state.view.quit_armed = true;
                state.view.status = Some("unsaved changes: quit again to discard them".into());
            } else {
                effects.push(Effect::Quit);
            }
        }
        Msg::Resize { width, height } => {
            state.view.viewport.width = width.max(1);
            state.view.viewport.height = height.max(1);
        }
        Msg::Tick { now_ms } => {
            state.doc.now_ms = now_ms;
        }
        Msg::ShowStatus { text } => {
            // One line: a newline would break the status bar.
            let line = text.lines().next().unwrap_or("").to_string();
            state.view.status = (!line.is_empty()).then_some(line);
        }
    }
}

/// The word (by Unicode word boundaries) at `pos`: a run of letters, of spaces, or one
/// punctuation mark.
fn word_at(text: RopeSlice, pos: usize) -> (usize, usize) {
    use unicode_segmentation::UnicodeSegmentation;
    let line = text.char_to_line(pos);
    let start = text.line_to_char(line);
    let s: String = text.line(line).chars().collect();
    let at = pos - start;
    let mut chars = 0;
    for w in s.split_word_bounds() {
        let n = w.chars().count();
        if at < chars + n && !w.contains(['\n', '\r']) {
            return (start + chars, start + chars + n);
        }
        chars += n;
    }
    (pos, pos)
}

/// Folds a message list into a state, dropping effects (headless replay).
pub fn replay(state: &mut State, msgs: impl IntoIterator<Item = Msg>) {
    for msg in msgs {
        update(state, msg);
    }
}

// ---------------------------------------------------------------------------------------
// Edits

/// Applies `txn` (which carries its resulting selection) and records it in the history,
/// amending the open run when `kind` continues it. Returns the marks the edit removed.
pub(crate) fn commit(state: &mut State, txn: Transaction, kind: Option<RunKind>, replaced_selection: bool) -> Vec<Mark> {
    commit_with(state, txn, Step { kind, replaced: replaced_selection, merge: false }, |_, _| {})
}

/// How an edit joins the undo history.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Step {
    /// The edit run it belongs to, if any (typing, held Backspace).
    pub kind: Option<RunKind>,
    /// It replaced a selection (which starts a new step).
    pub replaced: bool,
    /// Fold it into the current revision: the second half of one command.
    pub merge: bool,
}

/// [`commit`], with a hook that adjusts the marks after they are mapped through `txn`
/// (given the new text). An edit that changes only marks is still one undo step.
pub(crate) fn commit_with(
    state: &mut State,
    txn: Transaction,
    step: Step,
    fix: impl FnOnce(&mut Marks, RopeSlice),
) -> Vec<Mark> {
    let marks_before = state.doc.marks.clone();
    let old_text = state.doc.text.clone();
    let text_changed = !txn.changes().is_empty();
    if text_changed {
        txn.apply(&mut state.doc.text);
        state.view.wrap.edited(txn.changes());
        state.doc.journal.0.push(txn.changes().clone());
    }
    let removed = state.doc.marks.map(old_text.slice(..), state.doc.text.slice(..), txn.changes());
    let naive = state.doc.marks.clone();
    fix(&mut state.doc.marks, state.doc.text.slice(..));
    after_marks_changed(state);
    if state.doc.outline.is_some() {
        crate::outline::rules::settle(state);
    }
    let before_selection = state.view.selection.clone();
    if let Some(sel) = txn.selection() {
        state.view.selection = sel.clone();
    }
    if !text_changed && state.doc.marks == marks_before {
        return removed;
    }
    state.doc.edits.0 = state.doc.edits.0.wrapping_add(1);
    let tracked = !marks_before.is_empty() || !state.doc.marks.is_empty();

    let now = state.doc.now_ms;
    let changed = changed_chars(&txn);
    let continues = step.merge
        || match (step.kind, state.doc.run) {
            (Some(kind), Some(run)) => {
                !step.replaced
                    && run.kind == kind
                    && run.revision == state.doc.history.current_revision()
                    && now.saturating_sub(run.at_ms) < RUN_GAP_MS
                    && !run_is_full(run, &txn)
            }
            _ => false,
        };
    // A revision being amended: the marks as they were when it began.
    state.fit_mark_log();
    let rev = state.doc.history.current_revision();
    let start_marks = if continues && rev > 0 && (tracked || !state.doc.mark_log[rev].is_empty()) {
        let inversion = state.doc.history.current_inversion().clone();
        let mut start_text = old_text.clone();
        inversion.apply(&mut start_text);
        let mut m = marks_before.clone();
        m.map(old_text.slice(..), start_text.slice(..), inversion.changes());
        m.apply(&state.doc.mark_log[rev].undo);
        Some((m, start_text))
    } else {
        None
    };
    let before = HistoryState {
        doc: old_text.clone(),
        selection: before_selection,
    };
    // A change of marks alone folds into the current revision without touching its text
    // (Helix can't compose an empty change set).
    let amended = continues
        && if text_changed { state.doc.history.amend_current_revision(&txn, &old_text, now) } else { rev > 0 };
    if !amended {
        state.doc.history.commit_revision_at_timestamp(&txn, &before, now);
    }
    let rev = state.doc.history.current_revision();
    state.fit_mark_log();
    state.doc.mark_log[rev] = match (amended, start_marks) {
        (true, Some((start, start_text))) => {
            let forward = state.doc.history.current_transaction().clone();
            let backward = state.doc.history.current_inversion().clone();
            mark_delta(&start, &start_text, &forward, &backward, state)
        }
        (true, None) => MarkDelta::default(),
        (false, _) if tracked => {
            let backward = state.doc.history.current_inversion().clone();
            let undo = {
                let mut m = state.doc.marks.clone();
                m.map(state.doc.text.slice(..), old_text.slice(..), backward.changes());
                m.diff(&marks_before)
            };
            MarkDelta { undo, redo: naive.diff(&state.doc.marks) }
        }
        (false, _) => MarkDelta::default(),
    };

    let so_far = match state.doc.run {
        Some(run) if amended => run.chars,
        _ => 0,
    };
    state.doc.run = step.kind.map(|kind| EditRun {
        kind,
        revision: state.doc.history.current_revision(),
        at_ms: now,
        chars: so_far + changed,
    });
    removed
}

/// Changes only marks, as one undo step (a host's own block edit: binding, splitting or
/// joining blocks without touching the text). `f` gets the marks and the text.
pub fn mark_only_edit(state: &mut State, f: impl FnOnce(&mut Marks, RopeSlice)) {
    state.doc.run = None;
    let txn = Transaction::new(&state.doc.text);
    commit_with(state, txn, Step::default(), f);
    state.doc.dirty = state.compute_dirty();
}

/// The delta of a revision that turned `start` (on `start_text`) into the state's marks
/// through `forward`; `backward` is its inversion.
fn mark_delta(start: &Marks, start_text: &crate::helix::Rope, forward: &Transaction, backward: &Transaction, state: &State) -> MarkDelta {
    let mut fwd = start.clone();
    fwd.map(start_text.slice(..), state.doc.text.slice(..), forward.changes());
    let mut bwd = state.doc.marks.clone();
    bwd.map(state.doc.text.slice(..), start_text.slice(..), backward.changes());
    MarkDelta {
        undo: bwd.diff(start),
        redo: fwd.diff(&state.doc.marks),
    }
}

/// Whatever is derived from the marks or the text must be recomputed.
pub(crate) fn after_marks_changed(state: &mut State) {
    state.doc.derived.clear();
}

/// The marks a cut of `[from, to]` took, as offsets into the cut text.
pub(crate) fn carried(removed: &[Mark], from: usize, to: usize) -> Vec<ClipMark> {
    removed
        .iter()
        .filter(|m| from <= m.pos && m.pos <= to)
        .map(|m| ClipMark { offset: m.pos - from, id: m.id, attrs: m.attrs })
        .collect()
}

/// Pastes `text` at every range. With one range, marks carried in the register go back
/// where they were in the pasted text (when their ids aren't in use), and a mark at the
/// paste point stays there unless the register brings its own.
pub(crate) fn paste_text(state: &mut State, text: &str, carried: &[ClipMark]) {
    let single = state.view.selection.len() == 1;
    if carried.is_empty() || !single {
        insert(state, text, None);
        return;
    }
    let replaced = has_selection(state);
    let from = state.view.selection.primary().from();
    // Whole lines pasted at a line start push the line there (and its mark) down; any other
    // paste continues the line it lands in, which keeps its mark.
    let whole = text.ends_with('\n') && crate::marks::is_line_start(state.doc.text.slice(..), from);
    let at_from = if whole { None } else { state.doc.marks.at(from) };
    let tendril = Tendril::from(text);
    let txn = change_each(state, |_, r| (r.from(), r.to(), Some(tendril.clone())));
    commit_with(state, txn, Step { kind: None, replaced, merge: false }, |marks, new| {
        let leading = carried.iter().any(|c| c.offset == 0);
        if let (Some(id), false) = (at_from, leading) {
            if let Some(m) = marks.remove(id) {
                let _ = marks.insert(Mark { pos: from, ..m });
            }
        }
        for c in carried {
            let pos = from + c.offset;
            if pos <= new.len_chars() && crate::marks::is_line_start(new, pos) {
                let _ = marks.insert(Mark { pos, id: c.id, attrs: c.attrs });
            }
        }
    });
}

/// Characters a transaction inserts or deletes.
fn changed_chars(txn: &Transaction) -> usize {
    txn.changes()
        .changes()
        .iter()
        .map(|op| match op {
            Operation::Retain(_) => 0,
            Operation::Delete(n) => *n,
            Operation::Insert(s) => s.chars().count(),
        })
        .sum()
}

/// Whether `run` should end before `txn`: it reached [`RUN_MAX_CHARS`], or it is a typing
/// run past [`RUN_WORD_BREAK_CHARS`] and `txn` starts a new word.
fn run_is_full(run: EditRun, txn: &Transaction) -> bool {
    if run.chars >= RUN_MAX_CHARS {
        return true;
    }
    run.kind == RunKind::Typing
        && run.chars >= RUN_WORD_BREAK_CHARS
        && txn.changes().changes().iter().any(|op| {
            matches!(op, Operation::Insert(s) if s.chars().next().is_some_and(char::is_whitespace))
        })
}

/// Builds a transaction from one change per range (in selection order), dropping overlaps,
/// and puts a caret after each change's inserted text.
pub(crate) fn change_each(
    state: &State,
    mut f: impl FnMut(RopeSlice, Range) -> (usize, usize, Option<Tendril>),
) -> Transaction {
    let text = state.doc.text.slice(..);
    let len = text.len_chars();
    let mut changes: Vec<(usize, usize, Option<Tendril>)> = Vec::new();
    let mut last = 0usize;
    for range in state.view.selection.iter() {
        let (from, to, ins) = f(text, *range);
        let from = from.min(len).max(last);
        let to = to.min(len).max(from);
        last = to;
        changes.push((from, to, ins));
    }
    let mut carets: SmallVec<[Range; 1]> = SmallVec::new();
    let mut delta: isize = 0;
    for (from, to, ins) in &changes {
        let ins_len = ins.as_ref().map_or(0, |t| t.chars().count());
        let pos = (*from as isize + delta) as usize + ins_len;
        carets.push(Range::point(pos));
        delta += ins_len as isize - (*to as isize - *from as isize);
    }
    let primary = state.view.selection.primary_index().min(carets.len() - 1);
    let selection = Selection::new(carets, primary);
    Transaction::change(&state.doc.text, changes.into_iter()).with_selection(selection)
}

pub(crate) fn has_selection(state: &State) -> bool {
    state.view.selection.iter().any(|r| !r.is_empty())
}

/// Types or pastes `text` at every range, replacing selections.
pub(crate) fn insert(state: &mut State, text: &str, kind: Option<RunKind>) {
    let replaced = has_selection(state);
    let tendril = Tendril::from(text);
    let txn = change_each(state, |_, r| (r.from(), r.to(), Some(tendril.clone())));
    commit(state, txn, kind, replaced);
}

/// Deletes each selection; for an empty range, deletes the span `f` gives.
pub(crate) fn delete(state: &mut State, kind: Option<RunKind>, f: impl FnMut(RopeSlice, usize, usize) -> (usize, usize)) {
    delete_marks(state, kind, f);
}

/// [`delete`], returning the marks it removed.
pub(crate) fn delete_marks(
    state: &mut State,
    kind: Option<RunKind>,
    mut f: impl FnMut(RopeSlice, usize, usize) -> (usize, usize),
) -> Vec<Mark> {
    let replaced = has_selection(state);
    let kind = if replaced { None } else { kind };
    let txn = change_each(state, |text, r| {
        if replaced {
            if r.is_empty() {
                (r.head, r.head, None)
            } else {
                (r.from(), r.to(), None)
            }
        } else {
            let (from, to) = f(text, r.anchor, r.head);
            (from, to, None)
        }
    });
    commit(state, txn, kind, replaced)
}

/// Applies an undo or redo transaction from the history: `rev` is the revision being
/// undone, or the one redone.
fn apply_history(state: &mut State, txn: &Transaction, rev: usize, undo: bool) {
    let old = state.doc.text.clone();
    txn.apply(&mut state.doc.text);
    state.view.wrap.edited(txn.changes());
    state.doc.journal.0.push(txn.changes().clone());
    state.doc.edits.0 = state.doc.edits.0.wrapping_add(1);
    state.fit_mark_log();
    state.doc.derived.clear();
    let delta = &state.doc.mark_log[rev];
    if !state.doc.marks.is_empty() || !delta.is_empty() {
        let fixup = if undo { delta.undo.clone() } else { delta.redo.clone() };
        state.doc.marks.map(old.slice(..), state.doc.text.slice(..), txn.changes());
        state.doc.marks.apply(&fixup);
        after_marks_changed(state);
    }
    state.view.selection = match txn.selection() {
        Some(sel) => sel.clone(),
        None => state.view.selection.clone().map(txn.changes()),
    };
    state.doc.run = None;
}

pub(crate) fn selected_text(state: &State) -> Option<String> {
    let text = state.doc.text.slice(..);
    let parts: Vec<String> = state
        .view
        .selection
        .iter()
        .filter(|r| !r.is_empty())
        .map(|r| r.slice(text).to_string())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(state.doc.config.line_ending.as_str()))
    }
}

pub(crate) fn count_label(text: &str) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    let n = text.graphemes(true).count();
    if n == 1 {
        "1 char".into()
    } else {
        format!("{n} chars")
    }
}

/// Converts CRLF, CR and LF line breaks to `le`.
pub(crate) fn normalize_line_endings(text: &str, le: &str) -> String {
    if !text.contains('\r') && le == "\n" {
        return text.to_string();
    }
    let unified = text.replace("\r\n", "\n").replace('\r', "\n");
    if le == "\n" {
        unified
    } else {
        unified.replace('\n', le)
    }
}

// ---------------------------------------------------------------------------------------
// Motion

/// The start of the caret's visual row.
pub(crate) fn row_start(layout: &Layout, pos: usize) -> usize {
    let (at, _) = layout.pos_coords(pos);
    layout.pos_at(at, 0)
}

/// The end of the caret's visual row: the line end on a row that ends the line, else the
/// row's last grapheme (where the row wraps).
pub(crate) fn row_end(layout: &Layout, pos: usize) -> usize {
    let (at, _) = layout.pos_coords(pos);
    layout.pos_at(at, usize::MAX)
}

fn grapheme_is_word(text: RopeSlice, from: usize, to: usize) -> bool {
    from < to && char_is_word(text.char(from))
}

/// The end of the word at or after `pos` (skipping spaces and punctuation first).
pub(crate) fn word_right(text: RopeSlice, pos: usize) -> usize {
    let len = text.len_chars();
    let mut q = pos;
    while q < len {
        let n = next_grapheme_boundary(text, q);
        if grapheme_is_word(text, q, n) {
            break;
        }
        q = n;
    }
    while q < len {
        let n = next_grapheme_boundary(text, q);
        if !grapheme_is_word(text, q, n) {
            break;
        }
        q = n;
    }
    q
}

/// The start of the word at or before `pos` (skipping spaces and punctuation first).
pub(crate) fn word_left(text: RopeSlice, pos: usize) -> usize {
    let mut q = pos;
    while q > 0 {
        let p = prev_grapheme_boundary(text, q);
        if grapheme_is_word(text, p, q) {
            break;
        }
        q = p;
    }
    while q > 0 {
        let p = prev_grapheme_boundary(text, q);
        if !grapheme_is_word(text, p, q) {
            break;
        }
        q = p;
    }
    q
}

/// Moves `n` visual rows from `origin`, aiming for column `goal`. Past the first row it
/// lands on the document start, past the last on the document end (as a text field does).
fn vertical(layout: &Layout, origin: usize, n: isize, goal: usize) -> usize {
    let (at, _) = layout.pos_coords(origin);
    let (mut target, moved) = layout.step_rows(at, n);
    // Virtual rows (a block's blank row) are never caret stops: step over them.
    if moved == n && layout.is_virtual(target) {
        if n > 0 {
            target.row = layout.gap(target.line);
        } else {
            let (t, m) = layout.step_rows(target, -1);
            if m == 0 {
                return 0;
            }
            target = t;
        }
    }
    if moved != n {
        // Ran out of rows: clamp to the document's edge.
        if moved == 0 || n < 0 {
            return if n < 0 { 0 } else { layout.text().len_chars() };
        }
        return layout.text().len_chars();
    }
    layout.pos_at(target, goal)
}

fn motion(state: &mut State, dir: Dir, by: By, extend: bool) {
    // Outside an outline, moving by block is moving by a document line.
    let by = if by == By::Block && state.doc.outline.is_none() { By::Line } else { by };
    // The direction a selection collapses towards.
    let dir = match by {
        By::LineStart | By::DocStart => Dir::Backward,
        By::LineEnd | By::DocEnd => Dir::Forward,
        _ => dir,
    };
    let page = state.text_rows().max(1) as isize;
    let wrapped = Layout::new(state);
    let unwrapped = Layout::unwrapped(state);
    let text = wrapped.text();
    let len = text.len_chars();

    let focused: Vec<bool> = state
        .view
        .selection
        .iter()
        .map(|r| crate::outline::rules::is_focused_atomic(state, r))
        .collect();
    let mut index = 0;
    let st: &State = state;
    let selection = st.view.selection.clone().transform(|r| {
        let is_focused = focused.get(index).copied().unwrap_or(false);
        index += 1;
        let collapsing = !extend && !r.is_empty();
        let origin = if collapsing {
            match dir {
                Dir::Backward => r.from(),
                Dir::Forward => r.to(),
            }
        } else {
            r.head
        };
        let finish = |head: usize, goal: Option<u32>| {
            let mut range = if extend {
                Range::new(r.anchor, head)
            } else {
                Range::point(head)
            };
            range.old_visual_position = goal.map(|g| (0, g));
            range
        };
        match by {
            By::Grapheme => {
                if collapsing {
                    return finish(origin, None);
                }
                let head = match dir {
                    Dir::Backward => prev_grapheme_boundary(text, origin),
                    Dir::Forward => next_grapheme_boundary(text, origin),
                };
                finish(head, None)
            }
            By::Word => {
                let head = match dir {
                    Dir::Backward => word_left(text, origin),
                    Dir::Forward => word_right(text, origin),
                };
                finish(head, None)
            }
            By::Line | By::VisualLine | By::Page => {
                let layout = if by == By::Line { &unwrapped } else { &wrapped };
                let rows = if by == By::Page { page } else { 1 };
                let n = match dir {
                    Dir::Backward => -rows,
                    Dir::Forward => rows,
                };
                // A collapsing motion starts from the selection's edge, at that edge's x
                // (a focused atomic block keeps the goal it was reached with).
                let goal = match r.old_visual_position {
                    Some((_, col)) if !collapsing || is_focused => col as usize,
                    _ => layout.pos_coords(origin).1,
                };
                let head = vertical(layout, origin, n, goal);
                finish(head, Some(goal as u32))
            }
            By::LineStart => finish(row_start(&wrapped, origin), None),
            By::LineEnd => finish(row_end(&wrapped, origin), None),
            By::DocStart => finish(0, None),
            By::DocEnd => finish(len, None),
            By::Block => finish(crate::outline::rules::block_step(st, origin, dir), None),
        }
    });
    state.view.selection = selection;

    if by == By::Page {
        // Scroll by the same amount so the caret keeps its place on screen.
        let n = match dir {
            Dir::Backward => -page,
            Dir::Forward => page,
        };
        let top = wrapped.top(&state.view.scroll);
        let (top, _) = wrapped.step_rows(top, n);
        state.view.scroll = Scroll {
            line: top.line,
            row: top.row,
            col: state.view.scroll.col,
        };
    }
}

/// Scrolls the view; the caret moves only if it would leave the view, keeping its column.
fn scroll(state: &mut State, rows: i32) {
    let h = state.text_rows();
    if h == 0 {
        return;
    }
    let layout = Layout::new(state);
    let new_top = scrolled_top(&layout, state, rows);
    state.view.scroll = Scroll {
        line: new_top.line,
        row: new_top.row,
        col: state.view.scroll.col,
    };
    let so = (state.view.config.scrolloff as usize).min((h - 1) / 2);
    let primary = state.view.selection.primary();
    let (caret, col) = layout.pos_coords(primary.head);
    let dist = layout.rows_between(new_top, caret, h + so);
    let target_row = if dist < so as isize {
        Some(so.min(h - 1))
    } else if dist > (h - 1 - so) as isize {
        Some(h - 1 - so)
    } else {
        None
    };
    if let Some(row) = target_row {
        let goal = primary.old_visual_position.map_or(col, |(_, c)| c as usize);
        let (at, moved) = layout.step_rows(new_top, row as isize);
        let pos = if moved < row as isize {
            layout.text().len_chars()
        } else {
            layout.pos_at(at, goal)
        };
        let mut range = Range::point(pos);
        range.old_visual_position = Some((0, goal as u32));
        state.view.selection = Selection::single(pos, pos).transform(|_| range);
    }
}

/// The top `rows` rows from the view's top, never past the point where the document's last
/// row sits at the bottom.
fn scrolled_top(layout: &Layout, state: &State, rows: i32) -> crate::layout::RowPos {
    let h = state.text_rows().max(1);
    let top = layout.top(&state.view.scroll);
    let (mut new_top, _) = layout.step_rows(top, rows as isize);
    let end = layout.pos_coords(layout.text().len_chars()).0;
    let max_top = layout.step_rows(end, -(h as isize - 1)).0;
    if rows > 0 && new_top > max_top {
        new_top = max_top.max(top);
    }
    new_top
}

/// Scrolls the view without moving the caret (the wheel in a host that lets the caret leave
/// the view).
fn scroll_view(state: &mut State, rows: i32) {
    if state.text_rows() == 0 {
        return;
    }
    let layout = Layout::new(state);
    let top = scrolled_top(&layout, state, rows);
    state.view.scroll = Scroll { line: top.line, row: top.row, col: state.view.scroll.col };
}

/// The text a copy would put on the clipboard, if anything is selected.
pub fn selection_text(state: &State) -> Option<String> {
    selected_text(state)
}
