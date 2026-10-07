//! The state protocol: JSON requests in, JSON responses out, one per line. See
//! `crates/caretline-app/PROTOCOL.md` for the wire format with examples.
//!
//! [`Session::handle`] answers one request line. Transport concerns (subscriptions, who
//! receives events, performing effects) belong to the caller: the response carries a
//! [`Change`] when the state changed and a [`Control`] when the client asked to
//! (un)subscribe, and [`event_line`] builds the notification for each subscriber.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::msg::{Effect, Msg};
use crate::session::Session;
use crate::state::State;
use crate::trace::TraceLine;
use crate::view::{Frame, Role};

/// The protocol version `hello` reports. Within a version, responses only gain fields.
pub const PROTO: u32 = 1;

/// The operations this version understands.
pub const OPS: &[&str] = &[
    "hello",
    "state.get",
    "state.set",
    "msgs",
    "keys",
    "render",
    "subscribe",
    "unsubscribe",
    "trace.get",
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
    /// subscribe: a frame with every event.
    #[serde(default)]
    frame: Option<FrameSpec>,
    /// subscribe: the messages with every event (default true).
    #[serde(default)]
    with_msgs: Option<bool>,
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
    spans: Vec<(u16, u16, &'static str)>,
}

/// The name a role has on the wire.
pub fn role_name(role: Role) -> &'static str {
    match role {
        Role::Text => "text",
        Role::Selection => "selection",
        Role::Status => "status",
        Role::StatusAccent => "status_accent",
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
            let mut spans: Vec<(u16, u16, &'static str)> = Vec::new();
            for x in 0..frame.width {
                let cell = frame.cell(x, y);
                text.push_str(&cell.symbol);
                if cell.role == Role::Text {
                    continue;
                }
                let name = role_name(cell.role);
                match spans.last_mut() {
                    Some((sx, len, r)) if *r == name && *sx + *len == x => *len += 1,
                    _ => spans.push((x, 1, name)),
                }
            }
            CellRow { text, spans }
        })
        .collect()
}

fn render(session: &Session, spec: FrameSpec) -> Result<RenderedFrame, ProtoError> {
    let v = session.state().viewport;
    let (w, h) = (spec.w.unwrap_or(v.width), spec.h.unwrap_or(v.height));
    if w == 0 || h == 0 {
        return Err(err("bad_request", "w and h must be at least 1"));
    }
    Ok(RenderedFrame::new(&session.render(w, h), spec.format))
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
    msgs: Option<&'a [Msg]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame: Option<RenderedFrame>,
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
        msgs: sub.msgs.then_some(change.msgs.as_slice()),
        frame,
    })
    .expect("event serializes")
}

impl Session {
    /// Answers one request line. `exec`, when given, performs effects for requests that set
    /// `apply_effects`; without it such a request is refused (`unsupported`).
    pub fn handle(&mut self, line: &str, exec: Option<Executor<'_>>) -> Handled {
        let req: Request = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => return bad_line(line, e),
        };
        let id = req.id.clone();
        match self.handle_request(req, exec) {
            Ok(h) => h,
            Err(e) => Handled { response: error_line(id.as_ref(), &e), change: None, control: None },
        }
    }

    fn handle_request(&mut self, req: Request, exec: Option<Executor<'_>>) -> Result<Handled, ProtoError> {
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
                }),
            ))),
            "state.get" => {
                #[derive(Serialize)]
                struct R<'a> {
                    rev: u64,
                    state: &'a State,
                }
                Ok(reply(to_line(id, R { rev: self.rev(), state: self.state() })))
            }
            "state.set" => {
                check_rev(self.rev())?;
                let state = req.state.ok_or_else(|| err("bad_request", "state.set needs a state"))?;
                let rev = self.set_state(state);
                Ok(Handled {
                    response: to_line(id, serde_json::json!({ "rev": rev })),
                    change: Some(Change { rev, msgs: Vec::new(), state_set: true }),
                    control: None,
                })
            }
            "msgs" | "keys" => {
                check_rev(self.rev())?;
                if req.apply_effects && exec.is_none() {
                    return Err(err("unsupported", "this server returns effects; it doesn't perform them"));
                }
                let (msgs, from_keys) = if req.op == "keys" {
                    let script = req.keys.ok_or_else(|| err("bad_request", "keys needs a keys script"))?;
                    let msgs = crate::keymap::script_to_msgs(&script, self.state().now_ms)
                        .map_err(|e| err("bad_keys", e))?;
                    (msgs, true)
                } else {
                    (req.msgs.ok_or_else(|| err("bad_request", "msgs needs a msgs array"))?, false)
                };
                let mut effects = Vec::new();
                let mut applied = Vec::new();
                match (req.apply_effects, exec) {
                    (true, Some(exec)) => {
                        for msg in msgs.iter().cloned() {
                            let (e, m) = self.apply_with(msg, &mut *exec);
                            effects.extend(e);
                            applied.extend(m);
                        }
                    }
                    _ => {
                        effects = self.apply_all(msgs.iter().cloned());
                        applied = msgs.clone();
                    }
                }
                #[derive(Serialize)]
                struct R<'a> {
                    rev: u64,
                    effects: &'a [Effect],
                    #[serde(skip_serializing_if = "std::ops::Not::not")]
                    executed: bool,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    msgs: Option<&'a [Msg]>,
                }
                let rev = self.rev();
                let response = to_line(
                    id,
                    R {
                        rev,
                        effects: &effects,
                        executed: req.apply_effects,
                        msgs: from_keys.then_some(msgs.as_slice()),
                    },
                );
                let change = (!applied.is_empty()).then_some(Change { rev, msgs: applied, state_set: false });
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
                let frame = render(self, spec)?;
                Ok(reply(to_line(id, R { rev: self.rev(), frame })))
            }
            "subscribe" => {
                let sub = Subscription { msgs: req.with_msgs.unwrap_or(true), frame: req.frame };
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
                    trace: &'a [TraceLine],
                }
                Ok(reply(to_line(id, R { rev: self.rev(), trace: self.trace() })))
            }
            other => Err(err(
                "unknown_op",
                format!("unknown op {other:?}; known ops: {}", OPS.join(", ")),
            )),
        }
    }
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
