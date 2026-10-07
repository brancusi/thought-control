//! Traces: a JSON Lines record of a session. The first line holds the initial state and
//! every later line one message, so folding the messages into the state with `update`
//! reproduces the session exactly.

use serde::{Deserialize, Serialize};

use crate::msg::Msg;
use crate::state::{State, View};
use crate::update::update;
use crate::views::update_doc;

/// One line of a trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceLine {
    State(Box<State>),
    Msg(Msg),
    /// A second (third…) view of the document opened, with its id.
    ViewOpen(ViewOpen),
    /// A view closed.
    ViewClose(u32),
    /// A message through a view other than the state's own.
    On(OnView),
}

/// A view opened on the session's document (`{"view_open": {"id": 1, "view": {…}}}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewOpen {
    pub id: u32,
    pub view: Box<View>,
}

/// A message through view `view` (`{"on": {"view": 1, "msg": {…}}}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnView {
    pub view: u32,
    pub msg: Msg,
}

impl TraceLine {
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("trace line serializes")
    }
}

/// Parses a message list: one JSON message per line (blank lines and `//` comments are
/// skipped), or a single JSON array.
pub fn parse_msgs(input: &str) -> Result<Vec<Msg>, String> {
    let trimmed = input.trim_start();
    if trimmed.starts_with('[') {
        return serde_json::from_str(trimmed).map_err(|e| format!("messages: {e}"));
    }
    let mut msgs = Vec::new();
    for (i, line) in input.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let msg = serde_json::from_str(line).map_err(|e| format!("line {}: {e}", i + 1))?;
        msgs.push(msg);
    }
    Ok(msgs)
}

/// Replays a trace: returns the final state and the number of messages applied. A later
/// `state` line (a session restarted into the same file) resets the state and closes every
/// other view. Views opened in the trace are replayed too (see [`replay_trace_views`]).
pub fn replay_trace(input: &str) -> Result<(State, usize), String> {
    replay_trace_views(input).map(|(s, _, n)| (s, n))
}

/// A replayed trace: the final state, the other views open at its end (with their ids), and
/// the number of messages applied.
pub type Replayed = (State, Vec<(u32, View)>, usize);

/// [`replay_trace`], also returning the other views open at the end, with their ids.
pub fn replay_trace_views(input: &str) -> Result<Replayed, String> {
    let mut state: Option<State> = None;
    let mut views: Vec<(u32, View)> = Vec::new();
    let mut count = 0;
    for (i, line) in input.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let entry: TraceLine =
            serde_json::from_str(line).map_err(|e| format!("trace line {}: {e}", i + 1))?;
        let no_state = || format!("trace line {}: a message before any state", i + 1);
        match entry {
            TraceLine::State(s) => {
                let mut s = *s;
                s.sanitize();
                state = Some(s);
                views.clear();
            }
            TraceLine::Msg(msg) => {
                let s = state.as_mut().ok_or_else(no_state)?;
                apply_with_views(s, &mut views, 0, msg);
                count += 1;
            }
            TraceLine::On(OnView { view, msg }) => {
                let s = state.as_mut().ok_or_else(no_state)?;
                apply_with_views(s, &mut views, view, msg);
                count += 1;
            }
            TraceLine::ViewOpen(ViewOpen { id, view }) => {
                let s = state.as_ref().ok_or_else(no_state)?;
                let mut v = *view;
                v.fit(&s.doc);
                views.retain(|(k, _)| *k != id);
                views.push((id, v));
            }
            TraceLine::ViewClose(id) => views.retain(|(k, _)| *k != id),
        }
    }
    state
        .map(|s| (s, views, count))
        .ok_or_else(|| "the trace has no state line".to_string())
}

/// Applies `msg` through view `id` (0: the state's own) of a state with other views open,
/// rebasing the rest. Returns its effects, or nothing when there is no such view.
pub fn apply_with_views(state: &mut State, views: &mut [(u32, View)], id: u32, msg: Msg) -> Vec<crate::msg::Effect> {
    if views.is_empty() && id == 0 {
        return update(state, msg);
    }
    let acting = if id == 0 {
        0
    } else {
        match views.iter().position(|(k, _)| *k == id) {
            Some(i) => i + 1,
            None => return Vec::new(),
        }
    };
    let mut all: Vec<View> = Vec::with_capacity(views.len() + 1);
    all.push(std::mem::take(&mut state.view));
    all.extend(views.iter_mut().map(|(_, v)| std::mem::take(v)));
    let effects = update_doc(&mut state.doc, &mut all, acting, msg);
    let mut it = all.into_iter();
    state.view = it.next().expect("the state's view");
    for ((_, v), back) in views.iter_mut().zip(it) {
        *v = back;
    }
    effects
}
