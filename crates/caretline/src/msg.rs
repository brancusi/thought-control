//! Messages into `update` and effects out of it. Both are plain, serializable values.

use serde::{Deserialize, Serialize};

use crate::marks::MarkId;
use crate::external::ExtChange;
use crate::outline::NewBlock;

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
    /// The content start of the next block, or of this one (then the one before) going back.
    /// Outline documents only; elsewhere it moves by a document line.
    Block,
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
    /// A display frame at `now_ms`, sent by a runtime's frame clock while the view asks for
    /// one ([`crate::View::frame_clock`]). It advances the clock as `tick` does; animation
    /// state advances from it, so a dropped frame never stalls an animation. Passive.
    Frame { now_ms: u64 },
    /// Ask the runtime for a frame clock of `fps` frames per second (0 turns it off). The
    /// request is view state, so a replay asks for the same frames. Passive.
    FrameClock { fps: u16 },
    /// Show a one-line message in the status bar (until the next input). Passive: it
    /// doesn't end an edit run or disarm a pending quit.
    ShowStatus { text: String },

    // Outline documents (see docs/caretline/outline.md). Elsewhere these only set a status
    // message, except `soft_break` (a line break), `select_word_at` and `paste_plain`.
    /// A line break inside the block (a list item's second line). In a paragraph it is
    /// Enter.
    SoftBreak,
    /// Nest the caret's block, or every block the selection touches, one level deeper.
    Indent,
    /// Un-nest them one level.
    Outdent,
    /// Swap the caret's block (with its children) with its previous or next sibling.
    MoveBlock { dir: Dir },
    /// Select a block's whole content (a triple-click).
    SelectBlock { id: MarkId },
    /// Select the word at a char position (a double-click). A `click` with `extend` right
    /// after it extends by whole words.
    SelectWordAt { pos: usize },
    /// Insert blocks a host made (an attachment, recovered text) after a block, or at the
    /// start when `after` is absent. One undo step.
    InsertBlocks {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<MarkId>,
        blocks: Vec<NewBlock>,
    },
    /// Paste as plain text: in an outline, paragraphs with their line breaks kept, never
    /// list items.
    PastePlain {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },

    // Views (see docs/caretline/architecture.md#documents-and-views).
    /// Scroll the view by rows (negative is up) without moving the caret. The view stays
    /// where it is put (`free`) until the next caret motion or edit.
    ScrollView { rows: i32 },
    /// Hide a block's children in this view (outline documents). A caret inside them moves
    /// to the block's content end.
    Fold { id: MarkId },
    /// Show a folded block's children again.
    Unfold { id: MarkId },
    ToggleFold { id: MarkId },

    /// A host's own edit of the text, as one undo step (`join`: folded into the last step,
    /// with the edit before it): `[from, to)` replaced by `text`, in chars of the current
    /// text, ranges in order and apart. Recovered text, for example, that one undo takes back.
    Edit {
        changes: Vec<(usize, usize, String)>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        join: bool,
    },
    /// Changes from elsewhere (another device, a daemon, an agent), applied in order to the
    /// document outside the undo history: every view is mapped through them, and undo never
    /// takes them back (see docs/caretline/messages.md#external-changes). Passive: it
    /// doesn't end an edit run or clear the status.
    External { changes: Vec<ExtChange> },

    /// Run the host's command `name` (registered with [`crate::Host::command`]) with `args`:
    /// one transaction and one undo step. An unknown name changes nothing and says so.
    Command {
        name: String,
        #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
        args: serde_json::Value,
    },
}

impl Msg {
    /// Messages that never end an edit run or clear the status: the clock, a resize, a
    /// save's result, a status message.
    pub fn is_passive(&self) -> bool {
        matches!(
            self,
            Msg::Tick { .. }
                | Msg::Frame { .. }
                | Msg::FrameClock { .. }
                | Msg::Resize { .. }
                | Msg::Saved
                | Msg::SaveFailed { .. }
                | Msg::ShowStatus { .. }
                | Msg::External { .. }
        )
    }

    /// A change from outside the editor, applied to the document rather than through a view.
    pub fn is_external(&self) -> bool {
        matches!(self, Msg::External { .. })
    }

    /// Whether the message can change the text (or marks).
    pub fn edits(&self) -> bool {
        matches!(
            self,
            Msg::InsertText { .. }
                | Msg::InsertNewline
                | Msg::DeleteBackward
                | Msg::DeleteForward
                | Msg::DeleteWordBackward
                | Msg::DeleteWordForward
                | Msg::DeleteToLineStart
                | Msg::DeleteToLineEnd
                | Msg::KillLine
                | Msg::Cut
                | Msg::Paste { .. }
                | Msg::PastePlain { .. }
                | Msg::Undo
                | Msg::Redo
                | Msg::SoftBreak
                | Msg::Indent
                | Msg::Outdent
                | Msg::MoveBlock { .. }
                | Msg::InsertBlocks { .. }
                | Msg::Edit { .. }
                | Msg::Command { .. }
        )
    }
}

/// Work for the runtime. `update` never performs I/O; it returns these instead. New kinds may
/// be added: match with a wildcard arm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(tag = "effect", rename_all = "snake_case")]
pub enum Effect {
    /// Write the document. The runtime answers with `Saved` or `SaveFailed`.
    WriteFile { path: String, text: String },
    /// Put text on the system clipboard.
    ClipboardSet { text: String },
    Quit,
    /// A message for the person, from an outline document whose status bar is off.
    Notice { text: String },
    /// The primary caret moved from one block to another in an outline document (a commit
    /// point for a host).
    BlockLeft {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<MarkId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<MarkId>,
    },
    /// An editing message reached a read-only view: nothing changed.
    Refused,
    /// An effect of the host's own, from a host command or input rule ([`crate::host::Edit`]).
    Host {
        name: String,
        #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
        data: serde_json::Value,
    },
}
