//! The engine: caretline.
//!
//! A page or a day is one caretline outline document: one text line per row, each block
//! bounded by a mark (caretline's docs/structure.md). The engine owns the text, the shape of
//! every block, the selection, folds, undo and the rules that edit them. thc keeps one thing
//! beside it: a [`Line`] per block, tied to the block by `Line::mark`, holding what the
//! engine can't know: the vault node id, the save state (base revision, what was saved, where)
//! and the meta. A line's shape and text are a read-only copy of its block's, re-read after
//! every engine step (`sync`), so the save diff and drawing read plain strings
//! (caretline's docs/embedding.md, "What thc keeps beside the engine").
//!
//! The host (saving, refreshes from the vault, recovery) changes the lines; those changes go
//! into the engine as one `Msg::External` before anything reads the engine again (`flush`):
//! changes from elsewhere stay out of the undo history, and the history is transformed over
//! them, so a later undo takes back only local edits. Lines the host puts in inside an
//! undo step (`Doc::undo_step`) go in as one undoable `Msg::InsertBlocks` instead.
//!
//! Node ids for new blocks come from an [`IdPool`]. Saving stays thc's: `Doc::plan_save`
//! reads the lines and makes `BlockOp`s.

use super::doc::{Doc, Line, copy_saved_state, new_id};
use super::BlockPos;
use super::tasks;
use super::Outcome;
use caretline as cn;
use cn::helix::Selection;
use cn::marks::Mark;
use cn::outline::{BlockInfo, Hang, NewBlock, OutlineConfig};
use cn::{Category, Effect, ExtChange, MarkAttrs, MarkId, Msg, OutlineLayout, Viewport};
use std::collections::{HashMap, HashSet};
use thc_core::outline::Kind;

/// Node ids for blocks the engine makes (a split, Enter, a paste). The runtime fills it ahead
/// (`Doc::fill_ids`, before input), so the model mints none; only if it runs dry within one
/// step is an id minted here.
#[derive(Default)]
pub struct IdPool(Vec<String>);

/// How many ids the runtime keeps in the pool: more than one step makes (a paste of a long
/// outline asks for more, and the pool runs dry for it).
pub const POOL: usize = 64;

impl IdPool {
    pub fn take(&mut self) -> String {
        self.0.pop().unwrap_or_else(new_id)
    }

    /// Ids minted elsewhere, for later.
    pub fn fill(&mut self, ids: impl IntoIterator<Item = String>) {
        self.0.extend(ids);
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

/// An open page or day in the engine, with thc's lines beside its blocks.
pub(crate) struct Engine {
    st: cn::State,
    /// One line per block, in order (see the module docs).
    pub(super) lines: Vec<Line>,
    /// Saved notes whose blocks went: the next save deletes them.
    pub(super) deleted: Vec<String>,
    /// Lines whose blocks went, by mark: an undo (or a paste of what was cut) can bring the
    /// mark back, and the line with it.
    graveyard: HashMap<u64, Line>,
    /// The host changed the lines since the engine last saw them.
    dirty: bool,
    /// Each line's content version (parallel to `lines`), and the epoch they're good in: an
    /// edit through the engine gives the lines it touched new versions, anything else (the
    /// host's changes, blocks coming or going) starts a new epoch. What's derived per line
    /// (rows laid out, words) is redone only for lines whose version moved (vw384).
    vers: Vec<u64>,
    ver_next: u64,
    epoch: u64,
    /// The host's pending changes are one undo step (`Doc::undo_step`).
    undoable: bool,
    host_rev: u64,
    /// When the last unsaved edit was (ms, the document's clock).
    pub(super) changed_at: Option<u64>,
    /// The document's clock (`Doc::tick`).
    pub(super) now_ms: u64,
    pool: IdPool,
    /// The document's other views, by id (`st.view` is view `current`): a page shown in the
    /// main view and in a sidebar panel is one document with two views (sidebar.md §7.1). An
    /// edit through one maps every other one (`caretline::update_doc`).
    others: Vec<(u32, cn::View)>,
    current: u32,
}

/// thc's outline (`tasks::config`): two spaces per depth, its statuses as bullet tags,
/// whole-line images.
fn cfg() -> &'static OutlineConfig {
    static CFG: std::sync::OnceLock<OutlineConfig> = std::sync::OnceLock::new();
    CFG.get_or_init(tasks::config)
}

/// A block's thc kind: a tagged bullet is a task.
fn thc_kind(b: &BlockInfo) -> Kind {
    match b.kind {
        _ if tasks::is_task(b) => Kind::Task,
        cn::Kind::Bullet => Kind::Bullet,
        _ => Kind::Para,
    }
}

/// The chars of a block's first line before thc's text: indentation and a list or task marker.
/// A number, heading or quote marker is thc's text (as it is the content of the engine's
/// `NewBlock` and `ExtChange::SetShape`).
pub(super) fn list_len(b: &BlockInfo) -> usize {
    match (b.kind, b.hang) {
        (cn::Kind::Bullet, Hang::Bullet) => b.prefix_len,
        _ => b.indent.min(b.end - b.start),
    }
}

/// A line as a block to insert.
fn new_block(l: &Line, mark: Option<MarkId>) -> NewBlock {
    let (kind, tag) = match l.kind() {
        Kind::Task => (cn::Kind::Bullet, Some(tasks::status_char(l.status.as_deref()))),
        Kind::Bullet => (cn::Kind::Bullet, None),
        Kind::Para => (cn::Kind::Para, None),
    };
    NewBlock { depth: l.depth as u16, kind, tag, text: l.text.clone(), gap: l.gap, mark }
}

/// A line as buffer text, as the engine writes a block: indentation, marker, text.
fn block_text(l: &Line) -> String {
    new_block(l, None).to_lines(cfg())
}

impl Engine {
    /// A document from thc's lines (their save state kept): the text, a mark per line, then
    /// the engine's own reading of it.
    pub(super) fn load(mut lines: Vec<Line>) -> Engine {
        let mut text = String::new();
        let mut starts = Vec::with_capacity(lines.len());
        let mut chars = 0usize;
        for (i, l) in lines.iter().enumerate() {
            if i > 0 {
                text.push('\n');
                chars += 1;
            }
            starts.push(chars);
            let t = block_text(l);
            chars += t.chars().count();
            text.push_str(&t);
        }
        let mut st = cn::State::new(&text, None, Viewport { width: 4000, height: 50 });
        st.view.config.status_bar = false;
        st.view.layout = Some(OutlineLayout::default());
        st.doc.outline = Some(cfg().clone());
        st.doc.set_host(tasks::host());
        let marks = lines.iter_mut().zip(starts).enumerate().map(|(i, (l, pos))| {
            l.mark = Some(i as u64);
            Mark { pos, id: MarkId(i as u64), attrs: MarkAttrs::gap(l.gap) }
        });
        let _ = st.doc.marks.insert_all(marks.collect());
        st.outline_changed();
        let source: HashMap<u64, Line> = lines.iter().filter_map(|l| Some((l.mark?, l.clone()))).collect();
        let mut n = Engine {
            st,
            lines,
            deleted: Vec::new(),
            graveyard: HashMap::new(),
            dirty: false,
            vers: Vec::new(),
            ver_next: 0,
            epoch: 0,
            undoable: false,
            host_rev: 0,
            changed_at: None,
            now_ms: 0,
            pool: IdPool::default(),
            others: Vec::new(),
            current: 0,
        };
        n.sync(&HashMap::new());
        // What the text can't hold exactly (a paragraph that reads as a list item, an
        // unclosed fence) is the baseline, not a change: nothing is saved for it until it's
        // edited.
        for l in n.lines.iter_mut() {
            let Some(src) = l.mark.and_then(|m| source.get(&m)) else { continue };
            if src.is_new {
                continue;
            }
            if l.kind() != src.kind() && l.saved_kind == Some(src.kind()) {
                l.saved_kind = Some(l.kind());
            }
            if l.status != src.status && l.saved_status == src.status {
                l.saved_status = l.status.clone();
            }
            if l.text != src.text && l.saved.as_deref() == Some(src.text.as_str()) {
                l.saved = Some(l.text.clone());
            }
        }
        n
    }

    /// The engine's state, for the view (`view.rs`): the host's changes are in.
    pub(super) fn state(&self) -> &cn::State {
        debug_assert!(!self.dirty, "the engine is read only once it has the host's changes");
        &self.st
    }

    pub(super) fn state_mut(&mut self) -> &mut cn::State {
        self.flush();
        &mut self.st
    }

    pub(super) fn rev(&self) -> u64 {
        self.st.doc.rev.wrapping_add(self.host_rev)
    }

    /// The lines, for the host to change; the engine takes the changes in at `settle` (or the
    /// end of the undo step).
    pub(super) fn lines_mut(&mut self) -> &mut Vec<Line> {
        self.dirty = true;
        self.host_rev = self.host_rev.wrapping_add(1);
        &mut self.lines
    }

    /// Each line's content version and their epoch (see `vers`). A version is the same as
    /// before only if the epoch is too.
    pub(super) fn line_versions(&self) -> (u64, &[u64]) {
        (self.epoch, &self.vers)
    }

    /// Every line a new version, in a new epoch.
    fn new_epoch(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        let n = self.lines.len();
        self.vers = (0..n as u64).map(|i| self.ver_next + i).collect();
        self.ver_next += n as u64;
    }

    pub(super) fn pool(&mut self) -> &mut IdPool {
        &mut self.pool
    }

    #[cfg(test)]
    pub(super) fn text(&self) -> String {
        self.st.doc.text.to_string()
    }

    pub(super) fn pool_len(&self) -> usize {
        self.pool.len()
    }

    /// The lines, for fields the engine never reads (save state, meta): nothing to take in.
    pub(super) fn lines_state_mut(&mut self) -> &mut Vec<Line> {
        self.host_rev = self.host_rev.wrapping_add(1);
        &mut self.lines
    }

    pub(super) fn lines(&self) -> &[Line] {
        &self.lines
    }

    pub(super) fn deleted(&self) -> &[String] {
        &self.deleted
    }

    pub(super) fn deleted_mut(&mut self) -> &mut Vec<String> {
        &mut self.deleted
    }

    /// How long ago (ms) the last unsaved edit was, at `now`.
    pub(super) fn changed_since(&self, now: u64) -> Option<u64> {
        self.changed_at.map(|t| now.saturating_sub(t))
    }

    pub(super) fn mark_committed(&mut self) {
        self.changed_at = None;
    }

    pub(super) fn undo_depth(&self) -> usize {
        self.st.doc.history.current_revision()
    }

    /// Whether a blank row comes before line `i`: the engine's block, its gap as set or its
    /// kind's default.
    #[cfg(test)]
    pub(super) fn effective_gap(&self, i: usize) -> bool {
        debug_assert!(!self.dirty, "the engine is read only once it has the host's changes");
        let o = self.st.doc.blocks().expect("an outline document");
        o.blocks.get(i).is_some_and(|b| b.gap)
    }

    /// Line `i`'s children are folded away (in the engine's view).
    pub(super) fn is_folded(&self, i: usize) -> bool {
        self.lines.get(i).and_then(|l| l.mark).is_some_and(|m| self.st.view.folds.contains(&MarkId(m)))
    }

    /// Fold line `i`'s children away. Test-only (no key folds yet).
    #[cfg(test)]
    pub(super) fn fold(&mut self, i: usize) {
        self.flush();
        if let Some(m) = self.lines[i].mark {
            self.st.view.folds.insert(MarkId(m));
        }
    }

    /// The host's pending changes become one undo step, taken in when it ends.
    pub(super) fn begin_undo_step(&mut self) {
        self.flush();
        self.undoable = true;
    }

    /// The undo step's changes into the engine, as one step.
    pub(super) fn end_undo_step(&mut self) {
        self.flush();
        self.undoable = false;
    }

    /// The host's changes into the engine, unless an undo step is still collecting them.
    pub(super) fn settle(&mut self) {
        if !self.undoable {
            self.flush();
        }
    }

    /// The selection, as (anchor, caret); the anchor is None when nothing is selected.
    pub(super) fn selection(&self) -> (Option<BlockPos>, BlockPos) {
        debug_assert!(!self.dirty, "the engine is read only once it has the host's changes");
        let r = self.st.view.selection.primary();
        let head = self.pos_of(r.head);
        ((r.anchor != r.head).then(|| self.pos_of(r.anchor)), head)
    }

    /// Select from `anchor` (None: nothing selected) to the caret at `head`; up and down forget
    /// their column.
    pub(super) fn select(&mut self, anchor: Option<BlockPos>, head: BlockPos) {
        self.flush();
        let h = self.char_of(head);
        let a = anchor.map_or(h, |a| self.char_of(a));
        self.st.view.selection = Selection::single(a, h);
        // Out of markers and atomic blocks, as every engine step leaves it.
        self.st.outline_changed();
    }

    /// One message through the engine: the host's changes in first, then the lines re-read.
    pub(super) fn run(&mut self, last_saved: &HashMap<String, Line>, msg: Msg) -> Vec<Effect> {
        self.flush();
        cn::update(&mut self.st, Msg::Tick { now_ms: self.now_ms });
        let rev = self.st.doc.rev;
        let fx = self.step(msg);
        if self.st.doc.rev != rev {
            self.changed_at = Some(self.now_ms);
        }
        self.sync(last_saved);
        fx
    }

    /// A host position (line, byte in its text) as a char in the engine's text: never inside a
    /// marker.
    pub(super) fn char_of(&self, p: BlockPos) -> usize {
        let o = self.st.doc.blocks().expect("an outline document");
        let b = &o.blocks[p.line.min(o.blocks.len() - 1)];
        let rope = &self.st.doc.text;
        let cs = b.start + list_len(b);
        let base = rope.char_to_byte(cs);
        let byte = p.byte.min(rope.char_to_byte(b.end) - base);
        rope.byte_to_char(base + byte).max(b.start + b.prefix_len).min(b.end)
    }

    /// An engine char as a host position.
    pub(super) fn pos_of(&self, c: usize) -> BlockPos {
        let o = self.st.doc.blocks().expect("an outline document");
        let rope = &self.st.doc.text;
        let i = o.index_at(rope.slice(..), c);
        let b = &o.blocks[i];
        let cs = b.start + list_len(b);
        let c = c.clamp(cs, b.end.max(cs));
        BlockPos { line: i, byte: rope.char_to_byte(c) - rope.char_to_byte(cs) }
    }

    /// The lines from the engine's blocks: each block's line found by its mark (its save
    /// state kept), its shape and text re-read. A block whose mark is new gets a new line; one
    /// whose mark came back gets its line back (`revive`). Saved lines whose blocks went are
    /// deleted by the next save.
    fn sync(&mut self, last_saved: &HashMap<String, Line>) {
        let touched = self.st.doc.take_touched();
        let o = self.st.doc.blocks().expect("an outline document");
        let rope = self.st.doc.text.clone();
        // The common case, an edit inside blocks: the same marks in the same order. Only the
        // blocks the text changed in are read again (a blank row can change anywhere).
        if self.lines.len() == o.blocks.len() && self.lines.iter().zip(&o.blocks).all(|(l, b)| l.mark == Some(b.id.0)) {
            let (from, to) = match touched {
                Some((a, b)) => (o.index_at(rope.slice(..), a), o.index_at(rope.slice(..), b)),
                None => (1, 0),
            };
            // A line's version moves only when what it holds did: a touched range can be wider
            // than the change (a caret move off the fresh last line touches the whole page).
            let same_len = self.vers.len() == self.lines.len();
            for (i, (l, b)) in self.lines.iter_mut().zip(&o.blocks).enumerate() {
                if (from..=to).contains(&i) {
                    let before = same_len.then(|| content_key(l));
                    read_block(l, b, &rope);
                    if before.is_some_and(|k| k != content_key(l)) {
                        self.vers[i] = self.ver_next;
                        self.ver_next += 1;
                    }
                } else {
                    l.gap = b.attrs.gap;
                }
            }
            if !same_len {
                self.new_epoch();
            }
            return;
        }
        let mut old: HashMap<u64, Line> = HashMap::with_capacity(self.lines.len());
        for l in self.lines.drain(..) {
            if let Some(m) = l.mark {
                old.insert(m, l);
            }
        }
        let mut lines = Vec::with_capacity(o.blocks.len());
        for b in &o.blocks {
            let mut l = match old.remove(&b.id.0) {
                Some(l) => l,
                None => match self.graveyard.remove(&b.id.0) {
                    Some(mut l) => {
                        revive(&mut l, &mut self.deleted, last_saved, &mut self.pool);
                        l
                    }
                    None => {
                        let mut l = Line::new(0, Kind::Para, "");
                        l.id = self.pool.take();
                        l
                    }
                },
            };
            l.mark = Some(b.id.0);
            read_block(&mut l, b, &rope);
            lines.push(l);
        }
        for (m, l) in old {
            if !l.is_new && !self.deleted.contains(&l.id) {
                self.deleted.push(l.id.clone());
            }
            self.graveyard.insert(m, l);
        }
        self.lines = lines;
        self.new_epoch();
    }

    /// The host's changes to the lines into the engine (see the module docs). True: there
    /// were some.
    pub(super) fn flush(&mut self) -> bool {
        if !self.dirty {
            return false;
        }
        self.dirty = false;
        let o = self.st.doc.blocks().expect("an outline document");
        let live: HashSet<u64> = o.blocks.iter().map(|b| b.id.0).collect();
        let mut seen = HashSet::new();
        for l in self.lines.iter_mut() {
            if let Some(m) = l.mark {
                // A copy of a line (the second with one mark) is a new line.
                if !live.contains(&m) || !seen.insert(m) {
                    l.mark = None;
                }
            }
        }
        // Blocks the host took out, then lines it put in.
        let mut changes: Vec<ExtChange> = o.blocks.iter().filter(|b| !seen.contains(&b.id.0)).map(|b| ExtChange::RemoveBlock { id: b.id }).collect();
        let mut next = self.st.doc.marks.next_id().0;
        let mut prev: Option<MarkId> = None;
        let mut steps: Vec<(Option<MarkId>, Vec<NewBlock>)> = Vec::new();
        let mut run = false;
        for l in self.lines.iter_mut() {
            if let Some(m) = l.mark {
                prev = Some(MarkId(m));
                run = false;
                continue;
            }
            let id = MarkId(next);
            next += 1;
            let nb = new_block(l, Some(id));
            if self.undoable {
                match steps.last_mut() {
                    Some(s) if run => s.1.push(nb),
                    _ => steps.push((prev, vec![nb])),
                }
            } else {
                changes.push(ExtChange::InsertBlock { after: prev, block: nb });
            }
            l.mark = Some(id.0);
            prev = Some(id);
            run = true;
        }
        let undoable = std::mem::take(&mut self.undoable);
        // Blocks came or went: every version starts over (below). Else only the lines whose
        // text or blank row the host changed get new ones.
        let structural = !changes.is_empty() || !steps.is_empty();
        self.external(changes);
        let inserted = !steps.is_empty();
        for (after, blocks) in steps {
            self.step(Msg::InsertBlocks { after, blocks });
        }
        // Each line's text, shape and blank row.
        let o = self.st.doc.blocks().expect("an outline document");
        let at: HashMap<u64, usize> = o.blocks.iter().enumerate().map(|(i, b)| (b.id.0, i)).collect();
        let rope = &self.st.doc.text;
        let mut replace: Vec<(usize, usize, String)> = Vec::new();
        let mut gaps = Vec::new();
        let mut changed_marks: HashSet<u64> = HashSet::new();
        for l in &self.lines {
            let Some(b) = l.mark.and_then(|m| at.get(&m)).map(|&i| &o.blocks[i]) else { continue };
            let want = block_text(l);
            if rope.slice(b.start..b.end) != want.as_str() {
                replace.push((b.start, b.end, want));
                changed_marks.insert(b.id.0);
            }
            if l.gap != b.attrs.gap {
                gaps.push(ExtChange::SetGap { id: b.id, gap: l.gap });
                changed_marks.insert(b.id.0);
            }
        }
        if undoable && !replace.is_empty() {
            // The host's own change (recovered text): one undo step, with the lines it put in.
            replace.sort_by_key(|r| r.0);
            self.step(Msg::Edit { changes: replace, join: inserted });
            self.external(gaps);
        } else {
            // From the end back, so each range is still where it was.
            replace.sort_by(|a, b| b.0.cmp(&a.0));
            let changes = replace.into_iter().map(|(from, to, text)| ExtChange::Replace { from, to, text }).chain(gaps).collect();
            self.external(changes);
        }
        let epoch = self.epoch;
        self.sync(&HashMap::new());
        if structural || self.vers.len() != self.lines.len() {
            if self.epoch == epoch {
                self.new_epoch();
            }
        } else {
            for (i, l) in self.lines.iter().enumerate() {
                if l.mark.is_some_and(|m| changed_marks.contains(&m)) {
                    self.vers[i] = self.ver_next;
                    self.ver_next += 1;
                }
            }
        }
        true
    }

    fn external(&mut self, changes: Vec<ExtChange>) {
        if !changes.is_empty() {
            self.step(Msg::External { changes });
        }
    }

    /// One message through the current view, every other view of the document mapped
    /// through what it changed (in a stable order by id, so a typing run stays one view's).
    fn step(&mut self, msg: Msg) -> Vec<Effect> {
        if self.others.is_empty() {
            return cn::update(&mut self.st, msg);
        }
        let mut all: Vec<(u32, cn::View)> = std::mem::take(&mut self.others);
        all.push((self.current, std::mem::take(&mut self.st.view)));
        all.sort_by_key(|(id, _)| *id);
        let acting = all.iter().position(|(id, _)| *id == self.current).unwrap_or(0);
        let (ids, mut views): (Vec<u32>, Vec<cn::View>) = all.into_iter().unzip();
        let fx = cn::update_doc(&mut self.st.doc, &mut views, acting, msg);
        for (id, v) in ids.into_iter().zip(views) {
            if id == self.current {
                self.st.view = v;
            } else {
                self.others.push((id, v));
            }
        }
        fx
    }

    /// The view messages act through, and the one `state()` shows.
    pub(super) fn current_view(&self) -> u32 {
        self.current
    }

    /// The document's views, by id.
    pub(super) fn view_ids(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.others.iter().map(|(id, _)| *id).chain([self.current]).collect();
        v.sort();
        v
    }

    /// A new view `id` (or nothing, when there is one): laid out like the current one, its
    /// caret at the start of the first note, at the top, nothing folded (sidebar.md §9).
    pub(super) fn add_view(&mut self, id: u32) {
        self.flush();
        if self.view_ids().contains(&id) {
            return;
        }
        let mut v = cn::View::new(self.st.view.viewport);
        v.config = self.st.view.config.clone();
        v.layout = self.st.view.layout.clone();
        // Every view reports its caret: which one has the keyboard is thc's to draw (the
        // terminal's cursor, or a cell of `selection` in a view without it, sidebar.md §5.1).
        // An unfocused engine view has no caret at all, so a main view added to a panel's
        // document showed none (kh7ya).
        v.focused = true;
        let start = self.char_of(BlockPos { line: 0, byte: 0 });
        v.selection = Selection::point(start);
        self.others.push((id, v));
    }

    /// Make view `id` the current one. False: the document has no such view.
    pub(super) fn use_view(&mut self, id: u32) -> bool {
        if id == self.current {
            return true;
        }
        let Some(i) = self.others.iter().position(|(v, _)| *v == id) else { return false };
        self.flush();
        let (_, mut v) = self.others.swap_remove(i);
        std::mem::swap(&mut v, &mut self.st.view);
        self.others.push((self.current, v));
        self.current = id;
        true
    }

    /// Drop view `id`; when it's the current one, another becomes current. False: it's the
    /// document's last view (or not one of its views).
    pub(super) fn remove_view(&mut self, id: u32) -> bool {
        if self.others.is_empty() {
            return false;
        }
        if id == self.current {
            let next = self.others[0].0;
            self.use_view(next);
        }
        let before = self.others.len();
        self.others.retain(|(v, _)| *v != id);
        self.others.len() != before
    }

    /// The current view takes id `id` (a document that moves from the main view to a panel).
    pub(super) fn rename_view(&mut self, id: u32) -> bool {
        if self.view_ids().contains(&id) && id != self.current {
            return false;
        }
        self.current = id;
        true
    }

    /// `text` is what the engine last copied or cut (its register).
    pub(super) fn is_register(&self, text: &str) -> bool {
        let c = &self.st.doc.clipboard;
        !c.is_empty() && (c.text == text || c.external.as_deref() == Some(text))
    }

    /// A copy across notes with each note's fields (due, priority, …) after its first line, as
    /// the vault writes them: the engine's Markdown of the same range, the fields added. It
    /// stays the register's (a paste of it is a paste of what was copied).
    pub(super) fn with_fields(&mut self, text: String) -> String {
        let fields: HashMap<u64, String> = self.lines.iter().filter(|l| !l.fields.is_empty()).filter_map(|l| Some((l.mark?, l.fields.clone()))).collect();
        if fields.is_empty() || text.is_empty() {
            return text;
        }
        let Some(o) = self.st.doc.blocks() else { return text };
        let rope = self.st.doc.text.slice(..);
        let r = self.st.view.selection.primary();
        let (i0, i1) = (o.index_at(rope, r.from()), o.index_at(rope, r.to()));
        if i0 == i1 {
            return text;
        }
        // The range the engine wrote: the selection, or the whole blocks it covers.
        let mut ranges = vec![(r.from(), r.to())];
        for k in [i1, i1.saturating_sub(1)] {
            if k >= i0 {
                ranges.push((o.blocks[i0].content_start(), o.blocks[k].end));
            }
        }
        use cn::outline::markdown::{to_markdown, to_markdown_with};
        let Some(&(from, to)) = ranges.iter().find(|&&(a, b)| to_markdown(&self.st, &o, a, b) == text) else { return text };
        let md = to_markdown_with(&self.st, &o, from, to, &|id| fields.get(&id.0).cloned());
        if md != text {
            self.st.doc.clipboard.external = Some(md.clone());
        }
        md
    }
}

/// A line's shape and text from its block.
fn read_block(l: &mut Line, b: &BlockInfo, rope: &cn::helix::Rope) {
    let kind = thc_kind(b);
    let was = l.kind();
    if kind == Kind::Task {
        let st = tasks::status_name(b.tag);
        if l.status.as_deref() != Some(st) {
            l.status = Some(st.to_string());
        }
    } else if was == Kind::Task {
        l.status = None;
    }
    l.kind = kind;
    l.depth = b.depth as usize;
    let cs = b.start + list_len(b);
    let text = rope.slice(cs..b.end.max(cs));
    if text != l.text.as_str() {
        l.text = text.to_string();
    }
    l.gap = b.attrs.gap;
}

/// A line whose mark came back (undo, or a paste of what was cut). Its delete still pending:
/// it's still in the vault, as its last save left it. Its delete landed (or it was never
/// saved): a new note, under a new id.
fn revive(l: &mut Line, deleted: &mut Vec<String>, last_saved: &HashMap<String, Line>, pool: &mut IdPool) {
    l.saving_since = None;
    if let Some(i) = deleted.iter().position(|d| *d == l.id) {
        deleted.remove(i);
        if let Some(s) = last_saved.get(&l.id) {
            copy_saved_state(l, s);
        }
        l.is_new = false;
    } else {
        l.is_new = true;
        l.id = pool.take();
    }
}

// ---- commands -----------------------------------------------------------------------------------

impl Doc {
    /// A command at the caret, by its id: one of caretline's catalog (`move.left`,
    /// `select.word_right`, `structure.indent`, `history.undo`, see `caretline::commands`) or
    /// one of thc's host commands (`thc.task_cycle`). Up, down and pages follow the view's
    /// layout (`Doc::set_view`).
    pub fn run_command(&mut self, id: &str) -> Outcome {
        if id == "edit.newline" && !self.newline_as_saved() {
            return Outcome::Done;
        }
        let msg = if id.starts_with("thc.") {
            Msg::Command { name: id.into(), args: serde_json::Value::Null }
        } else {
            match cn::command_msg(id, None) {
                Some(m) => m,
                None => return Outcome::Nothing(format!("{id} isn't a command")),
            }
        };
        let rev = self.engine.st.doc.rev;
        let fx = self.run(msg);
        let changed = self.engine.st.doc.rev != rev;
        let history = matches!(id, "history.undo" | "history.redo");
        if fx.iter().any(|e| matches!(e, Effect::Host { name, .. } if name == tasks::COMPLETED)) {
            return Outcome::Completed;
        }
        if changed {
            return if history { Outcome::Restored } else { Outcome::Done };
        }
        if history {
            return Outcome::Nothing(if id == "history.undo" { "nothing to undo" } else { "nothing to redo" }.into());
        }
        // A motion that can't go further says nothing.
        let quiet = cn::commands::command(id).is_some_and(|c| matches!(c.category, Category::Move | Category::Select));
        match fx.into_iter().rev().find_map(|e| if let Effect::Notice { text } = e { Some(text) } else { None }) {
            Some(why) if !quiet => Outcome::Nothing(why),
            _ => Outcome::Done,
        }
    }

    /// Enter, as thc keeps notes (writing.md §1; 02pjq): what the screen shows while you type is
    /// what the saved page shows when it opens again. An empty paragraph isn't saved, so on one
    /// Enter does nothing: there's no note to end, and the blank rows it used to make vanished
    /// on save (the page jumped up when it opened again). False: Enter does nothing here.
    /// (A split drops the spaces after the caret: the input rule `thc.trim_split`, tasks.rs.)
    fn newline_as_saved(&self) -> bool {
        if self.selection().is_some() {
            return true;
        }
        let l = self.caret_block();
        !(l.kind() == thc_core::outline::Kind::Para && l.text.is_empty())
    }

    /// A paste of more than one line: Markdown (unless `plain`) read into notes by the engine.
    /// How many notes, and how many images were left out.
    pub(super) fn paste_blocks(&mut self, text: &str, plain: bool) -> (usize, usize) {
        let (blocks, images) = cn::outline::markdown::parse_markdown(text, plain, cfg());
        let text = Some(text.to_string());
        self.run(if plain { Msg::PastePlain { text } } else { Msg::Paste { text } });
        (blocks.len(), images)
    }

    /// Where `p` (a host position) is in the engine's text, the host's changes taken in.
    pub(super) fn char_of(&mut self, p: BlockPos) -> usize {
        self.engine.flush();
        self.engine.char_of(p)
    }
}


/// What a line holds as the engine lays it out (its text with its marker, its blank row), as a
/// hash: a line's version moves when this does (`Engine::vers`).
fn content_key(l: &Line) -> u64 {
    use std::hash::{BuildHasher, Hash, Hasher};
    let mut h = foldhash::fast::FixedState::with_seed(0).build_hasher();
    block_text(l).hash(&mut h);
    l.gap.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::super::{BlockPos, Doc, Target};
    use super::*;
    use thc_core::outline::{Block, BlockOp};

    fn today() -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap()
    }

    fn blk(id: &str, depth: usize, kind: &str, text: &str) -> Block {
        serde_json::from_value(serde_json::json!({"id": id, "parent": null, "depth": depth, "kind": kind, "text": text, "text_rev": format!("r-{id}")})).unwrap()
    }

    fn open(blocks: &[Block]) -> Doc {
        Doc::new(Target::Journal { date: today() }, Some("root".into()), blocks, today())
    }

    fn texts(d: &Doc) -> Vec<String> {
        d.blocks().iter().map(|l| l.text.clone()).collect()
    }

    /// The engine's text and the lines agree, line for line.
    fn same(d: &mut Doc) {
        d.engine.flush();
        let want: Vec<String> = d.engine.lines.iter().map(block_text).collect();
        assert_eq!(d.engine.st.doc.text.to_string(), want.join("\n"));
    }

    /// A copy across notes carries each note's fields (as the vault writes
    /// them), and pasting it back over the same selection changes nothing.
    #[test]
    fn a_copy_across_notes_keeps_their_fields() {
        copy_keeps_fields();
    }

    fn copy_keeps_fields() {
        let mut a = blk("a", 0, "task", "Book the venue");
        a.status = Some("todo".into());
        a.due = Some("2026-10-09".into());
        let mut b = blk("b", 1, "bullet", "ask about parking");
        b.priority = Some("high".into());
        let c = blk("c", 0, "para", "After");
        let mut d = Doc::new(Target::Journal { date: today() }, Some("root".into()), &[a, b, c], today());
        d.select_range(Some(BlockPos { line: 0, byte: 0 }), BlockPos { line: 1, byte: "ask about parking".len() });
        let text = d.copy_text();
        assert_eq!(text, "- [ ] Book the venue due:2026-10-09\n  - ask about parking !high");
    }

    #[test]
    fn a_change_from_elsewhere_stays_when_local_edits_are_undone() {
        let mut d = open(&[blk("a", 0, "para", "alpha"), blk("b", 0, "para", "beta"), blk("c", 0, "para", "gamma")]);
        d.set_caret(BlockPos { line: 0, byte: 5 });
        d.insert("!");
        let p = d.patch(vec![blk("c", 0, "para", "gamma (theirs)")], vec!["b".into()], None, "root", today());
        assert!(!p.reopen);
        assert_eq!(texts(&d), ["alpha!", "gamma (theirs)"]);
        assert_eq!(d.caret(), BlockPos { line: 0, byte: 6 }, "the caret stays where it was");
        d.run_command("history.undo");
        assert_eq!(texts(&d), ["alpha", "gamma (theirs)"], "undo takes back only the local edit");
        same(&mut d);
        d.run_command("history.redo");
        assert_eq!(texts(&d), ["alpha!", "gamma (theirs)"]);
        assert!(d.plan_save(true).ops.iter().all(|o| !matches!(o, BlockOp::Delete { .. })), "a note deleted elsewhere isn't deleted again");
    }

    #[test]
    fn a_note_new_elsewhere_comes_in_at_its_place_and_undo_leaves_it() {
        let a = blk("a", 0, "bullet", "one");
        let c = blk("c", 0, "bullet", "three");
        let mut d = open(&[a.clone(), c.clone()]);
        d.set_caret(BlockPos { line: 1, byte: 5 });
        d.insert("!");
        let b = blk("b", 0, "bullet", "two");
        d.patch(vec![], vec![], Some(&[a, b, c]), "root", today());
        assert_eq!(texts(&d), ["one", "two", "three!"]);
        assert_eq!(d.blocks()[1].id, "b");
        assert_eq!(d.caret(), BlockPos { line: 2, byte: 6 });
        d.run_command("history.undo");
        assert_eq!(texts(&d), ["one", "two", "three"]);
        same(&mut d);
    }

    #[test]
    fn what_the_text_cant_hold_exactly_isnt_saved_until_edited() {
        let mut d = open(&[blk("p", 0, "para", "- reads as a list"), blk("q", 0, "para", "plain")]);
        assert_eq!(d.blocks()[0].kind(), Kind::Bullet);
        assert!(d.plan_save(true).ops.is_empty(), "{:?}", d.plan_save(true).ops);
    }

    #[test]
    fn new_notes_get_ids_and_are_created_in_place() {
        let mut d = open(&[blk("a", 0, "bullet", "one")]);
        d.set_caret(BlockPos { line: 0, byte: 3 });
        d.run_command("edit.newline");
        d.insert("two");
        d.run_command("structure.indent");
        let id = d.blocks()[1].id.clone();
        assert!(d.blocks()[1].is_new && id != "a");
        let plan = d.plan_save(true);
        assert!(plan.ops.iter().any(|o| matches!(o, BlockOp::Create { id: i, parent: Some(p), text, kind: thc_core::outline::Kind::Bullet, .. } if *i == id && p == "a" && text == "two")), "{:?}", plan.ops);
    }

    #[test]
    fn a_joined_note_undone_before_its_delete_is_saved_keeps_its_id() {
        let mut d = open(&[blk("a", 0, "para", "alpha"), blk("b", 0, "para", "beta")]);
        d.set_caret(BlockPos { line: 1, byte: 0 });
        d.run_command("edit.backspace");
        assert_eq!(texts(&d), ["alpha\nbeta"]);
        assert_eq!(d.engine.deleted, ["b"]);
        d.run_command("history.undo");
        assert_eq!(texts(&d), ["alpha", "beta"]);
        assert_eq!(d.blocks()[1].id, "b");
        assert!(!d.blocks()[1].is_new);
        assert!(d.engine.deleted.is_empty());
        assert!(d.plan_save(true).ops.is_empty());
    }

    #[test]
    fn a_joined_note_undone_after_its_delete_landed_is_new() {
        let mut d = open(&[blk("a", 0, "para", "alpha"), blk("b", 0, "para", "beta")]);
        d.set_caret(BlockPos { line: 1, byte: 0 });
        d.run_command("edit.backspace");
        let plan = d.plan_save(true);
        assert!(plan.ops.iter().any(|o| matches!(o, BlockOp::Delete { id, .. } if id == "b")));
        d.run_command("history.undo");
        assert_ne!(d.blocks()[1].id, "b");
        assert!(d.blocks()[1].is_new);
    }

    #[test]
    fn tokens_parsed_out_by_a_save_leave_the_text_outside_undo() {
        let mut d = open(&[blk("a", 0, "para", "alpha")]);
        d.caret_to_end(true);
        d.insert("call due:fri");
        d.set_caret(BlockPos { line: 0, byte: 0 });
        let plan = d.plan_save(false);
        let id = d.blocks()[1].id.clone();
        let mut b = blk(&id, 0, "para", "call");
        b.due = Some("2026-10-09".into());
        let r = thc_core::outline::OpResult { index: 0, id: id.clone(), state: "ok", error: None, block: Some(b) };
        d.apply_results(&[r], &plan.afters, &plan.parsed, &plan.sent, today());
        assert_eq!(texts(&d), ["alpha", "call"]);
        same(&mut d);
        d.run_command("history.undo");
        assert_eq!(texts(&d), ["alpha", ""], "undo takes back the typing, not the parse");
    }

    /// Recovered lines (crash recovery): changed text and lines put back are one undo step.
    #[test]
    fn recovered_text_is_one_undo_step() {
        let mut d = open(&[blk("a", 0, "para", "alpha"), blk("b", 0, "bullet", "beta")]);
        d.undo_step(|d| {
            assert!(d.set_shape("a", 0, Kind::Para, None, "alpha, typed before the crash"));
            d.insert_block(2, crate::editor::NewBlock { id: None, depth: 1, kind: Kind::Task, status: Some("todo".into()), text: "a lost task".into() });
        });
        assert_eq!(texts(&d), ["alpha, typed before the crash", "beta", "a lost task"]);
        same(&mut d);
        d.run_command("history.undo");
        assert_eq!(texts(&d), ["alpha", "beta"], "one undo takes the recovery back");
        same(&mut d);
        d.run_command("history.redo");
        assert_eq!(texts(&d), ["alpha, typed before the crash", "beta", "a lost task"]);
    }

    #[test]
    fn an_attachment_goes_in_as_one_undo_step() {
        let mut d = open(&[blk("a", 0, "para", "alpha")]);
        d.caret_to_end(true);
        let ids = d.insert_blocks(&["![shot](files/a.png)", ""]);
        assert_eq!(texts(&d), ["alpha", "![shot](files/a.png)", ""]);
        assert_eq!(d.blocks()[1].id, ids[0]);
        assert_eq!(d.caret().line, 2);
        same(&mut d);
        d.run_command("history.undo");
        assert_eq!(texts(&d), ["alpha"], "{:?}", texts(&d));
    }

}
#[cfg(test)]
mod widths {
    /// The engine and thc's drawing measure every grapheme the same (one width table in the
    /// document: a cell the engine counts is a cell drawn), wide and narrow alike.
    #[test]
    fn the_engine_and_the_drawing_agree_on_widths() {
        for g in ["🙂", "👨\u{200d}👩\u{200d}👧", "🇯🇵", "字", "e\u{301}", "1\u{fe0f}\u{20e3}", "❤\u{fe0f}", "👍🏽", "⚠", "⚠\u{fe0f}", "★", "→", "a", " "] {
            assert_eq!(caretline::view::display_width(g), crate::text::width(g), "{g:?}");
        }
    }
}
