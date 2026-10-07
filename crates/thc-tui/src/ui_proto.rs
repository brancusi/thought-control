//! The UI protocol: one JSON request per line in, one JSON response per line out, against a
//! [`Session`]. The wire format is docs/ui-protocol.md; it follows caretline's state protocol
//! (`hello`, `state.get`, `state.set`, `msgs`, `keys`, `render`, `subscribe`, `trace.get`,
//! `rev` and `if_rev`), with `patch` for JSON merge patches.
//!
//! Transport (sockets, who gets events, performing effects) is the caller's: a response comes
//! with the [`Change`] it made and any subscription [`Control`].

use crate::session::{Msg, Session};
use serde::Deserialize;
use serde_json::{Value, json};

/// The protocol version `hello` reports. Within a version, results only gain fields.
pub const PROTO: u32 = 1;

pub const OPS: &[&str] = &["hello", "state.get", "state.set", "patch", "msgs", "keys", "render", "subscribe", "unsubscribe", "trace.get", "trace.checkpoint", "aside", "aside.ls"];

/// What a subscriber receives with each change.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Subscription {
    pub msgs: bool,
    /// A frame with each event: (w, h, format); no size means the session's.
    pub frame: Option<(Option<u16>, Option<u16>, String)>,
    pub state: bool,
}

/// A change: the rev after it and the messages applied (as trace JSON).
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub rev: u64,
    pub msgs: Vec<Value>,
    pub state_set: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    Subscribe(Subscription),
    Unsubscribe,
}

pub struct Handled {
    pub response: String,
    pub change: Option<Change>,
    pub control: Option<Control>,
}

/// How the session's host runs requests.
pub struct Host {
    /// Tick to the wall clock before a request's messages (a live TUI; never in tests).
    pub clock: bool,
    /// The host performs effects (`apply_effects: true` is allowed).
    pub effects: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    #[serde(default)]
    id: Option<Value>,
    op: String,
    #[serde(default)]
    state: Option<Value>,
    #[serde(default)]
    patch: Option<Value>,
    #[serde(default)]
    msgs: Option<Vec<Value>>,
    #[serde(default)]
    keys: Option<String>,
    #[serde(default)]
    w: Option<u16>,
    #[serde(default)]
    h: Option<u16>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    history: Option<bool>,
    #[serde(default)]
    if_rev: Option<u64>,
    #[serde(default)]
    apply_effects: bool,
    /// Who is asking (`THC_ACTOR`): a changed view says so in a toast.
    #[serde(default)]
    actor: Option<String>,
    #[serde(default)]
    since_rev: Option<u64>,
    #[serde(default)]
    all: bool,
    #[serde(default)]
    frame: Option<Value>,
    #[serde(default)]
    with_msgs: Option<bool>,
    #[serde(default)]
    with_state: bool,
    /// `aside`: what to open beside (sidebar.md §10.3), and how.
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    pin: bool,
    #[serde(default)]
    fold: bool,
    #[serde(default)]
    close: bool,
}

struct ProtoError {
    kind: &'static str,
    message: String,
}

fn err(kind: &'static str, message: impl Into<String>) -> ProtoError {
    ProtoError { kind, message: message.into() }
}

pub fn error_line(id: Option<&Value>, kind: &str, message: &str) -> String {
    let error = json!({"kind": kind, "message": message});
    match id {
        Some(id) => json!({"id": id, "error": error}),
        None => json!({"error": error}),
    }
    .to_string()
}

fn ok_line(id: Option<&Value>, result: Value) -> String {
    match id {
        Some(id) => json!({"id": id, "result": result}),
        None => json!({"result": result}),
    }
    .to_string()
}

/// Answer one request line.
pub fn handle(session: &mut Session, line: &str, host: &Host) -> Handled {
    let reply = |response| Handled { response, change: None, control: None };
    let raw: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return reply(error_line(None, "parse", &format!("not JSON: {e}"))),
    };
    let id = raw.get("id").cloned();
    let req: Request = match serde_json::from_value(raw) {
        Ok(r) => r,
        Err(e) => return reply(error_line(id.as_ref(), "bad_request", &e.to_string())),
    };
    match run(session, req, host) {
        Ok(h) => h,
        Err(e) => reply(error_line(id.as_ref(), e.kind, &e.message)),
    }
}

fn run(session: &mut Session, req: Request, host: &Host) -> Result<Handled, ProtoError> {
    let id = req.id.as_ref();
    let reply = |response| Ok(Handled { response, change: None, control: None });
    let check_rev = |rev: u64| match req.if_rev {
        Some(want) if want != rev => Err(err("stale", format!("rev is {rev}, the request expected {want}"))),
        _ => Ok(()),
    };
    match req.op.as_str() {
        "hello" => reply(ok_line(id, json!({"proto": PROTO, "version": env!("CARGO_PKG_VERSION"), "rev": session.rev, "ops": OPS, "vault": session.app.ui.vault_name, "size": [session.size.0, session.size.1]}))),
        "state.get" => {
            let rev = session.rev;
            let s = session.state();
            let state = if req.history.unwrap_or(true) { s.to_json() } else { s.to_json_without_history() };
            reply(ok_line(id, json!({"rev": rev, "state": state})))
        }
        "state.set" | "patch" => {
            check_rev(session.rev)?;
            let msg = if req.op == "patch" {
                Msg::Patch { patch: req.patch.ok_or_else(|| err("bad_request", "patch needs a patch (a JSON merge patch object)"))?, actor: req.actor.clone() }
            } else {
                Msg::SetState { state: req.state.ok_or_else(|| err("bad_request", "state.set needs a state"))?, actor: req.actor.clone() }
            };
            let before = session.state().clone();
            session.check(&msg).map_err(|e| err("invalid", e))?;
            let applied = apply(session, vec![msg], host, false)?;
            let changed = before.changed_fields(session.state());
            Ok(Handled {
                response: ok_line(id, json!({"rev": session.rev, "changed": changed})),
                change: Some(Change { rev: session.rev, msgs: applied, state_set: true }),
                control: None,
            })
        }
        "msgs" | "keys" => {
            check_rev(session.rev)?;
            if req.apply_effects && !host.effects {
                return Err(err("unsupported", "this host reports effects; it doesn't perform them"));
            }
            let msgs: Vec<Msg> = if req.op == "keys" {
                let script = req.keys.ok_or_else(|| err("bad_request", "keys needs a keys script"))?;
                crate::script::parse(&script, false).map_err(|e| err("bad_keys", e))?
            } else {
                let raw = req.msgs.ok_or_else(|| err("bad_request", "msgs needs a msgs array"))?;
                raw.into_iter()
                    .map(|m| serde_json::from_value::<Msg>(m).map_err(|e| err("bad_request", format!("a message: {e}"))))
                    .collect::<Result<_, _>>()?
            };
            for m in &msgs {
                if matches!(m, Msg::Fixture { .. }) {
                    return Err(err("bad_request", "fixtures are for THC_TUI_KEYS only"));
                }
                session.check(m).map_err(|e| err("invalid", e))?;
            }
            // A set or patch sent as a message is the request's actor's.
            let msgs = msgs
                .into_iter()
                .map(|m| match m {
                    Msg::SetState { state, actor: None } => Msg::SetState { state, actor: req.actor.clone() },
                    Msg::Patch { patch, actor: None } => Msg::Patch { patch, actor: req.actor.clone() },
                    m => m,
                })
                .collect();
            let state_set = false;
            let effects_before = session.pending_effects();
            let applied = apply(session, msgs, host, true)?;
            let effects: Vec<_> = session.pending_effects().into_iter().filter(|e| !effects_before.contains(e)).collect();
            let executed = req.apply_effects && !effects.is_empty();
            if !req.apply_effects {
                // Reported, not performed: the person's terminal stays theirs.
                session.drop_effects();
            }
            let mut result = json!({"rev": session.rev, "effects": effects, "msgs": applied});
            if executed {
                result["executed"] = json!(true);
            }
            Ok(Handled { response: ok_line(id, result), change: Some(Change { rev: session.rev, msgs: applied, state_set }), control: None })
        }
        "aside" => {
            check_rev(session.rev)?;
            let target = req.target.clone().ok_or_else(|| err("bad_request", "aside needs a target"))?;
            // Resolved first, so a bad target says which kind of bad (exit 3, 5 or 6).
            if !req.close {
                if let Err(e) = session.app.resolve_aside(&target) {
                    return Err(aside_err(e));
                }
            }
            let msg = Msg::Aside { target, pin: req.pin, fold: req.fold, close: req.close, actor: req.actor.clone() };
            let applied = apply(session, vec![msg], host, false).map_err(|e| ProtoError { kind: if e.message.contains("isn't beside you") { "not_found" } else { e.kind }, message: e.message })?;
            let key = session.app.ui.sidebar.focused.clone();
            let opened = session.app.ui.sidebar.open.first().map(|p| p.key());
            Ok(Handled {
                response: ok_line(id, json!({"rev": session.rev, "panel": opened, "focused": key, "focus": session.app.ui.focus, "sidebar": session.app.aside_ls()})),
                change: Some(Change { rev: session.rev, msgs: applied, state_set: true }),
                control: None,
            })
        }
        "aside.ls" => reply(ok_line(id, json!({"rev": session.rev, "sidebar": session.app.aside_ls()}))),
        "render" => {
            let (w, h) = (req.w.unwrap_or(session.size.0), req.h.unwrap_or(session.size.1));
            let format = req.format.unwrap_or_else(|| "text".into());
            let r = session.render(w, h, &format).map_err(|e| err("bad_request", e))?;
            reply(ok_line(id, rendered(session.rev, w, h, &format, r)))
        }
        "subscribe" => {
            let frame = match req.frame {
                None | Some(Value::Null) => None,
                Some(f) => Some((
                    f.get("w").and_then(Value::as_u64).map(|v| v as u16),
                    f.get("h").and_then(Value::as_u64).map(|v| v as u16),
                    f.get("format").and_then(Value::as_str).unwrap_or("text").to_string(),
                )),
            };
            let sub = Subscription { msgs: req.with_msgs.unwrap_or(true), frame, state: req.with_state };
            Ok(Handled { response: ok_line(id, json!({"rev": session.rev, "subscribed": true})), change: None, control: Some(Control::Subscribe(sub)) })
        }
        "unsubscribe" => Ok(Handled { response: ok_line(id, json!({"rev": session.rev, "subscribed": false})), change: None, control: Some(Control::Unsubscribe) }),
        "trace.get" => {
            let (from, lines) = session.trace(req.since_rev, req.all).map_err(|e| err("trimmed", e))?;
            reply(ok_line(id, json!({"rev": session.rev, "from_rev": from, "trace": lines})))
        }
        "trace.checkpoint" => {
            session.checkpoint_saved();
            reply(ok_line(id, json!({"rev": session.rev})))
        }
        other => Err(err("unknown_op", format!("unknown op {other:?} · hello lists them"))),
    }
}

fn aside_err(e: crate::sidebar_app::AsideError) -> ProtoError {
    use crate::sidebar_app::AsideError;
    let kind = match &e {
        AsideError::NotFound(_) => "not_found",
        AsideError::Ambiguous(..) => "ambiguous",
        AsideError::Invalid(_) => "invalid",
    };
    ProtoError { kind, message: e.message() }
}

/// Applies messages, a tick to the wall clock first when the host keeps time. The messages as
/// applied (trace JSON, without `_rev`).
fn apply(session: &mut Session, msgs: Vec<Msg>, host: &Host, tick: bool) -> Result<Vec<Value>, ProtoError> {
    let mut all = Vec::new();
    if host.clock && tick {
        let (now_ms, utc_offset_min) = crate::runtime_effects::wall_clock();
        if now_ms > session.app.ui.now_ms {
            all.push(Msg::Tick { now_ms, utc_offset_min });
        }
    }
    all.extend(msgs);
    let mut applied = Vec::new();
    for m in all {
        let v = serde_json::to_value(&m).expect("Msg serializes");
        session.apply(m).map_err(|e| err("invalid", e))?;
        applied.push(v);
    }
    Ok(applied)
}

fn rendered(rev: u64, w: u16, h: u16, format: &str, r: crate::session::Rendered) -> Value {
    let mut v = json!({"rev": rev, "w": w, "h": h, "format": format, "cursor": r.cursor});
    if let Some(f) = r.frame {
        v["frame"] = json!(f);
    }
    if let Some(rows) = r.rows {
        v["rows"] = rows;
    }
    v
}

/// The `{event: "state"}` line a subscriber gets for a change.
pub fn event_line(session: &mut Session, change: &Change, sub: &Subscription, source: &str) -> String {
    let mut v = json!({"event": "state", "rev": change.rev, "source": source});
    if change.state_set {
        v["state_set"] = json!(true);
    }
    if sub.msgs {
        v["msgs"] = json!(change.msgs);
    }
    if let Some((w, h, format)) = &sub.frame {
        let (w, h) = (w.unwrap_or(session.size.0), h.unwrap_or(session.size.1));
        if let Ok(r) = session.render(w, h, format) {
            v["frame"] = rendered(change.rev, w, h, format, r);
        }
    }
    if sub.state {
        v["state"] = session.state().to_json_without_history();
    }
    v.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(tag: &str) -> (crate::fuzz::Scratch, Session) {
        let (s, vault) = crate::fuzz::scratch(tag);
        crate::SNAPSHOT.with(|s| s.set(true));
        let mut app = crate::app::App::new(vault).unwrap();
        app.daemon_live = false;
        (s, Session::new(app, (100, 30)))
    }

    fn ask(s: &mut Session, req: Value) -> Value {
        let host = Host { clock: false, effects: false };
        serde_json::from_str(&handle(s, &req.to_string(), &host).response).unwrap()
    }

    #[test]
    fn every_op_answers() {
        let (_x, mut s) = session("proto-ops");
        let hello = ask(&mut s, json!({"id": 1, "op": "hello"}));
        assert_eq!(hello["id"], 1);
        assert_eq!(hello["result"]["ops"].as_array().unwrap().len(), OPS.len());
        let got = ask(&mut s, json!({"op": "state.get", "history": false}));
        assert!(got["result"]["state"].get("history").is_none());
        assert_eq!(got["result"]["state"]["view"], "today");
        let set = ask(&mut s, json!({"op": "state.set", "state": {"view": "log"}, "actor": "claude"}));
        assert_eq!(set["result"]["rev"], 1, "{set}");
        assert!(set["result"]["changed"].as_array().unwrap().contains(&json!("view")));
        let p = ask(&mut s, json!({"op": "patch", "patch": {"view": "tasks"}, "if_rev": 1}));
        assert_eq!(p["result"]["rev"], 2, "{p}");
        let stale = ask(&mut s, json!({"op": "patch", "patch": {"view": "pages"}, "if_rev": 1}));
        assert_eq!(stale["error"]["kind"], "stale");
        let bad = ask(&mut s, json!({"op": "patch", "patch": {"veiw": "pages"}}));
        assert_eq!(bad["error"]["kind"], "invalid");
        let keys = ask(&mut s, json!({"op": "keys", "keys": "1"}));
        assert_eq!(keys["result"]["rev"], 3, "{keys}");
        assert_eq!(keys["result"]["msgs"][0]["key"], "1");
        let msgs = ask(&mut s, json!({"op": "msgs", "msgs": [{"msg": "key", "key": "<tab>"}, {"msg": "resize", "w": 90, "h": 28}]}));
        assert_eq!(msgs["result"]["rev"], 5, "{msgs}");
        let quit = ask(&mut s, json!({"op": "keys", "keys": "<c-q>"}));
        assert!(quit["result"]["effects"].as_array().is_some(), "{quit}");
        assert!(!s.app.quit, "a reported effect is not performed");
        assert_eq!(ask(&mut s, json!({"op": "keys", "keys": "<oops>"}))["error"]["kind"], "bad_keys");
        assert_eq!(ask(&mut s, json!({"op": "keys", "keys": "<agent:x>"}))["error"]["kind"], "bad_keys");
        let r = ask(&mut s, json!({"op": "render", "w": 80, "h": 24}));
        assert_eq!(r["result"]["frame"].as_str().unwrap().lines().count(), 24);
        let cells = ask(&mut s, json!({"op": "render", "format": "cells"}));
        assert_eq!(cells["result"]["rows"].as_array().unwrap().len(), 28);
        let t = ask(&mut s, json!({"op": "trace.get"}));
        assert!(t["result"]["trace"][0].get("state").is_some());
        let since = ask(&mut s, json!({"op": "trace.get", "since_rev": 4}));
        assert!(since["result"]["trace"].as_array().unwrap().iter().all(|l| l["_rev"].as_u64().unwrap_or(99) > 4), "{since}");
        assert_eq!(ask(&mut s, json!({"op": "trace.checkpoint"}))["result"]["rev"], s.rev);
        assert_eq!(ask(&mut s, json!({"op": "trace.get"}))["result"]["trace"].as_array().unwrap().len(), 1);
        assert_eq!(ask(&mut s, json!({"op": "nope"}))["error"]["kind"], "unknown_op");
        let host = Host { clock: false, effects: false };
        let h = handle(&mut s, r#"{"op":"subscribe","frame":{"w":60,"h":24}}"#, &host);
        assert!(matches!(h.control, Some(Control::Subscribe(Subscription { frame: Some((Some(60), Some(24), _)), .. }))));
        assert!(serde_json::from_str::<Value>(&handle(&mut s, "not json", &host).response).unwrap()["error"]["kind"] == "parse");
    }
}
