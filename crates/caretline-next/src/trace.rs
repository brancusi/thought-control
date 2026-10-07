//! Traces: a JSON Lines record of a session. The first line holds the initial state and
//! every later line one message, so folding the messages into the state with `update`
//! reproduces the session exactly.

use serde::{Deserialize, Serialize};

use crate::msg::Msg;
use crate::state::State;
use crate::update::update;

/// One line of a trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceLine {
    State(Box<State>),
    Msg(Msg),
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
/// `state` line (a session restarted into the same file) resets the state.
pub fn replay_trace(input: &str) -> Result<(State, usize), String> {
    let mut state: Option<State> = None;
    let mut count = 0;
    for (i, line) in input.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let entry: TraceLine =
            serde_json::from_str(line).map_err(|e| format!("trace line {}: {e}", i + 1))?;
        match entry {
            TraceLine::State(s) => {
                let mut s = *s;
                s.sanitize();
                state = Some(s);
            }
            TraceLine::Msg(msg) => {
                let s = state
                    .as_mut()
                    .ok_or_else(|| format!("trace line {}: a message before any state", i + 1))?;
                update(s, msg);
                count += 1;
            }
        }
    }
    state
        .map(|s| (s, count))
        .ok_or_else(|| "the trace has no state line".to_string())
}
