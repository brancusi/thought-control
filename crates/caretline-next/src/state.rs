//! The editor's whole state: everything needed to reproduce the screen and the behaviour.

use serde::{Deserialize, Serialize};

use crate::helix::graphemes::ensure_grapheme_boundary_prev;
use crate::helix::history::History;
use crate::helix::line_ending::auto_detect_line_ending;
use crate::helix::{LineEnding, Range, Rope, Selection, SmallVec};

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
/// the same history revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditRun {
    pub kind: RunKind,
    pub revision: usize,
    pub at_ms: u64,
}

/// Edits further apart than this start a new undo step.
pub const RUN_GAP_MS: u64 = 1500;

/// The editor state. Serializes to JSON and back without loss; [`crate::update`] and
/// [`crate::view`] are pure functions of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// The document.
    #[serde(with = "rope_as_string")]
    pub text: Rope,
    /// The selection: one or more ranges, each an anchor and a head (the caret). A range's
    /// `old_visual_position` holds the goal column for vertical motion.
    pub selection: Selection,
    pub scroll: Scroll,
    pub viewport: Viewport,
    /// The internal clipboard register (the last copy or cut).
    pub clipboard: String,
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
            clipboard: String::new(),
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

    /// Clamps the selection and view to the document. A no-op on any state `update` made.
    pub fn sanitize(&mut self) {
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
        }
        self.dirty = self.compute_dirty();
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

mod rope_as_string {
    use crate::helix::Rope;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(rope: &Rope, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(rope)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Rope, D::Error> {
        let text = String::deserialize(d)?;
        Ok(Rope::from(text.as_str()))
    }
}
