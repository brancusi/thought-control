//! The editor seam: the one way the rest of thc-tui reads and changes an open page or day.
//!
//! Behind it is the engine, caretline (`next.rs`: one outline document per page or day, whose
//! blocks thc's lines mirror), thc's per-line save state (`doc.rs`) and refreshes from the
//! vault (`patch.rs`). Nothing outside this module touches the engine, its view or its lines
//! directly: the engine's fields are private to it, and every type here is thc's own.
//!
//! The seam, by what it's for:
//!
//! - **Reading:** [`Doc::blocks`], [`Doc::caret_block`], [`Doc::caret`],
//!   [`Doc::anchor`], [`Doc::selection`], [`Doc::caret_anchor`], [`Doc::place_anchor`],
//!   [`Doc::is_folded`], [`Doc::hidden_by_fold`], [`Doc::effective_gap`], [`Doc::descendants`],
//!   [`Doc::rows_of`], [`Doc::revision`], [`Doc::undo_depth`], [`Doc::selected_parts`],
//!   [`Doc::copy_text`].
//! - **Caret and selection:** [`Doc::set_caret`], [`Doc::select_range`],
//!   [`Doc::clear_selection`], [`Doc::click`], [`Doc::drag`], [`Doc::select_word_at`],
//!   [`Doc::select_block`], [`Doc::set_caret_anchor`], [`Doc::restore_caret`],
//!   [`Doc::caret_to_start`], [`Doc::caret_to_end`].
//! - **Editing:** [`Doc::apply`] (an [`EditCmd`]), [`Doc::insert`], [`Doc::newline`],
//!   [`Doc::delete_selection`], [`Doc::paste`], [`Doc::replace_before_caret`], [`Doc::task_box`].
//! - **The host's changes:** [`Doc::begin_undo_step`], [`Doc::replace_content`],
//!   [`Doc::set_shape`], [`Doc::insert_block`], [`Doc::insert_blocks`], [`Doc::patch`],
//!   [`Doc::apply_held_text`].
//! - **Saving:** [`Doc::plan_save`], [`Doc::apply_results`], [`Doc::idle_elapsed`],
//!   [`Doc::plan_idle`], [`Doc::idle_saved`], [`Doc::mark_saving`], [`Doc::mark_save_failed`],
//!   [`Doc::name_conflict`].

mod doc;
mod next;
mod patch;

pub use doc::{Doc, Line, Sent, Target, meta_text, short_repeat};

use thc_core::outline::Kind;

/// A note as the document reads it back from the vault: saved text that starts with a list
/// or task marker (`[x] done`, `- a`) reads as that kind. (Test-only: the fuzz compares a
/// reopened document with what it meant.)
#[cfg(test)]
pub fn read_back(n: (Kind, usize, Option<String>, String)) -> (Kind, usize, Option<String>, String) {
    let (kind, depth, status, text) = n;
    let b: thc_core::outline::Block = serde_json::from_value(serde_json::json!({
        "id": "x", "parent": null, "depth": depth, "kind": format!("{kind:?}").to_lowercase(),
        "status": status, "text": text, "text_rev": "r",
    }))
    .expect("a block");
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap();
    let d = Doc::new(Target::Journal { date: today }, None, &[b], today);
    let l = &d.blocks()[0];
    let status = if l.kind() == Kind::Task { l.status.clone() } else { None };
    (l.kind(), l.depth, status, l.text.trim().to_string())
}

/// A caret position: a block (by its index in the document) and a byte offset in its text, on a
/// grapheme boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub struct BlockPos {
    pub line: usize,
    pub byte: usize,
}

impl From<doc::Pos> for BlockPos {
    fn from(p: doc::Pos) -> Self {
        BlockPos { line: p.line, byte: p.byte }
    }
}

impl From<BlockPos> for doc::Pos {
    fn from(p: BlockPos) -> Self {
        doc::Pos { line: p.line, byte: p.byte }
    }
}

/// A caret held by node id and byte: it survives edits above it, reopening and other devices'
/// changes (caret memory, history).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub id: String,
    pub byte: usize,
}

/// A caret motion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Up,
    Down,
    /// PgUp / PgDn: this many rows (negative: up).
    Page(isize),
    Left,
    Right,
    WordLeft,
    WordRight,
    Home,
    End,
    DocStart,
    DocEnd,
    /// ⌃↑ / ⌃↓: the note's start, else the previous / next visible note's.
    NoteUp,
    NoteDown,
}

/// An editing or motion command at the caret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditCmd {
    /// Enter: a line break, a new note, the next list item, or the end of the list.
    Newline,
    /// ⇧Enter / ⌃J: a line break inside the note.
    SoftBreak,
    Backspace,
    Delete,
    DeleteWordBack,
    KillToEnd,
    KillToStart,
    Indent,
    Outdent,
    /// ⌃T: text → `[ ]` → `[x]` → text.
    TaskCycle,
    /// ⌥↑ / ⌥↓: the note (with its children) up or down.
    MoveLine(isize),
    SelectAll,
    Undo,
    Redo,
    /// A motion; `select` extends the selection from where it began.
    Move { motion: Motion, select: bool },
}

/// What a command did, so the app can say so or follow up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done,
    /// Nothing changed, and why.
    Nothing(&'static str),
    /// A task was completed: save at once.
    Completed,
    /// Undo or redo: re-read what the vault has.
    Restored,
}

/// A note the host puts in (a recovered line): `id` keeps a known id, else a new one.
#[derive(Clone, Debug, PartialEq)]
pub struct NewBlock {
    pub id: Option<String>,
    pub depth: usize,
    pub kind: Kind,
    pub status: Option<String>,
    pub text: String,
}

// ---- reading ------------------------------------------------------------------------------------

impl Doc {
    /// The document's notes, in order.
    pub fn blocks(&self) -> &[Line] {
        self.lines()
    }

    /// The caret's note.
    pub fn caret_block(&self) -> &Line {
        self.line()
    }

    /// Where the caret is.
    pub fn caret(&self) -> BlockPos {
        self.view.caret.into()
    }

    /// The selection's other end, when there is one (it may equal the caret).
    pub fn anchor(&self) -> Option<BlockPos> {
        self.view.anchor.map(Into::into)
    }

    /// The column up and down keep, from the last vertical motion.
    #[cfg(test)]
    pub fn goal(&self) -> Option<usize> {
        self.view.goal
    }

    /// The caret by its note's id.
    pub fn caret_anchor(&self) -> Anchor {
        Anchor { id: self.line().id.clone(), byte: self.view.caret.byte }
    }

    /// Where the caret is as a place to come back to: a new, empty line isn't one, the end of
    /// the line above it is (None: there's no line above).
    pub fn place_anchor(&self) -> Option<Anchor> {
        let l = self.line();
        if l.is_new && l.text.is_empty() {
            let p = self.view.caret.line.checked_sub(1).and_then(|i| self.lines().get(i))?;
            return Some(Anchor { id: p.id.clone(), byte: p.text.len() });
        }
        Some(self.caret_anchor())
    }

    /// Note `id`'s children are folded away.
    pub fn is_folded(&self, id: &str) -> bool {
        self.view.folds.contains(id)
    }

    /// Note `i` is hidden by a folded note above it.
    pub fn hidden_by_fold(&self, i: usize) -> bool {
        crate::doc_ui::folded_hidden(self.lines(), i, &self.view.folds)
    }

}

// ---- the caret and the selection ----------------------------------------------------------------

impl Doc {
    /// Put the caret at `p`; the selection's other end and the column stay.
    pub fn set_caret(&mut self, p: BlockPos) {
        self.view.caret = p.into();
    }

    /// Select from `anchor` (None: nothing selected) to the caret at `caret`.
    pub fn select_range(&mut self, anchor: Option<BlockPos>, caret: BlockPos) {
        self.view.anchor = anchor.map(Into::into);
        self.view.caret = caret.into();
    }

    pub fn clear_selection(&mut self) {
        self.view.anchor = None;
    }

    /// A click at `p`: the caret goes there, extending the selection with ⇧ (`extend`), and
    /// up and down forget their column.
    pub fn click(&mut self, p: BlockPos, extend: bool) {
        if extend {
            if self.view.anchor.is_none() {
                self.view.anchor = Some(self.view.caret);
            }
        } else {
            self.view.anchor = None;
        }
        self.view.caret = p.into();
        self.view.goal = None;
    }

    /// A drag from `from` to `to`: selects between them.
    pub fn drag(&mut self, from: BlockPos, to: BlockPos) {
        self.view.anchor = Some(from.into());
        self.view.caret = to.into();
        self.view.goal = None;
    }

    /// Put the caret back at a held place. False: its note isn't here.
    pub fn set_caret_anchor(&mut self, a: &Anchor) -> bool {
        let Some(i) = self.lines().iter().position(|l| l.id == a.id) else { return false };
        let t = &self.lines()[i].text;
        let mut b = a.byte.min(t.len());
        while !t.is_char_boundary(b) {
            b -= 1;
        }
        self.view.caret = doc::Pos { line: i, byte: b };
        true
    }

    /// The caret memory when a document opens: back where it was, if that note is still here.
    /// `drop_fresh_end`: a journal day opened on a fresh line at its end doesn't need it when
    /// the caret goes back elsewhere. The caret's line index, or None.
    pub fn restore_caret(&mut self, a: &Anchor, drop_fresh_end: bool) -> Option<usize> {
        let i = self.lines().iter().position(|l| l.id == a.id)?;
        let n = self.lines().len();
        if drop_fresh_end && n > 1 && self.lines().last().is_some_and(|l| l.is_new && l.text.is_empty()) && i + 1 < n {
            self.lines_mut().pop();
        }
        let b = a.byte.min(self.lines()[i].text.len());
        let b = (0..=b).rev().find(|x| self.lines()[i].text.is_char_boundary(*x)).unwrap_or(0);
        self.view.caret = doc::Pos { line: i, byte: b };
        Some(i)
    }

    /// A page opens at its top (on an empty line, when it has none).
    pub fn caret_to_start(&mut self) {
        if self.lines().is_empty() {
            self.lines_mut().push(Line::new(0, Kind::Para, ""));
        }
        self.view.caret = doc::Pos { line: 0, byte: 0 };
    }
}

// ---- editing --------------------------------------------------------------------------------------

impl Doc {
    /// Apply an editing or motion command at the caret, with thc's layout (`width_of`: the text
    /// column of a note) for motion.
    pub fn apply(&mut self, cmd: EditCmd, width_of: &dyn Fn(&Line) -> usize) -> Outcome {
        self.next_apply(cmd, width_of)
    }

    /// A paste of more than one line: Markdown (unless `plain`) read into notes at the caret,
    /// one undo step. How many notes, and how many images were left out.
    pub fn paste(&mut self, text: &str, plain: bool) -> (usize, usize) {
        self.next_paste(text, plain)
    }

    /// A paste of one line: typed in as is. The text the engine copied or cut itself (whole
    /// notes copy as one line) goes back as it was taken: notes, ids kept.
    pub fn paste_line(&mut self, text: &str) {
        if self.next_is_register(text) {
            self.next_run(caretline::Msg::Paste { text: Some(text.to_string()) });
            return;
        }
        self.insert(text);
    }

    /// Replace the caret line's text from `start` to the caret with `text` (a `[[link]]` picked
    /// from the popup).
    pub fn replace_before_caret(&mut self, start: usize, text: &str) {
        self.select(false);
        self.view.anchor = Some(doc::Pos { line: self.view.caret.line, byte: start });
        self.insert(text);
    }
}

// ---- the host's changes -------------------------------------------------------------------------

impl Doc {
    /// Note `id`'s text, replaced (a link taken from the near-miss chip, a drop turned back into
    /// its path). False: it isn't here.
    pub fn replace_content(&mut self, id: &str, text: &str) -> bool {
        let Some(l) = self.lines_mut().iter_mut().find(|l| l.id == id) else { return false };
        l.text = text.to_string();
        self.touch_content();
        true
    }

    /// Note `id`'s text, test-only and without telling the layout (to check that it notices).
    #[cfg(test)]
    pub fn set_text_unannounced(&mut self, i: usize, text: &str) {
        self.lines_mut()[i].text = text.to_string();
    }

    /// Note `id` takes this shape and text (a recovered line), when they differ. True: changed.
    pub fn set_shape(&mut self, id: &str, depth: usize, kind: Kind, status: Option<String>, text: &str) -> bool {
        let Some(l) = self.lines_mut().iter_mut().find(|l| l.id == id) else { return false };
        if l.text != text || l.kind() != kind || l.depth != depth {
            l.text = text.to_string();
            l.kind = kind;
            l.depth = depth;
            l.status = status;
            return true;
        }
        false
    }

    /// A note put in at index `at`. Its id.
    pub fn insert_block(&mut self, at: usize, b: NewBlock) -> String {
        let mut l = Line::new(b.depth, b.kind, &b.text);
        l.status = b.status;
        if let Some(id) = b.id {
            l.id = id;
        }
        let id = l.id.clone();
        self.lines_mut().insert(at, l);
        id
    }

    /// Paragraph notes at the caret, at its note's depth: the first on the caret's line when
    /// that's new and blank, else after it, the rest after that. One undo step; the caret goes
    /// to the start of the last. Their ids.
    pub fn insert_blocks(&mut self, texts: &[&str]) -> Vec<String> {
        self.begin_undo_step();
        let i = self.view.caret.line;
        let depth = self.lines()[i].depth;
        let mut ids = Vec::new();
        let mut at = i;
        for (k, text) in texts.iter().enumerate() {
            let l = Line::new(depth, Kind::Para, text);
            ids.push(l.id.clone());
            if k == 0 && self.lines()[i].text.trim().is_empty() && self.lines()[i].is_new {
                self.lines_mut()[i] = l;
            } else {
                at += 1;
                self.lines_mut().insert(at, l);
            }
        }
        self.view.caret = doc::Pos { line: at, byte: 0 };
        self.view.anchor = None;
        ids
    }

    /// Leaving note `id`: a text changed elsewhere and held while you were on it lands now,
    /// unless you typed on it (then your save carries its base and the vault keeps both). True:
    /// the text changed.
    pub fn apply_held_text(&mut self, id: &str) -> bool {
        let mut changed = false;
        // (Looked up first: most lines left have nothing held.)
        if !self.lines().iter().any(|l| l.id == id && l.remote_text.is_some()) {
            return false;
        }
        if let Some(l) = self.lines_mut().iter_mut().find(|l| l.id == id) {
            if let Some(t) = l.remote_text.take() {
                if !l.edited() {
                    l.text = t.clone();
                    l.saved = Some(t);
                    changed = true;
                }
            }
        }
        if changed {
            self.touch_content();
        }
        changed
    }

    /// Notes for a test, the caret at the start. Test-only.
    #[cfg(test)]
    pub fn set_blocks(&mut self, lines: Vec<Line>) {
        *self.engine = next::Next::load(lines);
    }

    /// Note `i`'s blank line before it, set and saved. Test-only.
    #[cfg(test)]
    pub fn set_saved_gap(&mut self, i: usize, gap: Option<bool>) {
        let l = &mut self.lines_mut()[i];
        l.gap = gap;
        l.saved_gap = gap;
    }

    /// Fold note `id`'s children away. Test-only (no key folds yet).
    #[cfg(test)]
    pub fn fold(&mut self, id: &str) {
        self.view.folds.insert(id.to_string());
    }
}

// ---- saving -------------------------------------------------------------------------------------

impl Doc {
    /// The last edit is at least `after` old and hasn't been idle-saved: true once, then it
    /// counts as committed.
    pub fn idle_elapsed(&mut self, after: std::time::Duration) -> bool {
        let now = self.now_ms;
        if !self.engine.changed_since(now).is_some_and(|age| age >= after.as_millis() as u64) {
            return false;
        }
        self.engine.mark_committed();
        true
    }

    /// An idle save of note `id` landed: `text` is saved at revision `rev`, and a remote text
    /// held for it is moot.
    pub fn idle_saved(&mut self, id: &str, rev: Option<String>, text: String) {
        if let Some(l) = self.lines_state_mut().iter_mut().find(|l| l.id == id) {
            l.base = rev;
            l.saved = Some(text);
            l.remote_text = None;
        }
    }

    /// The notes in `ids` are being saved since `since` (ms, [`ms`]; None: not any more).
    pub fn mark_saving(&mut self, ids: &[String], since: Option<u64>) {
        for l in self.lines_state_mut().iter_mut().filter(|l| ids.contains(&l.id)) {
            l.saving_since = since;
        }
    }

    /// The notes in `ids` weren't saved, and why.
    pub fn mark_save_failed(&mut self, ids: &[String], error: &str) {
        for l in self.lines_state_mut().iter_mut().filter(|l| ids.contains(&l.id)) {
            l.save_error = Some(error.to_string());
        }
    }

    /// Who else edited `≠` note `id`. False: it isn't here.
    pub fn name_conflict(&mut self, id: &str, who: &str) -> bool {
        let Some(l) = self.lines_state_mut().iter_mut().find(|l| l.id == id) else { return false };
        l.conflict_with = Some(who.to_string());
        true
    }
}
