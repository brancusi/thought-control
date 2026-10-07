//! A UI session: the TUI's state, the one door every change goes through, and its trace.
//!
//! Every input, whether a key at the terminal, a click, a paste, a resize, a clock tick, a
//! state pushed by an agent or a protocol request, is a [`Msg`]. [`Session::apply`] applies
//! it, moves the revision, records it in the trace and reports the [`Effect`]s it asks the
//! runtime for (quit, the external editor, the mouse mode, a vault switch). The live
//! terminal loop, `THC_TUI_SNAPSHOT`, `thc ui render` and `thc ui replay` all drive a
//! `Session`, so a recorded trace replays to the same frames. See docs/ui-protocol.md.

use crate::app::App;
use crate::ui_state::{DocumentState, UiState};
use thc_core::vault::Frontier;
use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// One input to the TUI. A trace is a list of these (after a `state` line).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "msg", rename_all = "snake_case", deny_unknown_fields)]
pub enum Msg {
    /// The clock moved: epoch milliseconds and the local offset from UTC (minutes).
    Tick {
        now_ms: u64,
        #[serde(default)]
        utc_offset_min: i32,
    },
    /// One key, as a key-script token: `j`, `<cr>`, `<c-o>`, `<s-tab>`.
    Key { key: String },
    Mouse {
        #[serde(flatten)]
        mouse: Mouse,
    },
    /// A bracketed paste.
    Paste { text: String },
    /// The terminal's size changed.
    Resize { w: u16, h: u16 },
    /// The terminal gained or lost focus (losing it saves).
    Focus { gained: bool },
    /// Replace the presentation state (fields left out take their defaults; see UiState).
    SetState {
        state: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<String>,
    },
    /// An RFC 7396 merge patch onto the presentation state.
    Patch {
        patch: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<String>,
    },
    /// What changed in the state outside any message (the daemon's push, a poll that found
    /// another device's change, an idle save, an update check), as a merge patch. The runtime
    /// records it so a trace and subscribers see every change; replay applies it.
    External { patch: Value },
    /// A frame was drawn: what waits for it ran (the save of a line just left). Recorded when
    /// something did; replay runs it there.
    Frame,
    /// The runtime's idle step for the open document passed its idle point: the typing so far
    /// becomes one undo step and is saved. Recorded when it happens; replay does it there.
    Idle,
    /// The runtime polled the vault and found changes from elsewhere (another process, another
    /// device): it reloads what's shown. Recorded when it finds some; replay polls there.
    Poll,
    /// Test fixtures: another actor writes to the vault (`THC_TUI_KEYS` only).
    Fixture { fixture: Fixture },
    /// Put something beside the person (`thc ui aside`, sidebar.md §10.3), or close a panel the
    /// actor opened.
    Aside {
        target: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        pin: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        fold: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        close: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mouse {
    pub kind: MouseKind,
    pub x: u16,
    pub y: u16,
    /// Held modifiers: any of `c` (⌃), `m` (⌥), `s` (⇧), `d` (⌘, super).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mods: String,
    /// A press's click count (2: a double click). Left out, it's counted from the clock: a
    /// left press within 400 ms on the same cell ±1 counts up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clicks: Option<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseKind {
    Down,
    Up,
    Drag,
    Moved,
    ScrollUp,
    ScrollDown,
    MiddleDown,
    MiddleUp,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Fixture {
    /// An agent adds a line to today's journal.
    Agent { text: String },
    /// Another device changes a note's text.
    Remote { id: String, text: String },
    /// The first pending alert fires.
    Alert,
}

/// What a message asks the runtime to do. The live terminal performs these; a headless
/// session reports them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "effect", rename_all = "snake_case")]
pub enum Effect {
    Quit,
    /// Hand over to a newer thc binary in place.
    Reexec,
    /// Open `$EDITOR` on a note (`id`), a saved view (`@view:name`), the keys (`@keys`) or the
    /// config (`@config`).
    SpawnEditor { target: String },
    SetMouse { on: bool },
    SwitchVault { path: std::path::PathBuf },
}

/// A terminal mouse event as a message's mouse (None: a button thc doesn't use).
pub fn mouse_msg(m: &MouseEvent) -> Option<Mouse> {
    let kind = match m.kind {
        MouseEventKind::Down(MouseButton::Left) => MouseKind::Down,
        MouseEventKind::Up(MouseButton::Left) => MouseKind::Up,
        MouseEventKind::Drag(MouseButton::Left) => MouseKind::Drag,
        MouseEventKind::Down(MouseButton::Middle) => MouseKind::MiddleDown,
        MouseEventKind::Up(MouseButton::Middle) => MouseKind::MiddleUp,
        MouseEventKind::Moved => MouseKind::Moved,
        MouseEventKind::ScrollUp => MouseKind::ScrollUp,
        MouseEventKind::ScrollDown => MouseKind::ScrollDown,
        _ => return None,
    };
    let mut mods = String::new();
    for (k, c) in [(KeyModifiers::CONTROL, 'c'), (KeyModifiers::ALT, 'm'), (KeyModifiers::SHIFT, 's'), (KeyModifiers::SUPER, 'd')] {
        if m.modifiers.contains(k) {
            mods.push(c);
        }
    }
    Some(Mouse { kind, x: m.column, y: m.row, mods, clicks: None })
}

/// The lines a session keeps by default (`--trace-limit`).
pub const TRACE_LIMIT: usize = 100_000;

pub struct Session {
    pub app: App,
    /// Goes up by one for every message applied.
    pub rev: u64,
    /// The terminal (or headless screen) size; layout follows the caret and cursor at it.
    pub size: (u16, u16),
    trace: Vec<Value>,
    /// The rev the kept trace starts at, and the index of the current segment's state line.
    trace_from: u64,
    segment: usize,
    /// How many lines were dropped from the front (trimmed).
    dropped: usize,
    pub trace_limit: usize,
    /// Every trace line also goes here (`thc tui --trace FILE`), never trimmed.
    trace_file: Option<std::io::BufWriter<std::fs::File>>,
    /// Messages applied since subscribers last heard (kept only while someone may listen).
    unannounced: Vec<Value>,
    pub announcing: bool,
    /// The state as the last message left it: a difference is an external change.
    last_seen: UiState,
    /// How far the store had read the vault's log at the last trace line (`Vault::frontier`):
    /// a `state` line pins it, and a message line carries what other writers added since.
    frontier: Option<Frontier>,
}

impl Session {
    pub fn new(app: App, size: (u16, u16)) -> Session {
        let mut s = Session { app, rev: 0, size, trace: Vec::new(), trace_from: 0, segment: 0, dropped: 0, trace_limit: TRACE_LIMIT, trace_file: None, unannounced: Vec::new(), announcing: false, last_seen: UiState::default(), frontier: None };
        s.sync_document();
        s.checkpoint();
        s
    }

    /// Write the trace to `path` too, from its current segment on.
    pub fn trace_to(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        use std::io::Write;
        let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
        for l in &self.trace[self.segment..] {
            writeln!(f, "{l}")?;
        }
        f.flush()?;
        self.trace_file = Some(f);
        Ok(())
    }

    /// The presentation state as it is now (the document's caret and scroll included).
    pub fn state(&mut self) -> &UiState {
        self.sync_document();
        &self.app.ui
    }

    /// Start a new trace segment with the current state. Neither the state nor the rev change.
    pub fn checkpoint(&mut self) {
        self.sync_document();
        self.last_seen = self.app.ui.clone();
        self.frontier = self.app.vault.frontier().ok();
        let line = self.state_line();
        self.segment = self.trace.len();
        self.push_trace(line);
    }

    /// `trace.checkpoint`: the open document's lines saved first (waiting for the save), so the
    /// vault the new segment pins holds every line on screen, then a checkpoint.
    pub fn checkpoint_saved(&mut self) {
        if self.app.doc.is_some() {
            self.app.save_everything();
            self.app.drain_saves(true);
            // The save's look ends the old segment.
            self.sync_external();
        }
        self.checkpoint();
    }

    /// A `state` line: the state, the size, the rev, and the vault it applies to (`log`, the
    /// frontier: replay rebuilds the vault as of it, so the trace's own writes land once).
    fn state_line(&self) -> Value {
        let mut line = json!({"state": self.app.ui.to_json(), "size": [self.size.0, self.size.1], "rev": self.rev});
        if let Some(f) = &self.frontier {
            line["log"] = json!(f);
        }
        // What the frames show from the process and terminal rather than the state (the theme
        // TERM and COLORTERM chose, the glyphs, the pinned-clock warning, inline images): replay
        // draws them as this session did, not as the replaying process would.
        let d = &self.app.derived;
        line["env"] = json!({"theme": self.app.theme, "pinned_warning": d.pinned_warning, "inline_images": d.inline_images});
        line
    }

    /// A `state` line's `env`, taken up by a replay (see `state_line`).
    pub fn set_env(&mut self, env: &Value) {
        if let Some(t) = env.get("theme").and_then(|t| serde_json::from_value::<crate::theme::Theme>(t.clone()).ok()) {
            self.app.theme = t;
        }
        let d = &mut self.app.derived;
        d.pinned_warning = env.get("pinned_warning").and_then(Value::as_str).map(str::to_string);
        if let Some(b) = env.get("inline_images").and_then(Value::as_bool) {
            d.inline_images = b;
        }
    }

    /// The lines other writers added to the vault's log since the last trace line (an agent's
    /// `thc add`, another device's sync), as written; this session's own writes are left out,
    /// since replaying its messages makes them again.
    fn drain_log(&mut self) -> Vec<Value> {
        let (Some(old), Ok(now)) = (self.frontier.as_ref(), self.app.vault.frontier()) else { return vec![] };
        if *old == now {
            return vec![];
        }
        let lines = match self.app.vault.foreign_lines(old, &now) {
            Ok(l) => l,
            Err(e) => {
                self.app.error(format!("ui trace: {e:#}"));
                vec![]
            }
        };
        self.frontier = Some(now);
        lines
    }

    fn push_trace(&mut self, line: Value) {
        if let Some(f) = &mut self.trace_file {
            use std::io::Write;
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
        self.trace.push(line);
        if self.trace.len() > self.trace_limit {
            if self.segment > 0 {
                // Drop the segments before the current one.
                self.dropped += self.segment;
                self.trace_from = self.trace[self.segment]["rev"].as_u64().unwrap_or(self.rev);
                self.trace.drain(..self.segment);
                self.segment = 0;
            } else {
                // One segment alone outgrew the limit: start a fresh one here.
                self.trace.clear();
                self.trace_from = self.rev;
                let line = self.state_line();
                self.trace.push(line);
            }
        }
    }

    /// The trace: the current segment (`None`), everything kept (`all`), or the lines after
    /// `since` (a `state` line first only when a segment began after it).
    pub fn trace(&self, since: Option<u64>, all: bool) -> Result<(u64, Vec<Value>), String> {
        if all {
            return Ok((self.trace_from, self.trace.clone()));
        }
        let Some(since) = since else {
            let from = self.trace[self.segment]["rev"].as_u64().unwrap_or(self.rev);
            return Ok((from, self.trace[self.segment..].to_vec()));
        };
        if since < self.trace_from {
            return Err(format!("the trace starts at rev {} now; since_rev {since} was trimmed", self.trace_from));
        }
        // Each line knows the rev it produced: messages carry it as `_rev` in the kept trace.
        let lines: Vec<Value> = self.trace.iter().filter(|l| l.get("rev").and_then(Value::as_u64).is_some_and(|r| r > since) || l.get("_rev").and_then(Value::as_u64).is_some_and(|r| r > since)).cloned().collect();
        Ok((since, lines))
    }

    /// The messages applied since the last call (for subscribers).
    pub fn take_unannounced(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.unannounced)
    }

    /// The effects the app is asking for now (its runtime flags), as values.
    pub fn pending_effects(&self) -> Vec<Effect> {
        let a = &self.app;
        let mut out = Vec::new();
        if let Some(path) = &a.switch_to {
            out.push(Effect::SwitchVault { path: path.clone() });
        }
        if let Some(id) = &a.editor_request {
            out.push(Effect::SpawnEditor { target: id.clone() });
        }
        if let Some(on) = a.mouse_request {
            out.push(Effect::SetMouse { on });
        }
        if a.reexec {
            out.push(Effect::Reexec);
        }
        if a.quit {
            out.push(Effect::Quit);
        }
        out
    }

    /// Forget requested effects (a client applied messages without performing them).
    pub fn drop_effects(&mut self) {
        let a = &mut self.app;
        a.switch_to = None;
        a.switch_focus = None;
        a.switch_return = None;
        a.editor_request = None;
        a.mouse_request = None;
        a.quit = false;
    }

    /// Put the state back exactly as `state` says (a trace's `state` line, `--state FILE`): no
    /// history step, no toast. Fields it leaves out take defaults (the session's own clock and
    /// vault). Starts a new trace segment.
    pub fn restore(&mut self, state: &Value) -> Result<(), String> {
        let new = UiState::from_json_over(&self.app.ui, state, state.get("history").is_none())?;
        let new = self.same_vault(new)?;
        let doc = new.document.clone();
        self.app.ui = new;
        rehydrate(&mut self.app);
        let _ = self.app.reload();
        if let (Some(ds), Some(d)) = (doc, self.app.doc.as_mut()) {
            if !ds.caret_id.is_empty() {
                // The caret goes back as it does on reopening (a remembered caret): a day's
                // fresh last line is only there when the caret is on it.
                let journal = matches!(d.target, crate::editor::Target::Journal { .. });
                d.restore_caret(&crate::editor::Anchor { id: ds.caret_id, byte: ds.caret_byte }, journal);
            }
            d.set_scroll(ds.scroll, self.app.ui.doc_scroll_free);
        }
        self.follow();
        self.rev += 1;
        self.checkpoint();
        Ok(())
    }

    /// Record what changed outside messages since the last one, as an `external` message (no-op
    /// when nothing did). The runtime calls this between frames and before input.
    pub fn sync_external(&mut self) {
        self.sync_document();
        if self.app.ui == self.last_seen {
            return;
        }
        let patch = crate::ui_state::merge_diff(&self.last_seen.to_json(), &self.app.ui.to_json());
        if let Err(e) = self.apply(Msg::External { patch }) {
            // A difference that doesn't round-trip: keep going, start a segment from here.
            self.app.error(format!("ui trace: {e}"));
            self.checkpoint();
        }
    }

    /// An external change replayed: the state takes the patch as it came (no history step, no
    /// toast). Live, the state already has it and nothing happens.
    fn external(&mut self, patch: &Value) -> Result<(), String> {
        let new = self.last_seen.patched(patch)?;
        if new == self.app.ui {
            return Ok(());
        }
        const SHAPING: &[&str] = &["view", "page_open", "journal_date", "tasks_filter", "search_terms", "pages_filter", "log_actor", "log_node", "review_lane", "agenda_mode", "show_all_done", "context_on", "scope_override", "today_by_vault", "collapsed", "today"];
        let changed = self.app.ui.changed_fields(&new);
        let doc = new.document.clone();
        self.app.ui = new;
        rehydrate(&mut self.app);
        if changed.iter().any(|f| SHAPING.contains(&f.as_str())) {
            let _ = self.app.reload();
        }
        if let (true, Some(ds), Some(d)) = (changed.iter().any(|f| f == "document"), doc, self.app.doc.as_mut()) {
            if !ds.caret_id.is_empty() {
                d.set_caret_anchor(&crate::editor::Anchor { id: ds.caret_id, byte: ds.caret_byte });
            }
            d.set_scroll(ds.scroll, self.app.ui.doc_scroll_free);
        }
        Ok(())
    }

    /// The runtime's tick: when the wall clock (or `THC_NOW`) has moved, a `Tick` message.
    pub fn tick_wall(&mut self) {
        let (now_ms, utc_offset_min) = crate::runtime_effects::wall_clock();
        if (now_ms, utc_offset_min) != (self.app.ui.now_ms, self.app.ui.utc_offset_min) {
            let _ = self.apply(Msg::Tick { now_ms, utc_offset_min });
        }
    }

    /// Validates a message without applying it (a state that doesn't parse, an unknown key).
    pub fn check(&self, msg: &Msg) -> Result<(), String> {
        match msg {
            Msg::Key { key } if crate::script::key_event(key).is_none() => Err(format!("unknown key {key}")),
            Msg::SetState { state, actor } => self.parse_state(state).and_then(|s| self.agent_may(&s, actor.as_deref())),
            Msg::Patch { patch, actor } => self.app.ui.patched(patch).and_then(|s| self.same_vault(s)).and_then(|s| self.agent_may(&s, actor.as_deref())),
            Msg::Resize { w, h } if *w == 0 || *h == 0 => Err("a size is at least 1x1".into()),
            _ => Ok(()),
        }
    }

    fn parse_state(&self, state: &Value) -> Result<UiState, String> {
        UiState::from_json_over(&self.app.ui, state, state.get("history").is_none()).and_then(|s| self.same_vault(s))
    }

    /// What an agent may not do to the sidebar (sidebar.md §10.4): close or unpin a pinned
    /// panel. (Focus and `opened_by` are put right when the state lands, in `set_state`.)
    fn agent_may(&self, s: &UiState, actor: Option<&str>) -> Result<(), String> {
        if actor.is_none() {
            return Ok(());
        }
        for (i, p) in self.app.ui.sidebar.open.iter().enumerate().filter(|(_, p)| p.pinned) {
            let k = p.key();
            match s.sidebar.get(&k) {
                None => return Err(format!("sidebar.open: {} is pinned (open[{i}]) · a pinned panel is the person's; an agent can't close it", self.app.panel_name(&k))),
                Some(q) if !q.pinned => return Err(format!("sidebar.open: {} is pinned · an agent can't unpin it", self.app.panel_name(&k))),
                _ => {}
            }
        }
        Ok(())
    }

    fn same_vault(&self, s: UiState) -> Result<UiState, String> {
        if s.vault_name != self.app.ui.vault_name {
            return Err(format!("vault_name is {} here; a state can't switch vaults (open thc on that vault)", self.app.ui.vault_name));
        }
        Ok(s)
    }

    /// Apply one message. Err: it was refused and nothing changed (not even the rev).
    pub fn apply(&mut self, msg: Msg) -> Result<Vec<Effect>, String> {
        self.check(&msg)?;
        // What other writers added before this message is what it ran on: its line carries it.
        let before = self.frontier.clone();
        let foreign = self.drain_log();
        if let Err(e) = self.run(&msg) {
            self.frontier = before;
            return Err(e);
        }
        if matches!(msg, Msg::Fixture { .. }) {
            // A fixture's write is the message's own: replaying the message makes it again.
            self.frontier = self.app.vault.frontier().ok().or(self.frontier.take());
        }
        self.record(&msg, foreign);
        Ok(self.pending_effects())
    }

    /// What a message does to the state (and, for writes, the vault).
    fn run(&mut self, msg: &Msg) -> Result<(), String> {
        match msg.clone() {
            Msg::Tick { now_ms, utc_offset_min } => self.app.ui.tick(now_ms, utc_offset_min),
            Msg::Key { key } => {
                let k = crate::script::key_event(&key).expect("checked");
                crate::input::handle_key(&mut self.app, k);
            }
            Msg::Mouse { mouse } => self.mouse(mouse),
            Msg::Paste { text } => {
                if crate::sidebar_app::paste(&mut self.app, &text) {
                } else if self.app.doc.is_some() {
                    crate::doc_keys::paste(&mut self.app, &text);
                }
            }
            Msg::Resize { w, h } => {
                self.size = (w, h);
                // Geometry from the old size never answers a click at the new one.
                self.app.render = crate::ui::RenderOutput::default();
            }
            Msg::Focus { gained } => {
                if gained {
                    self.app.check_installed(true);
                } else {
                    self.app.save_everything();
                }
            }
            Msg::SetState { state, actor } => {
                let new = self.parse_state(&state)?;
                self.set_state(new, actor.as_deref());
            }
            Msg::Patch { patch, actor } => {
                let new = self.app.ui.patched(&patch)?;
                self.set_state(new, actor.as_deref());
            }
            Msg::Fixture { fixture } => self.fixture(fixture),
            Msg::Aside { target, pin, fold, close, actor } => {
                self.app.agent_aside(&target, pin, fold, close, actor.as_deref()).map_err(|e| e.message())?;
            }
            Msg::External { patch } => self.external(&patch)?,
            Msg::Frame => {
                self.app.after_frame();
            }
            Msg::Idle => {
                self.app.doc_tick();
            }
            Msg::Poll => {
                if let Err(e) = self.app.poll_external() {
                    self.app.error(format!("sync: {e:#}"));
                }
            }
        }
        Ok(())
    }

    /// One of the runtime's own steps that read or write the vault outside any message (`Frame`,
    /// `Idle`, `Poll`), run now; recorded as that message only when it did something, so a replay runs
    /// it at the same point. What the step changed in the state is left for the next `external`
    /// message, as before: the step's line carries its data, the patch its look.
    pub fn runtime(&mut self, msg: Msg) {
        // Nothing waits for this frame (most keys): nothing to run or record, and no state read.
        if matches!(msg, Msg::Frame) && !self.app.doc_save_after_frame {
            return;
        }
        // Anything still unrecorded is recorded first, apart from the step.
        self.sync_external();
        let did = match &msg {
            Msg::Frame => self.app.after_frame(),
            Msg::Idle => self.app.doc_tick(),
            Msg::Poll => match self.app.poll_external() {
                Ok(did) => did,
                Err(e) => {
                    self.app.error(format!("sync: {e:#}"));
                    false
                }
            },
            _ => unreachable!("not a runtime step: {msg:?}"),
        };
        if did {
            // A poll took in what it found during the step: the line carries that too.
            let foreign = self.drain_log();
            self.record(&msg, foreign);
        }
    }

    /// Writes this TUI made since `mark` (`Vault::frontier` before them) that no message makes again ($EDITOR's round trip):
    /// recorded as a `poll` that carries them, so a replay takes them in as it would another
    /// writer's. (Before the mark, its own writes are its messages', left out as usual.)
    pub fn outside_writes(&mut self, mark: Option<Frontier>) {
        self.sync_external();
        let (Some(old), Some(mark), Ok(now)) = (self.frontier.clone(), mark, self.app.vault.frontier()) else { return };
        let lines = self.app.vault.log_lines(&old, &mark, true).and_then(|mut a| {
            a.extend(self.app.vault.log_lines(&mark, &now, false)?);
            Ok(a)
        });
        let lines = match lines {
            Ok(l) => l,
            Err(e) => {
                self.app.error(format!("ui trace: {e:#}"));
                return;
            }
        };
        self.frontier = Some(now);
        if !lines.is_empty() {
            self.record(&Msg::Poll, lines);
        }
    }

    /// The bookkeeping after a message: layout follows, the rev moves, and the line goes into
    /// the trace (with `_log`, what other writers added that it ran on) and to subscribers.
    fn record(&mut self, msg: &Msg, foreign: Vec<Value>) {
        let line = serde_json::to_value(msg).expect("Msg serializes");
        self.follow();
        self.sync_document();
        // A runtime step's look is the next `external` patch's, measured from before it.
        if !matches!(msg, Msg::Frame | Msg::Idle | Msg::Poll) {
            self.last_seen = self.app.ui.clone();
        }
        self.rev += 1;
        if self.announcing {
            self.unannounced.push(line.clone());
        }
        let mut line = line;
        line["_rev"] = json!(self.rev);
        if !foreign.is_empty() {
            line["_log"] = Value::Array(foreign);
        }
        self.push_trace(line);
    }

    /// Layout follows the caret and the list cursor at the session's size: part of applying a
    /// message, so where a view scrolls never depends on when a frame was drawn.
    fn follow(&mut self) {
        let (w, h) = self.size;
        crate::ui::follow_frame(&mut self.app, Rect::new(0, 0, w, h));
    }

    fn mouse(&mut self, m: Mouse) {
        if !self.app.tui_prefs.mouse {
            // With capture off the terminal sends nothing.
            return;
        }
        let mut mods = KeyModifiers::NONE;
        for (c, k) in [('c', KeyModifiers::CONTROL), ('m', KeyModifiers::ALT), ('s', KeyModifiers::SHIFT), ('d', KeyModifiers::SUPER)] {
            if m.mods.contains(c) {
                mods |= k;
            }
        }
        let kind = match m.kind {
            MouseKind::Down => MouseEventKind::Down(MouseButton::Left),
            MouseKind::Up => MouseEventKind::Up(MouseButton::Left),
            MouseKind::Drag => MouseEventKind::Drag(MouseButton::Left),
            MouseKind::Moved => MouseEventKind::Moved,
            MouseKind::ScrollUp => MouseEventKind::ScrollUp,
            MouseKind::ScrollDown => MouseEventKind::ScrollDown,
            MouseKind::MiddleDown => MouseEventKind::Down(MouseButton::Middle),
            MouseKind::MiddleUp => MouseEventKind::Up(MouseButton::Middle),
        };
        let x = if m.x == u16::MAX { self.app.screen_width / 2 } else { m.x };
        let ev = MouseEvent { kind, column: x, row: m.y, modifiers: mods };
        let clicks = match m.clicks {
            Some(n) => {
                if m.kind == MouseKind::Down {
                    self.app.ui.last_click = Some((self.app.ui.now_ms, ev.column, ev.row, n));
                }
                n
            }
            None => self.count_clicks(&ev),
        };
        crate::input::handle_mouse(&mut self.app, ev, clicks);
    }

    /// Double and triple clicks: another left press within 400 ms (logical clock) on the same
    /// cell ±1 counts up, to 3 (mouse.md §8).
    fn count_clicks(&mut self, m: &MouseEvent) -> u8 {
        let ui = &mut self.app.ui;
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let n = match ui.last_click {
                    Some((at, x, y, n)) if ui.now_ms.saturating_sub(at) < 400 && x.abs_diff(m.column) <= 1 && y.abs_diff(m.row) <= 1 => (n % 3) + 1,
                    _ => 1,
                };
                ui.last_click = Some((ui.now_ms, m.column, m.row, n));
                n
            }
            // A release or drag counts as one, as the terminal reports it.
            _ => 1,
        }
    }

    fn fixture(&mut self, f: Fixture) {
        let r = match f {
            Fixture::Agent { text } => crate::agent_write(&self.app, &text),
            Fixture::Remote { id, text } => crate::remote_write(&self.app, &id, &text),
            Fixture::Alert => {
                self.app.simulate_alert();
                return;
            }
        };
        match r {
            Ok(()) => {
                let _ = self.app.poll_external();
            }
            Err(e) => self.app.error(format!("fixture: {e:#}")),
        }
    }

    /// A pushed state lands like a step you took: where you were becomes a history step, the
    /// document opens parked (never mid-sentence in Write), its caret goes where the state says,
    /// and an agent's change says so in a toast (⌘[ goes back).
    fn set_state(&mut self, new: UiState, actor: Option<&str>) {
        let changed = self.app.ui.changed_fields(&new);
        if changed.is_empty() {
            return;
        }
        let mut new = new;
        let stack_before = self.app.ui.sidebar.clone();
        if let Some(a) = actor {
            // Agents never move the keyboard (§10.4): a `focus` or `sidebar.focused` change
            // makes that panel the active one, and focus stays where the person had it.
            if !crate::sidebar::policy::AGENTS_MOVE_FOCUS {
                new.focus = self.app.ui.focus;
                if new.focus == crate::app::Focus::Sidebar && !new.sidebar.has_panels() {
                    new.focus = crate::app::Focus::List;
                }
            }
            // `opened_by` is the TUI's: a panel the agent added is its; the rest keep theirs.
            for p in new.sidebar.open.iter_mut() {
                p.opened_by = match stack_before.get(&p.key()) {
                    Some(old) => old.opened_by.clone(),
                    None => Some(a.to_string()),
                };
            }
        }
        let app = &mut self.app;
        app.history_tick(false);
        let doc = new.document.clone();
        let doc_changed = changed.iter().any(|f| f == "document");
        app.ui = new;
        rehydrate(app);
        let _ = app.reload();
        if let (true, Some(ds), Some(d)) = (doc_changed, doc, app.doc.as_mut()) {
            if !ds.caret_id.is_empty() {
                d.set_caret_anchor(&crate::editor::Anchor { id: ds.caret_id, byte: ds.caret_byte });
            }
            d.set_scroll(ds.scroll, true);
            app.ui.doc_scroll_free = true;
        }
        app.history_tick(false);
        if let Some(a) = actor {
            let change = crate::sidebar::AgentChange::between(a, &stack_before, &app.ui.sidebar);
            if !change.is_empty() {
                app.history_agent_step(change);
            }
        }
        if app.ui.sidebar != stack_before {
            app.persist_sidebar();
        }
        if let Some(actor) = actor {
            let g = app.theme.glyphs();
            let back = if app.cmd_seen { "⌘[" } else { "⌃⌥←" };
            app.toast_parts(crate::app::ToastKind::Agent, vec![(format!("{} {actor} changed your view {} {back} back", g.agent, g.sep), crate::theme::Token::Agent)]);
        }
    }

    /// The document's caret and scroll, read from the editor into the state.
    pub fn sync_document(&mut self) {
        let doc = self.app.doc.as_ref().map(|d| {
            let a = d.caret_anchor();
            DocumentState {
                target: Some(d.target.clone()),
                caret_id: if d.caret_block().is_new { String::new() } else { a.id },
                caret_byte: a.byte,
                scroll: d.scroll(),
                dirty: d.blocks().iter().any(|l| l.edited()),
                revision: d.revision(),
            }
        });
        if self.app.ui.document != doc {
            self.app.ui.document = doc;
        }
        // The engine's view keeps whether it was scrolled freely; the state records it.
        let free = self.app.doc.as_ref().is_some_and(|d| d.scroll_free());
        if self.app.ui.doc_scroll_free != free {
            self.app.ui.doc_scroll_free = free;
        }
    }

    /// Draw one frame headlessly. The state is the same afterwards: a size other than the
    /// session's is drawn and then the scroll it would have followed to is put back.
    pub fn render(&mut self, w: u16, h: u16, format: &str) -> Result<Rendered, String> {
        if w == 0 || h == 0 {
            return Err("a size is at least 1x1".into());
        }
        let saved = (self.app.ui.clone(), self.app.doc.as_ref().map(|d| (d.scroll(), d.scroll_free())), self.app.render.clone());
        let mut term = ratatui::Terminal::new(crate::quiet::Snap { inner: ratatui::backend::TestBackend::new(w, h), visible: false }).map_err(|e| e.to_string())?;
        term.draw(|f| crate::ui::draw_app(f, &mut self.app)).map_err(|e| e.to_string())?;
        let buf = term.backend().inner.buffer().clone();
        let cursor = {
            use ratatui::backend::Backend;
            let visible = term.backend().visible;
            let Ok(p) = term.backend_mut().get_cursor_position();
            (visible && p.x < w && p.y < h).then_some([p.x, p.y])
        };
        if (w, h) != self.size {
            self.app.ui = saved.0;
            if let (Some(d), Some((s, free))) = (self.app.doc.as_mut(), saved.1) {
                d.set_scroll(s, free);
            }
            self.app.render = saved.2;
        }
        let out = match format {
            "text" => Rendered { cursor, frame: Some(frame_text(&buf)), rows: None },
            "ansi" => Rendered { cursor, frame: Some(crate::snapshot_fmt::ansi(&buf)), rows: None },
            "html" => Rendered { cursor, frame: Some(crate::snapshot_fmt::html(&buf, &self.app.theme)), rows: None },
            "cells" => Rendered { cursor, frame: None, rows: Some(cells(&buf)) },
            other => return Err(format!("format {other}: text, ansi, html or cells")),
        };
        Ok(out)
    }
}

/// A rendered frame: text (`frame`) or styled rows (`rows`), and the cursor when it shows.
pub struct Rendered {
    pub cursor: Option<[u16; 2]>,
    pub frame: Option<String>,
    pub rows: Option<Value>,
}

/// Overlay contents that come from the registry or the environment are derived, not state:
/// rebuilt after a state arrives.
fn rehydrate(app: &mut App) {
    use crate::app::Overlay;
    match app.ui.overlay.take() {
        Some(Overlay::Vaults { sel, naming, .. }) => {
            app.open_vault_picker();
            if let Some(Overlay::Vaults { rows, sel: s, naming: n }) = app.ui.overlay.as_mut() {
                *s = sel.min(rows.len().saturating_sub(1));
                *n = naming;
            }
        }
        Some(Overlay::About(mut a)) => {
            a.version = app.derived.version.clone();
            a.facts = crate::about::facts(app, &a.version);
            app.ui.overlay = Some(Overlay::About(a));
        }
        other => app.ui.overlay = other,
    }
}

/// The frame as text: one line per row, trailing spaces trimmed, wide characters once.
pub fn frame_text(buf: &ratatui::buffer::Buffer) -> String {
    let mut out = String::new();
    for y in 0..buf.area.height {
        let mut line = String::new();
        let mut skip = 0;
        for x in 0..buf.area.width {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let sym = buf[(x, y)].symbol();
            skip = unicode_width::UnicodeWidthStr::width(sym).saturating_sub(1);
            line.push_str(sym);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// Each row's text, and runs of style as `[x, len, fg, bg, modifiers]`.
fn cells(buf: &ratatui::buffer::Buffer) -> Value {
    let color = |c: ratatui::style::Color| format!("{c}").to_lowercase();
    let mut rows = Vec::new();
    for y in 0..buf.area.height {
        let mut text = String::new();
        let mut spans: Vec<Value> = Vec::new();
        let mut run: Option<(u16, u16, String, String, String)> = None;
        let mut skip = 0;
        for x in 0..buf.area.width {
            let cell = &buf[(x, y)];
            if skip > 0 {
                skip -= 1;
            } else {
                text.push_str(cell.symbol());
                skip = unicode_width::UnicodeWidthStr::width(cell.symbol()).saturating_sub(1);
            }
            let style = (color(cell.fg), color(cell.bg), format!("{:?}", cell.modifier).to_lowercase());
            match &mut run {
                Some((_, len, fg, bg, m)) if (fg.as_str(), bg.as_str(), m.as_str()) == (style.0.as_str(), style.1.as_str(), style.2.as_str()) => *len += 1,
                _ => {
                    if let Some((x0, len, fg, bg, m)) = run.take() {
                        spans.push(json!([x0, len, fg, bg, m]));
                    }
                    run = Some((x, 1, style.0, style.1, style.2));
                }
            }
        }
        if let Some((x0, len, fg, bg, m)) = run {
            spans.push(json!([x0, len, fg, bg, m]));
        }
        rows.push(json!({"text": text.trim_end(), "spans": spans}));
    }
    Value::Array(rows)
}

/// Opens a headless session for replay on a scratch copy of the vault: as of a frontier (a
/// `state` line's `log`), or as the vault is now for a trace that has none.
pub type Open<'a> = dyn FnMut(Option<&Frontier>) -> Result<Session, String> + 'a;

/// Replay a trace (JSON lines: `state` lines and messages); the frames after every line with
/// `every`, else only the last. Frames are drawn at `size`, or else at the session's size (the
/// trace's own, as its `state` lines and `resize` messages set it). Layout follows the trace's
/// size either way, as the live TUI's did, so `size` gives the frame `thc ui render WxH` drew of
/// the live TUI.
///
/// Each `state` line with a `log` frontier starts a session of its own, on the vault as of that
/// frontier: the writes the trace's messages make land once, on the vault they were made on. A
/// line's `_log` (what other writers added that the line ran on) goes into the vault before the
/// line runs, and the runtime's own steps (`idle`, `poll`) run where they ran live.
pub fn replay(open: &mut Open, trace: &str, size: Option<(u16, u16)>, format: &str, every: bool) -> Result<Vec<String>, String> {
    let mut frames = Vec::new();
    let mut session: Option<Session> = None;
    let draw = |s: &mut Session, frames: &mut Vec<String>| -> Result<(), String> {
        let (w, h) = size.unwrap_or(s.size);
        let r = s.render(w, h, format)?;
        frames.push(r.frame.unwrap_or_else(|| r.rows.map(|r| r.to_string()).unwrap_or_default()));
        Ok(())
    };
    for (i, line) in trace.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let at = |e: String| format!("line {}: {e}", i + 1);
        let mut v: Value = serde_json::from_str(line).map_err(|e| at(format!("not JSON: {e}")))?;
        if let Some(state) = v.get("state") {
            let pin: Option<Frontier> = v.get("log").map(|l| serde_json::from_value(l.clone())).transpose().map_err(|e| at(format!("log: {e}")))?;
            if session.is_none() || pin.is_some() {
                session = Some(open(pin.as_ref()).map_err(at)?);
            }
            let s = session.as_mut().expect("opened");
            if let Some(a) = v.get("size").and_then(|s| serde_json::from_value::<[u16; 2]>(s.clone()).ok()) {
                s.size = (a[0], a[1]);
            }
            s.restore(state).map_err(at)?;
            if let Some(env) = v.get("env") {
                s.set_env(env);
            }
        } else {
            if session.is_none() {
                session = Some(open(None).map_err(at)?);
            }
            let s = session.as_mut().expect("opened");
            let log = v.as_object_mut().and_then(|o| {
                o.remove("_rev");
                o.remove("_log")
            });
            let msg: Msg = serde_json::from_value(v).map_err(|e| at(e.to_string()))?;
            if let Some(Value::Array(lines)) = log {
                s.app.vault.ingest_lines(&lines).map_err(|e| at(format!("_log: {e:#}")))?;
            }
            s.apply(msg).map_err(at)?;
            s.drop_effects();
        }
        if every {
            draw(session.as_mut().expect("opened"), &mut frames)?;
        }
    }
    if !every {
        if session.is_none() {
            session = Some(open(None)?);
        }
        draw(session.as_mut().expect("opened"), &mut frames)?;
    }
    Ok(frames)
}
