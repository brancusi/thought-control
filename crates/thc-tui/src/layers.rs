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
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The content kind of a walkthrough step: `{title?, text, step, of}`.
pub const TOUR: &str = "thc.tour";
/// The id of the walkthrough's step layer.
pub const TOUR_ID: &str = "tour";

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

/// One step of a walkthrough.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TourStep {
    /// Fallbacks, as a layer's.
    pub anchor: Vec<cl::Anchor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub text: String,
}

/// A walkthrough: its steps, the one showing, and who started it. The step showing is the
/// layer `tour`; F2 (or its `next ›` button) moves on, ⇧F2 back, F3 or Esc stops.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tour {
    pub steps: Vec<TourStep>,
    pub at: usize,
    /// The agent that started it (None: the person or thc).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// Each step dims everything but itself.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub spotlight: bool,
}

/// The layers in the UI state. Serialized only when there are some, so states and traces
/// without layers are unchanged.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LayerState {
    #[serde(skip_serializing_if = "stack_is_empty")]
    pub stack: cl::Layers,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tour: Option<Tour>,
}

fn stack_is_empty(l: &cl::Layers) -> bool {
    *l == cl::Layers::default()
}

impl LayerState {
    pub fn is_empty(&self) -> bool {
        stack_is_empty(&self.stack) && self.tour.is_none()
    }

    /// Something to draw.
    pub fn showing(&self) -> bool {
        !self.stack.layers.is_empty() && !self.stack.hidden
    }

    /// Something waits on the clock (a layer with a lifetime).
    pub fn timed(&self) -> bool {
        self.stack
            .layers
            .iter()
            .any(|l| l.ttl_ms.is_some_and(|t| t < u64::MAX / 2))
    }

    pub fn has_text_anchors(&self) -> bool {
        self.stack.layers.iter().any(|l| {
            l.anchor
                .iter()
                .any(|a| matches!(a, cl::Anchor::Text { .. }))
        })
    }

    pub fn has_agent_layers(&self) -> bool {
        self.stack.layers.iter().any(|l| l.owner.is_agent())
    }

    /// The clock moved: expired layers go (and a walkthrough whose step went).
    pub fn expire(&mut self, now_ms: u64) {
        if cl::expire(&mut self.stack, now_ms) {
            self.tidy();
        }
    }

    /// A walkthrough without its step layer is over.
    fn tidy(&mut self) {
        if self.tour.is_some() && self.stack.get(TOUR_ID).is_none() {
            self.tour = None;
        }
    }

    /// Esc: every agent's layer goes (and an agent's walkthrough). True: some did.
    pub fn dismiss_agents(&mut self) -> bool {
        let n = self.stack.layers.len();
        self.stack.layers.retain(|l| !l.owner.is_agent());
        self.tidy();
        self.stack.layers.len() != n
    }

    /// ⌘[: the newest agent layer goes, one step back. True: one did.
    pub fn back_agent_step(&mut self) -> bool {
        let Some(i) = (0..self.stack.layers.len())
            .filter(|&i| self.stack.layers[i].owner.is_agent())
            .max_by_key(|&i| (self.stack.layers[i].since_ms, i))
        else {
            return false;
        };
        self.stack.layers.remove(i);
        self.tidy();
        true
    }

    /// The edit `changes` moved the open document's text: text anchors follow it.
    pub fn observe(&mut self, changes: &caretline::helix::ChangeSet, now_ms: u64) {
        if cl::observe(&mut self.stack, Some(changes), now_ms) {
            self.tidy();
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
    if let Some(a) = v.get("anchor").cloned() {
        v["anchor"] = serde_json::to_value(anchor_from(&a)?).unwrap_or_default();
    }
    if let Some(a) = v.get("layer").and_then(|l| l.get("anchor")).cloned() {
        if !a.is_array() || a.as_array().is_some_and(|x| x.iter().any(Value::is_string)) {
            v["layer"]["anchor"] = serde_json::to_value(anchor_from(&a)?).unwrap_or_default();
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
                st.tidy();
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
            let steps: Vec<Value> = req
                .get("steps")
                .and_then(Value::as_array)
                .cloned()
                .ok_or_else(|| invalid("tour.start needs steps: [{anchor, title?, text}]"))?;
            if steps.is_empty() {
                return Err(invalid("a walkthrough needs at least one step"));
            }
            let steps = steps
                .iter()
                .map(|s| {
                    let anchor = anchor_from(
                        s.get("anchor")
                            .ok_or_else(|| invalid("a step needs an anchor"))?,
                    )?;
                    Ok(TourStep {
                        anchor,
                        title: str_field(s, "title"),
                        text: str_field(s, "text").unwrap_or_default(),
                    })
                })
                .collect::<Result<Vec<_>, cl::Refusal>>()?;
            let spotlight = req
                .get("spotlight")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if spotlight && actor.is_some() && limits.agent_dim != cl::AgentDim::Always {
                return Err(refusal(
                    cl::Reason::DimNotAllowed,
                    "agents can't dim the screen unless the person allows it",
                ));
            }
            // A walkthrough already showing is replaced.
            st.stack.layers.retain(|l| l.id != TOUR_ID);
            st.tour = Some(Tour {
                steps,
                at: 0,
                actor: actor.clone(),
                spotlight,
            });
            show_step(st, now, limits)
        }
        "tour.next" | "tour.back" | "tour.stop" => {
            let Some(t) = st.tour.as_ref() else {
                return Err(refusal(cl::Reason::NotFound, "no walkthrough is showing"));
            };
            if actor.is_some() && t.actor != actor {
                return Err(refusal(
                    cl::Reason::NotAllowed,
                    "the walkthrough isn't this actor's",
                ));
            }
            step(st, &op, now, limits)
        }
        other => Err(invalid(format!("unknown layer op {other:?}"))),
    }
}

/// Moves the walkthrough (`tour.next`, `tour.back`, `tour.stop`): the person's keys and
/// buttons, and the socket.
pub fn step(
    st: &mut LayerState,
    op: &str,
    now: u64,
    limits: &cl::Limits,
) -> Result<Done, cl::Refusal> {
    let Some(t) = st.tour.as_mut() else {
        return Err(refusal(cl::Reason::NotFound, "no walkthrough is showing"));
    };
    match op {
        "tour.next" if t.at + 1 < t.steps.len() => t.at += 1,
        "tour.back" => t.at = t.at.saturating_sub(1),
        _ => {
            // Stop, or next on the last step: the walkthrough is over.
            st.tour = None;
            st.stack.layers.retain(|l| l.id != TOUR_ID);
            return Ok(Done::Applied(cl::Applied {
                layer: None,
                popped: vec![TOUR_ID.into()],
            }));
        }
    }
    show_step(st, now, limits)
}

/// The walkthrough's current step as the `tour` layer (pushed, or updated in place).
fn show_step(st: &mut LayerState, now: u64, limits: &cl::Limits) -> Result<Done, cl::Refusal> {
    let t = st.tour.as_ref().expect("a walkthrough");
    let s = &t.steps[t.at];
    let mut layer = cl::Layer::new(cl::Anchor::Caret);
    layer.id = TOUR_ID.into();
    layer.anchor = s.anchor.clone();
    layer.owner = t.actor.clone().map_or(cl::Owner::Guide, cl::Owner::Agent);
    let mut data = json!({"text": s.text, "step": t.at + 1, "of": t.steps.len()});
    if let Some(title) = &s.title {
        data["title"] = json!(title);
    }
    layer.content = Some(cl::Content::new(TOUR, data));
    layer.arrow = true;
    layer.ring = Some(cl::Ring::default());
    layer.spotlight = t.spotlight.then(cl::Spotlight::default);
    let op = if st.stack.get(TOUR_ID).is_some() {
        cl::LayerOp::Update(layer)
    } else {
        cl::LayerOp::Push(layer)
    };
    // thc moves the walkthrough itself: the owner is kept as set (an agent's stays its).
    let a = cl::apply(&mut st.stack, op, None, now, limits)?;
    Ok(Done::Applied(a))
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
    if let Some(t) = &st.tour {
        v["tour"] = json!({"at": t.at, "of": t.steps.len(), "actor": t.actor});
    }
    v
}

/// thc's ops' JSON Schemas (`thc schema --proto`), beside the protocol's.
pub fn schema() -> Value {
    let anchor = json!({"description": "row:<id> | ui:<element> | action:<command> | panel:<n> | caret | text:<from>..<to> | block:<n>, or a caretline-layers anchor object; a list is fallbacks", "oneOf": [{"type": "string"}, {"type": "object"}, {"type": "array"}]});
    json!({
        "hint.show": {"type": "object", "required": ["anchor", "text"], "properties": {"anchor": anchor, "text": {"type": "string"}, "title": {"type": "string"}, "ttl_ms": {"type": "integer"}, "place": {"type": "array", "items": {"enum": ["below", "above", "right", "left"]}}, "arrow": {"type": "boolean"}, "ring": {"type": "boolean"}, "actor": {"type": "string"}}},
        "hint.clear": {"type": "object", "properties": {"layer": {"type": "string"}, "all": {"type": "boolean"}, "actor": {"type": "string"}}},
        "highlight": {"type": "object", "required": ["anchor"], "properties": {"anchor": anchor, "ttl_ms": {"type": "integer"}, "actor": {"type": "string"}}},
        "focus": {"type": "object", "required": ["anchor"], "properties": {"anchor": anchor, "text": {"type": "string"}, "title": {"type": "string"}, "ttl_ms": {"type": "integer"}, "actor": {"type": "string"}}},
        "layer.push": {"type": "object", "required": ["layer"], "properties": {"layer": {"type": "object", "description": "a caretline-layers Layer"}, "actor": {"type": "string"}}},
        "layer.update": {"type": "object", "required": ["layer"], "properties": {"layer": {"type": "object"}, "actor": {"type": "string"}}},
        "layer.pop": {"type": "object", "properties": {"layer": {"type": "string"}, "owner": {"type": "string"}, "all": {"type": "boolean"}, "actor": {"type": "string"}}},
        "layer.ls": {"type": "object", "properties": {}},
        "tour.start": {"type": "object", "required": ["steps"], "properties": {"steps": {"type": "array", "items": {"type": "object", "required": ["anchor", "text"], "properties": {"anchor": anchor, "title": {"type": "string"}, "text": {"type": "string"}}}}, "spotlight": {"type": "boolean"}, "actor": {"type": "string"}}},
        "tour.next": {"type": "object"}, "tour.back": {"type": "object"}, "tour.stop": {"type": "object"},
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
        let data = |u: &UiState| {
            u.layers
                .stack
                .get(TOUR_ID)
                .unwrap()
                .content
                .clone()
                .unwrap()
                .data
        };
        assert_eq!(data(&u)["step"], 1);
        step(&mut u.layers, "tour.next", 0, &l).ok();
        assert_eq!(data(&u)["step"], 2);
        step(&mut u.layers, "tour.back", 0, &l).ok();
        step(&mut u.layers, "tour.back", 0, &l).ok();
        assert_eq!(data(&u)["step"], 1);
        for _ in 0..3 {
            step(&mut u.layers, "tour.next", 0, &l).ok();
        }
        assert!(u.layers.tour.is_none() && u.layers.stack.layers.is_empty());
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
        assert!(u.layers.dismiss_agents());
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
