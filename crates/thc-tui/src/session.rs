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
    /// Test fixtures: another actor writes to the vault (`THC_TUI_KEYS` only).
    Fixture { fixture: Fixture },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mouse {
    pub kind: MouseKind,
    pub x: u16,
    pub y: u16,
    /// Held modifiers: any of `c` (⌃), `m` (⌥), `s` (⇧).
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
    for (k, c) in [(KeyModifiers::CONTROL, 'c'), (KeyModifiers::ALT, 'm'), (KeyModifiers::SHIFT, 's')] {
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
}

impl Session {
    pub fn new(app: App, size: (u16, u16)) -> Session {
        let mut s = Session { app, rev: 0, size, trace: Vec::new(), trace_from: 0, segment: 0, dropped: 0, trace_limit: TRACE_LIMIT, trace_file: None, unannounced: Vec::new(), announcing: false, last_seen: UiState::default() };
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
        let line = json!({"state": self.app.ui.to_json(), "size": [self.size.0, self.size.1], "rev": self.rev});
        self.segment = self.trace.len();
        self.push_trace(line);
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
                let line = json!({"state": self.app.ui.to_json(), "size": [self.size.0, self.size.1], "rev": self.rev});
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
                d.set_caret_anchor(&crate::editor::Anchor { id: ds.caret_id, byte: ds.caret_byte });
            }
            d.scroll = ds.scroll;
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
            d.scroll = ds.scroll;
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
            Msg::SetState { state, .. } => self.parse_state(state).map(|_| ()),
            Msg::Patch { patch, .. } => self.app.ui.patched(patch).and_then(|s| self.same_vault(s)).map(|_| ()),
            Msg::Resize { w, h } if *w == 0 || *h == 0 => Err("a size is at least 1x1".into()),
            _ => Ok(()),
        }
    }

    fn parse_state(&self, state: &Value) -> Result<UiState, String> {
        UiState::from_json_over(&self.app.ui, state, state.get("history").is_none()).and_then(|s| self.same_vault(s))
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
        let line = serde_json::to_value(&msg).expect("Msg serializes");
        match msg {
            Msg::Tick { now_ms, utc_offset_min } => self.app.ui.tick(now_ms, utc_offset_min),
            Msg::Key { key } => {
                let k = crate::script::key_event(&key).expect("checked");
                crate::input::handle_key(&mut self.app, k);
            }
            Msg::Mouse { mouse } => self.mouse(mouse),
            Msg::Paste { text } => {
                if self.app.doc.is_some() {
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
                    self.app.save_doc(true);
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
            Msg::External { patch } => self.external(&patch)?,
        }
        self.follow();
        self.sync_document();
        self.last_seen = self.app.ui.clone();
        self.rev += 1;
        if self.announcing {
            self.unannounced.push(line.clone());
        }
        let mut line = line;
        line["_rev"] = json!(self.rev);
        self.push_trace(line);
        Ok(self.pending_effects())
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
        for (c, k) in [('c', KeyModifiers::CONTROL), ('m', KeyModifiers::ALT), ('s', KeyModifiers::SHIFT)] {
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
            d.scroll = ds.scroll;
            app.ui.doc_scroll_free = true;
        }
        app.history_tick(false);
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
                scroll: d.scroll,
                dirty: d.blocks().iter().any(|l| l.edited()),
                revision: d.revision(),
            }
        });
        if self.app.ui.document != doc {
            self.app.ui.document = doc;
        }
    }

    /// Draw one frame headlessly. The state is the same afterwards: a size other than the
    /// session's is drawn and then the scroll it would have followed to is put back.
    pub fn render(&mut self, w: u16, h: u16, format: &str) -> Result<Rendered, String> {
        if w == 0 || h == 0 {
            return Err("a size is at least 1x1".into());
        }
        let saved = (self.app.ui.clone(), self.app.doc.as_ref().map(|d| d.scroll), self.app.render.clone());
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
            if let (Some(d), Some(s)) = (self.app.doc.as_mut(), saved.1) {
                d.scroll = s;
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

/// Replay a trace (JSON lines: `state` lines and messages) onto a session; the frames after
/// every line with `every`, else only the last.
pub fn replay(session: &mut Session, trace: &str, size: Option<(u16, u16)>, format: &str, every: bool) -> Result<Vec<String>, String> {
    let mut frames = Vec::new();
    let mut size_set = size.is_some();
    if let Some(s) = size {
        session.size = s;
    }
    for (i, line) in trace.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line).map_err(|e| format!("line {}: not JSON: {e}", i + 1))?;
        if let Some(state) = v.get("state") {
            if !size_set {
                if let Some([w, h]) = v.get("size").and_then(|s| serde_json::from_value::<[u16; 2]>(s.clone()).ok()).map(|a| [a[0], a[1]]) {
                    session.size = (w, h);
                    size_set = true;
                }
            }
            session.restore(state).map_err(|e| format!("line {}: {e}", i + 1))?;
        } else {
            let mut v = v;
            if let Some(o) = v.as_object_mut() {
                o.remove("_rev");
            }
            let msg: Msg = serde_json::from_value(v).map_err(|e| format!("line {}: {e}", i + 1))?;
            session.apply(msg).map_err(|e| format!("line {}: {e}", i + 1))?;
            session.drop_effects();
        }
        if every {
            let (w, h) = session.size;
            let r = session.render(w, h, format)?;
            frames.push(r.frame.unwrap_or_else(|| r.rows.map(|r| r.to_string()).unwrap_or_default()));
        }
    }
    if !every {
        let (w, h) = session.size;
        let r = session.render(w, h, format)?;
        frames.push(r.frame.unwrap_or_else(|| r.rows.map(|r| r.to_string()).unwrap_or_default()));
    }
    Ok(frames)
}
