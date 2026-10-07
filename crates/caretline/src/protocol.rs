//! The state protocol: JSON requests in, JSON responses out, one per line. See
//! `docs/caretline/protocol.md` for the wire format with examples.
//!
//! [`Session::handle`] answers one request line. Transport concerns (subscriptions, who
//! receives events, performing effects) belong to the caller: the response carries a
//! [`Change`] when the state changed and a [`Control`] when the client asked to
//! (un)subscribe, and [`event_line`] builds the notification for each subscriber.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::msg::{Effect, Msg};
use crate::session::Session;
use crate::state::{State, View};
use crate::trace::TraceLine;
use crate::view::{Frame, Role};

/// The protocol version `hello` reports. Within a version, responses only gain fields.
pub const PROTO: u32 = 1;

/// The operations this version understands.
pub const OPS: &[&str] = &[
    "hello",
    "state.get",
    "state.set",
    "text.set",
    "history.get",
    "frame",
    "msgs",
    "keys",
    "render",
    "subscribe",
    "unsubscribe",
    "trace.get",
    "trace.checkpoint",
    "view.open",
    "view.close",
    "view.list",
];

/// A frame format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    #[default]
    Text,
    Ansi,
    Cells,
}

/// Which frame to render: a size (the state's viewport when absent) and a format.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameSpec {
    #[serde(default)]
    pub w: Option<u16>,
    #[serde(default)]
    pub h: Option<u16>,
    #[serde(default)]
    pub format: Format,
}

/// What a subscriber receives after each change.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Subscription {
    /// Include the applied messages in each event.
    pub msgs: bool,
    /// Include a rendered frame in each event.
    pub frame: Option<FrameSpec>,
    /// Include the state, without its undo history, in each event.
    pub state: bool,
}

/// A state change a request (or the runtime) made.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    /// The rev after the change.
    pub rev: u64,
    /// The messages applied, in order (effect results included).
    pub msgs: Vec<Msg>,
    /// The state was replaced (`state.set`).
    pub state_set: bool,
    /// The view the messages went through, when not the state's own (0), or the view opened
    /// or closed.
    pub view: Option<u32>,
}

/// A transport-level request the caller carries out.
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    Subscribe(Subscription),
    Unsubscribe,
}

/// The answer to one request.
#[derive(Debug, Clone)]
pub struct Handled {
    /// The response line (no trailing newline).
    pub response: String,
    pub change: Option<Change>,
    pub control: Option<Control>,
}

/// Runs an effect and returns the message that reports its result, if any.
pub type Executor<'a> = &'a mut dyn FnMut(&Effect) -> Option<Msg>;

#[derive(Deserialize)]
struct Request {
    #[serde(default)]
    id: Option<Value>,
    op: String,
    #[serde(default)]
    state: Option<State>,
    #[serde(default)]
    msgs: Option<Vec<Msg>>,
    #[serde(default)]
    keys: Option<String>,
    #[serde(default)]
    w: Option<u16>,
    #[serde(default)]
    h: Option<u16>,
    #[serde(default)]
    format: Option<Format>,
    #[serde(default)]
    apply_effects: bool,
    #[serde(default)]
    if_rev: Option<u64>,
    /// msgs, keys: the time to tick to before applying them.
    #[serde(default)]
    now_ms: Option<u64>,
    /// subscribe: a frame with every event.
    #[serde(default)]
    frame: Option<FrameSpec>,
    /// subscribe: the messages with every event (default true).
    #[serde(default)]
    with_msgs: Option<bool>,
    /// subscribe: the state, without its undo history, with every event.
    #[serde(default)]
    with_state: bool,
    /// state.get: include the undo history (default true; `false` leaves it to history.get).
    #[serde(default)]
    history: Option<bool>,
    /// trace.get: only the lines after this rev.
    #[serde(default)]
    since_rev: Option<u64>,
    /// trace.get: every line kept, not just the current segment.
    #[serde(default)]
    all: bool,
    /// msgs, keys, text.set, render, state.get, view.close: the view (0 is the state's own;
    /// others come from view.open). Without it, msgs, keys and text.set go through the
    /// client's own view when the server gives clients one (a live editor), else view 0.
    #[serde(default)]
    view: Option<u32>,
    /// frame: the whole text to show. text.set: the new text.
    #[serde(default)]
    text: Option<String>,
    /// frame: char ranges to draw in the selection's colour, `[start, end]`.
    #[serde(default)]
    highlights: Vec<(usize, usize)>,
    /// frame: where the primary caret goes.
    #[serde(default)]
    caret: Option<usize>,
    /// frame: the status bar's message.
    #[serde(default)]
    status: Option<String>,
    /// view.open: the new view (every field optional; a copy of view 0's when absent).
    #[serde(default)]
    open: Option<View>,
}

#[derive(Serialize)]
struct Reply<'a, T: Serialize> {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<&'a Value>,
    result: T,
}

#[derive(Serialize)]
struct ErrorReply<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<&'a Value>,
    error: ErrorBody<'a>,
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    kind: &'a str,
    message: String,
}

/// A protocol error: a kind clients branch on and a message for people.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtoError {
    pub kind: &'static str,
    pub message: String,
}

fn err(kind: &'static str, message: impl Into<String>) -> ProtoError {
    ProtoError { kind, message: message.into() }
}

fn to_line<T: Serialize>(id: Option<&Value>, result: T) -> String {
    serde_json::to_string(&Reply { id, result }).expect("response serializes")
}

/// An error response line.
pub fn error_line(id: Option<&Value>, e: &ProtoError) -> String {
    serde_json::to_string(&ErrorReply {
        id,
        error: ErrorBody { kind: e.kind, message: e.message.clone() },
    })
    .expect("error serializes")
}

#[derive(Serialize)]
struct CellRow {
    text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    spans: Vec<(u16, u16, String)>,
    /// What the row shows (text of a line, a block's blank row, …).
    info: crate::view::RowInfo,
}

/// The name a role has on the wire.
pub fn role_name(role: Role) -> &'static str {
    match role {
        Role::Text => "text",
        Role::Selection => "selection",
        Role::Status => "status",
        Role::StatusAccent => "status_accent",
        Role::Hang => "hang",
        // A host's role: its name is the frame's ([`Frame::role_name`]).
        Role::Named(_) => "host",
    }
}

/// A rendered frame as protocol JSON fields.
#[derive(Serialize)]
pub struct RenderedFrame {
    pub w: u16,
    pub h: u16,
    pub format: Format,
    /// The caret's cell, `[x, y]`, when it is on screen.
    pub cursor: Option<[u16; 2]>,
    /// `text` and `ansi`: the frame as `--snapshot` prints it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
    /// `cells`: one entry per row.
    #[serde(skip_serializing_if = "Option::is_none")]
    rows: Option<Vec<CellRow>>,
}

impl RenderedFrame {
    pub fn new(frame: &Frame, format: Format) -> RenderedFrame {
        let (text, rows) = match format {
            Format::Text => (Some(frame.to_text()), None),
            Format::Ansi => (Some(frame.to_ansi()), None),
            Format::Cells => (None, Some(cell_rows(frame))),
        };
        RenderedFrame {
            w: frame.width,
            h: frame.height,
            format,
            cursor: frame.cursor.map(|(x, y)| [x, y]),
            frame: text,
            rows,
        }
    }
}

/// Each row's symbols joined (a wide grapheme's second cell adds nothing) and its runs of
/// non-text roles as `[x, len, role]`, in cell columns.
fn cell_rows(frame: &Frame) -> Vec<CellRow> {
    (0..frame.height)
        .map(|y| {
            let mut text = String::with_capacity(frame.width as usize);
            let mut spans: Vec<(u16, u16, String)> = Vec::new();
            for x in 0..frame.width {
                let cell = frame.cell(x, y);
                text.push_str(&cell.symbol);
                if cell.role == Role::Text {
                    continue;
                }
                let name = frame.role_name(cell.role);
                match spans.last_mut() {
                    Some((sx, len, r)) if r == name && *sx + *len == x => *len += 1,
                    _ => spans.push((x, 1, name.to_string())),
                }
            }
            CellRow { text, spans, info: frame.rows.get(y as usize).cloned().unwrap_or(crate::view::RowInfo::Past) }
        })
        .collect()
}

fn render(session: &Session, spec: FrameSpec) -> Result<RenderedFrame, ProtoError> {
    render_view(session, 0, spec)
}

fn render_view(session: &Session, id: u32, spec: FrameSpec) -> Result<RenderedFrame, ProtoError> {
    let v = session.view(id).ok_or_else(|| no_view(id))?.viewport;
    let (w, h) = (spec.w.unwrap_or(v.width), spec.h.unwrap_or(v.height));
    if w == 0 || h == 0 {
        return Err(err("bad_request", "w and h must be at least 1"));
    }
    let frame = if id == 0 { session.render(w, h) } else { session.render_view(id, Some((w, h))).ok_or_else(|| no_view(id))? };
    Ok(RenderedFrame::new(&frame, spec.format))
}

fn no_view(id: u32) -> ProtoError {
    err("no_view", format!("no view {id}; view.list lists them"))
}

#[derive(Serialize)]
struct Event<'a> {
    event: &'static str,
    rev: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<&'a str>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    state_set: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    view: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    msgs: Option<&'a [Msg]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame: Option<RenderedFrame>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<crate::state::WithoutHistory<'a>>,
}

/// The `{event:"state"}` line a subscriber receives for `change`. `source` says where the
/// change came from (`"client"`, `"terminal"`, …).
pub fn event_line(session: &Session, change: &Change, sub: &Subscription, source: Option<&str>) -> String {
    let frame = sub.frame.and_then(|spec| render(session, spec).ok());
    serde_json::to_string(&Event {
        event: "state",
        rev: change.rev,
        source,
        state_set: change.state_set,
        view: change.view,
        msgs: sub.msgs.then_some(change.msgs.as_slice()),
        frame,
        state: sub.state.then(|| session.state().without_history()),
    })
    .expect("event serializes")
}

impl Session {
    /// Answers one request line. `exec`, when given, performs effects for requests that set
    /// `apply_effects`; without it such a request is refused (`unsupported`).
    pub fn handle(&mut self, line: &str, exec: Option<Executor<'_>>) -> Handled {
        self.handle_at(line, exec, None)
    }

    /// [`Session::handle`] for a runtime with a clock: before a `msgs` or `keys` request
    /// without its own `now_ms`, a `tick` to `clock_ms` is applied (when it is later than the
    /// state's clock), so typing runs and undo steps follow real time. The tick is an
    /// ordinary message: it is in the response, the events and the trace.
    pub fn handle_at(&mut self, line: &str, exec: Option<Executor<'_>>, clock_ms: Option<u64>) -> Handled {
        self.handle_client(line, exec, clock_ms, None)
    }

    /// [`Session::handle_at`] for one client of a server where clients don't act through the
    /// person's view (a live editor). `own` is the client's own view: a `msgs`, `keys` or
    /// `text.set` request without a `view` goes through it, and the first such request opens
    /// it (a copy of view 0, after the `if_rev` check) and stores its id in `own`. The caller
    /// closes it when the client goes. With `own` absent, those requests go through view 0.
    pub fn handle_client(
        &mut self,
        line: &str,
        exec: Option<Executor<'_>>,
        clock_ms: Option<u64>,
        own: Option<&mut Option<u32>>,
    ) -> Handled {
        let req: Request = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => return bad_line(line, e),
        };
        let id = req.id.clone();
        match self.handle_request(req, exec, clock_ms, own) {
            Ok(h) => h,
            Err(e) => Handled { response: error_line(id.as_ref(), &e), change: None, control: None },
        }
    }

    fn handle_request(
        &mut self,
        req: Request,
        exec: Option<Executor<'_>>,
        clock_ms: Option<u64>,
        own: Option<&mut Option<u32>>,
    ) -> Result<Handled, ProtoError> {
        let id = req.id.as_ref();
        let reply = |response: String| Handled { response, change: None, control: None };
        let check_rev = |rev: u64| match req.if_rev {
            Some(want) if want != rev => Err(err(
                "stale",
                format!("rev is {rev}, the request expected {want}"),
            )),
            _ => Ok(()),
        };
        match req.op.as_str() {
            "hello" => Ok(reply(to_line(
                id,
                serde_json::json!({
                    "proto": PROTO,
                    "version": env!("CARGO_PKG_VERSION"),
                    "rev": self.rev(),
                    "ops": OPS,
                    "commands": self.state().doc.host().command_names(),
                }),
            ))),
            "state.get" => {
                #[derive(Serialize)]
                struct R<S: Serialize> {
                    rev: u64,
                    state: S,
                }
                let full = req.history.unwrap_or(true);
                let line = |state: &State| match full {
                    true => to_line(id, R { rev: self.rev(), state }),
                    false => to_line(id, R { rev: self.rev(), state: state.without_history() }),
                };
                match req.view.unwrap_or(0) {
                    0 => Ok(reply(line(self.state()))),
                    v => Ok(reply(line(&self.state_of(v).ok_or_else(|| no_view(v))?))),
                }
            }
            "history.get" => {
                #[derive(Serialize)]
                struct R<'a> {
                    rev: u64,
                    #[serde(flatten)]
                    part: crate::state::HistoryPart<'a>,
                }
                Ok(reply(to_line(id, R { rev: self.rev(), part: self.state().history_part() })))
            }
            "state.set" => {
                check_rev(self.rev())?;
                let state = req.state.ok_or_else(|| err("bad_request", "state.set needs a state"))?;
                let rev = self.set_state(state);
                Ok(Handled {
                    response: to_line(id, serde_json::json!({ "rev": rev })),
                    change: Some(Change { rev, msgs: Vec::new(), state_set: true, view: None }),
                    control: None,
                })
            }
            "frame" => {
                check_rev(self.rev())?;
                let text = req.text.ok_or_else(|| err("bad_request", "frame needs a text"))?;
                let rev = self.push_frame(&text, &req.highlights, req.caret, req.status);
                Ok(Handled {
                    response: to_line(id, serde_json::json!({ "rev": rev })),
                    change: Some(Change { rev, msgs: Vec::new(), state_set: true, view: None }),
                    control: None,
                })
            }
            "text.set" => {
                check_rev(self.rev())?;
                let text = req.text.as_deref().ok_or_else(|| err("bad_request", "text.set needs a text"))?;
                let on = self.acting_view(req.view, own)?;
                let applied: Vec<Msg> = self.set_text_on(on, text).into_iter().collect();
                let rev = self.rev();
                let response = to_line(id, serde_json::json!({ "rev": rev, "changed": !applied.is_empty(), "view": on, "msgs": applied }));
                let change = (!applied.is_empty()).then_some(Change { rev, msgs: applied, state_set: false, view: (on != 0).then_some(on) });
                Ok(Handled { response, change, control: None })
            }
            "msgs" | "keys" => {
                check_rev(self.rev())?;
                let on = self.acting_view(req.view, own)?;
                if req.apply_effects && exec.is_none() {
                    return Err(err("unsupported", "this server returns effects; it doesn't perform them"));
                }
                // The clock: the request's own `now_ms`, else the runtime's (only forward).
                let now = self.state().doc.now_ms;
                let tick = req.now_ms.filter(|&t| t != now).or(clock_ms.filter(|&t| t > now));
                let base = tick.unwrap_or(now);
                let mut msgs: Vec<Msg> = tick.map(|now_ms| Msg::Tick { now_ms }).into_iter().collect();
                if req.op == "keys" {
                    let script = req.keys.ok_or_else(|| err("bad_request", "keys needs a keys script"))?;
                    let outline = self.state().doc.outline.is_some();
                    msgs.extend(crate::keymap::script_to_msgs_for(&script, base, outline).map_err(|e| err("bad_keys", e))?);
                } else {
                    msgs.extend(req.msgs.ok_or_else(|| err("bad_request", "msgs needs a msgs array"))?);
                }
                let mut effects = Vec::new();
                let mut applied = Vec::new();
                match (req.apply_effects, exec) {
                    (true, Some(exec)) => {
                        for msg in msgs {
                            let (e, m) = self.apply_with_on(on, msg, &mut *exec);
                            effects.extend(e);
                            applied.extend(m);
                        }
                    }
                    _ => {
                        for msg in msgs.iter().cloned() {
                            effects.extend(self.apply_on(on, msg));
                        }
                        applied = msgs;
                    }
                }
                #[derive(Serialize)]
                struct R<'a> {
                    rev: u64,
                    effects: &'a [Effect],
                    #[serde(skip_serializing_if = "std::ops::Not::not")]
                    executed: bool,
                    msgs: &'a [Msg],
                    /// The view the messages went through.
                    view: u32,
                }
                let rev = self.rev();
                let response = to_line(
                    id,
                    R {
                        rev,
                        effects: &effects,
                        executed: req.apply_effects,
                        msgs: &applied,
                        view: on,
                    },
                );
                let change = (!applied.is_empty()).then_some(Change { rev, msgs: applied, state_set: false, view: (on != 0).then_some(on) });
                Ok(Handled { response, change, control: None })
            }
            "render" => {
                let spec = FrameSpec { w: req.w, h: req.h, format: req.format.unwrap_or_default() };
                #[derive(Serialize)]
                struct R {
                    rev: u64,
                    #[serde(flatten)]
                    frame: RenderedFrame,
                }
                let frame = render_view(self, req.view.unwrap_or(0), spec)?;
                Ok(reply(to_line(id, R { rev: self.rev(), frame })))
            }
            "subscribe" => {
                let sub = Subscription { msgs: req.with_msgs.unwrap_or(true), frame: req.frame, state: req.with_state };
                Ok(Handled {
                    response: to_line(id, serde_json::json!({ "rev": self.rev(), "subscribed": true })),
                    change: None,
                    control: Some(Control::Subscribe(sub)),
                })
            }
            "unsubscribe" => Ok(Handled {
                response: to_line(id, serde_json::json!({ "rev": self.rev(), "subscribed": false })),
                change: None,
                control: Some(Control::Unsubscribe),
            }),
            "trace.get" => {
                #[derive(Serialize)]
                struct R<'a> {
                    rev: u64,
                    /// The rev the returned lines start from.
                    from_rev: u64,
                    trace: &'a [TraceLine],
                }
                let (from_rev, trace) = match (req.since_rev, req.all) {
                    (Some(_), true) => {
                        return Err(err("bad_request", "trace.get takes since_rev or all, not both"))
                    }
                    (Some(since), false) => {
                        let lines = self.trace_since(since).ok_or_else(|| {
                            err(
                                "trimmed",
                                format!(
                                    "the trace now starts at rev {}; ask for since_rev >= {} or the current segment",
                                    self.trace_start_rev(),
                                    self.trace_start_rev()
                                ),
                            )
                        })?;
                        (since.min(self.rev()), lines)
                    }
                    (None, true) => (self.trace_start_rev(), self.trace()),
                    (None, false) => (self.segment_rev(), self.segment_trace()),
                };
                Ok(reply(to_line(id, R { rev: self.rev(), from_rev, trace })))
            }
            "view.open" => {
                check_rev(self.rev())?;
                let mut view = req.open.unwrap_or_else(|| self.state().view.clone());
                if let (Some(w), Some(h)) = (req.w, req.h) {
                    view.viewport = crate::state::Viewport { width: w.max(1), height: h.max(1) };
                }
                let v = self.open_view(view);
                let rev = self.rev();
                Ok(Handled {
                    response: to_line(id, serde_json::json!({ "rev": rev, "view": v })),
                    change: Some(Change { rev, msgs: Vec::new(), state_set: false, view: Some(v) }),
                    control: None,
                })
            }
            "view.close" => {
                let v = req.view.ok_or_else(|| err("bad_request", "view.close needs a view"))?;
                if v == 0 {
                    return Err(err("bad_request", "view 0 is the state's own and stays open"));
                }
                if !self.close_view(v) {
                    return Err(no_view(v));
                }
                let rev = self.rev();
                Ok(Handled {
                    response: to_line(id, serde_json::json!({ "rev": rev, "closed": v })),
                    change: Some(Change { rev, msgs: Vec::new(), state_set: false, view: Some(v) }),
                    control: None,
                })
            }
            "view.list" => {
                let mut list = vec![view_summary(0, &self.state().view)];
                list.extend(self.views().iter().map(|(k, v)| view_summary(*k, v)));
                Ok(reply(to_line(id, serde_json::json!({ "rev": self.rev(), "views": list }))))
            }
            "trace.checkpoint" => {
                let rev = self.checkpoint();
                Ok(reply(to_line(id, serde_json::json!({ "rev": rev }))))
            }
            other => Err(err(
                "unknown_op",
                format!("unknown op {other:?}; known ops: {}", OPS.join(", ")),
            )),
        }
    }
}

impl Session {
    /// The view a writing request goes through: the one it names, else the client's own
    /// (opened now when it has none), else view 0.
    fn acting_view(&mut self, named: Option<u32>, own: Option<&mut Option<u32>>) -> Result<u32, ProtoError> {
        let on = match (named, own) {
            (Some(v), _) => v,
            (None, Some(own)) => match *own {
                Some(v) if self.view(v).is_some() => v,
                _ => {
                    let mut view = self.state().view.clone();
                    view.status = None;
                    let v = self.open_view(view);
                    *own = Some(v);
                    v
                }
            },
            (None, None) => 0,
        };
        if self.view(on).is_none() {
            return Err(no_view(on));
        }
        Ok(on)
    }
}

fn view_summary(id: u32, v: &View) -> Value {
    serde_json::json!({
        "view": id,
        "w": v.viewport.width,
        "h": v.viewport.height,
        "caret": v.caret(),
        "read_only": v.read_only,
    })
}

/// A line that isn't a valid request: report it with the id when the JSON at least parses.
fn bad_line(line: &str, e: serde_json::Error) -> Handled {
    let response = match serde_json::from_str::<Value>(line) {
        Ok(Value::Object(obj)) => {
            let id = obj.get("id").filter(|v| !v.is_null());
            let message = if obj.get("op").and_then(Value::as_str).is_none() {
                "a request needs an \"op\" string".to_string()
            } else {
                e.to_string()
            };
            error_line(id, &err("bad_request", message))
        }
        Ok(_) => error_line(None, &err("bad_request", "a request is a JSON object")),
        Err(e) => error_line(None, &err("parse", format!("not JSON: {e}"))),
    };
    Handled { response, change: None, control: None }
}
