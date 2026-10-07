//! ⌃P in the tour and the agent demo: the session's own trace, replayed on screen at about
//! the speed it was recorded (long pauses shortened), ending on the state it started from.

use std::time::{Duration, Instant};

use caretline::trace::{apply_with_views, OnView, TraceLine, ViewOpen};
use caretline::{Frame, Msg, State, View};

/// The longest pause a replay keeps, and the longest a whole replay takes.
const MAX_GAP: Duration = Duration::from_millis(400);
const MAX_TOTAL: Duration = Duration::from_secs(15);

pub struct Replay {
    lines: Vec<TraceLine>,
    /// When each line is due, from the start.
    due: Vec<Duration>,
    next: usize,
    state: Option<State>,
    views: Vec<(u32, View)>,
    applied: usize,
    total: usize,
    start: Instant,
    /// The state as it was when the replay began: where it must land.
    target: String,
    pane_rows: u16,
}

fn time_of(line: &TraceLine) -> Option<u64> {
    let msg = match line {
        TraceLine::Msg(m) => m,
        TraceLine::On(OnView { msg, .. }) => msg,
        _ => return None,
    };
    match msg {
        Msg::Tick { now_ms } | Msg::Frame { now_ms } => Some(*now_ms),
        _ => None,
    }
}

impl Replay {
    pub fn new(lines: &[TraceLine], target: &State, pane_rows: u16, now: Instant) -> Replay {
        let lines = lines.to_vec();
        let mut due = Vec::with_capacity(lines.len());
        let mut at = Duration::ZERO;
        let mut last: Option<u64> = None;
        for line in &lines {
            if let Some(t) = time_of(line) {
                if let Some(l) = last {
                    at += Duration::from_millis(t.saturating_sub(l)).min(MAX_GAP);
                }
                last = Some(t);
            }
            due.push(at);
        }
        if at > MAX_TOTAL {
            let k = MAX_TOTAL.as_secs_f64() / at.as_secs_f64();
            for d in &mut due {
                *d = d.mul_f64(k);
            }
        }
        let total = lines.iter().filter(|l| matches!(l, TraceLine::Msg(_) | TraceLine::On(_))).count();
        let mut r = Replay { lines, due, next: 0, state: None, views: Vec::new(), applied: 0, total, start: now, target: target.to_json(), pane_rows };
        // The initial state shows at once.
        r.advance(now);
        r
    }

    /// Applies every line due by `now`. Returns whether anything changed.
    pub fn advance(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.start);
        let from = self.next;
        while self.next < self.lines.len() && self.due[self.next] <= elapsed {
            match self.lines[self.next].clone() {
                TraceLine::State(s) => {
                    let mut s = *s;
                    s.sanitize();
                    self.state = Some(s);
                    self.views.clear();
                }
                TraceLine::Msg(msg) => {
                    if let Some(s) = &mut self.state {
                        apply_with_views(s, &mut self.views, 0, msg);
                        self.applied += 1;
                    }
                }
                TraceLine::On(OnView { view, msg }) => {
                    if let Some(s) = &mut self.state {
                        apply_with_views(s, &mut self.views, view, msg);
                        self.applied += 1;
                    }
                }
                TraceLine::ViewOpen(ViewOpen { id, view }) => {
                    if let Some(s) = &self.state {
                        let mut v = *view;
                        v.fit(&s.doc);
                        self.views.retain(|(k, _)| *k != id);
                        self.views.push((id, v));
                    }
                }
                TraceLine::ViewClose(id) => self.views.retain(|(k, _)| *k != id),
            }
            self.next += 1;
        }
        self.next != from
    }

    pub fn done(&self) -> bool {
        self.next >= self.lines.len()
    }

    pub fn next_due(&self) -> Option<Instant> {
        self.due.get(self.next).map(|d| self.start + *d)
    }

    /// Messages applied so far.
    pub fn applied(&self) -> usize {
        self.applied
    }

    /// Whether the replay landed on the state it started from.
    pub fn matches(&self) -> bool {
        self.state.as_ref().is_some_and(|s| s.to_json() == self.target)
    }

    /// The replayed editor, with a status line saying so.
    pub fn frame(&self) -> Frame {
        let Some(state) = &self.state else {
            return caretline::view(&State::new("", None, caretline::Viewport { width: 80, height: 24 }));
        };
        let mut s = state.clone();
        s.view.status = Some(format!("▶ replay {}/{} · any key stops", self.applied, self.total));
        crate::runtime::compose_state(&s, &self.views, if self.views.is_empty() { 0 } else { self.pane_rows })
    }
}
