//! TUI state, data loading and actions. All writes go through `thc_core::builder::TxBuilder`
//! via `Vault::transact`, exactly like the CLI, with `via = "tui"`.

use crate::input::LineInput;
use crate::theme::Token;
use anyhow::Result;
use chrono::{Duration, NaiveDate};
use std::collections::{HashMap, HashSet};
use std::time::Instant;
use thc_core::builder::TxBuilder;
use thc_core::capture;
use thc_core::dates;
use thc_core::event::Event;
use thc_core::model::{HistoryEntry, Node};
use thc_core::vault::Vault;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum View {
    Today,
    Inbox,
    Tasks,
    Pages,
    Journal,
    Search,
    Log,
}

pub const VIEWS: [View; 7] = [View::Today, View::Inbox, View::Tasks, View::Pages, View::Journal, View::Search, View::Log];

impl View {
    pub fn name(&self) -> &'static str {
        match self {
            View::Today => "Today",
            View::Inbox => "Inbox",
            View::Tasks => "Tasks",
            View::Pages => "Pages",
            View::Journal => "Journal",
            View::Search => "Search",
            View::Log => "Log",
        }
    }
}

#[derive(Clone, Debug)]
pub enum Row {
    Section { title: String, count: Option<usize>, token: Token, note: Option<String> },
    Node { node: Node, depth: usize, outline: bool, has_children: bool, collapsed: bool, child_count: usize, under_day: Option<NaiveDate> },
    Tag { id: String, name: String, count: usize },
    /// A saved view in the Pages finder (`@` filter): `@week  status:open due<=+7d  5`.
    ViewItem { name: String, query: String, count: usize },
    Tx { tx: String, entries: Vec<HistoryEntry> },
    TxDetail { tx: String, text: String },
    Empty { l1: String, l2: String },
    Muted(String),
    /// A styled, non-selectable line with an optional right-aligned muted note.
    /// `narrow`: the parts instead on a main area under 120 columns (decided when drawn, so
    /// the rows don't depend on the width they were last read at; cjn86).
    Note { parts: Vec<(String, Token)>, right: Option<String>, narrow: Option<Vec<(String, Token)>> },
    Blank,
    /// A new line being written in place (tui-handoff §10); never saved while empty.
    Editing,
    /// `+ new page "…"` in the Pages finder (§10.6): reached only with ↓, never the default.
    NewPage { title: String },
}

impl Row {
    pub fn key(&self) -> Option<String> {
        match self {
            Row::Node { node, .. } => Some(node.id.clone()),
            Row::Tag { name, .. } => Some(format!("tag:{name}")),
            Row::ViewItem { name, .. } => Some(format!("view:{name}")),
            Row::Tx { tx, .. } => Some(format!("tx:{tx}")),
            Row::Editing => Some(EDIT_KEY.into()),
            Row::NewPage { title } => Some(format!("newpage:{title}")),
            _ => None,
        }
    }

    pub fn selectable(&self) -> bool {
        self.key().is_some()
    }

    pub fn node(&self) -> Option<&Node> {
        match self {
            Row::Node { node, .. } => Some(node),
            _ => None,
        }
    }
}

/// The row key of a new line being edited.
pub const EDIT_KEY: &str = "edit:new";

/// In-place editing (tui-handoff §10): one outline row is a text input. For an existing
/// node `node` is set; a new line has `node: None` and sits by `after` / `before` (or last
/// under `parent`), shown as a `Row::Editing`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Edit {
    pub node: Option<String>,
    pub parent: Option<String>,
    pub after: Option<String>,
    pub before: Option<String>,
    pub depth: usize,
    pub input: LineInput,
    /// The text when editing began (an unchanged line writes nothing).
    pub orig: String,
    /// A refusal or failure from the last save, shown in the chips area (§10.7).
    pub error: Option<String>,
    /// Written as a paragraph (§10.8): new lines inherit it from the line they came from.
    pub para: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum PromptKind {
    Due(String),
    Sched(String),
    Text(String),
    Tags(String),
    Filter,
    Search,
    GoDate,
    PagesFilter,
    NewPage,
    Snooze(String),
    /// `c` in a view's recipe: the new view's name (copying `@name`).
    ViewCopy(String),
}

impl PromptKind {
    pub fn label(&self) -> &'static str {
        match self {
            PromptKind::Due(_) => "due",
            PromptKind::Sched(_) => "scheduled",
            PromptKind::Text(_) => "text",
            PromptKind::Tags(_) => "tags",
            PromptKind::Filter => "q",
            PromptKind::Search => "/",
            PromptKind::GoDate => "go to date",
            PromptKind::PagesFilter => "find",
            PromptKind::NewPage => "new page",
            PromptKind::Snooze(_) => "snooze until",
            PromptKind::ViewCopy(_) => "copy as view",
        }
    }

    /// Prompts drawn in the view's own input row instead of the bar.
    pub fn inline(&self) -> bool {
        matches!(self, PromptKind::Filter | PromptKind::Search | PromptKind::PagesFilter)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureTarget {
    Journal(NaiveDate),
    Under(String, String),
    Inbox,
}

impl CaptureTarget {
    pub fn label(&self, today: NaiveDate, g: &crate::theme::Glyphs) -> String {
        match self {
            CaptureTarget::Journal(d) if *d == today => format!("{} today", g.journal),
            CaptureTarget::Journal(d) => format!("{} {}", g.journal, d.format("%b %-d")),
            CaptureTarget::Under(_, label) => label.clone(),
            CaptureTarget::Inbox => "inbox".into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct MoveItem {
    pub label: String,
    pub note: String,
    pub target: MoveTarget,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MoveTarget {
    Inbox,
    Journal(NaiveDate),
    Under(String),
    NewPage(String),
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Overlay {
    /// `scroll`: the first line shown when the keys don't fit (↑↓ PgUp PgDn, the wheel).
    Help { all: bool, scroll: u16 },
    Palette { input: LineInput, sel: usize },
    Capture { input: LineInput, targets: Vec<CaptureTarget>, which: usize },
    Move { node: String, input: LineInput, sel: usize },
    Compare { detail: thc_core::model::ConflictDetail },
    /// `:focus`: what Focus shows, toggled live (tui-editor.md §8.4).
    Focus,
    /// ⌃O: go to a page or a day by name (writing.md §3).
    Finder { input: LineInput, sel: usize },
    /// `V` / `space g v`: the vaults on this device (vaults.md §8). `naming`: `n` asks for a
    /// new vault's name.
    Vaults {
        /// Derived from the registry when the picker opens (and again after a state set).
        #[serde(skip)]
        rows: Vec<VaultRow>,
        sel: usize,
        naming: Option<LineInput>,
    },
    /// `:history` / `space g h`: the places, newest first (navigation.md §7.5). `sel` counts
    /// from the newest.
    History { sel: usize },
    /// `*` on Today: which vaults it reads (view-explain.md §2). Rows: all, this vault, then each
    /// vault; `picked` is the vault names checked (all of them for "all").
    Scope { sel: usize, picked: Vec<String> },
    /// `?` on a view: how it's built (view-explain.md §3), by view name.
    Recipe { name: String },
    /// `:about`, the footer version, `space a`: what changed and what this thc is (about.md).
    About(Box<crate::about::About>),
}

impl Overlay {
    /// The selected row of an overlay that is a list (mouse.md "Overlays are menus"): a click
    /// sets it and acts as Enter, hover sets it, the wheel moves it.
    pub fn sel_mut(&mut self) -> Option<&mut usize> {
        match self {
            Overlay::Palette { sel, .. } | Overlay::Finder { sel, .. } | Overlay::Move { sel, .. } => Some(sel),
            Overlay::Vaults { sel, naming: None, .. } => Some(sel),
            Overlay::History { sel } => Some(sel),
            Overlay::Scope { sel, .. } => Some(sel),
            _ => None,
        }
    }

    /// The text field an overlay types into, for a click that places its caret.
    pub fn input_mut(&mut self) -> Option<&mut LineInput> {
        match self {
            Overlay::Palette { input, .. } | Overlay::Finder { input, .. } | Overlay::Move { input, .. } | Overlay::Capture { input, .. } => Some(input),
            Overlay::Vaults { naming: Some(input), .. } => Some(input),
            _ => None,
        }
    }
}

/// A view's recipe, for the panel (view-explain.md §3).
pub struct Recipe {
    pub view: thc_core::views::Sectioned,
    pub counts: Vec<usize>,
    pub readings: Vec<String>,
    pub scope_text: String,
    pub names: Vec<String>,
    pub overridden: bool,
}

/// A row of the vault picker.
#[derive(Clone, Debug, PartialEq)]
pub struct VaultRow {
    pub name: String,
    pub path: std::path::PathBuf,
    pub open: Option<usize>,
    pub inbox: Option<usize>,
    pub sync: &'static str,
    pub current: bool,
    pub home: bool,
    /// Its colour (`[theme] accent` in its settings.toml; ember at home), so the picker previews it.
    pub accent: String,
}

/// A row of the ⌃O finder.
#[derive(Clone, Debug, PartialEq)]
pub enum Go {
    Page(String),
    Day(NaiveDate),
    New(String),
}

/// An in-place update (tui-editor.md §11).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum UpdateState {
    Idle,
    /// `thc update` running in the background.
    Downloading { version: String },
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToastKind {
    Confirm,
    Agent,
    Error,
    Info,
    /// An alert fired while the TUI is open (daemon.md §2.7), shown for 8 s.
    Alert,
    /// A start-up problem (refused remaps, keymap.md §8.3): 5 s, and keys don't clear it.
    Notice,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Toast {
    pub kind: ToastKind,
    /// (text, token) pieces
    pub parts: Vec<(String, Token)>,
    /// When it appeared (logical ms, UiState::now_ms).
    pub at: u64,
}

impl Toast {
    pub fn alive_at(&self, now_ms: u64) -> bool {
        let age = std::time::Duration::from_millis(now_ms.saturating_sub(self.at));
        match self.kind {
            ToastKind::Error => true,
            ToastKind::Alert => age.as_secs() < 8,
            ToastKind::Notice => age.as_secs() < 5,
            _ => age.as_secs() < 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Focus {
    /// The main view (a list or a document).
    List,
    Detail,
    /// A sidebar panel: `sidebar.focused` names it (sidebar.md §5.1).
    Sidebar,
}

/// Accumulated by one reload, then published alongside its rows. Render reads ordinary
/// immutable values; list queries never mutate App through an immutable reference.
#[derive(Default)]
struct RowProjection {
    hidden: usize,
    totals: HashMap<String, usize>,
    tags: HashMap<usize, usize>,
}

pub struct App {
    /// Every presentation choice, serializable (ui_state.rs). `App` derefs to it, so
    /// `app.view` reads `app.ui.view`.
    pub ui: crate::ui_state::UiState,
    pub vault: Vault,
    pub theme: crate::theme::Theme,
    /// Whether this is the home vault (the crumb names the others).
    pub vault_home: bool,
    /// The vault picker asked to switch to this vault (the loop does it between frames).
    pub switch_to: Option<std::path::PathBuf>,
    /// With `switch_to`: the node to open there, and the vault Esc comes back to (vaults.md §3.5).
    pub switch_focus: Option<String>,
    pub switch_return: Option<std::path::PathBuf>,
    /// The other registered vaults, read by the cross-vault views (Today, Agenda).
    pub others: Vec<OtherVault>,
    /// Rows on screen from another vault: row index → index in `others`. By position, not id:
    /// two vaults may hold different notes with the same (keyed) id, and both show.
    pub row_vault: HashMap<usize, usize>,
    /// The last write went to `others[i]` (so `u` undoes there and the toast names it).
    pub last_write_vault: Option<usize>,
    /// The next write goes to `others[i]` (a routed undo).
    pub route_next: Option<usize>,
    /// Where the caret was in each document (this device): crate::doc_app::Carets.
    pub carets: crate::doc_app::Carets,
    /// The other vaults' (open, inbox, overdue), summed.
    pub others_counts: (usize, usize, usize),
    pub rows: Vec<Row>,
    pub tasks_ms: f64,
    /// The Tasks filter's `group:` (the table blanks a `where` that repeats its heading).
    pub tasks_group: Option<String>,
    /// Notes written as paragraphs (`style = "para"`, §10.8), refreshed on reload.
    pub para_ids: std::collections::HashSet<String>,
    /// (message, bad token, suggested replacement token)
    pub tasks_error: Option<(String, String, Option<String>)>,
    tasks_last_good: Vec<Row>,
    pub search_ms: f64,
    /// Saved views (views.md §1.3): the Tasks saved row and palette `view:` entries.
    pub saved_views: Vec<thc_core::views::View>,
    /// The context (views.md §2), resolved at start; `C` turns it off and on for this session.
    pub context: Option<thc_core::context::Context>,
    context_active: Option<thc_core::context::Active>,
    /// Rows the context hid in the current view, and each section's total before filtering.
    pub context_hidden: usize,
    pub section_totals: HashMap<String, usize>,
    pub to_review: usize,
    pub quit: bool,
    pub editor_request: Option<String>,
    /// `:mouse on|off` asked for capture to change; the event loop applies it (mouse.md §2).
    pub mouse_request: Option<bool>,
    /// The open document (a journal day, an open page): tui-editor.md.
    pub doc: Option<crate::editor::Doc>,
    /// The journal opened with no day ever written: the footer says `just type`.
    pub doc_first_ever: bool,
    /// A remembered scroll (row, scrolled freely) for the document just opened, put on at its
    /// first layout: rows count as the view's width wraps them, which a view not yet laid out
    /// doesn't have (vw384, zszv1).
    pub doc_pending_scroll: Option<(usize, bool)>,
    /// A ⌘ key has arrived from this terminal (now or in an earlier session: <cache>/cmd-seen,
    /// per TERM_PROGRAM and version), so ⌘ keys are known to reach thc (keymap.md §7.0).
    pub cmd_seen: bool,
    /// The first drag's hint (mouse.md §7), until shown once on this device: the key that
    /// gives the terminal's own selection. Read at start (<cache>/mouse-hint-shown,
    /// TERM_PROGRAM) and carried in a trace's `env`, so a replay draws it as the live TUI did.
    pub drag_hint: Option<String>,
    /// Terminal images drawn last (and overlay coverage), reconciled with RenderOutput.
    pub images_drawn: (Vec<crate::images::Place>, bool),
    /// A history step into another vault: restored once the TUI has reopened there.
    pub hist_pending: Option<crate::history::Place>,
    /// The terminal speaks the kitty keyboard protocol (⌥ arrives as Alt there).
    pub kitty: bool,
    /// Under the document: `also today` (today's journal) or `linked from` (a page).
    pub doc_footer: Option<(String, Vec<crate::doc_app::FooterRow>)>,
    /// The writer thread (saves through the daemon while it's live, so typing never waits).
    pub doc_saver: Option<crate::doc_app::Saver>,
    /// `[tui]` settings from ~/.config/thought/config.toml.
    pub tui_prefs: TuiPrefs,
    /// A save waiting for the frame to be drawn first (leaving a line offline).
    /// A newer thc the last release check found (the bar's `update 0.7.1 · :update`).
    pub update_available: Option<String>,
    /// A newer thc installed on disk under this running one (`thc update` in another window):
    /// its version. `:update` then reloads in place.
    pub installed: Option<String>,
    /// The binary's modification time when last checked, and when that was.
    exe_seen: Option<std::time::SystemTime>,
    exe_checked: std::time::Instant,
    /// The device's vault registry as last seen (names, paths, home), and when it was checked:
    /// a vault made, renamed or removed elsewhere shows up live.
    registry_sig: String,
    registry_checked: std::time::Instant,
    /// The background `thc update`'s result: Ok(new version) or Err(reason).
    pub update_rx: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    /// Set when the new binary is in place: the loop saves where we are and re-execs it.
    pub reexec: bool,
    pub inbox_count: usize,
    pub open_count: usize,
    pub overdue_count: usize,
    pub conflicts: Vec<(i64, String, String, Option<String>)>,
    pub(crate) log_sizes: Vec<(String, u64)>,
    /// Transactions the daemon pushed that came from elsewhere, not shown yet (live mode: the
    /// daemon catches up, so this vault's news never sees them).
    pub(crate) live_txs: Vec<String>,
    /// The rail beside the open document (navigation.md §3), rebuilt with it.
    pub rail: Vec<RailItem>,
    /// The daemon said something changed that has no transaction (a conflict): refresh.
    pub(crate) live_dirty: bool,
    /// The open page laid out ahead, in idle time, at the width a sidebar column would leave
    /// it: (the document, that geometry, the next note to lay out; None when all are done).
    pub(crate) prewarm: Option<(String, crate::editor::ViewGeometry, Option<usize>)>,
    /// Where the last poll's time went (THC_TUI_TRACE's slow-key log).
    pub(crate) poll_split: Vec<(&'static str, f64)>,
    /// The daemon pushed a change: poll now, not at the next interval.
    pub poll_wanted: bool,
    last_agent_tx: Option<String>,
    pub screen_width: u16,
    /// Last accepted frame: input consumes only this frame's geometry and cells.
    pub render: crate::ui::RenderOutput,
    /// Session-owned immutable inputs for drawing; only preparation replaces its caches.
    pub derived: crate::derived::Derived,
    pub live_rx: Option<std::sync::mpsc::Receiver<crate::live::LiveMsg>>,
    pub daemon_live: bool,
    /// The sidebar's doc panels at runtime: each one's view and, when the main view isn't on
    /// the same page, its document (sidebar_app.rs).
    pub panels: HashMap<crate::sidebar::PanelKey, crate::sidebar_app::PanelRt>,
    /// The next view id a panel gets (the main view is 0).
    pub next_vid: u32,
    /// A panel's key is running as the open document (sidebar_app.rs `with_panel`).
    pub in_panel: Option<crate::sidebar::PanelKey>,
    /// What a key in a panel asked of the main view, run after it.
    pub panel_defer: Vec<crate::sidebar_app::Deferred>,
    /// The panels were checked once (deleted pages dropped at launch).
    pub sidebar_checked: bool,
    /// The panel a press started in: its drag and release go there too.
    pub panel_pointer: Option<crate::sidebar::PanelKey>,
    /// The sidebar's list panels at runtime: rows and selection (sidebar_list.rs).
    pub lists: HashMap<crate::sidebar::PanelKey, crate::sidebar_list::ListRt>,
    /// An agent's opens in a row, for the toast's count: (actor, how many, when).
    pub agent_opens: Option<(String, usize, u64)>,
    /// A list panel's key is running as the main list (sidebar_list.rs `with_list_panel`).
    pub in_list: Option<crate::sidebar::PanelKey>,
    /// The sidebar's column width this frame (None: no column), set before drawing.
    pub sidebar_col: Option<u16>,
    /// The sidebar's place this frame when it isn't a column: the drawer's or replace's rect.
    pub sidebar_over: Option<(crate::sidebar::Layout, ratatui::layout::Rect)>,
    /// Where the main view's caret was drawn last layout: it stays on that row when the
    /// geometry changes under it (doc_ui::prepare).
    pub caret_pin: Option<crate::doc_ui::CaretPin>,
    /// The pointer rests on a link's title (main view or a panel): the terminal is asked to
    /// report ⇧ with clicks there (cmd_click::ShiftCapture).
    pub pointer_on_link: bool,
    /// The terminal's width at the last layout (screen_width is the main area's).
    pub term_width: u16,
    /// A drag in the sidebar: the divider (resizing) or a header (reordering, and where it
    /// would drop).
    pub sidebar_drag: Option<crate::sidebar_app::Drag>,
}

impl std::ops::Deref for App {
    type Target = crate::ui_state::UiState;
    fn deref(&self) -> &crate::ui_state::UiState {
        &self.ui
    }
}

impl std::ops::DerefMut for App {
    fn deref_mut(&mut self) -> &mut crate::ui_state::UiState {
        &mut self.ui
    }
}

pub struct PaletteItem {
    pub label: &'static str,
    /// The keymap action it runs (keymap.rs), or a palette-only one (`page.new`, `update`).
    pub action: &'static str,
    pub cmd: &'static str,
}

pub const PALETTE: &[PaletteItem] = &[
    // History (navigation.md §7.5): the places you've been.
    PaletteItem { label: "history (places you've been)", action: "nav.history", cmd: ":history" },
    PaletteItem { label: "back", action: "nav.back", cmd: ":back" },
    PaletteItem { label: "explain this view (how it's built)", action: "view.explain", cmd: ":explain" },
    PaletteItem { label: "forward", action: "nav.forward", cmd: ":forward" },
    PaletteItem { label: "Capture", action: "capture.here", cmd: "thc add" },
    PaletteItem { label: "Capture to inbox", action: "capture.inbox", cmd: "thc add --inbox" },
    PaletteItem { label: "Done", action: "node.done", cmd: "thc done <id>" },
    PaletteItem { label: "Reopen", action: "node.reopen", cmd: "thc reopen <id>" },
    PaletteItem { label: "Task on / off", action: "node.task_toggle", cmd: "thc set <id> status=todo" },
    PaletteItem { label: "Set due date", action: "node.due", cmd: "thc set <id> due=…" },
    PaletteItem { label: "Set scheduled date", action: "node.scheduled", cmd: "thc set <id> scheduled=…" },
    PaletteItem { label: "Set priority", action: "prefix:p", cmd: "thc set <id> priority=…" },
    PaletteItem { label: "Edit text", action: "node.text", cmd: "thc text <id> …" },
    PaletteItem { label: "Edit in $EDITOR", action: "node.edit_external", cmd: "thc edit <id>" },
    PaletteItem { label: "Edit page or day in $EDITOR", action: "doc.edit_external", cmd: "thc edit <page>" },
    PaletteItem { label: "Tags", action: "node.tags", cmd: "thc tag <id> …" },
    PaletteItem { label: "Move", action: "node.move", cmd: "thc mv <id> --under …" },
    PaletteItem { label: "Skip occurrence", action: "node.skip", cmd: "thc skip <id>" },
    PaletteItem { label: "Snooze alert", action: "node.snooze", cmd: "thc alert snooze …" },
    PaletteItem { label: "Acknowledge alert", action: "node.ack", cmd: "thc alert ack …" },
    PaletteItem { label: "Delete", action: "node.delete", cmd: "thc rm <id>" },
    PaletteItem { label: "Undo", action: "undo", cmd: "thc undo" },
    PaletteItem { label: "History of this node", action: "node.history", cmd: "thc history <id>" },
    PaletteItem { label: "Copy short id", action: "node.copy_id", cmd: "" },
    PaletteItem { label: "Copy full id", action: "node.copy_full_id", cmd: "" },
    PaletteItem { label: "New page", action: "page.new", cmd: "thc page new …" },
    PaletteItem { label: "Search", action: "search.find", cmd: "thc search …" },
    PaletteItem { label: "Filter tasks", action: "tasks.filter", cmd: "thc q …" },
    PaletteItem { label: "Today ⇄ agenda", action: "today.agenda_toggle", cmd: "thc agenda" },
    PaletteItem { label: "Toggle detail pane", action: "pane.detail_toggle", cmd: "" },
    PaletteItem { label: "Open beside (in the sidebar)", action: "sidebar.open_aside", cmd: ":aside <page | day | @view | #tag>" },
    PaletteItem { label: "Sidebar: focus · back to main", action: "sidebar.focus", cmd: "" },
    PaletteItem { label: "Sidebar: close all", action: "sidebar.close_all", cmd: ":sidebar close all" },
    PaletteItem { label: "Sidebar: reopen closed panel", action: "sidebar.reopen", cmd: "" },
    PaletteItem { label: "Sidebar: hide · show", action: "sidebar.toggle", cmd: ":sidebar hide" },
    PaletteItem { label: "Go to Today", action: "go.today", cmd: "thc today" },
    PaletteItem { label: "Go to Inbox", action: "go.inbox", cmd: "thc inbox" },
    PaletteItem { label: "Go to Tasks", action: "go.tasks", cmd: "thc q" },
    PaletteItem { label: "Go to Pages", action: "go.pages", cmd: "thc pages" },
    PaletteItem { label: "Go to Journal", action: "go.journal", cmd: "thc journal" },
    PaletteItem { label: "Go to Search", action: "go.search", cmd: "thc search" },
    PaletteItem { label: "Go to Log", action: "go.log", cmd: "thc log" },
    PaletteItem { label: "Start the daemon", action: "daemon.start", cmd: "thc daemon start" },
    PaletteItem { label: "Update thc", action: "update", cmd: "thc update" },
    PaletteItem { label: "What's new", action: "changes", cmd: ":changes" },
    PaletteItem { label: "About thc (version, vault, changelog)", action: "about", cmd: ":about" },
    PaletteItem { label: "Focus: what it shows", action: "focus.overlay", cmd: "" },
    PaletteItem { label: "Mouse capture on / off", action: "mouse.toggle", cmd: "" },
    PaletteItem { label: "Help", action: "help.context", cmd: "" },
    PaletteItem { label: "Remap keys in $EDITOR", action: "keys.remap", cmd: ":remap" },
    PaletteItem { label: "Quit", action: "quit", cmd: "" },
];

/// Views, or the built-in definitions before the first seeding (the TUI never writes on read).
fn load_views(s: &thc_core::store::Store) -> Vec<thc_core::views::View> {
    if thc_core::views::page_exists(s).unwrap_or(false) {
        return thc_core::views::list(s).unwrap_or_default();
    }
    thc_core::views::BUILTIN
        .iter()
        .map(|(name, title, q, slot)| thc_core::views::View { id: String::new(), name: name.to_string(), query: q.to_string(), title: Some(title.to_string()), tasks: Some(*slot), bar: false, capture: None })
        .collect()
}

/// A palette entry: the static commands plus one `view: <name>` per saved view.
#[derive(Clone, Debug)]
pub struct PaletteEntry {
    pub label: String,
    /// What running it does: an action ID, `@view` or `ctx:name`.
    pub keys: String,
    pub cmd: String,
    /// The key it's on now, from the keymap (so a remap shows here), or empty.
    pub shown: String,
}

/// One vault's Today, by section (App::today_secs).
struct TodaySecs {
    overdue: Vec<Node>,
    today: Vec<Node>,
    alerted: Vec<Node>,
    doing: Vec<Node>,
    next: Vec<Node>,
    done: Vec<Node>,
}

/// Another registered vault, as the cross-vault views read it.
pub struct OtherVault {
    /// Its `name_short` when set, else its name.
    pub name: String,
    pub path: std::path::PathBuf,
    pub accent: crate::theme::Accent,
    pub vault: Vault,
}

/// The other registered vaults, open (a snapshot reads scratch copies, so its keys never
/// write to a real vault).
/// This device's view scopes (view-explain.md §2), beside the vault caches.
fn scopes_file(cache: &std::path::Path) -> std::path::PathBuf {
    cache.parent().unwrap_or(cache).join("scopes.json")
}

fn load_scopes(cache: &std::path::Path) -> std::collections::BTreeMap<String, String> {
    std::fs::read(scopes_file(cache)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_scopes(cache: &std::path::Path, s: &std::collections::BTreeMap<String, String>) {
    if let Ok(b) = serde_json::to_vec(s) {
        let _ = std::fs::write(scopes_file(cache), b);
    }
}

/// What about the registry a running TUI shows: each vault's name and path, and home.
fn registry_sig(reg: &thc_core::registry::Registry) -> String {
    let mut s = format!("home={:?}\n", reg.home);
    for e in &reg.vaults {
        s.push_str(&format!("{}={}\n", e.name, e.path.display()));
    }
    s
}

fn open_others(vault: &Vault) -> Vec<OtherVault> {
    open_others_from(vault, &thc_core::registry::Registry::load(), &[])
}

/// The other vaults in `reg`, opened, except those at `skip` (already open).
fn open_others_from(vault: &Vault, reg: &thc_core::registry::Registry, skip: &[std::path::PathBuf]) -> Vec<OtherVault> {
    if reg.vaults.len() < 2 || vault.actor.kind == "agent" {
        return vec![];
    }
    let here = vault.origin.as_ref().map_or(vault.paths.vault.clone(), |o| o.vault.clone());
    let me = reg.by_path(&here).map(|e| e.name.clone());
    let snapshot = crate::SNAPSHOT.with(|s| s.get()) && vault.origin.is_some();
    let entries: Vec<&thc_core::registry::Entry> = reg.vaults.iter().filter(|e| Some(&e.name) != me.as_ref() && !skip.contains(&e.path) && e.path.join(thc_core::vault::VAULT_MARKER).exists()).collect();
    let actor = vault.actor.clone();
    // In parallel: each opens (and catches up) its own store.
    std::thread::scope(|sc| {
        let hs: Vec<_> = entries
            .iter()
            .map(|e| {
                let actor = actor.clone();
                sc.spawn(move || -> Option<OtherVault> {
                    let real = thc_core::vault::Paths { vault: e.path.clone(), cache: thc_core::vault::default_cache(&e.path) };
                    let (paths, origin) = if snapshot { (thc_core::vault::scratch_copy(&real).ok()?, Some(real)) } else { (real, None) };
                    let mut v = Vault::open(paths, actor, "tui").ok()?;
                    v.origin = origin;
                    let eff = thc_core::settings::load(Some(&e.path));
                    let name = eff.str("vault.name_short").map(str::to_string).unwrap_or_else(|| e.name.clone());
                    Some(OtherVault { name, path: e.path.clone(), accent: crate::theme::Accent::parse(eff.accent()), vault: v })
                })
            })
            .collect();
        hs.into_iter().filter_map(|h| h.join().ok().flatten()).collect()
    })
}

/// Open and inbox counts for another vault (its own store, read-only use).
fn vault_counts(path: &std::path::Path) -> Option<(usize, usize)> {
    if !path.join(thc_core::vault::VAULT_MARKER).exists() {
        return None;
    }
    let v = Vault::open(thc_core::vault::Paths { vault: path.to_path_buf(), cache: thc_core::vault::default_cache(path) }, thc_core::event::Actor { kind: "human".into(), name: None }, "tui").ok()?;
    let today = dates::today();
    let open = v.store.query("status:open", today, 1_000_000).ok()?.len();
    let inbox = v.store.query("is:inbox", today, 1_000_000).ok()?.len();
    Some((open, inbox))
}

fn ymd(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

impl App {
    pub fn new(vault: Vault) -> Result<App> {
        Self::open(vault, false)
    }

    /// Like `new`, but leaves the first reload to the caller: `thc j` / `thc p` go straight to a
    /// document, and building Today's rows first cost ~45 ms on a cold store.
    pub fn new_deferred(vault: Vault) -> Result<App> {
        Self::open(vault, true)
    }

    fn open(vault: Vault, deferred: bool) -> Result<App> {
        let history = crate::history::History::load(&vault.paths.cache);
        let initial_context = std::env::current_dir().ok().and_then(|d| thc_core::context::resolve(None, &vault.paths.cache, &d));
        let today = dates::today();
        let log_sizes = vault.log.files()?;
        let prefs = TuiPrefs::load();
        // A snapshot's scratch copy is named for the vault it was copied from.
        let others = open_others(&vault);
        let vault_cache = vault.paths.cache.clone();
        let (vault_name, vault_home) = {
            let reg = thc_core::registry::Registry::load();
            let real = vault.origin.as_ref().map_or(&vault.paths.vault, |o| &o.vault);
            (reg.name_for(real), reg.vaults.is_empty() || reg.is_home(real))
        };
        let vault_name_for_sidebar = vault_name.clone();
        let ui = crate::ui_state::UiState {
            vault_name: vault_name,
            return_vault: None,
            today_by_vault: false,
            view: View::Today,
            cursor: 0,
            scroll: 0,
            selected: None,
            today: today,
            tasks_filter: "status:open sort:due".into(),
            edit: None,
            page_ids: false,
            input_untouched: false,
            log_actor: None,
            recent_cmds: vec![],
            context_on: true,
            search_terms: String::new(),
            pages_filter: String::new(),
            page_open: None,
            journal_date: today,
            agenda_mode: false,
            show_all_done: false,
            log_node: None,
            review_lane: false,
            collapsed: Default::default(),
            prompt: None,
            awaiting: None,
            overlay: None,
            toast: None,
            paste_plain: false,
            focus_mode: false,
            focus_cfg: prefs.focus,
            focus_hint_shown: false,
            write_alt_hint: false,
            doc_back: None,
            hover: None,
            scroll_drag: false,
            doc_scroll_free: false,
            history: history,
            scope_override: load_scopes(&vault_cache),
            last_click: None,
            about_new: false,
            meta_hint: false,
            update_state: UpdateState::Idle,
            flashes: Default::default(),
            show_detail: true,
            focus: Focus::List,
            pending_keys: Vec::new(),
            pending_since: None,
            recent_docs: Vec::new(),
            recent_moves: vec![],
            last_page: None,
            rail_frozen: None,
            doc_origin: None,
            offline_toast_shown: false,
            alert_toast_node: None,
            sidebar: crate::sidebar_app::load_sidebar(&vault_cache, &vault_name_for_sidebar),
            ..Default::default()
        };
        let mut ui = ui;
        {
            let (now_ms, offset) = crate::runtime_effects::wall_clock();
            ui.tick(now_ms, offset);
        }
        let mut app = App {
            ui,
            vault,
            theme: crate::theme::Theme::detect(),
            vault_home,
            switch_to: None,
            switch_focus: None,
            switch_return: None,
            others,
            row_vault: HashMap::new(),
            last_write_vault: None,
            route_next: None,
            carets: crate::doc_app::load_carets(&vault_cache),
            others_counts: (0, 0, 0),
            rows: vec![],
            tasks_ms: 0.0,
            tasks_group: None,
            para_ids: Default::default(),
            tasks_error: None,
            tasks_last_good: vec![],
            search_ms: 0.0,
            saved_views: vec![],
            context: initial_context,
            context_active: None,
            context_hidden: 0,
            section_totals: HashMap::new(),
            to_review: 0,
            doc: None,
            doc_footer: None,
            doc_saver: None,
            tui_prefs: prefs.clone(),
            doc_first_ever: false,
            doc_pending_scroll: None,
            cmd_seen: crate::keymap::cmd_seen_cached(&vault_cache),
            drag_hint: crate::doc_keys::drag_hint_pending(&vault_cache),
            images_drawn: (Vec::new(), false),
            hist_pending: None,
            kitty: false,
            update_available: thc_core::release::available(env!("CARGO_PKG_VERSION")),
            installed: None,
            exe_seen: std::env::current_exe().ok().and_then(|p| std::fs::metadata(p).ok()).and_then(|m| m.modified().ok()),
            exe_checked: std::time::Instant::now(),
            registry_sig: registry_sig(&thc_core::registry::Registry::load()),
            registry_checked: std::time::Instant::now(),
            update_rx: None,
            reexec: false,
            quit: false,
            editor_request: None,
            mouse_request: None,
            inbox_count: 0,
            open_count: 0,
            overdue_count: 0,
            conflicts: vec![],
            log_sizes,
            live_txs: Vec::new(),
            rail: Vec::new(),
            live_dirty: false,
            poll_split: Vec::new(),
            prewarm: None,
            poll_wanted: false,
            last_agent_tx: None,
            screen_width: 80,
            render: crate::ui::RenderOutput::default(),
            derived: crate::derived::Derived::new(),
            live_rx: None,
            daemon_live: false,
            panels: HashMap::new(),
            next_vid: 1,
            in_panel: None,
            panel_defer: Vec::new(),
            sidebar_checked: false,
            panel_pointer: None,
            lists: HashMap::new(),
            agent_opens: None,
            in_list: None,
            sidebar_col: None,
            sidebar_over: None,
            caret_pin: None,
            pointer_on_link: false,
            term_width: 0,
            sidebar_drag: None,
        };
        app.load_page_ids();
        if !deferred {
            app.reload()?;
        }
        // What opening caught up is on screen already: not news.
        app.vault.take_news();
        Ok(app)
    }

    // ---- toasts --------------------------------------------------------------------------------

    pub fn toast_parts(&mut self, kind: ToastKind, parts: Vec<(String, Token)>) {
        self.ui.toast_parts(kind, parts);
    }

    /// The open document is waiting on time: unsaved typing (an idle save comes 1.5 s after
    /// the last key), a save in flight (◌ after 3 s), or a settling flash.
    pub fn doc_wants_clock(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.blocks().iter().any(|l| l.edited() || l.saving_since.is_some() || l.flash_until.is_some())) || self.panels_want_clock()
    }

    /// `:focus [writer | +month -footer | save | off]` (tui-editor.md §8.4). Bare, it opens the
    /// overlay; anything else changes this session's Focus and turns it on.
    pub fn focus_command(&mut self, args: &str) {
        match args.trim() {
            "" => {
                if self.doc.is_none() {
                    return self.info("focus is for a journal day or a page · 5 opens the journal");
                }
                self.set_focus_mode(true);
                self.overlay = Some(Overlay::Focus);
            }
            "save" => self.save_focus(),
            "off" => self.set_focus_mode(false),
            spec => match self.focus_cfg.apply(spec) {
                Ok(()) => {
                    if self.doc.is_some() {
                        self.set_focus_mode(true);
                    }
                    self.info(format!("focus · {}", self.focus_cfg.label()));
                }
                Err(e) => self.error(e),
            },
        }
    }

    /// Save this session's Focus to `[tui.focus]` (only what differs from the preset).
    pub fn save_focus(&mut self) {
        let Some(p) = thc_core::vault::global_config_path() else { return self.error("no config file: HOME isn't set") };
        // Snapshot keys never change real files unless asked (THC_TUI_SNAPSHOT_WRITE=1).
        if crate::SNAPSHOT.with(|x| x.get()) && std::env::var("THC_TUI_SNAPSHOT_WRITE").map_or(true, |v| v != "1") {
            return self.info(format!("focus saved · {} (snapshot: not written)", thc_core::vault::tilde(&p)));
        }
        match thc_core::tui_config::save_focus(&p, &self.focus_cfg) {
            Ok(()) => self.info(format!("focus saved · {}", thc_core::vault::tilde(&p))),
            Err(e) => self.error(format!("focus not saved: {e}")),
        }
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.toast_parts(ToastKind::Info, vec![(text.into(), Token::Muted)]);
    }

    pub fn notice(&mut self, text: impl Into<String>) {
        self.toast_parts(ToastKind::Notice, vec![(text.into(), Token::Overdue)]);
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.toast_parts(ToastKind::Error, vec![(text.into(), Token::Overdue)]);
    }

    /// `done    65p83  Water plants · ↻ next Tue Oct 6`
    pub fn confirm(&mut self, verb: &str, id: Option<&str>, rest: String) {
        let padded = if verb.chars().count() >= 8 { format!("{verb} ") } else { format!("{verb:<8}") };
        let mut parts = vec![(padded, Token::Text)];
        if let Some(id) = id {
            parts.push((format!("{}  ", self.vault.store.short(id)), Token::Muted));
        }
        // `done vy0jc Book the venue · acme · u undo`: a write routed to another vault says where
        // (before the undo hint, when there is one).
        let rest = match self.last_write_vault.and_then(|i| self.others.get(i)) {
            Some(o) => {
                let sep = self.theme.glyphs().sep;
                let hint = format!(" {sep} u undo");
                match rest.rfind(&hint) {
                    Some(at) => format!("{} {sep} {}{}", &rest[..at], o.name, &rest[at..]),
                    None => format!("{rest} {sep} {}", o.name),
                }
            }
            None => rest,
        };
        parts.push((rest, Token::Muted));
        self.toast_parts(ToastKind::Confirm, parts);
    }

    pub fn report(&mut self, r: Result<Vec<Event>>, verb: &str, id: Option<&str>, rest: String) {
        match r {
            Ok(_) => self.confirm(verb, id, rest),
            Err(e) => self.error(friendly_error(&e)),
        }
    }

    // ---- selection ----------------------------------------------------------------------------

    /// The vault (index in `others`) a row on screen came from; None: this one.
    pub fn row_from(&self, row: usize) -> Option<usize> {
        self.row_vault.get(&row).copied()
    }

    /// Today's stored scope: your @today's, else the shipped `vault:*`.
    pub fn scope_default(&self) -> String {
        self.edited_today().and_then(|v| v.scope).unwrap_or_else(|| "vault:*".into())
    }

    /// Today's scope on this device: the override, else the stored one.
    pub fn scope_now(&self) -> String {
        self.scope_override.get("today").cloned().unwrap_or_else(|| self.scope_default())
    }

    /// The vault names a scope covers, in registry order (home first).
    pub fn scope_names(&self, scope: &str) -> Vec<String> {
        let reg = thc_core::registry::Registry::load();
        let mut names: Vec<String> = thc_core::federated::split_scope(scope)
            .ok()
            .and_then(|(s, _)| s)
            .and_then(|s| thc_core::federated::resolve(&s, &reg, &self.vault_name).ok())
            .map(|(e, _)| e.into_iter().map(|e| e.name).collect())
            .unwrap_or_else(|| vec![self.vault_name.clone()]);
        names.sort_by_key(|n| (reg.home.as_deref() != Some(n.as_str()), reg.vaults.iter().position(|e| &e.name == n)));
        // A vault the registry doesn't know (opened by path) is still here.
        if names.is_empty() || (scope.contains("vault:*") && !names.contains(&self.vault_name)) {
            names.insert(0, self.vault_name.clone());
        }
        names
    }

    pub fn scope_vaults(&self) -> Vec<String> {
        self.scope_names(&self.scope_now())
    }

    /// Every vault the picker offers: this one and the others open here, home first.
    pub fn scope_choices(&self) -> Vec<String> {
        self.scope_names("vault:*")
    }

    /// A scope from the names picked: all of them is `vault:*`.
    pub fn scope_of(&self, picked: &[String]) -> String {
        let all = self.scope_choices();
        if all.iter().all(|n| picked.contains(n)) {
            return "vault:*".into();
        }
        match picked {
            [one] => format!("vault:{one}"),
            many => format!("vault:({})", many.join(" or ")),
        }
    }

    /// Apply a scope on this device (the default clears the override), and show it.
    pub fn set_scope(&mut self, scope: String) {
        if scope == self.scope_default() {
            self.scope_override.remove("today");
        } else {
            self.scope_override.insert("today".into(), scope);
        }
        save_scopes(&self.vault.paths.cache, &self.scope_override);
        let _ = self.reload();
    }

    /// `s` in the picker: the scope becomes @today's own (`thc view set today --scope`), synced.
    pub fn save_scope_default(&mut self, scope: String) {
        let v = self.edited_today().or_else(|| thc_core::views::sectioned(&self.vault.store, "today").ok().flatten());
        let Some(v) = v else { return };
        let today = self.today;
        let r = self.write_in(None, |b| thc_core::views::set_sectioned(b, "today", Some(Some(&scope)), &v.sections, today).map(|_| ()));
        match r {
            Ok(_) => {
                self.scope_override.remove("today");
                save_scopes(&self.vault.paths.cache, &self.scope_override);
                let _ = self.reload();
                self.info(format!("Today's scope saved: {}", self.scope_text(&scope)));
            }
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    /// The view on screen that has a recipe (view-explain.md §3): today, agenda, inbox, tasks,
    /// or the saved view Tasks is showing.

    /// A view's recipe: its definition, each section's count now and plain reading, and its
    /// scope's vaults.
    pub fn recipe(&self, name: &str) -> Option<Recipe> {
        let v = if name == "today" { self.edited_today().or_else(|| thc_core::views::sectioned(&self.vault.store, "today").ok().flatten()) } else { thc_core::views::sectioned(&self.vault.store, name).ok().flatten() }?;
        let scope = if name == "today" { self.scope_now() } else { v.scope.clone().unwrap_or_default() };
        let names = if scope.is_empty() { vec![self.vault_name.clone()] } else { self.scope_names(&scope) };
        let reg = thc_core::registry::Registry::load();
        let mut stores: Vec<&thc_core::store::Store> = Vec::new();
        if names.contains(&self.vault_name) {
            stores.push(&self.vault.store);
        }
        for o in &self.others {
            if names.iter().any(|n| reg.by_path(&o.path).is_some_and(|e| &e.name == n)) {
                stores.push(&o.vault.store);
            }
        }
        let mut counts: Vec<usize> = thc_core::federated::run_sections(&v.sections, &stores, self.today, 10_000).map(|r| r.iter().map(|x| x.len()).collect()).unwrap_or_else(|_| vec![0; v.sections.len()]);
        // The view on screen: its sections' counts as the list shows them (every vault in scope,
        // what Today adds for alerts), so the recipe and the list never disagree.
        if self.recipe_name().as_deref() == Some(name) {
            for (i, s) in v.sections.iter().enumerate() {
                let shown = self.rows.iter().find_map(|r| match r {
                    Row::Section { title, count, .. } if title == &s.title => Some(count.unwrap_or(0)),
                    _ => None,
                });
                counts[i] = shown.unwrap_or(0);
            }
        }
        let readings: Vec<String> = v.sections.iter().map(|s| thc_core::query::explain(&s.query, &self.vault.store, self.today).map(|e| e.lines().join(", ")).unwrap_or_else(|e| format!("doesn't parse: {}", e.error))).collect();
        Some(Recipe { scope_text: if scope.is_empty() { format!("this vault ({})", self.vault_name) } else { self.scope_text(&scope) }, names, overridden: name == "today" && self.scope_override.contains_key("today"), view: v, counts, readings })
    }

    /// `r` in a recipe: back to the shipped view (undoable, one transaction).
    pub fn reset_view(&mut self, name: &str) {
        let n = name.to_string();
        match self.write_in(None, |b| thc_core::views::reset(b, &n)) {
            Ok(_) => {
                let _ = self.reload();
                self.info(format!("@{name} is the shipped one again · u undoes"));
            }
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    /// `all vaults (3)`, `work`, `personal, acme`.
    pub fn scope_text(&self, scope: &str) -> String {
        let names = self.scope_names(scope);
        if scope.contains("vault:*") {
            format!("all vaults ({})", names.len())
        } else {
            names.join(", ")
        }
    }

    /// The store a row's node lives in: another vault's for a row from it, else this one.
    pub fn store_of_row(&self, row: usize) -> &thc_core::store::Store {
        match self.row_from(row).and_then(|i| self.others.get(i)) {
            Some(o) => &o.vault.store,
            None => &self.vault.store,
        }
    }

    /// The selected row's store.
    pub fn store_of_selected(&self) -> &thc_core::store::Store {
        self.store_of_row(self.cursor)
    }

    pub fn selected_node(&self) -> Option<&Node> {
        self.rows.get(self.cursor).and_then(|r| r.node())
    }

    pub fn selected_id(&self) -> Option<String> {
        self.selected_node().map(|n| n.id.clone())
    }

    pub fn node_label(&self, id: &str) -> String {
        self.vault.store.node(id).ok().flatten().map(|n| self.vault.store.render_text(&n.label())).unwrap_or_default()
    }

    // ---- data --------------------------------------------------------------------------------

    /// A reload that keeps the order on screen (navigation.md §8): after your own
    /// row action or a change from elsewhere, rows still there keep their place with their new
    /// content (done dims where it is), rows gone drop out, new rows go in after the row before
    /// them in the fresh order. The selection stays on its row. Arriving fresh in a view, a sort
    /// or filter change and ⌃L use `reload`, which re-sorts.
    pub fn reload_stable(&mut self) -> Result<()> {
        let at = (self.view, self.page_open.clone(), self.agenda_mode, self.doc.is_some());
        let old: Vec<(Row, Option<usize>)> = self.rows.iter().enumerate().map(|(i, r)| (r.clone(), self.row_from(i))).collect();
        self.reload()?;
        if (self.view, self.page_open.clone(), self.agenda_mode, self.doc.is_some()) != at || self.doc.is_some() {
            return Ok(());
        }
        type Id = (String, Option<usize>);
        let id = |r: &Row, v: Option<usize>| -> Option<Id> { if matches!(r, Row::Node { .. }) { r.key().map(|k| (k, v)) } else { None } };
        if !old.iter().any(|(r, v)| id(r, *v).is_some()) {
            return Ok(());
        }
        let new: Vec<(Row, Option<usize>)> = self.rows.iter().enumerate().map(|(i, r)| (r.clone(), self.row_from(i))).collect();
        let fresh: HashMap<Id, Row> = new.iter().filter_map(|(r, v)| id(r, *v).map(|k| (k, r.clone()))).collect();
        let had: std::collections::HashSet<Id> = old.iter().filter_map(|(r, v)| id(r, *v)).collect();
        let mut out: Vec<(Row, Option<usize>)> = Vec::new();
        for (r, v) in old {
            match id(&r, v) {
                Some(k) => {
                    if let Some(nr) = fresh.get(&k) {
                        out.push((nr.clone(), v));
                    } else if let Row::Node { node, depth, outline, has_children, collapsed, child_count, under_day } = r {
                        // No longer matched (done in Tasks, say): it stays where it is, as it
                        // is now, until the next fresh arrival. Deleted: it goes.
                        let store = v.and_then(|i| self.others.get(i)).map_or(&self.vault.store, |o| &o.vault.store);
                        if let Some(n) = store.node(&node.id).ok().flatten().filter(|n| !n.deleted) {
                            out.push((Row::Node { node: n, depth, outline, has_children, collapsed, child_count, under_day }, v));
                        }
                    }
                }
                None => out.push((r, v)),
            }
        }
        let mut prev: Option<Id> = None;
        for (r, v) in &new {
            let Some(k) = id(r, *v) else { continue };
            if !had.contains(&k) {
                let at = prev.as_ref().and_then(|p| out.iter().position(|(o, ov)| id(o, *ov).as_ref() == Some(p))).map_or_else(
                    // Nothing before it: after the first heading, else at the top.
                    || out.iter().position(|(o, _)| matches!(o, Row::Section { .. })).map_or(0, |i| i + 1),
                    |i| i + 1,
                );
                out.insert(at, (r.clone(), *v));
            }
            prev = Some(k);
        }
        let tags = &mut self.row_vault;
        tags.clear();
        for (i, (_, v)) in out.iter().enumerate() {
            if let Some(v) = v {
                tags.insert(i, *v);
            }
        }
        // Tasks: the order on screen goes into the state (`tasks_order`).
        if self.view == View::Tasks {
            let ids: Vec<String> = out.iter().filter_map(|(r, _)| match r {
                Row::Node { node, .. } => Some(node.id.clone()),
                _ => None,
            }).collect();
            self.ui.tasks_order = Some((self.tasks_filter.clone(), ids));
        }
        self.rows = out.into_iter().map(|(r, _)| r).collect();
        self.restore_cursor();
        Ok(())
    }

    pub fn reload(&mut self) -> Result<()> {
        // The Tasks order lasts while you stay on the list (`tasks_order`).
        if self.view != View::Tasks || self.doc.is_some() {
            self.ui.tasks_order = None;
        }
        // The other vaults' changes (their logs, a few stat calls when nothing moved).
        for o in self.others.iter_mut() {
            let _ = o.vault.catch_up();
        }
        let s = &self.vault.store;
        let t = ymd(self.today);
        // Counted in SQL: loading the notes to count them cost 37 ms at 600 open tasks.
        let count = |sql: &str, p: &[&dyn rusqlite::ToSql]| -> usize { s.conn.query_row(&format!("SELECT count(*) FROM nodes n WHERE {sql}"), p, |r| r.get::<_, i64>(0)).map(|n| n as usize).unwrap_or(0) };
        self.inbox_count = count("n.parent IS NULL AND n.title IS NULL AND n.journal IS NULL AND n.is_tag=0 AND n.deleted=0", &[]);
        self.open_count = count("n.status IN ('todo','doing','waiting') AND n.deleted=0", &[]);
        self.overdue_count = count("n.status IN ('todo','doing','waiting') AND n.deleted=0 AND substr(n.due,1,10) < ?1", &[&t]);
        // The other vaults' open, inbox and overdue counts, for a cross-vault Today's bar.
        let (mut oo, mut oi, mut od) = (0, 0, 0);
        for o in &self.others {
            let c = |sql: &str, p: &[&dyn rusqlite::ToSql]| -> usize { o.vault.store.conn.query_row(&format!("SELECT count(*) FROM nodes n WHERE {sql}"), p, |r| r.get::<_, i64>(0)).map(|n| n as usize).unwrap_or(0) };
            oo += c("n.status IN ('todo','doing','waiting') AND n.deleted=0", &[]);
            oi += c("n.parent IS NULL AND n.title IS NULL AND n.journal IS NULL AND n.is_tag=0 AND n.deleted=0", &[]);
            od += c("n.status IN ('todo','doing','waiting') AND n.deleted=0 AND substr(n.due,1,10) < ?1", &[&t]);
        }
        self.others_counts = (oo, oi, od);
        self.conflicts = s.open_conflicts().unwrap_or_default();
        self.saved_views = load_views(s);
        self.para_ids = thc_core::edit::para_ids(s);
        self.context_active = match (&self.context, self.context_on) {
            (Some(c), true) => thc_core::context::Active::load(s, c.clone(), self.today).ok(),
            _ => None,
        };
        self.context_hidden = 0;
        self.section_totals.clear();
        self.to_review = thc_core::review::pending_count(s).unwrap_or(0);
        let now = self.ui.now_ms;
        self.ui.flashes.retain(|_, (at, _)| now.saturating_sub(*at) < 3000);
        // Which vault the selected row came from, before the rows (and their tags) are rebuilt:
        // two vaults' rows may share an id, and the cursor stays on the one it was on.
        let was_from = self.rows.get(self.cursor).filter(|r| r.key() == self.selected).map(|_| self.row_from(self.cursor));
        // The rows' vault tags belong to the rows they were built with: a view that tags none
        // (Pages, Search, …) must never inherit the last view's (Enter on a page
        // in one vault once moved you to another, because Today's row 0 had been from it).
        self.row_vault.clear();
        let mut projection = RowProjection::default();
        self.rows = match self.view {
            // A document draws from its buffer; rows only carry the selected line's node, for
            // Navigate's single-key commands (building outline rows for a 5,000-line page is slow).
            _ if self.doc_target().is_some() => self.selected.as_ref().and_then(|id| s.node(id).ok().flatten()).filter(|n| !n.deleted).map(|n| vec![Self::node_row(n)]).unwrap_or_default(),
            View::Today if self.agenda_mode => self.rows_agenda(&mut projection)?,
            View::Today => self.rows_today(&mut projection)?,
            View::Inbox => self.rows_inbox(&mut projection)?,
            View::Tasks => self.rows_tasks(&mut projection)?,
            View::Pages => self.rows_pages()?,
            View::Journal => self.rows_journal()?,
            View::Search => self.rows_search()?,
            View::Log => self.rows_log()?,
        };
        self.context_hidden = projection.hidden;
        self.section_totals = projection.totals;
        self.row_vault = projection.tags;
        // Never a hidden filter (views.md §2.3): say how much the context hid.
        let hidden = self.context_hidden;
        let none_shown = self.rows.iter().all(|r| !matches!(r, Row::Node { .. }));
        if let (true, true, Some(a)) = (hidden > 0 && self.view != View::Search, none_shown, &self.context_active) {
            // Not "Nothing due today." over "3 hidden by @work": one empty state that says both.
            let what = match self.view {
                View::Today => "due ",
                View::Inbox => "to triage ",
                _ => "",
            };
            let e = Row::Empty { l1: format!("Nothing {what}in @{}.", a.ctx.name), l2: format!("C show the {hidden} hidden") };
            match self.rows.iter().position(|r| matches!(r, Row::Empty { .. })) {
                Some(i) => self.rows[i] = e,
                None => self.rows.push(e),
            }
        } else if hidden > 0 && self.view != View::Search {
            if let Some(a) = &self.context_active {
                self.rows.push(Row::Blank);
                self.rows.push(Row::Muted(format!("{hidden} hidden by @{} · C turn it off", a.ctx.name)));
            }
        }
        if self.edit.as_ref().is_some_and(|e| e.node.is_none()) {
            self.place_edit_row();
        }
        self.restore_cursor_from(was_from);
        // A list panel's rows (sidebar_list.rs): no document, no other panels.
        if self.in_list.is_some() {
            return Ok(());
        }
        self.sync_doc();
        // The sidebar's panels: loaded, live (sidebar.md §4.3, §11). Not from inside one.
        if self.in_panel.is_none() {
            self.ensure_panels();
            self.patch_panels();
            self.refresh_lists();
        }
        Ok(())
    }

    /// Put the new line's row where it will land: after `after`'s subtree, before `before`,
    /// or last under `parent` (§10.4).
    fn place_edit_row(&mut self) {
        let Some(e) = self.edit.clone() else { return };
        let node_at = |rows: &[Row], id: &str| rows.iter().position(|r| matches!(r, Row::Node { node, outline: true, .. } if node.id == id));
        let depth_at = |rows: &[Row], i: usize| match &rows[i] {
            Row::Node { depth, .. } => *depth,
            _ => 0,
        };
        let subtree_end = |rows: &[Row], i: usize| {
            let d = depth_at(rows, i);
            let mut j = i + 1;
            while j < rows.len() && matches!(&rows[j], Row::Node { outline: true, depth, .. } if *depth > d) {
                j += 1;
            }
            j
        };
        self.rows.retain(|r| !matches!(r, Row::Empty { .. }));
        let last_outline = self.rows.iter().rposition(|r| matches!(r, Row::Node { outline: true, .. })).map(|i| i + 1).unwrap_or(0);
        let idx = if let Some(i) = e.after.as_deref().and_then(|a| node_at(&self.rows, a)) {
            subtree_end(&self.rows, i)
        } else if let Some(i) = e.before.as_deref().and_then(|b| node_at(&self.rows, b)) {
            i
        } else if let Some(i) = e.parent.as_deref().and_then(|p| node_at(&self.rows, p)) {
            subtree_end(&self.rows, i)
        } else {
            last_outline
        };
        let idx = idx.min(self.rows.len());
        self.rows.insert(idx, Row::Editing);
        // Rows below moved down one: so do their vault tags.
        let tags = &mut self.row_vault;
        let moved: HashMap<usize, usize> = tags.drain().map(|(k, v)| (if k >= idx { k + 1 } else { k }, v)).collect();
        *tags = moved;
        self.selected = Some(EDIT_KEY.into());
    }

    /// `.` (§10.5): show or hide IDs on pages and Journal days, remembered on this device.
    pub fn toggle_page_ids(&mut self) {
        crate::runtime_effects::toggle_page_ids(self);
    }

    /// This device's choice (`<cache>/tui.toml`), else `[tui] page_ids` in device config.
    pub fn load_page_ids(&mut self) {
        let read = |p: std::path::PathBuf, section: bool| -> Option<bool> {
            let t: toml::Table = std::fs::read_to_string(p).ok()?.parse().ok()?;
            let t = if section { t.get("tui")?.as_table()?.clone() } else { t };
            t.get("page_ids")?.as_bool()
        };
        self.page_ids = read(self.vault.paths.cache.join("tui.toml"), false)
            .or_else(|| read(thc_core::policy::config_dir().join("config.toml"), true))
            .unwrap_or(false);
    }

    /// Whether an outline row in this view shows its ID.
    pub fn outline_ids(&self) -> bool {
        self.page_ids || !matches!(self.view, View::Pages | View::Journal)
    }

    fn row_depth(&self, id: &str) -> usize {
        self.rows.iter().find_map(|r| match r {
            Row::Node { node, depth, outline: true, .. } if node.id == id => Some(*depth),
            _ => None,
        }).unwrap_or(0)
    }

    /// `i` / `I` on an outline row: edit it in place, caret at the end or the start.
    pub fn edit_begin(&mut self, caret_start: bool) -> bool {
        let Some(n) = self.selected_node().cloned() else { return false };
        if !self.rows.get(self.cursor).is_some_and(|r| matches!(r, Row::Node { outline: true, .. })) {
            return false;
        }
        if n.title.is_some() {
            self.info("rename pages with thc set <id> title=…");
            return true;
        }
        let text = self.vault.store.render_text(&n.text);
        let cur = if caret_start { 0 } else { text.chars().count() };
        self.edit = Some(Edit {
            node: Some(n.id.clone()),
            parent: n.parent.clone(),
            after: None,
            before: None,
            depth: self.row_depth(&n.id),
            input: LineInput { buf: text.clone(), cur },
            orig: text,
            error: None,
            para: self.para_ids.contains(&n.id),
        });
        true
    }

    /// A new line in place: below or above `rel` (same depth), or last under `parent`.
    pub fn edit_new(&mut self, parent: Option<String>, after: Option<String>, before: Option<String>, depth: usize, text: &str, caret_start: bool) {
        self.edit_new_as(parent, after, before, depth, text, caret_start, false)
    }

    /// A new line in a given form: `para` for a line continued from a paragraph.
    #[allow(clippy::too_many_arguments)]
    pub fn edit_new_as(&mut self, parent: Option<String>, after: Option<String>, before: Option<String>, depth: usize, text: &str, caret_start: bool, para: bool) {
        // An open document takes the typing itself: a row editor under it would hide every key
        // until Esc (a bug: typing not shown in a new page).
        if self.doc.is_some() {
            return;
        }
        let cur = if caret_start { 0 } else { text.chars().count() };
        if let Some(p) = &parent {
            self.collapsed.remove(p);
        }
        // Only top-level lines can be paragraphs.
        let para = para && depth == 0;
        self.edit = Some(Edit { node: None, parent, after, before, depth, input: LineInput { buf: text.to_string(), cur }, orig: String::new(), error: None, para });
        self.selected = Some(EDIT_KEY.into());
        let _ = self.reload();
    }

    /// Save the line being edited (§10.3): one transaction, tokens parsed as in capture. An
    /// unchanged line writes nothing; an empty new line is dropped (`Ok(None)`). On a refusal
    /// the line stays in edit mode with the message in its chips (`Err`).
    pub fn edit_commit(&mut self) -> std::result::Result<Option<String>, ()> {
        let Some(e) = self.edit.clone() else { return Ok(None) };
        let text = e.input.buf.clone();
        if let Some(id) = &e.node {
            if text == e.orig || text.trim().is_empty() {
                return Ok(Some(id.clone()));
            }
        } else if text.trim().is_empty() {
            return Ok(None);
        }
        let (cap, bad) = match capture::parse_lenient(&text, self.today) {
            Ok(x) => x,
            Err(err) => {
                if let Some(ed) = self.edit.as_mut() {
                    ed.error = Some(friendly_error(&err));
                }
                return Err(());
            }
        };
        let journal = (self.view == View::Journal && e.parent.is_none()).then_some(self.journal_date);
        let mut saved = e.node.clone().unwrap_or_default();
        let out = &mut saved;
        let r = self.write(|b| {
            if let Some(id) = &e.node {
                return b.edit_from_capture(id, &cap);
            }
            let parent = match (&e.parent, journal) {
                (Some(p), _) => Some(p.clone()),
                (None, Some(d)) => Some(b.journal(d)?),
                (None, None) => None,
            };
            let id = b.create_from_capture(parent.clone(), &cap, None)?;
            if e.para && e.depth == 0 && cap.status.is_none() {
                thc_core::edit::mark_para(b, &id)?;
            }
            let siblings: Vec<Node> = match &parent {
                Some(p) => b.store.children(p)?,
                None => vec![],
            };
            let ord = if let Some(a) = &e.after {
                siblings.iter().position(|s| &s.id == a).map(|i| thc_core::ord::key_between(Some(&siblings[i].ord), siblings.get(i + 1).map(|s| s.ord.as_str())))
            } else if let Some(bf) = &e.before {
                siblings.iter().position(|s| &s.id == bf).map(|i| thc_core::ord::key_between(if i == 0 { None } else { Some(siblings[i - 1].ord.as_str()) }, Some(&siblings[i].ord)))
            } else {
                None
            };
            if let Some(o) = ord {
                b.set_created_order(&id, o);
            }
            *out = id;
            Ok(())
        });
        match r {
            Ok(_) => {
                if let Some(t) = bad.first() {
                    let v = t.split_once(':').map(|(_, v)| v.trim_matches('"')).unwrap_or(t);
                    self.info(format!("\"{v}\" kept as text"));
                }
                Ok(Some(saved))
            }
            Err(err) => {
                if let Some(ed) = self.edit.as_mut() {
                    ed.error = Some(friendly_error(&err));
                }
                Err(())
            }
        }
    }

    /// `Enter`: save, then a new line below at the same depth (continuous writing, §10.4).
    pub fn edit_enter(&mut self) {
        let Some(e) = self.edit.clone() else { return };
        if e.input.buf.trim().is_empty() {
            if e.node.is_some() {
                return self.info("empty line · Backspace deletes it");
            }
            // An empty Enter outdents one level; at the top it closes (never saved).
            return if e.depth > 0 { self.edit_nest(false) } else { self.edit_leave(None) };
        }
        // Mid-line Enter splits: the text after the caret becomes the next line.
        let chars: Vec<char> = e.input.buf.chars().collect();
        let (left, right): (String, String) = (chars[..e.input.cur.min(chars.len())].iter().collect(), chars[e.input.cur.min(chars.len())..].iter().collect());
        if !right.is_empty() {
            if let Some(ed) = self.edit.as_mut() {
                ed.input = LineInput { buf: left.trim_end().to_string(), cur: left.trim_end().chars().count() };
            }
        }
        let depth = e.depth;
        let para = e.para;
        match self.edit_commit() {
            Ok(Some(id)) => {
                let parent = self.vault.store.node(&id).ok().flatten().and_then(|n| n.parent);
                self.edit_new_as(parent, Some(id), None, depth, right.trim_start(), true, para);
            }
            Ok(None) => self.edit_leave(None),
            Err(()) => {}
        }
    }

    /// `Tab` / `⇧Tab` (§10.4): nest under the previous sibling / un-nest. A new line just
    /// moves (nothing is saved yet); an existing one is saved, then moved.
    pub fn edit_nest(&mut self, indent: bool) {
        let Some(e) = self.edit.clone() else { return };
        if let Some(id) = &e.node {
            if self.edit_commit().is_err() {
                return;
            }
            self.selected = Some(id.clone());
            let _ = self.reload();
            self.restructure(if indent { '>' } else { '<' });
            if let Some(ed) = self.edit.as_mut() {
                ed.orig = ed.input.buf.clone();
            }
            return;
        }
        let s = &self.vault.store;
        if indent {
            // The previous sibling: the row it was opened after, or the one before `before`,
            // or the parent's last child for an appended line.
            let prev = match (&e.after, &e.before, &e.parent) {
                (Some(a), _, _) => Some(a.clone()),
                (None, Some(b), Some(p)) => {
                    let kids = s.children(p).unwrap_or_default();
                    kids.iter().position(|k| &k.id == b).and_then(|i| i.checked_sub(1)).map(|i| kids[i].id.clone())
                }
                (None, None, Some(p)) => s.children(p).unwrap_or_default().last().map(|k| k.id.clone()),
                _ => None,
            };
            match prev {
                Some(p) => {
                    let (text, cur) = (e.input.buf.clone(), e.input.cur);
                    self.edit_new(Some(p), None, None, e.depth + 1, &text, false);
                    if let Some(ed) = self.edit.as_mut() {
                        ed.input.cur = cur;
                    }
                }
                None => self.info("nothing above to indent under"),
            }
        } else {
            if e.depth == 0 {
                return self.info("already at the top level");
            }
            let Some(p) = e.parent.clone() else { return };
            let gp = s.node(&p).ok().flatten().and_then(|n| n.parent);
            let (text, cur) = (e.input.buf.clone(), e.input.cur);
            self.edit_new(gp, Some(p), None, e.depth - 1, &text, false);
            if let Some(ed) = self.edit.as_mut() {
                ed.input.cur = cur;
            }
        }
    }

    /// `Backspace` on an empty line (§10.4): delete it and continue at the end of the row above.
    pub fn edit_backspace_empty(&mut self) {
        let Some(e) = self.edit.clone() else { return };
        let here = self.cursor;
        let above = (0..here).rev().find_map(|j| match &self.rows[j] {
            Row::Node { node, outline: true, .. } => Some(node.id.clone()),
            _ => None,
        });
        if let Some(id) = &e.node {
            let id = id.clone();
            if let Err(err) = self.write(|b| b.delete(&id).map(|_| ())) {
                if let Some(ed) = self.edit.as_mut() {
                    ed.error = Some(friendly_error(&err));
                }
                return;
            }
        }
        self.edit_leave(above.clone());
        if above.is_some() {
            self.edit_begin(false);
        }
    }

    /// `↑` / `↓`: save and move the input to the outline row above or below, caret at the end.
    pub fn edit_move(&mut self, delta: isize) {
        let here = self.cursor;
        let step = |i: isize| -> Option<String> {
            let mut j = here as isize + i;
            while j >= 0 && (j as usize) < self.rows.len() {
                if let Row::Node { node, outline: true, .. } = &self.rows[j as usize] {
                    return Some(node.id.clone());
                }
                j += i;
            }
            None
        };
        let target = step(delta.signum());
        let Ok(saved) = self.edit_commit() else { return };
        match target {
            Some(id) => {
                self.edit_leave(Some(id));
                self.edit_begin(false);
            }
            None => {
                // Nothing further: stay on this line (a saved new line becomes an ordinary row).
                if let (Some(id), true) = (saved, self.edit.as_ref().is_some_and(|e| e.node.is_none())) {
                    self.edit_leave(Some(id));
                    self.edit_begin(false);
                }
            }
        }
    }

    /// Back to Navigate, on the saved row (or where the dropped line was).
    pub fn edit_leave(&mut self, select: Option<String>) {
        let was = self.edit.take();
        // A dropped new line hands the cursor back to the row it was opened from.
        let back = was.and_then(|e| e.after.or(e.before).or(e.parent));
        if let Some(id) = select.or(back) {
            self.selected = Some(id);
        } else if self.selected.as_deref() == Some(EDIT_KEY) {
            self.selected = None;
        }
        let _ = self.reload();
    }

    /// `restore_cursor`, preferring the selected key's row from the vault it was in.
    fn restore_cursor_from(&mut self, from: Option<Option<usize>>) {
        if let (Some(key), Some(v)) = (self.selected.clone(), from) {
            if let Some(i) = (0..self.rows.len()).find(|&i| self.rows[i].key().as_deref() == Some(key.as_str()) && self.row_from(i) == v) {
                self.cursor = i;
                return;
            }
        }
        self.restore_cursor();
    }

    fn restore_cursor(&mut self) {
        if let Some(key) = &self.selected {
            if let Some(i) = self.rows.iter().position(|r| r.key().as_deref() == Some(key.as_str())) {
                self.cursor = i;
                return;
            }
        }
        let start = self.cursor.min(self.rows.len().saturating_sub(1));
        // `+ new page` is never where the cursor lands by itself (§10.6): a stray Enter
        // must not create a page.
        let pick = |r: &Row| r.selectable() && !matches!(r, Row::NewPage { .. });
        match (start..self.rows.len()).chain((0..start).rev()).find(|&i| pick(&self.rows[i])) {
            Some(i) => {
                self.cursor = i;
                self.selected = self.rows.get(i).and_then(|r| r.key());
            }
            None => {
                self.cursor = self.rows.iter().position(|r| !r.selectable()).unwrap_or(0);
                self.selected = None;
            }
        }
    }

    fn node_row(n: Node) -> Row {
        Row::Node { node: n, depth: 0, outline: false, has_children: false, collapsed: false, child_count: 0, under_day: None }
    }

    /// Keep what the context lets through, counting what it hid.
    fn cf(&self, nodes: Vec<Node>, projection: &mut RowProjection) -> Vec<Node> {
        match &self.context_active {
            Some(a) => {
                let (kept, total) = a.filter(nodes, |n| n.id.as_str());
                projection.hidden += total - kept.len();
                kept
            }
            None => nodes,
        }
    }

    /// The context filter on this vault's rows of a section, recording `2 of 3` when it hid some
    /// (`extra`: the section's rows from other vaults, which it never filters).
    fn cf_counted(&self, projection: &mut RowProjection, title: &str, nodes: Vec<Node>, extra: usize) -> Vec<Node> {
        let total = nodes.len();
        let kept = self.cf(nodes, projection);
        if kept.len() < total {
            projection.totals.insert(title.to_string(), total + extra);
        }
        kept
    }

    /// `section` for rows tagged with their vault (None: here): another vault's rows are
    /// recorded by position in `tags`.
    fn section_from(rows: &mut Vec<Row>, title: &str, token: Token, items: Vec<(Option<usize>, Node)>, tags: &mut HashMap<usize, usize>) {
        if items.is_empty() {
            return;
        }
        if !rows.is_empty() {
            rows.push(Row::Blank);
        }
        rows.push(Row::Section { title: title.into(), count: Some(items.len()), token, note: None });
        for (v, n) in items {
            if let Some(i) = v {
                tags.insert(rows.len(), i);
            }
            rows.push(Self::node_row(n));
        }
    }

    fn section(rows: &mut Vec<Row>, title: &str, token: Token, nodes: Vec<Node>) {
        if nodes.is_empty() {
            return;
        }
        if !rows.is_empty() {
            rows.push(Row::Blank);
        }
        rows.push(Row::Section { title: title.into(), count: Some(nodes.len()), token, note: None });
        rows.extend(nodes.into_iter().map(Self::node_row));
    }

    /// One vault's Today sections (views.md §3.3): overdue, today (and the rows only an alert
    /// puts there, by time), doing, next 7 days, done today.
    fn today_secs(s: &thc_core::store::Store, day: NaiveDate) -> Result<TodaySecs> {
        let t = ymd(day);
        let horizon = ymd(day + Duration::days(7));
        let open = "n.status IN ('todo','doing','waiting') AND n.deleted=0";
        let overdue = s.nodes_where(&format!("{open} AND substr(n.due,1,10) < ?1 ORDER BY n.due"), &[&t])?;
        // ISO ranges so each branch uses an index (as in the CLI and daemon).
        let t1 = ymd(day + Duration::days(1));
        let today = s.nodes_where(
            "n.deleted=0 AND n.is_tag=0 AND (n.status IS NULL OR n.status IN ('todo','waiting')) \
             AND NOT (n.due IS NOT NULL AND n.due < ?1) AND n.id IN ( \
               SELECT id FROM nodes WHERE status IN ('todo','doing','waiting') AND deleted=0 AND scheduled < ?2 \
               UNION SELECT id FROM nodes WHERE due >= ?1 AND due < ?2 \
               UNION SELECT id FROM nodes WHERE status IS NULL AND scheduled >= ?1 AND scheduled < ?2) \
             ORDER BY n.scheduled IS NULL, n.scheduled, CASE n.priority WHEN 'high' THEN 0 WHEN 'med' THEN 1 ELSE 2 END",
            &[&t, &t1],
        )?;
        let doing = s.nodes_where(
            "n.status='doing' AND n.deleted=0 AND NOT (n.due IS NOT NULL AND substr(n.due,1,10) < ?1) ORDER BY n.updated_ms DESC",
            &[&t],
        )?;
        // Dates as ranges (`> today` is `>= tomorrow`, `<= horizon` is `< the day after`) so the
        // due and scheduled indexes serve it: substr() forced a scan of every note.
        let h1 = ymd(day + Duration::days(8));
        let next = s.nodes_where(
            "n.deleted=0 AND n.is_tag=0 AND (n.status IS NULL OR n.status IN ('todo','waiting')) AND n.id IN ( \
               SELECT id FROM nodes WHERE scheduled >= ?1 AND scheduled < ?2 \
               UNION SELECT id FROM nodes WHERE due >= ?1 AND due < ?2) \
             AND NOT (n.status IS NOT NULL AND coalesce(n.scheduled,'9999') < ?1) \
             ORDER BY min(coalesce(n.scheduled,'9'), coalesce(n.due,'9')), n.rowid",
            &[&t1, &h1],
        )?;
        let _ = &horizon;
        // In Today only because an alert fires today: Today rows, by alert time, and no longer
        // under Next 7 days (views.md §3.3).
        let shown: std::collections::HashSet<String> = overdue.iter().chain(today.iter()).chain(doing.iter()).map(|n| n.id.clone()).collect();
        let alerted: Vec<Node> = s.alert_rows(day, &shown)?.into_iter().map(|(n, _)| n).collect();
        let next: Vec<Node> = next.into_iter().filter(|n| !today.iter().chain(alerted.iter()).any(|t| t.id == n.id)).collect();
        let done = s.nodes_where("n.deleted=0 AND n.status='done' AND n.done_at >= ?1 AND n.done_at < ?2 ORDER BY n.done_at", &[&t, &t1])?;
        Ok(TodaySecs { overdue, today, alerted, doing, next, done })
    }

    /// Your @today (`thc view set today --section …`), from the home vault (or here).
    fn edited_today(&self) -> Option<thc_core::views::Sectioned> {
        let mine = thc_core::views::sectioned(&self.vault.store, "today").ok().flatten().filter(|v| v.edited);
        if mine.is_some() {
            return mine;
        }
        let home = thc_core::registry::Registry::load().home?;
        let o = self.others.iter().find(|o| o.path.ends_with(&home) || thc_core::registry::Registry::load().find(&home).is_some_and(|e| e.path == o.path))?;
        thc_core::views::sectioned(&o.vault.store, "today").ok().flatten().filter(|v| v.edited)
    }

    /// Today from your sections: each section's query in each vault of its scope, merged in its
    /// own order, rows tagged with their vault.
    fn rows_sectioned(&self, v: &thc_core::views::Sectioned, projection: &mut RowProjection) -> Result<Vec<Row>> {
        let names: Option<Vec<String>> =
            v.scope.as_deref().and_then(|sc| thc_core::federated::split_scope(sc).ok()).and_then(|(scope, _)| scope).map(|scope| {
                let reg = thc_core::registry::Registry::load();
                thc_core::federated::resolve(&scope, &reg, &self.vault_name).map(|(e, _)| e.into_iter().map(|e| e.name).collect()).unwrap_or_default()
            });
        let reg = thc_core::registry::Registry::load();
        let name_of = |o: &OtherVault| reg.by_path(&o.path).map(|e| e.name.clone()).unwrap_or_else(|| o.name.clone());
        let in_scope = |n: &str| names.as_ref().is_none_or(|ns| ns.iter().any(|x| x == n));
        // Stores in scope: (vault tag, store); None is here.
        let mut stores: Vec<(Option<usize>, &thc_core::store::Store)> = Vec::new();
        if in_scope(&self.vault_name) {
            stores.push((None, &self.vault.store));
        }
        if names.is_some() {
            for (i, o) in self.others.iter().enumerate() {
                if in_scope(&name_of(o)) {
                    stores.push((Some(i), &o.vault.store));
                }
            }
        }
        let refs: Vec<&thc_core::store::Store> = stores.iter().map(|(_, s)| *s).collect();
        let rows_by = thc_core::federated::run_sections(&v.sections, &refs, self.today, 500)?;
        let mut tags = HashMap::new();
        tags.clear();
        let mut rows = Vec::new();
        for (sec, items) in v.sections.iter().zip(rows_by) {
            let items: Vec<(Option<usize>, Node)> = items
                .into_iter()
                .filter(|(si, n)| stores[*si].0.is_some() || self.cf(vec![n.clone()], projection).len() == 1)
                .map(|(si, n)| (stores[si].0, n))
                .collect();
            let token = if sec.title == "Overdue" { Token::Overdue } else { Token::Text };
            Self::section_from(&mut rows, &sec.title, token, items, &mut tags);
        }
        if rows.is_empty() {
            rows.push(Row::Empty { l1: "Nothing in your Today's sections.".into(), l2: "thc view ls · thc view reset today".into() });
        }
        projection.tags = tags;
        Ok(rows)
    }

    fn rows_today(&self, projection: &mut RowProjection) -> Result<Vec<Row>> {
        if let Some(mut v) = self.edited_today() {
            v.scope = Some(self.scope_now());
            return self.rows_sectioned(&v, projection);
        }
        let s = &self.vault.store;
        // The scope on this device (view-explain.md §2): which of these vaults Today reads.
        let names = self.scope_vaults();
        let reg = thc_core::registry::Registry::load();
        let in_scope_other = |o: &OtherVault| names.iter().any(|n| reg.by_path(&o.path).is_some_and(|e| &e.name == n));
        let mut cur = Self::today_secs(s, self.today)?;
        if !self.others.is_empty() && !names.contains(&self.vault_name) {
            cur = TodaySecs { overdue: vec![], today: vec![], alerted: vec![], doing: vec![], next: vec![], done: vec![] };
        }
        // Every vault (vaults.md §3.5): the others' rows merge into each section in its own
        // order, each remembering its vault (its meta names it; writes go there). Two vaults'
        // notes with the same id are two rows.
        let all: Vec<TodaySecs> = self
            .others
            .iter()
            .map(|o| {
                if in_scope_other(o) {
                    Self::today_secs(&o.vault.store, self.today)
                } else {
                    Ok(TodaySecs { overdue: vec![], today: vec![], alerted: vec![], doing: vec![], next: vec![], done: vec![] })
                }
            })
            .collect::<Result<_>>()?;
        let tag = |v: Option<usize>, ns: Vec<Node>| -> Vec<(Option<usize>, Node)> { ns.into_iter().map(|n| (v, n)).collect() };
        let theirs = |f: fn(&TodaySecs) -> &Vec<Node>| -> Vec<(Option<usize>, Node)> {
            all.iter().enumerate().flat_map(|(i, x)| f(x).iter().cloned().map(move |n| (Some(i), n))).collect()
        };
        let merge = |mine: Vec<Node>, other: Vec<(Option<usize>, Node)>, key: &dyn Fn(&Node) -> String| -> Vec<(Option<usize>, Node)> {
            let mut v = tag(None, mine);
            v.extend(other);
            v.sort_by_key(|(_, n)| key(n));
            v
        };
        let prio = |n: &Node| match n.priority.as_deref() {
            Some("high") => '0',
            Some("med") => '1',
            _ => '2',
        };
        let overdue = merge(self.cf_counted(projection, "Overdue", cur.overdue, theirs(|x| &x.overdue).len()), theirs(|x| &x.overdue), &|n| {
            n.due.clone().unwrap_or_default()
        });
        let mut today =
            merge(self.cf_counted(projection, "Today", cur.today, theirs(|x| &x.today).len() + theirs(|x| &x.alerted).len()), theirs(|x| &x.today), &|n| {
                format!("{}{}{}", if n.scheduled.is_none() { 1 } else { 0 }, n.scheduled.clone().unwrap_or_default(), prio(n))
            });
        // Rows only an alert puts in Today come after, by alert time (§3.3).
        today.extend(tag(None, self.cf(cur.alerted, projection)));
        today.extend(theirs(|x| &x.alerted));
        let doing = merge(self.cf_counted(projection, "Doing", cur.doing, theirs(|x| &x.doing).len()), theirs(|x| &x.doing), &|n| {
            format!("{:020}", i64::MAX - n.updated_ms)
        });
        let next = merge(self.cf_counted(projection, "Next 7 days", cur.next, theirs(|x| &x.next).len()), theirs(|x| &x.next), &|n| {
            let a = n.scheduled.clone().unwrap_or_else(|| "9".into());
            let b = n.due.clone().unwrap_or_else(|| "9".into());
            a.min(b)
        });
        let done_t = merge(self.cf(cur.done, projection), theirs(|x| &x.done), &|n| n.done_at.clone().unwrap_or_default());
        let mut tags = HashMap::new();
        tags.clear();
        let mut rows = Vec::new();
        if self.today_by_vault && !self.others.is_empty() {
            // A section per vault (this one first), each in Today's order.
            let open: Vec<(Option<usize>, Node)> = overdue.into_iter().chain(today).chain(doing).chain(next).collect();
            let mine: Vec<(Option<usize>, Node)> = open.iter().filter(|(v, _)| v.is_none()).cloned().collect();
            Self::section_from(&mut rows, &self.vault_name, Token::Text, mine, &mut tags);
            for (i, o) in self.others.iter().enumerate() {
                let them: Vec<(Option<usize>, Node)> = open.iter().filter(|(v, _)| *v == Some(i)).cloned().collect();
                Self::section_from(&mut rows, &o.name, Token::Text, them, &mut tags);
            }
        } else {
            Self::section_from(&mut rows, "Overdue", Token::Overdue, overdue, &mut tags);
            Self::section_from(&mut rows, "Today", Token::Text, today, &mut tags);
            Self::section_from(&mut rows, "Doing", Token::Text, doing, &mut tags);
            Self::section_from(&mut rows, "Next 7 days", Token::Text, next, &mut tags);
        }
        let done: Vec<Node> = done_t.iter().map(|(_, n)| n.clone()).collect();
        if done.len() > 3 && !self.show_all_done {
            if !rows.is_empty() {
                rows.push(Row::Blank);
            }
            rows.push(Row::Section { title: "Done today".into(), count: Some(done.len()), token: Token::Text, note: Some("v show".into()) });
        } else {
            Self::section_from(&mut rows, "Done today", Token::Text, done_t, &mut tags);
        }
        if rows.iter().all(|r| !matches!(r, Row::Node { .. })) {
            // A vault with nothing written yet gets the first-run welcome: a
            // block of lines, `key<TAB>what`, drawn centred.
            let written: i64 = s.conn.query_row("SELECT count(*) FROM nodes WHERE deleted=0 AND is_tag=0 AND trim(text) <> ''", [], |r| r.get(0)).unwrap_or(1);
            let (l1, l2) = if written == 0 {
                ("Nothing here yet.".to_string(), "5 or thc j\twrite in today's journal\na\tcapture a thought\n?\tevery key".to_string())
            } else if done.is_empty() {
                ("Nothing due today.".to_string(), "5 write in today's journal  ·  a capture".to_string())
            } else {
                (format!("All clear. {} done today.", done.len()), "a capture  ·  w agenda".to_string())
            };
            rows.insert(0, Row::Empty { l1, l2 });
        }
        projection.tags = tags;
        Ok(rows)
    }

    fn rows_agenda(&self, projection: &mut RowProjection) -> Result<Vec<Row>> {
        let s = &self.vault.store;
        let t = ymd(self.today);
        let mut rows = vec![Row::Section { title: "Agenda".into(), count: None, token: Token::Text, note: Some("7 days   w sections".into()) }];
        // Every vault (vaults.md §3.5): the others' rows merge into each part, each with its
        // vault (by position: the same id in two vaults is two rows).
        let mut tags = HashMap::new();
        tags.clear();
        let od_sql = "n.status IN ('todo','doing','waiting') AND n.deleted=0 AND substr(n.due,1,10) < ?1 ORDER BY n.due";
        let mut overdue: Vec<(Option<usize>, Node)> = self.cf(s.nodes_where(od_sql, &[&t])?, projection).into_iter().map(|n| (None, n)).collect();
        for (i, o) in self.others.iter().enumerate() {
            overdue.extend(o.vault.store.nodes_where(od_sql, &[&t])?.into_iter().map(|n| (Some(i), n)));
        }
        overdue.sort_by(|a, b| a.1.due.cmp(&b.1.due));
        if !overdue.is_empty() {
            rows.push(Row::Section { title: "Overdue".into(), count: Some(overdue.len()), token: Token::Overdue, note: None });
            for (v, n) in overdue {
                if let Some(i) = v {
                    tags.insert(rows.len(), i);
                }
                rows.push(Self::node_row(n));
            }
        }
        let day_sql = "n.deleted=0 AND n.is_tag=0 AND (n.status IS NULL OR n.status IN ('todo','doing','waiting')) AND \
                 (substr(n.scheduled,1,10) = ?1 OR substr(n.due,1,10) = ?1) ORDER BY coalesce(n.scheduled, n.due)";
        let others = &self.others;
        let day_items = |d: NaiveDate| -> Result<Vec<(Option<usize>, Node)>> {
            let mut v: Vec<(Option<usize>, Node)> = s.nodes_where(day_sql, &[&ymd(d)])?.into_iter().map(|n| (None, n)).collect();
            for (i, o) in others.iter().enumerate() {
                v.extend(o.vault.store.nodes_where(day_sql, &[&ymd(d)])?.into_iter().map(|n| (Some(i), n)));
            }
            v.sort_by(|a, b| a.1.scheduled.as_ref().or(a.1.due.as_ref()).cmp(&b.1.scheduled.as_ref().or(b.1.due.as_ref())));
            Ok(v)
        };
        let mut empty_run: Vec<NaiveDate> = Vec::new();
        let flush = |run: &mut Vec<NaiveDate>, rows: &mut Vec<Row>| {
            if run.is_empty() {
                return;
            }
            let label = run.iter().map(|d| d.format("%a %b %-d").to_string()).collect::<Vec<_>>().join(" · ");
            rows.push(Row::Note { parts: vec![(label, Token::Muted)], right: Some("nothing scheduled".into()), narrow: None });
            run.clear();
        };
        for i in 0..7 {
            let d = self.today + Duration::days(i);
            let items = day_items(d)?;
            if items.is_empty() && i > 0 {
                empty_run.push(d);
                continue;
            }
            flush(&mut empty_run, &mut rows);
            let rel = dates::relative(d, self.today);
            let title = if i < 2 { format!("{} · {rel}", d.format("%a %b %-d")) } else { d.format("%a %b %-d").to_string() };
            rows.push(Row::Section { title, count: None, token: Token::Text, note: None });
            for (v, n) in items {
                if let Some(vi) = v {
                    tags.insert(rows.len(), vi);
                }
                rows.push(match Self::node_row(n) {
                    Row::Node { node, depth, outline, has_children, collapsed, child_count, .. } => {
                        Row::Node { node, depth, outline, has_children, collapsed, child_count, under_day: Some(d) }
                    }
                    r => r,
                });
            }
        }
        flush(&mut empty_run, &mut rows);
        let later = s.nodes_where(
            "n.deleted=0 AND n.is_tag=0 AND (n.status IS NULL OR n.status IN ('todo','doing','waiting')) AND \
             min(coalesce(substr(n.scheduled,1,10),'9'), coalesce(substr(n.due,1,10),'9')) BETWEEN ?1 AND ?2 \
             ORDER BY min(coalesce(n.scheduled,'9'), coalesce(n.due,'9'))",
            &[&ymd(self.today + Duration::days(7)), &ymd(self.today + Duration::days(13))],
        )?;
        if !later.is_empty() {
            rows.push(Row::Section { title: "Later".into(), count: None, token: Token::Text, note: None });
            rows.extend(later.into_iter().map(Self::node_row));
        }
        projection.tags = tags;
        Ok(rows)
    }

    fn rows_inbox(&self, projection: &mut RowProjection) -> Result<Vec<Row>> {
        let nodes = self
            .vault
            .store
            .nodes_where("n.parent IS NULL AND n.title IS NULL AND n.journal IS NULL AND n.is_tag=0 AND n.deleted=0 ORDER BY n.created_ms", &[])?;
        let total = nodes.len();
        let nodes = self.cf(nodes, projection);
        if nodes.len() < total {
            projection.totals.insert("Inbox".into(), total);
        }
        if nodes.is_empty() {
            return Ok(vec![Row::Empty { l1: "Inbox empty.".into(), l2: "A captures here · or drop a file in vault/drop/".into() }]);
        }
        let mut rows = vec![Row::Section { title: "Inbox".into(), count: Some(nodes.len()), token: Token::Text, note: Some("oldest first".into()) }];
        rows.extend(nodes.into_iter().map(Self::node_row));
        Ok(rows)
    }

    /// The query's tasks in the order you've been looking at: each where it was (one no longer
    /// matching too, as it is now; deleted, it goes), and a new one after the task before it
    /// in the query's order (at the top when none is). As `reload_stable` keeps a list.
    fn in_tasks_order(&self, query: Vec<Node>, order: &[String]) -> Vec<Node> {
        let mut by_id: HashMap<String, Node> = query.iter().map(|n| (n.id.clone(), n.clone())).collect();
        let mut out: Vec<Node> = Vec::new();
        for id in order {
            match by_id.remove(id) {
                Some(n) => out.push(n),
                None => {
                    if let Some(n) = self.vault.store.node(id).ok().flatten().filter(|n| !n.deleted) {
                        out.push(n);
                    }
                }
            }
        }
        let mut prev: Option<String> = None;
        for n in query {
            if by_id.contains_key(&n.id) {
                let at = prev.as_ref().and_then(|p| out.iter().position(|o| &o.id == p)).map_or(0, |i| i + 1);
                prev = Some(n.id.clone());
                out.insert(at, n);
            } else {
                prev = Some(n.id);
            }
        }
        out
    }

    fn rows_tasks(&mut self, projection: &mut RowProjection) -> Result<Vec<Row>> {
        let started = Instant::now();
        let nodes = match self.vault.store.query(&self.tasks_filter, self.today, 500).map(|n| self.cf(n, projection)) {
            Ok(n) => match self.ui.tasks_order.clone() {
                // The order you've been looking at (the state's `tasks_order`, so a fresh
                // session draws it too; cjn86).
                Some((filter, order)) if filter == self.tasks_filter => self.in_tasks_order(n, &order),
                Some(_) => {
                    self.ui.tasks_order = None;
                    n
                }
                None => n,
            },
            Err(e) => {
                let msg = friendly_error(&e);
                // `unknown status "opn" · did you mean open?` -> bad token + fix
                let bad_val = msg.split('"').nth(1).unwrap_or("").to_string();
                let fix_val = msg.split("did you mean ").nth(1).map(|s| s.trim_end_matches('?').to_string());
                let bad_tok = self.tasks_filter.split_whitespace().find(|t| !bad_val.is_empty() && t.ends_with(&bad_val)).unwrap_or("").to_string();
                let fix_tok = fix_val.map(|f| format!("{}{f}", &bad_tok[..bad_tok.len() - bad_val.len()]));
                self.tasks_error = Some((msg.clone(), bad_tok, fix_tok));
                if self.tasks_last_good.is_empty() {
                    return Ok(vec![Row::Empty { l1: msg, l2: "f edit the filter  ·  Esc clear it".into() }]);
                }
                return Ok(self.tasks_last_good.clone());
            }
        };
        self.tasks_error = None;
        self.tasks_ms = started.elapsed().as_secs_f64() * 1000.0;
        let rows = if nodes.is_empty() {
            let (l1, l2) = if self.tasks_filter == "status:open sort:due" {
                ("No open tasks.".to_string(), "a capture  ·  t on any node makes it a task".to_string())
            } else {
                (format!("No tasks match  {}", self.tasks_filter), "f edit the filter  ·  Esc clear it".to_string())
            };
            vec![Row::Empty { l1, l2 }]
        } else {
            // `group:` turns the list into sections styled like Today's (views.md §3.2).
            let by = thc_core::query::explain(&self.tasks_filter, &self.vault.store, self.today).ok().and_then(|e| e.group.map(|g| (g, !e.sorts.is_empty())));
            self.tasks_group = by.as_ref().map(|(g, _)| g.clone());
            let mut rows: Vec<Row> = match by {
                Some((by, sorted)) => {
                    let mut rows = Vec::new();
                    for g in thc_core::group::group(&self.vault.store, nodes, &by, self.today, sorted)?.groups {
                        Self::section(&mut rows, &g.label, if g.overdue { Token::Overdue } else { Token::Text }, g.items);
                    }
                    rows
                }
                None => nodes.into_iter().map(Self::node_row).collect(),
            };
            rows.push(Row::Blank);
            let mut parts = vec![("saved".to_string(), Token::Muted)];
            let mut narrow = parts.clone();
            let slots = self.view_slots();
            for (slot, v) in &slots {
                for p in [&mut parts, &mut narrow] {
                    p.push((format!("  {slot}"), Token::Text));
                    p.push((format!(" {}", v.title.clone().unwrap_or_else(|| v.name.clone())), Token::Muted));
                }
                // Wide, each view's query too.
                let short = v.query.replace(" sort:due", "");
                parts.push((format!(" {short}"), Token::Muted));
            }
            let max = slots.last().map(|(n, _)| *n).unwrap_or(1);
            rows.push(Row::Note { parts, right: Some(format!("f then 1-{max}")), narrow: Some(narrow) });
            rows
        };
        self.tasks_last_good = rows.clone();
        Ok(rows)
    }

    fn outline(&self, parent: &str, depth: usize, rows: &mut Vec<Row>) -> Result<()> {
        for c in self.vault.store.children(parent)? {
            let kids = self.vault.store.children(&c.id)?;
            let collapsed = self.collapsed.contains(&c.id);
            let id = c.id.clone();
            rows.push(Row::Node { node: c, depth, outline: true, has_children: !kids.is_empty(), collapsed, child_count: kids.len(), under_day: None });
            if !collapsed && !kids.is_empty() {
                self.outline(&id, depth + 1, rows)?;
            }
        }
        Ok(())
    }

    /// A top-level plain note on a page or day, drawn as a wrapped paragraph (§10.8).
    pub fn is_prose(&self, row: &Row) -> bool {
        let outline_view = self.view == View::Journal || (self.view == View::Pages && self.page_open.is_some());
        match row {
            Row::Node { node: n, depth: 0, outline: true, has_children: false, .. } => {
                // Only notes written as paragraphs: everything else stays a bullet (§10.8).
                outline_view && self.para_ids.contains(&n.id) && n.status.is_none() && n.scheduled.is_none() && n.due.is_none() && n.priority.is_none() && n.repeat.is_none() && n.title.is_none()
            }
            _ => false,
        }
    }

    /// One blank row between a top-level paragraph and its neighbours; bullets stay tight.
    fn space_paragraphs(&self, rows: Vec<Row>) -> Vec<Row> {
        let mut out: Vec<Row> = Vec::with_capacity(rows.len());
        let mut prev_para: Option<bool> = None;
        for r in rows {
            if matches!(r, Row::Node { depth: 0, outline: true, .. }) {
                let para = self.is_prose(&r);
                if prev_para.is_some_and(|p| p || para) {
                    out.push(Row::Blank);
                }
                prev_para = Some(para);
            }
            out.push(r);
        }
        out
    }

    pub fn outline_rows(&self, page: &str) -> Result<Vec<Row>> {
        let s = &self.vault.store;
        let mut rows = Vec::new();
        self.outline(page, 0, &mut rows)?;
        let mut rows = self.space_paragraphs(rows);
        if rows.is_empty() {
            rows.push(Row::Empty { l1: "This page is empty.".into(), l2: "Enter write  ·  e $EDITOR".into() });
        }
        let back = s.backlinks(page)?;
        if !back.is_empty() {
            rows.push(Row::Blank);
            rows.push(Row::Section { title: format!("{} linked from", self.theme.glyphs().backlink), count: Some(back.len()), token: Token::Muted, note: None });
            rows.extend(back.into_iter().map(Self::node_row));
        }
        Ok(rows)
    }

    fn rows_pages(&self) -> Result<Vec<Row>> {
        let s = &self.vault.store;
        if let Some(pid) = &self.page_open {
            return self.outline_rows(pid);
        }
        // `@…` lists saved views; Enter opens one in Tasks.
        if let Some(q) = self.pages_filter.strip_prefix('@') {
            let matched: Vec<&thc_core::views::View> = self.saved_views.iter().filter(|v| fuzzy(&q.to_lowercase(), &v.name).is_some()).collect();
            let mut rows = vec![Row::Section { title: "Views".into(), count: Some(matched.len()), token: Token::Text, note: Some("Enter opens in Tasks".into()) }];
            for v in matched {
                let count = s.query(&v.query, self.today, 100_000).map(|n| n.len()).unwrap_or(0);
                rows.push(Row::ViewItem { name: v.name.clone(), query: v.query.clone(), count });
            }
            return Ok(rows);
        }
        let pages = s.nodes_where(&format!("n.parent IS NULL AND n.title IS NOT NULL AND n.is_tag=0 AND n.deleted=0 AND {} ORDER BY n.title COLLATE NOCASE", thc_core::views::HIDDEN_SQL), &[])?;
        let f = self.pages_filter.to_lowercase();
        let mut matched: Vec<(i64, Node)> = pages.into_iter().filter_map(|p| fuzzy(&f, &p.label().to_lowercase()).map(|sc| (sc, p))).collect();
        if !f.is_empty() {
            matched.sort_by_key(|(sc, _)| -*sc);
        }
        let mut rows = Vec::new();
        let title = self.pages_filter.trim().to_string();
        // A one-character query never offers a page (§10.6).
        let can_create = title.chars().count() >= 2;
        if matched.is_empty() {
            if f.is_empty() {
                rows.push(Row::Empty { l1: "No pages yet.".into(), l2: "Link one into existence: [[Page Title]]".into() });
            } else if can_create {
                rows.push(Row::Empty { l1: format!("No page matches \"{title}\"."), l2: format!("↓ then Enter creates {} {title}  ·  Esc clear", self.theme.glyphs().page) });
                rows.push(Row::NewPage { title: title.clone() });
            } else {
                rows.push(Row::Empty { l1: format!("No page matches \"{title}\"."), l2: "type a longer name to create a page  ·  Esc clear".into() });
            }
        } else {
            rows.push(Row::Section { title: "Pages".into(), count: Some(matched.len()), token: Token::Text, note: Some("sort: name".into()) });
            rows.extend(matched.into_iter().map(|(_, p)| Self::node_row(p)));
            if can_create && !rows.iter().any(|r| matches!(r, Row::Node { node, .. } if node.label().eq_ignore_ascii_case(&title))) {
                rows.push(Row::NewPage { title: title.clone() });
            }
        }
        if !f.is_empty() {
            let n = rows.iter().filter(|r| matches!(r, Row::Node { .. })).count();
            rows.push(Row::Blank);
            rows.push(Row::Note { parts: vec![(format!("{n} page{} · fuzzy: {}", if n == 1 { "" } else { "s" }, self.pages_filter), Token::Muted)], right: None, narrow: None });
            return Ok(rows);
        }
        let mut st = s.conn.prepare(
            "SELECT t.title, count(e.src) FROM nodes t JOIN edges e ON e.dst=t.id AND e.rel='tag' \
             WHERE t.is_tag=1 AND t.deleted=0 GROUP BY t.id HAVING count(e.src) > 0 ORDER BY t.title",
        )?;
        let tags: Vec<(String, i64)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
        if !tags.is_empty() {
            rows.push(Row::Blank);
            rows.push(Row::Section { title: "Tags".into(), count: Some(tags.len()), token: Token::Text, note: None });
            let mut parts = Vec::new();
            for (i, (name, c)) in tags.iter().enumerate() {
                if i > 0 {
                    parts.push(("   ".to_string(), Token::Muted));
                }
                parts.push((format!("#{name}"), Token::Tag));
                parts.push((format!(" {c}"), Token::Muted));
            }
            rows.push(Row::Note { parts, right: None, narrow: None });
        }
        let mut st = s.conn.prepare(
            "SELECT j.journal, (SELECT count(*) FROM nodes c WHERE c.parent=j.id AND c.deleted=0) AS n FROM nodes j \
             WHERE j.journal IS NOT NULL AND j.deleted=0 AND n > 0 ORDER BY j.journal DESC LIMIT 4",
        )?;
        let days: Vec<(String, i64)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
        if !days.is_empty() {
            rows.push(Row::Blank);
            rows.push(Row::Section { title: "Journal".into(), count: None, token: Token::Text, note: Some("recent".into()) });
            let g = self.theme.glyphs();
            let mut parts = Vec::new();
            for (i, (d, c)) in days.iter().enumerate() {
                if let Ok(date) = NaiveDate::parse_from_str(d, "%Y-%m-%d") {
                    if i > 0 {
                        parts.push(("     ".to_string(), Token::Muted));
                    }
                    parts.push((format!("{} {}", g.journal, date.format("%a %b %-d")), Token::Text));
                    parts.push((format!(" {} {c}", g.sep), Token::Muted));
                }
            }
            rows.push(Row::Note { parts, right: None, narrow: None });
        }
        Ok(rows)
    }

    fn rows_journal(&self) -> Result<Vec<Row>> {
        let key = ymd(self.journal_date);
        let mut rows = Vec::new();
        if let Some(j) = self.vault.store.journal_node(&key)? {
            self.outline(&j, 0, &mut rows)?;
        }
        let mut rows = self.space_paragraphs(rows);
        if rows.is_empty() {
            rows.push(if self.journal_date == self.today {
                Row::Empty { l1: "Today's page is blank.".into(), l2: "a write the first line".into() }
            } else {
                Row::Empty { l1: format!("Nothing written on {}.", self.journal_date.format("%a %b %-d")), l2: "[ ] move a day  ·  T today".into() }
            });
        }
        if self.journal_date == self.today {
            let done = self.vault.store.nodes_where("n.deleted=0 AND n.status='done' AND substr(n.done_at,1,10) = ?1 ORDER BY n.done_at", &[&key])?;
            if !done.is_empty() {
                rows.push(Row::Blank);
                rows.push(Row::Section { title: "Completed today".into(), count: Some(done.len()), token: Token::Text, note: None });
                rows.extend(done.into_iter().map(Self::node_row));
            }
        }
        Ok(rows)
    }

    fn rows_search(&mut self) -> Result<Vec<Row>> {
        if self.search_terms.trim().is_empty() {
            return Ok(vec![Row::Empty { l1: "Search everything.".into(), l2: "/ type to search  ·  Enter open  ·  ⌃Q as a task filter".into() }]);
        }
        let started = Instant::now();
        let nodes = self.vault.store.search(&self.search_terms, 200)?;
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        if nodes.is_empty() {
            return Ok(vec![Row::Empty {
                l1: format!("No matches for \"{}\".", self.search_terms),
                l2: "try fewer words  ·  add is:deleted to include deleted".into(),
            }]);
        }
        self.search_ms = elapsed;
        let (pages, rest): (Vec<Node>, Vec<Node>) = nodes.into_iter().partition(|n| n.parent.is_none() && n.title.is_some() && !n.is_tag);
        let (tags, nodes): (Vec<Node>, Vec<Node>) = rest.into_iter().partition(|n| n.is_tag);
        let mut rows = Vec::new();
        Self::section(&mut rows, "Pages", Token::Text, pages);
        if !tags.is_empty() {
            if !rows.is_empty() {
                rows.push(Row::Blank);
            }
            rows.push(Row::Section { title: "Tags".into(), count: Some(tags.len()), token: Token::Text, note: None });
            for t in tags {
                let c = self.vault.store.conn.query_row("SELECT count(*) FROM edges WHERE dst=?1 AND rel='tag'", [&t.id], |r| r.get::<_, i64>(0))?;
                rows.push(Row::Tag { id: t.id.clone(), name: t.label(), count: c as usize });
            }
        }
        Self::section(&mut rows, "Nodes", Token::Text, nodes);
        // (The is:deleted tip shows with no results only.)
        Ok(rows)
    }

    fn rows_review(&self) -> Result<Vec<Row>> {
        let by = self.log_actor.as_deref().map(|a| a.trim_start_matches("agent:"));
        let items = thc_core::review::queue(&self.vault.store, by, None)?;
        if items.is_empty() {
            let sep = self.theme.glyphs().sep;
            return Ok(vec![Row::Empty { l1: "Nothing to review.".into(), l2: format!("r all changes  {sep}  @ pick an actor") }]);
        }
        let mut rows = Vec::new();
        for it in items {
            let entries = self.vault.store.history_where("tx = ?1 ORDER BY okey", &[&it.tx])?;
            if entries.is_empty() {
                continue;
            }
            let text = review_summary(&self.vault.store, &it, self.theme.glyphs(), self.today);
            rows.push(Row::Tx { tx: it.tx.clone(), entries });
            rows.push(Row::TxDetail { tx: it.tx, text });
        }
        Ok(rows)
    }

    fn rows_log(&self) -> Result<Vec<Row>> {
        if self.review_lane && self.log_node.is_none() {
            return self.rows_review();
        }
        let since = chrono::Utc::now().timestamp_millis() - 24 * 3600 * 1000;
        let h: Vec<HistoryEntry> = match &self.log_node {
            Some(id) => self.vault.store.history(id, 300)?,
            None => self.vault.store.history_where("ms >= ?1 ORDER BY okey DESC LIMIT 400", &[&since])?,
        };
        let h: Vec<HistoryEntry> = match &self.log_actor {
            Some(a) => h.into_iter().filter(|e| &e.actor == a).collect(),
            None => h,
        };
        let mut txs: Vec<(String, Vec<HistoryEntry>)> = Vec::new();
        for e in h {
            match txs.last_mut() {
                Some((tx, v)) if *tx == e.tx => v.push(e),
                _ => txs.push((e.tx.clone(), vec![e])),
            }
        }
        if txs.is_empty() {
            return Ok(vec![match &self.log_actor {
                Some(a) => Row::Empty { l1: format!("{} hasn't changed anything in the last 24h.", a.trim_start_matches("agent:")), l2: "@ everyone".into() },
                None => Row::Empty { l1: "No changes in the last 24h.".into(), l2: "@ pick an actor  ·  thc log --since 7d".into() },
            }]);
        }
        let mut rows = Vec::new();
        let g = self.theme.glyphs();
        for (tx, entries) in txs {
            let mut detail = tx_detail(&self.vault.store, &entries, g);
            // In a node's log, the second line is that node's field changes (as in the lane).
            if let Some(node) = &self.log_node {
                if let Ok(it) = thc_core::review::item(&self.vault.store, &tx, 0) {
                    if let Some(c) = it.changes.iter().find(|c| &c.node == node) {
                        detail = change_words(&self.vault.store, c, g, self.today);
                    }
                }
            }
            let verdict: Option<(String, String, i64)> = self
                .vault
                .store
                .conn
                .query_row("SELECT verdict, actor, ms FROM reviews WHERE tx=?1", [&tx], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .ok();
            if let Some(("reverted", who, ms)) = verdict.as_ref().map(|(v, a, m)| (v.as_str(), a.trim_start_matches("agent:").to_string(), *m)) {
                let mark = if g.arrow == "→" { "↶" } else { "<-" };
                let r = format!("{mark} reverted by {who} {} {}", g.sep, crate::clock_snapshot::when_words(ms, self.today));
                detail = if detail.is_empty() { r } else { format!("{detail} {} {r}", g.sep) };
            }
            rows.push(Row::Tx { tx: tx.clone(), entries });
            if !detail.is_empty() {
                rows.push(Row::TxDetail { tx, text: detail });
            }
        }
        Ok(rows)
    }

    // ---- navigation ---------------------------------------------------------------------------

    /// The Pages list, always (navigation.md §1): an open page closes (saved), and the cursor
    /// rests on the page last opened, so Enter reopens it.
    /// The vault picker: every registered vault, the current one marked, home last.
    pub fn open_vault_picker(&mut self) {
        let reg = thc_core::registry::Registry::load();
        let here = self.vault.origin.as_ref().map_or(self.vault.paths.vault.clone(), |o| o.vault.clone());
        let cur = reg.by_path(&here).map(|e| e.name.clone());
        let mut rows: Vec<VaultRow> = reg
            .vaults
            .iter()
            .map(|e| {
                let counts = if cur.as_deref() == Some(e.name.as_str()) {
                    let open = self.open_count;
                    let inbox = self.inbox_count;
                    Some((open, inbox))
                } else {
                    vault_counts(&e.path)
                };
                VaultRow {
                    name: e.name.clone(),
                    path: e.path.clone(),
                    open: counts.map(|c| c.0),
                    inbox: counts.map(|c| c.1),
                    sync: thc_core::registry::sync_of(&e.path),
                    current: cur.as_deref() == Some(e.name.as_str()),
                    home: reg.home.as_deref() == Some(e.name.as_str()),
                    accent: thc_core::settings::load(Some(&e.path)).accent().to_string(),
                }
            })
            .collect();
        rows.sort_by_key(|r| r.home);
        let sel = rows.iter().position(|r| r.current).unwrap_or(0);
        self.overlay = Some(Overlay::Vaults { rows, sel, naming: None });
    }

    pub fn show_pages(&mut self) {
        self.doc_origin = None;
        let last = self.page_open.take().or_else(|| self.last_page.clone());
        self.last_page = last.clone();
        self.set_view(View::Pages);
        if let Some(id) = last {
            if self.rows.iter().any(|r| r.key().as_deref() == Some(id.as_str())) {
                self.selected = Some(id);
                let _ = self.reload();
            }
        }
    }

    pub fn set_view(&mut self, v: View) {
        self.ui.enter_view(v);
        let _ = self.reload();
    }

    /// Tasks: `s` cycles the sort term of the active query.
    pub fn cycle_sort(&mut self) {
        const SORTS: [&str; 4] = ["sort:due", "sort:priority", "sort:updated-", "sort:title"];
        let cur = SORTS.iter().position(|s| self.tasks_filter.contains(s));
        let next = SORTS[cur.map(|i| (i + 1) % SORTS.len()).unwrap_or(0)];
        let base: Vec<&str> = self.tasks_filter.split_whitespace().filter(|t| !t.starts_with("sort:")).collect();
        self.tasks_filter = format!("{} {next}", base.join(" ")).trim().to_string();
        self.selected = None;
        let _ = self.reload();
        self.info(format!("sorted by {}", next.trim_start_matches("sort:").trim_end_matches('-')));
    }

    pub fn cycle_log_actor(&mut self) {
        let actors: Vec<String> = {
            let mut st = match self.vault.store.conn.prepare("SELECT DISTINCT actor FROM events ORDER BY actor LIKE 'agent:%', actor") {
                Ok(st) => st,
                Err(_) => return,
            };
            st.query_map([], |r| r.get(0)).map(|it| it.flatten().collect()).unwrap_or_default()
        };
        self.log_actor = match &self.log_actor {
            None => actors.first().cloned(),
            Some(cur) => actors.iter().position(|a| a == cur).and_then(|i| actors.get(i + 1)).cloned(),
        };
        self.selected = None;
        let _ = self.reload();
    }

    pub fn open_compare(&mut self) {
        if self.in_panel.is_some() {
            if let Some(id) = self.selected.clone() {
                self.panel_defer.push(crate::sidebar_app::Deferred::Compare(id));
            }
            return;
        }
        let details = self.vault.store.conflict_details(None).unwrap_or_default();
        let target = self.selected_id().and_then(|id| details.iter().find(|d| d.node == id).cloned());
        match target.or_else(|| details.first().cloned()) {
            Some(detail) => self.overlay = Some(Overlay::Compare { detail }),
            None => self.info("no conflicts"),
        }
    }

    /// Dismiss a rejected move: a move to where the node already is closes the notice.
    pub fn dismiss_move(&mut self, node: &str) {
        let Some(n) = self.vault.store.node(node).ok().flatten() else { return };
        let id = n.id.clone();
        let (parent, ord) = (n.parent.clone(), n.ord.clone());
        let r = self.write(|b| {
            b.ops.push(thc_core::event::Op::NodeMove { id: id.clone(), parent, order: ord });
            Ok(())
        });
        let g = self.theme.glyphs();
        let place = n.parent.as_deref().map(|p| self.vault.store.short(p)).unwrap_or_else(|| "inbox".into());
        match r {
            Ok(_) => self.toast_parts(
                ToastKind::Confirm,
                vec![("dismissed ".into(), Token::Text), (format!("{} {} it stays under {place}", self.vault.store.short(node), g.sep), Token::Muted)],
            ),
            Err(e) => self.error(friendly_error(&e)),
        }
        self.open_next_conflict();
    }

    /// Keep a line that was moved here (its parent was deleted elsewhere): the move becomes real,
    /// so it stays where it shows and the flag clears (daemon.md §4.0a).
    pub fn keep_rehomed(&mut self, d: &thc_core::model::ConflictDetail) {
        let Some(n) = self.vault.store.node(&d.node).ok().flatten() else { return };
        let under = d.current.as_ref().map(|c| c.text.clone()).filter(|t| !t.is_empty());
        let (id, ord) = (n.id.clone(), n.ord.clone());
        let parent = under.clone();
        let r = self.write(|b| {
            b.ops.push(thc_core::event::Op::NodeMove { id: id.clone(), parent, order: ord });
            Ok(())
        });
        let g = self.theme.glyphs();
        let place = under.as_deref().map(|u| self.node_label(u)).unwrap_or_else(|| "the inbox".into());
        match r {
            Ok(_) => self.toast_parts(ToastKind::Confirm, vec![("kept ".into(), Token::Text), (format!("{} under {place} {} flag cleared", self.vault.store.short(&d.node), g.sep), Token::Muted)]),
            Err(e) => self.error(friendly_error(&e)),
        }
        self.patch_after_resolve();
        self.open_next_conflict();
    }

    /// Delete a line that was moved here, as the device that deleted its parent meant.
    pub fn delete_rehomed(&mut self, d: &thc_core::model::ConflictDetail) {
        let id = d.node.clone();
        let dev = d.other.as_ref().map(|o| if o.dev == self.vault.device { "this device".to_string() } else { o.dev.clone() }).unwrap_or_default();
        let r = self.write(|b| b.delete(&id).map(|_| ()));
        let g = self.theme.glyphs();
        let short = self.vault.store.short(&id);
        match r {
            Ok(_) => self.toast_parts(ToastKind::Confirm, vec![(format!("deleted {short} "), Token::Text), (format!("(as {dev} meant) {} u undo", g.sep), Token::Muted)]),
            Err(e) => self.error(friendly_error(&e)),
        }
        self.patch_after_resolve();
        self.open_next_conflict();
    }

    /// After a resolution written here: the open document takes it up.
    fn patch_after_resolve(&mut self) {
        if self.doc.is_some() {
            self.patch_doc();
        }
    }

    fn open_next_conflict(&mut self) {
        if let Some(detail) = self.vault.store.conflict_details(None).unwrap_or_default().into_iter().next() {
            self.overlay = Some(Overlay::Compare { detail });
        }
    }

    /// Resolve a text conflict: `current`, `other` or `both` (daemon.md §4.2).
    /// Which version is yours (tui-editor.md §9): the one the open document shows, else the
    /// human's on this device, else the engine's `current`.
    pub fn yours_is_current(&self, d: &thc_core::model::ConflictDetail) -> bool {
        let s = &self.vault.store;
        let text = |v: &Option<thc_core::model::ConflictVersion>| v.as_ref().map(|v| s.render_text(&v.text));
        if let Some(line) = self.doc.as_ref().and_then(|doc| doc.blocks().iter().find(|l| l.id == d.node)) {
            if text(&d.current).as_deref() == Some(line.text.as_str()) {
                return true;
            }
            if text(&d.other).as_deref() == Some(line.text.as_str()) {
                return false;
            }
        }
        let mine = |v: &Option<thc_core::model::ConflictVersion>| v.as_ref().is_some_and(|v| v.actor.starts_with("human") && v.dev == self.vault.device);
        !(mine(&d.other) && !mine(&d.current))
    }

    /// Who the other version is from: `claude`, or a device.
    pub fn theirs_name(&self, d: &thc_core::model::ConflictDetail) -> String {
        let v = if self.yours_is_current(d) { &d.other } else { &d.current };
        v.as_ref()
            .map(|v| if v.actor.starts_with("human") { if v.dev == self.vault.device { "this device".into() } else { v.dev.clone() } } else { v.actor.trim_start_matches("agent:").to_string() })
            .unwrap_or_else(|| "theirs".into())
    }

    /// 1 keep yours / 2 keep theirs / b both (yours stays, theirs goes right below).
    pub fn resolve_conflict(&mut self, node: &str, choice: &str) {
        let Some(detail) = self.vault.store.conflict_details(Some(node)).unwrap_or_default().into_iter().find(|d| d.kind == "text") else {
            return self.info("no open text conflict here");
        };
        let yours = if self.yours_is_current(&detail) { "current" } else { "other" };
        let theirs = if yours == "current" { "other" } else { "current" };
        let who = self.theirs_name(&detail);
        // Lists and the Log speak current / other; documents speak yours / theirs.
        let keep = match choice {
            "yours" => yours,
            "theirs" => theirs,
            "current" | "other" => choice,
            _ => "both",
        };
        let yours = if matches!(choice, "current" | "other") { "current" } else { yours };
        let id = node.to_string();
        let r = self.write(|b| b.resolve_text_conflict(&id, keep, yours).map(|_| ()));
        let what = match choice {
            "yours" => "kept yours".to_string(),
            "theirs" => format!("kept {who}'s"),
            "current" => "kept current".to_string(),
            "other" => "kept other".to_string(),
            _ => "kept both".to_string(),
        };
        match r {
            Ok(_) => self.toast_parts(ToastKind::Confirm, vec![(what, Token::Text)]),
            Err(e) => self.error(friendly_error(&e)),
        }
        self.open_next_conflict();
    }

    pub fn move_cursor(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let mut i = self.cursor as isize;
        let step = delta.signum();
        for _ in 0..delta.abs() {
            let mut j = i + step;
            while j >= 0 && (j as usize) < self.rows.len() && !self.rows[j as usize].selectable() {
                j += step;
            }
            if j < 0 || j as usize >= self.rows.len() {
                break;
            }
            i = j;
        }
        self.cursor = i as usize;
        self.selected = self.rows.get(self.cursor).and_then(|r| r.key());
    }

    /// Put the cursor on row `i` itself (a click, a scrollbar drag): absolute, where
    /// `move_cursor` counts selectable steps and overshot by every heading or Log detail line
    /// in between. A row that can't be selected gives the nearest one before it,
    /// else after it.
    pub fn select_row(&mut self, i: usize) {
        let n = self.rows.len();
        if n == 0 {
            return;
        }
        let i = i.min(n - 1);
        let Some(j) = (0..=i).rev().find(|&j| self.rows[j].selectable()).or_else(|| (i..n).find(|&j| self.rows[j].selectable())) else { return };
        self.cursor = j;
        self.selected = self.rows[j].key();
    }

    /// Key of the next selectable row (used to advance after triage actions).
    fn next_key(&self) -> Option<String> {
        (self.cursor + 1..self.rows.len()).find(|&i| self.rows[i].selectable()).and_then(|i| self.rows[i].key())
    }

    fn advance_if_triage(&mut self) {
        if self.view == View::Inbox {
            self.selected = self.next_key();
        }
    }

    // ---- writes ---------------------------------------------------------------------------------

    /// A write on the selected row: in the row's own vault when it came from another one
    /// (vaults.md §3.5), else here. Capture and new pages use `write_here`.
    pub fn write(&mut self, f: impl FnOnce(&mut TxBuilder) -> Result<()>) -> Result<Vec<Event>> {
        let target = self.route_next.take().or_else(|| self.selected_node().and_then(|_| self.row_from(self.cursor)));
        self.write_in(target, f)
    }

    /// A write in this (the header's) vault, whatever row is selected.
    pub fn write_here(&mut self, f: impl FnOnce(&mut TxBuilder) -> Result<()>) -> Result<Vec<Event>> {
        self.route_next = None;
        self.write_in(None, f)
    }

    fn write_in(&mut self, target: Option<usize>, f: impl FnOnce(&mut TxBuilder) -> Result<()>) -> Result<Vec<Event>> {
        let today = self.today;
        let target = target.filter(|i| *i < self.others.len());
        if let Some(i) = target {
            std::mem::swap(&mut self.vault, &mut self.others[i].vault);
        }
        let r = self.vault.transact(|s| {
            let mut b = TxBuilder::new(s, today);
            f(&mut b)?;
            Ok((b.finish(), ()))
        });
        if let Some(i) = target {
            std::mem::swap(&mut self.vault, &mut self.others[i].vault);
        }
        let (events, ()) = r?;
        self.last_write_vault = target;
        self.log_sizes = self.vault.log.files()?;
        self.reload_stable()?;
        Ok(events)
    }

    /// A write from outside App (the view editor), as the TUI's own writes go.
    pub fn write_in_public(&mut self, f: impl FnOnce(&mut TxBuilder) -> Result<()>) -> Result<Vec<Event>> {
        self.write_in(None, f)
    }

    pub fn capture_targets(&self, inbox: bool) -> Vec<CaptureTarget> {
        let today = CaptureTarget::Journal(self.today);
        let mut v = vec![];
        if inbox || self.view == View::Inbox {
            v.push(CaptureTarget::Inbox);
        }
        match self.view {
            View::Journal if self.journal_date != self.today => v.push(CaptureTarget::Journal(self.journal_date)),
            View::Pages => {
                if let Some(p) = &self.page_open {
                    if let Some(n) = self.selected_node().filter(|n| n.parent.is_some()) {
                        let label = format!("under {}", crate::ui::truncate_str(&self.vault.store.render_text(&n.label()), 24, "…"));
                        v.push(CaptureTarget::Under(n.id.clone(), label));
                    }
                    let label = format!("{} {}", self.theme.glyphs().page, self.node_label(p));
                    v.push(CaptureTarget::Under(p.clone(), label));
                }
            }
            _ => {}
        }
        // The vault's capture target (`[capture] target`, vaults.md §9) comes before today.
        if v.is_empty() {
            if let Some((id, title)) = thc_core::settings::capture_target(&thc_core::settings::current(), &self.vault.store) {
                v.push(CaptureTarget::Under(id, format!("{} {title}", self.theme.glyphs().page)));
            }
        }
        for t in [today, CaptureTarget::Inbox] {
            if !v.contains(&t) {
                v.push(t);
            }
        }
        v
    }

    pub fn capture(&mut self, text: &str, target: &CaptureTarget) -> Result<()> {
        // Enter always saves; tokens that don't parse stay as literal text. The context's
        // default tags are added (typing -#work skips one).
        let text = match &self.context_active {
            Some(a) => a.capture_defaults().apply(text).0,
            None => text.to_string(),
        };
        let (cap, _bad) = capture::parse_lenient(&text, self.today)?;
        let target = target.clone();
        let mut new_id = String::new();
        let out = &mut new_id;
        self.write_here(|b| {
            let parent = match &target {
                CaptureTarget::Inbox => None,
                CaptureTarget::Under(p, _) => Some(p.clone()),
                CaptureTarget::Journal(d) => Some(b.journal(*d)?),
            };
            *out = b.create_from_capture(parent, &cap, None)?;
            Ok(())
        })?;
        self.selected = Some(new_id.clone());
        self.reload()?;
        let label = self.node_label(&new_id);
        self.confirm("added", Some(&new_id), label);
        Ok(())
    }

    pub fn toggle_done(&mut self, reopen_only: bool) {
        let Some(n) = self.selected_node().cloned() else { return };
        let label = self.vault.store.render_text(&n.label());
        if matches!(n.status.as_deref(), Some("done" | "cancelled")) || reopen_only {
            if n.status.is_none() {
                return;
            }
            let r = self.write(|b| b.set_props(&n.id, &[("status".into(), "todo".into()), ("done_at".into(), String::new())]));
            self.report(r, "reopened", Some(&n.id), label);
            return;
        }
        if n.status.is_none() {
            self.info("not a task · t makes it one");
            return;
        }
        let mut next = None;
        let out = &mut next;
        self.advance_if_triage();
        let r = self.write(|b| {
            *out = b.complete(&n.id)?;
            Ok(())
        });
        let g = self.theme.glyphs();
        let rest = match next.and_then(|nd| nd.scheduled.or(nd.due)) {
            Some(d) => {
                let pretty = dates::DateVal::from_stored(&d).map(|v| v.date().format("%a %b %-d").to_string()).unwrap_or(d);
                format!("{label} {} {} next {pretty}", g.sep, g.repeat)
            }
            None => format!("{label} {} u undo", g.sep),
        };
        self.report(r, "done", Some(&n.id), rest);
    }

    pub fn toggle_task(&mut self) {
        let Some(n) = self.selected_node().cloned() else { return };
        let v = if n.status.is_some() { "" } else { "todo" };
        let label = self.vault.store.render_text(&n.label());
        self.advance_if_triage();
        let r = self.write(|b| b.set_props(&n.id, &[("status".into(), v.into())]));
        self.report(r, if v.is_empty() { "untasked" } else { "task" }, Some(&n.id), label);
    }

    pub fn set_status(&mut self, id: &str, key: char) {
        let v = match key {
            ' ' => "todo",
            '/' => "doing",
            'w' => "waiting",
            'x' => "done",
            '-' => "cancelled",
            _ => {
                self.info("status: space todo · / doing · w waiting · x done · - cancelled");
                return;
            }
        };
        let id = id.to_string();
        let label = self.node_label(&id);
        let r = self.write(|b| b.set_props(&id, &[("status".into(), v.into())]));
        self.report(r, v, Some(&id), label);
    }

    pub fn set_priority(&mut self, id: &str, key: char) {
        let v = match key {
            'h' | '1' => "high",
            'm' | '2' => "med",
            'l' | '3' => "low",
            '-' | '0' | ' ' => "",
            _ => {
                self.info("priority: h high · m med · l low · - clear");
                return;
            }
        };
        let id = id.to_string();
        let label = self.node_label(&id);
        let r = self.write(|b| b.set_props(&id, &[("priority".into(), v.into())]));
        self.report(r, "priority", Some(&id), if v.is_empty() { format!("{label} · cleared") } else { format!("{label} · !{v}") });
    }

    pub fn submit_prompt(&mut self, kind: PromptKind, value: String) {
        let g = self.theme.glyphs();
        match kind {
            PromptKind::Due(_) | PromptKind::Sched(_) if !value.trim().is_empty() && dates::parse(&value, self.today).is_err() => {
                self.error(format!("can't read \"{}\" as a date {} try fri, +3d or 2026-10-09", value.trim(), g.sep));
            }
            PromptKind::ViewCopy(from) => {
                // `c` in a recipe (view-explain.md §3): the same sections and scope, a new name.
                let Some(v) = self.recipe(&from).map(|r| r.view) else { return };
                let name = value.trim().trim_start_matches('@').to_string();
                if name.is_empty() {
                    return;
                }
                let today = self.today;
                let scope = v.scope.clone();
                let r = self.write_in(None, |b| thc_core::views::set_sectioned(b, &name, Some(scope.as_deref()), &v.sections, today).map(|_| ()));
                match r {
                    Ok(_) => self.info(format!("@{name} saved · thc q @{name}")),
                    Err(e) => self.error(format!("{e:#}")),
                }
            }
            PromptKind::Due(id) => {
                let label = self.node_label(&id);
                self.advance_if_triage();
                let r = self.write(|b| b.set_props(&id, &[("due".into(), value.clone())]));
                let rest = if value.trim().is_empty() { format!("{label} {} due cleared", g.sep) } else { format!("{label} {} due {}", g.sep, self.pretty_date(&value)) };
                self.report(r, "due", Some(&id), rest);
            }
            PromptKind::Sched(id) => {
                let label = self.node_label(&id);
                let r = self.write(|b| b.set_props(&id, &[("scheduled".into(), value.clone())]));
                let rest = if value.trim().is_empty() { format!("{label} {} unscheduled", g.sep) } else { format!("{label} {} {}", g.sep, self.pretty_date(&value)) };
                self.report(r, "sched", Some(&id), rest);
            }
            PromptKind::Text(id) => {
                if value.trim().is_empty() {
                    self.info("text can't be empty · D deletes");
                    return;
                }
                let r = self.write(|b| b.set_text(&id, &value));
                self.report(r, "edited", Some(&id), value.clone());
            }
            PromptKind::Tags(id) => {
                let (mut add, mut remove) = (vec![], vec![]);
                for t in value.split_whitespace() {
                    match t.strip_prefix('-') {
                        Some(r) => remove.push(r.to_string()),
                        None => add.push(t.trim_start_matches('+').trim_start_matches('#').to_string()),
                    }
                }
                // The prompt edits the full set: tags no longer listed are removed.
                let current = self.vault.store.tags_of(&id).unwrap_or_default();
                for c in current {
                    if !add.iter().any(|a| a.eq_ignore_ascii_case(&c)) && !remove.contains(&c) {
                        remove.push(c);
                    }
                }
                let label = self.node_label(&id);
                let r = self.write(|b| b.add_tags(&id, &add, &remove));
                self.report(r, "tagged", Some(&id), label);
            }
            PromptKind::Filter => {
                self.tasks_filter = if value.trim().is_empty() { "status:open sort:due".into() } else { value };
                self.selected = None;
                let _ = self.reload();
                if self.tasks_error.is_some() {
                    // Stay in the filter so it can be fixed (Tab applies the suggestion).
                    let f = self.tasks_filter.clone();
                    self.prompt = Some((PromptKind::Filter, LineInput::with(&f)));
                    self.input_untouched = false;
                }
            }
            PromptKind::Search => {
                self.search_terms = value;
                self.selected = None;
                if self.view == View::Search {
                    let _ = self.reload();
                } else {
                    self.set_view(View::Search);
                    self.prompt = None;
                }
            }
            PromptKind::PagesFilter => {
                let sel = self.selected.clone();
                self.pages_filter = value;
                let _ = self.reload();
                if let Some(title) = sel.as_deref().and_then(|k| k.strip_prefix("newpage:")).map(str::to_string) {
                    // `+ new page` chosen with ↓: create it and start writing (§10.6). The page
                    // opens as a document, ready to type: no row editor on top of it, which took
                    // the keys out of sight until Esc (a bug: typing not shown in a new page).
                    self.submit_prompt(PromptKind::NewPage, title);
                } else if self.selected_node().is_some() {
                    // Enter opens the highlighted page.
                    self.open_selected();
                } else if self.pages_filter.trim().chars().count() >= 2 {
                    // Nothing chosen: Enter never creates a page by itself.
                    let t = self.pages_filter.trim().to_string();
                    self.prompt = Some((PromptKind::PagesFilter, LineInput::with(&t)));
                    self.info(format!("↓ then Enter creates {} {t}", g.page));
                } else if !self.pages_filter.trim().is_empty() {
                    let t = self.pages_filter.trim().to_string();
                    self.prompt = Some((PromptKind::PagesFilter, LineInput::with(&t)));
                    self.info("type a longer name to create a page");
                }
            }
            PromptKind::GoDate => match dates::parse(&value, self.today) {
                Ok(d) => {
                    self.journal_date = d.date();
                    self.set_view(View::Journal);
                }
                Err(_) => self.error(format!("can't read \"{}\" as a date {} try fri, +3d or 2026-10-09", value.trim(), g.sep)),
            },
            PromptKind::NewPage => {
                let mut pid = String::new();
                let out = &mut pid;
                let title = value.trim().to_string();
                let r = self.write(|b| {
                    *out = b.create_page(&title, &[])?;
                    Ok(())
                });
                if r.is_ok() {
                    self.pages_filter.clear();
                    self.page_open = Some(pid.clone());
                    self.set_view(View::Pages);
                }
                let id = if pid.is_empty() { None } else { Some(pid.as_str()) };
                let rest = format!("{} {title}", g.page);
                match r {
                    Ok(_) => self.confirm("page", id, rest),
                    Err(e) => self.error(friendly_error(&e)),
                }
            }
            PromptKind::Snooze(alert) => {
                if let Err(e) = dates::reject_bare_m(&value) {
                    return self.error(friendly_error(&e));
                }
                let until = match dates::parse_duration(&value) {
                    Ok(d) => (dates::now_local() + d).format("%Y-%m-%dT%H:%M").to_string(),
                    Err(_) => match dates::parse(&value, self.today) {
                        Ok(d) => d.fmt(),
                        Err(_) => return self.error(format!("can't read \"{}\" · try 1h, 30min or tomorrow 9am", value.trim())),
                    },
                };
                let u = until.clone();
                let r = self.write(|b| {
                    b.ops.push(thc_core::event::Op::AlertSnooze { id: alert.clone(), until: u });
                    Ok(())
                });
                self.report(r, "snoozed", None, format!("until {}", until.replace('T', " ")));
            }
        }
    }

    fn pretty_date(&self, raw: &str) -> String {
        dates::parse(raw, self.today)
            .map(|d| {
                let t = d.time().map(|t| format!(" {}", t.format("%H:%M"))).unwrap_or_default();
                format!("{}{t}", d.date().format("%a %b %-d"))
            })
            .unwrap_or_else(|_| raw.to_string())
    }

    pub fn delete_selected(&mut self) {
        let Some(id) = self.selected_id() else { return };
        let label = self.node_label(&id);
        let kids = self.vault.store.descendants(&id).map(|d| d.len()).unwrap_or(0);
        self.selected = self.next_key();
        let r = self.write(|b| b.delete(&id).map(|_| ()));
        let sep = self.theme.glyphs().sep;
        let rest = if kids > 0 { format!("{label} and {kids} children {sep} u undo") } else { format!("{label} {sep} u undo") };
        self.report(r, "deleted", Some(&id), rest);
    }

    pub fn skip_selected(&mut self) {
        let Some(id) = self.selected_id() else { return };
        let label = self.node_label(&id);
        let mut next = None;
        let out = &mut next;
        let r = self.write(|b| {
            *out = Some(b.skip(&id)?);
            Ok(())
        });
        let when = next.and_then(|n| n.scheduled.or(n.due)).map(|d| self.pretty_date(&d)).unwrap_or_default();
        let sep = self.theme.glyphs().sep;
        self.report(r, "skipped", Some(&id), format!("{label} {sep} next {when}"));
    }

    pub fn alert_of_selected(&mut self) -> Option<String> {
        let id = self.selected_id()?;
        let alerts = self.vault.store.alerts_of(&id).unwrap_or_default();
        match alerts.first() {
            Some(a) => Some(a.id.clone()),
            None => {
                self.info("no alert on this node · thc alert add <id> --before 1d");
                None
            }
        }
    }

    pub fn ack_selected(&mut self) {
        let Some(a) = self.alert_of_selected() else { return };
        let r = self.write(|b| {
            b.ops.push(thc_core::event::Op::AlertAck { id: a.clone() });
            Ok(())
        });
        self.report(r, "acked", None, "alert acknowledged".into());
    }

    pub fn undo(&mut self) {
        // Priority: the selected tx in Log, the agent tx a toast is showing, else the latest tx.
        let from_log = if self.view == View::Log {
            self.rows.get(self.cursor).and_then(|r| match r {
                Row::Tx { tx, .. } => Some(tx.clone()),
                _ => None,
            })
        } else {
            None
        };
        let from_toast = self.toast.as_ref().filter(|t| t.kind == ToastKind::Agent && t.alive_at(self.ui.now_ms)).and(self.last_agent_tx.clone());
        // The last write went to another vault (a row from it): undo there (vaults.md §3.5).
        if from_log.is_none() && from_toast.is_none() {
            if let Some(i) = self.last_write_vault.filter(|i| *i < self.others.len()) {
                std::mem::swap(&mut self.vault, &mut self.others[i].vault);
                let tx: Option<String> = self.vault.store.conn.query_row("SELECT tx FROM events ORDER BY okey DESC LIMIT 1", [], |r| r.get(0)).ok();
                let inverse = tx.as_deref().and_then(|t| thc_core::review::undo_ops(&self.vault.store, t).ok()).unwrap_or_default();
                std::mem::swap(&mut self.vault, &mut self.others[i].vault);
                if inverse.is_empty() {
                    return self.info("that change can't be undone");
                }
                let n = inverse.iter().filter(|o| !o.is_review()).count();
                let r = self.write_in(Some(i), |b| {
                    b.ops.extend(inverse);
                    Ok(())
                });
                return self.report(r, "undone", None, format!("{n} change(s)"));
            }
        }
        let tx = from_log
            .or(from_toast)
            .or_else(|| self.vault.store.conn.query_row("SELECT tx FROM events ORDER BY okey DESC LIMIT 1", [], |r| r.get(0)).ok());
        let Some(tx) = tx else {
            self.info("nothing to undo");
            return;
        };
        let actor: String = self.vault.store.conn.query_row("SELECT actor FROM events WHERE tx=?1 LIMIT 1", [&tx], |r| r.get(0)).unwrap_or_default();
        let inverse = match thc_core::review::undo_ops(&self.vault.store, &tx) {
            Ok(i) if !i.is_empty() => i,
            _ => return self.info("that change can't be undone"),
        };
        let n = inverse.iter().filter(|o| !o.is_review()).count();
        let r = self.write(|b| {
            b.ops.extend(inverse);
            Ok(())
        });
        let short = tx[tx.len().saturating_sub(6)..].to_string();
        let who = actor.trim_start_matches("agent:").to_string();
        let sep = self.theme.glyphs().sep;
        self.report(r, "undone", None, format!("tx {short} {sep} {who} {sep} {n} change(s) {sep} U redo"));
    }

    // ---- review lane (agents.md §1.6) and rewind (§6.3) -----------------------------------------

    pub fn last_agent_tx(&self) -> Option<String> {
        self.last_agent_tx.clone()
    }

    pub fn toggle_review_lane(&mut self) {
        self.review_lane = !self.review_lane;
        self.log_node = None;
        self.cursor = 0;
        self.scroll = 0;
        self.selected = None;
        let _ = self.reload();
    }

    /// Open the review lane with the cursor on `tx` (from the agent toast's `L`).
    pub fn review_tx(&mut self, tx: &str) {
        self.view = View::Log;
        self.log_node = None;
        self.review_lane = true;
        self.selected = None;
        let _ = self.reload();
        if !self.rows.iter().any(|r| matches!(r, Row::Tx { tx: t, .. } if t == tx)) {
            // Already reviewed: show it among all changes instead.
            self.review_lane = false;
            let _ = self.reload();
        }
        self.cursor = self.rows.iter().position(|r| matches!(r, Row::Tx { tx: t, .. } if t == tx)).unwrap_or(0);
    }

    fn selected_tx(&self) -> Option<String> {
        match self.rows.get(self.cursor) {
            Some(Row::Tx { tx, .. }) | Some(Row::TxDetail { tx, .. }) => Some(tx.clone()),
            _ => None,
        }
    }

    fn after_review(&mut self) {
        let cur = self.cursor;
        let _ = self.reload();
        let txs: Vec<usize> = self.rows.iter().enumerate().filter(|(_, r)| matches!(r, Row::Tx { .. })).map(|(i, _)| i).collect();
        self.cursor = txs.iter().copied().find(|&i| i >= cur).or(txs.last().copied()).unwrap_or(0);
    }

    fn accept_txs(&mut self, txs: Vec<String>) -> Result<Vec<Event>> {
        if self.vault.actor.kind != "human" {
            let name = self.vault.actor.name.clone().unwrap_or_default();
            anyhow::bail!(thc_core::error::invalid(format!("only a person can review agent changes · this session is {name}")));
        }
        self.write(|b| {
            b.ops.push(thc_core::event::Op::TxReview { txs, verdict: "accepted".into() });
            Ok(())
        })
    }

    pub fn review_accept(&mut self) {
        let Some(tx) = self.selected_tx() else { return };
        let short = thc_core::review::tx_short(&tx);
        let r = self.accept_txs(vec![tx]);
        self.report(r, "accepted", None, short);
        self.after_review();
    }

    pub fn ask_accept_all(&mut self) {
        let n = self.rows.iter().filter(|r| matches!(r, Row::Tx { .. })).count();
        if n == 0 {
            return self.info("nothing to review");
        }
        self.awaiting = Some(('A', String::new()));
        self.info(format!("accept {n} change{}? y/n", if n == 1 { "" } else { "s" }));
    }

    pub fn review_accept_all(&mut self) {
        let txs: Vec<String> = self.rows.iter().filter_map(|r| match r {
            Row::Tx { tx, .. } => Some(tx.clone()),
            _ => None,
        }).collect();
        let n = txs.len();
        let r = self.accept_txs(txs);
        self.report(r, "accepted", None, format!("{n} change{}", if n == 1 { "" } else { "s" }));
        self.after_review();
    }

    pub fn review_revert(&mut self) {
        let Some(tx) = self.selected_tx() else { return };
        let short = thc_core::review::tx_short(&tx);
        let plan = match thc_core::review::plan_revert(&self.vault.store, std::slice::from_ref(&tx), false) {
            Ok(p) => p,
            Err(e) => return self.error(friendly_error(&e)),
        };
        let kept: Vec<String> = plan
            .skipped
            .iter()
            .filter(|s| s.reason == "changed_later")
            .map(|s| format!("{} {}", self.vault.store.short(&s.node), if s.field == "parent" { "place" } else { s.field.as_str() }))
            .collect();
        let ops = plan.ops;
        let r = self.write(|b| {
            b.ops.extend(ops);
            Ok(())
        });
        let rest = if kept.is_empty() { short } else { format!("{short}, kept your later {}", kept.join(", ")) };
        self.report(r, "reverted", None, rest);
        self.after_review();
    }

    /// `R` on a node log row: the bar asks first (§6.3).
    pub fn ask_rewind(&mut self) {
        let (Some(id), Some(tx)) = (self.log_node.clone(), self.selected_tx()) else {
            return self.info("R rewinds a node: open its log with L first");
        };
        let Ok(Some(cut)) = thc_core::review::Cut::tx(&self.vault.store, &tx, false) else { return };
        let d = match thc_core::review::node_diff(&self.vault.store, &id, &cut) {
            Ok(d) => d,
            Err(e) => return self.error(friendly_error(&e)),
        };
        let n = d.fields.iter().filter(|f| f.key != "done_at").count() + usize::from(matches!(d.deleted, Some((true, _))));
        let short = self.vault.store.short(&id);
        let at = crate::clock_snapshot::when_words(cut.ms, self.today);
        if n == 0 {
            return self.info(format!("{short} already matches {at}"));
        }
        let txs = thc_core::review::tx_short(&tx);
        self.awaiting = Some(('W', tx));
        self.info(format!("rewind {short} to after {txs} ({at})? {n} field{}  y/n", if n == 1 { "" } else { "s" }));
    }

    pub fn rewind_to(&mut self, tx: &str) {
        let Some(id) = self.log_node.clone() else { return };
        let Ok(Some(cut)) = thc_core::review::Cut::tx(&self.vault.store, tx, false) else { return };
        let ops = thc_core::review::node_diff(&self.vault.store, &id, &cut).and_then(|d| {
            let n = d.fields.iter().filter(|f| f.key != "done_at").count();
            thc_core::review::rewind_ops(&self.vault.store, &d).map(|o| (o, n))
        });
        let (ops, n) = match ops {
            Ok(x) => x,
            Err(e) => return self.error(friendly_error(&e)),
        };
        let short = self.vault.store.short(&id);
        let sep = self.theme.glyphs().sep;
        let r = self.write(|b| {
            b.ops.extend(ops);
            Ok(())
        });
        self.report(r, "rewound", None, format!("{short} {sep} {n} field{}", if n == 1 { "" } else { "s" }));
        let _ = self.reload();
    }

    /// Views on the Tasks saved row, by slot (1–9).
    pub fn view_slots(&self) -> Vec<(u32, thc_core::views::View)> {
        let mut v: Vec<(u32, thc_core::views::View)> = self.saved_views.iter().filter_map(|v| v.tasks.map(|t| (t, v.clone()))).collect();
        v.sort_by_key(|(t, _)| *t);
        v
    }

    /// The palette's entries in their empty-query order: the last 3 commands,
    /// `Update thc` while an update is waiting, then the table (common ones first).
    pub fn palette_entries(&self) -> Vec<PaletteEntry> {
        let mut out: Vec<PaletteEntry> = PALETTE.iter().map(|p| PaletteEntry { label: p.label.into(), keys: p.action.into(), cmd: p.cmd.into(), shown: crate::keymap::key_for(self, p.action).unwrap_or_default() }).collect();
        let mut front: Vec<String> = self.recent_cmds.clone();
        if self.update_available.is_some() && !front.iter().any(|k| k == "update") {
            front.push("update".into());
        }
        for (i, k) in front.iter().enumerate() {
            if let Some(j) = out.iter().position(|p| &p.keys == k) {
                let e = out.remove(j);
                out.insert(i.min(out.len()), e);
            }
        }
        // In a document: caretline's editing commands, in its words (editing_keys.rs).
        if self.doc.is_some() {
            let mut seen = Vec::new();
            for (cmd, action) in crate::editing_keys::ACTIONS {
                let Some(info) = caretline::commands::command(cmd) else { continue };
                if cmd.starts_with("select.") && *cmd != "select.all" || seen.contains(action) {
                    continue;
                }
                seen.push(*action);
                out.push(PaletteEntry { label: format!("edit: {}", info.name.to_lowercase()), keys: format!("edit:{action}"), cmd: String::new(), shown: crate::keymap::key_for(self, action).unwrap_or_default() });
            }
        }
        for v in &self.saved_views {
            out.push(PaletteEntry { label: format!("view: {}", v.name), keys: format!("@{}", v.name), cmd: format!("thc q @{}", v.name), shown: String::new() });
        }
        for v in &self.saved_views {
            out.push(PaletteEntry { label: format!("context: {}", v.name), keys: format!("ctx:{}", v.name), cmd: format!("thc context {}", v.name), shown: String::new() });
        }
        out.push(PaletteEntry { label: "context: none".into(), keys: "ctx:none".into(), cmd: "thc context none".into(), shown: String::new() });
        // `vault: acme`: switch this session to another vault (vaults.md §8).
        for e in thc_core::registry::Registry::load().vaults {
            if e.name != self.vault_name {
                out.push(PaletteEntry { label: format!("vault: {}", e.name), keys: format!("vault:{}", e.name), cmd: format!("thc --vault {} tui", e.name), shown: String::new() });
            }
        }
        out
    }

    /// `C`: the context off for this TUI session, then back on.
    pub fn toggle_context(&mut self) {
        let Some(c) = self.context.clone() else { return self.info("no context · :context <name> turns one on") };
        self.context_on = !self.context_on;
        let _ = self.reload();
        self.info(if self.context_on { format!("context @{} on", c.name) } else { format!("context @{} off for this session · C turns it back on", c.name) });
    }

    /// `:context <name>` / `:context none`: this device's context, as `thc context` sets it.
    pub fn set_context(&mut self, name: Option<&str>) {
        let cache = self.vault.paths.cache.clone();
        if let Some(n) = name {
            if thc_core::views::find(&self.vault.store, n).ok().flatten().is_none() {
                return self.error(format!("no view @{n}"));
            }
        }
        if let Err(e) = thc_core::context::set_device(&cache, name) {
            return self.error(friendly_error(&e));
        }
        self.context = std::env::current_dir().ok().and_then(|d| thc_core::context::resolve(None, &cache, &d));
        self.context_on = true;
        let _ = self.reload();
        self.info(match name {
            Some(n) => format!("context @{n} is on"),
            None => "context off".into(),
        });
    }

    /// The context as the bar and header show it, when it's on.
    pub fn context_name(&self) -> Option<String> {
        self.context_active.as_ref().map(|a| a.ctx.name.clone())
    }

    /// Capture defaults' tags, for the drawer's target line and the capture itself.
    pub fn context_tags(&self) -> Vec<String> {
        self.context_active.as_ref().map(|a| a.capture_defaults().tags).unwrap_or_default()
    }

    /// Choose a view: Tasks with `q› @work` (the expansion shows beside it, muted).
    pub fn use_view(&mut self, name: &str) {
        self.tasks_filter = format!("@{name}");
        self.selected = None;
        if self.view != View::Tasks {
            self.set_view(View::Tasks);
        } else {
            let _ = self.reload();
        }
    }

    /// `:view add <name>` / `:view save <name>`: the current Tasks filter as a view.
    pub fn save_view(&mut self, name: &str, add: bool) {
        // Saved as typed (`@work due<=+3d`), so a later edit to @work flows through.
        let query = self.tasks_filter.trim().to_string();
        if let Err(e) = thc_core::query::compile(&query, &self.vault.store, self.today) {
            return self.error(friendly_error(&e));
        }
        let r = self.write_here(|b| {
            if add {
                thc_core::views::add(b, name, &query, None, None, false, None).map(|_| ())
            } else {
                thc_core::views::ensure_seeded(b)?;
                thc_core::views::set(b, name, thc_core::views::Update { query: Some(&query), ..Default::default() }).map(|_| ())
            }
        });
        let name = name.trim_start_matches('@').to_string();
        match r {
            Ok(_) => {
                self.tasks_filter = format!("@{name}");
                let _ = self.reload();
                self.info(format!("saved view @{name} · {query}"));
            }
            Err(e) => self.error(friendly_error(&e)),
        }
    }

    pub fn copy_id(&mut self, full: bool) {
        let Some(n) = self.selected_node() else { return };
        let id = if full { n.id.clone() } else { self.vault.store.short(&n.id) };
        // OSC 52 works over SSH and in most modern terminals; native tools as a backup.
        let b64 = base64(id.as_bytes());
        print!("\x1b]52;c;{b64}\x07");
        let _ = ["pbcopy", "wl-copy"].iter().any(|prog| {
            std::process::Command::new(prog)
                .stdin(std::process::Stdio::piped())
                .spawn()
                .and_then(|mut c| {
                    use std::io::Write;
                    c.stdin.take().unwrap().write_all(id.as_bytes())?;
                    c.wait().map(|_| ())
                })
                .is_ok()
        });
        self.confirm("copied", None, id);
    }

    /// Enter: open a page from the finder, the day/page a node lives in, a tag's tasks, etc.
    pub fn open_selected(&mut self) {
        self.remember_origin();
        if let Some(Row::ViewItem { name, .. }) = self.rows.get(self.cursor).cloned() {
            return self.use_view(&name);
        }
        match self.rows.get(self.cursor).cloned() {
            Some(Row::Tag { name, .. }) => {
                self.tasks_filter = format!("#{name}");
                self.set_view(View::Tasks);
                return;
            }
            Some(Row::Tx { entries, .. }) => {
                if let Some(e) = entries.iter().find(|e| e.entity.len() == 12) {
                    let id = e.entity.clone();
                    self.jump_to_node(&id);
                }
                return;
            }
            _ => {}
        }
        let Some(n) = self.selected_node().cloned() else { return };
        // A row from another vault: you move into that vault while it's open (vaults.md §3.5).
        if let Some(i) = self.row_from(self.cursor) {
            if let Some(o) = self.others.get(i) {
                self.switch_to = Some(o.path.clone());
                self.switch_focus = Some(n.id.clone());
                self.switch_return = Some(self.vault.origin.as_ref().map_or(self.vault.paths.vault.clone(), |x| x.vault.clone()));
            }
            return;
        }
        // An issue (issues.md §1): a task with a body, or one on the vault's capture page, opens
        // as its own document.
        if self.is_issue(&n) {
            self.doc_back = None;
            self.page_open = Some(n.id.clone());
            self.view = View::Pages;
            self.selected = None;
            let _ = self.reload();
            return;
        }
        if self.view == View::Pages && self.page_open.is_none() && n.parent.is_none() && n.title.is_some() {
            self.page_open = Some(n.id.clone());
            self.cursor = 0;
            self.selected = None;
            let _ = self.reload();
            return;
        }
        self.jump_to_node(&n.id);
    }

    /// Open the page or day a node lives in, with the cursor on it (`thc tui --focus`).
    pub fn focus_node(&mut self, id: &str) {
        self.jump_to_node(id);
    }

    /// Where Esc returns from a document opened now (navigation.md §2): this view and its row.
    /// Only from a list view: a document opened from a document keeps the chain's origin.
    pub(crate) fn remember_origin(&mut self) {
        if self.doc.is_none() && !matches!(self.view, View::Journal) {
            self.doc_origin = Some((self.view, self.rows.get(self.cursor).and_then(|r| r.key())));
        }
    }

    /// Esc in a document: save, then back to the view it was opened from (with its row), or
    /// Today (navigation.md §2).
    /// An issue: a task with children, or a task directly on the vault's capture target page.
    pub fn is_issue(&self, n: &Node) -> bool {
        if n.status.is_none() {
            return false;
        }
        let target = thc_core::settings::capture_target(&thc_core::settings::current(), &self.vault.store).map(|(id, _)| id);
        n.parent.is_some() && n.parent == target || self.vault.store.children(&n.id).map(|c| !c.is_empty()).unwrap_or(false)
    }

    pub fn leave_doc(&mut self) {
        self.save_doc(true);
        // An issue opened from its page: back to that page, the caret on the issue.
        if let Some((back, line, _)) = self.doc_back.take().filter(|b| self.page_open.as_deref() == Some(b.2.as_str())) {
            match back {
                crate::editor::Target::Page { id, .. } => {
                    self.page_open = Some(id);
                    self.set_view(View::Pages);
                }
                crate::editor::Target::Journal { date } => {
                    self.page_open = None;
                    self.journal_date = date;
                    self.set_view(View::Journal);
                }
            }
            if let Some(d) = self.doc.as_mut() {
                d.set_caret_anchor(&crate::editor::Anchor { id: line, byte: 0 });
            }
            return;
        }
        // Entered from a cross-vault view: back to it, in the vault you were in (vaults.md §3.5).
        if let Some(back) = self.return_vault.take() {
            self.switch_to = Some(back);
            return;
        }
        match self.doc_origin.take() {
            Some((View::Pages, sel)) => {
                self.page_open = None;
                if sel.is_some() {
                    self.last_page = sel;
                }
                self.show_pages();
            }
            Some((v, sel)) => {
                self.page_open = None;
                self.set_view(v);
                if let Some(k) = sel.filter(|k| self.rows.iter().any(|r| r.key().as_deref() == Some(k.as_str()))) {
                    self.selected = Some(k);
                    let _ = self.reload();
                }
            }
            None => {
                // (A page left for Today is still where the Pages list's cursor rests.)
                if let Some(p) = self.page_open.take() {
                    self.last_page = Some(p);
                }
                self.set_view(View::Today);
            }
        }
    }

    fn jump_to_node(&mut self, id: &str) {
        let Some(n) = self.vault.store.node(id).ok().flatten() else { return };
        let mut root = n.clone();
        while let Some(p) = root.parent.clone().and_then(|p| self.vault.store.node(&p).ok().flatten()) {
            root = p;
        }
        if let Some(j) = &root.journal {
            if let Some(dv) = dates::DateVal::from_stored(j) {
                self.journal_date = dv.date();
                self.view = View::Journal;
            }
        } else if root.title.is_some() {
            self.page_open = Some(root.id.clone());
            self.view = View::Pages;
        } else {
            self.view = View::Inbox;
        }
        self.selected = Some(n.id.clone());
        let _ = self.reload();
    }

    /// Whether `back` has something to undo (an open page, a filter, a node's history, the
    /// detail pane); `q` quits otherwise.
    pub fn can_back(&self) -> bool {
        (self.view == View::Pages && (self.page_open.is_some() || !self.pages_filter.is_empty()))
            || (self.view == View::Log && self.log_node.is_some())
            || (self.view == View::Tasks && self.tasks_filter != "status:open sort:due")
            || self.focus == Focus::Detail
    }

    pub fn back(&mut self) {
        if self.toast.as_ref().is_some_and(|t| t.kind == ToastKind::Error) {
            self.toast = None;
            return;
        }
        if self.view == View::Pages && self.page_open.is_some() {
            self.selected = self.page_open.take();
            let _ = self.reload();
        } else if self.view == View::Pages && !self.pages_filter.is_empty() {
            self.pages_filter.clear();
            let _ = self.reload();
        } else if self.view == View::Log && self.log_node.is_some() {
            self.log_node = None;
            let _ = self.reload();
        } else if self.view == View::Tasks && self.tasks_filter != "status:open sort:due" {
            self.tasks_filter = "status:open sort:due".into();
            let _ = self.reload();
        } else if self.focus == Focus::Detail {
            self.focus = Focus::List;
        }
    }

    pub fn toggle_fold(&mut self, open: Option<bool>) {
        let Some(Row::Node { node, has_children, collapsed, .. }) = self.rows.get(self.cursor).cloned() else { return };
        if !has_children {
            return;
        }
        if open.map(|o| !o).unwrap_or(!collapsed) {
            self.collapsed.insert(node.id);
        } else {
            self.collapsed.remove(&node.id);
        }
        let _ = self.reload();
    }

    /// Outline structure edits: `>` indent, `<` outdent, `J`/`K` move among siblings.
    pub fn restructure(&mut self, op: char) {
        let Some(n) = self.selected_node().cloned() else { return };
        if !self.rows.get(self.cursor).is_some_and(|r| matches!(r, Row::Node { outline: true, .. })) {
            return self.info("indent and reorder work in outlines (Pages, Journal)");
        }
        let Some(parent) = n.parent.clone() else { return };
        let siblings = self.vault.store.children(&parent).unwrap_or_default();
        let i = siblings.iter().position(|x| x.id == n.id).unwrap_or(0);
        let label = self.node_label(&n.id);
        let r = match op {
            '>' => {
                if i == 0 {
                    return self.info("nothing above to indent under");
                }
                let new_parent = siblings[i - 1].id.clone();
                self.collapsed.remove(&new_parent);
                self.write(|b| b.move_to(&n.id, Some(new_parent), None, None))
            }
            '<' => {
                let Some(gp) = self.vault.store.node(&parent).ok().flatten() else { return };
                if gp.parent.is_none() {
                    return self.info("already at the top level");
                }
                let gpid = gp.parent.clone();
                self.write(|b| b.move_to(&n.id, gpid, Some(&parent), None))
            }
            'J' => match siblings.get(i + 1) {
                Some(nx) => {
                    let after = nx.id.clone();
                    self.write(|b| b.move_to(&n.id, Some(parent.clone()), Some(&after), None))
                }
                None => return,
            },
            _ => match i.checked_sub(1).and_then(|j| siblings.get(j)) {
                Some(pv) => {
                    let before = pv.id.clone();
                    self.write(|b| b.move_to(&n.id, Some(parent.clone()), None, Some(&before)))
                }
                None => return,
            },
        };
        match r {
            Err(e) => self.error(friendly_error(&e)),
            Ok(_) if op == '>' || op == '<' => self.confirm("moved", Some(&n.id), label),
            Ok(_) => {}
        }
    }

    /// Picker rows: pages (fuzzy), then a rule, then days and the inbox. `None` = the rule.
    pub fn move_items(&self, query: &str) -> Vec<Option<MoveItem>> {
        let g = self.theme.glyphs();
        let s = &self.vault.store;
        let q = query.trim().to_lowercase();
        let mut pages: Vec<(i64, MoveItem)> = Vec::new();
        for p in s.nodes_where(&format!("n.parent IS NULL AND n.title IS NOT NULL AND n.is_tag=0 AND n.deleted=0 AND {} ORDER BY n.updated_ms DESC", thc_core::views::HIDDEN_SQL), &[]).unwrap_or_default() {
            let label = format!("{} {}", g.page, p.label());
            if let Some(score) = fuzzy(&q, &p.label().to_lowercase()) {
                let kids = s.children(&p.id).map(|k| k.len()).unwrap_or(0);
                pages.push((score, MoveItem { label, note: format!("{kids} children"), target: MoveTarget::Under(p.id.clone()) }));
            }
        }
        if !q.is_empty() {
            pages.sort_by_key(|(sc, _)| -*sc);
        }
        let mut items: Vec<Option<MoveItem>> = pages.into_iter().map(|(_, i)| Some(i)).collect();
        if !q.is_empty() && items.is_empty() {
            items.push(Some(MoveItem { label: format!("+ new page \"{}\"", query.trim()), note: "keep typing".into(), target: MoveTarget::NewPage(query.trim().to_string()) }));
        }
        let mut days: Vec<MoveItem> = Vec::new();
        if !q.is_empty() {
            if let Ok(d) = dates::parse(query, self.today) {
                days.push(MoveItem { label: format!("{} {}", g.journal, d.date().format("%a %b %-d")), note: "journal".into(), target: MoveTarget::Journal(d.date()) });
            }
        }
        for it in [
            MoveItem { label: format!("{} today", g.journal), note: self.today.format("%a %b %-d").to_string(), target: MoveTarget::Journal(self.today) },
            MoveItem { label: "inbox".into(), note: "top level".into(), target: MoveTarget::Inbox },
        ] {
            if fuzzy(&q, &it.label.to_lowercase()).is_some() {
                days.push(it);
            }
        }
        if !days.is_empty() {
            if !items.is_empty() {
                items.push(None);
            }
            items.extend(days.into_iter().map(Some));
        }
        items
    }

    pub fn do_move(&mut self, node: &str, target: MoveTarget) {
        let id = node.to_string();
        let label = self.node_label(&id);
        let t = target.clone();
        self.advance_if_triage();
        let mut created_page = None;
        let cp = &mut created_page;
        let r = self.write(|b| {
            let parent = match &t {
                MoveTarget::Inbox => None,
                MoveTarget::Journal(d) => Some(b.journal(*d)?),
                MoveTarget::Under(p) => Some(p.clone()),
                MoveTarget::NewPage(title) => {
                    let pid = b.create_page(title, &[])?;
                    *cp = Some(pid.clone());
                    Some(pid)
                }
            };
            b.move_to(&id, parent, None, None)
        });
        let target = match (target, created_page) {
            (MoveTarget::NewPage(_), Some(pid)) => MoveTarget::Under(pid),
            (t, _) => t,
        };
        let g = self.theme.glyphs();
        let dest = match &target {
            MoveTarget::Inbox => "inbox".to_string(),
            MoveTarget::Journal(d) if *d == self.today => format!("{} today", g.journal),
            MoveTarget::Journal(d) => format!("{} {}", g.journal, d.format("%b %-d")),
            MoveTarget::Under(p) => format!("{} {}", g.page, self.node_label(p)),
            MoveTarget::NewPage(t) => format!("{} {t}", g.page),
        };
        if r.is_ok() {
            self.recent_moves.retain(|m| !same_target(m, &target));
            self.recent_moves.insert(0, target);
            self.recent_moves.truncate(3);
        }
        self.report(r, "moved", Some(node), format!("{label} {} {dest}", g.arrow));
    }

    /// Handle messages from the live daemon connection.
    pub fn drain_live(&mut self) {
        let Some(rx) = &self.live_rx else { return };
        let msgs: Vec<crate::live::LiveMsg> = rx.try_iter().collect();
        for m in msgs {
            match m {
                crate::live::LiveMsg::Connected { version_mismatch } => {
                    self.daemon_live = true;
                    if version_mismatch {
                        self.info("the daemon runs a different thc version · thc daemon restart");
                    }
                }
                crate::live::LiveMsg::Disconnected => {
                    self.daemon_live = false;
                    let soon = self.vault.store.pending_alerts_within(thc_core::alerts::now(), chrono::Duration::hours(24)).unwrap_or(0);
                    if !self.offline_toast_shown && soon > 0 {
                        self.offline_toast_shown = true;
                        self.info("alerts paused until the daemon runs");
                    }
                }
                crate::live::LiveMsg::Event { event, data } => match event.as_str() {
                    "changed" | "conflict" => {
                        // A push for this view's own save (same device, by the TUI) is old news.
                        match data.get("tx").and_then(|t| t.as_str()) {
                            Some(tx) => {
                                let foreign = self
                                    .vault
                                    .store
                                    .conn
                                    .query_row("SELECT EXISTS(SELECT 1 FROM events WHERE tx = ?1 AND (dev <> ?2 OR via <> 'tui'))", rusqlite::params![tx, self.vault.device], |r| r.get::<_, bool>(0))
                                    .unwrap_or(true);
                                if foreign {
                                    self.live_txs.push(tx.to_string());
                                }
                            }
                            None => self.live_dirty = true,
                        }
                        // The loop polls next, recorded as a `poll` message (session.rs).
                        self.poll_wanted = true;
                    }
                    "alert.fire" => self.alert_toast(&data),
                    "alert.withdraw" => {
                        if self.toast.as_ref().is_some_and(|t| t.kind == ToastKind::Alert) {
                            self.toast = None;
                        }
                        let _ = self.reload();
                    }
                    _ => {}
                },
            }
        }
    }

    /// Snapshot fixture: show the alert toast for the first pending alert.
    pub fn simulate_alert(&mut self) {
        let s = &self.vault.store;
        let Some(a) = s.alerts_where("deleted=0 AND state IN ('pending','snoozed') ORDER BY fire_at", &[]).ok().and_then(|v| v.into_iter().next()) else { return };
        let Some(n) = s.node(&a.node).ok().flatten() else { return };
        let d = thc_core::alerts::Delivery::Single { notification: thc_core::alerts::build(s, &a, &n, self.today) };
        if let Ok(v) = serde_json::to_value(&d) {
            self.alert_toast(&v);
        }
    }

    fn alert_toast(&mut self, d: &serde_json::Value) {
        let g = self.theme.glyphs();
        let (text, node) = match d.get("kind").and_then(|k| k.as_str()) {
            Some("single") => {
                let n = &d["notification"];
                let title = n["title"].as_str().unwrap_or("").to_string();
                let when = n["fire_at"].as_str().and_then(|f| f.get(11..16)).unwrap_or("").to_string();
                (format!("{title} {} {when}", g.sep), n["node"].as_str().map(str::to_string))
            }
            Some("summary") => (format!("{} {} {}", d["title"].as_str().unwrap_or(""), g.sep, d["body"].as_str().unwrap_or("")), None),
            _ => return,
        };
        self.alert_toast_node = node.clone();
        if let Some(n) = node {
            let at = self.ui.now_ms;
            self.ui.flashes.insert(n, (at, false));
        }
        self.toast_parts(ToastKind::Alert, vec![(format!("{} ", g.alert), Token::Overdue), (text, Token::Text)]);
        let _ = self.reload();
    }

    pub fn start_daemon(&mut self) {
        let exe = std::env::current_exe().unwrap_or_else(|_| "thc".into());
        let r = std::process::Command::new(exe)
            .args(["daemon", "start"])
            .env("THC_VAULT", &self.vault.paths.vault)
            .env("THC_CACHE_DIR", &self.vault.paths.cache)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match r {
            Ok(s) if s.success() => self.confirm("started", None, "daemon · alerts are live".into()),
            _ => self.error("couldn't start the daemon · run thc daemon start in a shell to see why"),
        }
        // Reconnect promptly instead of waiting for the 10 s retry.
        self.live_rx = Some(crate::live::spawn(self.vault.paths.clone()));
    }

    /// Poll the log for changes from other processes (agents, other devices via sync). True when
    /// it found some (and reloaded): the session records that as a `poll` message.
    pub fn poll_external(&mut self) -> Result<bool> {
        let mut at = Instant::now();
        let mut split = |name: &'static str, out: &mut Vec<(&'static str, f64)>| {
            let now = Instant::now();
            out.push((name, (now - at).as_secs_f64() * 1000.0));
            at = now;
        };
        self.poll_split.clear();
        // The other vaults a cross-vault view shows: their changes reload it.
        let mut others_moved = false;
        for o in self.others.iter_mut() {
            if o.vault.catch_up().unwrap_or(0) > 0 || o.vault.take_news().foreign() {
                others_moved = true;
            }
        }
        if others_moved {
            self.reload_stable()?;
        }
        split("others", &mut self.poll_split);
        let sizes = self.vault.log.files()?;
        let changed = sizes != self.log_sizes;
        self.log_sizes = sizes;
        split("files", &mut self.poll_split);
        if changed {
            self.vault.catch_up()?;
        }
        split("catch_up", &mut self.poll_split);
        // What arrived from elsewhere since the last poll: what this vault's catch-ups took in
        // (a local save catches up first, too) and what the daemon pushed. Not a watermark:
        // another device's events arrive with older times, and a rebuild renumbers rows.
        let news = self.vault.take_news();
        let txs = std::mem::take(&mut self.live_txs);
        let dirty = std::mem::take(&mut self.live_dirty);
        split("news", &mut self.poll_split);
        if !news.foreign() && txs.is_empty() && !dirty {
            return Ok(others_moved);
        }
        let eids: Vec<&str> = news.events.iter().filter(|(_, own)| !own).map(|(e, _)| e.as_str()).collect();
        let fresh = self.vault.store.history_where(
            "eid IN (SELECT value FROM json_each(?1)) OR tx IN (SELECT value FROM json_each(?2)) ORDER BY okey",
            &[&serde_json::to_string(&eids)?, &serde_json::to_string(&txs)?],
        )?;
        let now = self.ui.now_ms;
        for e in &fresh {
            if e.entity.len() == 12 {
                self.flashes.insert(e.entity.clone(), (now, e.actor.starts_with("agent")));
            }
        }
        self.reload_stable()?;
        let g = self.theme.glyphs();
        let agent_events: Vec<&HistoryEntry> = fresh.iter().filter(|e| e.actor.starts_with("agent")).collect();
        if let Some(first) = agent_events.first() {
            let name = first.actor.trim_start_matches("agent:").to_string();
            self.last_agent_tx = Some(agent_events.last().unwrap().tx.clone());
            let nodes: HashSet<&str> = agent_events.iter().filter(|e| e.entity.len() == 12).map(|e| e.entity.as_str()).collect();
            let created: Vec<&&HistoryEntry> = agent_events.iter().filter(|e| e.op == "node.create").collect();
            let mut parts = vec![(format!("{}{}{name} ", g.agent, g.agent_sep), Token::Agent)];
            if created.len() == 1 {
                let id = created[0].entity.clone();
                parts.push(("added ".into(), Token::Text));
                parts.push((format!("{} ", self.vault.store.short(&id)), Token::Muted));
                parts.push((format!("\"{}\"", crate::ui::truncate_str(&self.node_label(&id), 40, g.ellipsis)), Token::Muted));
            } else {
                // Real plurals: lines in a document, nodes in lists.
                let n = nodes.len().max(1);
                let noun = if self.doc.is_some() { "line" } else { "node" };
                parts.push((format!("changed {n} {noun}{}", if n == 1 { "" } else { "s" }), Token::Text));
            }
            self.toast_parts(ToastKind::Agent, parts);
        } else if news.overflow {
            self.info("changes synced".to_string());
        } else if !fresh.is_empty() {
            let n = fresh.len();
            self.info(format!("{n} change{} synced", if n == 1 { "" } else { "s" }));
        }
        // On the caret's line §9's copy wins, with who.
        if let Some(edited) = self.main.announce.take() {
            let who = agent_events.first().map(|e| format!("{}{}{}", g.agent, g.agent_sep, e.actor.trim_start_matches("agent:"))).unwrap_or_else(|| "• another device".into());
            let text = if edited { " changed this line too · both versions are kept" } else { " changed this line · it updates when you leave it" };
            self.toast_parts(ToastKind::Agent, vec![(who, Token::Agent), (text.into(), Token::Text)]);
        }
        Ok(true)
    }
}

fn same_target(a: &MoveTarget, b: &MoveTarget) -> bool {
    match (a, b) {
        (MoveTarget::Inbox, MoveTarget::Inbox) => true,
        (MoveTarget::Journal(x), MoveTarget::Journal(y)) => x == y,
        (MoveTarget::Under(x), MoveTarget::Under(y)) => x == y,
        (MoveTarget::NewPage(x), MoveTarget::NewPage(y)) => x == y,
        _ => false,
    }
}

/// Second log line: what the transaction did, in words a person would say.
/// e.g. `in § today · due tomorrow · links ¶ Reading List · tagged #reading`
pub fn tx_detail(store: &thc_core::store::Store, entries: &[HistoryEntry], g: &crate::theme::Glyphs) -> String {
    let today = thc_core::dates::today();
    let place = |id: &str| -> String {
        match store.node(id).ok().flatten() {
            Some(p) if p.journal.is_some() => {
                let d = p.journal.as_deref().and_then(thc_core::dates::DateVal::from_stored).map(|d| d.date());
                match d {
                    Some(d) if d == today => format!("{} today", g.journal),
                    Some(d) => format!("{} {}", g.journal, d.format("%b %-d")),
                    None => g.journal.to_string(),
                }
            }
            Some(p) if p.parent.is_none() && p.title.is_some() => format!("{} {}", g.page, p.label()),
            Some(p) => crate::ui::truncate_str(&store.render_text(&p.label()), 24, g.ellipsis),
            None => "?".into(),
        }
    };
    let pretty = |v: &str| -> String {
        thc_core::dates::DateVal::from_stored(v)
            .map(|d| {
                let rel = thc_core::dates::relative(d.date(), today);
                let t = d.time().map(|t| format!(" {}", t.format("%H:%M"))).unwrap_or_default();
                if (0..7).contains(&(d.date() - today).num_days()) { format!("{rel}{t}") } else { format!("{}{t}", d.date().format("%b %-d")) }
            })
            .unwrap_or_else(|| v.to_string())
    };
    let prop_words = |k: &str, v: &serde_json::Value| -> Option<String> {
        let val = match v {
            serde_json::Value::Null => return Some(format!("{k} cleared")),
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Object(o) => o.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string(),
            serde_json::Value::Bool(_) => return None,
            o => o.to_string(),
        };
        Some(match k {
            "due" => format!("due {}", pretty(&val)),
            "scheduled" => format!("scheduled {}", pretty(&val)),
            "status" => format!("status {val}"),
            "priority" => format!("!{val}"),
            "repeat" => format!("{} {val}", g.repeat),
            "done_at" | "journal" | "tag" => return None,
            "title" => format!("title “{val}”"),
            _ => format!("{k} {} {val}", g.arrow),
        })
    };
    let mut parts: Vec<String> = Vec::new();
    for e in entries.iter().rev() {
        match e.op.as_str() {
            "node.create" => {
                if e.body.get("props").and_then(|p| p.get("tag")).is_some() || e.body.get("props").and_then(|p| p.get("journal")).is_some() {
                    continue;
                }
                if let Some(p) = e.body.get("parent").and_then(|v| v.as_str()) {
                    parts.push(format!("in {}", place(p)));
                } else if e.body.get("title").is_some() {
                    parts.push("new page".into());
                } else {
                    parts.push("in inbox".into());
                }
                if let Some(props) = e.body.get("props").and_then(|v| v.as_object()) {
                    for (k, v) in props {
                        if k != "status" {
                            parts.extend(prop_words(k, v));
                        }
                    }
                }
            }
            "node.set" => {
                if let Some(props) = e.body.get("props").and_then(|v| v.as_object()) {
                    for (k, v) in props {
                        parts.extend(prop_words(k, v));
                    }
                }
            }
            "node.move" => {
                let dest = e.body.get("parent").and_then(|v| v.as_str()).map(place).unwrap_or_else(|| "inbox".into());
                parts.push(format!("{} {dest}", g.arrow));
            }
            "node.complete" => {
                if let Some(n) = e.body.get("next").and_then(|n| n.get("scheduled").or(n.get("due"))).and_then(|v| v.as_str()) {
                    parts.push(format!("{} next {}", g.repeat, pretty(n)));
                }
            }
            "edge.add" | "edge.remove" => {
                let rel = e.body.get("rel").and_then(|v| v.as_str()).unwrap_or("");
                let dst = e.body.get("dst").and_then(|v| v.as_str()).unwrap_or("");
                let label = store.node(dst).ok().flatten().map(|n| n.label()).unwrap_or_default();
                let add = e.op == "edge.add";
                parts.push(match rel {
                    "tag" if add => format!("tagged #{label}"),
                    "tag" => format!("untagged #{label}"),
                    "mention" if add => format!("links {} {label}", g.page),
                    "mention" => format!("unlinks {} {label}", g.page),
                    r => format!("{} {r} {label}", if add { "+" } else { "-" }),
                });
            }
            "alert.add" => parts.push(format!("{} alert", g.alert)),
            _ => {}
        }
    }
    parts.dedup();
    parts.join(&format!(" {} ", g.sep))
}

/// Make core errors read like the product (tui-handoff §8.2).
pub fn friendly_error(e: &anyhow::Error) -> String {
    // Read-only answers every write key the same way (policy.md §2.2).
    if let Some(thc_core::error::ThcError::Refused { kind: "readonly", .. }) = e.downcast_ref() {
        return "read-only · nothing written".into();
    }
    let s = format!("{e:#}");
    s.replace("invalid: ", "").replace("not found: ", "")
}

/// Subsequence fuzzy match; higher is better. Empty pattern matches everything.
pub fn fuzzy(pat: &str, s: &str) -> Option<i64> {
    if pat.is_empty() {
        return Some(0);
    }
    let mut score = 0i64;
    let mut it = s.char_indices();
    let mut last: Option<usize> = None;
    for pc in pat.chars() {
        let (i, _) = it.by_ref().find(|(_, c)| *c == pc)?;
        score += match last {
            Some(l) if i == l + 1 => 5,
            _ => 1,
        };
        if i == 0 {
            score += 3;
        }
        last = Some(i);
    }
    Some(score - s.len() as i64 / 10)
}

/// Char positions in `s` matched by `pat` (for highlighting).
pub fn fuzzy_positions(pat: &str, s: &str) -> Vec<usize> {
    let lower: Vec<char> = s.to_lowercase().chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    for pc in pat.to_lowercase().chars().filter(|c| *c != ' ') {
        while i < lower.len() && lower[i] != pc {
            i += 1;
        }
        if i == lower.len() {
            return vec![];
        }
        out.push(i);
        i += 1;
    }
    out
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A review value in words: dates relative, `!high`, `↻ every week`, `§ today`.
pub fn review_value(v: &serde_json::Value, key: &str, g: &crate::theme::Glyphs, today: NaiveDate) -> String {
    use serde_json::Value;
    match (key, v) {
        (_, Value::Null) => "—".into(),
        ("priority", Value::String(p)) => format!("!{p}"),
        ("repeat", Value::String(t)) => format!("{} {t}", g.repeat),
        ("parent", Value::String(p)) => match p.strip_prefix("§ ").and_then(thc_core::dates::DateVal::from_stored) {
            Some(d) if d.date() == today => format!("{} today", g.journal),
            Some(d) => format!("{} {}", g.journal, d.date().format("%b %-d")),
            None => p.replacen("§ ", &format!("{} ", g.journal), 1).replacen("¶ ", &format!("{} ", g.page), 1),
        },
        (_, Value::String(s)) => match thc_core::dates::DateVal::from_stored(s) {
            Some(d) => {
                let rel = thc_core::dates::relative(d.date(), today);
                let t = d.time().map(|t| format!(" {}", t.format("%H:%M"))).unwrap_or_default();
                if (0..7).contains(&(d.date() - today).num_days()) { format!("{rel}{t}") } else { format!("{}{t}", d.date().format("%b %-d")) }
            }
            None => s.clone(),
        },
        (_, other) => other.to_string(),
    }
}

fn review_set(f: &serde_json::Value, prefix: &str, minus: &str) -> String {
    let mut parts = Vec::new();
    for (side, sign) in [("add", "+"), ("remove", minus)] {
        for v in f.get(side).and_then(|a| a.as_array()).into_iter().flatten() {
            parts.push(format!("{sign}{prefix}{}", v.as_str().unwrap_or("")));
        }
    }
    parts.join(" ")
}

/// `(key, old, new)` lines for one change, in display order (agents.md §1.6 detail pane).
pub fn review_field_lines(c: &thc_core::review::Change, g: &crate::theme::Glyphs, today: NaiveDate) -> Vec<(String, Option<String>, String)> {
    let mut keys: Vec<&String> = c.fields.keys().filter(|k| k.as_str() != "done_at").collect();
    keys.sort_by_key(|k| thc_core::review::field_rank(k));
    if c.change == "create" {
        keys.sort_by_key(|k| k.as_str() != "parent");
    }
    keys.into_iter()
        .map(|k| {
            let f = &c.fields[k];
            let label = match k.as_str() {
                "parent" => "in".to_string(),
                "scheduled" => "sched".to_string(),
                _ => k.clone(),
            };
            if k == "tags" || k == "links" {
                let minus = if g.arrow == "→" { "−" } else { "-" };
                return (label, None, review_set(f, if k == "tags" { "#" } else { "" }, minus));
            }
            let old = match f.get("from") {
                Some(v) => Some(review_value(v, k, g, today)),
                None if c.change != "create" => Some(review_value(&serde_json::Value::Null, k, g, today)),
                None => None,
            };
            let new = review_value(f.get("to").unwrap_or(&serde_json::Value::Null), k, g, today);
            (label, old, new)
        })
        .collect()
}

/// The second line of a review row: the first change's marker, node and what changed.
pub fn review_summary(store: &thc_core::store::Store, it: &thc_core::review::Item, g: &crate::theme::Glyphs, today: NaiveDate) -> String {
    let Some(c) = it.changes.first() else { return String::new() };
    if it.changes.len() > 1 {
        // A batch: `+ 3 created · ~ 1 changed · ✓ 1 done · → 1 moved`.
        let mut parts: Vec<String> = thc_core::review::counts(&it.changes).iter().map(|(k, n)| format!("{} {n} {}", review_marker(k, g), thc_core::review::count_word(k))).collect();
        let links = thc_core::review::link_count(&it.changes);
        if links > 0 {
            parts.push(format!("{links} link{}", if links == 1 { "" } else { "s" }));
        }
        return parts.join(&format!(" {} ", g.sep));
    }
    let mark = review_marker(c.change, g);
    let more = if it.changes.len() > 1 { format!(" {} +{} more", g.sep, it.changes.len() - 1) } else { String::new() };
    let flag = if it.conflict { format!(" {} {} conflict", g.sep, g.conflict) } else if it.later_changed_by_human { format!(" {} you changed it afterwards", g.sep) } else { String::new() };
    // The node ID only when the transaction touched several nodes.
    let id = if it.changes.len() > 1 { format!("{} ", c.short) } else { String::new() };
    format!("{mark} {id}{}{more}{flag}", change_words(store, c, g, today))
}

/// `done · ↻ next Tue Oct 6`, `moved from … to …`, or the field lines joined.
pub fn change_words(store: &thc_core::store::Store, c: &thc_core::review::Change, g: &crate::theme::Glyphs, today: NaiveDate) -> String {
    let to = |k: &str| c.fields.get(k).and_then(|f| f.get("to")).cloned().unwrap_or(serde_json::Value::Null);
    match c.change {
        "complete" => {
            // A repeating completion moves its dates (status stays todo).
            {
                let next = ["scheduled", "due"].iter().find_map(|k| to(k).as_str().and_then(thc_core::dates::DateVal::from_stored)).map(|d| d.date());
                match next {
                    Some(d) => format!("done {} {} next {}", g.sep, g.repeat, d.format("%a %b %-d")),
                    None => "done".into(),
                }
            }
        }
        "delete" => match c.children.filter(|k| *k > 0) {
            Some(k) => format!("deleted with {k} children"),
            None => "deleted".into(),
        },
        "restore" => "restored".into(),
        _ => review_fields_text(store, c, g, today),
    }
}

fn review_fields_text(store: &thc_core::store::Store, c: &thc_core::review::Change, g: &crate::theme::Glyphs, today: NaiveDate) -> String {
    let create = c.change == "create";
    let mut lines = review_field_lines(c, g, today);
    if create {
        // `in § today · due tomorrow · #reading`: place first, no status (the checkbox shows it).
        lines.retain(|(k, _, _)| k != "text" && k != "status");
    }
    let fields: Vec<String> = lines
        .into_iter()
        .map(|(k, old, new)| {
            let (old, new) = if k == "text" { (old.map(|o| store.render_text(&o)), store.render_text(&new)) } else { (old, new) };
            match old {
                Some(o) => format!("{k} {o} {} {new}", g.arrow),
                None if k == "in" => format!("in {new}"),
                None if create && k == "tags" => new.replace('+', ""),
                None if create && matches!(k.as_str(), "priority" | "repeat") => new,
                None => format!("{k} {new}"),
            }
        })
        .collect();
    fields.join(&format!(" {} ", g.sep))
}

pub fn review_marker(change: &str, g: &crate::theme::Glyphs) -> &'static str {
    let ascii = g.arrow != "→";
    match change {
        "create" | "restore" => "+",
        "move" => if ascii { ">" } else { "→" },
        "complete" => if ascii { "x" } else { "✓" },
        "delete" => if ascii { "-" } else { "−" },
        "alert" => if ascii { "(o)" } else { "◎" },
        _ => "~",
    }
}

// ---- in-place update (tui-editor.md §11) ---------------------------------------------------

/// Where the TUI was, carried across a re-exec into the new binary.
#[derive(serde::Serialize, serde::Deserialize, Default, Debug)]
pub struct Resume {
    pub view: String,
    pub selected: Option<String>,
    pub page_open: Option<String>,
    pub journal_date: Option<String>,
    pub tasks_filter: Option<String>,
    pub search_terms: Option<String>,
    pub updated_to: Option<String>,
}

impl App {
    /// Whether the thc on disk changed under this running one (checked at most once a minute,
    /// or now when `force`, e.g. the terminal regained focus). A different version there is
    /// said in the bar: `thc 0.9.35 installed · :update reloads here`.
    /// The registry changed under a running TUI (an agent's `thc vault new`, a rename, an rm):
    /// the other vaults are reopened as needed, the cross-vault views reload, and an open
    /// picker rebuilds in place, keeping its selection and any name being typed.
    /// A stat-cheap read every half second, daemon or not.
    pub fn check_registry(&mut self) {
        if self.registry_checked.elapsed() < std::time::Duration::from_millis(500) {
            return;
        }
        self.registry_checked = std::time::Instant::now();
        let reg = thc_core::registry::Registry::load();
        let sig = registry_sig(&reg);
        if sig == self.registry_sig {
            return;
        }
        self.registry_sig = sig;
        // Keep what's still registered at the same path (renamed: just the name), drop the rest,
        // open the new ones.
        let wanted: Vec<std::path::PathBuf> = reg.vaults.iter().map(|e| e.path.clone()).collect();
        self.others.retain(|o| wanted.contains(&o.path) && o.path.join(thc_core::vault::VAULT_MARKER).exists());
        if reg.vaults.len() < 2 {
            self.others.clear();
        }
        for o in self.others.iter_mut() {
            if let Some(e) = reg.by_path(&o.path) {
                o.name = thc_core::settings::load(Some(&o.path)).str("vault.name_short").map(str::to_string).unwrap_or_else(|| e.name.clone());
            }
        }
        let keep: Vec<std::path::PathBuf> = self.others.iter().map(|o| o.path.clone()).collect();
        let fresh = open_others_from(&self.vault, &reg, &keep);
        self.others.extend(fresh);
        let order = |p: &std::path::Path| reg.vaults.iter().position(|e| e.path == p).unwrap_or(usize::MAX);
        self.others.sort_by_key(|o| order(&o.path));
        let _ = self.reload();
        if let Some(Overlay::Vaults { rows, sel, naming }) = self.overlay.take() {
            let was = rows.get(sel).map(|r| r.path.clone());
            self.open_vault_picker();
            if let Some(Overlay::Vaults { rows, sel, naming: n }) = self.overlay.as_mut() {
                if let Some(i) = was.and_then(|p| rows.iter().position(|r| r.path == p)) {
                    *sel = i;
                }
                *sel = (*sel).min(rows.len().saturating_sub(1));
                *n = naming;
            }
        }
    }

    pub fn check_installed(&mut self, force: bool) {
        if !force && self.exe_checked.elapsed() < std::time::Duration::from_secs(60) {
            return;
        }
        self.exe_checked = std::time::Instant::now();
        let Ok(exe) = std::env::current_exe() else { return };
        let Some(m) = std::fs::metadata(&exe).ok().and_then(|m| m.modified().ok()) else { return };
        if self.exe_seen == Some(m) {
            return;
        }
        self.exe_seen = Some(m);
        let Ok(o) = std::process::Command::new(&exe).arg("--version").env("THC_NO_UPDATE_CHECK", "1").output() else { return };
        let v = String::from_utf8_lossy(&o.stdout).trim().trim_start_matches("thc ").to_string();
        if o.status.success() && !v.is_empty() && v != env!("CARGO_PKG_VERSION") {
            self.installed = Some(v);
        }
    }

    /// `:update`: run the verified `thc update` in the background; the bar shows progress.
    pub fn start_update(&mut self) {
        // Already installed under us: reload in place, nothing to download.
        if let Some(v) = self.installed.clone() {
            if self.edit.is_some() {
                return self.info("finish the line first (Esc), then :update");
            }
            self.update_state = UpdateState::Downloading { version: v };
            self.reexec = true;
            return;
        }
        if matches!(self.update_state, UpdateState::Downloading { .. }) {
            return self.info("already updating");
        }
        if self.edit.is_some() {
            return self.info("finish the line first (Esc), then :update");
        }
        let target = self.update_available.clone().unwrap_or_else(|| "the latest".into());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let r = (|| -> Result<String, String> {
                let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                let o = std::process::Command::new(exe).args(["--json", "update"]).env("THC_ACTOR", "human").output().map_err(|e| e.to_string())?;
                let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_default();
                if !o.status.success() {
                    let err: serde_json::Value = serde_json::from_slice(&o.stderr).unwrap_or_default();
                    let msg = err["error"]["message"].as_str().map(str::to_string).unwrap_or_else(|| String::from_utf8_lossy(&o.stderr).trim().trim_start_matches("thc: ").to_string());
                    return Err(msg);
                }
                match v["to"].as_str() {
                    Some(to) => Ok(to.to_string()),
                    None => Err(format!("already up to date ({})", v["current"].as_str().unwrap_or(env!("CARGO_PKG_VERSION")))),
                }
            })();
            let _ = tx.send(r);
        });
        self.update_rx = Some(rx);
        self.update_state = UpdateState::Downloading { version: target };
    }

    /// Poll the background update; on success ask the loop to re-exec.
    pub fn drain_update(&mut self) {
        let Some(rx) = &self.update_rx else { return };
        let Ok(r) = rx.try_recv() else { return };
        self.update_rx = None;
        match r {
            Ok(v) => {
                self.update_state = UpdateState::Downloading { version: v };
                self.reexec = true;
            }
            Err(e) if e.starts_with("already up to date") => {
                self.update_state = UpdateState::Idle;
                self.update_available = None;
                self.info(e);
            }
            Err(e) => self.update_state = UpdateState::Failed(e),
        }
    }

    /// `:changes`: the release notes the last check recorded.
    pub fn resume_state(&self, updated_to: Option<String>) -> Resume {
        Resume {
            view: format!("{:?}", self.view),
            selected: self.selected.clone(),
            page_open: self.page_open.clone(),
            journal_date: Some(ymd(self.journal_date)),
            tasks_filter: Some(self.tasks_filter.clone()),
            search_terms: Some(self.search_terms.clone()),
            updated_to,
        }
    }

    /// Back where we were before the re-exec, with the `updated to` toast.
    pub fn apply_resume(&mut self, r: Resume) {
        if let Some(f) = r.tasks_filter {
            self.tasks_filter = f;
        }
        if let Some(s) = r.search_terms {
            self.search_terms = s;
        }
        if let Some(d) = r.journal_date.and_then(|d| NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok()) {
            self.journal_date = d;
        }
        let view = VIEWS.iter().copied().find(|v| format!("{v:?}") == r.view).unwrap_or(View::Today);
        self.page_open = r.page_open;
        self.set_view(view);
        if let Some(id) = r.selected {
            self.selected = Some(id);
            self.restore_cursor_public();
        }
        if let Some(v) = r.updated_to {
            self.update_available = None;
            self.toast_parts(ToastKind::Info, vec![(format!("updated to {v}"), Token::Text), (" · what's new: :about".into(), Token::Muted)]);
        }
    }

    pub fn node_row_public(n: Node) -> Row {
        Self::node_row(n)
    }

    pub fn restore_cursor_public(&mut self) {
        self.restore_cursor();
    }
}

/// `[tui]` settings (thc_core::tui_config): read once at start.
pub type TuiPrefs = thc_core::tui_config::TuiConfig;

/// One row of the rail beside a document (navigation.md §3).
#[derive(Clone, Debug)]
pub enum RailItem {
    Page { id: String, title: String, open: usize },
    Day { date: NaiveDate, count: usize },
}

impl App {
    /// Idle time: lay the open page out ahead at the width a sidebar column would leave it, a
    /// slice at a time, so opening a panel beside a long page only sums rows it already has.
    /// Nothing on screen changes (it fills a cache). True: more to do.
    pub(crate) fn prewarm_step(&mut self, area: ratatui::layout::Rect, budget: std::time::Duration) -> bool {
        if self.sidebar_col.is_some() {
            return false;
        }
        let Some(g) = crate::ui::doc_geometry_beside(self, area) else { return false };
        let Some(d) = self.doc.as_mut() else { return false };
        let key = crate::doc_app::caret_key(&d.target);
        let next = match &self.prewarm {
            Some((k, pg, next)) if *k == key && *pg == g => match next {
                Some(n) => *n,
                None => return false,
            },
            _ => 0,
        };
        let left = d.prewarm_rows(&g, next, budget);
        self.prewarm = Some((key, g, left));
        left.is_some()
    }
}
