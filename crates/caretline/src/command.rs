//! What a host asks the engine to do: editing and motion as data (caretline SPEC §3, §5). A host
//! maps its keys to commands; the engine applies them and says what happened.

use crate::motion::Motion;

/// One editing or motion command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Enter: a line break, a new block, the next list item, or the end of the list.
    Newline,
    /// ⇧Enter / ⌃J: a line break inside the block.
    SoftBreak,
    /// ⌫: one grapheme back, or joins with the block above.
    Backspace,
    /// Delete: one grapheme forward, or joins the block below.
    Delete,
    /// ⌥⌫: the word before the caret.
    DeleteWordBack,
    /// ⌃K: to the end of the line.
    KillToEnd,
    /// ⌃U: to the start of the line.
    KillToStart,
    /// Tab: nest one level deeper.
    Indent,
    /// ⇧Tab: one level out.
    Outdent,
    /// ⌃T: text → `[ ]` → `[x]` → text.
    TaskCycle,
    /// ⌥↑ / ⌥↓: the block (with its children) up or down.
    MoveLine(isize),
    /// Select the whole document.
    SelectAll,
    Undo,
    Redo,
    /// A caret motion; `select` extends the selection from where it began.
    Move {
        motion: Motion,
        select: bool,
    },
}

/// What a command did, so the host can say so or follow up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Done.
    Done,
    /// Nothing changed, and why (`nothing to nest under`, `nothing to undo`).
    Nothing(&'static str),
    /// A task was completed (a host may want to commit at once).
    Completed,
    /// The document went back to an earlier state (undo, redo): a host re-reads what it shows.
    Restored,
}
