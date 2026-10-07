//! An agent's own view on the open document (caretline's per-client views, sidebar.md §7.1).
//!
//! `doc.view.open` gives the agent a caretline `View` of its own on the document the person has
//! open: its own caret and selection, laid out like the main view. `doc.msgs` runs caretline
//! messages through it (`insert_text`, `insert_newline`, `move`…) and `doc.text.set` replaces a
//! note's text through it. The edits are ordinary engine edits, so every other view (the
//! person's caret, a sidebar panel on the same page) is mapped through them and stays where it
//! was, and they save through thc's normal save path (`save_doc`), as the person's own lines
//! do. Each op is one `doc_view` message, so traces record and replay it.

use crate::app::App;
use crate::editor::{BlockPos, ViewId};
use serde_json::{Value, json};

/// thc's ops for agents' document views.
pub const OPS: &[&str] = &[
    "doc.view.open",
    "doc.view.close",
    "doc.msgs",
    "doc.text.set",
];

/// Agents' view ids start here (the main view is 0, sidebar panels count up from 1).
const BASE: ViewId = 1_000;

/// Messages an agent may not send through its view: the runtime's own, and the clipboard's.
const REFUSED: &[&str] = &[
    "quit",
    "save",
    "saved",
    "save_failed",
    "resize",
    "tick",
    "frame",
    "frame_clock",
    "copy",
    "cut",
    "paste",
    "paste_plain",
    "show_status",
    "external",
];

fn actor_of(actor: Option<&str>, req: &Value) -> String {
    actor
        .map(str::to_string)
        .or_else(|| req.get("actor").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| "agent".into())
}

fn msgs_of(req: &Value) -> Result<Vec<caretline::Msg>, String> {
    let raw = req
        .get("msgs")
        .and_then(Value::as_array)
        .ok_or("doc.msgs needs msgs: [caretline messages]")?;
    raw.iter()
        .map(|m| {
            let name = m.get("msg").and_then(Value::as_str).unwrap_or("");
            if REFUSED.contains(&name) {
                return Err(format!("doc.msgs: {name:?} isn't an agent's to send"));
            }
            serde_json::from_value::<caretline::Msg>(m.clone())
                .map_err(|e| format!("doc.msgs: a message: {e}"))
        })
        .collect()
}

/// Validates a request without applying it.
pub fn check(app: &App, req: &Value, _actor: Option<&str>) -> Result<(), String> {
    let op = req.get("op").and_then(Value::as_str).unwrap_or("");
    if !OPS.contains(&op) {
        return Err(format!("unknown doc op {op:?}"));
    }
    let Some(d) = app.doc.as_ref() else {
        return Err("no document is open".into());
    };
    match op {
        "doc.msgs" => msgs_of(req).map(|_| ()),
        "doc.text.set" => {
            let id = req
                .get("id")
                .and_then(Value::as_str)
                .ok_or("doc.text.set needs an id (a note on the open document)")?;
            req.get("text")
                .and_then(Value::as_str)
                .ok_or("doc.text.set needs a text")?;
            line_of(d, id).map(|_| ())
        }
        _ => Ok(()),
    }
}

/// A note by id or a unique prefix of 4 or more characters.
fn line_of(d: &crate::editor::Doc, id: &str) -> Result<usize, String> {
    if let Some(i) = d.blocks().iter().position(|l| l.id == id) {
        return Ok(i);
    }
    let m: Vec<usize> = (0..d.blocks().len())
        .filter(|&i| id.len() >= 4 && d.blocks()[i].id.starts_with(id))
        .collect();
    match m.as_slice() {
        [i] => Ok(*i),
        [] => Err(format!("no note {id} on the open document")),
        _ => Err(format!("{id} matches {} notes", m.len())),
    }
}

/// The agent's view, opened when it has none (`at`: `start`, `end`, or `{"id", "byte"}`).
fn ensure(app: &mut App, actor: &str, at: Option<&Value>) -> ViewId {
    let n = app.agent_views.len() as ViewId;
    let vid = *app.agent_views.entry(actor.to_string()).or_insert(BASE + n);
    let d = app.doc.as_mut().expect("checked");
    let fresh = !d.has_view(vid);
    if fresh {
        d.add_view(vid);
    }
    if fresh || at.is_some() {
        d.with_view(vid, |d| {
            let last = d.blocks().len().saturating_sub(1);
            let p = match at {
                Some(Value::String(s)) if s == "end" => BlockPos {
                    line: last,
                    byte: d.blocks()[last].text.len(),
                },
                Some(v @ Value::Object(_)) => {
                    let id = v.get("id").and_then(Value::as_str).unwrap_or("");
                    let line = line_of(d, id).unwrap_or(0);
                    let byte = v
                        .get("byte")
                        .and_then(Value::as_u64)
                        .map_or(0, |b| b as usize)
                        .min(d.blocks()[line].text.len());
                    BlockPos { line, byte }
                }
                _ => BlockPos { line: 0, byte: 0 },
            };
            d.set_caret(p);
        });
    }
    vid
}

/// The agent view's caret and the note it's on.
fn place(app: &mut App, vid: ViewId) -> Value {
    let d = app.doc.as_mut().expect("checked");
    d.with_view(vid, |d| {
        let c = d.caret();
        let id = d.blocks()[c.line].id.clone();
        let range = d.char_range(c.line);
        json!({"view": vid, "caret": {"id": id, "byte": c.byte}, "line": {"id": id, "text": range.map(|(f, t)| json!({"from": f, "to": t}))}})
    })
    .unwrap_or(Value::Null)
}

/// Runs one request. Its reply is the agent view's place after it.
pub fn run(app: &mut App, req: &Value, actor: Option<&str>) -> Result<Value, String> {
    check(app, req, actor)?;
    let actor = actor_of(actor, req);
    let op = req
        .get("op")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    app.clock_tick();
    match op.as_str() {
        "doc.view.open" => {
            let vid = ensure(app, &actor, req.get("at"));
            Ok(place(app, vid))
        }
        "doc.view.close" => {
            if let Some(vid) = app.agent_views.remove(&actor) {
                if let Some(d) = app.doc.as_mut() {
                    d.remove_view(vid);
                }
            }
            Ok(json!({"closed": true}))
        }
        "doc.msgs" => {
            let msgs = msgs_of(req)?;
            let vid = ensure(app, &actor, req.get("at"));
            let d = app.doc.as_mut().expect("checked");
            d.with_view(vid, |d| {
                for m in msgs {
                    d.run_msg(m);
                }
            });
            app.save_doc(false);
            Ok(place(app, vid))
        }
        "doc.text.set" => {
            let id = req["id"].as_str().unwrap_or("").to_string();
            let text = req["text"].as_str().unwrap_or("").to_string();
            let vid = ensure(app, &actor, None);
            let d = app.doc.as_mut().expect("checked");
            d.with_view(vid, |d| {
                let Ok(i) = line_of(d, &id) else { return };
                let len = d.blocks()[i].text.len();
                d.select_range(
                    Some(BlockPos { line: i, byte: 0 }),
                    BlockPos { line: i, byte: len },
                );
                if text.is_empty() {
                    d.delete_selection();
                } else {
                    d.insert(&text);
                }
            });
            app.save_doc(false);
            Ok(place(app, vid))
        }
        _ => unreachable!("checked"),
    }
}
