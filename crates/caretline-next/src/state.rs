//! The editor's whole state: everything needed to reproduce the screen and the behaviour.

use serde::{Deserialize, Serialize};

use crate::helix::graphemes::ensure_grapheme_boundary_prev;
use crate::helix::history::History;
use crate::helix::line_ending::auto_detect_line_ending;
use crate::helix::{LineEnding, Range, Rope, Selection, SmallVec};
use crate::layout::WrapCache;
use crate::marks::{Clipboard, MarkDelta, Marks};
use crate::outline::{OutlineCache, OutlineConfig};

/// Editor settings that change behaviour or layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Columns per tab stop.
    pub tab_width: u16,
    /// Wrap long lines at the viewport's width. Below 11 columns wrapping is off.
    pub soft_wrap: bool,
    /// Rows kept between the caret and the top or bottom edge when scrolling.
    pub scrolloff: u16,
    /// The line ending inserted by Enter and used to normalize pasted text.
    pub line_ending: LineEnding,
    /// Draw the status bar on the last row. Off, every row shows text (for embedders and
    /// panels that show their own chrome).
    #[serde(default = "yes")]
    pub status_bar: bool,
}

fn yes() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Config {
            tab_width: 4,
            soft_wrap: true,
            scrolloff: 2,
            line_ending: LineEnding::LF,
            status_bar: true,
        }
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
}

/// Edits further apart than this start a new undo step.
pub const RUN_GAP_MS: u64 = 1500;

/// A run never grows past this many characters: the next edit starts a new undo step. This
/// also bounds the cost of amending a revision, which is linear in the run's length.
pub const RUN_MAX_CHARS: usize = 256;

/// Once a typing run holds this many characters, text that starts with whitespace (the
/// start of the next word) begins a new undo step.
pub const RUN_WORD_BREAK_CHARS: usize = 128;

/// The editor state. Serializes to JSON and back without loss; [`crate::update`] and
/// [`crate::view`] are pure functions of it.
///
/// When deserializing, only what a client would know is needed: every field is optional
/// (see [`StateInput`]), so `{"text":"hello"}` is a working state with a fresh history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "StateInput")]
pub struct State {
    /// The document. Change it through [`crate::update`]; after assigning it directly, call
    /// [`State::sanitize`].
    #[serde(serialize_with = "rope_as_string::serialize")]
    pub text: Rope,
    /// The selection: one or more ranges, each an anchor and a head (the caret). A range's
    /// `old_visual_position` holds the goal column for vertical motion.
    pub selection: Selection,
    pub scroll: Scroll,
    pub viewport: Viewport,
    /// The internal clipboard register (the last copy or cut, with the marks a cut took).
    /// Serializes as a plain string when it carries no marks.
    pub clipboard: Clipboard,
    /// The file the document saves to, if any.
    pub path: Option<String>,
    /// Undo history (Helix's revision tree).
    pub history: History,
    /// The history revision last written to disk.
    pub saved_revision: Option<usize>,
    /// A save in flight: the revision being written.
    pub saving: Option<usize>,
    /// Whether the document differs from the last save.
    pub dirty: bool,
    pub config: Config,
    /// A one-line message for the status bar, cleared by the next input.
    pub status: Option<String>,
    /// The clock, as last reported by a `Tick` message (milliseconds).
    pub now_ms: u64,
    /// The open edit run, if any.
    pub run: Option<EditRun>,
    /// A first quit with unsaved changes arms this; a second quit then exits.
    pub quit_armed: bool,
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
    /// A double-click's word, while a shift-click or drag may extend it by words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub word_drag: Option<(usize, usize)>,
    /// The outline derived from the text and marks: a memo, not part of the value.
    #[serde(skip)]
    pub(crate) derived: OutlineCache,
    /// Counts recorded edits (a change of text or marks), for `update`'s own bookkeeping.
    #[serde(skip)]
    pub(crate) edits: EditCount,
    /// Where long lines' rows start: a layout memo, not part of the state's value.
    #[serde(skip)]
    pub(crate) wrap: WrapCache,
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
}

/// The deserialized form of [`Config`] inside a [`StateInput`]: every field optional, with
/// [`Config::default`]'s values, except the line ending, which is detected from the text.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConfigInput {
    pub tab_width: Option<u16>,
    pub soft_wrap: Option<bool>,
    pub scrolloff: Option<u16>,
    pub line_ending: Option<LineEnding>,
    pub status_bar: Option<bool>,
}

/// Tells a field that is present (even as `null`) from one that is absent.
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

impl From<StateInput> for State {
    fn from(input: StateInput) -> State {
        let text = Rope::from(input.text.as_str());
        let defaults = Config::default();
        let c = input.config.unwrap_or_default();
        let config = Config {
            tab_width: c.tab_width.unwrap_or(defaults.tab_width),
            soft_wrap: c.soft_wrap.unwrap_or(defaults.soft_wrap),
            scrolloff: c.scrolloff.unwrap_or(defaults.scrolloff),
            line_ending: c
                .line_ending
                .or_else(|| auto_detect_line_ending(&text))
                .unwrap_or(defaults.line_ending),
            status_bar: c.status_bar.unwrap_or(defaults.status_bar),
        };
        let history = input.history.unwrap_or_default();
        let saved_revision = input
            .saved_revision
            .unwrap_or(Some(history.current_revision()));
        let mut state = State {
            text,
            selection: input.selection.unwrap_or_else(|| Selection::point(0)),
            scroll: input.scroll,
            viewport: input.viewport.unwrap_or(Viewport { width: 80, height: 24 }),
            clipboard: input.clipboard,
            path: input.path,
            history,
            saved_revision,
            saving: input.saving,
            dirty: false,
            config,
            status: input.status,
            now_ms: input.now_ms,
            run: input.run,
            quit_armed: input.quit_armed,
            marks: input.marks,
            mark_log: input.mark_log,
            outline: input.outline,
            word_drag: input.word_drag,
            derived: OutlineCache::default(),
            edits: EditCount::default(),
            wrap: WrapCache::default(),
        };
        state.marks.repair(state.text.slice(..));
        state.fit_mark_log();
        if state.outline.is_some() {
            crate::outline::mint_missing(&mut state);
        }
        state.dirty = state.compute_dirty();
        state
    }
}

impl State {
    /// A fresh state for `text`. A file that doesn't exist yet counts as saved (empty).
    pub fn new(text: &str, path: Option<String>, viewport: Viewport) -> State {
        let rope = Rope::from(text);
        let line_ending = auto_detect_line_ending(&rope).unwrap_or(LineEnding::LF);
        let mut state = State {
            text: rope,
            selection: Selection::point(0),
            scroll: Scroll::default(),
            viewport: Viewport {
                width: viewport.width.max(1),
                height: viewport.height.max(1),
            },
            clipboard: Clipboard::default(),
            path,
            history: History::default(),
            saved_revision: Some(0),
            saving: None,
            dirty: false,
            config: Config {
                line_ending,
                ..Config::default()
            },
            status: None,
            now_ms: 0,
            run: None,
            quit_armed: false,
            marks: Marks::default(),
            mark_log: vec![MarkDelta::default()],
            outline: None,
            word_drag: None,
            derived: OutlineCache::default(),
            edits: EditCount::default(),
            wrap: WrapCache::default(),
        };
        state.dirty = state.compute_dirty();
        state
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
        self.wrap.clear();
        let text = self.text.slice(..);
        let len = text.len_chars();
        let fix = |pos: usize| ensure_grapheme_boundary_prev(text, pos.min(len));
        let mut ranges: SmallVec<[Range; 1]> = self
            .selection
            .ranges()
            .iter()
            .map(|r| Range {
                anchor: fix(r.anchor),
                head: fix(r.head),
                old_visual_position: r.old_visual_position,
            })
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
        self.config.tab_width = self.config.tab_width.max(1);
        let last_line = self.text.len_lines().saturating_sub(1);
        if self.scroll.line > last_line {
            self.scroll = Scroll::default();
        }
        if self.history.current_revision() >= self.history.len() {
            self.history = History::default();
            self.mark_log.clear();
        }
        self.marks.repair(self.text.slice(..));
        self.fit_mark_log();
        self.outline_changed();
        if let Some((a, b)) = self.word_drag {
            if a > b || b > len {
                self.word_drag = None;
            }
        }
        self.dirty = self.compute_dirty();
    }

    /// Keeps the mark log exactly as long as the history (a state from an older version, or
    /// written by hand, may have none).
    pub(crate) fn fit_mark_log(&mut self) {
        self.mark_log.resize(self.history.len(), MarkDelta::default());
    }

    pub fn compute_dirty(&self) -> bool {
        self.saved_revision != Some(self.history.current_revision())
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

/// A counter that is not part of the state's value (it compares equal to any other).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct EditCount(pub u64);

impl PartialEq for EditCount {
    fn eq(&self, _: &EditCount) -> bool {
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
