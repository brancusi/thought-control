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
use cn::outline::{BlockInfo, Kind, OutlineConfig};
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
    OutlineConfig {
        tags: STATUSES.iter().map(|(c, _)| *c).collect(),
        new_tag: Some(OPEN),
        task_markers: Vec::new(),
        ..OutlineConfig::default()
    }
}

/// The host every thc document runs with.
pub fn host() -> Host {
    Host::new().command(TASK_CYCLE, task_cycle).command(SET_STATUS, set_status).input_rule("thc.task_shorthand", shorthand).decorator(decorate)
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
            Target::Text => {
                if x.kind != Kind::Para {
                    changes.push(removal(x.start, x.content_start()));
                }
            }
        }
    }
    // Back to text next to a paragraph with no blank row between: it joins it (the reverse of
    // a split). Never across a blank row.
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
    })
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
    if !r.is_empty() || !typed.ends_with(' ') || typed.contains(['\n', '\r']) {
        return None;
    }
    let o = ctx.blocks()?;
    let text = ctx.text();
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
        0 => OPEN,
        1 => inner.chars().next().filter(|&c| is_status(c))?,
        _ => return None,
    };
    let from = ls + indent.min(p - ls);
    let ins = format!("- [{ch}] ");
    let caret = from + ins.chars().count();
    Some(Edit { changes: vec![(from, p, ins)], selection: Some(Selection::point(caret)), ..Edit::default() })
}

/// A task's box in the hang, styled by its status (`thc.task.done`), hit as `box`.
fn decorate(_: &Ctx, b: &BlockInfo) -> Decoration {
    match b.tag.filter(|_| is_task(b)) {
        Some(c) => Decoration {
            hang: Some(Deco { text: format!("[{c}]"), role: format!("thc.task.{}", status_name(Some(c))), id: Some("box".into()) }),
            gutter: None,
        },
        None => Decoration::default(),
    }
}
