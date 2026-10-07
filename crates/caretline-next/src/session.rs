//! An in-process editing session: a [`State`], a revision counter and the trace of what was
//! applied, split into segments. It is the library face of the state protocol
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

/// How many trace lines a session keeps by default (see [`Session::set_trace_limit`]).
pub const DEFAULT_TRACE_LIMIT: usize = 100_000;

/// A state plus its revision and trace.
///
/// `rev` starts at 0 and goes up by one for every message applied and every state
/// replacement, so a client that saw rev `n` knows exactly how many changes it missed.
///
/// The trace is a list of segments. Each starts with a `state` line (the initial state, a
/// [`Session::set_state`], or a [`Session::checkpoint`]) followed by the messages applied
/// since, so every segment replays on its own. The session keeps at most its trace limit of
/// lines: past it, older segments are dropped, and a segment that alone outgrows the limit
/// is cut by an automatic checkpoint.
#[derive(Debug, Clone)]
pub struct Session {
    state: State,
    rev: u64,
    trace: Vec<TraceLine>,
    /// The rev each trace line leaves the session at (for a `state` line, the rev it starts).
    trace_revs: Vec<u64>,
    /// Where the current segment starts in `trace`.
    segment: usize,
    /// Lines dropped from the front of the trace so far.
    dropped: usize,
    limit: usize,
}

impl Session {
    pub fn new(state: State) -> Session {
        let trace = vec![TraceLine::State(Box::new(state.clone()))];
        Session {
            state,
            rev: 0,
            trace,
            trace_revs: vec![0],
            segment: 0,
            dropped: 0,
            limit: DEFAULT_TRACE_LIMIT,
        }
    }

    /// Keeps at most `lines` trace lines (at least 2), dropping old segments as needed.
    pub fn set_trace_limit(&mut self, lines: usize) {
        self.limit = lines.max(2);
        self.trim();
    }

    pub fn trace_limit(&self) -> usize {
        self.limit
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// Every trace line kept: the initial state and every message and replacement since,
    /// unless the trace limit dropped older segments. It starts with a `state` line, so its
    /// lines joined with newlines are a trace `caretline --replay` reads.
    pub fn trace(&self) -> &[TraceLine] {
        &self.trace
    }

    /// The current segment: the latest `state` line (initial state, replacement or
    /// checkpoint) and every message since. Replays on its own to the current state.
    pub fn segment_trace(&self) -> &[TraceLine] {
        &self.trace[self.segment..]
    }

    /// The rev the current segment starts at.
    pub fn segment_rev(&self) -> u64 {
        self.trace_revs[self.segment]
    }

    /// The rev the kept trace starts at: `trace_since` answers for any rev from here on.
    pub fn trace_start_rev(&self) -> u64 {
        self.trace_revs[0]
    }

    /// The trace lines after rev `rev`: what changed since a client saw it (messages, and
    /// `state` lines for replacements and checkpoints). `None` when the trace no longer
    /// reaches back to `rev` (see [`Session::trace_start_rev`]).
    pub fn trace_since(&self, rev: u64) -> Option<&[TraceLine]> {
        if rev < self.trace_start_rev() {
            return None;
        }
        let from = self.trace_revs.partition_point(|&r| r <= rev);
        Some(&self.trace[from..])
    }

    /// Starts a new trace segment with the current state, without changing the state or the
    /// rev. Returns the rev.
    pub fn checkpoint(&mut self) -> u64 {
        self.push_line(TraceLine::State(Box::new(self.state.clone())));
        self.segment = self.trace.len() - 1;
        self.trim();
        self.rev
    }

    /// How many trace lines this session has recorded, dropped ones included. With
    /// [`Session::trace_lines_from`], lets a runtime copy new lines to a file.
    pub fn trace_lines_total(&self) -> usize {
        self.dropped + self.trace.len()
    }

    /// The lines from absolute position `n` (counting dropped lines) on. When some of them
    /// were already dropped, returns `None`.
    pub fn trace_lines_from(&self, n: usize) -> Option<&[TraceLine]> {
        if n < self.dropped {
            return None;
        }
        Some(&self.trace[(n - self.dropped).min(self.trace.len())..])
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

    fn push_line(&mut self, line: TraceLine) {
        self.trace.push(line);
        self.trace_revs.push(self.rev);
    }

    /// Enforces the trace limit: drops segments before the current one, then, if the current
    /// segment alone is too long, checkpoints and drops everything before the checkpoint.
    fn trim(&mut self) {
        if self.trace.len() <= self.limit {
            return;
        }
        if self.segment > 0 {
            self.drop_front(self.segment);
        }
        if self.trace.len() > self.limit {
            self.push_line(TraceLine::State(Box::new(self.state.clone())));
            self.drop_front(self.trace.len() - 1);
        }
    }

    fn drop_front(&mut self, n: usize) {
        self.trace.drain(..n);
        self.trace_revs.drain(..n);
        self.dropped += n;
        self.segment = self.segment.saturating_sub(n);
    }

    /// Applies one message and returns its effects, unperformed.
    pub fn apply(&mut self, msg: Msg) -> Vec<Effect> {
        self.rev += 1;
        self.push_line(TraceLine::Msg(msg.clone()));
        let effects = update(&mut self.state, msg);
        self.trim();
        effects
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
    /// Recorded in the trace as the start of a new segment. Returns the new rev.
    pub fn set_state(&mut self, mut state: State) -> u64 {
        state.sanitize();
        self.rev += 1;
        self.push_line(TraceLine::State(Box::new(state.clone())));
        self.segment = self.trace.len() - 1;
        self.state = state;
        self.trim();
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
