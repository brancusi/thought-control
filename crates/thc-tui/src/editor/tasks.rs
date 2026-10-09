//! thc's tasks, on top of caretline's generic pieces.
//!
//! caretline knows a bullet may carry a one-character tag (`- [c] `, `OutlineConfig::tags`)
//! and nothing about what it means. thc gives the tags its statuses (` ` todo, `x` done, `/`
//! doing, `w` waiting, `-` cancelled) and registers on each document's host:
//!
//! - `thc.task_cycle` (⌃T): text → open task → done → text, over the caret's block or every
//!   selected block. Inside a multi-line paragraph the selected lines split out as tasks; back
//!   to text joins the paragraph next to it again.
//! - `thc.set_status {id, status}` (a click on a box).
//! - The `[ ] ` shorthand (an input rule): `[ ] `, `[x] ` or `[] ` typed at a paragraph line's
//!   start makes the line a task.
//! - A decorator that draws a task's box in the hang.
//!
//! Completing a task emits the host effect [`COMPLETED`]; thc saves at once on it.

use caretline as cn;
use cn::helix::{Assoc, Range, Selection, Tendril, Transaction};
use cn::outline::{BlockInfo, Hang, Kind, OutlineConfig};
use cn::{Ctx, Deco, Decoration, Edit, Host, MarkAttrs, MarkId, MarkOp, Msg};
use serde_json::{Value, json};

/// thc's statuses: each tag character and its name. The first is a new task's.
pub const STATUSES: [(char, &str); 5] = [(' ', "todo"), ('x', "done"), ('/', "doing"), ('w', "waiting"), ('-', "cancelled")];

/// ⌃T's cycle: open, then done.
const OPEN: char = ' ';
const DONE: char = 'x';

pub const TASK_CYCLE: &str = "thc.task_cycle";
pub const SET_STATUS: &str = "thc.set_status";
/// The effect a command emits when a task reaches done: `{"id": <mark>}`.
pub const COMPLETED: &str = "thc.completed";

/// thc's outline: two spaces per depth, its statuses as tags, a new item after a task an open
/// task, whole-line images.
pub fn config() -> OutlineConfig {
    let mut c = OutlineConfig::default();
    c.tags = STATUSES.iter().map(|(c, _)| *c).collect();
    c.new_tag = Some(OPEN);
    // Tab nests a note right under the one above: no blank row between them (tta6t, decided
    // 2026-10-08). ⇧Tab leaves blank rows as they are, so it moves nothing.
    c.nest_joins = true;
    c
}

/// The host every thc document runs with.
pub fn host() -> Host {
    Host::new()
        .command(TASK_CYCLE, task_cycle)
        .command(SET_STATUS, set_status)
        .input_rule("thc.task_shorthand", shorthand)
        .input_rule("thc.enter", enter)
        .input_rule("thc.join", join)
        .input_rule("thc.trim_split", trim_split)
        .decorator(decorate)
}

pub fn is_task(b: &BlockInfo) -> bool {
    b.kind == Kind::Bullet && b.tag.is_some()
}

fn is_status(c: char) -> bool {
    STATUSES.iter().any(|(s, _)| *s == c)
}

/// A status's tag character (`todo` → ' ').
pub fn status_char(status: Option<&str>) -> char {
    status.and_then(|s| STATUSES.iter().find(|(_, n)| *n == s)).map_or(OPEN, |(c, _)| *c)
}

/// A tag character's status (' ' → `todo`).
pub fn status_name(ch: Option<char>) -> &'static str {
    ch.and_then(|c| STATUSES.iter().find(|(s, _)| *s == c)).map_or("todo", |(_, n)| n)
}

fn removal(at: usize, to: usize) -> (usize, usize, String) {
    (at, to, String::new())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Open(char),
    Done(char),
    Text,
}

/// ⌃T (see the module docs).
fn task_cycle(ctx: &Ctx, _: &Value) -> Result<Edit, String> {
    let o = ctx.blocks().ok_or("only in outline documents")?;
    let text = ctx.text();
    let r = ctx.selection().primary();
    let (fi, li) = (o.index_at(text, r.from()), o.index_at(text, r.to()));
    let b = o.blocks[fi].clone();
    if fi == li && b.kind == Kind::Para && !b.fence && b.line_count > 1 {
        return Ok(split(ctx, &b));
    }
    let target = match b.tag {
        Some(c) if is_task(&b) && c == DONE => Target::Text,
        Some(_) if is_task(&b) => Target::Done(DONE),
        _ => Target::Open(OPEN),
    };
    let mut changes: Vec<(usize, usize, String)> = Vec::new();
    let mut completed = Vec::new();
    for x in &o.blocks[fi..=li] {
        if x.fence {
            continue;
        }
        let at = x.start + x.indent;
        match target {
            Target::Open(c) | Target::Done(c) => {
                if matches!(target, Target::Done(_)) && x.tag != Some(c) {
                    completed.push(x.id);
                }
                if is_task(x) {
                    if x.tag != Some(c) {
                        changes.push((at + 3, at + 4, c.to_string()));
                    }
                } else if x.kind == Kind::Para {
                    changes.push((at, at, format!("- [{c}] ")));
                } else {
                    changes.push((at, x.content_start(), format!("- [{c}] ")));
                }
            }
            Target::Text => {}
        }
    }
    // Back to text next to a paragraph with no blank row between: it joins it (the reverse of
    // a split, ⌃T in a paragraph). Never across a blank row.
    let mut joins: Vec<MarkId> = Vec::new();
    if target == Target::Text && r.is_empty() {
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
    // Back to text is a bullet note, as typed lines are (the Logseq model, writing.md §1); a
    // task that rejoins a paragraph becomes paragraph text again.
    if target == Target::Text {
        for x in o.blocks[fi..=li].iter().filter(|x| is_task(x) && !x.fence) {
            let at = x.start + x.indent;
            if joins.is_empty() {
                changes.push(removal(at + 2, x.content_start()));
            } else {
                changes.push(removal(x.start, x.content_start()));
            }
        }
    }
    if changes.is_empty() && joins.is_empty() {
        return Ok(Edit::default());
    }
    let status = match target {
        Target::Open(_) => "task",
        Target::Done(_) => "done",
        Target::Text => "text",
    };
    Ok(Edit {
        selection: Some(ctx.mapped_selection(&changes)),
        changes,
        marks: joins.into_iter().map(|id| MarkOp::Remove { id }).collect(),
        status: Some(status.into()),
        effects: completed.into_iter().map(|id| (COMPLETED.to_string(), json!({ "id": id.0 }))).collect(),
        keep_gaps: true,
        then_default: false,
    })
}

/// Enter, the Logseq way (writing.md §1, decided 2026-10-08): Enter always starts a new note,
/// and ⇧Enter (⌃J) breaks the line inside one. caretline's own Enter already splits a list
/// item and makes a new empty item after one; this rule covers the rest:
///
/// - an item with children (unfolded), Enter at its end: the new note is its first child, as
///   an outliner makes it, never a sibling its children would then nest under;
/// - a heading or quote, Enter at its end: the next note is a bullet;
/// - a paragraph (from imported Markdown): Enter splits it into two paragraph notes where
///   caretline would break the line; at a line's end the new empty note between is a bullet,
///   since a typed line is a bullet note.
///
/// An empty item is the editor's call (`Doc::run_command`: it outdents, ends a task, or does
/// nothing at the top level). A split drops the spaces after the caret, as `trim_split` does.
fn enter(ctx: &Ctx, msg: &Msg) -> Option<Edit> {
    if !matches!(msg, Msg::InsertNewline) || ctx.selection().len() != 1 {
        return None;
    }
    let r = ctx.selection().primary();
    if !r.is_empty() {
        return None;
    }
    let o = ctx.blocks()?;
    let text = ctx.text();
    let p = r.head;
    let i = o.index_at(text, p);
    let b = &o.blocks[i];
    if is_task(b) && b.is_empty() && b.depth == 0 {
        // An empty task ends the checklist: a plain bullet.
        let at = b.start + b.indent;
        return Some(Edit { changes: vec![(at + 2, b.content_start(), String::new())], selection: Some(Selection::point(at + 2)), ..Edit::default() });
    }
    if b.fence || b.atomic || b.is_empty() {
        return None;
    }
    let le = ctx.line_ending();
    let pad = " ".repeat(b.indent);
    let insert = |at: usize, ins: String, caret: usize| Edit { changes: vec![(at, at, ins)], selection: Some(Selection::point(caret)), ..Edit::default() };
    match b.hang {
        Hang::Bullet if p == b.end => {
            let child = o.blocks.get(i + 1).filter(|c| c.depth > b.depth && !ctx.view.folds.contains(&b.id))?;
            let marker = if is_task(b) { format!("- [{OPEN}] ") } else { "- ".to_string() };
            let ins = format!("{le}{}{marker}", " ".repeat(child.indent));
            let caret = p + ins.chars().count();
            Some(insert(p, ins, caret))
        }
        Hang::Heading(_) | Hang::Quote if p == b.end => {
            let ins = format!("{le}{pad}- ");
            let caret = p + ins.chars().count();
            Some(insert(p, ins, caret))
        }
        Hang::None if b.kind == Kind::Para => {
            let line = text.char_to_line(p);
            let ls = text.line_to_char(line);
            let lend = ls + text.line(line).chars().take_while(|c| *c != '\n' && *c != '\r').count();
            let first = line == b.first_line;
            if first && p == b.content_start() {
                // At its very start: a new empty bullet above; the paragraph keeps its id.
                let ins = format!("{pad}- {le}");
                let caret = p + ins.chars().count();
                return Some(insert(b.start, ins, caret));
            }
            if !first && p == ls {
                // At a later line's start: caretline's split (this line starts a paragraph).
                return None;
            }
            if p == lend {
                // At a line's end: a new empty bullet after it; the lines below, if any, are a
                // paragraph of their own.
                let ins = format!("{le}{pad}- ");
                let n = ins.chars().count();
                if line == b.last_line() {
                    return Some(insert(p, ins, p + n));
                }
                let next = text.line_to_char(line + 1);
                let mut changes = vec![(p, p, ins)];
                if !pad.is_empty() {
                    changes.push((next, next, pad.clone()));
                }
                let mark = MarkOp::Mint { pos: next + n, attrs: MarkAttrs::default() };
                return Some(Edit { changes, selection: Some(Selection::point(p + n)), marks: vec![mark], ..Edit::default() });
            }
            // Inside a line: the rest is a new paragraph note.
            let trim = text.chars_at(p).take_while(|c| *c == ' ').count();
            let ins = format!("{le}{pad}");
            let n = ins.chars().count();
            let mark = MarkOp::Mint { pos: p + le.chars().count(), attrs: MarkAttrs::default() };
            Some(Edit { changes: vec![(p, p + trim, ins)], selection: Some(Selection::point(p + n)), marks: vec![mark], ..Edit::default() })
        }
        _ => None,
    }
}

/// ⌫ at the start of a bullet note, the Logseq way (writing.md §1): the note joins the one
/// above, its text after that note's (an empty one just goes), and its children become that
/// note's. Never a paragraph: typed lines are bullets, and paragraphs come only from imported
/// Markdown. The first note stays as it is. (⌫ at a task's start still takes its box off
/// first, caretline's own step.)
fn join(ctx: &Ctx, msg: &Msg) -> Option<Edit> {
    if !matches!(msg, Msg::DeleteBackward) || ctx.selection().len() != 1 {
        return None;
    }
    let r = ctx.selection().primary();
    if !r.is_empty() {
        return None;
    }
    let o = ctx.blocks()?;
    let text = ctx.text();
    let i = o.index_at(text, r.head);
    let b = &o.blocks[i];
    if b.hang != Hang::Bullet || b.tag.is_some() || r.head != b.content_start() {
        return None;
    }
    if i == 0 {
        return Some(Edit::default());
    }
    let a = &o.blocks[i - 1];
    if a.atomic {
        return None;
    }
    let mut changes = vec![(a.end, b.content_start(), String::new())];
    // Its children under the note above: as many levels out as it was deeper than that note.
    let out = b.depth.saturating_sub(a.depth) as usize * ctx.doc.outline.as_ref().map_or(2, |c| c.indent.max(1) as usize);
    if out > 0 {
        for c in o.blocks[i + 1..].iter().take_while(|c| c.depth > b.depth) {
            changes.push((c.start, c.start + out.min(c.indent), String::new()));
        }
    }
    Some(Edit { changes, selection: Some(Selection::point(a.end)), marks: vec![MarkOp::Remove { id: b.id }], keep_gaps: true, ..Edit::default() })
}

/// Enter that splits a line takes the spaces after the caret with it (0d61e): a saved note never
/// starts with spaces, so the new one would read differently on reopening. One undo step with
/// the split (`then_default`: these changes, then Enter's own).
fn trim_split(ctx: &Ctx, msg: &caretline::Msg) -> Option<Edit> {
    if !matches!(msg, caretline::Msg::InsertNewline) || ctx.selection().len() != 1 {
        return None;
    }
    let r = ctx.selection().primary();
    let n = ctx.text().chars_at(r.head).take_while(|c| *c == ' ').count();
    (r.is_empty() && n > 0).then(|| Edit { changes: vec![(r.head, r.head + n, String::new())], ..Edit::then_default() })
}

/// ⌃T inside a multi-line paragraph: each selected line becomes its own task, and the lines
/// before and after stay paragraphs. The piece holding the first line keeps the paragraph's
/// id; the new pieces sit right against it (no blank row).
fn split(ctx: &Ctx, b: &BlockInfo) -> Edit {
    let text = ctx.text();
    let r = ctx.selection().primary();
    let (ka, kb) = (text.char_to_line(r.from()), text.char_to_line(r.to()));
    let marker = format!("- [{OPEN}] ");
    let changes: Vec<(usize, usize, String)> = (ka..=kb)
        .map(|k| {
            let at = if k == b.first_line { b.start + b.indent } else { text.line_to_char(k) };
            (at, at, marker.clone())
        })
        .collect();
    let after = (kb < b.last_line()).then_some(kb + 1);
    // Where each new piece's line starts once the markers are in.
    let txn = Transaction::change(&ctx.doc.text, changes.iter().map(|(a, z, t)| (*a, *z, Some(Tendril::from(t.as_str())))));
    let marks = (ka..=kb)
        .chain(after)
        .filter(|&k| k != b.first_line)
        .map(|k| MarkOp::Mint { pos: txn.changes().map_pos(text.line_to_char(k), Assoc::Before), attrs: MarkAttrs::gap(Some(false)) })
        .collect();
    Edit { selection: Some(ctx.mapped_selection(&changes)), changes, marks, status: Some("task".into()), keep_gaps: true, ..Edit::default() }
}

/// A task's status set (a click on its box): `{"id": <mark>, "status": "x"}`.
fn set_status(ctx: &Ctx, args: &Value) -> Result<Edit, String> {
    let o = ctx.blocks().ok_or("only in outline documents")?;
    let id = args.get("id").and_then(Value::as_u64).map(MarkId).ok_or("set_status needs an id")?;
    let ch = args.get("status").and_then(Value::as_str).and_then(|s| s.chars().next()).ok_or("set_status needs a status")?;
    let b = o.get(id).ok_or("no such block")?;
    if !is_task(b) {
        return Ok(Edit::status("not a task"));
    }
    if !is_status(ch) {
        return Ok(Edit::status(format!("'{ch}' isn't a task state")));
    }
    if b.tag == Some(ch) {
        return Ok(Edit::default());
    }
    let at = b.start + b.indent + 3;
    let changes = vec![(at, at + 1, ch.to_string())];
    Ok(Edit {
        selection: Some(ctx.mapped_selection(&changes)),
        changes,
        effects: if ch == DONE { vec![(COMPLETED.to_string(), json!({ "id": id.0 }))] } else { Vec::new() },
        ..Edit::default()
    })
}

/// A bare box typed at the start of a paragraph's line (`[ ] `, `[x] `, `[] `) is shorthand
/// for a task: it becomes `- [c] `, and that line a task (on a later line, a new block). One
/// step, the typed space included.
fn shorthand(ctx: &Ctx, msg: &Msg) -> Option<Edit> {
    let Msg::InsertText { text: typed } = msg else { return None };
    let sel = ctx.selection();
    if sel.len() != 1 {
        return None;
    }
    let r: Range = sel.primary();
    if !r.is_empty() || !(typed.ends_with(' ') || typed.starts_with(' ')) || typed.contains(['\n', '\r']) {
        return None;
    }
    let o = ctx.blocks()?;
    let text = ctx.text();
    let p = r.head;
    let b = o.block_at(text, p);
    if b.hang == Hang::Bullet && p == b.content_start() && typed.starts_with(' ') {
        // A saved note never starts with a space (0d61e): one typed at a note's start would
        // vanish on save and the line shift on reopening, so it never goes in.
        let rest = typed.trim_start_matches(' ');
        return Some(Edit { changes: vec![(p, p, rest.to_string())], selection: Some(Selection::point(p + rest.chars().count())), ..Edit::default() });
    }
    if b.hang == Hang::Bullet && b.tag.is_none() {
        return bullet_shorthand(ctx, b, p, typed);
    }
    if !typed.ends_with(' ') || !b.is_plain_para() {
        return None;
    }
    let line = text.char_to_line(p);
    let ls = text.line_to_char(line);
    let indent = if line == b.first_line { b.indent } else { 0 };
    let before: String = text.slice(ls + indent.min(p - ls)..p).chars().chain(typed.chars()).collect();
    let inner = before.strip_prefix('[')?.strip_suffix("] ")?;
    let ch = match inner.chars().count() {
        0 => OPEN,
        1 => inner.chars().next().filter(|&c| is_status(c))?,
        _ => return None,
    };
    let from = ls + indent.min(p - ls);
    let ins = format!("- [{ch}] ");
    let caret = from + ins.chars().count();
    Some(Edit { changes: vec![(from, p, ins)], selection: Some(Selection::point(caret)), ..Edit::default() })
}

/// Markdown typed at the start of a bullet note's text, where typed lines are bullets already
/// (writing.md §1): a box (`[ ] `, `[x] `, `[] `) makes it a task, `- ` or `* ` is the bullet it
/// already is (nothing changes), `1. ` makes it a numbered item, and `# ` (to `### `) on a
/// top-level note makes it a heading.
fn bullet_shorthand(ctx: &Ctx, b: &BlockInfo, p: usize, typed: &str) -> Option<Edit> {
    let text = ctx.text();
    let cs = b.content_start();
    if p < cs || text.char_to_line(p) != b.first_line {
        return None;
    }
    let before: String = text.slice(cs..p).chars().chain(typed.chars()).collect();
    if before == "- " || before == "* " {
        return Some(Edit { changes: vec![(cs, p, String::new())], selection: Some(Selection::point(cs)), ..Edit::default() });
    }
    let numbered = {
        let digits = before.chars().take_while(char::is_ascii_digit).count();
        (1..=3).contains(&digits) && matches!(&before[digits..], ". " | ") ")
    };
    if numbered {
        // `1. ` starts a numbered item: the number is its marker, in place of the bullet.
        let from = b.start + b.indent;
        return Some(Edit { changes: vec![(from, p, before.clone())], selection: Some(Selection::point(from + before.chars().count())), ..Edit::default() });
    }
    if matches!(before.as_str(), "# " | "## " | "### ") && b.depth == 0 {
        let from = b.start + b.indent;
        return Some(Edit { changes: vec![(from, p, before.clone())], selection: Some(Selection::point(from + before.chars().count())), ..Edit::default() });
    }
    let inner = before.strip_prefix('[')?.strip_suffix("] ")?;
    let ch = match inner.chars().count() {
        0 => OPEN,
        1 => inner.chars().next().filter(|&c| is_status(c))?,
        _ => return None,
    };
    let ins = format!("[{ch}] ");
    let caret = cs + ins.chars().count();
    Some(Edit { changes: vec![(cs, p, ins)], selection: Some(Selection::point(caret)), ..Edit::default() })
}

/// The id a task's box is hit as.
pub const BOX: &str = "box";

/// A task's box in the hang, styled by its status (`thc.task.done`), hit as [`BOX`].
fn decorate(_: &Ctx, b: &BlockInfo) -> Decoration {
    match b.tag.filter(|_| is_task(b)) {
        Some(c) => Decoration {
            hang: Some(Deco { text: format!("[{c}]"), role: format!("thc.task.{}", status_name(Some(c))), id: Some(BOX.into()) }),
            gutter: None,
        },
        None => Decoration::default(),
    }
}

/// thc's task goldens (moved from caretline's outline goldens: they are this host's commands).
/// Notation as there: each block is its first line's text, `⏎` a soft break, ` ‖ ` a blank row
/// between blocks and ` ¦ ` none; `▮` the caret, `⟦…⟧` a selection. Blocks get ids 0, 1, 2…
#[cfg(test)]
mod tests {
    use super::*;
    use cn::keymap::{parse_keys, ScriptItem};
    use cn::outline::markdown;
    use cn::{update, Effect, KeyCode, State, Viewport};

    fn doc(notation: &str) -> State {
        let mut blocks: Vec<(String, bool)> = Vec::new();
        let mut rest = notation;
        let mut gap = false;
        loop {
            let (a, b) = (rest.find(" ‖ "), rest.find(" ¦ "));
            let (at, next_gap, len) = match (a, b) {
                (Some(x), Some(y)) if x < y => (x, true, " ‖ ".len()),
                (_, Some(y)) => (y, false, " ¦ ".len()),
                (Some(x), None) => (x, true, " ‖ ".len()),
                (None, None) => break,
            };
            blocks.push((rest[..at].to_string(), gap));
            gap = next_gap;
            rest = &rest[at + len..];
        }
        blocks.push((rest.to_string(), gap));
        let (mut text, mut n, mut starts) = (String::new(), 0usize, Vec::new());
        let (mut caret, mut open, mut close) = (None, None, None);
        for (k, (b, _)) in blocks.iter().enumerate() {
            if k > 0 {
                text.push('\n');
                n += 1;
            }
            starts.push(n);
            for c in b.chars() {
                match c {
                    '▮' => caret = Some(n),
                    '⟦' => open = Some(n),
                    '⟧' => close = Some(n),
                    '⏎' => {
                        text.push('\n');
                        n += 1;
                    }
                    c => {
                        text.push(c);
                        n += 1;
                    }
                }
            }
        }
        let mut s = State::new(&text, Some("t.md".into()), Viewport { width: 80, height: 24 });
        for &p in &starts {
            s.doc.marks.mint(p);
        }
        s.doc.set_host(host());
        s.enable_outline(config());
        let o = s.blocks().unwrap();
        for (k, (_, want)) in blocks.iter().enumerate().skip(1) {
            let b = o.get(MarkId(k as u64)).unwrap();
            if b.gap != *want {
                s.doc.marks.set_gap(b.id, Some(*want));
            }
        }
        s.outline_changed();
        let head = caret.expect("notation needs a caret ▮");
        let anchor = match (open, close) {
            (Some(o), Some(c)) => {
                if head == o {
                    c
                } else {
                    o
                }
            }
            _ => head,
        };
        s.view.selection = Selection::single(anchor, head);
        update(&mut s, Msg::resize(80, 24));
        s
    }

    fn show(s: &State) -> String {
        let o = s.blocks().unwrap();
        let r = s.view.selection.primary();
        let chars: Vec<char> = s.doc.text.chars().collect();
        let mut out = String::new();
        let mark = |out: &mut String, i: usize| {
            if r.anchor == r.head {
                if i == r.head {
                    out.push('▮');
                }
                return;
            }
            if i == r.from() {
                out.push('⟦');
                if r.head == i {
                    out.push('▮');
                }
            }
            if i == r.to() {
                if r.head == i {
                    out.push('▮');
                }
                out.push('⟧');
            }
        };
        for (k, b) in o.blocks.iter().enumerate() {
            if k > 0 {
                out.push_str(if b.gap { " ‖ " } else { " ¦ " });
            }
            for (i, &c) in chars.iter().enumerate().take(b.end).skip(b.start) {
                mark(&mut out, i);
                out.push(if c == '\n' { '⏎' } else { c });
            }
            mark(&mut out, b.end);
        }
        out
    }

    fn ids(s: &State) -> Vec<u64> {
        s.blocks().unwrap().blocks.iter().map(|b| b.id.0).collect()
    }

    /// Keys through caretline's outline keymap, with thc's ⌃T on top.
    fn keys(s: &mut State, script: &str) -> Vec<Effect> {
        let mut fx = Vec::new();
        for item in parse_keys(script).unwrap() {
            let ScriptItem::Key(k) = item else { continue };
            let msg = if k.code == KeyCode::Char('t') && k.mods.ctrl { Some(Msg::Command { name: TASK_CYCLE.into(), args: Value::Null }) } else { cn::keymap_for(true, &k) };
            if let Some(m) = msg {
                fx.extend(update(s, m));
            }
        }
        fx
    }

    #[track_caller]
    fn golden(before: &str, script: &str, after: &str) -> State {
        let mut s = doc(before);
        keys(&mut s, script);
        assert_eq!(show(&s), after, "{before:?} · {script:?}");
        s
    }

    fn completed(fx: &[Effect]) -> Vec<u64> {
        fx.iter().filter_map(|e| match e {
            Effect::Host { name, data } if name == COMPLETED => data["id"].as_u64(),
            _ => None,
        })
        .collect()
    }

    #[test]
    fn e23_the_task_cycle_applies_to_every_selected_block() {
        golden("o⟦ne ‖ tw▮⟧o", "<c-t>", "- [ ] o⟦ne ‖ - [ ] tw▮⟧o");
    }

    /// `[ ] `, `[x] ` or `[] ` typed at the start of a paragraph's line is a task: the box gets
    /// its list marker, and on a later line the task is a block of its own.
    #[test]
    fn a_bare_task_box_typed_at_a_line_start_makes_a_task() {
        golden("▮", "[ ] call", "- [ ] call▮");
        golden("▮", "[x] paid", "- [x] paid▮");
        golden("▮", "[] call", "- [ ] call▮");
        golden("intro⏎▮", "[ ] call", "intro ‖ - [ ] call▮");
        golden("say ▮", "[ ] hi", "say [ ] hi▮");
        golden("▮", "[q] hi", "[q] hi▮");
    }

    #[test]
    fn e70_the_task_cycle_inside_a_paragraph_splits_out_that_line() {
        let s = golden("First line⏎sec▮ond line⏎third line", "<c-t>", "First line ¦ - [ ] sec▮ond line ¦ third line");
        assert_eq!(ids(&s), [0, 1, 2]);
    }

    #[test]
    fn e71_on_the_first_line_the_task_keeps_the_id() {
        let s = golden("fir▮st⏎second", "<c-t>", "- [ ] fir▮st ¦ second");
        assert_eq!(ids(&s), [0, 1]);
    }

    #[test]
    fn e72_each_selected_line_becomes_its_own_task() {
        golden("a⏎⟦b⏎c▮⟧⏎d", "<c-t>", "a ¦ - [ ] ⟦b ¦ - [ ] c▮⟧ ¦ d");
    }

    #[test]
    fn e73_a_list_item_with_a_soft_break_stays_one_item() {
        golden("- item one⏎more of it▮", "<c-t>", "- [ ] item one⏎more of it▮");
    }

    #[test]
    fn e74_a_task_after_a_task_keeps_its_blank_row() {
        golden("- [ ] Buy milk ‖ Call ▮the bank", "<c-t>", "- [ ] Buy milk ‖ - [ ] Call ▮the bank");
    }

    #[test]
    fn e75_back_to_text_adds_no_blank_row() {
        golden("- [ ] A ¦ - [ ] B▮", "<c-t><c-t>", "- [ ] A ¦ - B▮");
    }

    #[test]
    fn e76_back_to_text_never_joins_across_a_blank_row() {
        // Back to text is a bullet note (the Logseq model): no join across the blank row.
        let s = golden("Para one ‖ - [ ] Ta▮sk", "<c-t><c-t>", "Para one ‖ - Ta▮sk");
        assert_eq!(ids(&s), [0, 1]);
    }

    #[test]
    fn e77_tab_joins_the_note_above() {
        // tta6t: the blank row goes with the nesting, in one undo step; ⇧Tab adds none back.
        let mut s = golden("- [ ] A ‖ - [ ] B▮", "<tab>", "- [ ] A ¦   - [ ] B▮");
        keys(&mut s, "<s-tab>");
        assert_eq!(show(&s), "- [ ] A ¦ - [ ] B▮");
        keys(&mut s, "<c-z><c-z>");
        assert_eq!(show(&s), "- [ ] A ‖ - [ ] B▮");
    }

    #[test]
    fn e78_undo_puts_the_paragraph_back() {
        let mut s = golden("First line⏎sec▮ond line⏎third line", "<c-t><c-z>", "First line⏎sec▮ond line⏎third line");
        assert_eq!(ids(&s), [0]);
        keys(&mut s, "<c-y>");
        assert_eq!(show(&s), "First line ¦ - [ ] sec▮ond line ¦ third line");
        assert_eq!(ids(&s), [0, 1, 2], "redo brings back the same ids");
    }

    #[test]
    fn e79_deleting_the_marker_adds_no_blank_row() {
        // ⌫ takes the box off; the first note stays a bullet (the Logseq model: never a paragraph).
        golden("- [ ] ▮A ¦ - [ ] B", "<bs><bs><bs><bs>", "- ▮A ¦ - [ ] B");
    }

    #[test]
    fn e80_blank_rows_round_trip_through_markdown() {
        let s = doc("- [ ] A ‖ - [ ] B▮");
        let md = markdown::to_file(&s);
        assert_eq!(md, "- [ ] A\n\n- [ ] B\n");
        let back = markdown::load(&md, None, Viewport { width: 80, height: 24 }, config());
        let gaps: Vec<bool> = back.blocks().unwrap().blocks.iter().map(|b| b.gap).collect();
        assert_eq!(gaps, [false, true]);
    }

    #[test]
    fn e81_back_to_text_joins_the_lines_it_split_from() {
        let s = golden("First line⏎sec▮ond line⏎third line", "<c-t><c-t><c-t>", "First line⏎sec▮ond line⏎third line");
        assert_eq!(ids(&s), [0]);
    }

    /// Enter starts a new note, ⇧Enter breaks the line (the Logseq model, writing.md §1).
    #[test]
    fn enter_starts_a_note_and_shift_enter_breaks_the_line() {
        golden("- one▮", "<cr>", "- one ¦ - ▮");
        golden("- on▮e", "<cr>", "- on ¦ - ▮e");
        golden("- one▮", "<s-cr>", "- one⏎▮");
        golden("- one▮", "<c-j>", "- one⏎▮");
        golden("- ▮one", "<cr>", "-  ¦ - ▮one");
        // A note with children: the new note is the first child.
        golden("- A▮ ¦   - A1", "<cr>", "- A ¦   - ▮ ¦   - A1");
        golden("- [ ] A▮ ¦   - A1", "<cr>", "- [ ] A ¦   - [ ] ▮ ¦   - A1");
        // A task's next note is a task; an empty one at the top ends the checklist.
        golden("- [ ] A▮", "<cr>", "- [ ] A ¦ - [ ] ▮");
        golden("- [ ] A ¦ - [ ] ▮", "<cr>", "- [ ] A ¦ - ▮");
        // A heading: the next note is a bullet.
        golden("# Title▮", "<cr>", "# Title ‖ - ▮");
    }

    /// Paragraphs (imported Markdown) keep working: Enter splits them into notes, at a line's
    /// end the new note is a bullet; ⇧Enter still breaks the line.
    #[test]
    fn enter_in_a_paragraph_starts_a_note() {
        let s = golden("Para one▮", "<cr>", "Para one ‖ - ▮");
        assert_eq!(ids(&s), [0, 1]);
        let s = golden("Para ▮one", "<cr>", "Para  ‖ ▮one");
        assert_eq!(ids(&s), [0, 1]);
        golden("Para one▮⏎two", "<cr>", "Para one ‖ - ▮ ‖ two");
        golden("▮Para one", "<cr>", "-  ‖ ▮Para one");
        golden("Para one▮", "<s-cr>", "Para one⏎▮");
    }

    /// Markdown typed at a bullet's start: a box is a task, `- ` is the bullet it is already,
    /// `# ` a heading.
    #[test]
    fn markdown_typed_on_a_bullet() {
        let mut s = doc("- ▮");
        for t in ["[", " ", "]", " "] {
            update(&mut s, Msg::InsertText { text: t.into() });
        }
        assert_eq!(show(&s), "- [ ] ▮");
        let mut s = doc("- ▮");
        for t in ["[", "]", " "] {
            update(&mut s, Msg::InsertText { text: t.into() });
        }
        assert_eq!(show(&s), "- [ ] ▮");
        let mut s = doc("- ▮");
        for t in ["-", " ", "x"] {
            update(&mut s, Msg::InsertText { text: t.into() });
        }
        assert_eq!(show(&s), "- x▮");
        let mut s = doc("- ▮");
        for t in ["#", " ", "T"] {
            update(&mut s, Msg::InsertText { text: t.into() });
        }
        assert_eq!(show(&s), "# T▮");
        let mut s = doc("- ▮");
        for t in ["1", ".", " ", "a"] {
            update(&mut s, Msg::InsertText { text: t.into() });
        }
        assert_eq!(show(&s), "1. a▮");
        keys(&mut s, "<cr>");
        assert_eq!(show(&s), "1. a ¦ 2. ▮");
    }

    /// ⌫ at a bullet's start joins the note above (Logseq), its children with it; never a
    /// paragraph.
    #[test]
    fn backspace_at_a_bullets_start_joins_the_note_above() {
        let s = golden("- one ¦ - ▮two", "<bs>", "- one▮two");
        assert_eq!(ids(&s), [0]);
        golden("- one ¦ - ▮", "<bs>", "- one▮");
        golden("- ▮one", "<bs>", "- ▮one");
        golden("- A ¦   - ▮B ¦     - C", "<bs>", "- A▮B ¦   - C");
        golden("- [ ] A ¦ - [ ] ▮B", "<bs>", "- [ ] A ¦ - ▮B");
    }

    #[test]
    fn the_task_cycle_goes_text_open_done_text() {
        let mut s = doc("Call ▮Sam");
        keys(&mut s, "<c-t>");
        assert_eq!(show(&s), "- [ ] Call ▮Sam");
        let fx = keys(&mut s, "<c-t>");
        assert_eq!(show(&s), "- [x] Call ▮Sam");
        assert_eq!(completed(&fx), [0]);
        keys(&mut s, "<c-t>");
        assert_eq!(show(&s), "- Call ▮Sam", "back to text is a bullet note");
        let s = golden("Before⏎Call ▮Sam⏎After", "<c-t><c-t><c-t>", "Before⏎Call ▮Sam⏎After");
        assert_eq!(ids(&s), [0], "a task split from a paragraph joins it again as text");
        golden("- [x] Call ▮Sam", "<c-t>", "- Call ▮Sam");
        golden("- bul▮let", "<c-t>", "- [ ] bul▮let");
        golden("  - [/] doing▮", "<c-t>", "  - [x] doing▮");
    }

    #[test]
    fn a_click_on_the_box_sets_the_status_and_never_makes_text() {
        let set = |id: u64, st: &str| Msg::Command { name: SET_STATUS.into(), args: json!({ "id": id, "status": st }) };
        let mut s = doc("- [ ] Pay▮ ¦ - [x] Done");
        let fx = update(&mut s, set(0, "x"));
        assert_eq!(show(&s), "- [x] Pay▮ ¦ - [x] Done");
        assert_eq!(completed(&fx), [0]);
        update(&mut s, set(1, " "));
        assert_eq!(show(&s), "- [x] Pay▮ ¦ - [ ] Done");
        update(&mut s, set(1, "q"));
        assert_eq!(s.view.status.as_deref(), Some("'q' isn't a task state"));
    }

    #[test]
    fn enter_after_a_task_opens_one_and_backspace_steps_back() {
        golden("- [x] done▮", "<cr>", "- [x] done ¦ - [ ] ▮");
        golden("- [ ] ▮task", "<bs>", "- ▮task");
        golden("  - [x] ▮deep", "<bs><bs>", "  - ▮deep");
    }

    #[test]
    fn a_trace_with_task_commands_replays_with_thcs_host() {
        let mut session = cn::Session::new(doc("Call ▮Sam"));
        session.apply(Msg::Command { name: TASK_CYCLE.into(), args: Value::Null });
        session.apply(Msg::Command { name: TASK_CYCLE.into(), args: Value::Null });
        let (replayed, _, _) = cn::trace::replay_trace_with(&session.trace_jsonl(), &host()).unwrap();
        assert_eq!(replayed.doc.text.to_string(), "- [x] Call Sam");
    }

    #[test]
    fn the_box_is_a_decoration() {
        let mut s = doc("- [x] Pay▮ ¦ plain");
        s.view.config.status_bar = false;
        s.view.layout = Some(cn::OutlineLayout::default());
        let f = cn::view(&s);
        let row = f.to_text().lines().next().unwrap().to_string();
        assert!(row.starts_with("  [x] Pay"), "{row:?}");
        let x = (0..f.width).find(|&x| &*f.cell(x, 0).symbol == "[").unwrap();
        assert_eq!(f.role_name(f.cell(x, 0).role), "thc.task.done");
        assert_eq!(cn::view::hit(&s.doc, &s.view, x, 0), cn::view::Hit::Hang { block: MarkId(0), deco: Some("box".into()) });
    }
}
