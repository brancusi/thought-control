//! The tools: what an agent can do to a caretline editor, and how each maps onto the state
//! protocol. Every function here is blocking; the MCP layer runs them off the async runtime.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::engine::{self, Engine, ProtoError};
use crate::text::{self, Pos};

/// How the server was started.
#[derive(Debug, Clone)]
pub struct Config {
    /// Refuse every tool that changes a document or a file.
    pub read_only: bool,
    /// Accept `edit` without `if_rev`.
    pub allow_unguarded: bool,
    /// Show "<name>: edited line 3" in a live editor's status bar after each edit.
    pub announce: bool,
    /// The name edits are announced under (else the MCP client's name).
    pub name: Option<String>,
}

/// A tool's failure: a message, or a structured result (a stale write) the agent acts on.
#[derive(Debug)]
pub enum ToolError {
    Msg(String),
    Data(Value),
}

impl From<String> for ToolError {
    fn from(s: String) -> Self {
        ToolError::Msg(s)
    }
}

impl From<&str> for ToolError {
    fn from(s: &str) -> Self {
        ToolError::Msg(s.to_string())
    }
}

impl From<ProtoError> for ToolError {
    fn from(e: ProtoError) -> Self {
        ToolError::Msg(e.to_string())
    }
}

/// A tool's success: a JSON result and, for `read`, the text itself as a second block.
pub struct Output {
    pub data: Value,
    pub text: Option<String>,
}

impl From<Value> for Output {
    fn from(data: Value) -> Self {
        Output { data, text: None }
    }
}

type R = Result<Output, ToolError>;

/// How many texts a session remembers by rev, to tell a caret move from a text change.
const TEXTS_KEPT: usize = 64;

/// One open document: a live editor or a headless engine.
pub struct Sess {
    pub id: String,
    pub engine: Engine,
    pub file: Option<String>,
    pub pid: Option<u32>,
    /// The agent's own view: id and size.
    pub agent_view: Option<(u32, u16, u16)>,
    /// The text at each rev this session has seen.
    texts: BTreeMap<u64, Arc<str>>,
    /// The revs this session's own requests made.
    mine: BTreeSet<u64>,
}

impl Sess {
    fn remember(&mut self, rev: u64, text: &str) {
        if self.texts.get(&rev).is_some_and(|t| &**t == text) {
            return;
        }
        self.texts.insert(rev, Arc::from(text));
        while self.texts.len() > TEXTS_KEPT {
            let first = *self.texts.keys().next().unwrap();
            self.texts.remove(&first);
        }
    }

    /// Notes the revs a `msgs`/`keys` result made: one per message, ending at `rev`.
    fn mark_mine(&mut self, result: &Value) {
        let Some(rev) = result["rev"].as_u64() else { return };
        let n = result["msgs"].as_array().map(|m| m.len() as u64).unwrap_or(1).max(1);
        for r in rev.saturating_sub(n - 1)..=rev {
            self.mine.insert(r);
        }
        while self.mine.len() > 100_000 {
            let first = *self.mine.iter().next().unwrap();
            self.mine.remove(&first);
        }
    }

    /// The state without its history (view 0, or another view), remembered by rev.
    fn state(&mut self, view: u32) -> Result<(u64, String, Value), ToolError> {
        let r = self.engine.call(json!({"op": "state.get", "history": false, "view": view}))?;
        let rev = r["rev"].as_u64().unwrap_or(0);
        let text = r["state"]["text"].as_str().unwrap_or("").to_string();
        self.remember(rev, &text);
        Ok((rev, text, r["state"].clone()))
    }

    fn who(&self, event: &Value) -> &'static str {
        if event["rev"].as_u64().is_some_and(|r| self.mine.contains(&r)) {
            return "this agent";
        }
        match event["source"].as_str() {
            Some("terminal") => "person",
            Some("client") => "another client",
            Some("runtime") => "editor",
            _ => "unknown",
        }
    }

    /// The view an agent's caret works through: its own view, or view 0 in a headless engine.
    fn caret_view(&self) -> Result<u32, ToolError> {
        match (self.agent_view, self.engine.is_live()) {
            (Some((v, _, _)), _) => Ok(v),
            (None, false) => Ok(0),
            (None, true) => Err("keys and select move a caret, and view 0 is the person's: call view_open first, so the agent works with its own caret".into()),
        }
    }
}

/// Messages that change nothing anyone would notice: the clock, frames, resizes.
fn noise(msg: &Value) -> bool {
    matches!(msg["msg"].as_str(), Some("tick" | "frame" | "frame_clock" | "resize"))
}

/// An event with nothing but noise (a view opened or closed has no messages at all).
fn is_noise(event: &Value) -> bool {
    event["state_set"] != json!(true) && event["msgs"].as_array().is_some_and(|m| m.iter().all(noise))
}

/// A short account of messages: typing joined into strings, other messages by name.
fn describe(msgs: &[Value], actions: &mut Vec<String>) {
    for m in msgs {
        if noise(m) {
            continue;
        }
        let typed = match m["msg"].as_str() {
            Some("insert_text") => m["text"].as_str().map(String::from),
            Some("insert_newline") => Some("\n".into()),
            _ => None,
        };
        if let Some(t) = typed {
            if let Some(last) = actions.last_mut().filter(|a| a.starts_with("typed ")) {
                let mut s: String = serde_json::from_str(&last["typed ".len()..]).unwrap_or_default();
                s.push_str(&t);
                *last = format!("typed {}", json!(s));
            } else {
                actions.push(format!("typed {}", json!(t)));
            }
            continue;
        }
        let name = m["msg"].as_str().unwrap_or("?");
        let detail = match name {
            "move" => format!("move {} {}{}", m["dir"].as_str().unwrap_or(""), m["by"].as_str().unwrap_or(""), if m["extend"] == json!(true) { " (extend)" } else { "" }),
            "edit" => format!("edit ({} change(s))", m["changes"].as_array().map_or(0, |c| c.len())),
            "external" => format!("external change ({} change(s))", m["changes"].as_array().map_or(0, |c| c.len())),
            "show_status" => format!("status {}", m["text"]),
            "paste" => "paste".into(),
            _ => name.to_string(),
        };
        actions.push(detail);
    }
}

pub struct Tools {
    pub config: Config,
    sessions: Mutex<HashMap<String, Arc<Mutex<Sess>>>>,
    next: Mutex<u64>,
    /// The MCP client's name, for announcements.
    pub client_name: Mutex<Option<String>>,
}

fn arg<'a>(args: &'a Map<String, Value>, k: &str) -> Option<&'a Value> {
    args.get(k).filter(|v| !v.is_null())
}

fn arg_str<'a>(args: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    arg(args, k).and_then(|v| v.as_str())
}

fn arg_u64(args: &Map<String, Value>, k: &str) -> Result<Option<u64>, ToolError> {
    match arg(args, k) {
        None => Ok(None),
        Some(v) => v
            .as_u64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            .map(Some)
            .ok_or_else(|| ToolError::Msg(format!("{k} must be a non-negative integer"))),
    }
}

fn arg_bool(args: &Map<String, Value>, k: &str) -> bool {
    arg(args, k).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn parse_pos(v: Option<&Value>, what: &str) -> Result<Pos, ToolError> {
    let v = v.ok_or_else(|| ToolError::Msg(format!("{what} is required: {{\"line\": 1, \"col\": 1}}")))?;
    if let Some(s) = v.as_str() {
        let (l, c) = s.split_once(':').ok_or_else(|| ToolError::Msg(format!("{what}: write {{\"line\": L, \"col\": C}} or \"L:C\"")))?;
        let line = l.trim().parse().map_err(|_| ToolError::Msg(format!("{what}: bad line {l:?}")))?;
        let col = c.trim().parse().map_err(|_| ToolError::Msg(format!("{what}: bad col {c:?}")))?;
        return Ok(Pos { line, col });
    }
    let line = v["line"].as_u64().ok_or_else(|| ToolError::Msg(format!("{what}.line is required (1-based)")))? as usize;
    let col = v["col"].as_u64().unwrap_or(1) as usize;
    Ok(Pos { line, col })
}

/// A selection described for an agent: the caret and, when something is selected, its ends
/// and text.
fn describe_selection(text: &str, state: &Value) -> Value {
    let ranges = state["selection"]["ranges"].as_array().cloned().unwrap_or_default();
    let primary = state["selection"]["primary_index"].as_u64().unwrap_or(0) as usize;
    let Some(r) = ranges.get(primary).or(ranges.first()) else { return Value::Null };
    let (a, h) = (r["anchor"].as_u64().unwrap_or(0) as usize, r["head"].as_u64().unwrap_or(0) as usize);
    let mut out = json!({ "caret": text::to_pos(text, h).json() });
    if a != h {
        let (from, to) = (a.min(h), a.max(h));
        let sel: String = text.chars().skip(from).take(to - from).collect();
        let shown: String = sel.chars().take(200).collect();
        out["selection"] = json!({
            "from": text::to_pos(text, from).json(),
            "to": text::to_pos(text, to).json(),
            "chars": to - from,
            "text": if shown.len() < sel.len() { format!("{shown}…") } else { shown },
        });
    }
    if ranges.len() > 1 {
        out["ranges"] = json!(ranges.len());
    }
    out
}

impl Tools {
    pub fn new(config: Config) -> Tools {
        Tools { config, sessions: Mutex::new(HashMap::new()), next: Mutex::new(1), client_name: Mutex::new(None) }
    }

    fn name(&self) -> String {
        self.config.name.clone().or_else(|| self.client_name.lock().unwrap().clone()).unwrap_or_else(|| "agent".into())
    }

    fn session(&self, args: &Map<String, Value>) -> Result<Arc<Mutex<Sess>>, ToolError> {
        let sessions = self.sessions.lock().unwrap();
        let id = match arg_str(args, "session") {
            Some(id) => id.to_string(),
            None if sessions.len() == 1 => sessions.keys().next().unwrap().clone(),
            None if sessions.is_empty() => return Err("no open session: call open first".into()),
            None => return Err(format!("several sessions are open ({}): pass session", sessions.keys().cloned().collect::<Vec<_>>().join(", ")).into()),
        };
        sessions.get(&id).cloned().ok_or_else(|| ToolError::Msg(format!("no session {id:?}: call open, or list sessions with open's result")))
    }

    fn writable(&self, what: &str) -> Result<(), ToolError> {
        if self.config.read_only {
            return Err(format!("this server is read-only (--read-only): {what} is refused").into());
        }
        Ok(())
    }

    /// Runs a tool by name.
    pub fn call(&self, name: &str, args: &Map<String, Value>) -> R {
        match name {
            "list_editors" => Ok(self.list_editors()),
            "open" => self.open(args),
            "close" => self.close(args),
            "read" => self.read(args),
            "edit" => self.edit(args),
            "view_open" => self.view_open(args),
            "view_close" => self.view_close(args),
            "watch" => self.watch(args),
            "trace" => self.trace(args),
            "save" => self.save(args),
            _ => Err(format!("no tool {name:?}").into()),
        }
    }

    fn list_editors(&self) -> Output {
        let editors: Vec<Value> = engine::list_editors().iter().map(|e| e.json()).collect();
        let sessions: Vec<Value> = {
            let s = self.sessions.lock().unwrap();
            let mut v: Vec<Value> = s
                .values()
                .map(|s| {
                    let s = s.lock().unwrap();
                    json!({"session": s.id, "live": s.engine.is_live(), "file": s.file, "pid": s.pid})
                })
                .collect();
            v.sort_by_key(|x| x["session"].as_str().unwrap_or("").to_string());
            v
        };
        json!({
            "editors": editors,
            "sessions": sessions,
            "discovery_dir": engine::discovery_dir(),
            "hint": if editors.is_empty() { "No live editor. A person starts one with: caretline FILE --listen. Or open a file headless with open {file}." } else { "Attach with open {pid} (or open {} for the newest)." },
        })
        .into()
    }

    fn open(&self, args: &Map<String, Value>) -> R {
        let width = arg_u64(args, "width")?.unwrap_or(80).clamp(1, 1000) as u16;
        let height = arg_u64(args, "height")?.unwrap_or(24).clamp(1, 1000) as u16;
        let outline = arg_bool(args, "outline") || arg_bool(args, "layout");
        let (engine, file, pid) = if let Some(path) = arg_str(args, "file") {
            let abs = std::path::absolute(path).map_err(|e| format!("{path}: {e}"))?;
            let body = match std::fs::read_to_string(&abs) {
                Ok(t) => t,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => return Err(format!("{}: {e}", abs.display()).into()),
            };
            let state = new_state(&body, Some(abs.display().to_string()), width, height, outline, arg_bool(args, "layout"));
            (Engine::headless(state, Some(abs.clone())), Some(abs.display().to_string()), None)
        } else if let Some(body) = arg_str(args, "text") {
            let state = new_state(body, None, width, height, outline, arg_bool(args, "layout"));
            (Engine::headless(state, None), None, None)
        } else {
            let editors = engine::list_editors();
            let target = if let Some(sock) = arg_str(args, "socket") {
                engine::Editor { pid: 0, socket: PathBuf::from(sock), file: None, started_ms: 0, alive: true }
            } else if let Some(pid) = arg_u64(args, "pid")? {
                editors
                    .into_iter()
                    .find(|e| e.pid as u64 == pid)
                    .ok_or_else(|| ToolError::Msg(format!("no editor with pid {pid} in {}: list_editors shows the live ones", engine::discovery_dir().display())))?
            } else {
                editors.into_iter().find(|e| e.alive).ok_or_else(|| {
                    ToolError::Msg("no live caretline editor found. A person starts one with `caretline FILE --listen`; or pass file or text to open a headless engine".into())
                })?
            };
            let engine = Engine::attach(&target.socket)?;
            (engine, target.file, (target.pid != 0).then_some(target.pid))
        };
        let id = {
            let mut n = self.next.lock().unwrap();
            let id = format!("s{n}");
            *n += 1;
            id
        };
        let mut sess = Sess { id: id.clone(), engine, file: file.clone(), pid, agent_view: None, texts: BTreeMap::new(), mine: BTreeSet::new() };
        let hello = sess.engine.call(json!({"op": "hello"}))?;
        let (rev, body, state) = sess.state(0)?;
        let live = sess.engine.is_live();
        let socket = sess.engine.socket().map(|p| p.display().to_string());
        self.sessions.lock().unwrap().insert(id.clone(), Arc::new(Mutex::new(sess)));
        Ok(json!({
            "session": id,
            "live": live,
            "file": file,
            "pid": pid,
            "socket": socket,
            "proto": hello["proto"],
            "engine_version": hello["version"],
            "rev": rev,
            "lines": text::line_count(&body),
            "dirty": state["dirty"],
            "read_only": self.config.read_only,
            "next": if live { "read the text, then view_open before keys or select so the person's caret stays theirs; edit with if_rev from read" } else { "read, then edit with if_rev from read" },
        })
        .into())
    }

    fn close(&self, args: &Map<String, Value>) -> R {
        let s = self.session(args)?;
        let mut s = s.lock().unwrap();
        if let Some((v, _, _)) = s.agent_view.take()
            && let Ok(r) = s.engine.call(json!({"op": "view.close", "view": v})) {
                s.mark_mine(&r);
            }
        let id = s.id.clone();
        drop(s);
        self.sessions.lock().unwrap().remove(&id);
        Ok(json!({"closed": id}).into())
    }

    fn read(&self, args: &Map<String, Value>) -> R {
        let s = self.session(args)?;
        let mut s = s.lock().unwrap();
        let (rev, body, state) = s.state(0)?;
        let total = text::line_count(&body);
        let from = arg_u64(args, "from_line")?.unwrap_or(1) as usize;
        let to = arg_u64(args, "to_line")?.map(|t| t as usize).unwrap_or(total);
        let (from, to, part) = text::slice_lines(&body, from, to);
        let mut carets = json!({ "person": describe_selection(&body, &state) });
        if !s.engine.is_live() {
            carets = json!({ "engine": describe_selection(&body, &state) });
        }
        if let Some((v, _, _)) = s.agent_view {
            let (_, _, vs) = s.state(v)?;
            carets["agent"] = describe_selection(&body, &vs);
        }
        let mut data = json!({
            "session": s.id,
            "rev": rev,
            "file": s.file,
            "lines": total,
            "from_line": from,
            "to_line": to,
            "dirty": state["dirty"],
            "carets": carets,
        });
        let numbered = arg(args, "numbered").and_then(|v| v.as_bool()).unwrap_or(true);
        let mut shown = if numbered { text::numbered(&part, from) } else { part.clone() };
        if let Some(r) = arg(args, "render").filter(|r| r != &&json!(false)) {
            let w = r["width"].as_u64().or_else(|| arg_u64(args, "width").ok().flatten()).unwrap_or(80).clamp(1, 1000);
            let h = r["height"].as_u64().or_else(|| arg_u64(args, "height").ok().flatten()).unwrap_or(24).clamp(1, 1000);
            let view = match arg_str(args, "view") {
                Some("person") => 0,
                Some("agent") => s.agent_view.map(|v| v.0).ok_or("no agent view: call view_open")?,
                _ => s.agent_view.map(|v| v.0).unwrap_or(0),
            };
            let f = s.engine.call(json!({"op": "render", "w": w, "h": h, "view": view}))?;
            data["render"] = json!({"view": view, "width": w, "height": h, "cursor": f["cursor"]});
            shown = f["frame"].as_str().unwrap_or("").to_string();
            data["shown"] = json!("the rendered frame (not the raw text)");
        } else {
            data["text"] = json!(part);
            data["shown"] = json!(if numbered { "lines prefixed with their numbers: `N│ ` is not part of the text" } else { "the raw text" });
        }
        Ok(Output { data, text: Some(shown) })
    }

    /// Checks `if_rev` against the document: a write goes ahead when the text is the one the
    /// agent read at `if_rev`, even if carets moved or the clock ticked since.
    fn guard(&self, s: &mut Sess, if_rev: Option<u64>) -> Result<(u64, String, Value), ToolError> {
        let (rev, body, state) = s.state(0)?;
        let Some(want) = if_rev else {
            if self.config.allow_unguarded {
                return Ok((rev, body, state));
            }
            return Err("if_rev is required: call read and pass its rev, so the edit can't land on text you haven't seen".into());
        };
        if want == rev {
            return Ok((rev, body, state));
        }
        let base = s.texts.get(&want).cloned();
        if base.as_deref() == Some(body.as_str()) {
            return Ok((rev, body, state));
        }
        // Who changed it, from the event log.
        let mut by: BTreeSet<&str> = BTreeSet::new();
        {
            let events = s.engine.events.clone();
            let log = events.snapshot();
            for e in log.events.iter().filter(|e| e["rev"].as_u64().is_some_and(|r| r > want && r <= rev)) {
                if !is_noise(e) {
                    by.insert(s.who(e));
                }
            }
        }
        let diff = base.as_deref().and_then(|b| text::diff(b, &body, 8));
        Err(ToolError::Data(json!({
            "error": "stale",
            "message": match &base {
                Some(_) => format!("the text changed since rev {want}; nothing was written. Read again (now rev {rev}) and redo the edit against the new text"),
                None => format!("rev {want} isn't one this session read, and the document is now at rev {rev}; nothing was written. Read again and pass its rev"),
            },
            "if_rev": want,
            "rev": rev,
            "changed_by": by,
            "diff": diff,
        })))
    }

    fn edit(&self, args: &Map<String, Value>) -> R {
        self.writable("edit")?;
        let s = self.session(args)?;
        let mut s = s.lock().unwrap();
        let if_rev = arg_u64(args, "if_rev")?;
        let mut ops: Vec<Value> = match arg(args, "ops") {
            Some(Value::Array(a)) => a.clone(),
            Some(v @ Value::Object(_)) => vec![v.clone()],
            Some(_) => return Err("ops must be an array of edit operations".into()),
            None => Vec::new(),
        };
        // A single op may be given inline, without `ops`.
        if ops.is_empty() && ["search", "at", "keys", "from"].iter().any(|k| args.contains_key(*k)) {
            ops.push(Value::Object(args.clone()));
        }
        if ops.is_empty() {
            return Err("ops is empty: give at least one operation".into());
        }
        let kinds: Vec<String> = ops.iter().map(op_kind).collect::<Result<_, _>>()?;
        let caret_ops = kinds.iter().filter(|k| *k == "keys" || *k == "select").count();
        if caret_ops > 0 && caret_ops != kinds.len() {
            return Err("text operations (replace, replace_range, insert) and caret operations (select, keys) can't be mixed in one edit: send them as two edits".into());
        }
        let shared = arg_str(args, "undo").unwrap_or("shared");
        if !matches!(shared, "shared" | "outside") {
            return Err("undo must be \"shared\" or \"outside\"".into());
        }
        for attempt in 0..4 {
            let (rev, body, state) = self.guard(&mut s, if_rev)?;
            let result = if caret_ops > 0 {
                self.caret_ops(&mut s, rev, &body, &ops, &kinds)
            } else {
                self.text_ops(&mut s, rev, &body, &state, &ops, &kinds, shared == "outside")
            };
            match result {
                // The document moved between the check and the write: check again.
                Err(ToolError::Msg(m)) if m.starts_with("stale:") && attempt < 3 => continue,
                Err(e) => return Err(e),
                Ok(mut out) => {
                    let (new_rev, new_body, new_state) = s.state(0)?;
                    if new_body != body && s.engine.is_live() && self.config.announce {
                        let line = text::diff(&body, &new_body, 1).and_then(|d| d["first_line"].as_u64()).unwrap_or(1);
                        let status = format!("{}: edited line {line}", self.name());
                        if let Ok(r) = s.engine.call(json!({"op": "msgs", "view": 0, "msgs": [{"msg": "show_status", "text": status}]})) {
                            s.mark_mine(&r);
                        }
                    }
                    let (new_rev, new_state) = if s.engine.is_live() && self.config.announce && new_body != body {
                        let (r, _, st) = s.state(0)?;
                        (r, st)
                    } else {
                        (new_rev, new_state)
                    };
                    // If someone else changed the document right after this write, the text
                    // now isn't what this edit made: return the write's own rev, so the next
                    // guarded edit asks the agent to read first.
                    let write_rev = out.data.as_object_mut().and_then(|o| o.remove("write_rev")).and_then(|v| v.as_u64()).unwrap_or(new_rev);
                    let foreign = (write_rev + 1..=new_rev).any(|r| !s.mine.contains(&r));
                    if foreign {
                        out.data["rev"] = json!(write_rev);
                        out.data["note"] = json!(format!("the document changed again after this edit (now rev {new_rev}): the diff includes those changes. Read before the next edit"));
                    } else {
                        out.data["rev"] = json!(new_rev);
                    }
                    out.data["previous_rev"] = json!(rev);
                    out.data["dirty"] = new_state["dirty"].clone();
                    out.data["diff"] = text::diff(&body, &new_body, 12).unwrap_or(Value::Null);
                    if let Some((v, _, _)) = s.agent_view {
                        let (_, _, vs) = s.state(v)?;
                        out.data["agent"] = describe_selection(&new_body, &vs);
                    }
                    return Ok(out);
                }
            }
        }
        Err("the document kept changing while writing; read again and retry".into())
    }

    #[allow(clippy::too_many_arguments)]
    fn text_ops(&self, s: &mut Sess, rev: u64, body: &str, state: &Value, ops: &[Value], kinds: &[String], outside: bool) -> R {
        let mut changes: Vec<(usize, usize, String)> = Vec::new();
        for (op, kind) in ops.iter().zip(kinds) {
            let new_text = op.get("text").or_else(|| op.get("replace")).and_then(|v| v.as_str());
            match kind.as_str() {
                "replace" => {
                    let search = op["search"].as_str().filter(|s| !s.is_empty()).ok_or("replace needs a non-empty search")?;
                    let with = new_text.ok_or("replace needs text: what the match becomes")?;
                    let found = text::find_all(body, search);
                    match found.len() {
                        0 => {
                            let hint = if text::find_all(&body.to_lowercase(), &search.to_lowercase()).is_empty() {
                                "Read again: the text may have changed, or check whitespace and line breaks"
                            } else {
                                "It matches when case is ignored: check the case"
                            };
                            return Err(ToolError::Data(json!({"error": "not_found", "message": format!("search not found: {search:?}. {hint}"), "rev": rev})));
                        }
                        1 => changes.push((found[0].0, found[0].1, with.to_string())),
                        n => {
                            let at: Vec<Value> = found.iter().take(20).map(|(a, _)| text::to_pos(body, *a).json()).collect();
                            return Err(ToolError::Data(json!({
                                "error": "ambiguous",
                                "message": format!("search matches {n} times; include more surrounding text so it matches once, or use replace_range"),
                                "matches": at,
                                "rev": rev,
                            })));
                        }
                    }
                }
                "replace_range" => {
                    let from = text::to_char(body, parse_pos(op.get("from"), "from")?)?;
                    let to = text::to_char(body, parse_pos(op.get("to"), "to")?)?;
                    if to < from {
                        return Err("replace_range: to is before from".into());
                    }
                    changes.push((from, to, new_text.ok_or("replace_range needs text (\"\" deletes)")?.to_string()));
                }
                "insert" => {
                    let at = text::to_char(body, parse_pos(op.get("at"), "at")?)?;
                    changes.push((at, at, new_text.ok_or("insert needs text")?.to_string()));
                }
                _ => unreachable!(),
            }
        }
        changes.sort_by_key(|c| (c.0, c.1));
        if changes.windows(2).any(|w| w[0].1 > w[1].0) {
            return Err("two operations overlap: each must touch different text".into());
        }
        let crlf = state["config"]["line_ending"].as_str() == Some("CRLF");
        let fix = |t: &str| if crlf { t.replace("\r\n", "\n").replace('\n', "\r\n") } else { t.to_string() };
        let msg = if outside {
            // Applied in reverse, so each change's positions are still those of the text read.
            let ch: Vec<Value> = changes.iter().rev().map(|(a, b, t)| json!({"change": "replace", "from": a, "to": b, "text": t})).collect();
            json!({"msg": "external", "changes": ch})
        } else {
            let ch: Vec<Value> = changes.iter().map(|(a, b, t)| json!([a, b, fix(t)])).collect();
            json!({"msg": "edit", "changes": ch, "join": false})
        };
        // Through the agent's view, else the connection's own (a live editor gives each client
        // one, so the person's caret is never the acting one), else view 0 (headless).
        let mut req = json!({"op": "msgs", "msgs": [msg], "if_rev": rev});
        if let Some((v, _, _)) = s.agent_view {
            req["view"] = v.into();
        }
        let r = s.engine.call(req);
        let r = match r {
            Err(e) if e.kind == "stale" => return Err(ToolError::Msg(format!("stale: {}", e.message))),
            r => r?,
        };
        s.mark_mine(&r);
        Ok(json!({
            "ok": true,
            "write_rev": r["rev"],
            "applied": changes.len(),
            "undo": if outside { "outside: the person's undo skips this change, and it doesn't make the document unsaved" } else { "shared: one undo step in the document's history" },
            "effects": r["effects"],
        })
        .into())
    }

    fn caret_ops(&self, s: &mut Sess, rev: u64, body: &str, ops: &[Value], kinds: &[String]) -> R {
        let view = s.caret_view()?;
        let mut rev = rev;
        let mut out = json!({"ok": true});
        for (op, kind) in ops.iter().zip(kinds) {
            match kind.as_str() {
                "select" => {
                    let from = text::to_char(body, parse_pos(op.get("from"), "from")?)?;
                    let to = match op.get("to") {
                        Some(v) if !v.is_null() => text::to_char(body, parse_pos(Some(v), "to")?)?,
                        _ => from,
                    };
                    rev = self.select(s, rev, view, from, to)?;
                }
                "keys" => {
                    let keys = op["keys"].as_str().ok_or("keys needs a key script, e.g. \"<down><end>!\"")?;
                    let view = s.agent_view.map(|v| v.0).unwrap_or(view);
                    let r = match s.engine.call(json!({"op": "keys", "keys": keys, "if_rev": rev, "view": view})) {
                        Err(e) if e.kind == "stale" => return Err(ToolError::Msg(format!("stale: {}", e.message))),
                        r => r?,
                    };
                    s.mark_mine(&r);
                    rev = r["rev"].as_u64().unwrap_or(rev);
                    let mut actions = Vec::new();
                    describe(r["msgs"].as_array().map(|v| v.as_slice()).unwrap_or(&[]), &mut actions);
                    out["actions"] = json!(actions);
                    if r["effects"].as_array().is_some_and(|e| !e.is_empty()) {
                        out["effects"] = r["effects"].clone();
                        out["effects_note"] = json!("effects are returned, not performed: nothing was written or quit. Use save to write the file");
                    }
                }
                _ => unreachable!(),
            }
        }
        out["write_rev"] = json!(rev);
        Ok(out.into())
    }

    /// Puts the caret view's selection at `[from, to)`: the agent's view is reopened with it
    /// (views have no message for that); a headless engine's own view gets it by state.set.
    fn select(&self, s: &mut Sess, rev: u64, view: u32, from: usize, to: usize) -> Result<u64, ToolError> {
        let sel = json!({"ranges": [{"anchor": from, "head": to}]});
        if let Some((v, w, h)) = s.agent_view {
            let open = json!({"selection": sel, "focused": true, "read_only": self.config.read_only});
            let r = s.engine.call(json!({"op": "view.open", "open": open, "w": w, "h": h}))?;
            s.mark_mine(&r);
            let nv = r["view"].as_u64().ok_or("view.open returned no view")? as u32;
            s.agent_view = Some((nv, w, h));
            let r = s.engine.call(json!({"op": "view.close", "view": v}))?;
            s.mark_mine(&r);
            return Ok(r["rev"].as_u64().unwrap_or(rev));
        }
        debug_assert_eq!(view, 0);
        let mut full = s.engine.call(json!({"op": "state.get"}))?;
        full["state"]["selection"] = sel;
        let r = match s.engine.call(json!({"op": "state.set", "state": full["state"], "if_rev": rev})) {
            Err(e) if e.kind == "stale" => return Err(ToolError::Msg(format!("stale: {}", e.message))),
            r => r?,
        };
        let new = r["rev"].as_u64().unwrap_or(rev);
        s.mine.insert(new);
        Ok(new)
    }

    fn view_open(&self, args: &Map<String, Value>) -> R {
        let s = self.session(args)?;
        let mut s = s.lock().unwrap();
        if let Some((v, w, h)) = s.agent_view {
            return Ok(json!({"view": v, "width": w, "height": h, "already_open": true}).into());
        }
        let w = arg_u64(args, "width")?.unwrap_or(80).clamp(1, 1000) as u16;
        let h = arg_u64(args, "height")?.unwrap_or(24).clamp(1, 1000) as u16;
        // A copy of the person's view (their caret), focused, read-only in a read-only server.
        let r = s.engine.call(json!({"op": "view.open", "open": {"focused": true, "read_only": self.config.read_only}, "w": w, "h": h}))?;
        s.mark_mine(&r);
        let v = r["view"].as_u64().ok_or("view.open returned no view")? as u32;
        s.agent_view = Some((v, w, h));
        let (_, body, vs) = s.state(v)?;
        Ok(json!({
            "view": v,
            "rev": r["rev"],
            "width": w,
            "height": h,
            "agent": describe_selection(&body, &vs),
            "note": "keys and select now move this view's caret, not the person's. Text edits go through it too",
        })
        .into())
    }

    fn view_close(&self, args: &Map<String, Value>) -> R {
        let s = self.session(args)?;
        let mut s = s.lock().unwrap();
        let Some((v, _, _)) = s.agent_view.take() else { return Ok(json!({"closed": null, "note": "no agent view was open"}).into()) };
        let r = s.engine.call(json!({"op": "view.close", "view": v}))?;
        s.mark_mine(&r);
        Ok(json!({"closed": v, "rev": r["rev"]}).into())
    }

    fn watch(&self, args: &Map<String, Value>) -> R {
        let sess = self.session(args)?;
        let since = arg_u64(args, "since_rev")?.ok_or("since_rev is required: the rev you last saw (from read or edit)")?;
        let timeout = arg_u64(args, "timeout_ms")?.unwrap_or(20_000).min(300_000);
        let settle = arg_u64(args, "settle_ms")?.unwrap_or(400).min(10_000);
        let include_own = arg_bool(args, "include_own");
        let all = arg_bool(args, "include_noise");
        let (events, mine) = {
            let s = sess.lock().unwrap();
            (s.engine.events.clone(), s.mine.clone())
        };
        let wanted = |e: &Value| {
            e["rev"].as_u64().is_some_and(|r| r > since)
                && (all || !is_noise(e))
                && (include_own || !e["rev"].as_u64().is_some_and(|r| mine.contains(&r)))
        };
        let start = Instant::now();
        let deadline = start + Duration::from_millis(timeout);
        let got = events.wait_until(deadline, |log| log.events.iter().any(&wanted)).events.iter().any(&wanted);
        if got && settle > 0 {
            // Let a burst of typing finish, so one watch sees the whole word.
            let mut last = events.snapshot().events.len();
            loop {
                let until = Instant::now() + Duration::from_millis(settle);
                let n = events.wait_until(until, |log| log.events.len() > last).events.len();
                if n == last {
                    break;
                }
                last = n;
            }
        }
        let mut s = sess.lock().unwrap();
        let (rev, body, state) = s.state(0)?;
        let log = events.snapshot();
        let mut groups: Vec<Value> = Vec::new();
        for e in log.events.iter().filter(|e| wanted(e)) {
            let who = s.who(e);
            let r = e["rev"].as_u64().unwrap_or(0);
            let mut actions = Vec::new();
            if e["state_set"] == json!(true) {
                actions.push("replaced the whole state".to_string());
            }
            describe(e["msgs"].as_array().map(|v| v.as_slice()).unwrap_or(&[]), &mut actions);
            if all && actions.is_empty() {
                describe(&[], &mut actions);
                actions.push(e["msgs"].as_array().map(|m| m.iter().filter_map(|x| x["msg"].as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default());
            }
            let view = e.get("view").cloned();
            match groups.last_mut() {
                Some(g) if g["who"] == json!(who) && g.get("view") == view.as_ref() => {
                    g["to_rev"] = json!(r);
                    let mut acts: Vec<String> = serde_json::from_value(g["actions"].take()).unwrap_or_default();
                    for a in actions {
                        match (acts.last_mut(), a.strip_prefix("typed ")) {
                            (Some(last), Some(more)) if last.starts_with("typed ") => {
                                let mut t: String = serde_json::from_str(&last["typed ".len()..]).unwrap_or_default();
                                t.push_str(&serde_json::from_str::<String>(more).unwrap_or_default());
                                *last = format!("typed {}", json!(t));
                            }
                            _ => acts.push(a),
                        }
                    }
                    g["actions"] = json!(acts);
                }
                _ => {
                    let mut g = json!({"who": who, "from_rev": r, "to_rev": r, "actions": actions});
                    if let Some(v) = view {
                        g["view"] = v;
                    }
                    groups.push(g);
                }
            }
        }
        let missed = log.dropped_before.is_some_and(|d| d > since);
        let closed = log.closed;
        drop(log);
        let base = s.texts.get(&since).cloned();
        let diff = base.as_deref().and_then(|b| text::diff(b, &body, 12));
        let mut out = json!({
            "rev": rev,
            "since_rev": since,
            "changes": groups,
            "timed_out": !got,
            "text_changed": base.as_deref().map(|b| b != body),
            "diff": diff,
            "carets": { "person": describe_selection(&body, &state) },
            "waited_ms": start.elapsed().as_millis() as u64,
        });
        if missed {
            out["note"] = json!("some events before these were dropped (the buffer is bounded)");
        }
        if closed {
            out["closed"] = json!(true);
            out["note"] = json!("the editor is gone (it quit). Its last state is what read returned before");
        }
        Ok(out.into())
    }

    fn trace(&self, args: &Map<String, Value>) -> R {
        let s = self.session(args)?;
        let mut s = s.lock().unwrap();
        let mut req = json!({"op": "trace.get"});
        if let Some(r) = arg_u64(args, "since_rev")? {
            req["since_rev"] = json!(r);
        }
        if arg_bool(args, "all") {
            req["all"] = json!(true);
        }
        let r = s.engine.call(req)?;
        let lines: Vec<Value> = r["trace"].as_array().cloned().unwrap_or_default();
        let jsonl: String = lines.iter().map(|l| format!("{l}\n")).collect();
        let standalone = lines.first().is_some_and(|l| l.get("state").is_some());
        let mut out = json!({
            "rev": r["rev"],
            "from_rev": r["from_rev"],
            "lines": lines.len(),
            "replayable_alone": standalone,
            "note": if standalone { "starts with a state line: `caretline --replay FILE --dump-state -` rebuilds the state exactly" } else { "the lines after since_rev: they replay onto the state at that rev. Omit since_rev for a trace that replays on its own" },
        });
        if let Some(path) = arg_str(args, "path") {
            let abs = std::path::absolute(path).map_err(|e| format!("{path}: {e}"))?;
            std::fs::write(&abs, &jsonl).map_err(|e| format!("{}: {e}", abs.display()))?;
            out["path"] = json!(abs);
            out["replay"] = json!(format!("caretline --replay {} --snapshot 80x24", abs.display()));
            return Ok(out.into());
        }
        if jsonl.len() > 512 * 1024 {
            return Err(format!("the trace is {} KB: pass path to write it to a file", jsonl.len() / 1024).into());
        }
        Ok(Output { data: out, text: Some(jsonl) })
    }

    fn save(&self, args: &Map<String, Value>) -> R {
        self.writable("save")?;
        let s = self.session(args)?;
        let mut s = s.lock().unwrap();
        if !s.engine.is_live() && s.file.is_none() {
            return Err("this headless session was opened from text and has no file to save to".into());
        }
        let r = s.engine.call(json!({"op": "msgs", "view": 0, "msgs": [{"msg": "save"}], "apply_effects": true}))?;
        s.mark_mine(&r);
        let msgs: Vec<&str> = r["msgs"].as_array().map(|m| m.iter().filter_map(|x| x["msg"].as_str()).collect()).unwrap_or_default();
        let failed = r["msgs"].as_array().and_then(|m| m.iter().find(|x| x["msg"] == "save_failed").map(|x| x["err"].clone()));
        let (rev, _, state) = s.state(0)?;
        let saved = msgs.contains(&"saved");
        let written: Vec<Value> = r["effects"].as_array().map(|e| e.iter().filter(|x| x["effect"] == "write_file").map(|x| x["path"].clone()).collect()).unwrap_or_default();
        let out = json!({
            "saved": saved,
            "path": written.first().cloned().or_else(|| s.file.clone().map(Value::from)),
            "rev": rev,
            "dirty": state["dirty"],
            "error": failed,
            "status": state["status"],
        });
        if !saved {
            return Err(ToolError::Data(out));
        }
        Ok(out.into())
    }
}

fn op_kind(op: &Value) -> Result<String, ToolError> {
    if let Some(k) = op["kind"].as_str() {
        return match k {
            "replace" | "replace_range" | "insert" | "keys" | "select" => Ok(k.to_string()),
            _ => Err(format!("unknown op kind {k:?}: use replace, replace_range, insert, select or keys").into()),
        };
    }
    let has = |k: &str| op.get(k).is_some_and(|v| !v.is_null());
    Ok(if has("search") {
        "replace"
    } else if has("keys") {
        "keys"
    } else if has("at") {
        "insert"
    } else if has("from") && has("text") {
        "replace_range"
    } else if has("from") {
        "select"
    } else {
        return Err("each op needs a kind: replace, replace_range, insert, select or keys".into());
    }
    .to_string())
}

fn new_state(body: &str, path: Option<String>, width: u16, height: u16, outline: bool, layout: bool) -> caretline::State {
    let viewport = caretline::Viewport { width, height };
    let mut state = if outline {
        caretline::outline::markdown::load(body, path, viewport, caretline::OutlineConfig::default())
    } else {
        caretline::State::new(body, path, viewport)
    };
    if layout {
        state.view.layout = Some(caretline::OutlineLayout { hang_glyphs: true, ..Default::default() });
    }
    state
}
