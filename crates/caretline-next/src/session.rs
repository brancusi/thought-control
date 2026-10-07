//! An in-process editing session: a [`State`], a revision counter and the trace of
//! everything applied since load. It is the library face of the state protocol
//! ([`crate::protocol`]); the `caretline` binary serves the same operations over stdio and
//! Unix sockets.
//!
//! The session never performs effects. [`Session::apply`] returns them; a runtime that
//! wants them performed passes an executor to [`Session::apply_with`], which feeds the
//! executor's result messages (`saved`, `save_failed`) back through `update`.

use std::collections::VecDeque;

use crate::keymap::script_to_msgs;
use crate::msg::{Effect, Msg};
use crate::state::State;
use crate::trace::TraceLine;
use crate::update::update;
use crate::view::{view, Frame};

/// A state plus its revision and trace.
///
/// `rev` starts at 0 and goes up by one for every message applied and every state
/// replacement, so a client that saw rev `n` knows exactly how many changes it missed.
#[derive(Debug, Clone)]
pub struct Session {
    state: State,
    rev: u64,
    trace: Vec<TraceLine>,
}

impl Session {
    pub fn new(state: State) -> Session {
        let trace = vec![TraceLine::State(Box::new(state.clone()))];
        Session { state, rev: 0, trace }
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// The initial state and every message and replacement since, in order. Its lines
    /// joined with newlines are a trace `caretline --replay` reads.
    pub fn trace(&self) -> &[TraceLine] {
        &self.trace
    }

    /// The trace as JSON Lines.
    pub fn trace_jsonl(&self) -> String {
        let mut out = String::new();
        for line in &self.trace {
            out.push_str(&line.to_line());
            out.push('\n');
        }
        out
    }

    /// Applies one message and returns its effects, unperformed.
    pub fn apply(&mut self, msg: Msg) -> Vec<Effect> {
        self.trace.push(TraceLine::Msg(msg.clone()));
        self.rev += 1;
        update(&mut self.state, msg)
    }

    /// Applies messages in order and returns all their effects, unperformed.
    pub fn apply_all(&mut self, msgs: impl IntoIterator<Item = Msg>) -> Vec<Effect> {
        let mut effects = Vec::new();
        for msg in msgs {
            effects.extend(self.apply(msg));
        }
        effects
    }

    /// Applies a message and performs its effects with `exec`. Whatever message `exec`
    /// returns (a save's result) is applied next, so it lands in the trace too. Returns
    /// every effect performed and every message applied, the given one first.
    pub fn apply_with(
        &mut self,
        msg: Msg,
        exec: &mut dyn FnMut(&Effect) -> Option<Msg>,
    ) -> (Vec<Effect>, Vec<Msg>) {
        let mut queue = VecDeque::from([msg]);
        let mut effects = Vec::new();
        let mut applied = Vec::new();
        while let Some(msg) = queue.pop_front() {
            applied.push(msg.clone());
            for effect in self.apply(msg) {
                if let Some(next) = exec(&effect) {
                    queue.push_back(next);
                }
                effects.push(effect);
            }
        }
        (effects, applied)
    }

    /// Runs a key script through the keymap (the `--keys` syntax). Returns the messages it
    /// became and their effects, unperformed.
    pub fn keys(&mut self, script: &str) -> Result<(Vec<Msg>, Vec<Effect>), String> {
        let msgs = script_to_msgs(script, self.state.now_ms)?;
        let effects = self.apply_all(msgs.iter().cloned());
        Ok((msgs, effects))
    }

    /// Replaces the state (repairing anything out of bounds, as loading a file does).
    /// Recorded in the trace as a new starting point. Returns the new rev.
    pub fn set_state(&mut self, mut state: State) -> u64 {
        state.sanitize();
        self.trace.push(TraceLine::State(Box::new(state.clone())));
        self.state = state;
        self.rev += 1;
        self.rev
    }

    /// Renders the current state.
    pub fn frame(&self) -> Frame {
        view(&self.state)
    }

    /// Renders at `width`x`height` without changing the session: the frame `--snapshot`
    /// would print, which resizes through `update` first when the size differs.
    pub fn render(&self, width: u16, height: u16) -> Frame {
        let v = self.state.viewport;
        if (v.width, v.height) == (width, height) {
            return view(&self.state);
        }
        let mut sized = self.state.clone();
        update(&mut sized, Msg::Resize { width, height });
        view(&sized)
    }
}
