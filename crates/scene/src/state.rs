//! The screen's whole state, the messages that change it and a pure `update`. The runtime owns
//! the terminal, the socket and child processes; it feeds messages in and carries effects out.
//! `State` serializes, so `thc-scene get --state` shows exactly what is drawn.

use crate::model::{Action, Kind, LayerSpec, Node, Ui, lookup};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct State {
    pub ui: Ui,
    /// Per-node view state, by node id. Survives a push for every id still in the new tree.
    #[serde(default)]
    pub local: BTreeMap<String, Local>,
    #[serde(default)]
    pub focus: Option<String>,
    /// The last value fetched for each source.
    #[serde(default)]
    pub data: BTreeMap<String, Slot>,
    /// One line at the bottom: the last error or message.
    #[serde(default)]
    pub status: Option<String>,
    /// How many UIs have been pushed (the first load counts).
    #[serde(default)]
    pub version: u64,
    /// The runtime's measurements, also bindable as the `$stats` source.
    #[serde(default)]
    pub stats: Stats,
    /// What the mouse is over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hover: Option<Hover>,
    /// The logical clock (ms), moved only by `Msg::Tick`; animations read it.
    #[serde(default)]
    pub now_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Hover {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row: Option<usize>,
}

/// Frames and messages over the last measuring window (the runtime reports twice a second).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    /// Frames drawn per second.
    pub fps: f64,
    /// Mean time to build and draw one frame, in milliseconds.
    pub draw_ms: f64,
    /// Slowest frame in the window, in milliseconds.
    pub max_ms: f64,
    /// Messages applied per second (keys, data, pushes, patches).
    pub msgs: f64,
    /// Frames drawn since start.
    pub frames: u64,
}

/// Sources whose names start with `$` are the runtime's own (`$stats`); a push keeps them.
pub fn builtin(name: &str) -> bool {
    name.starts_with('$')
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Local {
    pub selected: usize,
    /// The shown child of a tabs node.
    #[serde(default)]
    pub tab: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Slot {
    Loading,
    Ready { value: Value },
    Error { message: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "msg", rename_all = "snake_case")]
pub enum Msg {
    /// Replace the whole UI.
    Push { ui: Ui },
    /// Replace one node by id.
    Patch { id: String, node: Node },
    /// A key, by name (`j`, `tab`, `s-tab`, `c-c`, `enter`).
    Key { key: String },
    /// A source finished fetching.
    Data { name: String, result: Result<Value, String> },
    /// A `run` finished.
    Ran { argv: Vec<String>, result: Result<(), String> },
    /// The runtime's measurements.
    Stats { stats: Stats },
    /// A click on a node (a row of it, or a tab title).
    Click { id: String, row: Option<usize>, tab: Option<usize> },
    /// The mouse moved over a node (a row of it), or off everything.
    Hover { hover: Option<Hover> },
    /// The wheel over a node: rows to move (negative is up).
    Scroll { id: String, by: isize },
    /// Animation time.
    Tick { now_ms: u64 },
    /// Replace the layers only.
    Layers { layers: Vec<LayerSpec> },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Fetch a source (the runtime reads its definition from `ui.data`).
    Fetch(String),
    Run(Vec<String>),
    Load(String),
    Quit,
}

pub fn update(st: &mut State, msg: Msg) -> Vec<Effect> {
    match msg {
        Msg::Push { ui } => {
            st.ui = ui;
            st.version += 1;
            st.status = None;
            settle(st);
            // Sources can change with the UI; fetch them all again.
            st.data.retain(|k, _| builtin(k) || st.ui.data.contains_key(k));
            st.ui.data.keys().map(|k| Effect::Fetch(k.clone())).collect()
        }
        Msg::Patch { id, node } => {
            if st.ui.root.replace(&id, &node) {
                st.status = None;
                settle(st);
            } else {
                st.status = Some(format!("patch: no node with id {id:?}"));
            }
            vec![]
        }
        Msg::Data { name, result } => {
            if st.ui.data.contains_key(&name) {
                let slot = match result {
                    Ok(value) => Slot::Ready { value },
                    Err(message) => Slot::Error { message },
                };
                st.data.insert(name, slot);
                settle(st);
            }
            vec![]
        }
        Msg::Stats { stats } => {
            let value = serde_json::to_value(&stats).unwrap_or_default();
            st.data.insert("$stats".into(), Slot::Ready { value });
            st.stats = stats;
            vec![]
        }
        Msg::Ran { argv, result } => {
            st.status = result.err().map(|e| format!("{}: {e}", argv.join(" ")));
            refresh(st)
        }
        Msg::Click { id, row, tab } => {
            let Some(n) = st.ui.root.find(&id) else { return vec![] };
            let click = n.click.clone();
            if n.focusable() {
                st.focus = Some(id.clone());
            }
            let local = st.local.entry(id).or_default();
            if let Some(r) = row {
                local.selected = r;
            }
            if let Some(t) = tab {
                local.tab = t;
                settle(st);
            }
            match click {
                Some(a) => act(st, a),
                None => vec![],
            }
        }
        Msg::Hover { hover } => {
            st.hover = hover;
            vec![]
        }
        Msg::Scroll { id, by } => {
            if st.ui.root.find(&id).is_some_and(Node::focusable) {
                st.focus = Some(id);
                step(st, |sel, len| sel.saturating_add_signed(by).min(len.saturating_sub(1)))
            } else {
                vec![]
            }
        }
        Msg::Tick { now_ms } => {
            st.now_ms = now_ms;
            vec![]
        }
        Msg::Layers { layers } => {
            st.ui.layers = layers;
            vec![]
        }
        Msg::Key { key } => match resolve(st, &key) {
            Some(a) => act(st, a),
            None => vec![],
        },
    }
}

/// The action for a key: the focused node's own, then the UI's, then the defaults.
fn resolve(st: &State, key: &str) -> Option<Action> {
    let focused = st.focus.as_deref().and_then(|id| st.ui.root.find(id));
    if let Some(a) = focused.and_then(|n| n.keys.get(key)) {
        return Some(a.clone());
    }
    if let Some(a) = st.ui.keys.get(key) {
        return Some(a.clone());
    }
    let name = match key {
        "q" | "c-c" => "quit",
        "j" | "down" => "down",
        "k" | "up" => "up",
        "g" | "home" => "top",
        "G" | "end" => "bottom",
        "tab" => "focus_next",
        "s-tab" => "focus_prev",
        "h" | "left" => "tab_prev",
        "l" | "right" => "tab_next",
        "r" => "refresh",
        _ => return None,
    };
    Some(Action::Named(name.into()))
}

fn act(st: &mut State, a: Action) -> Vec<Effect> {
    match a {
        Action::Run { run } => {
            let sel = selected_item(st).unwrap_or(Value::Null);
            vec![Effect::Run(run.iter().map(|s| crate::model::fill(s, &sel)).collect())]
        }
        Action::Load { load } => vec![Effect::Load(load)],
        Action::Focus { focus } => {
            if st.ui.root.find(&focus).is_some_and(Node::focusable) {
                st.focus = Some(focus);
            }
            vec![]
        }
        Action::Named(n) => match n.as_str() {
            "quit" => vec![Effect::Quit],
            "refresh" => refresh(st),
            "down" => step(st, |sel, len| (sel + 1).min(len.saturating_sub(1))),
            "up" => step(st, |sel, _| sel.saturating_sub(1)),
            "top" => step(st, |_, _| 0),
            "bottom" => step(st, |_, len| len.saturating_sub(1)),
            "tab_next" => switch(st, 1),
            "tab_prev" => switch(st, -1),
            "focus_next" => cycle(st, 1),
            "focus_prev" => cycle(st, -1),
            other => {
                st.status = Some(format!("unknown action {other:?}"));
                vec![]
            }
        },
    }
}

/// Show the next or previous child of the focused tabs node.
fn switch(st: &mut State, dir: isize) -> Vec<Effect> {
    let Some(id) = st.focus.clone() else { return vec![] };
    let n = match st.ui.root.find(&id).map(|n| &n.kind) {
        Some(Kind::Tabs { children, .. }) => children.len() as isize,
        _ => return vec![],
    };
    let local = st.local.entry(id).or_default();
    local.tab = ((local.tab as isize + dir).rem_euclid(n.max(1))) as usize;
    settle(st);
    vec![]
}

fn cycle(st: &mut State, dir: isize) -> Vec<Effect> {
    let ids = focus_order(st);
    if ids.is_empty() {
        return vec![];
    }
    let at = st.focus.as_ref().and_then(|f| ids.iter().position(|i| i == f)).unwrap_or(0) as isize;
    let n = ids.len() as isize;
    st.focus = Some(ids[((at + dir).rem_euclid(n)) as usize].clone());
    vec![]
}

fn step(st: &mut State, f: impl Fn(usize, usize) -> usize) -> Vec<Effect> {
    let Some(id) = st.focus.clone() else { return vec![] };
    let len = st.ui.root.find(&id).map(|n| items(st, n).len()).unwrap_or(0);
    let local = st.local.entry(id).or_default();
    local.selected = f(local.selected, len);
    vec![]
}

fn refresh(st: &State) -> Vec<Effect> {
    st.ui.data.keys().map(|k| Effect::Fetch(k.clone())).collect()
}

/// After any change: drop view state for ids that are gone, keep focus on something focusable,
/// and keep each selection inside its list.
fn settle(st: &mut State) {
    let order = focus_order(st);
    let mut all = Vec::new();
    st.ui.root.walk(&mut all);
    let ids: Vec<String> = all.iter().filter_map(|n| n.id.clone()).collect();
    st.local.retain(|k, _| ids.contains(k));
    if !st.focus.as_ref().is_some_and(|f| order.contains(f)) {
        st.focus = order.first().cloned();
    }
    let lens: Vec<(String, usize)> =
        all.iter().filter(|n| n.focusable()).map(|n| (n.id.clone().unwrap(), items(st, n).len())).collect();
    for (id, len) in lens {
        if let Some(l) = st.local.get_mut(&id) {
            l.selected = l.selected.min(len.saturating_sub(1));
        }
    }
}

/// The focusable nodes on screen, in tree order (hidden tabs left out).
pub fn focus_order(st: &State) -> Vec<String> {
    let mut all = Vec::new();
    st.ui.root.walk_shown(&|n| tab_of(st, n), &mut all);
    all.into_iter().filter(|n| n.focusable()).filter_map(|n| n.id.clone()).collect()
}

/// The shown child of a tabs node.
pub fn tab_of(st: &State, n: &Node) -> usize {
    n.id.as_ref().and_then(|id| st.local.get(id)).map(|l| l.tab).unwrap_or(0)
}

/// The value a node is bound to, if it has one and it has arrived.
pub fn bound<'a>(st: &'a State, n: &Node) -> Option<&'a Value> {
    value_of(st, n.bind.as_deref()?)
}

/// A bind's value: a source (and a path into it), or `@id` (and a path), the selected row of
/// node `id`, so a detail panel follows a list's selection.
pub fn value_of<'a>(st: &'a State, bind: &str) -> Option<&'a Value> {
    let (source, path) = split_bind(bind);
    if let Some(id) = source.strip_prefix('@') {
        let n = st.ui.root.find(id)?;
        return lookup(items(st, n).get(selected(st, id))?, path);
    }
    match st.data.get(source)? {
        Slot::Ready { value } => lookup(value, path),
        _ => None,
    }
}

/// `bind` is a source name, then optionally a path into its value: `"tick.rows"` reads `rows` of
/// the `tick` source, so one source can feed many nodes.
pub fn split_bind(bind: &str) -> (&str, &str) {
    bind.split_once('.').unwrap_or((bind, ""))
}

/// A list's or table's rows: its literal `items`, else its bound array. Borrowed: a 50,000-row
/// source is read in place, never copied per message or per frame.
pub fn items<'a>(st: &'a State, n: &'a Node) -> &'a [Value] {
    let literal = match &n.kind {
        Kind::List { items, .. } | Kind::Table { items, .. } | Kind::Bars { items, .. } => items,
        _ => return &[],
    };
    if !literal.is_empty() {
        return literal;
    }
    match bound(st, n) {
        Some(Value::Array(a)) => a,
        _ => &[],
    }
}

pub fn selected(st: &State, id: &str) -> usize {
    st.local.get(id).map(|l| l.selected).unwrap_or(0)
}

/// The focused list's selected row, for `run` templates.
pub fn selected_item(st: &State) -> Option<Value> {
    let id = st.focus.as_ref()?;
    let n = st.ui.root.find(id)?;
    items(st, n).get(selected(st, id)).cloned()
}

/// A source's value from a command's stdout, narrowed by its `path`.
pub fn narrow(raw: &str, path: Option<&str>) -> Result<Value, String> {
    let v: Value = serde_json::from_str(raw.trim()).map_err(|e| format!("not JSON: {e}"))?;
    match path {
        None => Ok(v),
        Some(p) => lookup(&v, p).cloned().ok_or_else(|| format!("no {p:?} in the output")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ui(v: Value) -> Ui {
        serde_json::from_value(v).unwrap()
    }

    fn two_lists() -> Ui {
        ui(json!({
            "root": {"type": "row", "children": [
                {"type": "list", "id": "a", "items": ["one", "two", "three"]},
                {"type": "list", "id": "b", "bind": "tasks", "item": "{text}",
                 "keys": {"x": {"run": ["thc", "done", "{id}"]}}}
            ]},
            "data": {"tasks": {"value": [{"id": "t1", "text": "Buy milk"}, {"id": "t2", "text": "Essay"}]}}
        }))
    }

    #[test]
    fn push_focuses_the_first_list_and_fetches_sources() {
        let mut st = State::default();
        let fx = update(&mut st, Msg::Push { ui: two_lists() });
        assert_eq!(st.focus.as_deref(), Some("a"));
        assert_eq!(fx, vec![Effect::Fetch("tasks".into())]);
    }

    #[test]
    fn keys_move_the_selection_and_run_templates_the_selected_row() {
        let mut st = State::default();
        update(&mut st, Msg::Push { ui: two_lists() });
        update(&mut st, Msg::Data { name: "tasks".into(), result: Ok(json!([{"id": "t1"}, {"id": "t2"}])) });
        for k in ["j", "j", "j"] {
            update(&mut st, Msg::Key { key: k.into() });
        }
        assert_eq!(selected(&st, "a"), 2, "clamped to the last row");
        update(&mut st, Msg::Key { key: "tab".into() });
        update(&mut st, Msg::Key { key: "j".into() });
        let fx = update(&mut st, Msg::Key { key: "x".into() });
        assert_eq!(fx, vec![Effect::Run(vec!["thc".into(), "done".into(), "t2".into()])]);
    }

    #[test]
    fn view_state_survives_a_push_for_ids_that_remain() {
        let mut st = State::default();
        update(&mut st, Msg::Push { ui: two_lists() });
        update(&mut st, Msg::Key { key: "j".into() });
        let mut next = two_lists();
        next.root.children_mut().unwrap().remove(1);
        update(&mut st, Msg::Push { ui: next });
        assert_eq!(selected(&st, "a"), 1);
        assert!(!st.local.contains_key("b"));
        assert_eq!(st.version, 2);
    }

    #[test]
    fn a_bind_can_reach_into_its_source() {
        let mut st = State::default();
        let ui = ui(json!({"root": {"type": "list", "id": "l", "bind": "feed.rows.all"},
                           "data": {"feed": {"value": null}}}));
        update(&mut st, Msg::Push { ui });
        update(&mut st, Msg::Data { name: "feed".into(), result: Ok(json!({"rows": {"all": [1, 2, 3]}})) });
        assert_eq!(items(&st, st.ui.root.find("l").unwrap()).len(), 3);
    }

    #[test]
    fn clicks_focus_select_and_switch_tabs_and_focus_skips_hidden_tabs() {
        let mut st = State::default();
        let ui = ui(json!({"root": {"type": "tabs", "id": "t", "tabs": ["A", "B"], "children": [
            {"type": "list", "id": "a", "items": [1, 2, 3]},
            {"type": "list", "id": "b", "items": [1, 2, 3], "click": {"run": ["echo", "{.}"]}}
        ]}}));
        update(&mut st, Msg::Push { ui });
        assert_eq!(focus_order(&st), vec!["t", "a"]);
        update(&mut st, Msg::Click { id: "t".into(), row: None, tab: Some(1) });
        assert_eq!(focus_order(&st), vec!["t", "b"]);
        let fx = update(&mut st, Msg::Click { id: "b".into(), row: Some(2), tab: None });
        assert_eq!(st.focus.as_deref(), Some("b"));
        assert_eq!(fx, vec![Effect::Run(vec!["echo".into(), "3".into()])]);
        update(&mut st, Msg::Scroll { id: "b".into(), by: -5 });
        assert_eq!(selected(&st, "b"), 0);
    }

    #[test]
    fn an_at_bind_follows_another_nodes_selection() {
        let mut st = State::default();
        let ui = ui(json!({"root": {"type": "col", "children": [
            {"type": "list", "id": "l", "items": [{"n": "a"}, {"n": "b"}]},
            {"type": "text", "id": "d", "bind": "@l", "text": "{n}"}
        ]}}));
        update(&mut st, Msg::Push { ui });
        let detail = |st: &State| crate::model::fill("{n}", bound(st, st.ui.root.find("d").unwrap()).unwrap());
        assert_eq!(detail(&st), "a");
        update(&mut st, Msg::Key { key: "j".into() });
        assert_eq!(detail(&st), "b");
    }

    #[test]
    fn patch_replaces_one_node() {
        let mut st = State::default();
        update(&mut st, Msg::Push { ui: two_lists() });
        let node: Node = serde_json::from_value(json!({"type": "list", "id": "a", "items": ["only"]})).unwrap();
        update(&mut st, Msg::Patch { id: "a".into(), node });
        assert_eq!(items(&st, st.ui.root.find("a").unwrap()), &[json!("only")]);
        update(&mut st, Msg::Patch { id: "zz".into(), node: Node::default() });
        assert!(st.status.unwrap().contains("zz"));
    }
}
