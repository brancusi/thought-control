//! The editor: the one way the rest of thc-tui reads and changes an open page or day.
//!
//! The engine is caretline (`engine.rs`: one outline document per page or day; the text, the
//! selection, folds, undo and every editing rule are its own). Beside each of its blocks thc
//! keeps a line with the vault node's id and save state (`doc.rs`), takes in refreshes from
//! the vault (`patch.rs`) and gives tasks their meaning on caretline's extension points
//! (`tasks.rs`). Nothing outside this module touches the engine or the lines directly.
//!
//! By what it's for:
//!
//! - **Reading:** [`Doc::blocks`], [`Doc::caret_block`], [`Doc::caret`],
//!   [`Doc::anchor`], [`Doc::selection`], [`Doc::caret_anchor`], [`Doc::place_anchor`],
//!   [`Doc::is_folded`], [`Doc::hidden_by_fold`], [`Doc::effective_gap`], [`Doc::descendants`],
//!   [`Doc::frame`], [`Doc::hit`], [`Doc::revision`], [`Doc::undo_depth`], [`Doc::selected_parts`],
//!   [`Doc::copy_text`].
//! - **Caret and selection:** [`Doc::set_caret`], [`Doc::select_range`],
//!   [`Doc::clear_selection`], [`Doc::click`], [`Doc::drag`], [`Doc::select_word_at`],
//!   [`Doc::select_block`], [`Doc::set_caret_anchor`], [`Doc::restore_caret`],
//!   [`Doc::caret_to_start`], [`Doc::caret_to_end`].
//! - **Editing:** [`Doc::run_command`] (a caretline command id, or thc's `thc.task_cycle`),
//!   [`Doc::insert`], [`Doc::newline`],
//!   [`Doc::delete_selection`], [`Doc::paste`], [`Doc::replace_before_caret`], [`Doc::task_box`].
//! - **The host's changes:** [`Doc::undo_step`], [`Doc::replace_content`],
//!   [`Doc::set_shape`], [`Doc::insert_block`], [`Doc::insert_blocks`], [`Doc::patch`],
//!   [`Doc::apply_held_text`].
//! - **Saving:** [`Doc::plan_save`], [`Doc::apply_results`], [`Doc::idle_elapsed`],
//!   [`Doc::plan_idle`], [`Doc::idle_saved`], [`Doc::mark_saving`], [`Doc::mark_save_failed`],
//!   [`Doc::name_conflict`].

mod doc;
mod engine;
mod patch;
mod tasks;
mod view;

pub use doc::{Doc, Line, Target, meta_text, short_repeat};
pub use tasks::TASK_CYCLE;
pub use view::{DocHit, DocRow, ViewGeometry, HANG, MARKS};

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

/// A caret held by node id and byte: it survives edits above it, reopening and other devices'
/// changes (caret memory, history).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub id: String,
    pub byte: usize,
}

/// What a command did, so the app can say so or follow up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done,
    /// Nothing changed, and why.
    Nothing(String),
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
        self.engine.selection().1
    }

    /// The selection's other end, when something is selected.
    pub fn anchor(&self) -> Option<BlockPos> {
        self.engine.selection().0
    }

    /// The caret by its note's id.
    pub fn caret_anchor(&self) -> Anchor {
        Anchor { id: self.line().id.clone(), byte: self.caret().byte }
    }

    /// Where the caret is as a place to come back to: a new, empty line isn't one, the end of
    /// the line above it is (None: there's no line above).
    pub fn place_anchor(&self) -> Option<Anchor> {
        let l = self.line();
        // The fresh line the document arrived with: coming back is arriving again (None).
        if l.is_new && l.text.is_empty() && self.fresh_end.as_deref() == Some(l.id.as_str()) {
            return None;
        }
        if l.is_new && l.text.is_empty() {
            let p = self.caret().line.checked_sub(1).and_then(|i| self.lines().get(i))?;
            return Some(Anchor { id: p.id.clone(), byte: p.text.len() });
        }
        Some(self.caret_anchor())
    }

    /// Note `id`'s children are folded away.
    pub fn is_folded(&self, id: &str) -> bool {
        self.lines().iter().position(|l| l.id == id).is_some_and(|i| self.engine.is_folded(i))
    }

}

// ---- views (sidebar.md §7.1) ------------------------------------------------------------------

/// A view of a document: the main view is [`MAIN_VIEW`], each sidebar panel has its own id.
pub type ViewId = u32;
/// The main view's id.
pub const MAIN_VIEW: ViewId = 0;

impl Doc {
    /// The view the caret, the selection, the layout and every command read and act through.
    pub fn current_view(&self) -> ViewId {
        self.engine.current_view()
    }

    /// The document's views, by id.
    pub fn views(&self) -> Vec<ViewId> {
        self.engine.view_ids()
    }

    pub fn has_view(&self, id: ViewId) -> bool {
        self.engine.view_ids().contains(&id)
    }

    /// A new view `id`: laid out like the current one, its caret at the start of the first
    /// note, at the top, parked (sidebar.md §9). Nothing when it has one.
    pub fn add_view(&mut self, id: ViewId) {
        self.engine.add_view(id);
    }

    /// Act through view `id` from now on. False: the document has no such view.
    pub fn use_view(&mut self, id: ViewId) -> bool {
        self.engine.use_view(id)
    }

    /// Drop view `id` (another becomes current when it was). False: it's the last view.
    pub fn remove_view(&mut self, id: ViewId) -> bool {
        self.rows.borrow_mut().remove(&id);
        self.engine.remove_view(id)
    }

    /// The current view takes id `id`: a document moving from the main view to a panel, or
    /// back. False: another view has that id.
    pub fn rename_view(&mut self, id: ViewId) -> bool {
        let was = self.current_view();
        let ok = self.engine.rename_view(id);
        if ok && was != id {
            let mut rows = self.rows.borrow_mut();
            if let Some(r) = rows.remove(&was) {
                rows.insert(id, r);
            }
        }
        ok
    }

    /// Run `f` through view `id`, then go back to the view that was current. None: no such
    /// view.
    pub fn with_view<R>(&mut self, id: ViewId, f: impl FnOnce(&mut Doc) -> R) -> Option<R> {
        let was = self.current_view();
        if !self.use_view(id) {
            return None;
        }
        let r = f(self);
        self.use_view(was);
        Some(r)
    }

    /// Each view's place (caret anchor and first row on screen), to rebuild them on a document
    /// read again from the vault ([`Doc::adopt_views`]).
    pub fn view_places(&mut self) -> (ViewId, Vec<(ViewId, Anchor, usize)>) {
        let was = self.current_view();
        let mut out = Vec::new();
        for id in self.views() {
            if let Some(p) = self.with_view(id, |d| (d.caret_anchor(), d.scroll())) {
                out.push((id, p.0, p.1));
            }
        }
        (was, out)
    }

    /// The views a document had before it was read again: each one back at its place, the
    /// same one current. (A new document has only [`MAIN_VIEW`].)
    pub fn adopt_views(&mut self, current: ViewId, places: &[(ViewId, Anchor, usize)]) {
        if places.is_empty() {
            return;
        }
        let first = self.current_view();
        for (id, _, _) in places {
            self.add_view(*id);
        }
        if !places.iter().any(|(id, _, _)| *id == first) {
            let other = places[0].0;
            self.use_view(other);
            self.remove_view(first);
        }
        for (id, a, scroll) in places {
            self.with_view(*id, |d| {
                if d.set_caret_anchor(a) {
                    d.set_scroll(*scroll, false);
                }
            });
        }
        self.use_view(current);
    }
}

// ---- the caret and the selection ----------------------------------------------------------------

impl Doc {
    /// Put the caret at `p`; the selection's other end stays.
    pub fn set_caret(&mut self, p: BlockPos) {
        self.engine.flush();
        let anchor = self.engine.selection().0;
        self.engine.select(anchor, p);
    }

    /// Select from `anchor` (None: nothing selected) to the caret at `caret`.
    pub fn select_range(&mut self, anchor: Option<BlockPos>, caret: BlockPos) {
        self.engine.select(anchor, caret);
    }

    pub fn clear_selection(&mut self) {
        self.engine.flush();
        let caret = self.engine.selection().1;
        self.engine.select(None, caret);
    }

    /// A click at `p`: the caret goes there, extending the selection with ⇧ (`extend`), and
    /// up and down forget their column.
    pub fn click(&mut self, p: BlockPos, extend: bool) {
        self.engine.flush();
        let (anchor, caret) = self.engine.selection();
        self.engine.select(extend.then(|| anchor.unwrap_or(caret)), p);
    }

    /// A drag with the button held, at cell (`col`, `row`) of the view: the selection extends
    /// there from where the press put the caret; on the view's first or last text row (or past
    /// it) the view scrolls a row (caretline `Msg::Drag`).
    pub fn drag_to(&mut self, col: u16, row: u16) {
        self.run(caretline::Msg::Drag { col, row });
        // The pointer moves the view a row at a time, no more: no margin pulled after it.
        self.hold_view();
    }

    /// One caretline message through the current view (an agent's own view, doc_view.rs).
    pub fn run_msg(&mut self, msg: caretline::Msg) -> Vec<caretline::Effect> {
        self.run(msg)
    }

    /// Line `i`'s text as engine chars (`from..to`), for text anchors.
    pub fn char_range(&mut self, i: usize) -> Option<(usize, usize)> {
        self.engine.flush();
        let len = self.lines().get(i)?.text.len();
        Some((
            self.engine.char_of(BlockPos { line: i, byte: 0 }),
            self.engine.char_of(BlockPos { line: i, byte: len }),
        ))
    }

    /// Put the caret back at a held place. False: its note isn't here.
    pub fn set_caret_anchor(&mut self, a: &Anchor) -> bool {
        let Some(i) = self.lines().iter().position(|l| l.id == a.id) else { return false };
        let t = &self.lines()[i].text;
        let mut b = a.byte.min(t.len());
        while !t.is_char_boundary(b) {
            b -= 1;
        }
        self.set_caret(BlockPos { line: i, byte: b });
        true
    }

    /// The fresh line the document arrived with (doc_app::arrive) is still at its end, empty.
    pub fn has_fresh_end(&self) -> bool {
        self.fresh_end.as_deref().is_some_and(|id| self.lines().last().is_some_and(|l| l.id == id && l.is_new && l.text.is_empty()))
    }

    /// Where the caret's line is when it's empty and not saved (and not the fresh line at the
    /// end, which `fresh_end` keeps): after which saved note, how deep, what kind.
    pub fn new_caret_line(&self) -> Option<crate::ui_state::NewCaretLine> {
        let i = self.caret().line;
        let l = self.lines().get(i)?;
        if !l.is_new || !l.text.trim().is_empty() || (self.has_fresh_end() && i + 1 == self.lines().len()) {
            return None;
        }
        let after = self.lines()[..i].iter().rev().find(|p| !p.is_new).map(|p| p.id.clone());
        Some(crate::ui_state::NewCaretLine { after, depth: l.depth, kind: l.kind() })
    }

    /// The blank space the caret's saved note ends with: the vault trims it (a note never ends
    /// in a space or a line break), the caret's line keeps it until it's left.
    pub fn caret_tail(&self) -> Option<String> {
        let l = self.lines().get(self.caret().line).filter(|l| !l.is_new)?;
        let tail = &l.text[l.text.trim_end().len()..];
        (!tail.is_empty()).then(|| tail.to_string())
    }

    /// Note `id` ends with `tail` again (blank space its save trimmed), the caret at `byte`.
    pub fn restore_caret_tail(&mut self, id: &str, tail: &str, byte: usize) {
        let Some(i) = self.lines().iter().position(|l| l.id == id) else { return };
        if !tail.trim().is_empty() || self.lines()[i].text.ends_with(tail) {
            return;
        }
        self.lines_mut()[i].text.push_str(tail);
        self.touch_content();
        let t = &self.lines()[i].text;
        let b = (0..=byte.min(t.len())).rev().find(|x| t.is_char_boundary(*x)).unwrap_or(0);
        self.set_caret(BlockPos { line: i, byte: b });
    }

    /// The caret on an empty, unsaved line after note `after` (None: at the top), `depth` deep:
    /// an empty line there already is used (the fresh one the document arrived with, if it's
    /// there), else one is put in; a fresh line at the end elsewhere goes unless `keep_fresh_end`.
    /// False: no `after`.
    pub fn caret_to_new_line(&mut self, after: Option<&str>, depth: usize, kind: Kind, keep_fresh_end: bool) -> bool {
        let at = match after {
            Some(id) => match self.lines().iter().position(|l| l.id == id) {
                Some(i) => i + 1,
                None => return false,
            },
            None => 0,
        };
        let empty = |l: &Line| l.is_new && l.text.trim().is_empty();
        let n = self.lines().len();
        // The fresh line at the end, when it isn't the one wanted.
        if !keep_fresh_end && n > 1 && at + 1 < n && self.lines().last().is_some_and(empty) {
            self.lines_mut().pop();
            self.fresh_end = None;
        }
        // An unsaved caret line and a remembered fresh end are distinct stops, even when
        // reload initially supplied only the fresh end at this position.
        let is_fresh = self.fresh_end.as_deref() == self.lines().get(at).map(|l| l.id.as_str());
        if !self.lines().get(at).is_some_and(empty) || keep_fresh_end && is_fresh {
            let mut l = Line::new(depth, kind, "");
            l.id = self.take_id();
            self.lines_mut().insert(at, l);
        } else if self.lines()[at].depth != depth || self.lines()[at].kind() != kind {
            let l = &mut self.lines_mut()[at];
            l.depth = depth;
            l.kind = kind;
            l.status = (kind == Kind::Task).then(|| "todo".to_string());
        }
        if at + 1 < self.lines().len() && !keep_fresh_end {
            self.fresh_end = None;
        }
        self.set_caret(BlockPos { line: at, byte: 0 });
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
            self.fresh_end = None;
        }
        let b = a.byte.min(self.lines()[i].text.len());
        let b = (0..=b).rev().find(|x| self.lines()[i].text.is_char_boundary(*x)).unwrap_or(0);
        self.set_caret(BlockPos { line: i, byte: b });
        Some(i)
    }

    /// A page opens at its top (on an empty line, when it has none).
    pub fn caret_to_start(&mut self) {
        if self.lines().is_empty() {
            self.lines_mut().push(Line::new(0, Kind::Bullet, ""));
        }
        self.set_caret(BlockPos { line: 0, byte: 0 });
    }
}

// ---- editing --------------------------------------------------------------------------------------

impl Doc {
    /// A paste of more than one line: Markdown (unless `plain`) read into notes at the caret,
    /// one undo step. How many notes, and how many images were left out.
    pub fn paste(&mut self, text: &str, plain: bool) -> (usize, usize) {
        self.paste_blocks(text, plain)
    }

    /// A paste of one line: typed in as is. The text the engine copied or cut itself (whole
    /// notes copy as one line) goes back as it was taken: notes, ids kept.
    pub fn paste_line(&mut self, text: &str) {
        if self.engine.is_register(text) {
            self.run(caretline::Msg::Paste { text: Some(text.to_string()) });
            return;
        }
        self.insert(text);
    }

    /// Replace the caret line's text from `start` to the caret with `text` (a `[[link]]` picked
    /// from the popup).
    pub fn replace_before_caret(&mut self, start: usize, text: &str) {
        self.engine.flush();
        let caret = self.engine.selection().1;
        self.engine.select(Some(BlockPos { line: caret.line, byte: start }), caret);
        self.insert(text);
    }
}

// ---- the host's changes -------------------------------------------------------------------------

impl Doc {
    /// Note `id`'s text, replaced (a link taken from the near-miss chip, a drop turned back into
    /// its path). False: it isn't here.
    pub fn replace_content(&mut self, id: &str, text: &str) -> bool {
        let Some(i) = self.lines().iter().position(|l| l.id == id) else { return false };
        self.lines_mut()[i].text = text.to_string();
        self.touch_content();
        self.engine.settle();
        true
    }

    /// Note `id`'s text, test-only and without telling the layout (to check that it notices).
    #[cfg(test)]
    pub fn set_text_unannounced(&mut self, i: usize, text: &str) {
        self.lines_mut()[i].text = text.to_string();
        self.engine.settle();
    }

    /// Note `id` takes this shape and text (a recovered line), when they differ. True: changed.
    pub fn set_shape(&mut self, id: &str, depth: usize, kind: Kind, status: Option<String>, text: &str) -> bool {
        let Some(i) = self.lines().iter().position(|l| l.id == id) else { return false };
        let l = &self.lines()[i];
        if l.text == text && l.kind() == kind && l.depth == depth {
            return false;
        }
        let l = &mut self.lines_mut()[i];
        l.text = text.to_string();
        l.kind = kind;
        l.depth = depth;
        l.status = status;
        self.engine.settle();
        true
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
        self.engine.settle();
        id
    }

    /// Paragraph notes at the caret, at its note's depth: the first on the caret's line when
    /// that's new and blank, else after it, the rest after that. One undo step; the caret goes
    /// to the start of the last. Their ids.
    pub fn insert_blocks(&mut self, texts: &[&str]) -> Vec<String> {
        let i = self.caret().line;
        let (ids, at) = self.undo_step(|d| {
            let depth = d.lines()[i].depth;
            let mut ids = Vec::new();
            let mut at = i;
            for (k, text) in texts.iter().enumerate() {
                let l = Line::new(depth, Kind::Para, text);
                ids.push(l.id.clone());
                if k == 0 && d.lines()[i].text.trim().is_empty() && d.lines()[i].is_new {
                    d.lines_mut()[i] = l;
                } else {
                    at += 1;
                    d.lines_mut().insert(at, l);
                }
            }
            (ids, at)
        });
        self.engine.select(None, BlockPos { line: at, byte: 0 });
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
        self.engine.settle();
        changed
    }

    /// Notes for a test, the caret at the start. Test-only.
    #[cfg(test)]
    pub fn set_blocks(&mut self, lines: Vec<Line>) {
        *self.engine = engine::Engine::load(lines);
    }

    /// Note `i`'s blank line before it, set and saved. Test-only.
    #[cfg(test)]
    pub fn set_saved_gap(&mut self, i: usize, gap: Option<bool>) {
        let l = &mut self.lines_mut()[i];
        l.gap = gap;
        l.saved_gap = gap;
        self.engine.settle();
    }

    /// Fold note `id`'s children away. Test-only (no key folds yet).
    #[cfg(test)]
    pub fn fold(&mut self, id: &str) {
        if let Some(i) = self.lines().iter().position(|l| l.id == id) {
            self.engine.fold(i);
        }
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
