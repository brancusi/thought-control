#![doc = include_str!("../README.md")]

pub mod buffer;
pub mod command;
pub mod doc;
pub mod markdown;
pub mod motion;
pub mod text;

pub use buffer::BlockLine;
pub use command::{Command, Outcome};
pub use doc::{Anchor, Doc, Rect, View};

/// What a block is. Markdown markers are text (`## `, `12. `, `> `): a heading is a paragraph
/// whose text starts `## `, and the host draws the marker in the hang.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum Kind {
    /// A paragraph.
    Para,
    /// A list item.
    Bullet,
    /// A task: a list item with a status.
    Task,
}

/// One block of a document: its host-opaque `id`, nesting `depth`, kind, a task's `status`, its
/// text (a `\n` is a soft break inside it) and whether its children are folded.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Block<Id> {
    pub id: Id,
    pub depth: usize,
    pub kind: Kind,
    pub status: Option<String>,
    pub text: String,
    pub folded: bool,
    /// Whether a blank line comes before it (thc's `gap`): None is the default for its kind
    /// (`buffer::Buffer::effective_gap`). Kind changes pin it, so nothing moves.
    pub gap: Option<bool>,
}

impl<Id> Block<Id> {
    /// A block with no status (a task starts `todo`).
    pub fn new(id: Id, depth: usize, kind: Kind, text: &str) -> Self {
        Block {
            id,
            depth,
            kind,
            status: (kind == Kind::Task).then(|| "todo".to_string()),
            text: text.to_string(),
            folded: false,
            gap: None,
        }
    }
}

pub use motion::{Layout, Motion, Stop, note_stops};
pub use text::{gwidth, next_char, prev_char, width, wrap, wrap_with};

/// A caret position: a block (by index in the document) and a byte offset in its text, always
/// on a grapheme boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Pos {
    pub line: usize,
    pub byte: usize,
}
