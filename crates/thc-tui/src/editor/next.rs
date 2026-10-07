//! The second engine behind the seam: caretline-next (`THC_EDITOR=next`).
//!
//! A page or a day is one caretline-next outline document: one text line per row, each block
//! bounded by a mark (docs/caretline/outline.md). thc's [`Line`]s are a mirror of its blocks,
//! one per block in order, each tied to its block by `Line::mark`. A line keeps thc's save
//! state (node id, base, what was saved, the meta); its shape and text are the block's,
//! re-read after every engine step (`sync`).
//!
//! The host (saving, refreshes from the vault, recovery) changes the mirror as it changes the
//! old engine's lines. Before the engine's next step those changes go in as one
//! `Msg::External` (`flush`): changes from elsewhere stay out of the undo history, and the
//! history is transformed over them, so a later undo takes back only local edits. Lines the
//! host puts in after `begin_undo_step` go in as one undoable `Msg::InsertBlocks` instead.
//!
//! The caret is the host's `View` (a line index and a byte), mirrored both ways: pushed into
//! the engine's selection when the host moves it, pulled back after each step.
//!
//! Node ids for new marks come from an [`IdPool`]. Saving stays thc's: `Doc::plan_save` reads
//! the mirror and makes the same `BlockOp`s.

use super::doc::{Doc, Engine, Line, Pos, copy_saved_state, engine_kind, new_id};
use super::{EditCmd, Motion, Outcome};
use caretline_next as cn;
use cn::helix::Selection;
use cn::layout::{Layout, RowPos};
use cn::marks::Mark;
use cn::outline::{BlockInfo, Hang, NewBlock, OutlineConfig};
use cn::{BlockAttrs, By, Dir, Effect, ExtChange, MarkId, Msg, OutlineLayout, Viewport};
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

/// An open page or day on caretline-next, with thc's lines mirrored.
pub(crate) struct Next {
    st: cn::State,
    /// One line per block, in order (see the module docs).
    pub(super) lines: Vec<Line>,
    /// Saved notes whose blocks went: the next save deletes them.
    pub(super) deleted: Vec<String>,
    /// Lines whose blocks went, by mark: an undo (or a paste of what was cut) can bring the
    /// mark back, and the line with it.
    graveyard: HashMap<u64, Line>,
    /// The host changed the mirror since the engine last saw it.
    dirty: bool,
    /// Lines the host puts in next are one undo step.
    undoable: bool,
    /// The host's caret, anchor and folds as last given to (or taken from) the engine.
    synced: Option<(Pos, Option<Pos>, Vec<String>)>,
    /// The host changed the mirror, so the host's caret is the one to keep.
    force_push: bool,
    host_rev: u64,
    /// When the last unsaved edit was (ms, the document's clock).
    pub(super) changed_at: Option<u64>,
    /// The document's clock (`Doc::tick`).
    pub(super) now_ms: u64,
    pool: IdPool,
    /// Layouts for `rows_of`, by (text revision, width): one per text column in use.
    row_layouts: Vec<(u64, usize, Layout)>,
}

/// thc's outline: two spaces per depth, its task vocabulary, the ⌃T cycle, whole-line images.
fn cfg() -> &'static OutlineConfig {
    static CFG: std::sync::OnceLock<OutlineConfig> = std::sync::OnceLock::new();
    CFG.get_or_init(OutlineConfig::default)
}

fn cn_kind(k: Kind) -> cn::Kind {
    match k {
        Kind::Para => cn::Kind::Para,
        Kind::Bullet => cn::Kind::Bullet,
        Kind::Task => cn::Kind::Task,
    }
}

fn thc_kind(k: cn::Kind) -> Kind {
    match k {
        cn::Kind::Para => Kind::Para,
        cn::Kind::Bullet => Kind::Bullet,
        cn::Kind::Task => Kind::Task,
    }
}

/// A task's box character for a thc status (`todo` → ' ', `done` → 'x', …).
fn status_char(cfg: &OutlineConfig, status: Option<&str>) -> char {
    status.and_then(|s| cfg.task_markers.iter().find(|m| m.name == s)).map_or(cfg.cycle[0], |m| m.ch)
}

/// A thc status for a task's box character.
fn status_name(cfg: &OutlineConfig, ch: Option<char>) -> &str {
    ch.and_then(|c| cfg.task_markers.iter().find(|m| m.ch == c)).map_or("todo", |m| m.name.as_str())
}

/// `12. ` or `12) ` at the start: a numbered item keeps its number as text.
fn numbered(text: &str) -> bool {
    let n = text.chars().take_while(|c| c.is_ascii_digit()).count();
    n > 0 && n <= 9 && (text[n..].starts_with(". ") || text[n..].starts_with(") "))
}

/// A line as buffer text: indentation and list marker, then its text (soft breaks as lines).
fn block_text(l: &Line, cfg: &OutlineConfig) -> String {
    let indent = " ".repeat(cfg.indent as usize * l.depth);
    let marker = match l.kind() {
        Kind::Task => format!("- [{}] ", status_char(cfg, l.status.as_deref())),
        Kind::Bullet if numbered(&l.text) => String::new(),
        Kind::Bullet => "- ".to_string(),
        Kind::Para => String::new(),
    };
    format!("{indent}{marker}{}", l.text)
}

/// The chars of a block's first line before thc's text: indentation and a list or task marker.
/// A number, heading or quote marker is thc's text.
fn list_len(b: &BlockInfo) -> usize {
    match (b.kind, b.hang) {
        (cn::Kind::Task, _) | (cn::Kind::Bullet, Hang::Bullet) => b.prefix_len,
        _ => b.indent.min(b.end - b.start),
    }
}

/// A line as a block to insert.
fn new_block(l: &Line, mark: Option<MarkId>, cfg: &OutlineConfig) -> NewBlock {
    let kind = cn_kind(l.kind());
    NewBlock { depth: l.depth as u16, kind, status: (kind == cn::Kind::Task).then(|| status_char(cfg, l.status.as_deref())), text: l.text.clone(), gap: l.gap, mark }
}

/// The blank row before a line by default (the engine's rule, over thc's lines): a paragraph
/// keeps one on either side (a `##` or `###` heading only above it), list items stay tight.
fn default_gap(a: Option<&Line>, b: &Line) -> bool {
    let Some(a) = a else { return false };
    let para = |l: &Line| l.kind() == Kind::Para;
    let heading = |l: &Line| {
        let n = l.text.chars().take_while(|&c| c == '#').count();
        ((1..=3).contains(&n) && l.text[n..].starts_with(' ')).then_some(n)
    };
    let after = para(a) && !matches!(heading(a), Some(2 | 3));
    let before = para(b) && heading(b).is_some();
    after || before || (para(b) && !para(a))
}

/// Messages that leave this text: kept once, for `Outcome::Nothing`.
fn intern(s: String) -> &'static str {
    static SEEN: std::sync::OnceLock<std::sync::Mutex<HashSet<&'static str>>> = std::sync::OnceLock::new();
    let mut seen = SEEN.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = seen.get(s.as_str()) {
        return s;
    }
    let s: &'static str = Box::leak(s.into_boxed_str());
    seen.insert(s);
    s
}

impl Next {
    /// A document from thc's lines (their save state kept): the text, a mark per line, then
    /// the engine's own reading of it.
    pub(super) fn load(mut lines: Vec<Line>) -> Next {
        let cfg = cfg();
        let mut text = String::new();
        let mut starts = Vec::with_capacity(lines.len());
        let mut chars = 0usize;
        for (i, l) in lines.iter().enumerate() {
            if i > 0 {
                text.push('\n');
                chars += 1;
            }
            starts.push(chars);
            let t = block_text(l, &cfg);
            chars += t.chars().count();
            text.push_str(&t);
        }
        let mut st = cn::State::new(&text, None, Viewport { width: 4000, height: 50 });
        st.view.config.status_bar = false;
        st.view.layout = Some(OutlineLayout::default());
        st.doc.outline = Some(cfg.clone());
        let marks = lines.iter_mut().zip(starts).enumerate().map(|(i, (l, pos))| {
            l.mark = Some(i as u64);
            Mark { pos, id: MarkId(i as u64), attrs: BlockAttrs { gap: l.gap } }
        });
        let _ = st.doc.marks.insert_all(marks.collect());
        st.outline_changed();
        let source: HashMap<u64, Line> = lines.iter().filter_map(|l| Some((l.mark?, l.clone()))).collect();
        let mut n = Next {
            st,
            lines,
            deleted: Vec::new(),
            graveyard: HashMap::new(),
            dirty: false,
            undoable: false,
            synced: None,
            force_push: false,
            host_rev: 0,
            changed_at: None,
            now_ms: 0,
            pool: IdPool::default(),
            row_layouts: Vec::new(),
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

    pub(super) fn rev(&self) -> u64 {
        self.st.doc.rev.wrapping_add(self.host_rev)
    }

    /// The mirror, for the host to change; the engine takes the changes in before its next
    /// step.
    pub(super) fn lines_mut(&mut self) -> &mut Vec<Line> {
        self.dirty = true;
        self.host_rev = self.host_rev.wrapping_add(1);
        &mut self.lines
    }

    pub(super) fn pool(&mut self) -> &mut IdPool {
        &mut self.pool
    }

    pub(super) fn pool_len(&self) -> usize {
        self.pool.len()
    }

    #[allow(dead_code)]
    /// The document as data: the engine's state (text, marks, history), thc's lines and what's
    /// pending. The host's changes are taken in first.
    pub(super) fn to_value(&mut self) -> serde_json::Value {
        self.flush();
        let mut graveyard: Vec<(&u64, &Line)> = self.graveyard.iter().collect();
        graveyard.sort_by_key(|(m, _)| **m);
        serde_json::json!({
            "state": serde_json::from_str::<serde_json::Value>(&self.st.to_json()).expect("the engine's state is JSON"),
            "lines": self.lines,
            "deleted": self.deleted,
            "graveyard": graveyard,
            "pool": self.pool.0,
            "changed_at": self.changed_at,
            "now_ms": self.now_ms,
        })
    }

    #[allow(dead_code)]
    pub(super) fn from_value(v: serde_json::Value) -> Result<Next, String> {
        let field = |k: &str| v.get(k).cloned().ok_or_else(|| format!("no {k}"));
        let st = cn::State::from_json(&field("state")?.to_string()).map_err(|e| e.to_string())?;
        let lines: Vec<Line> = serde_json::from_value(field("lines")?).map_err(|e| e.to_string())?;
        let deleted: Vec<String> = serde_json::from_value(field("deleted")?).map_err(|e| e.to_string())?;
        let graveyard: Vec<(u64, Line)> = serde_json::from_value(field("graveyard")?).map_err(|e| e.to_string())?;
        let pool: Vec<String> = serde_json::from_value(field("pool")?).map_err(|e| e.to_string())?;
        let mut n = Next {
            st,
            lines,
            deleted,
            graveyard: graveyard.into_iter().collect(),
            dirty: false,
            undoable: false,
            synced: None,
            force_push: true,
            host_rev: 0,
            changed_at: serde_json::from_value(field("changed_at")?).map_err(|e| e.to_string())?,
            now_ms: serde_json::from_value(field("now_ms")?).map_err(|e| e.to_string())?,
            pool: IdPool(pool),
            row_layouts: Vec::new(),
        };
        // The engine's view is laid out by the host each step; the mirror is the engine's.
        n.st.doc.touch_all();
        n.sync(&HashMap::new());
        Ok(n)
    }

    /// The mirror, for fields the engine never reads (save state, meta): nothing to take in.
    pub(super) fn lines_state_mut(&mut self) -> &mut Vec<Line> {
        self.host_rev = self.host_rev.wrapping_add(1);
        &mut self.lines
    }

    pub(super) fn undo_depth(&self) -> usize {
        self.st.doc.history.current_revision()
    }

    pub(super) fn effective_gap(&self, i: usize) -> bool {
        let l = &self.lines[i];
        match l.gap {
            Some(g) if i > 0 => g,
            _ => default_gap(i.checked_sub(1).map(|p| &self.lines[p]), l),
        }
    }

    #[cfg(test)]
    pub(super) fn default_gap(&self, i: usize) -> bool {
        default_gap(i.checked_sub(1).map(|p| &self.lines[p]), &self.lines[i])
    }

    /// The lines the host puts in next are one undo step.
    pub(super) fn begin_undo_step(&mut self) {
        self.flush();
        self.undoable = true;
    }

    /// One message through the main view: the host's changes and caret in first, then the
    /// mirror and the caret back out.
    pub(super) fn run(&mut self, view: &mut caretline::View<String>, last_saved: &HashMap<String, Line>, msg: Msg) -> Vec<Effect> {
        self.prepare(view);
        cn::update(&mut self.st, Msg::Tick { now_ms: self.now_ms });
        let rev = self.st.doc.rev;
        let fx = cn::update(&mut self.st, msg);
        if self.st.doc.rev != rev {
            self.changed_at = Some(self.now_ms);
        }
        self.sync(last_saved);
        self.pull_view(view);
        fx
    }

    /// The host's changes and caret into the engine.
    fn prepare(&mut self, view: &caretline::View<String>) {
        self.flush();
        let mut folds: Vec<String> = view.folds.iter().cloned().collect();
        folds.sort();
        let now = (view.caret, view.anchor, folds);
        if !self.force_push && self.synced.as_ref() == Some(&now) {
            return;
        }
        self.force_push = false;
        let head = self.char_of(view.caret);
        let anchor = view.anchor.map_or(head, |a| self.char_of(a));
        self.st.view.selection = Selection::single(anchor, head);
        self.st.view.folds = self.lines.iter().filter(|l| now.2.contains(&l.id)).filter_map(|l| l.mark.map(MarkId)).collect();
        // Out of markers and folds, as every engine step leaves it.
        self.st.outline_changed();
        self.synced = Some(now);
    }

    /// The engine's caret into the host's view.
    fn pull_view(&mut self, view: &mut caretline::View<String>) {
        let r = self.st.view.selection.primary();
        view.caret = self.pos_of(r.head);
        view.anchor = (r.anchor != r.head).then(|| self.pos_of(r.anchor));
        view.goal = None;
        let mut folds: Vec<String> = view.folds.iter().cloned().collect();
        folds.sort();
        self.synced = Some((view.caret, view.anchor, folds));
    }

    /// A host position (line, byte in its text) as a char in the engine's text: never inside a
    /// marker.
    pub(super) fn char_of(&self, p: Pos) -> usize {
        let o = self.st.doc.blocks().expect("an outline document");
        let b = &o.blocks[p.line.min(o.blocks.len() - 1)];
        let rope = &self.st.doc.text;
        let cs = b.start + list_len(b);
        let base = rope.char_to_byte(cs);
        let byte = p.byte.min(rope.char_to_byte(b.end) - base);
        rope.byte_to_char(base + byte).max(b.start + b.prefix_len).min(b.end)
    }

    /// An engine char as a host position.
    fn pos_of(&self, c: usize) -> Pos {
        let o = self.st.doc.blocks().expect("an outline document");
        let rope = &self.st.doc.text;
        let i = o.index_at(rope.slice(..), c);
        let b = &o.blocks[i];
        let cs = b.start + list_len(b);
        let c = c.clamp(cs, b.end.max(cs));
        Pos { line: i, byte: rope.char_to_byte(c) - rope.char_to_byte(cs) }
    }

    /// The mirror from the engine's blocks: each block's line found by its mark (its save
    /// state kept), its shape and text re-read. A block whose mark is new gets a new line; one
    /// whose mark came back gets its line back (`revive`). Saved lines whose blocks went are
    /// deleted by the next save.
    fn sync(&mut self, last_saved: &HashMap<String, Line>) {
        let cfg = cfg();
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
            for (i, (l, b)) in self.lines.iter_mut().zip(&o.blocks).enumerate() {
                if (from..=to).contains(&i) {
                    read_block(l, b, &rope, &cfg);
                } else {
                    l.gap = b.attrs.gap;
                }
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
            read_block(&mut l, b, &rope, &cfg);
            lines.push(l);
        }
        for (m, l) in old {
            if !l.is_new && !self.deleted.contains(&l.id) {
                self.deleted.push(l.id.clone());
            }
            self.graveyard.insert(m, l);
        }
        self.lines = lines;
    }

    /// The host's changes to the mirror into the engine (see the module docs). True: there
    /// were some.
    fn flush(&mut self) -> bool {
        if !self.dirty {
            return false;
        }
        self.dirty = false;
        self.force_push = true;
        let cfg = cfg();
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
            let nb = new_block(l, Some(id), &cfg);
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
        self.external(changes);
        let inserted = !steps.is_empty();
        for (after, blocks) in steps {
            cn::update(&mut self.st, Msg::InsertBlocks { after, blocks });
        }
        // Each line's text, shape and blank row.
        let o = self.st.doc.blocks().expect("an outline document");
        let at: HashMap<u64, usize> = o.blocks.iter().enumerate().map(|(i, b)| (b.id.0, i)).collect();
        let rope = &self.st.doc.text;
        let mut replace: Vec<(usize, usize, String)> = Vec::new();
        let mut gaps = Vec::new();
        for l in &self.lines {
            let Some(b) = l.mark.and_then(|m| at.get(&m)).map(|&i| &o.blocks[i]) else { continue };
            let want = block_text(l, &cfg);
            if rope.slice(b.start..b.end) != want.as_str() {
                replace.push((b.start, b.end, want));
            }
            if l.gap != b.attrs.gap {
                gaps.push(ExtChange::SetGap { id: b.id, gap: l.gap });
            }
        }
        if undoable && !replace.is_empty() {
            // The host's own change (recovered text): one undo step, with the lines it put in.
            replace.sort_by_key(|r| r.0);
            cn::update(&mut self.st, Msg::Edit { changes: replace, join: inserted });
            self.external(gaps);
        } else {
            // From the end back, so each range is still where it was.
            replace.sort_by(|a, b| b.0.cmp(&a.0));
            let changes = replace.into_iter().map(|(from, to, text)| ExtChange::Replace { from, to, text }).chain(gaps).collect();
            self.external(changes);
        }
        self.sync(&HashMap::new());
        true
    }

    fn external(&mut self, changes: Vec<ExtChange>) {
        if !changes.is_empty() {
            cn::update(&mut self.st, Msg::External { changes });
        }
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

    /// Up and down follow thc's text column (`width_of` a line at its depth).
    fn set_geometry(&mut self, width_of: &dyn Fn(&Line) -> usize) {
        let at = |d: usize| width_of(&Line::new(d, Kind::Para, "")).min(u16::MAX as usize) as u16;
        let g = OutlineLayout { column: at(0), min_column: at(64), ..OutlineLayout::default() };
        if self.st.view.layout.as_ref() != Some(&g) {
            self.st.view.layout = Some(g);
        }
    }

    /// The rows line `i` wraps into at `w` columns, as byte ranges of its text (the first from
    /// 0, its marker included, as the old wrap gives them): the engine's own wrap.
    pub(super) fn rows_of(&mut self, i: usize, w: usize, wraps: &mut super::doc::Wraps) -> Vec<(usize, usize)> {
        self.flush();
        use std::hash::{Hash, Hasher};
        let l = &self.lines[i];
        // A short plain line is one row (the engine measures an ASCII char as one cell and
        // wraps only a row that reaches the width): no layout needed.
        if l.text.len() + 1 < w && l.text.bytes().all(|b| (0x20..0x7f).contains(&b)) && (l.kind() != Kind::Para || !l.text.starts_with("```")) {
            return vec![(0, l.text.len())];
        }
        // What the block's text is made of (`block_text`), without building it.
        let mut h = super::doc::content_hasher();
        ("next", &l.text, l.depth, l.kind(), l.status.as_deref()).hash(&mut h);
        let key = (h.finish(), w);
        if let Some(r) = wraps.get(&key) {
            return r.clone();
        }
        let o = self.st.doc.blocks().expect("an outline document");
        let b = &o.blocks[i];
        // Every depth wraps at `w` here (no marks, hang or indent): one layout per width and
        // text, not one per line.
        let rev = self.st.doc.rev;
        self.row_layouts.retain(|(r, _, _)| *r == rev);
        let k = match self.row_layouts.iter().position(|(_, lw, _)| *lw == w) {
            Some(k) => k,
            None => {
                let mut view = cn::View::new(Viewport { width: 4000, height: 1 });
                view.layout = Some(OutlineLayout { marks: 0, hang: 0, indent: 0, column: w.min(3000) as u16, min_column: 1, ..OutlineLayout::default() });
                self.row_layouts.push((rev, w, Layout::of(&self.st.doc, &view)));
                self.row_layouts.len() - 1
            }
        };
        let lay = &self.row_layouts[k].2;
        let rope = &self.st.doc.text;
        let cs = b.start + list_len(b);
        let base = rope.char_to_byte(cs);
        let byte = |c: usize| rope.char_to_byte(c.max(cs)) - base;
        let mut rows = Vec::new();
        for line in b.first_line..=b.last_line() {
            let first = if line == b.first_line { b.start + b.prefix_len } else { rope.line_to_char(line) };
            let end = if line == b.last_line() { b.end } else { rope.line_to_char(line + 1) - 1 };
            let mut starts = vec![first];
            let mut row = 0;
            let lf = lay.line_format(line);
            for g in lay.formatter_at_row(RowPos { line, row: lf.before }) {
                if g.line_idx != line || g.char_idx >= end {
                    break;
                }
                if g.is_virtual() {
                    continue;
                }
                if g.visual_pos.row != row {
                    row = g.visual_pos.row;
                    if g.char_idx > *starts.last().unwrap() {
                        starts.push(g.char_idx);
                    }
                }
            }
            for k in 0..starts.len() {
                let a = if line == b.first_line && k == 0 { 0 } else { byte(starts[k]) };
                let e = starts.get(k + 1).map_or(byte(end), |&c| byte(c));
                rows.push((a, e));
            }
        }
        if wraps.len() > 20_000 {
            wraps.clear();
        }
        wraps.insert(key, rows.clone());
        rows
    }
}

/// A line's shape and text from its block.
fn read_block(l: &mut Line, b: &BlockInfo, rope: &cn::helix::Rope, cfg: &OutlineConfig) {
    let kind = thc_kind(b.kind);
    let was = l.kind();
    if kind == Kind::Task {
        let st = status_name(cfg, b.status);
        if l.status.as_deref() != Some(st) {
            l.status = Some(st.to_string());
        }
    } else if was == Kind::Task {
        l.status = None;
    }
    l.block.kind = engine_kind(kind);
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

// ---- the seam's commands on caretline-next ---------------------------------------------------

impl Doc {
    fn next_mut(&mut self) -> &mut Next {
        let Engine::Next(n) = &mut self.engine else { panic!("caretline-next") };
        n
    }

    /// An editing or motion command, as caretline-next messages.
    pub(super) fn next_apply(&mut self, cmd: EditCmd, width_of: &dyn Fn(&Line) -> usize) -> Outcome {
        self.next_mut().set_geometry(width_of);
        let mv = |dir, by, extend| Msg::Move { dir, by, extend };
        let (f, b) = (Dir::Forward, Dir::Backward);
        let msgs: Vec<Msg> = match cmd {
            EditCmd::Newline => vec![Msg::InsertNewline],
            EditCmd::SoftBreak => vec![Msg::SoftBreak],
            EditCmd::Backspace => vec![Msg::DeleteBackward],
            EditCmd::Delete => vec![Msg::DeleteForward],
            EditCmd::DeleteWordBack => vec![Msg::DeleteWordBackward],
            EditCmd::KillToEnd => vec![Msg::KillLine],
            EditCmd::KillToStart => vec![Msg::DeleteToLineStart],
            EditCmd::Indent => vec![Msg::Indent],
            EditCmd::Outdent => vec![Msg::Outdent],
            EditCmd::TaskCycle => vec![Msg::TaskCycle],
            EditCmd::MoveLine(n) => (0..n.unsigned_abs()).map(|_| Msg::MoveBlock { dir: if n < 0 { b } else { f } }).collect(),
            EditCmd::SelectAll => vec![Msg::SelectAll],
            EditCmd::Undo => vec![Msg::Undo],
            EditCmd::Redo => vec![Msg::Redo],
            EditCmd::Move { motion, select } => match motion {
                Motion::Left => vec![mv(b, By::Grapheme, select)],
                Motion::Right => vec![mv(f, By::Grapheme, select)],
                Motion::Up => vec![mv(b, By::VisualLine, select)],
                Motion::Down => vec![mv(f, By::VisualLine, select)],
                Motion::Page(n) => {
                    // A page is the view's height: this many rows.
                    self.next_mut().st.view.viewport.height = n.unsigned_abs().clamp(1, u16::MAX as usize) as u16;
                    vec![mv(if n < 0 { b } else { f }, By::Page, select)]
                }
                Motion::WordLeft => vec![mv(b, By::Word, select)],
                Motion::WordRight => vec![mv(f, By::Word, select)],
                Motion::Home => vec![mv(b, By::LineStart, select)],
                Motion::End => vec![mv(f, By::LineEnd, select)],
                Motion::DocStart => vec![mv(b, By::DocStart, select)],
                Motion::DocEnd => vec![mv(f, By::DocEnd, select)],
                Motion::NoteUp => vec![mv(b, By::Block, select)],
                Motion::NoteDown => vec![mv(f, By::Block, select)],
            },
        };
        let rev = self.next_mut().st.doc.rev;
        let mut fx = Vec::new();
        for m in msgs {
            fx.extend(self.next_run(m));
        }
        let changed = self.next_mut().st.doc.rev != rev;
        if fx.iter().any(|e| matches!(e, Effect::Completed { .. })) {
            return Outcome::Completed;
        }
        if fx.iter().any(|e| matches!(e, Effect::Restored)) {
            return Outcome::Restored;
        }
        if changed {
            return Outcome::Done;
        }
        match cmd {
            EditCmd::Undo => return Outcome::Nothing("nothing to undo"),
            EditCmd::Redo => return Outcome::Nothing("nothing to redo"),
            _ => {}
        }
        match fx.into_iter().rev().find_map(|e| if let Effect::Notice { text } = e { Some(text) } else { None }) {
            Some(why) if !matches!(cmd, EditCmd::Move { .. } | EditCmd::SelectAll) => Outcome::Nothing(intern(why)),
            _ => Outcome::Done,
        }
    }

    /// A paste of more than one line: Markdown (unless `plain`) read into notes by the engine.
    /// How many notes, and how many images were left out.
    pub(super) fn next_paste(&mut self, text: &str, plain: bool) -> (usize, usize) {
        let (blocks, images) = cn::outline::markdown::parse_markdown(text, plain);
        let text = Some(text.to_string());
        self.next_run(if plain { Msg::PastePlain { text } } else { Msg::Paste { text } });
        (blocks.len(), images)
    }

    /// `text` is what the engine last copied or cut (its register).
    pub(super) fn next_is_register(&mut self, text: &str) -> bool {
        let c = &self.next_mut().st.doc.clipboard;
        !c.is_empty() && (c.text == text || c.external.as_deref() == Some(text))
    }

    /// Where `p` (a host position) is in the engine's text, the host's changes taken in.
    pub(super) fn next_char_of(&mut self, p: Pos) -> usize {
        let n = self.next_mut();
        n.flush();
        n.char_of(p)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{BlockPos, Doc, EditCmd, EngineKind, Target, with_engine};
    use super::*;
    use thc_core::outline::{Block, BlockOp};

    fn today() -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap()
    }

    fn blk(id: &str, depth: usize, kind: &str, text: &str) -> Block {
        serde_json::from_value(serde_json::json!({"id": id, "parent": null, "depth": depth, "kind": kind, "text": text, "text_rev": format!("r-{id}")})).unwrap()
    }

    fn open(blocks: &[Block]) -> Doc {
        with_engine(EngineKind::Next, || Doc::new(Target::Journal { date: today() }, Some("root".into()), blocks, today()))
    }

    fn texts(d: &Doc) -> Vec<String> {
        d.blocks().iter().map(|l| l.text.clone()).collect()
    }

    /// The engine's text and the mirror agree, line for line.
    fn same(d: &mut Doc) {
        let n = d.next_mut();
        n.flush();
        let want: Vec<String> = n.lines.iter().map(|l| block_text(l, &cfg())).collect();
        assert_eq!(n.st.doc.text.to_string(), want.join("\n"));
    }

    const W: fn(&Line) -> usize = |_| 72;

    /// A copy across notes carries each note's fields, on both engines (as the vault writes
    /// them), and pasting it back over the same selection changes nothing.
    #[test]
    fn a_copy_across_notes_keeps_their_fields() {
        crate::editor::on_both_engines(copy_keeps_fields);
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
        assert_eq!(text, "- [ ] Book the venue due:2026-10-09\n  - ask about parking !high", "[{:?}]", d.engine());
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
        d.apply(EditCmd::Undo, &W);
        assert_eq!(texts(&d), ["alpha", "gamma (theirs)"], "undo takes back only the local edit");
        same(&mut d);
        d.apply(EditCmd::Redo, &W);
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
        d.apply(EditCmd::Undo, &W);
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
        d.apply(EditCmd::Newline, &W);
        d.insert("two");
        d.apply(EditCmd::Indent, &W);
        let id = d.blocks()[1].id.clone();
        assert!(d.blocks()[1].is_new && id != "a");
        let plan = d.plan_save(true);
        assert!(plan.ops.iter().any(|o| matches!(o, BlockOp::Create { id: i, parent: Some(p), text, kind: thc_core::outline::Kind::Bullet, .. } if *i == id && p == "a" && text == "two")), "{:?}", plan.ops);
    }

    #[test]
    fn a_joined_note_undone_before_its_delete_is_saved_keeps_its_id() {
        let mut d = open(&[blk("a", 0, "para", "alpha"), blk("b", 0, "para", "beta")]);
        d.set_caret(BlockPos { line: 1, byte: 0 });
        d.apply(EditCmd::Backspace, &W);
        assert_eq!(texts(&d), ["alpha\nbeta"]);
        assert_eq!(d.next_mut().deleted, ["b"]);
        d.apply(EditCmd::Undo, &W);
        assert_eq!(texts(&d), ["alpha", "beta"]);
        assert_eq!(d.blocks()[1].id, "b");
        assert!(!d.blocks()[1].is_new);
        assert!(d.next_mut().deleted.is_empty());
        assert!(d.plan_save(true).ops.is_empty());
    }

    #[test]
    fn a_joined_note_undone_after_its_delete_landed_is_new() {
        let mut d = open(&[blk("a", 0, "para", "alpha"), blk("b", 0, "para", "beta")]);
        d.set_caret(BlockPos { line: 1, byte: 0 });
        d.apply(EditCmd::Backspace, &W);
        let plan = d.plan_save(true);
        assert!(plan.ops.iter().any(|o| matches!(o, BlockOp::Delete { id, .. } if id == "b")));
        d.apply(EditCmd::Undo, &W);
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
        d.apply(EditCmd::Undo, &W);
        assert_eq!(texts(&d), ["alpha", ""], "undo takes back the typing, not the parse");
    }

    /// The document is data: serialized mid-session and read back, it is the same document,
    /// and the same edits after it do the same things (text, ids, caret, undo, what saves).
    #[test]
    fn a_document_survives_json_and_edits_the_same_after() {
        let blocks = [blk("a", 0, "para", "alpha beta"), blk("b", 0, "bullet", "one"), blk("c", 1, "task", "two"), blk("d", 0, "para", "gamma")];
        let mut d = open(&blocks);
        d.tick(1_000);
        d.fill_ids((0..64).map(|i| format!("id{i:010}")).collect());
        let script: Vec<Result<EditCmd, &str>> = vec![
            Ok(EditCmd::Move { motion: crate::editor::Motion::End, select: false }),
            Err(" and more"),
            Ok(EditCmd::Newline),
            Err("new note"),
            Ok(EditCmd::Indent),
            Ok(EditCmd::Move { motion: crate::editor::Motion::Down, select: false }),
            Ok(EditCmd::TaskCycle),
            Ok(EditCmd::Backspace),
            Ok(EditCmd::Undo),
            Err("x"),
            Ok(EditCmd::MoveLine(-1)),
            Ok(EditCmd::Undo),
            Ok(EditCmd::Redo),
        ];
        let run = |d: &mut Doc, step: &Result<EditCmd, &str>| match step {
            Ok(c) => {
                d.apply(*c, &W);
            }
            Err(t) => d.insert(t),
        };
        let mid = 5;
        for s in &script[..mid] {
            run(&mut d, s);
        }
        let json = d.to_json().expect("next documents are data");
        let mut e = Doc::from_json(&json).expect("reads back");
        assert_eq!(e.to_json().unwrap(), json, "the same document");
        let shape = |d: &Doc| -> Vec<(String, usize, Kind, Option<String>, String, bool)> { d.blocks().iter().map(|l| (l.id.clone(), l.depth, l.kind(), l.status.clone(), l.text.clone(), l.is_new)).collect() };
        for (k, s) in script[mid..].iter().enumerate() {
            run(&mut d, s);
            run(&mut e, s);
            assert_eq!(shape(&d), shape(&e), "step {k} {s:?}");
            assert_eq!(d.caret(), e.caret(), "step {k} {s:?}");
        }
        assert_eq!(format!("{:?}", d.plan_save(true).ops), format!("{:?}", e.plan_save(true).ops));
    }

    /// Recovered lines (crash recovery): changed text and lines put back are one undo step.
    #[test]
    fn recovered_text_is_one_undo_step() {
        let mut d = open(&[blk("a", 0, "para", "alpha"), blk("b", 0, "bullet", "beta")]);
        d.begin_undo_step();
        assert!(d.set_shape("a", 0, Kind::Para, None, "alpha, typed before the crash"));
        d.insert_block(2, crate::editor::NewBlock { id: None, depth: 1, kind: Kind::Task, status: Some("todo".into()), text: "a lost task".into() });
        assert_eq!(texts(&d), ["alpha, typed before the crash", "beta", "a lost task"]);
        same(&mut d);
        d.apply(EditCmd::Undo, &W);
        assert_eq!(texts(&d), ["alpha", "beta"], "one undo takes the recovery back");
        same(&mut d);
        d.apply(EditCmd::Redo, &W);
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
        d.apply(EditCmd::Undo, &W);
        assert_eq!(texts(&d), ["alpha"], "{:?}", texts(&d));
    }

    /// The rows the document draws are the engine's (motion uses the same), and they are the
    /// old wrap's: words move whole, a word that fills the row keeps its space at the row's
    /// end (the next row starts with the next word), and only a word longer than a row breaks.
    #[test]
    fn rows_follow_the_engines_wrap() {
        let texts = [
            "one two three four five six seven eight nine ten eleven",
            "a\nb",
            "",
            "trailing\n",
            "漢字漢字漢字漢字漢字漢字漢字漢字漢字漢字漢字漢字",
            "👨\u{200d}👩\u{200d}👧 family 👨\u{200d}👩\u{200d}👧 again and again and again",
            &"x".repeat(50),
            "The quick brown fox jumps over the lazy dog and keeps running through the long grass until dusk.",
            "aaaa bbbbbbbbbbbbbbb cc",
            "aaaa bbbbbbbbbbbbbbbb cc",
            "0123456789012345678 01234567890123456789 x",
            "a 🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂 b 字字字字字 🇯🇵🇯🇵 e\u{301}e\u{301}",
        ];
        let blocks: Vec<Block> = texts.iter().enumerate().map(|(i, t)| blk(&format!("n{i}"), 0, "para", t)).collect();
        let mut d = open(&blocks);
        let mut wraps = super::super::doc::Wraps::default();
        for w in [20, 24, 33, 40] {
            for (i, t) in texts.iter().enumerate() {
                let rows = d.next_mut().rows_of(i, w, &mut wraps);
                let old = crate::text::wrap(t, w);
                assert_eq!(rows.len() >= old.len(), true, "{t:?} at {w}: {rows:?} vs {old:?}");
                let mut at = 0;
                for &(a, b) in &rows {
                    assert!(a == at || (a == at + 1 && &t[at..a] == "\n"), "{t:?} at {w}: contiguous {rows:?}");
                    assert!(t.is_char_boundary(a) && t.is_char_boundary(b));
                    assert!(crate::text::width(t[a..b].trim_end()) <= w, "{t:?} at {w}: {rows:?}");
                    at = b;
                }
                assert_eq!(at, t.len(), "{t:?} at {w}: {rows:?}");
                assert_eq!(rows, old, "{t:?} at {w}: the old wrap's rows");
            }
        }
    }
}
