//! The outline's editing rules: what Enter, Backspace, Delete, Tab, the task cycle, moving
//! blocks, copy and paste do in an outline document, and the caret invariant.
//!
//! Each rule is one Transaction and one undo step, with explicit mark edits where the plain
//! mapping isn't the rule (a paragraph split off, two paragraphs joined, blocks moved). After
//! every edit the update loop gives unmarked block starts a mark and pins every surviving
//! block's blank row (`gap`) to what it was, so no edit moves another block.

use crate::helix::graphemes::{next_grapheme_boundary, prev_grapheme_boundary};
use crate::helix::line_ending::line_end_char_index;
use crate::helix::{Assoc, Range, RopeSlice, Selection, Tendril, Transaction};
use crate::layout::Layout;
use crate::marks::{BlockAttrs, ClipMark, Clipboard, Mark, MarkId, Marks};
use crate::msg::{By, Dir, Effect, Msg};
use crate::outline::markdown;
use crate::outline::{BlockInfo, Hang, Kind, NewBlock, Outline};
use crate::state::{Document, State};
use crate::update::{self, Step};

/// Applies `msg` with the outline's rules, when one applies. `None` leaves it to the plain
/// editor.
pub(crate) fn update(state: &mut State, msg: &Msg) -> Option<Vec<Effect>> {
    match msg {
        Msg::InsertNewline => newline(state, false),
        Msg::SoftBreak => newline(state, true),
        Msg::InsertText { text } => type_text(state, text),
        Msg::DeleteBackward => backward(state, Back::Char),
        Msg::DeleteWordBackward => backward(state, Back::Word),
        Msg::DeleteToLineStart => backward(state, Back::Row),
        Msg::DeleteForward => forward(state, Fwd::Char),
        Msg::DeleteWordForward => forward(state, Fwd::Word),
        Msg::DeleteToLineEnd => forward(state, Fwd::Row),
        Msg::KillLine => forward(state, Fwd::Line),
        Msg::Indent => Some(nest(state, 1)),
        Msg::Outdent => Some(nest(state, -1)),
        Msg::TaskCycle => Some(task_cycle(state)),
        Msg::SetStatus { id, ch } => Some(set_status(state, *id, *ch)),
        Msg::MoveBlock { dir } => Some(move_block(state, *dir)),
        Msg::SelectBlock { id } => Some(select_block(state, *id)),
        Msg::InsertBlocks { after, blocks } => Some(insert_blocks(state, *after, blocks)),
        Msg::Paste { text } => paste(state, text.as_deref(), false),
        Msg::PastePlain { text } => paste(state, text.as_deref(), true),
        Msg::Copy => copy(state, false),
        Msg::Cut => copy(state, true),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------
// Helpers

fn le(state: &State) -> String {
    state.doc.config.line_ending.as_str().to_string()
}

fn single(state: &State) -> Option<Range> {
    (state.view.selection.len() == 1).then(|| state.view.selection.primary())
}

fn line_end(text: RopeSlice, line: usize) -> usize {
    line_end_char_index(&text, line)
}

fn notice(state: &mut State, text: &str) -> Vec<Effect> {
    state.view.status = Some(text.to_string());
    Vec::new()
}

/// One edit: `changes` (sorted, in current positions), the selection after it, and a mark
/// fix-up that sees the new text.
fn edit(
    state: &mut State,
    changes: Vec<(usize, usize, Option<String>)>,
    selection: Selection,
    merge: bool,
    fix: impl FnOnce(&mut Marks, RopeSlice),
) {
    let txn = Transaction::change(&state.doc.text, changes.into_iter().map(|(a, b, t)| (a, b, t.map(|t| Tendril::from(t.as_str())))))
        .with_selection(selection);
    update::commit_with(state, txn, Step { kind: None, replaced: false, merge }, fix);
}

/// Maps a selection through changes (positions at an insertion move past it).
fn mapped(state: &State, changes: &[(usize, usize, Option<String>)], assoc: Assoc) -> Selection {
    let txn = Transaction::change(&state.doc.text, changes.iter().map(|(a, b, t)| (*a, *b, t.as_deref().map(Tendril::from))));
    let cs = txn.changes();
    state.view.selection.clone().transform(|r| Range {
        anchor: cs.map_pos(r.anchor, assoc),
        head: cs.map_pos(r.head, assoc),
        old_visual_position: None,
    })
}

fn caret_at(pos: usize) -> Selection {
    Selection::point(pos)
}

/// The block an exactly-selected atomic block, if the range is one.
fn focused_atomic(o: &Outline, text: RopeSlice, r: &Range) -> Option<usize> {
    if r.is_empty() {
        return None;
    }
    let i = o.index_at(text, r.from());
    let b = &o.blocks[i];
    (b.atomic && r.from() == b.content_start() && r.to() == b.end).then_some(i)
}

fn focus(state: &mut State, b: &BlockInfo) -> Vec<Effect> {
    state.view.selection = Selection::single(b.content_start(), b.end);
    Vec::new()
}

/// The line start after `line`, if there is a next line.
fn next_line_start(text: RopeSlice, line: usize) -> Option<usize> {
    (line + 1 < text.len_lines()).then(|| text.line_to_char(line + 1))
}

/// The marker text of an item (`- `, `* `, `12. `, `- [x] `).
fn marker_of(text: RopeSlice, b: &BlockInfo) -> String {
    text.slice(b.start + b.indent..b.content_start()).to_string()
}

/// The marker for the item after `b` (Enter): the same bullet, the next number, a task open.
fn next_marker(state: &State, text: RopeSlice, b: &BlockInfo) -> String {
    let cfg = state.doc.outline.as_ref().expect("outline");
    let m = marker_of(text, b);
    match (b.kind, b.hang) {
        (_, Hang::Number(n)) => {
            let delim = m.chars().find(|c| *c == '.' || *c == ')').unwrap_or('.');
            format!("{}{delim} ", n + 1)
        }
        (Kind::Task, _) => format!("{} [{}] ", m.chars().next().unwrap_or('-'), cfg.cycle[0]),
        _ => m,
    }
}

/// The marker for a new empty item like `b` (an open task, the same bullet or number).
fn empty_marker(state: &State, text: RopeSlice, b: &BlockInfo) -> String {
    let cfg = state.doc.outline.as_ref().expect("outline");
    let m = marker_of(text, b);
    match b.kind {
        Kind::Task => format!("{} [{}] ", m.chars().next().unwrap_or('-'), cfg.cycle[0]),
        _ => m,
    }
}

// ---------------------------------------------------------------------------------------
// Enter and soft breaks

fn newline(state: &mut State, soft: bool) -> Option<Vec<Effect>> {
    let r = single(state)?;
    let o = state.blocks()?;
    let text = state.doc.text.slice(..);
    if let Some(i) = focused_atomic(&o, text, &r) {
        // A new empty paragraph after the block.
        let b = o.blocks[i].clone();
        let le = le(state);
        let at = b.end + le.chars().count();
        edit(state, vec![(b.end, b.end, Some(le))], caret_at(at), false, |m, _| {
            m.mint(at);
        });
        return Some(Vec::new());
    }
    let mut merge = false;
    if !r.is_empty() {
        update::delete(state, None, |_, _, head| (head, head));
        merge = true;
    }
    newline_at(state, soft, merge);
    Some(Vec::new())
}

fn newline_at(state: &mut State, soft: bool, merge: bool) {
    let o = state.blocks().expect("outline");
    let rope = state.doc.text.clone();
    let text = rope.slice(..);
    let le = le(state);
    let n = le.chars().count();
    let p = state.caret();
    let b = o.block_at(text, p).clone();
    let line = text.char_to_line(p);
    let ls = text.line_to_char(line);
    let lend = line_end(text, line);
    let first = line == b.first_line;
    let soft_break = |state: &mut State| edit(state, vec![(p, p, Some(le.clone()))], caret_at(p + n), merge, |_, _| {});

    if b.fence || b.atomic {
        return soft_break(state);
    }
    if b.is_item() {
        if soft {
            return soft_break(state);
        }
        if b.is_empty() {
            // An empty item ends the list: it becomes a paragraph.
            return edit(state, vec![(b.start, b.content_start(), None)], caret_at(b.start), merge, |_, _| {});
        }
        let indent = " ".repeat(b.indent);
        if p == b.content_start() {
            // At the content's start: a new empty item above; the item keeps its id.
            let ins = format!("{indent}{}{le}", empty_marker(state, text, &b));
            let len = ins.chars().count();
            return edit(state, vec![(b.start, b.start, Some(ins))], caret_at(p + len), merge, |_, _| {});
        }
        let ins = format!("{le}{indent}{}", next_marker(state, text, &b));
        let len = ins.chars().count();
        return edit(state, vec![(p, p, Some(ins))], caret_at(p + len), merge, |_, _| {});
    }
    if matches!(b.hang, Hang::Heading(_) | Hang::Quote) {
        if soft {
            return soft_break(state);
        }
        if first && p == b.content_start() && !b.is_empty() {
            let start = b.start;
            return edit(state, vec![(start, start, Some(le.clone()))], caret_at(p + n), merge, move |m, _| {
                m.mint(start);
            });
        }
        // A heading or quote is one line: what follows the caret is a new paragraph.
        return edit(state, vec![(p, p, Some(le.clone()))], caret_at(p + n), merge, move |m, _| {
            m.mint(p + n);
        });
    }
    // A paragraph.
    if first && p == b.content_start() {
        // At its very start: a new empty paragraph above; the paragraph keeps its id.
        let start = b.start;
        return edit(state, vec![(start, start, Some(le.clone()))], caret_at(p + n), merge, move |m, _| {
            m.mint(start);
        });
    }
    if !first && p == ls {
        // At the start of a later line: the paragraph ends above, and this line starts a new
        // one (an empty line here is dropped when more follows).
        if ls == lend && line < b.last_line() {
            let next = text.line_to_char(line + 1);
            return edit(state, vec![(ls, next, None)], caret_at(ls), merge, move |m, _| {
                m.mint(ls);
            });
        }
        return edit(state, vec![], caret_at(ls), merge, move |m, _| {
            m.mint(ls);
        });
    }
    if p == lend && line < b.last_line() {
        // At the end of a line with more below: the rest is a new paragraph, and the caret
        // sits on a new empty one between.
        return edit(state, vec![(p, p, Some(le.clone()))], caret_at(p + n), merge, move |m, _| {
            m.mint(p + n);
            m.mint(p + 2 * n);
        });
    }
    soft_break(state)
}

// ---------------------------------------------------------------------------------------
// Typing

fn type_text(state: &mut State, typed: &str) -> Option<Vec<Effect>> {
    let r = single(state)?;
    let o = state.blocks()?;
    let Some(i) = focused_atomic(&o, state.doc.text.slice(..), &r) else {
        return task_shorthand(state, &o, r, typed);
    };
    if typed.is_empty() || typed.contains(['\n', '\r']) {
        return None;
    }
    // Typing on a focused atomic block starts a new paragraph after it.
    let b = o.blocks[i].clone();
    let le = le(state);
    let at = b.end + le.chars().count();
    let caret = at + typed.chars().count();
    edit(state, vec![(b.end, b.end, Some(format!("{le}{typed}")))], caret_at(caret), false, |m, _| {
        m.mint(at);
    });
    Some(Vec::new())
}

/// A bare task box typed at the start of a paragraph's line (`[ ] `, `[x] `, `[] `) is
/// shorthand for a task: it becomes `- [c] `, and that line a task (on a later line, a new
/// block). One step, the typed space included.
fn task_shorthand(state: &mut State, o: &Outline, r: Range, typed: &str) -> Option<Vec<Effect>> {
    if !r.is_empty() || !typed.ends_with(' ') || typed.contains(['\n', '\r']) {
        return None;
    }
    let cfg = state.doc.outline.clone()?;
    let text = state.doc.text.slice(..);
    let p = r.head;
    let b = o.block_at(text, p);
    if !b.is_plain_para() {
        return None;
    }
    let line = text.char_to_line(p);
    let ls = text.line_to_char(line);
    let indent = if line == b.first_line { b.indent } else { 0 };
    let before: String = text.slice(ls + indent.min(p - ls)..p).chars().chain(typed.chars()).collect();
    let inner = before.strip_prefix('[')?.strip_suffix("] ")?;
    let ch = match inner.chars().count() {
        0 => cfg.cycle[0],
        1 => inner.chars().next().filter(|&c| cfg.is_task_char(c))?,
        _ => return None,
    };
    let from = ls + indent.min(p - ls);
    let ins = format!("- [{ch}] ");
    let caret = from + ins.chars().count();
    edit(state, vec![(from, p, Some(ins))], caret_at(caret), false, |_, _| {});
    Some(Vec::new())
}

// ---------------------------------------------------------------------------------------
// Backspace and Delete

#[derive(Clone, Copy, PartialEq, Eq)]
enum Back {
    Char,
    Word,
    Row,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fwd {
    Char,
    Word,
    Row,
    Line,
}

fn backward(state: &mut State, how: Back) -> Option<Vec<Effect>> {
    let r = single(state)?;
    let o = state.blocks()?;
    let text = state.doc.text.slice(..);
    if let Some(i) = focused_atomic(&o, text, &r) {
        return Some(remove_block(state, &o, i, true));
    }
    if !r.is_empty() {
        return None;
    }
    let p = r.head;
    let i = o.index_at(text, p);
    let b = o.blocks[i].clone();
    if p == b.content_start() {
        return Some(backspace_at_start(state, &o, i));
    }
    if how == Back::Char {
        return None;
    }
    // A word or row delete stops at the content's start (never into the marker).
    let floor = if text.char_to_line(p) == b.first_line { b.content_start() } else { 0 };
    let layout = (how == Back::Row).then(|| Layout::new(state));
    update::delete(state, None, |text, _, head| {
        let from = match how {
            Back::Row => {
                let start = update::row_start(layout.as_ref().expect("layout"), head);
                if start == head { prev_grapheme_boundary(text, head) } else { start }
            }
            _ => {
                let prev = prev_grapheme_boundary(text, head);
                if prev < head && crate::helix::chars::char_is_line_ending(text.char(prev)) {
                    prev
                } else {
                    update::word_left(text, head)
                }
            }
        };
        (from.max(floor), head)
    });
    Some(Vec::new())
}

/// Backspace at a block's content start: the marker goes a step at a time (a task becomes a
/// bullet, a bullet or heading a paragraph), and a paragraph joins the block above.
fn backspace_at_start(state: &mut State, o: &Outline, i: usize) -> Vec<Effect> {
    let b = o.blocks[i].clone();
    let text = state.doc.text.slice(..);
    if b.prefix_len > 0 {
        let (from, to) = match b.kind {
            Kind::Task => (b.start + b.indent + 2, b.content_start()),
            _ => (b.start, b.content_start()),
        };
        edit(state, vec![(from, to, None)], caret_at(from), false, |_, _| {});
        return Vec::new();
    }
    if i == 0 {
        return Vec::new();
    }
    let a = o.blocks[i - 1].clone();
    if a.atomic {
        return focus(state, &a);
    }
    if a.is_plain_para() && b.is_plain_para() && !a.is_empty() && !b.is_empty() {
        // Two paragraphs join keeping the line break: only the lower block's mark goes.
        let id = b.id;
        edit(state, vec![], caret_at(b.start), false, move |m, _| {
            m.remove(id);
        });
        return Vec::new();
    }
    let prev_end = line_end(text, b.first_line - 1);
    edit(state, vec![(prev_end, b.start, None)], caret_at(prev_end), false, |_, _| {});
    Vec::new()
}

fn forward(state: &mut State, how: Fwd) -> Option<Vec<Effect>> {
    let r = single(state)?;
    let o = state.blocks()?;
    let text = state.doc.text.slice(..);
    if let Some(i) = focused_atomic(&o, text, &r) {
        return Some(remove_block(state, &o, i, false));
    }
    if !r.is_empty() {
        return None;
    }
    let p = r.head;
    let i = o.index_at(text, p);
    let b = o.blocks[i].clone();
    if p == b.end {
        return Some(delete_at_end(state, &o, i));
    }
    if how == Fwd::Char {
        return None;
    }
    // A word, row or line delete stops at the line's end (never into the next block).
    let ceiling = line_end(text, text.char_to_line(p));
    let layout = (how == Fwd::Row).then(|| Layout::new(state));
    update::delete(state, None, |text, _, head| {
        let to = match how {
            Fwd::Row => {
                let end = update::row_end(layout.as_ref().expect("layout"), head);
                if end == head { next_grapheme_boundary(text, head) } else { end }
            }
            Fwd::Line => {
                let end = line_end_char_index(&text, text.char_to_line(head));
                if end == head { next_grapheme_boundary(text, head) } else { end }
            }
            _ => {
                if head < text.len_chars() && crate::helix::chars::char_is_line_ending(text.char(head)) {
                    next_grapheme_boundary(text, head)
                } else {
                    update::word_right(text, head)
                }
            }
        };
        if head < ceiling { (head, to.min(ceiling)) } else { (head, to) }
    });
    Some(Vec::new())
}

/// Delete at a block's end: the next block joins it (or, if it's atomic, is selected).
fn delete_at_end(state: &mut State, o: &Outline, i: usize) -> Vec<Effect> {
    let b = o.blocks[i].clone();
    let Some(c) = o.blocks.get(i + 1).cloned() else { return Vec::new() };
    if c.atomic {
        return focus(state, &c);
    }
    if b.is_plain_para() && c.is_plain_para() && !b.is_empty() && !c.is_empty() {
        let id = c.id;
        edit(state, vec![], caret_at(b.end), false, move |m, _| {
            m.remove(id);
        });
        return Vec::new();
    }
    edit(state, vec![(b.end, c.content_start(), None)], caret_at(b.end), false, |_, _| {});
    Vec::new()
}

/// Removes a whole (atomic) block, as one undo step.
fn remove_block(state: &mut State, o: &Outline, i: usize, backward: bool) -> Vec<Effect> {
    let b = o.blocks[i].clone();
    let text = state.doc.text.slice(..);
    let last = text.len_lines() - 1;
    let (from, to) = if b.last_line() < last {
        (b.start, text.line_to_char(b.last_line() + 1))
    } else if b.first_line > 0 {
        (line_end(text, b.first_line - 1), b.end)
    } else {
        (b.start, b.end)
    };
    let caret = if backward && b.first_line > 0 { line_end(text, b.first_line - 1) } else { from };
    let caret = if caret > from { caret - (to - from) } else { caret };
    let name: String = text.slice(b.content_start()..b.end).to_string();
    edit(state, vec![(from, to, None)], caret_at(caret), false, |_, _| {});
    let label = name.split("](").nth(1).and_then(|s| s.trim_end_matches(')').rsplit('/').next().map(str::to_string));
    notice(state, &format!("removed {}", label.unwrap_or_else(|| "the block".into())))
}

// ---------------------------------------------------------------------------------------
// Tab and Shift-Tab

fn nest(state: &mut State, delta: i32) -> Vec<Effect> {
    let Some(o) = state.blocks() else { return notice(state, "only in outline documents") };
    let cfg = state.doc.outline.clone().expect("outline");
    let text = state.doc.text.slice(..);
    let r = state.view.selection.primary();
    let range = o.indices_between(text, r.from(), r.to());
    let unit = cfg.indent.max(1) as i32;
    // The depth of the last non-empty block above: an item nests at most one below it.
    let mut above: Option<u16> = o.blocks[..*range.start()].iter().rev().find(|b| !b.is_empty()).map(|b| b.depth);
    let mut changes = Vec::new();
    let mut paragraphs = 0;
    let count = range.clone().count();
    for i in range {
        let b = &o.blocks[i];
        let mut depth = b.depth;
        if b.kind == Kind::Para && (delta > 0 || b.depth == 0) {
            paragraphs += 1;
        } else if !b.fence {
            let max = above.map_or(0, |d| d as i32 + 1);
            let want = if delta > 0 {
                if (b.depth as i32) < max { b.depth as i32 + 1 } else { b.depth as i32 }
            } else {
                (b.depth as i32 - 1).max(0)
            };
            if want != b.depth as i32 {
                let target = (want * unit) as usize;
                if target > b.indent {
                    changes.push((b.start, b.start, Some(" ".repeat(target - b.indent))));
                } else {
                    changes.push((b.start, b.start + (b.indent - target), None));
                }
                depth = want as u16;
            }
        }
        if !b.is_empty() {
            above = Some(depth);
        }
    }
    if changes.is_empty() {
        let why = if paragraphs == count {
            "paragraphs don't nest"
        } else if delta > 0 {
            "nothing to nest under"
        } else {
            "already at the top level"
        };
        return notice(state, why);
    }
    let sel = mapped(state, &changes, Assoc::After);
    edit(state, changes, sel, false, |_, _| {});
    Vec::new()
}

// ---------------------------------------------------------------------------------------
// The task cycle

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cycle {
    Open(char),
    Done(char),
    Text,
}

fn task_cycle(state: &mut State) -> Vec<Effect> {
    let Some(o) = state.blocks() else { return notice(state, "only in outline documents") };
    let cfg = state.doc.outline.clone().expect("outline");
    let rope = state.doc.text.clone();
    let text = rope.slice(..);
    let r = state.view.selection.primary();
    let (fi, li) = (o.index_at(text, r.from()), o.index_at(text, r.to()));
    let b = o.blocks[fi].clone();
    if fi == li && b.kind == Kind::Para && !b.fence && b.line_count > 1 {
        return split_for_task(state, &o, fi);
    }
    let target = match (b.kind, b.status) {
        (Kind::Task, Some(c)) if c == cfg.cycle[1] => Cycle::Text,
        (Kind::Task, _) => Cycle::Done(cfg.cycle[1]),
        _ => Cycle::Open(cfg.cycle[0]),
    };
    let mut changes: Vec<(usize, usize, Option<String>)> = Vec::new();
    let mut completed = Vec::new();
    for i in fi..=li {
        let b = &o.blocks[i];
        if b.fence {
            continue;
        }
        let at = b.start + b.indent;
        match target {
            Cycle::Open(c) | Cycle::Done(c) => {
                if matches!(target, Cycle::Done(_)) && b.status != Some(c) {
                    completed.push(b.id);
                }
                match b.kind {
                    Kind::Para => changes.push((at, at, Some(format!("- [{c}] ")))),
                    Kind::Bullet => changes.push((at, b.content_start(), Some(format!("- [{c}] ")))),
                    Kind::Task if b.status != Some(c) => changes.push((at + 3, at + 4, Some(c.to_string()))),
                    Kind::Task => {}
                }
            }
            Cycle::Text => {
                if b.kind != Kind::Para {
                    changes.push((b.start, b.content_start(), None));
                }
            }
        }
    }
    // Back to text next to a paragraph with no blank row between: it joins it (the reverse of
    // a split). Never across a blank row.
    let mut joins: Vec<MarkId> = Vec::new();
    if target == Cycle::Text && r.is_empty() {
        let joinable = |x: &BlockInfo| x.is_plain_para() && x.depth == 0;
        if fi > 0 && joinable(&o.blocks[fi - 1]) && !b.gap {
            joins.push(b.id);
        }
        if let Some(c) = o.blocks.get(fi + 1) {
            if joinable(c) && !c.gap {
                joins.push(c.id);
            }
        }
    }
    if changes.is_empty() && joins.is_empty() {
        return Vec::new();
    }
    state.view.status = Some(match target {
        Cycle::Open(_) => "task",
        Cycle::Done(_) => "done",
        Cycle::Text => "text",
    }
    .into());
    let sel = mapped(state, &changes, Assoc::After);
    edit(state, changes, sel, false, move |m, _| {
        for id in joins {
            m.remove(id);
        }
    });
    completed.into_iter().map(|id| Effect::Completed { id }).collect()
}

/// The task cycle inside a multi-line paragraph: each selected line becomes its own task, and
/// the lines before and after stay paragraphs. The piece holding the first line keeps the
/// paragraph's id; the new pieces sit right against it (no blank row).
fn split_for_task(state: &mut State, o: &Outline, i: usize) -> Vec<Effect> {
    let cfg = state.doc.outline.clone().expect("outline");
    let b = o.blocks[i].clone();
    let text = state.doc.text.slice(..);
    let r = state.view.selection.primary();
    let (ka, kb) = (text.char_to_line(r.from()), text.char_to_line(r.to()));
    let marker = format!("- [{}] ", cfg.cycle[0]);
    let mut changes = Vec::new();
    for k in ka..=kb {
        let at = if k == b.first_line { b.start + b.indent } else { text.line_to_char(k) };
        changes.push((at, at, Some(marker.clone())));
    }
    let after = (kb < b.last_line()).then_some(kb + 1);
    let first = b.first_line;
    let sel = mapped(state, &changes, Assoc::After);
    state.view.status = Some("task".into());
    edit(state, changes, sel, false, move |m, new| {
        let tight = BlockAttrs { gap: Some(false) };
        for k in (ka..=kb).chain(after) {
            if k != first {
                m.mint_with(new.line_to_char(k), tight);
            }
        }
    });
    Vec::new()
}

fn set_status(state: &mut State, id: MarkId, ch: char) -> Vec<Effect> {
    let Some(o) = state.blocks() else { return notice(state, "only in outline documents") };
    let cfg = state.doc.outline.clone().expect("outline");
    let Some(b) = o.get(id).cloned() else { return notice(state, "no such block") };
    if b.kind != Kind::Task {
        return notice(state, "not a task");
    }
    if !cfg.is_task_char(ch) {
        return notice(state, &format!("'{ch}' isn't a task state"));
    }
    if b.status == Some(ch) {
        return Vec::new();
    }
    let at = b.start + b.indent + 3;
    let changes = vec![(at, at + 1, Some(ch.to_string()))];
    let sel = mapped(state, &changes, Assoc::After);
    edit(state, changes, sel, false, |_, _| {});
    if ch == cfg.cycle[1] {
        vec![Effect::Completed { id }]
    } else {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------------------
// Moving blocks

fn move_block(state: &mut State, dir: Dir) -> Vec<Effect> {
    let Some(o) = state.blocks() else { return notice(state, "only in outline documents") };
    let rope = state.doc.text.clone();
    let text = rope.slice(..);
    let i = o.index_at(text, state.caret());
    let d = o.blocks[i].depth;
    let n = o.blocks.len();
    let (upper, lower) = match dir {
        Dir::Backward => {
            let mut j = i as isize - 1;
            while j >= 0 && o.blocks[j as usize].depth > d {
                j -= 1;
            }
            if j < 0 || o.blocks[j as usize].depth != d {
                return notice(state, "first in its list · Shift-Tab to move out");
            }
            (j as usize, i)
        }
        Dir::Forward => {
            let e = o.subtree_end(i);
            if e >= n || o.blocks[e].depth != d {
                return notice(state, "last in its list · Shift-Tab to move out");
            }
            (i, e)
        }
    };
    let lower_end_block = o.subtree_end(lower);
    let u_start = o.blocks[upper].start;
    let l_start = o.blocks[lower].start;
    let last = &o.blocks[lower_end_block - 1];
    let l_end = next_line_start(text, last.last_line()).unwrap_or(text.len_chars());
    let le = le(state);
    let mut l_text = text.slice(l_start..l_end).to_string();
    let mut u_text = text.slice(u_start..l_start).to_string();
    if l_end == text.len_chars() && !l_text.ends_with('\n') {
        l_text.push_str(&le);
        if let Some(t) = u_text.strip_suffix(&le) {
            u_text = t.to_string();
        }
    }
    let l_len = l_text.chars().count();
    let u_marks: Vec<Mark> = state.doc.marks.in_range(u_start, l_start).to_vec();
    let l_marks: Vec<Mark> = state.doc.marks.in_range(l_start, l_end).to_vec();
    let new_text = format!("{l_text}{u_text}");
    let new_len = new_text.chars().count();
    // The caret moves with its block.
    let len = text.len_chars();
    let shift = |pos: usize| -> usize {
        if (l_start..l_end).contains(&pos) || (pos == l_end && l_end == len) {
            pos - l_start + u_start
        } else if (u_start..l_start).contains(&pos) {
            pos - u_start + u_start + l_len
        } else {
            pos
        }
    };
    let sel = state.view.selection.clone().transform(|r| Range { anchor: shift(r.anchor), head: shift(r.head), old_visual_position: None });
    edit(state, vec![(u_start, l_end, Some(new_text))], sel, false, move |m, _| {
        m.remove_range(u_start, u_start + new_len);
        for mk in &l_marks {
            let _ = m.insert(Mark { pos: mk.pos - l_start + u_start, ..*mk });
        }
        for mk in &u_marks {
            let _ = m.insert(Mark { pos: mk.pos - u_start + u_start + l_len, ..*mk });
        }
    });
    Vec::new()
}

// ---------------------------------------------------------------------------------------
// Selecting

fn select_block(state: &mut State, id: MarkId) -> Vec<Effect> {
    let Some(o) = state.blocks() else { return notice(state, "only in outline documents") };
    let Some(b) = o.get(id).cloned() else { return notice(state, "no such block") };
    state.view.selection = Selection::single(b.content_start(), b.end);
    Vec::new()
}

// ---------------------------------------------------------------------------------------
// Host blocks, paste, copy

fn insert_blocks(state: &mut State, after: Option<MarkId>, blocks: &[NewBlock]) -> Vec<Effect> {
    let Some(o) = state.blocks() else { return notice(state, "only in outline documents") };
    if blocks.is_empty() {
        return Vec::new();
    }
    let cfg = state.doc.outline.clone().expect("outline");
    let le = le(state);
    let n = le.chars().count();
    let at = match after {
        Some(id) => match o.get(id) {
            Some(b) => b.end,
            None => return notice(state, "no such block"),
        },
        None => 0,
    };
    let _ = n;
    // Built with `\n`; block starts are counted in lines from the insertion point's line.
    let mut body = String::new();
    let mut starts = Vec::new();
    let mut line = if after.is_some() { 1 } else { 0 };
    for (k, nb) in blocks.iter().enumerate() {
        if k > 0 {
            body.push('\n');
            line += 1;
        }
        starts.push((line, nb.mark, nb.gap));
        let lines = nb.to_lines(&cfg);
        line += lines.matches('\n').count();
        body.push_str(&lines);
    }
    let ins = if after.is_some() { format!("\n{body}") } else { format!("{body}\n") };
    let ins = if le == "\n" { ins } else { ins.replace('\n', &le) };
    let base = state.doc.text.char_to_line(at);
    let changes = vec![(at, at, Some(ins))];
    let sel = mapped(state, &changes, Assoc::Before);
    edit(state, changes, sel, false, move |m, new| {
        for (line, mark, gap) in starts {
            let pos = new.line_to_char(base + line);
            let attrs = BlockAttrs { gap };
            let placed = mark.is_some_and(|id| m.insert(Mark { pos, id, attrs }).is_ok());
            if !placed {
                if let Some(id) = m.at(pos) {
                    m.set_attrs(id, attrs);
                } else {
                    m.mint_with(pos, attrs);
                }
            }
        }
    });
    Vec::new()
}

fn paste(state: &mut State, text: Option<&str>, plain: bool) -> Option<Vec<Effect>> {
    let own = text.is_none_or(|t| state.doc.clipboard.is_own(&update::normalize_line_endings(t, "\n")));
    if own && !plain {
        if let Some(fx) = paste_whole(state) {
            return Some(fx);
        }
        // Whole blocks elsewhere (inside a block's text): as Markdown blocks.
        if state.doc.clipboard.blocks && single(state).is_some() {
            if let Some(md) = state.doc.clipboard.external.clone() {
                let (blocks, _) = markdown::parse_markdown(&md, false);
                if !blocks.is_empty() {
                    paste_blocks(state, &blocks);
                    return Some(Vec::new());
                }
            }
        }
    }
    let text = text?;
    let text = update::normalize_line_endings(text, "\n");
    if state.doc.clipboard.is_own(&text) || !text.contains('\n') {
        // The register (with its marks), or one line: the plain editor pastes it as is.
        return None;
    }
    single(state)?;
    let (blocks, images) = markdown::parse_markdown(&text, plain);
    if blocks.is_empty() {
        return Some(notice(state, &format!("left out {}", plural(images, "image"))));
    }
    paste_blocks(state, &blocks);
    if images > 0 {
        state.view.status = Some(format!("left out {}", plural(images, "image")));
    }
    Some(Vec::new())
}

fn plural(n: usize, what: &str) -> String {
    if n == 1 { format!("1 {what}") } else { format!("{n} {what}s") }
}

/// Pasted blocks: the first joins the text before the caret (taking its shape when nothing
/// is before it), the rest follow as new blocks, and the text after the caret ends the last.
fn paste_blocks(state: &mut State, blocks: &[NewBlock]) {
    let mut merge = false;
    if !state.view.selection.primary().is_empty() {
        update::delete(state, None, |_, _, head| (head, head));
        merge = true;
    }
    let o = state.blocks().expect("outline");
    let cfg = state.doc.outline.clone().expect("outline");
    let text = state.doc.text.slice(..);
    let le = le(state);
    let n = le.chars().count();
    let p = state.caret();
    let b = o.block_at(text, p).clone();
    let before_empty = p == b.content_start();
    let base = if b.is_item() { b.depth } else { 0 };
    let from = if before_empty { b.start } else { p };
    let _ = n;
    let mut body = String::new();
    let mut starts = Vec::new();
    let mut line = 0;
    for (k, nb) in blocks.iter().enumerate() {
        let mut nb = nb.clone();
        if nb.kind != Kind::Para {
            nb.depth += base;
        }
        let piece = if k == 0 && !before_empty { nb.text.clone() } else { nb.to_lines(&cfg) };
        if k > 0 {
            body.push('\n');
            line += 1;
            starts.push((line, nb.gap));
        }
        line += piece.matches('\n').count();
        body.push_str(&piece);
    }
    let body = if le == "\n" { body } else { body.replace('\n', &le) };
    let base_line = text.char_to_line(from);
    let caret = from + body.chars().count();
    let (own, own_start) = (b.id, b.start);
    edit(state, vec![(from, p, Some(body))], caret_at(caret), merge, move |m, new| {
        // The first piece joins the caret's block, which keeps its line.
        if let Some(mk) = m.remove(own) {
            let _ = m.insert(Mark { pos: own_start, ..mk });
        }
        for (line, gap) in starts {
            let pos = new.line_to_char(base_line + line);
            if let Some(id) = m.at(pos) {
                m.set_attrs(id, BlockAttrs { gap });
            } else {
                m.mint_with(pos, BlockAttrs { gap });
            }
        }
    });
}

fn copy(state: &mut State, cut: bool) -> Option<Vec<Effect>> {
    let r = single(state)?;
    if r.is_empty() {
        return None;
    }
    let o = state.blocks()?;
    if let Some((i0, i1)) = whole_blocks(state, &o, r) {
        return Some(copy_blocks(state, &o, i0, i1, cut));
    }
    let text = state.doc.text.slice(..);
    let raw = text.slice(r.from()..r.to()).to_string();
    let md = markdown::to_markdown(state, &o, r.from(), r.to());
    let n_blocks = o.indices_between(text, r.from(), r.to()).count();
    let what = if n_blocks > 1 { plural(n_blocks, "block") } else { update::count_label(&raw) };
    state.view.status = Some(format!("{} {what}", if cut { "cut" } else { "copied" }));
    let mut marks = Vec::new();
    if cut {
        let removed = update::delete_marks(state, None, |_, _, head| (head, head));
        marks = removed
            .iter()
            .filter(|m| r.from() <= m.pos && m.pos <= r.to())
            .map(|m| ClipMark { offset: m.pos - r.from(), id: m.id, attrs: m.attrs })
            .collect();
    }
    let external = (md != raw).then(|| md.clone());
    state.doc.clipboard = Clipboard { text: raw, external, marks, blocks: false };
    Some(vec![Effect::ClipboardSet { text: md }])
}

/// Whether a selection takes whole blocks: from a block's content start to the end of a later
/// block, or to the start (or content start) of a block after it (as Shift-↓ from a content
/// start selects). Returns the first and last block taken.
fn whole_blocks(state: &State, o: &Outline, r: Range) -> Option<(usize, usize)> {
    let text = state.doc.text.slice(..);
    let i0 = o.index_at(text, r.from());
    let first = &o.blocks[i0];
    if r.from() < first.start || r.from() > first.content_start() || first.atomic {
        return None;
    }
    let j = o.index_at(text, r.to());
    let b = &o.blocks[j];
    // At an empty block's content start the selection is also at its end: it takes that
    // block too, as deleting the selection would (its marker is selected).
    if j > i0 && r.to() == b.end {
        return Some((i0, j));
    }
    (j > i0 && (r.to() == b.start || r.to() == b.content_start())).then(|| (i0, j - 1))
}

/// Copies (or cuts) blocks `i0..=i1` whole: their lines, markers and indentation included,
/// so a paste puts them back as blocks. A cut takes their lines out and keeps their ids in
/// the register.
fn copy_blocks(state: &mut State, o: &Outline, i0: usize, i1: usize, cut: bool) -> Vec<Effect> {
    let (first, last) = (o.blocks[i0].clone(), o.blocks[i1].clone());
    let le = le(state);
    let text = state.doc.text.slice(..);
    let lines = text.slice(first.start..last.end).to_string();
    let raw = format!("{lines}{le}");
    let md = markdown::to_markdown(state, o, first.content_start(), last.end);
    let n = i1 - i0 + 1;
    state.view.status = Some(format!("{} {}", if cut { "cut" } else { "copied" }, plural(n, "block")));
    let mut marks = Vec::new();
    if cut {
        // The lines go with one line break: the one after them, or the one before the last
        // line of the document.
        let (from, to) = match o.blocks.get(i1 + 1) {
            Some(next) => (first.start, next.start),
            None if first.first_line > 0 => (line_end(text, first.first_line - 1), last.end),
            None => (0, last.end),
        };
        let caret = from.min(text.len_chars() - (to - from));
        let txn = Transaction::change(&state.doc.text, [(from, to, None)].into_iter()).with_selection(caret_at(caret));
        let removed = update::commit_with(state, txn, Step::default(), |_, _| {});
        marks = removed
            .iter()
            .filter(|m| first.start <= m.pos && m.pos <= last.end)
            .map(|m| ClipMark { offset: m.pos - first.start, id: m.id, attrs: m.attrs })
            .collect();
    }
    state.doc.clipboard = Clipboard { text: raw, external: Some(md.clone()), marks, blocks: true };
    vec![Effect::ClipboardSet { text: md }]
}

/// Pastes whole blocks from the register at the caret, when the caret is on an empty item
/// (the blocks take its place), at a block's content start (they go before it) or at the
/// end of a block (they follow its subtree as siblings). Their own kinds and statuses stay; their depths move to the target's; a cut's
/// ids come back. `None` when the caret is elsewhere.
fn paste_whole(state: &mut State) -> Option<Vec<Effect>> {
    let clip = state.doc.clipboard.clone();
    let mut r = single(state)?;
    if !clip.blocks {
        return None;
    }
    // Over a selection of whole blocks (what a copy of them selects): they go, and the
    // register takes their place, in one step.
    let mut merge = false;
    if !r.is_empty() {
        let o = state.blocks()?;
        whole_blocks(state, &o, r)?;
        update::delete(state, None, |_, _, head| (head, head));
        merge = true;
        r = single(state)?;
    }
    let o = state.blocks()?;
    let cfg = state.doc.outline.clone()?;
    let text = state.doc.text.slice(..);
    let i = o.index_at(text, r.head);
    let b = o.blocks[i].clone();
    let empty = b.is_empty() && r.head == b.content_start();
    let before = !empty && r.head == b.content_start();
    if !empty && !before && r.head != b.end {
        return None;
    }
    // The register's blocks, re-indented to the target's depth.
    let reg = crate::helix::Rope::from(clip.text.trim_end_matches(['\n', '\r']));
    let ro = crate::outline::derive(reg.slice(..), &Marks::new(), &cfg);
    let base = ro.blocks.iter().filter(|x| x.is_item()).map(|x| x.depth).min().unwrap_or(0);
    let target = if b.is_item() { b.depth } else { 0 };
    let unit = cfg.indent.max(1) as usize;
    let mut body_lines: Vec<String> = Vec::new();
    let mut line_offsets: Vec<usize> = Vec::new(); // each register line's start in the register
    let mut at = 0usize;
    for (k, line) in reg.lines().enumerate() {
        line_offsets.push(at);
        at += line.len_chars();
        let s = line.to_string();
        let s = s.trim_end_matches(['\n', '\r']).to_string();
        let blk = ro.block_of_line(k);
        if blk.first_line == k && blk.is_item() {
            let depth = (blk.depth as isize - base as isize + target as isize).max(0) as usize;
            body_lines.push(format!("{}{}", " ".repeat(depth * unit), &s[blk.indent.min(s.len())..]));
        } else {
            body_lines.push(s);
        }
    }
    let le = le(state);
    let body = body_lines.join(&le);
    let reg_line = |offset: usize| line_offsets.partition_point(|&s| s <= offset).saturating_sub(1);
    let carried: Vec<(usize, ClipMark)> = clip.marks.iter().map(|c| (reg_line(c.offset), *c)).collect();
    let starts: Vec<usize> = ro.blocks.iter().map(|x| x.first_line).collect();
    let (from, to, ins, first_line) = if empty {
        (b.start, b.end, body, b.first_line)
    } else if before {
        (b.start, b.start, format!("{body}{le}"), b.first_line)
    } else {
        let end = o.blocks[o.subtree_end(i) - 1].clone();
        (end.end, end.end, format!("{le}{body}"), end.last_line() + 1)
    };
    let caret = from + ins.chars().count() - if before { le.chars().count() } else { 0 };
    let own = empty.then_some(b.id);
    edit(state, vec![(from, to, Some(ins))], caret_at(caret), merge, move |m, new| {
        // The emptied item's id stays on the first pasted line unless the register brings one.
        let lead = carried.iter().any(|(l, _)| *l == 0);
        if let Some(id) = own {
            if let Some(mk) = m.remove(id) {
                if !lead {
                    let _ = m.insert(Mark { pos: from, ..mk });
                }
            }
        }
        for (l, c) in carried {
            let pos = new.line_to_char(first_line + l);
            if m.at(pos).is_none() && !m.contains(c.id) {
                let _ = m.insert(Mark { pos, id: c.id, attrs: c.attrs });
            }
        }
        // Every pasted block starts a block here, an empty paragraph too (nothing else would
        // tell it from a line of the block above).
        for l in starts {
            let pos = new.line_to_char(first_line + l);
            if m.at(pos).is_none() {
                m.mint(pos);
            }
        }
    });
    Some(Vec::new())
}

// ---------------------------------------------------------------------------------------
// The caret invariant

/// Keeps every selection end out of block prefixes and atomic blocks, after any message.
/// `prev` is the selection before the message.
pub(crate) fn normalize(state: &mut State, prev: &Selection, msg: &Msg) {
    if let Some(selection) = normalized(&state.doc, &state.view.selection, prev, msg) {
        state.view.selection = selection;
    }
}

/// [`normalize`] for any selection of `doc`: the fixed selection, when it changes.
pub(crate) fn normalized(doc: &Document, selection: &Selection, prev: &Selection, msg: &Msg) -> Option<Selection> {
    let o = doc.blocks()?;
    let rope = doc.text.clone();
    let text = rope.slice(..);
    let moving = matches!(msg, Msg::Move { .. });
    let back = matches!(msg, Msg::Move { dir: Dir::Backward, by: By::Grapheme | By::Word, .. });
    let snap = |pos: usize, back: bool| -> usize {
        let i = o.index_at(text, pos);
        let b = &o.blocks[i];
        if b.prefix_len > 0 && pos >= b.start && pos < b.content_start() {
            if back && b.first_line > 0 {
                line_end(text, b.first_line - 1)
            } else {
                b.content_start()
            }
        } else {
            pos
        }
    };
    let was_focused = |b: &BlockInfo| prev.iter().any(|r| r.from() == b.content_start() && r.to() == b.end && !r.is_empty());
    let focus_range = |b: &BlockInfo, r: &Range| Range {
        anchor: b.content_start(),
        head: b.end,
        old_visual_position: r.old_visual_position,
    };
    let fix = |r: &Range| -> Range {
        let mut head = snap(r.head, back);
        let mut anchor = if r.anchor == r.head { head } else { snap(r.anchor, false) };
        if anchor == head {
            let b = o.block_at(text, head);
            if b.atomic && head >= b.content_start() && head <= b.end {
                if moving && was_focused(b) && (head == b.content_start() || head == b.end) {
                    // Leaving a focused block: one press to the neighbour.
                    let out = if head == b.end {
                        next_line_start(text, b.last_line()).map(|p| snap(p, false))
                    } else {
                        (b.first_line > 0).then(|| line_end(text, b.first_line - 1))
                    };
                    return match out {
                        Some(p) => {
                            let nb = o.block_at(text, p);
                            if nb.atomic && p >= nb.content_start() && p <= nb.end {
                                focus_range(nb, r)
                            } else {
                                Range { anchor: p, head: p, old_visual_position: r.old_visual_position }
                            }
                        }
                        None => focus_range(b, r),
                    };
                }
                return focus_range(b, r);
            }
            return Range { anchor, head, old_visual_position: r.old_visual_position };
        }
        // A selection takes an atomic block whole.
        let hb = o.block_at(text, head);
        if hb.atomic {
            let (cs, end) = (hb.content_start(), hb.end);
            if head > anchor && (cs..end).contains(&head) {
                head = end;
            } else if head < anchor && head > cs && head <= end {
                head = cs;
            }
        }
        let ab = o.block_at(text, anchor);
        if ab.atomic {
            let (cs, end) = (ab.content_start(), ab.end);
            if anchor > cs && anchor < end {
                anchor = if anchor < head { cs } else { end };
            }
        }
        Range { anchor, head, old_visual_position: r.old_visual_position }
    };
    let ranges: crate::helix::SmallVec<[Range; 1]> = selection.iter().map(fix).collect();
    let primary = selection.primary_index();
    let fixed = Selection::new(ranges, primary);
    (fixed != *selection).then_some(fixed)
}

/// Whether `r` is a focused atomic block (a vertical motion from it keeps its goal column).
pub(crate) fn is_focused_atomic(state: &State, r: &Range) -> bool {
    match state.blocks() {
        Some(o) => focused_atomic(&o, state.doc.text.slice(..), r).is_some(),
        None => false,
    }
}

/// The content start of the block before or after the one holding `pos` (`⌃↑` / `⌃↓`).
pub(crate) fn block_step(state: &State, pos: usize, dir: Dir) -> usize {
    let Some(o) = state.blocks() else { return pos };
    let text = state.doc.text.slice(..);
    let i = o.index_at(text, pos);
    match dir {
        Dir::Forward => o.blocks.get(i + 1).map_or(text.len_chars(), |b| b.content_start()),
        Dir::Backward => {
            let b = &o.blocks[i];
            if pos > b.content_start() {
                b.content_start()
            } else if i > 0 {
                o.blocks[i - 1].content_start()
            } else {
                0
            }
        }
    }
}

/// After an edit, new block starts get a mark. Runs inside the commit.
pub(crate) fn settle(state: &mut State) {
    crate::outline::mint_missing(&mut state.doc);
}

/// What an edit may not change: blank rows before blocks, by id (see [`pin`]).
pub(crate) enum Pins {
    /// A key at the caret (typing, Backspace, Delete): if the caret's block changes kind or
    /// depth, it and the block after it keep their blank rows.
    Near { id: MarkId, shape: (Kind, u16), gaps: Vec<(MarkId, bool)> },
    /// Tab, Shift-Tab, the task cycle: every block keeps its blank row.
    All(Vec<(MarkId, bool)>),
}

/// The blank rows an edit by `msg` must keep, read before it runs.
pub(crate) fn pins_for(state: &State, msg: &Msg) -> Option<Pins> {
    let o = state.blocks()?;
    match msg {
        Msg::InsertText { .. }
        | Msg::DeleteBackward
        | Msg::DeleteForward
        | Msg::DeleteWordBackward
        | Msg::DeleteWordForward
        | Msg::DeleteToLineStart
        | Msg::DeleteToLineEnd
        | Msg::KillLine => {
            let i = o.index_at(state.doc.text.slice(..), state.caret());
            let b = &o.blocks[i];
            let gaps = o.blocks[i..(i + 2).min(o.blocks.len())].iter().map(|b| (b.id, b.gap)).collect();
            Some(Pins::Near { id: b.id, shape: (b.kind, b.depth), gaps })
        }
        Msg::Indent | Msg::Outdent | Msg::TaskCycle => Some(Pins::All(o.blocks.iter().map(|b| (b.id, b.gap)).collect())),
        _ => None,
    }
}

/// Writes the blank rows `pins` kept wherever the edit changed them, as part of the same
/// undo step (a kind change never moves another block).
pub(crate) fn pin(state: &mut State, pins: Pins) {
    let Some(o) = state.blocks() else { return };
    let keep = match pins {
        Pins::Near { id, shape, gaps } => match o.get(id) {
            Some(b) if (b.kind, b.depth) != shape => gaps,
            _ => return,
        },
        Pins::All(gaps) => gaps,
    };
    let mut changes = Vec::new();
    for (id, gap) in keep {
        if let Some(i) = o.index_of(id) {
            if i > 0 && o.blocks[i].gap != gap {
                changes.push((id, gap));
            }
        }
    }
    if changes.is_empty() {
        return;
    }
    let txn = Transaction::new(&state.doc.text);
    update::commit_with(state, txn, Step { kind: None, replaced: false, merge: true }, move |m, _| {
        for (id, gap) in changes {
            m.set_attrs(id, BlockAttrs { gap: Some(gap) });
        }
    });
}
