//! Layers: hints, highlights, spotlights and walkthroughs drawn over the TUI (caretline-layers).
//!
//! The model is caretline-layers' [`cl::Layers`], kept in the one UI state (`UiState.layers`)
//! beside a walkthrough ([`Tour`]) and changed only by messages: a `layer` message (an op from
//! the `thc ui` socket, or the person's own keys), a tick (expiry), and every edit of the open
//! document (text anchors follow it). Nothing here reads a clock or the vault: time is the
//! state's `now_ms`, so a trace replays to the same layers.
//!
//! Anchors are stable keys, never screen positions. thc's own (`Anchor::Host`) are
//! `{"host": {"kind": "row", "key": <node id>}}` (a list row in any view, a sidebar list
//! panel's row, a document line), `ui:<element>` (`tab:tasks`, `footer:<action>`, `scope`,
//! `detail`, `calendar`, `panel:<n>:header`), `action:<command id>` (a footer hint or button
//! for that keymap action) and `panel:<n>`. The CLI and the socket also accept the short form
//! `row:<id>`. Text, block and caret anchors resolve in the open document's editor first,
//! then each sidebar document panel (see `layers_ui::editors`).
//!
//! Drawing is `layers_ui.rs`; the agent policy is `[layers] agent_limits` (`off` by default).

use crate::ui_state::UiState;
use caretline_layers as cl;
use caretline_tour as ct;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The content kind of a walkthrough step: `{title?, text, step, of}`.
pub const TOUR: &str = "thc.tour";

/// The agent policy (`[layers] agent_limits` in the config). `off` (the default): no limits,
/// agents push what they like, spotlights included; the person keeps control with Esc (every
/// agent layer goes) and ⌘[ (the newest goes), and thc attributes every agent layer.
/// `defaults`: caretline-layers' `Limits::agent_defaults()` (3 agent layers, 8 s lifetimes,
/// 2 pushes a second, size limits, no spotlight).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentLimits {
    #[default]
    Off,
    Defaults,
}

impl AgentLimits {
    pub fn parse(s: &str) -> AgentLimits {
        match s.trim() {
            "defaults" | "default" | "on" => AgentLimits::Defaults,
            _ => AgentLimits::Off,
        }
    }

    /// The limits `apply` holds agents to.
    pub fn limits(self) -> cl::Limits {
        match self {
            AgentLimits::Defaults => cl::Limits::agent_defaults(),
            AgentLimits::Off => cl::Limits::none(),
        }
    }
}

/// The layers in the UI state, and the walkthrough (caretline-tour's `TourState`). Serialized
/// only when there are some, so states and traces without layers are unchanged.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LayerState {
    #[serde(skip_serializing_if = "stack_is_empty")]
    pub stack: cl::Layers,
    /// The walkthrough: its tour, the step showing, its history, and which tours this device
    /// has seen. Its steps' layers are the `guide` layers in `stack`.
    #[serde(skip_serializing_if = "tour_is_empty")]
    pub tour: ct::TourState,
    /// The agent that started the walkthrough (None: the person or thc): its steps carry its
    /// name, and Esc stops it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tour_actor: Option<String>,
    /// The host patches the steps just entered asked for (`host`), for the session to apply
    /// after the message. Never serialized: drained within the message.
    #[serde(skip)]
    pub host_patches: Vec<Value>,
    /// A walkthrough ended within the message (its seen-state goes to the device's cache).
    #[serde(skip)]
    pub ended: bool,
}

fn stack_is_empty(l: &cl::Layers) -> bool {
    *l == cl::Layers::default()
}

fn tour_is_empty(t: &ct::TourState) -> bool {
    *t == ct::TourState::default()
}

impl LayerState {
    pub fn is_empty(&self) -> bool {
        stack_is_empty(&self.stack) && tour_is_empty(&self.tour) && self.tour_actor.is_none()
    }

    /// Something to draw.
    pub fn showing(&self) -> bool {
        !self.stack.layers.is_empty() && !self.stack.hidden
    }

    /// A walkthrough is running.
    pub fn touring(&self) -> bool {
        self.tour.running()
    }

    /// Something waits on the clock (a layer with a lifetime, a step's nudge or `after_ms`).
    pub fn timed(&self) -> bool {
        self.stack.layers.iter().any(|l| l.ttl_ms.is_some_and(|t| t < u64::MAX / 2))
            || self.tour.current().is_some_and(|s| s.nudge.is_some() || s.advance.is_some())
    }

    pub fn has_text_anchors(&self) -> bool {
        let text = |a: &cl::Anchor| matches!(a.unscoped(), cl::Anchor::Text { .. });
        self.stack.layers.iter().any(|l| l.anchor.iter().any(text))
            || self.tour.tour.as_ref().is_some_and(|t| t.steps.iter().any(|s| s.layers.iter().any(|l| l.anchor.iter().filter_map(ct::StepAnchor::anchor).any(text))))
    }

    /// An agent's layer, or an agent's walkthrough, is showing.
    pub fn has_agent_layers(&self) -> bool {
        self.stack.layers.iter().any(|l| l.owner.is_agent()) || (self.touring() && self.tour_actor.is_some())
    }

    /// The clock moved: expired layers go.
    pub fn expire(&mut self, now_ms: u64) {
        cl::expire(&mut self.stack, now_ms);
    }

    /// Esc: every agent's layer goes, and an agent's walkthrough stops. True: some did.
    pub fn dismiss_agents(&mut self, now_ms: u64) -> bool {
        let n = self.stack.layers.len();
        self.stack.layers.retain(|l| !l.owner.is_agent());
        let mut did = self.stack.layers.len() != n;
        if self.touring() && self.tour_actor.is_some() {
            if let Ok(fx) = ct::apply(&mut self.tour, ct::TourOp::Stop, now_ms) {
                self.effects(fx, now_ms);
            }
            did = true;
        }
        did
    }

    /// ⌘[: the newest agent layer goes, one step back. True: one did.
    pub fn back_agent_step(&mut self) -> bool {
        let Some(i) = (0..self.stack.layers.len()).filter(|&i| self.stack.layers[i].owner.is_agent()).max_by_key(|&i| (self.stack.layers[i].since_ms, i)) else {
            return false;
        };
        self.stack.layers.remove(i);
        true
    }

    /// The edit `changes` moved the open document's text: text anchors follow it, the layers
    /// showing and a walkthrough's steps still to come alike.
    pub fn observe(&mut self, changes: &caretline::helix::ChangeSet, edited: cl::Edited, now_ms: u64) {
        if let Some(t) = self.tour.tour.as_mut() {
            map_steps(&mut t.steps, edited, changes);
        }
        cl::observe(&mut self.stack, edited, Some(changes), now_ms);
    }

    /// What the walkthrough's reducer asked for: its steps' layers replace the guide layers,
    /// its host patches wait for the session, and an ended tour leaves only its seen-state.
    pub fn effects(&mut self, fx: Vec<ct::TourEffect>, now_ms: u64) {
        for e in fx {
            match e {
                ct::TourEffect::Host { patch } => self.host_patches.push(patch),
                ct::TourEffect::Layers { layers } => {
                    let _ = ct::replace_guide(&mut self.stack, layers, now_ms);
                }
                ct::TourEffect::Step { .. } => {}
                ct::TourEffect::Ended { .. } => {
                    self.tour.tour = None;
                    self.tour_actor = None;
                    self.ended = true;
                }
            }
        }
    }
}

/// The tours this device has seen (caretline-tour's seen-state), kept beside the vault's caches:
/// a walkthrough someone finished or stopped isn't offered again until its version is.
pub fn load_seen(cache: &std::path::Path) -> std::collections::BTreeMap<String, ct::Seen> {
    std::fs::read(cache.join("tours-seen.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save_seen(cache: &std::path::Path, seen: &std::collections::BTreeMap<String, ct::Seen>) {
    if crate::SNAPSHOT.with(|s| s.get()) {
        return;
    }
    if let Ok(b) = serde_json::to_vec(seen) {
        let _ = std::fs::write(cache.join("tours-seen.json"), b);
    }
}

/// A walkthrough's steps through an edit: each step layer's text anchors move as a layer's
/// would. A layer whose anchors all went (their text deleted) points at the screen's centre.
fn map_steps(steps: &mut [ct::Step], edited: cl::Edited, changes: &caretline::helix::ChangeSet) {
    let text = |a: &ct::StepAnchor| a.anchor().is_some_and(|a| matches!(a.unscoped(), cl::Anchor::Text { .. }));
    if !steps.iter().any(|s| s.layers.iter().any(|l| l.anchor.iter().any(text))) {
        return;
    }
    let mut tmp = cl::Layers::default();
    for (i, s) in steps.iter().enumerate() {
        for (k, l) in s.layers.iter().enumerate() {
            let mut t = cl::Layer::new(cl::Anchor::Caret);
            t.id = format!("{i}/{k}");
            t.anchor = l.anchor.iter().filter_map(ct::StepAnchor::anchor).cloned().collect();
            t.ring = Some(cl::Ring::default());
            tmp.layers.push(t);
        }
    }
    cl::map_anchors(&mut tmp, edited, changes);
    for (i, s) in steps.iter_mut().enumerate() {
        for (k, l) in s.layers.iter_mut().enumerate() {
            if !l.anchor.iter().any(text) {
                continue;
            }
            let mapped = tmp.get(&format!("{i}/{k}")).map_or_else(|| vec![cl::Anchor::Screen(cl::ScreenPos::Center)], |t| t.anchor.clone());
            l.anchor = mapped.into_iter().map(ct::StepAnchor::At).collect();
        }
    }
}

/// thc's ops on the `thc ui` socket, beside the protocol's own.
pub const OPS: &[&str] = &[
    "hint.show",
    "hint.clear",
    "hint.hide",
    "highlight",
    "focus",
    "layer.push",
    "layer.update",
    "layer.pop",
    "layer.ls",
    "tour.start",
    "tour.next",
    "tour.back",
    "tour.stop",
    "tour.step",
    "tour.restart",
    "tour.list",
];

/// A refusal as `(reason, detail)`.
pub fn refusal(reason: cl::Reason, detail: impl Into<String>) -> cl::Refusal {
    cl::Refusal {
        reason,
        detail: detail.into(),
    }
}

fn invalid(detail: impl Into<String>) -> cl::Refusal {
    refusal(cl::Reason::Invalid, detail)
}

/// `row:<id>`, `ui:<el>`, `action:<id>` and `panel:<n>` as host anchors; a JSON anchor as is.
pub fn anchor_from(v: &Value) -> Result<Vec<cl::Anchor>, cl::Refusal> {
    let one = |v: &Value| -> Result<cl::Anchor, cl::Refusal> {
        if let Some(s) = v.as_str() {
            return parse_short(s).ok_or_else(|| invalid(format!("anchor {s:?}: row:<id>, ui:<element>, action:<command>, panel:<n>, caret, or a JSON anchor")));
        }
        serde_json::from_value(v.clone()).map_err(|e| invalid(format!("anchor: {e}")))
    };
    match v {
        Value::Array(a) if !a.is_empty() => a.iter().map(one).collect(),
        Value::Array(_) => Err(invalid("an anchor list needs at least one anchor")),
        v => Ok(vec![one(v)?]),
    }
}

/// The short anchor forms.
pub fn parse_short(s: &str) -> Option<cl::Anchor> {
    let s = s.trim();
    // `…@main`, `…@panel:2`: a text, block or caret anchor in that view only.
    if let Some((a, view)) = s.rsplit_once('@').filter(|(a, v)| !v.is_empty() && !a.starts_with("row:") && !a.starts_with("ui:") && !a.starts_with("action:")) {
        let inner = parse_short(a)?;
        return matches!(inner, cl::Anchor::Text { .. } | cl::Anchor::Block(_) | cl::Anchor::Caret).then(|| cl::Anchor::scoped(view, inner));
    }
    if s == "caret" {
        return Some(cl::Anchor::Caret);
    }
    if let Some(p) = s.strip_prefix("screen:") {
        return serde_json::from_value(json!({"screen": p})).ok();
    }
    if let Some(b) = s.strip_prefix("block:") {
        return b.parse().ok().map(cl::Anchor::Block);
    }
    if let Some(r) = s.strip_prefix("text:") {
        let (f, t) = r.split_once("..").or_else(|| r.split_once('-'))?;
        return Some(cl::Anchor::Text {
            from: f.parse().ok()?,
            to: t.parse().ok()?,
        });
    }
    let (kind, key) = s.split_once(':')?;
    matches!(kind, "row" | "ui" | "action" | "panel")
        .then(|| cl::Anchor::Host {
            kind: kind.into(),
            key: key.into(),
        })
        .filter(|_| !key.is_empty())
}

/// A request with its short anchors (`"row:abc"`) turned into JSON ones, for the crate's
/// strict parser.
fn normalize(req: &Value) -> Result<Value, cl::Refusal> {
    let mut v = req.clone();
    for k in ["anchor", "avoid"] {
        if let Some(a) = v.get(k).cloned() {
            v[k] = serde_json::to_value(anchor_from(&a)?).unwrap_or_default();
        }
        if let Some(a) = v.get("layer").and_then(|l| l.get(k)).cloned() {
            if !a.is_array() || a.as_array().is_some_and(|x| x.iter().any(Value::is_string)) {
                v["layer"][k] = serde_json::to_value(anchor_from(&a)?).unwrap_or_default();
            }
        }
    }
    Ok(v)
}

fn str_field(req: &Value, k: &str) -> Option<String> {
    req.get(k).and_then(Value::as_str).map(str::to_string)
}

/// What a `layer` message did, for the reply: the crate's `Applied`, or a listing.
pub enum Done {
    Applied(cl::Applied),
    Listed(Value),
}

/// Applies one layer request to the state. Pure: the state, the request, the actor (None: the
/// person or thc), the policy. The `actor` given wins over one in the request.
pub fn request(
    ui: &mut UiState,
    req: &Value,
    actor: Option<&str>,
    limits: &cl::Limits,
) -> Result<Done, cl::Refusal> {
    let op = req
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("a layer request needs an op"))?
        .to_string();
    let actor = actor
        .map(str::to_string)
        .or_else(|| str_field(req, "actor"))
        .filter(|a| !a.is_empty());
    let mut req = normalize(req)?;
    if let Some(m) = req.as_object_mut() {
        m.remove("op");
        m.remove("if_rev");
        m.remove("actor");
        if let Some(a) = &actor {
            m.insert("actor".into(), json!(a));
        }
    }
    let now = ui.now_ms;
    let st = &mut ui.layers;
    let crate_op = |name: &str, req: &Value, st: &mut LayerState| -> Result<Done, cl::Refusal> {
        let (parsed, who) = cl::ops::parse(name, req)?;
        match parsed {
            cl::ops::Request::List => Ok(Done::Listed(list(st))),
            cl::ops::Request::Apply(op) => {
                let a = cl::apply(&mut st.stack, op, who.as_deref(), now, limits)?;
                Ok(Done::Applied(a))
            }
        }
    };
    match op.as_str() {
        "hint.show" | "layer.push" | "layer.update" | "layer.pop" | "hint.hide" => {
            crate_op(&op, &req, st)
        }
        "hint.clear" => {
            if req.get("layer").is_none() && req.get("all").is_none() {
                req["all"] = json!(true);
            }
            crate_op("hint.hide", &req, st)
        }
        "layer.ls" | "layer.list" => Ok(Done::Listed(list(st))),
        "highlight" | "focus" => {
            let anchor = req
                .get("anchor")
                .cloned()
                .ok_or_else(|| invalid(format!("{op} needs an anchor")))?;
            let mut layer = json!({"anchor": anchor, "ring": {}});
            if let Some(t) = req.get("ttl_ms") {
                layer["ttl_ms"] = t.clone();
            }
            if op == "focus" {
                layer["spotlight"] = json!({});
                if let Some(text) = str_field(&req, "text") {
                    layer["content"] = json!({"kind": cl::HINT, "data": {"text": text, "title": req.get("title").cloned().unwrap_or(Value::Null)}});
                    if layer["content"]["data"]["title"].is_null() {
                        layer["content"]["data"]
                            .as_object_mut()
                            .unwrap()
                            .remove("title");
                    }
                    layer["arrow"] = json!(true);
                }
            }
            let mut push = json!({"layer": layer});
            if let Some(a) = &actor {
                push["actor"] = json!(a);
            }
            let done = crate_op("layer.push", &push, st)?;
            if op == "focus" {
                reveal(ui, &anchor_from(&anchor)?);
            }
            Ok(done)
        }
        "tour.start" => {
            let tour = tour_from(&req)?;
            let dims = tour.steps.iter().any(|s| s.layers.iter().any(|l| l.place.spotlight.is_some()));
            if dims && actor.is_some() && limits.agent_dim != cl::AgentDim::Always {
                return Err(refusal(cl::Reason::DimNotAllowed, "agents can't dim the screen unless the person allows it"));
            }
            let fx = ct::apply(&mut st.tour, ct::TourOp::Start(Box::new(tour)), now).map_err(tour_err)?;
            st.tour_actor = actor.clone();
            st.effects(fx, now);
            Ok(Done::Applied(cl::Applied { layer: st.stack.layers.iter().find(|l| l.owner == cl::Owner::Guide).map(|l| l.id.clone()), popped: vec![] }))
        }
        "tour.list" => Ok(Done::Listed(ct::ops::list(&[], &st.tour))),
        "tour.next" | "tour.back" | "tour.stop" | "tour.step" | "tour.restart" => {
            if !st.touring() && op != "tour.restart" {
                return Err(refusal(cl::Reason::NotFound, "no walkthrough is showing"));
            }
            if actor.is_some() && st.tour_actor != actor {
                return Err(refusal(cl::Reason::NotAllowed, "the walkthrough isn't this actor's"));
            }
            let op = match op.as_str() {
                "tour.step" => match req.get("to").and_then(Value::as_str) {
                    Some("next") => ct::TourOp::Next,
                    Some("back") => ct::TourOp::Back,
                    Some(to) if !to.is_empty() => ct::TourOp::To(to.into()),
                    _ => return Err(invalid("tour.step: `to` is a step id, \"next\" or \"back\"")),
                },
                "tour.next" => ct::TourOp::Next,
                "tour.back" => ct::TourOp::Back,
                "tour.restart" => ct::TourOp::Restart,
                _ => ct::TourOp::Stop,
            };
            tour_op(st, op, now)
        }
        other => Err(invalid(format!("unknown layer op {other:?}"))),
    }
}

/// Moves the walkthrough (`tour.next`, `tour.back`, `tour.stop`): the person's keys and
/// buttons, and the socket. Back on the first step does nothing.
pub fn step(st: &mut LayerState, op: &str, now: u64, _limits: &cl::Limits) -> Result<Done, cl::Refusal> {
    let op = match op {
        "tour.next" => ct::TourOp::Next,
        "tour.back" => ct::TourOp::Back,
        _ => ct::TourOp::Stop,
    };
    tour_op(st, op, now)
}

fn tour_op(st: &mut LayerState, op: ct::TourOp, now: u64) -> Result<Done, cl::Refusal> {
    let before: Vec<String> = st.stack.layers.iter().map(|l| l.id.clone()).collect();
    match ct::apply(&mut st.tour, op, now) {
        Ok(fx) => st.effects(fx, now),
        Err(e) if e.reason == ct::TourReason::NoBack => {}
        Err(e) => return Err(tour_err(e)),
    }
    let popped = before.into_iter().filter(|id| st.stack.get(id).is_none()).collect();
    Ok(Done::Applied(cl::Applied { layer: st.stack.layers.iter().find(|l| l.owner == cl::Owner::Guide).map(|l| l.id.clone()), popped }))
}

fn tour_err(e: ct::TourError) -> cl::Refusal {
    let reason = match e.reason {
        ct::TourReason::NotFound | ct::TourReason::NotRunning => cl::Reason::NotFound,
        _ => cl::Reason::Invalid,
    };
    refusal(reason, e.detail)
}

/// A walkthrough from a request: caretline-tour's own (`{"tour": {id, version, title, kind,
/// step[]}}`), or thc's short form (`{"steps": [{anchor, title?, text, host?}], "spotlight"?,
/// "title"?}`), whose steps show thc's step box (dots, back and next) with an arrow and a ring.
fn tour_from(req: &Value) -> Result<ct::Tour, cl::Refusal> {
    if let Some(t) = req.get("tour") {
        // thc's short anchors (`row:<id>`, `ui:tab:tasks`…) in the steps and their layers.
        let mut t = t.clone();
        let short = |a: &mut Value| -> Result<(), cl::Refusal> {
            let has_short = a.is_string() || a.as_array().is_some_and(|x| x.iter().any(Value::is_string));
            if has_short {
                let list: Vec<Value> = match a.take() {
                    Value::Array(x) => x,
                    v => vec![v],
                };
                let mut out = Vec::new();
                for v in list {
                    out.push(if v.is_string() { serde_json::to_value(anchor_from(&v)?).unwrap_or_default()[0].clone() } else { v });
                }
                *a = Value::Array(out);
            }
            Ok(())
        };
        for key in ["step", "steps"] {
            for st in t.get_mut(key).and_then(Value::as_array_mut).into_iter().flatten() {
                if let Some(a) = st.get_mut("anchor") {
                    short(a)?;
                }
                for l in st.get_mut("layers").and_then(Value::as_array_mut).into_iter().flatten() {
                    if let Some(a) = l.get_mut("anchor") {
                        short(a)?;
                    }
                }
            }
        }
        return match ct::ops::parse("tour.start", &json!({"tour": t}), &[]).map_err(tour_err)? {
            ct::ops::Request::Apply(ct::TourOp::Start(t)) => Ok(*t),
            _ => Err(invalid("tour.start: a tour")),
        };
    }
    let steps = req.get("steps").and_then(Value::as_array).ok_or_else(|| invalid("tour.start needs steps: [{anchor, title?, text}] (or a caretline-tour `tour`)"))?;
    if steps.is_empty() {
        return Err(invalid("a walkthrough needs at least one step"));
    }
    let spotlight = req.get("spotlight").and_then(Value::as_bool).unwrap_or(false);
    let mut out = Vec::new();
    for (i, s) in steps.iter().enumerate() {
        let anchor = anchor_from(s.get("anchor").ok_or_else(|| invalid("a step needs an anchor"))?)?;
        let mut data = json!({"text": str_field(s, "text").unwrap_or_default()});
        if let Some(t) = str_field(s, "title") {
            data["title"] = json!(t);
        }
        let mut step = json!({"id": str_field(s, "id").unwrap_or_else(|| format!("s{}", i + 1)), "anchor": anchor, "data": data, "place": {"arrow": true, "ring": true, "spotlight": spotlight}});
        for k in ["host", "narration", "advance"] {
            if let Some(v) = s.get(k) {
                step[k] = v.clone();
            }
        }
        out.push(step);
    }
    let tour = json!({"id": str_field(req, "id").unwrap_or_else(|| "thc.walkthrough".into()), "version": 1, "title": str_field(req, "title").unwrap_or_default(), "kind": TOUR, "step": out});
    ct::parse_json(&tour.to_string()).map_err(|e| invalid(format!("tour.start: {e}")))
}

/// `focus`'s ordinary reveal: a row anchor selects that row (the list follows it). Text in the
/// document is revealed by the spotlight's edge chip; the person's caret never moves.
fn reveal(ui: &mut UiState, anchor: &[cl::Anchor]) {
    if let Some(cl::Anchor::Host { kind, key }) = anchor.first() {
        let in_doc = ui.view == crate::app::View::Journal || (ui.view == crate::app::View::Pages && ui.page_open.is_some());
        if kind == "row" && !in_doc {
            ui.selected = Some(key.clone());
        }
    }
}

/// `layer.ls`: every layer, and the walkthrough.
pub fn list(st: &LayerState) -> Value {
    let mut v = cl::ops::list(&st.stack);
    if st.touring() {
        let mut t = ct::ops::reply(&st.tour);
        t["actor"] = json!(st.tour_actor);
        v["tour"] = t;
    }
    v
}

/// thc's ops' JSON Schemas (`thc schema --proto`), beside the protocol's.
pub fn schema() -> Value {
    let anchor = json!({"description": "row:<id> | ui:<element> | action:<command> | panel:<n> | caret | text:<from>..<to> | block:<n>, or a caretline-layers anchor object; a list is fallbacks", "oneOf": [{"type": "string"}, {"type": "object"}, {"type": "array"}]});
    json!({
        "hint.show": {"type": "object", "required": ["anchor", "text"], "properties": {"anchor": anchor, "avoid": anchor, "text": {"type": "string"}, "title": {"type": "string"}, "ttl_ms": {"type": "integer"}, "place": {"type": "array", "items": {"enum": ["below", "above", "right", "left"]}}, "arrow": {"type": "boolean"}, "ring": {"type": "boolean"}, "actor": {"type": "string"}}},
        "hint.clear": {"type": "object", "properties": {"layer": {"type": "string"}, "all": {"type": "boolean"}, "actor": {"type": "string"}}},
        "highlight": {"type": "object", "required": ["anchor"], "properties": {"anchor": anchor, "ttl_ms": {"type": "integer"}, "actor": {"type": "string"}}},
        "focus": {"type": "object", "required": ["anchor"], "properties": {"anchor": anchor, "text": {"type": "string"}, "title": {"type": "string"}, "ttl_ms": {"type": "integer"}, "actor": {"type": "string"}}},
        "layer.push": {"type": "object", "required": ["layer"], "properties": {"layer": {"type": "object", "description": "a caretline-layers Layer"}, "actor": {"type": "string"}}},
        "layer.update": {"type": "object", "required": ["layer"], "properties": {"layer": {"type": "object"}, "actor": {"type": "string"}}},
        "layer.pop": {"type": "object", "properties": {"layer": {"type": "string"}, "owner": {"type": "string"}, "all": {"type": "boolean"}, "actor": {"type": "string"}}},
        "layer.ls": {"type": "object", "properties": {}},
        "tour.start": {"type": "object", "description": "thc's short form (steps), or a caretline-tour tour (see caretline_tour_ops)", "properties": {"steps": {"type": "array", "items": {"type": "object", "required": ["anchor", "text"], "properties": {"anchor": anchor, "title": {"type": "string"}, "text": {"type": "string"}, "host": {"type": "object", "description": "a UI state merge patch applied on entering the step"}, "advance": {"type": "object"}}}}, "tour": {"oneOf": [{"type": "object"}, {"type": "string"}]}, "spotlight": {"type": "boolean"}, "title": {"type": "string"}, "actor": {"type": "string"}}},
        "tour.next": {"type": "object"}, "tour.back": {"type": "object"}, "tour.stop": {"type": "object"}, "tour.restart": {"type": "object"}, "tour.list": {"type": "object"},
        "tour.step": {"type": "object", "required": ["to"], "properties": {"to": {"type": "string"}}},
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ui() -> UiState {
        let mut u = UiState::default();
        u.now_ms = 1_000;
        u
    }

    #[test]
    fn short_anchors_parse() {
        assert_eq!(
            parse_short("row:abc"),
            Some(cl::Anchor::Host {
                kind: "row".into(),
                key: "abc".into()
            })
        );
        assert_eq!(
            parse_short("ui:tab:tasks"),
            Some(cl::Anchor::Host {
                kind: "ui".into(),
                key: "tab:tasks".into()
            })
        );
        assert_eq!(
            parse_short("text:4..9"),
            Some(cl::Anchor::Text { from: 4, to: 9 })
        );
        assert_eq!(parse_short("caret"), Some(cl::Anchor::Caret));
        assert_eq!(parse_short("text:4..9@panel:2"), Some(cl::Anchor::scoped("panel:2", cl::Anchor::Text { from: 4, to: 9 })));
        assert_eq!(parse_short("row:a@b"), Some(cl::Anchor::Host { kind: "row".into(), key: "a@b".into() }));
        assert_eq!(parse_short("nope:x"), None);
        assert_eq!(parse_short("row:"), None);
    }

    #[test]
    fn hints_push_and_clear() {
        let mut u = ui();
        let l = AgentLimits::Off.limits();
        let d = request(
            &mut u,
            &json!({"op": "hint.show", "anchor": "row:n1", "text": "hello"}),
            Some("claude"),
            &l,
        )
        .unwrap();
        let Done::Applied(a) = d else { panic!() };
        assert_eq!(a.layer.as_deref(), Some("L-1"));
        assert!(u.layers.stack.layers[0].owner.is_agent());
        // No forced lifetime when the limits are off.
        assert!(!u.layers.timed());
        request(&mut u, &json!({"op": "hint.clear"}), Some("claude"), &l).unwrap();
        assert!(u.layers.stack.layers.is_empty());
    }

    #[test]
    fn limits_hold_agents_when_on() {
        let mut u = ui();
        let l = AgentLimits::Defaults.limits();
        u.now_ms += 1_000;
        for i in 0..2 {
            request(
                &mut u,
                &json!({"op": "hint.show", "anchor": format!("row:n{i}"), "text": "x"}),
                Some("a"),
                &l,
            )
            .unwrap();
        }
        // The rate limit: a third within the same second is refused.
        let e = request(
            &mut u,
            &json!({"op": "hint.show", "anchor": "row:n9", "text": "x"}),
            Some("a"),
            &l,
        )
        .err()
        .unwrap();
        assert_eq!(e.reason, cl::Reason::RateLimited);
        // A fourth agent layer replaces that agent's oldest.
        for i in 2..4 {
            u.now_ms += 1_000;
            request(
                &mut u,
                &json!({"op": "hint.show", "anchor": format!("row:n{i}"), "text": "x"}),
                Some("a"),
                &l,
            )
            .unwrap();
        }
        assert_eq!(u.layers.stack.layers.len(), 3);
        // The default lifetime expires them.
        assert!(u.layers.timed());
        u.layers.expire(u.now_ms + 60_000);
        assert!(u.layers.stack.layers.is_empty());
        // Dimming is the person's to allow under the defaults.
        let e = request(
            &mut u,
            &json!({"op": "focus", "anchor": "row:n1"}),
            Some("a"),
            &l,
        )
        .err()
        .unwrap();
        assert_eq!(e.reason, cl::Reason::DimNotAllowed);
    }

    #[test]
    fn a_walkthrough_steps_and_stops() {
        let mut u = ui();
        let l = AgentLimits::Off.limits();
        let steps = json!([{"anchor": "row:a", "text": "one"}, {"anchor": "row:b", "text": "two"}, {"anchor": "ui:tab:tasks", "text": "three"}]);
        request(
            &mut u,
            &json!({"op": "tour.start", "steps": steps}),
            Some("claude"),
            &l,
        )
        .unwrap();
        let data = |u: &UiState| u.layers.stack.layers.iter().find(|l| l.owner == cl::Owner::Guide).unwrap().content.clone().unwrap().data;
        assert_eq!(data(&u)["at"], 1);
        step(&mut u.layers, "tour.next", 0, &l).ok();
        assert_eq!(data(&u)["at"], 2);
        step(&mut u.layers, "tour.back", 0, &l).ok();
        step(&mut u.layers, "tour.back", 0, &l).ok();
        assert_eq!(data(&u)["at"], 1);
        for _ in 0..3 {
            step(&mut u.layers, "tour.next", 0, &l).ok();
        }
        assert!(!u.layers.touring() && u.layers.stack.layers.is_empty());
        assert!(u.layers.tour.seen.contains_key("thc.walkthrough"), "seen-state kept");
        // Another agent can't move it.
        request(
            &mut u,
            &json!({"op": "tour.start", "steps": steps}),
            Some("claude"),
            &l,
        )
        .unwrap();
        assert!(request(&mut u, &json!({"op": "tour.next"}), Some("other"), &l).is_err());
    }

    #[test]
    fn esc_and_back_take_agent_layers() {
        let mut u = ui();
        let l = AgentLimits::Off.limits();
        request(
            &mut u,
            &json!({"op": "hint.show", "anchor": "row:a", "text": "a"}),
            Some("x"),
            &l,
        )
        .unwrap();
        u.now_ms += 5;
        request(
            &mut u,
            &json!({"op": "highlight", "anchor": "row:b"}),
            Some("x"),
            &l,
        )
        .unwrap();
        request(
            &mut u,
            &json!({"op": "highlight", "anchor": "row:c"}),
            None,
            &l,
        )
        .unwrap();
        assert!(u.layers.back_agent_step());
        assert_eq!(u.layers.stack.layers.len(), 2);
        assert!(
            u.layers.stack.layers.iter().any(|l| l.content.is_some()),
            "the newer highlight went first"
        );
        assert!(u.layers.dismiss_agents(0));
        assert_eq!(u.layers.stack.layers.len(), 1, "the person's own stays");
    }

    #[test]
    fn the_state_round_trips_and_stays_out_of_an_empty_state() {
        let mut u = ui();
        assert!(u.to_json().get("layers").is_none());
        let l = AgentLimits::Off.limits();
        request(
            &mut u,
            &json!({"op": "hint.show", "anchor": ["row:a", "caret"], "title": "T", "text": "a"}),
            Some("x"),
            &l,
        )
        .unwrap();
        let j = u.to_json();
        assert!(j.get("layers").is_some());
        assert_eq!(UiState::parse(&j).unwrap(), u);
    }
}
