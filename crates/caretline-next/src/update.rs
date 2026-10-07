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
use crate::state::{EditRun, RunKind, Scroll, State, RUN_GAP_MS, RUN_MAX_CHARS, RUN_WORD_BREAK_CHARS};

/// Applies one message. Returns the effects for the runtime to perform.
pub fn update(state: &mut State, msg: Msg) -> Vec<Effect> {
    let mut effects = Vec::new();
    let passive = matches!(
        msg,
        Msg::Tick { .. }
            | Msg::Resize { .. }
            | Msg::Saved
            | Msg::SaveFailed { .. }
            | Msg::ShowStatus { .. }
    );
    if !passive {
        state.status = None;
        if !matches!(msg, Msg::Quit) {
            state.quit_armed = false;
        }
    }
    // Anything but an edit of the same kind (or a passive message) ends an edit run.
    let keeps_run = passive
        || matches!(
            msg,
            Msg::InsertText { .. } | Msg::InsertNewline | Msg::DeleteBackward | Msg::DeleteForward
        );
    if !keeps_run {
        state.run = None;
    }

    match msg {
        Msg::InsertText { text } => {
            let text = normalize_line_endings(&text, state.config.line_ending.as_str());
            if !text.is_empty() {
                insert(state, &text, Some(RunKind::Typing));
            }
        }
        Msg::InsertNewline => {
            let le = state.config.line_ending.as_str().to_string();
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
            let pos = layout.pos_at_screen(&state.scroll, col, row);
            let primary = state.selection.primary();
            let range = if extend {
                Range::new(primary.anchor, pos)
            } else {
                Range::point(pos)
            };
            state.selection = Selection::single(range.anchor, range.head);
        }
        Msg::Scroll { rows } => scroll(state, rows),
        Msg::SelectAll => {
            state.selection = Selection::single(0, state.text.len_chars());
        }
        Msg::Collapse => {
            state.selection = state
                .selection
                .clone()
                .transform(|r| Range::point(r.head));
        }
        Msg::Copy => {
            if let Some(text) = selected_text(state) {
                state.status = Some(format!("copied {}", count_label(&text)));
                state.clipboard = text.clone();
                effects.push(Effect::ClipboardSet { text });
            } else {
                state.status = Some("nothing selected".into());
            }
        }
        Msg::Cut => {
            if let Some(text) = selected_text(state) {
                state.status = Some(format!("cut {}", count_label(&text)));
                state.clipboard = text.clone();
                effects.push(Effect::ClipboardSet { text });
                // Only the non-empty ranges are cut; carets elsewhere stay as they are.
                delete(state, None, |_, _, head| (head, head));
            } else {
                state.status = Some("nothing selected".into());
            }
        }
        Msg::Paste { text } => {
            // Outside text takes the document's line ending; the register came from this
            // document, so it goes back exactly as it was copied.
            let text = match text {
                Some(text) => normalize_line_endings(&text, state.config.line_ending.as_str()),
                None => state.clipboard.clone(),
            };
            if text.is_empty() {
                state.status = Some("the clipboard is empty".into());
            } else {
                insert(state, &text, None);
            }
        }
        Msg::Undo => {
            let txn = state.history.undo().cloned();
            match txn {
                Some(txn) => apply_history(state, &txn),
                None => state.status = Some("nothing to undo".into()),
            }
        }
        Msg::Redo => {
            let txn = state.history.redo().cloned();
            match txn {
                Some(txn) => apply_history(state, &txn),
                None => state.status = Some("nothing to redo".into()),
            }
        }
        Msg::Save => match &state.path {
            Some(path) => {
                state.saving = Some(state.history.current_revision());
                effects.push(Effect::WriteFile {
                    path: path.clone(),
                    text: state.text.to_string(),
                });
            }
            None => state.status = Some("no file name: start caretline with a path".into()),
        },
        Msg::Saved => {
            if let Some(rev) = state.saving.take() {
                state.saved_revision = Some(rev);
                state.status = Some(format!("saved {}", state.name()));
            }
        }
        Msg::SaveFailed { err } => {
            state.saving = None;
            state.status = Some(format!("save failed: {err}"));
        }
        Msg::Quit => {
            if state.dirty && !state.quit_armed {
                state.quit_armed = true;
                state.status = Some("unsaved changes: quit again to discard them".into());
            } else {
                effects.push(Effect::Quit);
            }
        }
        Msg::Resize { width, height } => {
            state.viewport.width = width.max(1);
            state.viewport.height = height.max(1);
        }
        Msg::Tick { now_ms } => {
            state.now_ms = now_ms;
        }
        Msg::ShowStatus { text } => {
            // One line: a newline would break the status bar.
            let line = text.lines().next().unwrap_or("").to_string();
            state.status = (!line.is_empty()).then_some(line);
        }
    }

    state.dirty = state.compute_dirty();
    if state.run.is_some_and(|r| r.revision != state.history.current_revision()) {
        state.run = None;
    }
    ensure_caret_visible(state);
    effects
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
/// amending the open run when `kind` continues it.
fn commit(state: &mut State, txn: Transaction, kind: Option<RunKind>, replaced_selection: bool) {
    if txn.changes().is_empty() {
        if let Some(sel) = txn.selection() {
            state.selection = sel.clone();
        }
        return;
    }
    let before = HistoryState {
        doc: state.text.clone(),
        selection: state.selection.clone(),
    };
    txn.apply(&mut state.text);
    state.wrap.edited(txn.changes());
    if let Some(sel) = txn.selection() {
        state.selection = sel.clone();
    }
    let now = state.now_ms;
    let changed = changed_chars(&txn);
    let continues = match (kind, state.run) {
        (Some(kind), Some(run)) => {
            !replaced_selection
                && run.kind == kind
                && run.revision == state.history.current_revision()
                && now.saturating_sub(run.at_ms) < RUN_GAP_MS
                && !run_is_full(run, &txn)
        }
        _ => false,
    };
    let amended = continues && state.history.amend_current_revision(&txn, &before.doc, now);
    if !amended {
        state.history.commit_revision_at_timestamp(&txn, &before, now);
    }
    let so_far = match state.run {
        Some(run) if amended => run.chars,
        _ => 0,
    };
    state.run = kind.map(|kind| EditRun {
        kind,
        revision: state.history.current_revision(),
        at_ms: now,
        chars: so_far + changed,
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
fn change_each(
    state: &State,
    mut f: impl FnMut(RopeSlice, Range) -> (usize, usize, Option<Tendril>),
) -> Transaction {
    let text = state.text.slice(..);
    let len = text.len_chars();
    let mut changes: Vec<(usize, usize, Option<Tendril>)> = Vec::new();
    let mut last = 0usize;
    for range in state.selection.iter() {
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
    let primary = state.selection.primary_index().min(carets.len() - 1);
    let selection = Selection::new(carets, primary);
    Transaction::change(&state.text, changes.into_iter()).with_selection(selection)
}

fn has_selection(state: &State) -> bool {
    state.selection.iter().any(|r| !r.is_empty())
}

/// Types or pastes `text` at every range, replacing selections.
fn insert(state: &mut State, text: &str, kind: Option<RunKind>) {
    let replaced = has_selection(state);
    let tendril = Tendril::from(text);
    let txn = change_each(state, |_, r| (r.from(), r.to(), Some(tendril.clone())));
    commit(state, txn, kind, replaced);
}

/// Deletes each selection; for an empty range, deletes the span `f` gives.
fn delete(
    state: &mut State,
    kind: Option<RunKind>,
    mut f: impl FnMut(RopeSlice, usize, usize) -> (usize, usize),
) {
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
    commit(state, txn, kind, replaced);
}

/// Applies an undo or redo transaction from the history.
fn apply_history(state: &mut State, txn: &Transaction) {
    txn.apply(&mut state.text);
    state.wrap.edited(txn.changes());
    state.selection = match txn.selection() {
        Some(sel) => sel.clone(),
        None => state.selection.clone().map(txn.changes()),
    };
    state.run = None;
}

fn selected_text(state: &State) -> Option<String> {
    let text = state.text.slice(..);
    let parts: Vec<String> = state
        .selection
        .iter()
        .filter(|r| !r.is_empty())
        .map(|r| r.slice(text).to_string())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(state.config.line_ending.as_str()))
    }
}

fn count_label(text: &str) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    let n = text.graphemes(true).count();
    if n == 1 {
        "1 char".into()
    } else {
        format!("{n} chars")
    }
}

/// Converts CRLF, CR and LF line breaks to `le`.
fn normalize_line_endings(text: &str, le: &str) -> String {
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
fn row_start(layout: &Layout, pos: usize) -> usize {
    let (at, _) = layout.pos_coords(pos);
    layout.pos_at(at, 0)
}

/// The end of the caret's visual row: the line end on a row that ends the line, else the
/// row's last grapheme (where the row wraps).
fn row_end(layout: &Layout, pos: usize) -> usize {
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
    let (target, moved) = layout.step_rows(at, n);
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

    let selection = state.selection.clone().transform(|r| {
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
                // A collapsing motion starts from the selection's edge, at that edge's x.
                let goal = match r.old_visual_position {
                    Some((_, col)) if !collapsing => col as usize,
                    _ => layout.pos_coords(origin).1,
                };
                let head = vertical(layout, origin, n, goal);
                finish(head, Some(goal as u32))
            }
            By::LineStart => finish(row_start(&wrapped, origin), None),
            By::LineEnd => finish(row_end(&wrapped, origin), None),
            By::DocStart => finish(0, None),
            By::DocEnd => finish(len, None),
        }
    });
    state.selection = selection;

    if by == By::Page {
        // Scroll by the same amount so the caret keeps its place on screen.
        let n = match dir {
            Dir::Backward => -page,
            Dir::Forward => page,
        };
        let top = wrapped.top(&state.scroll);
        let (top, _) = wrapped.step_rows(top, n);
        state.scroll = Scroll {
            line: top.line,
            row: top.row,
            col: state.scroll.col,
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
    let top = layout.top(&state.scroll);
    let (mut new_top, _) = layout.step_rows(top, rows as isize);
    // Never scroll past the point where the last row sits at the bottom.
    let end = layout.pos_coords(layout.text().len_chars()).0;
    let max_top = layout.step_rows(end, -(h as isize - 1)).0;
    if rows > 0 && new_top > max_top {
        new_top = max_top.max(top);
    }
    state.scroll = Scroll {
        line: new_top.line,
        row: new_top.row,
        col: state.scroll.col,
    };
    let so = (state.config.scrolloff as usize).min((h - 1) / 2);
    let primary = state.selection.primary();
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
        state.selection = Selection::single(pos, pos).transform(|_| range);
    }
}

/// The text a copy would put on the clipboard, if anything is selected.
pub fn selection_text(state: &State) -> Option<String> {
    selected_text(state)
}
