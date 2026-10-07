//! Messages into `update` and effects out of it. Both are plain, serializable values.

use serde::{Deserialize, Serialize};

/// Which way a motion goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dir {
    Backward,
    Forward,
}

/// What a motion moves by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum By {
    /// One grapheme cluster.
    Grapheme,
    /// To the end of the next word (forward) or the start of the previous one (backward).
    Word,
    /// One document line up or down, ignoring soft wrap.
    Line,
    /// One visual (wrapped) row up or down, keeping the goal column.
    VisualLine,
    /// The start of the caret's visual row.
    LineStart,
    /// The end of the caret's visual row.
    LineEnd,
    /// A screenful of visual rows up or down.
    Page,
    DocStart,
    DocEnd,
}

/// Everything that can happen to the editor. `update` is a pure function of the state
/// and one of these; any time it needs arrives inside a message (`Tick`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "msg", rename_all = "snake_case")]
pub enum Msg {
    /// Type text at every caret, replacing any selection.
    InsertText { text: String },
    InsertNewline,
    DeleteBackward,
    DeleteForward,
    /// Delete back to the previous word start (or the selection).
    DeleteWordBackward,
    /// Delete forward to the next word end (or the selection).
    DeleteWordForward,
    /// Delete back to the visual row's start (or the selection).
    DeleteToLineStart,
    /// Delete forward to the visual row's end (or the selection).
    DeleteToLineEnd,
    /// Delete forward to the end of the document line, or the line break when already there.
    KillLine,
    /// Move every caret. With `extend`, the anchors stay put and the selection grows or
    /// shrinks; without it, a selection collapses (see the README for the exact rules).
    Move {
        dir: Dir,
        by: By,
        #[serde(default)]
        extend: bool,
    },
    /// Place the caret at a screen cell (a click), or extend to it (a drag or shift-click).
    Click {
        col: u16,
        row: u16,
        #[serde(default)]
        extend: bool,
    },
    /// Scroll the view by rows (negative is up). The caret follows if it would leave the view.
    Scroll { rows: i32 },
    SelectAll,
    /// Collapse every selection to its caret.
    Collapse,
    Copy,
    Cut,
    /// Paste `text`, or the internal clipboard register when `text` is absent.
    Paste {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    Undo,
    Redo,
    Save,
    /// The runtime finished a save.
    Saved,
    /// The runtime could not save.
    SaveFailed { err: String },
    Quit,
    Resize { width: u16, height: u16 },
    /// The current time. Typing runs (one undo step) are measured with it.
    Tick { now_ms: u64 },
    /// Show a one-line message in the status bar (until the next input). Passive: it
    /// doesn't end an edit run or disarm a pending quit.
    ShowStatus { text: String },
}

/// Work for the runtime. `update` never performs I/O; it returns these instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "effect", rename_all = "snake_case")]
pub enum Effect {
    /// Write the document. The runtime answers with `Saved` or `SaveFailed`.
    WriteFile { path: String, text: String },
    /// Put text on the system clipboard.
    ClipboardSet { text: String },
    Quit,
}
