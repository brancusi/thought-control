//! The editor's whole state, split in two: a [`Document`] (the text, its marks, undo history and
//! outline) and a [`View`] of it (selection, scroll, viewport, folds). Several views can share
//! one document ([`crate::update_doc`]); [`State`] is one document seen through one view, the
//! single-view editor the protocol, traces and the `caretline` binary use.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::helix::graphemes::ensure_grapheme_boundary_prev;
use crate::helix::history::History;
use crate::helix::line_ending::auto_detect_line_ending;
use crate::helix::{ChangeSet, LineEnding, Range, Rope, Selection, SmallVec};
use crate::layout::{OutlineLayout, WrapCache};
use crate::marks::{Clipboard, MarkDelta, MarkId, Marks};
use crate::outline::{OutlineCache, OutlineConfig};

/// Document settings: how the text is edited and wrapped, whichever view shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Columns per tab stop.
    pub tab_width: u16,
    /// Wrap long lines at the viewport's width. Below 11 columns wrapping is off.
    pub soft_wrap: bool,
    /// The line ending inserted by Enter and used to normalize pasted text.
    pub line_ending: LineEnding,
    /// What a change from elsewhere ([`crate::Msg::External`]) does to the undo history.
    #[serde(default, skip_serializing_if = "ExternalUndo::is_default")]
    pub external_undo: ExternalUndo,
}

impl Default for Config {
    fn default() -> Self {
        Config { tab_width: 4, soft_wrap: true, line_ending: LineEnding::LF, external_undo: ExternalUndo::default() }
    }
}

/// How undo treats a change from elsewhere.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalUndo {
    /// The history is transformed over the change: undo takes back only local edits and
    /// keeps the change (operational transform).
    #[default]
    Transform,
    /// A fallback: the change trims the history before it, so undo stops there.
    Barrier,
}

impl ExternalUndo {
    fn is_default(&self) -> bool {
        *self == ExternalUndo::Transform
    }
}

/// View settings: how one view draws and follows the caret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewConfig {
    /// Draw the status bar on the last row. Off, every row shows text (for embedders and
    /// panels that show their own chrome).
    pub status_bar: bool,
    /// Rows kept between the caret and the top or bottom edge when scrolling.
    pub scrolloff: u16,
    /// How the view scrolls to keep the caret in sight.
    #[serde(default, skip_serializing_if = "Follow::is_default")]
    pub follow: Follow,
}

impl Default for ViewConfig {
    fn default() -> Self {
        ViewConfig { status_bar: true, scrolloff: 2, follow: Follow::Margin }
    }
}

/// How a view follows the caret after a message.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Follow {
    /// Scroll the least needed, keeping `scrolloff` rows of margin.
    #[default]
    Margin,
    /// Keep the caret's row at `percent` of the view's height (typewriter scrolling).
    Typewriter { percent: u8 },
}

impl Follow {
    fn is_default(&self) -> bool {
        *self == Follow::Margin
    }
}

/// The terminal area the editor draws into: text rows plus one status row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Viewport {
    pub width: u16,
    pub height: u16,
}

impl Viewport {
    /// Rows available for text (the last row is the status bar).
    pub fn text_rows(&self) -> usize {
        self.height.saturating_sub(1) as usize
    }
}

/// Where the view starts: a document line, the visual row within it (when the line is
/// wrapped), and a column offset (only when wrapping is off).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scroll {
    pub line: usize,
    pub row: usize,
    pub col: usize,
}

/// Which consecutive edits merge into one undo step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    Typing,
    DeleteBackward,
    DeleteForward,
}

/// An open run of edits: the next edit of the same kind within [`RUN_GAP_MS`] amends
/// the same history revision, until the run is [`RUN_MAX_CHARS`] long (or, for typing,
/// [`RUN_WORD_BREAK_CHARS`] long and the next text starts a new word).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditRun {
    pub kind: RunKind,
    pub revision: usize,
    pub at_ms: u64,
    /// Characters inserted or deleted so far in this run.
    #[serde(default)]
    pub chars: usize,
    /// The view it is typed in (its index in `update_doc`'s views): an edit through another
    /// view starts a new undo step.
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub view: usize,
}

fn is_zero_u16(n: &u16) -> bool {
    *n == 0
}

fn is_zero_usize(n: &usize) -> bool {
    *n == 0
}

/// Edits further apart than this start a new undo step.
pub const RUN_GAP_MS: u64 = 1500;

/// A run never grows past this many characters: the next edit starts a new undo step. This
/// also bounds the cost of amending a revision, which is linear in the run's length.
pub const RUN_MAX_CHARS: usize = 256;

/// Once a typing run holds this many characters, text that starts with whitespace (the
/// start of the next word) begins a new undo step.
pub const RUN_WORD_BREAK_CHARS: usize = 128;

/// One document: the text and everything every view of it shares (marks, undo history,
/// the outline, the clipboard register, the clock, the save state).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Document {
    /// The text. Change it through [`crate::update`]; after assigning it directly, call
    /// [`State::sanitize`] (or [`View::fit`] on every view).
    #[serde(serialize_with = "rope_as_string::serialize")]
    pub text: Rope,
    /// Undo history (Helix's revision tree).
    pub history: History,
    /// The history revision last written to disk.
    pub saved_revision: Option<usize>,
    /// A save in flight: the revision being written.
    pub saving: Option<usize>,
    /// Whether the document differs from the last save.
    pub dirty: bool,
    /// The file the document saves to, if any.
    pub path: Option<String>,
    pub config: Config,
    /// The internal clipboard register (the last copy or cut, with the marks a cut took).
    pub clipboard: Clipboard,
    /// The clock, as last reported by a `Tick` message (milliseconds).
    pub now_ms: u64,
    /// The open edit run, if any.
    pub run: Option<EditRun>,
    /// Block marks: numeric ids at line starts, mapped through every edit
    /// ([`crate::marks`]). Empty unless a host or the outline layer adds some.
    #[serde(skip_serializing_if = "Marks::is_unused")]
    pub marks: Marks,
    /// What each history revision did to the marks, indexed by revision (so undo and redo
    /// restore them exactly). Always as long as the history.
    #[serde(skip_serializing_if = "mark_log_is_empty")]
    pub mark_log: Vec<MarkDelta>,
    /// Set for an outline document: blocks bounded by marks, and the outline's editing
    /// rules ([`crate::outline`]). Absent, the document is plain text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outline: Option<OutlineConfig>,
    /// Goes up by one for every change of the text or the marks, from any view or from
    /// elsewhere ([`crate::Msg::External`]).
    #[serde(rename = "doc_rev", skip_serializing_if = "is_zero")]
    pub rev: u64,
    /// The history was trimmed at a change from elsewhere ([`ExternalUndo::Barrier`]):
    /// undo stops there.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub undo_floor: bool,
    /// The outline derived from the text and marks: a memo, not part of the value.
    #[serde(skip)]
    pub(crate) derived: OutlineCache,
    /// Counts recorded edits (a change of text or marks), for `update`'s own bookkeeping.
    #[serde(skip)]
    pub(crate) edits: EditCount,
    /// The changes applied to the text during the current message, for rebasing the other
    /// views. Emptied by every update.
    #[serde(skip)]
    pub(crate) journal: Journal,
    /// Where the text changed since a host last asked ([`Document::take_touched`]).
    #[serde(skip)]
    pub(crate) touched: Touched,
}

/// One view of a [`Document`]: where its carets are, what it shows, and how.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct View {
    /// The selection: one or more ranges, each an anchor and a head (the caret). A range's
    /// `old_visual_position` holds the goal column for vertical motion.
    pub selection: Selection,
    pub scroll: Scroll,
    pub viewport: Viewport,
    pub config: ViewConfig,
    /// A one-line message for the status bar, cleared by the next input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// A first quit with unsaved changes arms this; a second quit then exits.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub quit_armed: bool,
    /// A double-click's word, while a shift-click or drag may extend it by words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub word_drag: Option<(usize, usize)>,
    /// Folded blocks: their children are hidden in this view (outline documents with a
    /// layout).
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub folds: BTreeSet<MarkId>,
    /// A read-only view: editing messages are refused ([`crate::Effect::Refused`]); motion,
    /// selection and copy still work.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,
    /// Whether the view has focus: only a focused view draws its caret.
    #[serde(skip_serializing_if = "is_true")]
    pub focused: bool,
    /// Scrolled freely (the wheel, [`crate::Msg::ScrollView`]): the view doesn't follow the
    /// caret until the next caret motion or edit.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub free: bool,
    /// The outline layout: markers in a hang, nested columns, folds. Absent, the text is drawn
    /// as it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<OutlineLayout>,
    /// The frame clock this view asks of its runtime, in frames per second: 0 (the default)
    /// for none. Set with [`crate::Msg::FrameClock`]; a runtime sends [`crate::Msg::Frame`]
    /// at this rate while it is set. See [`View::frame_rate`].
    #[serde(skip_serializing_if = "is_zero_u16")]
    pub frame_clock: u16,
    /// Where long lines' rows start: a layout memo, not part of the view's value.
    #[serde(skip)]
    pub(crate) wrap: WrapCache,
}

impl Default for View {
    fn default() -> Self {
        View::new(Viewport { width: 80, height: 24 })
    }
}

impl View {
    /// A view with a caret at the start, the default config and no folds.
    pub fn new(viewport: Viewport) -> View {
        View {
            selection: Selection::point(0),
            scroll: Scroll::default(),
            viewport: Viewport { width: viewport.width.max(1), height: viewport.height.max(1) },
            config: ViewConfig::default(),
            status: None,
            quit_armed: false,
            word_drag: None,
            folds: BTreeSet::new(),
            read_only: false,
            focused: true,
            free: false,
            layout: None,
            frame_clock: 0,
            wrap: WrapCache::default(),
        }
    }

    /// The frames per second this view needs from a runtime's frame clock, if any: what
    /// [`crate::Msg::FrameClock`] asked for. A runtime sends [`crate::Msg::Frame`] at this
    /// rate while it is `Some`, and none otherwise, so plain editing costs no frames.
    pub fn frame_rate(&self) -> Option<u16> {
        (self.frame_clock > 0).then_some(self.frame_clock)
    }

    /// The same view, read-only.
    pub fn read_only(mut self, on: bool) -> View {
        self.read_only = on;
        self
    }

    /// Rows available for text: the viewport, less the status bar when it is shown.
    pub fn text_rows(&self) -> usize {
        if self.config.status_bar {
            self.viewport.text_rows()
        } else {
            self.viewport.height as usize
        }
    }

    /// The primary caret.
    pub fn caret(&self) -> usize {
        self.selection.primary().head
    }

    /// Clamps the view to `doc`: selections inside the text and on grapheme boundaries, the
    /// scroll on a line, folds on live marks. A no-op on any view `update_doc` left.
    pub fn fit(&mut self, doc: &Document) {
        let text = doc.text.slice(..);
        let len = text.len_chars();
        let fix = |pos: usize| ensure_grapheme_boundary_prev(text, pos.min(len));
        let mut ranges: SmallVec<[Range; 1]> = self
            .selection
            .ranges()
            .iter()
            .map(|r| Range { anchor: fix(r.anchor), head: fix(r.head), old_visual_position: r.old_visual_position })
            .collect();
        if ranges.is_empty() {
            ranges.push(Range::point(0));
        }
        let primary = self.selection.primary_index().min(ranges.len() - 1);
        let selection = Selection::new(ranges, primary);
        if selection != self.selection {
            self.selection = selection;
        }
        self.viewport.width = self.viewport.width.max(1);
        self.viewport.height = self.viewport.height.max(1);
        let last_line = doc.text.len_lines().saturating_sub(1);
        if self.scroll.line > last_line {
            self.scroll = Scroll::default();
        }
        self.folds.retain(|id| doc.marks.contains(*id));
        if let Some((a, b)) = self.word_drag {
            if a > b || b > len {
                self.word_drag = None;
            }
        }
    }
}

/// One document seen through one view: the single-view editor. Serializes to JSON and back
/// without loss, with the document's and the view's fields side by side; [`crate::update`] and
/// [`crate::view`] are pure functions of it.
///
/// When deserializing, only what a client would know is needed: every field is optional
/// (see [`StateInput`]), so `{"text":"hello"}` is a working state with a fresh history.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    pub doc: Document,
    pub view: View,
}

impl Default for Document {
    fn default() -> Self {
        Document::new("", None)
    }
}

impl Document {
    /// A fresh document for `text`, counted as saved.
    pub fn new(text: &str, path: Option<String>) -> Document {
        let rope = Rope::from(text);
        let line_ending = auto_detect_line_ending(&rope).unwrap_or(LineEnding::LF);
        let mut doc = Document {
            text: rope,
            history: History::default(),
            saved_revision: Some(0),
            saving: None,
            dirty: false,
            path,
            config: Config { line_ending, ..Config::default() },
            clipboard: Clipboard::default(),
            now_ms: 0,
            run: None,
            marks: Marks::default(),
            mark_log: vec![MarkDelta::default()],
            outline: None,
            rev: 0,
            undo_floor: false,
            derived: OutlineCache::default(),
            edits: EditCount::default(),
            journal: Journal::default(),
            touched: Touched::all(),
        };
        doc.dirty = doc.compute_dirty();
        doc
    }

    pub fn compute_dirty(&self) -> bool {
        self.saved_revision != Some(self.history.current_revision())
    }

    /// Keeps the mark log exactly as long as the history (a state from an older version, or
    /// written by hand, may have none).
    pub(crate) fn fit_mark_log(&mut self) {
        self.mark_log.resize(self.history.len(), MarkDelta::default());
    }

    /// Repairs what a hand-edited state could get wrong: a history whose current revision
    /// doesn't exist, marks off line starts. Recomputes `dirty`.
    pub fn sanitize(&mut self) {
        self.config.tab_width = self.config.tab_width.max(1);
        if self.history.current_revision() >= self.history.len() {
            self.history = History::default();
            self.mark_log.clear();
        }
        self.marks.repair(self.text.slice(..));
        self.fit_mark_log();
        self.derived.clear();
        self.touched = Touched::all();
        if self.outline.is_some() {
            crate::outline::mint_missing(self);
        }
        self.dirty = self.compute_dirty();
    }

    /// The display name for the status bar.
    pub fn name(&self) -> String {
        match &self.path {
            Some(p) => std::path::Path::new(p)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.clone()),
            None => "[scratch]".to_string(),
        }
    }
}

impl<'de> Deserialize<'de> for Document {
    /// A document reads as a [`State`] does (every field optional), keeping the document.
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        StateInput::deserialize(d).map(|i| State::from(i).doc)
    }
}

/// The deserialized form of [`State`]: every field optional. Missing fields get what
/// [`State::new`] would give: an empty text, a caret at 0, an 80x24 viewport, a fresh
/// history, the default config (line ending detected from the text), and a document that
/// counts as saved at the current history revision (`dirty` is always recomputed). Parse
/// through [`State::from_json`] (or [`State::sanitize`] after) to repair out-of-range values.
#[derive(Debug, Clone, Deserialize)]
pub struct StateInput {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub selection: Option<Selection>,
    #[serde(default)]
    pub scroll: Scroll,
    #[serde(default)]
    pub viewport: Option<Viewport>,
    #[serde(default)]
    pub clipboard: Clipboard,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub history: Option<History>,
    /// Absent: saved at the current revision. `null`: never saved.
    #[serde(default, deserialize_with = "present")]
    pub saved_revision: Option<Option<usize>>,
    #[serde(default)]
    pub saving: Option<usize>,
    /// Ignored: recomputed from `saved_revision`.
    #[serde(default)]
    pub dirty: Option<bool>,
    #[serde(default)]
    pub config: Option<ConfigInput>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub now_ms: u64,
    #[serde(default)]
    pub run: Option<EditRun>,
    #[serde(default)]
    pub quit_armed: bool,
    #[serde(default)]
    pub marks: Marks,
    #[serde(default)]
    pub mark_log: Vec<MarkDelta>,
    #[serde(default)]
    pub outline: Option<OutlineConfig>,
    #[serde(default)]
    pub word_drag: Option<(usize, usize)>,
    #[serde(default)]
    pub doc_rev: u64,
    #[serde(default)]
    pub undo_floor: bool,
    #[serde(default)]
    pub folds: BTreeSet<MarkId>,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default = "yes")]
    pub focused: bool,
    #[serde(default)]
    pub free: bool,
    #[serde(default)]
    pub layout: Option<OutlineLayout>,
    #[serde(default)]
    pub frame_clock: u16,
}

/// The deserialized form of the `config` inside a [`StateInput`]: the document's and the
/// view's settings side by side, every one optional, with the defaults, except the line
/// ending, which is detected from the text.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConfigInput {
    pub tab_width: Option<u16>,
    pub soft_wrap: Option<bool>,
    pub scrolloff: Option<u16>,
    pub line_ending: Option<LineEnding>,
    pub status_bar: Option<bool>,
    pub follow: Option<Follow>,
    pub external_undo: Option<ExternalUndo>,
}

/// Tells a field that is present (even as `null`) from one that is absent.
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl From<StateInput> for State {
    fn from(input: StateInput) -> State {
        let text = Rope::from(input.text.as_str());
        let d = Config::default();
        let vd = ViewConfig::default();
        let c = input.config.unwrap_or_default();
        let config = Config {
            tab_width: c.tab_width.unwrap_or(d.tab_width),
            soft_wrap: c.soft_wrap.unwrap_or(d.soft_wrap),
            line_ending: c.line_ending.or_else(|| auto_detect_line_ending(&text)).unwrap_or(d.line_ending),
            external_undo: c.external_undo.unwrap_or_default(),
        };
        let view_config = ViewConfig {
            status_bar: c.status_bar.unwrap_or(vd.status_bar),
            scrolloff: c.scrolloff.unwrap_or(vd.scrolloff),
            follow: c.follow.unwrap_or_default(),
        };
        let history = input.history.unwrap_or_default();
        let saved_revision = input.saved_revision.unwrap_or(Some(history.current_revision()));
        let doc = Document {
            text,
            history,
            saved_revision,
            saving: input.saving,
            dirty: false,
            path: input.path,
            config,
            clipboard: input.clipboard,
            now_ms: input.now_ms,
            run: input.run,
            marks: input.marks,
            mark_log: input.mark_log,
            outline: input.outline,
            rev: input.doc_rev,
            undo_floor: input.undo_floor,
            derived: OutlineCache::default(),
            edits: EditCount::default(),
            journal: Journal::default(),
            touched: Touched::all(),
        };
        let view = View {
            selection: input.selection.unwrap_or_else(|| Selection::point(0)),
            scroll: input.scroll,
            viewport: input.viewport.unwrap_or(Viewport { width: 80, height: 24 }),
            config: view_config,
            status: input.status,
            quit_armed: input.quit_armed,
            word_drag: input.word_drag,
            folds: input.folds,
            read_only: input.read_only,
            focused: input.focused,
            free: input.free,
            layout: input.layout,
            frame_clock: input.frame_clock,
            wrap: WrapCache::default(),
        };
        let mut state = State { doc, view };
        state.doc.marks.repair(state.doc.text.slice(..));
        state.doc.fit_mark_log();
        if state.doc.outline.is_some() {
            crate::outline::mint_missing(&mut state.doc);
        }
        state.doc.dirty = state.doc.compute_dirty();
        state
    }
}

impl<'de> Deserialize<'de> for State {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        StateInput::deserialize(d).map(State::from)
    }
}

/// The serialized form of [`State`]: the document's and the view's fields side by side, as
/// one object (the shape every earlier state has).
#[derive(Serialize)]
struct StateOut<'a> {
    #[serde(serialize_with = "rope_as_string::serialize")]
    text: &'a Rope,
    selection: &'a Selection,
    scroll: &'a Scroll,
    viewport: &'a Viewport,
    clipboard: &'a Clipboard,
    path: &'a Option<String>,
    /// Left out (with `saving` and `run`) in a state without its history.
    #[serde(skip_serializing_if = "Option::is_none")]
    history: Option<&'a History>,
    saved_revision: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    saving: Option<Option<usize>>,
    dirty: bool,
    config: ConfigOut<'a>,
    status: &'a Option<String>,
    now_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    run: Option<&'a Option<EditRun>>,
    quit_armed: bool,
    #[serde(skip_serializing_if = "Marks::is_unused")]
    marks: &'a Marks,
    #[serde(skip_serializing_if = "mark_log_is_empty")]
    mark_log: &'a [MarkDelta],
    #[serde(skip_serializing_if = "Option::is_none")]
    outline: &'a Option<OutlineConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    word_drag: &'a Option<(usize, usize)>,
    #[serde(skip_serializing_if = "is_zero")]
    doc_rev: u64,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    undo_floor: bool,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    folds: &'a BTreeSet<MarkId>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    read_only: bool,
    #[serde(skip_serializing_if = "is_true")]
    focused: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    free: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    layout: &'a Option<OutlineLayout>,
    #[serde(skip_serializing_if = "is_zero_u16")]
    frame_clock: u16,
}

#[derive(Serialize)]
struct ConfigOut<'a> {
    tab_width: u16,
    soft_wrap: bool,
    scrolloff: u16,
    line_ending: LineEnding,
    status_bar: bool,
    #[serde(skip_serializing_if = "Follow::is_default")]
    follow: Follow,
    #[serde(skip_serializing_if = "ExternalUndo::is_default")]
    external_undo: &'a ExternalUndo,
}

impl Serialize for State {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.state_out(true).serialize(s)
    }
}

/// A [`State`] serialized without its undo history ([`State::without_history`]).
pub struct WithoutHistory<'a>(&'a State);

impl Serialize for WithoutHistory<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.state_out(false).serialize(s)
    }
}

/// The undo history of a [`State`] and the fields that refer to its revisions: what
/// [`State::without_history`] leaves out. Its fields set over a state without its history
/// give the whole state back.
#[derive(Serialize)]
pub struct HistoryPart<'a> {
    pub history: &'a History,
    pub saved_revision: Option<usize>,
    pub saving: Option<usize>,
    pub run: &'a Option<EditRun>,
    #[serde(skip_serializing_if = "mark_log_is_empty")]
    pub mark_log: &'a [MarkDelta],
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub undo_floor: bool,
}

impl State {
    /// The state as JSON without the undo history: `history`, `saving`, `run`, `mark_log` and
    /// `undo_floor` are left out, and `saved_revision` is 0 when the document is clean and
    /// null when it is dirty. Parsed back, it is the same document and view with a fresh
    /// history that keeps `dirty`. [`State::history_part`] has what it leaves out.
    pub fn without_history(&self) -> WithoutHistory<'_> {
        WithoutHistory(self)
    }

    /// What [`State::without_history`] leaves out.
    pub fn history_part(&self) -> HistoryPart<'_> {
        let d = &self.doc;
        HistoryPart {
            history: &d.history,
            saved_revision: d.saved_revision,
            saving: d.saving,
            run: &d.run,
            mark_log: &d.mark_log,
            undo_floor: d.undo_floor,
        }
    }

    fn state_out(&self, history: bool) -> StateOut<'_> {
        let (d, v) = (&self.doc, &self.view);
        let empty: &'static [MarkDelta] = &[];
        StateOut {
            text: &d.text,
            selection: &v.selection,
            scroll: &v.scroll,
            viewport: &v.viewport,
            clipboard: &d.clipboard,
            path: &d.path,
            history: history.then_some(&d.history),
            // A fresh history's only revision is 0: saved there when clean, never when dirty.
            saved_revision: if history { d.saved_revision } else { (!d.dirty).then_some(0) },
            saving: history.then_some(d.saving),
            dirty: d.dirty,
            config: ConfigOut {
                tab_width: d.config.tab_width,
                soft_wrap: d.config.soft_wrap,
                scrolloff: v.config.scrolloff,
                line_ending: d.config.line_ending,
                status_bar: v.config.status_bar,
                follow: v.config.follow,
                external_undo: &d.config.external_undo,
            },
            status: &v.status,
            now_ms: d.now_ms,
            run: history.then_some(&d.run),
            quit_armed: v.quit_armed,
            marks: &d.marks,
            mark_log: if history { &d.mark_log } else { empty },
            outline: &d.outline,
            word_drag: &v.word_drag,
            doc_rev: d.rev,
            undo_floor: history && d.undo_floor,
            folds: &v.folds,
            read_only: v.read_only,
            focused: v.focused,
            free: v.free,
            layout: &v.layout,
            frame_clock: v.frame_clock,
        }
    }
}

impl State {
    /// A fresh state for `text`. A file that doesn't exist yet counts as saved (empty).
    pub fn new(text: &str, path: Option<String>, viewport: Viewport) -> State {
        State { doc: Document::new(text, path), view: View::new(viewport) }
    }

    /// One document and one view of it.
    pub fn from_parts(doc: Document, view: View) -> State {
        State { doc, view }
    }

    pub fn into_parts(self) -> (Document, View) {
        (self.doc, self.view)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("state serializes")
    }

    /// Parses a state and repairs anything a hand-edited file could get wrong (selections
    /// out of bounds or off grapheme boundaries, a zero-sized viewport).
    pub fn from_json(json: &str) -> Result<State, serde_json::Error> {
        let mut state: State = serde_json::from_str(json)?;
        state.sanitize();
        Ok(state)
    }

    /// Clamps the selection and view to the document. A no-op on any state `update` made
    /// (apart from forgetting the layout memo, which changes nothing visible).
    pub fn sanitize(&mut self) {
        self.doc.sanitize();
        self.view.wrap.clear();
        self.view.fit(&self.doc);
        self.outline_changed();
    }

    /// Keeps the mark log exactly as long as the history.
    pub(crate) fn fit_mark_log(&mut self) {
        self.doc.fit_mark_log();
    }

    pub fn compute_dirty(&self) -> bool {
        self.doc.compute_dirty()
    }

    /// Rows available for text: the viewport, less the status bar when it is shown.
    pub fn text_rows(&self) -> usize {
        self.view.text_rows()
    }

    /// The primary caret.
    pub fn caret(&self) -> usize {
        self.view.caret()
    }

    /// The display name for the status bar.
    pub fn name(&self) -> String {
        self.doc.name()
    }
}

/// A counter that is not part of the state's value (it compares equal to any other).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct EditCount(pub u64);

impl PartialEq for EditCount {
    fn eq(&self, _: &EditCount) -> bool {
        true
    }
}

/// The chars of the current text that changed since a host last asked: a range, or
/// everything (a new or repaired document). A memo for hosts that mirror the text (they
/// re-read only what changed), not part of the value.
#[derive(Debug, Clone)]
pub struct Touched {
    range: Option<(usize, usize)>,
    all: bool,
}

impl Default for Touched {
    /// Unknown history: everything.
    fn default() -> Touched {
        Touched::all()
    }
}

impl PartialEq for Touched {
    fn eq(&self, _: &Touched) -> bool {
        true
    }
}

impl Touched {
    pub(crate) fn all() -> Touched {
        Touched { range: None, all: true }
    }

    /// Notes a change applied to the text: the range so far is mapped through it, then
    /// joined with what it changed (in the new text's chars).
    pub(crate) fn note(&mut self, cs: &ChangeSet) {
        if !self.all && !cs.is_empty() {
            self.range = Some(changed_span(self.range, cs));
        }
    }
}

/// The chars that differ after `cs` from a text whose chars `range` already differed from
/// another: `range` mapped through `cs`, joined with what `cs` changed (new text's chars).
pub(crate) fn changed_span(range: Option<(usize, usize)>, cs: &ChangeSet) -> (usize, usize) {
    use crate::helix::transaction::{Assoc, Operation};
    let mut pos = 0usize;
    let mut span: Option<(usize, usize)> = None;
    for op in cs.changes() {
        match op {
            Operation::Retain(n) => pos += n,
            Operation::Delete(_) => {
                let (a, b) = span.unwrap_or((pos, pos));
                span = Some((a.min(pos), b.max(pos)));
            }
            Operation::Insert(t) => {
                let end = pos + t.chars().count();
                let (a, b) = span.unwrap_or((pos, end));
                span = Some((a.min(pos), b.max(end)));
                pos = end;
            }
        }
    }
    let mapped = range.map(|(x, y)| (cs.map_pos(x, Assoc::Before), cs.map_pos(y, Assoc::After)));
    match (mapped, span) {
        (Some((x, y)), Some((a, b))) => (x.min(a), y.max(b)),
        (Some(r), None) | (None, Some(r)) => r,
        (None, None) => (pos, pos),
    }
}

impl Document {
    /// The chars of the text that changed since the last call, as `[from, to)` in the current
    /// text (`to` may reach the end): `None` when nothing did, the whole text after a load or a
    /// repair. For a host that mirrors the text and re-reads only what changed.
    pub fn take_touched(&mut self) -> Option<(usize, usize)> {
        let t = std::mem::replace(&mut self.touched, Touched { range: None, all: false });
        let n = self.text.len_chars();
        if t.all {
            return Some((0, n));
        }
        t.range.map(|(a, b)| (a.min(n), b.min(n)))
    }

    /// Everything counts as changed for the next [`Document::take_touched`] (after the text
    /// or marks were set directly).
    pub fn touch_all(&mut self) {
        self.touched = Touched::all();
    }
}

/// The text changes of the message being applied: not part of the value.
#[derive(Debug, Clone, Default)]
pub(crate) struct Journal(pub Vec<ChangeSet>);

impl PartialEq for Journal {
    fn eq(&self, _: &Journal) -> bool {
        true
    }
}

fn mark_log_is_empty(log: &[MarkDelta]) -> bool {
    log.iter().all(MarkDelta::is_empty)
}

mod rope_as_string {
    use crate::helix::Rope;
    use serde::Serializer;

    pub fn serialize<S: Serializer>(rope: &Rope, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(rope)
    }
}
