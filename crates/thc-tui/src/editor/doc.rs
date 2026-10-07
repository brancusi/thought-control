//! The document editor (docs/design/tui-editor.md): a page or a journal day as one buffer you
//! type into. Lines are nodes; the core `outline` module turns the buffer into block ops (one
//! transaction per save, `base` on every edit so a concurrent change becomes a conflict).
//!
//! This file is the pure part: thc's lines (a block's shape and text plus its save state) and
//! the save diff. The editing rules and undo are caretline's (`next.rs` mirrors its blocks).
//! The rest of thc-tui reaches it only through the seam in `editor/mod.rs`.

use super::BlockPos;
use std::collections::{HashMap, HashSet};
use thc_core::outline::{Block, BlockOp, Kind};

/// Crockford base32 as the core writes ids (FORMAT.md).
pub fn new_id() -> String {
    thc_core::id::new_id()
}

/// One line of the document: a node, its shape and its save state.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Line {
    /// The node's id.
    pub id: String,
    pub depth: usize,
    /// Paragraph, list item or task (read it with [`Line::kind`]).
    pub(crate) kind: Kind,
    /// A task's status (`todo`, `done`, …).
    pub status: Option<String>,
    /// Its text; soft breaks are `\n`.
    pub text: String,
    /// Whether a blank line comes before it (thc's `gap`): None is the default for its kind.
    pub gap: Option<bool>,
    /// The meta, as shown (`due fri · !high`), from the last save or read.
    pub meta: String,
    /// The fields as tokens, for copy (` due:2026-10-09 !high`).
    pub fields: String,
    /// The text revision this line started from (an edit's base).
    pub base: Option<String>,
    pub saved: Option<String>,
    pub saved_parent: Option<String>,
    pub saved_after: Option<String>,
    pub saved_kind: Option<Kind>,
    pub saved_status: Option<String>,
    pub is_new: bool,
    pub conflict: bool,
    /// The meta flashes accent until then (values settling after a save), in ms ([`super::ms`]).
    pub flash_until: Option<u64>,
    /// A save in flight since (◌ after 3 s), in ms ([`super::ms`]).
    pub saving_since: Option<u64>,
    pub save_error: Option<String>,
    /// Changed elsewhere while you're on it (§9): the new text, applied when you leave.
    pub remote_text: Option<String>,
    /// Its kind, parent or place changed elsewhere, or it was deleted, while you were on it (or
    /// had typed on it): taken up when you leave it (two-device soak: it kept the old kind, or
    /// stayed after its deletion, for good).
    pub remote_shape: bool,
    /// Who else edited a ≠ line (`claude`).
    pub conflict_with: Option<String>,
    /// The `gap` (writing.md §1) as last saved; the live one is the block's.
    pub saved_gap: Option<bool>,
    /// The engine's mark for this line: which block of the text it is.
    /// None for a line the host just made, until the engine takes it in.
    pub(super) mark: Option<u64>,
}

impl Line {
    pub fn new(depth: usize, kind: Kind, text: &str) -> Line {
        Line {
            id: new_id(),
            depth,
            kind,
            status: (kind == Kind::Task).then(|| "todo".to_string()),
            text: text.to_string(),
            gap: None,
            meta: String::new(),
            fields: String::new(),
            base: None,
            saved: None,
            saved_parent: None,
            saved_after: None,
            saved_kind: None,
            saved_status: None,
            is_new: true,
            conflict: false,
            flash_until: None,
            saving_since: None,
            save_error: None,
            remote_text: None,
            remote_shape: false,
            conflict_with: None,
            saved_gap: None,
            mark: None,
        }
    }

    pub fn from_block(b: &Block, today: chrono::NaiveDate) -> Line {
        let mut l = Line::new(b.depth, b.kind, &b.text);
        l.id = b.id.clone();
        l.status = b.status.clone();
        l.is_new = false;
        l.base = b.text_rev.clone();
        l.saved = Some(b.text.clone());
        l.saved_parent = b.parent.clone();
        l.saved_kind = Some(b.kind);
        l.saved_status = b.status.clone();
        l.conflict = b.conflict;
        l.gap = b.gap;
        l.saved_gap = b.gap;
        l.take_fields(b, today);
        l
    }

    /// Take a block's fields (the meta and the copy form).
    pub fn take_fields(&mut self, b: &Block, today: chrono::NaiveDate) {
        self.meta = meta_text(b, today);
        self.fields = thc_core::outline::fields(b.scheduled.as_deref(), b.due.as_deref(), b.priority.as_deref(), b.repeat.as_deref());
    }

    /// The line's kind (paragraph, list item or task).
    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn edited(&self) -> bool {
        self.is_new || self.saved.as_deref() != Some(self.text.as_str())
    }
}

/// `due fri · !high · ↻ every 3d` in words, as the TUI's node rows say it.
pub fn meta_text(b: &Block, today: chrono::NaiveDate) -> String {
    let mut parts: Vec<String> = Vec::new();
    let day = |s: &str| -> String {
        let Some(d) = thc_core::dates::DateVal::from_stored(s) else { return s.to_string() };
        let date = d.date();
        let n = (date - today).num_days();
        let word = match n {
            0 => "today".to_string(),
            1 => "tomorrow".to_string(),
            -1 => "yesterday".to_string(),
            2..=6 => date.format("%a").to_string().to_lowercase(),
            _ => date.format("%b %d").to_string().to_lowercase(),
        };
        match d.time() {
            Some(t) => format!("{word} {}", t.format("%H:%M")),
            None => word,
        }
    };
    if b.status.as_deref() == Some("done") {
        parts.push(match b.done_at.as_deref() {
            Some(d) if d.starts_with(&today.format("%Y-%m-%d").to_string()) => format!("done {}", d.get(11..16).unwrap_or("")),
            Some(d) => format!("done {}", day(d)),
            None => "done".into(),
        });
    }
    if let Some(s) = &b.scheduled {
        parts.push(day(s));
    }
    if let Some(d) = &b.due {
        parts.push(format!("due {}", day(d)));
    }
    if let Some(p) = &b.priority {
        parts.push(format!("!{p}"));
    }
    if let Some(r) = &b.repeat {
        parts.push(format!("↻ {}", short_repeat(r)));
    }
    parts.join(" · ")
}

/// A repeat in the meta's short form (`↻ monthly`, `↻ 3d`, `↻ 3mo!`, `↻ weekdays`), in lists
/// and documents alike; the full rule is in the detail pane. `!`
/// marks a repeat counted from completion; months are `mo` (`m` reads as minutes).
pub fn short_repeat(rule: &str) -> String {
    let rule = rule.trim();
    let from_done = rule.starts_with("every!");
    let short = short_repeat_rule(rule.trim_start_matches("every!").trim_start_matches("every").trim());
    if from_done { format!("{short}!") } else { short }
}

fn short_repeat_rule(r: &str) -> String {
    let words: Vec<&str> = r.split_whitespace().collect();
    let unit = |w: &str| -> Option<&'static str> {
        Some(match w.trim_end_matches('s') {
            "day" => "d",
            "week" => "w",
            "month" => "mo",
            "year" => "y",
            _ => return None,
        })
    };
    match words.as_slice() {
        [] => "repeats".into(),
        ["day", ..] | ["daily", ..] => "daily".into(),
        ["weekday", ..] | ["weekdays", ..] => "weekdays".into(),
        ["week", ..] | ["weekly", ..] => "weekly".into(),
        ["month", ..] | ["monthly", ..] => "monthly".into(),
        ["year", ..] | ["yearly", ..] => "yearly".into(),
        [n, u, ..] if n.parse::<u32>().is_ok() && unit(u).is_some() => format!("{n}{}", unit(u).unwrap()),
        [one] if one.len() <= 10 => (*one).to_string(),
        _ => r.chars().take(12).collect(),
    }
}

/// A caret position the host keeps (a line and a byte offset in its text); the seam's is
/// [`BlockPos`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub(super) struct Pos {
    pub line: usize,
    pub byte: usize,
}

/// Where you are in a document: the caret, the selection's other end, folds (the main
/// column's view; the engine's view follows it).
#[derive(Clone, Debug, Default)]
pub(super) struct HostView {
    pub caret: Pos,
    pub anchor: Option<Pos>,
    pub goal: Option<usize>,
    pub folds: HashSet<String>,
}

/// What the document is.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Target {
    Page { id: String, title: String },
    Journal { date: chrono::NaiveDate },
}

pub struct Doc {
    pub target: Target,
    /// The document's root node (None: a journal day not created yet; its first save makes it).
    pub root: Option<String>,
    /// The lines, pending deletes and undo: the engine's document, whose rules edit it, shared
    /// by every view of the page or day.
    pub(super) engine: Box<super::next::Next>,
    /// Where you are in it: caret, selection, goal column, folds (the main column's view).
    pub(super) view: HostView,
    /// The first visual row on screen.
    pub scroll: usize,
    /// Content changes the buffer doesn't make (remote text, a line added for typing).
    host_revision: u64,
    /// Each line as its last save left it (what the vault has); see `Doc::undo`.
    pub(super) last_saved: HashMap<String, Line>,
    pub(super) wraps: Wraps,
    /// The word count at a revision (the footer shows it every frame).
    words: std::cell::Cell<Option<(u64, usize)>>,
    /// The clock as the runtime last gave it ([`Doc::tick`], ms on [`super::ms`]'s scale).
    pub(super) now_ms: u64,
}

/// A document as data (the model is serializable): the target, the
/// engine's document and view, thc's lines (save state included), what's pending, and the
/// main column's caret, selection, folds and scroll.
#[derive(serde::Serialize, serde::Deserialize)]
#[allow(dead_code)] // Replay fixtures and tests now; crash recovery next.
struct DocJson {
    target: Target,
    root: Option<String>,
    engine: serde_json::Value,
    caret: (usize, usize),
    anchor: Option<(usize, usize)>,
    folds: Vec<String>,
    scroll: usize,
    last_saved: HashMap<String, Line>,
    now_ms: u64,
}

#[allow(dead_code)] // Replay fixtures and tests now; crash recovery next.
impl Doc {
    /// The document as JSON.
    pub fn to_json(&mut self) -> Option<String> {
        let n = &mut self.engine;
        let mut folds: Vec<String> = self.view.folds.iter().cloned().collect();
        folds.sort();
        let j = DocJson {
            target: self.target.clone(),
            root: self.root.clone(),
            engine: n.to_value(),
            caret: (self.view.caret.line, self.view.caret.byte),
            anchor: self.view.anchor.map(|a| (a.line, a.byte)),
            folds,
            scroll: self.scroll,
            last_saved: self.last_saved.clone(),
            now_ms: self.now_ms,
        };
        Some(serde_json::to_string(&j).expect("a document serializes"))
    }

    /// A document from [`Doc::to_json`].
    pub fn from_json(json: &str) -> Result<Doc, String> {
        let j: DocJson = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let next = super::next::Next::from_value(j.engine)?;
        let mut view = HostView::default();
        view.caret = Pos { line: j.caret.0, byte: j.caret.1 };
        view.anchor = j.anchor.map(|(line, byte)| Pos { line, byte });
        view.folds = j.folds.into_iter().collect();
        Ok(Doc {
            target: j.target,
            root: j.root,
            engine: Box::new(next),
            view,
            scroll: j.scroll,
            host_revision: 0,
            last_saved: j.last_saved,
            wraps: Wraps::default(),
            words: Default::default(),
            now_ms: j.now_ms,
        })
    }
}

/// Rows by (a line's content hash, width).
pub(super) type Wraps = HashMap<(u64, usize), Vec<(usize, usize)>, foldhash::fast::FixedState>;

/// A fast, fixed-seed hasher for content keys.
pub(super) fn content_hasher() -> impl std::hash::Hasher {
    use std::hash::BuildHasher;
    foldhash::fast::FixedState::with_seed(0).build_hasher()
}

impl Doc {
    pub fn new(target: Target, root: Option<String>, blocks: &[Block], today: chrono::NaiveDate) -> Doc {
        let mut lines: Vec<Line> = blocks.iter().map(|b| Line::from_block(b, today)).collect();
        // The notes that can still be a predecessor, depths increasing (see `plan_save`).
        let mut before: Vec<(usize, String)> = Vec::new();
        for l in lines.iter_mut() {
            l.saved_after = place(l.depth, &before).1;
            while before.last().is_some_and(|(d, _)| *d >= l.depth) {
                before.pop();
            }
            before.push((l.depth, l.id.clone()));
        }
        let view = HostView::default();
        let engine = Box::new(super::next::Next::load(lines));
        Doc { target, root, engine, view, scroll: 0, host_revision: 0, last_saved: HashMap::new(), wraps: Wraps::default(), words: Default::default(), now_ms: 0 }
    }

    /// Content generation, independent of caret motion and undo coalescing.
    pub fn revision(&self) -> u64 {
        self.engine.rev().wrapping_add(self.host_revision)
    }

    /// How many ids the document wants handed in before input ([`Doc::fill_ids`]): the model
    /// mints none.
    pub fn ids_wanted(&self) -> usize {
        super::next::POOL.saturating_sub(self.engine.pool_len())
    }

    /// Ids minted by the runtime, for the notes the next edits make.
    pub fn fill_ids(&mut self, ids: Vec<String>) {
        self.engine.pool().fill(ids);
    }

    /// An id for a note this document makes (from the pool).
    pub(super) fn take_id(&mut self) -> String {
        self.engine.pool().take()
    }

    /// The host's changes to the lines, taken into the engine now (the engine takes them
    /// before its next step; a caller about to read the undo depth wants them in).
    pub fn take_host_changes(&mut self) {
        self.engine.take_host_changes();
    }

    /// The runtime's clock (ms, [`super::ms`]), before it hands the document input: idle saves,
    /// flashes and the engine's typing runs read it; the model reads no clock itself.
    pub fn tick(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
        self.engine.now_ms = now_ms;
    }

    /// The engine's text (tests: the mirror against it).
    #[cfg(test)]
    pub fn engine_text(&self) -> Option<String> {
        Some(self.engine.text())
    }

    /// The document's words (its own lines), remembered for the revision.
    pub fn word_count(&self) -> usize {
        let rev = self.revision();
        if let Some((_, n)) = self.words.get().filter(|(r, _)| *r == rev) {
            return n;
        }
        let n = self.lines().iter().map(|l| l.text.split_whitespace().count()).sum();
        self.words.set(Some((rev, n)));
        n
    }

    /// Invalidate content-derived inputs after a local edit or deferred remote text.
    pub(super) fn touch_content(&mut self) {
        self.host_revision = self.host_revision.wrapping_add(1);
    }

    pub(super) fn lines(&self) -> &[Line] {
        self.engine.lines()
    }

    /// The lines, for thc's own bookkeeping (save state, meta). Views aren't rebased.
    pub(super) fn lines_mut(&mut self) -> &mut Vec<Line> {
        self.engine.lines_mut()
    }

    /// The lines, for fields the engine never reads: save state, meta, marks (never text,
    /// shape or gap).
    pub(super) fn lines_state_mut(&mut self) -> &mut Vec<Line> {
        self.engine.lines_state_mut()
    }

    /// The caret's line.
    pub(super) fn line(&self) -> &Line {
        &self.lines()[self.view.caret.line]
    }

    /// The selection, ordered (start, end), when there is one.
    pub fn selection(&self) -> Option<(BlockPos, BlockPos)> {
        let a = self.view.anchor?;
        let c = self.view.caret;
        let ok = |p: Pos| self.lines().get(p.line).is_some_and(|l| p.byte <= l.text.len() && l.text.is_char_boundary(p.byte));
        (a != c && ok(a) && ok(c)).then(|| if a < c { (a.into(), c.into()) } else { (c.into(), a.into()) })
    }

    /// A message for caretline through the main view, the vault's save state at hand.
    pub(super) fn next_run(&mut self, msg: caretline::Msg) -> Vec<caretline::Effect> {
        let Doc { engine, view, last_saved, .. } = self;
        engine.run(view, last_saved, msg)
    }

    pub fn insert(&mut self, s: &str) {
        self.next_run(caretline::Msg::InsertText { text: s.to_string() });
    }

    pub fn newline(&mut self) {
        self.next_run(caretline::Msg::InsertNewline);
    }

    pub fn delete_selection(&mut self) -> bool {
        if self.selection().is_none() {
            return false;
        }
        self.next_run(caretline::Msg::DeleteBackward);
        true
    }

    /// A click on a task's box: its next status.
    pub fn task_box(&mut self, line: usize) -> &'static str {
        let Some(l) = self.lines().get(line) else { return "" };
        let (Some(mark), Kind::Task) = (l.mark, l.kind()) else { return "" };
        let done = l.status.as_deref() == Some("done");
        let ch = if done { ' ' } else { 'x' };
        self.next_run(caretline::Msg::SetStatus { id: caretline::MarkId(mark), ch });
        if done { "reopened" } else { "done" }
    }

    /// One undo step before the host puts lines in (recovered lines, an attachment).
    pub fn begin_undo_step(&mut self) {
        self.engine.begin_undo_step();
    }

    pub(super) fn select(&mut self, select: bool) {
        if !select {
            self.view.anchor = None;
        } else if self.view.anchor.is_none() {
            self.view.anchor = Some(self.view.caret);
        }
    }

    /// A double-click: the word at `p`.
    pub fn select_word_at(&mut self, p: BlockPos) {
        let pos = self.next_char_of(p.into());
        self.next_run(caretline::Msg::SelectWordAt { pos });
    }

    /// A triple-click: the whole note at line `line`.
    pub fn select_block(&mut self, line: usize) {
        if let Some(m) = self.lines().get(line).and_then(|l| l.mark) {
            self.next_run(caretline::Msg::SelectBlock { id: caretline::MarkId(m) });
        }
    }

    pub fn selected_parts(&mut self) -> Vec<(usize, String)> {
        let Some((s, e)) = self.selection() else { return Vec::new() };
        (s.line..=e.line)
            .map(|i| {
                let t = &self.lines()[i].text;
                let a = if i == s.line { s.byte } else { 0 };
                let b = if i == e.line { e.byte } else { t.len() };
                (i, t[a.min(b)..b].to_string())
            })
            .collect()
    }

    pub fn undo_depth(&self) -> usize {
        self.engine.undo_depth()
    }

    pub fn effective_gap(&self, i: usize) -> bool {
        self.engine.effective_gap(i)
    }

    #[cfg(test)]
    pub fn default_gap(&self, i: usize) -> bool {
        self.engine.default_gap(i)
    }

    #[cfg(test)]
    pub fn gaps(&self) -> HashMap<String, bool> {
        self.lines().iter().enumerate().map(|(i, l)| (l.id.clone(), self.effective_gap(i))).collect()
    }

    /// How many lines nest under line `i`.
    pub fn descendants(&self, i: usize) -> usize {
        let lines = self.lines();
        lines[i + 1..].iter().take_while(|l| l.depth > lines[i].depth).count()
    }

    /// Put the caret at the end of the document's last line, or on a fresh line when the last
    /// line has text (a journal day: ready to type).
    pub fn caret_to_end(&mut self, fresh_line: bool) {
        let lines = self.engine.lines();
        if lines.is_empty() || (fresh_line && !lines.last().unwrap().text.is_empty()) {
            self.engine.lines_mut().push(Line::new(0, Kind::Para, ""));
            self.touch_content();
        }
        let i = self.lines().len() - 1;
        self.view.caret = Pos { line: i, byte: self.lines()[i].text.len() };
        self.view.anchor = None;
    }
}

/// A line's save state as its last save left it (a line coming back while its delete is
/// pending).
pub(super) fn copy_saved_state(l: &mut Line, s: &Line) {
    l.base = s.base.clone();
    l.saved = s.saved.clone();
    l.saved_parent = s.saved_parent.clone();
    l.saved_after = s.saved_after.clone();
    l.saved_kind = s.saved_kind;
    l.saved_status = s.saved_status.clone();
    l.saved_gap = s.saved_gap;
}

/// Where a line lives given the lines before it: its parent (None = the root) and the sibling
/// it follows (None = first).
pub fn place(depth: usize, before: &[(usize, String)]) -> (Option<String>, Option<String>) {
    let (parent, after) = place_by(depth, before.len(), |i| (before[i].0, before[i].1.as_str()));
    (parent.map(str::to_string), after.map(str::to_string))
}

/// [`place`] over `n` notes read by index, borrowing their ids.
fn place_by<'a>(depth: usize, n: usize, at: impl Fn(usize) -> (usize, &'a str)) -> (Option<&'a str>, Option<&'a str>) {
    let mut parent = None;
    let mut after = None;
    for i in (0..n).rev() {
        let (d, id) = at(i);
        if d == depth && after.is_none() && parent.is_none() {
            after = Some(id);
        }
        if d < depth {
            parent = Some(id);
            break;
        }
    }
    if depth > 0 && parent.is_none() {
        after = None;
    }
    (parent, after)
}

// ---- saving ---------------------------------------------------------------------------------------

/// What a save sends: the ops, and which line each `after` / parse belongs to.
pub struct SavePlan {
    pub ops: Vec<BlockOp>,
    /// Line ids whose text is parsed (tokens leave the text on this save).
    pub parsed: Vec<String>,
    /// The `after` each moved / created line was sent with.
    pub afters: HashMap<String, Option<String>>,
    /// Each line as the plan saw it.
    pub sent: Sent,
}

/// What a save saw when it was planned. A result folds into a line only as far as the line is
/// still what was sent; an edit made while the save was out stays, and the next save sends it.
#[derive(Default, Clone, Debug)]
pub struct Sent {
    /// Each line's (text, status).
    pub lines: HashMap<String, (String, Option<String>)>,
    /// The lines this save creates: one gone from the buffer by the time it's made (joined,
    /// cut, undone) is deleted by the next save.
    pub created: std::collections::HashSet<String>,
}

impl Doc {
    /// The ops that bring the server to the buffer: every changed line except the caret's
    /// (unless `all`), in document order; `raw` saves the caret's line as text only (the idle
    /// save).
    pub fn plan_save(&mut self, all: bool) -> SavePlan {
        // A note is at most one deeper than the note above it (an empty line isn't a note): an
        // edit that removed or flattened a parent leaves no gap the vault can't hold (fuzz).
        // (Each pass reads first and changes only when it must: a change to
        // the lines is taken into the engine before its next step.)
        let clamp = |lines: &[Line]| {
            let mut above: Option<usize> = None;
            let mut out = Vec::new();
            for (i, l) in lines.iter().enumerate() {
                if l.text.trim().is_empty() {
                    continue;
                }
                let max = above.map_or(0, |d| d + 1);
                if l.depth > max {
                    out.push((i, max));
                }
                above = Some(l.depth.min(max));
            }
            out
        };
        let deeper = clamp(self.engine.lines());
        if !deeper.is_empty() {
            let lines = self.engine.lines_mut();
            for (i, d) in deeper {
                lines[i].depth = d;
            }
        }
        let caret = self.view.caret.line;
        // A saved note emptied to nothing (and left) goes: an empty line isn't a note, and an
        // empty node kept its old place, which other notes were then placed after (fuzz). The
        // line stays as a plain empty line; its children are placed by this save before the
        // delete runs (deletes go last).
        let emptied_at = |i: usize, l: &Line| !l.is_new && l.text.trim().is_empty() && (all || i != caret) && !l.conflict;
        let mut emptied = Vec::new();
        let n = self.engine.lines().iter().enumerate().filter(|(i, l)| emptied_at(*i, l)).count();
        if n > 0 {
            let mut ids: Vec<String> = (0..n).map(|_| self.take_id()).collect();
            for (i, l) in self.engine.lines_mut().iter_mut().enumerate() {
                if emptied_at(i, l) {
                    emptied.push(std::mem::replace(&mut l.id, ids.pop().expect("an id each")));
                    l.is_new = true;
                    l.text.clear();
                    l.saved = None;
                    l.base = None;
                    l.saved_parent = None;
                    l.saved_after = None;
                    l.saved_kind = None;
                    l.saved_status = None;
                    l.saved_gap = None;
                    l.remote_text = None;
                }
            }
        }
        self.engine.deleted_mut().extend(emptied);
        let mut ops = Vec::new();
        let mut parsed = Vec::new();
        let mut afters = HashMap::new();
        let lines = self.engine.lines();
        // The notes placed so far that can still be a parent or a predecessor: (depth, index
        // into `lines`), depths increasing. `place` over every note placed would give the same.
        let mut present: Vec<(usize, usize)> = Vec::new();
        let push = |present: &mut Vec<(usize, usize)>, d: usize, i: usize| {
            while present.last().is_some_and(|&(pd, _)| pd >= d) {
                present.pop();
            }
            present.push((d, i));
        };
        for (i, l) in lines.iter().enumerate() {
            let skip = !all && i == caret;
            let (parent, after) = place_by(l.depth, present.len(), |k| (present[k].0, lines[present[k].1].id.as_str()));
            if l.is_new {
                let (parent, after) = (parent.map(str::to_string), after.map(str::to_string));
                // Blank (spaces only) isn't a note: capture refuses it, and a child placed under it
                // would be moved under a parent that never got made (fuzz).
                if l.text.trim().is_empty() || skip {
                    continue;
                }
                ops.push(BlockOp::Create { id: l.id.clone(), parent, after: after.clone(), kind: l.kind(), text: l.text.clone() });
                afters.insert(l.id.clone(), after);
                parsed.push(l.id.clone());
                push(&mut present, l.depth, i);
                if let Some(st) = l.status.as_deref().filter(|s| *s != "todo" && l.kind() == Kind::Task) {
                    ops.push(BlockOp::Status { id: l.id.clone(), status: st.into(), rev: None });
                }
                if l.gap.is_some() {
                    ops.push(BlockOp::Gap { id: l.id.clone(), gap: l.gap, rev: None });
                }
                continue;
            }
            // An empty line is never a parent, saved or not (the same rule as Tab's).
            if !l.text.trim().is_empty() {
                push(&mut present, l.depth, i);
            }
            // The caret's line keeps its text (still being typed), never its place: a parent
            // deleted in this save takes its children with it, so the caret's line must move
            // out first (fuzz: it was deleted with its old parent).
            let reparented = parent != l.saved_parent.as_deref() && !(parent.is_none() && l.saved_parent == self.root);
            if reparented || after != l.saved_after.as_deref() {
                let after = after.map(str::to_string);
                ops.push(BlockOp::Move { id: l.id.clone(), parent: parent.map(str::to_string), after: after.clone(), rev: None });
                afters.insert(l.id.clone(), after);
            }
            // The blank line before it is layout, saved as it changes, the caret's line too.
            if l.gap != l.saved_gap {
                ops.push(BlockOp::Gap { id: l.id.clone(), gap: l.gap, rev: None });
            }
            if skip {
                continue;
            }
            if l.edited() {
                ops.push(BlockOp::Edit { id: l.id.clone(), text: l.text.clone(), base: l.base.clone(), rev: None, raw: false });
                parsed.push(l.id.clone());
            }
            if Some(l.kind()) != l.saved_kind {
                ops.push(BlockOp::Kind { id: l.id.clone(), kind: l.kind(), rev: None });
            }
            // Kind(Task) initializes todo. A second ⌃T can already have completed the
            // live line before its kind was saved: carry that status in the same plan.
            if l.kind() == Kind::Task && l.status.is_some() && l.status != l.saved_status && (Some(l.kind()) == l.saved_kind || l.status.as_deref() != Some("todo")) {
                ops.push(BlockOp::Status { id: l.id.clone(), status: l.status.clone().unwrap(), rev: None });
            }
        }
        for id in std::mem::take(self.engine.deleted_mut()).into_iter() {
            ops.push(BlockOp::Delete { id, rev: None });
        }
        // What was sent, to tell later edits apart: nothing to send, nothing to remember.
        let sent = Sent {
            lines: self.engine.lines().iter().filter(|_| !ops.is_empty()).map(|l| (l.id.clone(), (l.text.clone(), l.status.clone()))).collect(),
            created: ops.iter().filter_map(|o| if let BlockOp::Create { id, .. } = o { Some(id.clone()) } else { None }).collect(),
        };
        SavePlan { ops, parsed, afters, sent }
    }

    /// The idle save: the caret's line, text only (tokens not read).
    pub fn plan_idle(&self) -> Option<BlockOp> {
        let l = self.line();
        (!l.is_new && l.edited()).then(|| BlockOp::Edit { id: l.id.clone(), text: l.text.clone(), base: l.base.clone(), rev: None, raw: true })
    }

    /// Fold a save's results back: new revisions and fields; a line just parsed loses its tokens
    /// from the text (only shorter, and never the caret's line).
    pub fn apply_results(&mut self, results: &[thc_core::outline::OpResult], plan_afters: &HashMap<String, Option<String>>, parsed: &[String], sent: &Sent, today: chrono::NaiveDate) -> Vec<String> {
        let now = self.now_ms;
        let mut messages = Vec::new();
        let caret_id = self.line().id.clone();
        // Lines edited since the plan (before any result here: one line can have several, a
        // create then its status): their text and status stay as they are now.
        let (moved_text, moved_status): (std::collections::HashSet<String>, std::collections::HashSet<String>) = {
            let mut t = std::collections::HashSet::new();
            let mut st = std::collections::HashSet::new();
            for l in self.engine.lines() {
                if let Some((text, status)) = sent.lines.get(&l.id) {
                    if *text != l.text {
                        t.insert(l.id.clone());
                    }
                    if *status != l.status {
                        st.insert(l.id.clone());
                    }
                }
            }
            (t, st)
        };
        // One save can carry several ops for a note (a move, then its text): each result has the
        // note as that op left it, so only its last result speaks for it. (An earlier one put
        // back the text from before the edit, and the next save sent it: typing lost, fuzz.)
        let last: HashMap<&str, usize> = results.iter().map(|r| (r.id.as_str(), r.index)).collect();
        for r in results {
            let latest = last.get(r.id.as_str()) == Some(&r.index);
            let Some(i) = self.engine.lines().iter().position(|l| l.id == r.id) else {
                // Made by this save, gone from the buffer since: the vault has it now, so it goes.
                if r.state == "ok" && sent.created.contains(&r.id) && !self.engine.deleted().contains(&r.id) {
                    self.engine.deleted_mut().push(r.id.clone());
                }
                continue;
            };
            let l = &mut self.engine.lines_mut()[i];
            l.saving_since = None;
            match r.state {
                // Only the note's last result: it has every op of this save applied.
                "ok" if !latest => {}
                "ok" => {
                    let Some(b) = &r.block else { continue };
                    l.save_error = None;
                    // Your save of this line landed (with its base, so the core kept both if they
                    // differ): a remote text held for it no longer applies; the refresh brings
                    // what the vault has. Applying it on leave hid what you'd typed (fuzz).
                    if l.text == b.text || parsed.contains(&l.id) {
                        l.remote_text = None;
                    }
                    l.is_new = false;
                    // The text's bookkeeping moves only when this save carried the text (a
                    // move or a kind change alone doesn't save what's typed: fuzz).
                    let text_sent = parsed.contains(&l.id);
                    if text_sent {
                        l.base = b.text_rev.clone();
                    }
                    l.saved_parent = b.parent.clone();
                    l.saved_kind = Some(b.kind);
                    // A status changed since the plan (⌃T again while this was out) stays: the
                    // vault's is the saved one, and the next save sends the difference.
                    if !moved_status.contains(&l.id) {
                        l.status = b.status.clone();
                    }
                    l.saved_status = b.status.clone();
                    l.saved_gap = b.gap;
                    l.conflict = b.conflict;
                    if let Some(a) = plan_afters.get(&l.id) {
                        l.saved_after = a.clone();
                    }
                    let had_meta = l.meta.clone();
                    l.take_fields(b, today);
                    if parsed.contains(&l.id) && l.id != caret_id && !moved_text.contains(&l.id) && b.text.len() < l.text.len() {
                        l.text = b.text.clone();
                        if self.view.caret.line == i {
                            self.view.caret.byte = self.view.caret.byte.min(l.text.len());
                        }
                    }
                    if parsed.contains(&l.id) && l.meta != had_meta && !l.meta.is_empty() {
                        l.flash_until = Some(now + 300);
                    }
                    if parsed.contains(&l.id) {
                        if let Some(bad) = b.text.split_whitespace().find(|w| ["due:", "sched:", "at:"].iter().any(|p| w.starts_with(p))) {
                            let v = bad.split_once(':').map(|x| x.1).unwrap_or(bad).trim_matches('"');
                            messages.push(format!("\"{v}\" kept as text"));
                        }
                    }
                    if text_sent {
                        l.saved = Some(l.text.clone()).filter(|_| l.text == b.text).or(Some(b.text.clone()));
                        if l.text != b.text && l.id == caret_id && !moved_text.contains(&l.id) {
                            // The caret's line keeps what's typed; the server has the tokens parsed out.
                            // (Not what was typed after this save was planned: that's still to send.)
                            l.saved = Some(l.text.clone());
                        }
                    }
                    if l.conflict {
                        messages.push("≠ changed elsewhere too · both kept · ⌃O compare".into());
                    }
                    let kept = l.clone();
                    self.last_saved.insert(kept.id.clone(), kept);
                }
                "stale" | "exists" => messages.push("changed elsewhere · reload to see it".into()),
                _ => {
                    l.save_error = r.error.clone();
                    messages.push(format!("not saved: {} · :retry", r.error.clone().unwrap_or_default()));
                }
            }
        }
        messages
    }
}

// ---- paste and copy --------------------------------------------------------------------------------

impl Doc {
    /// What ⌘C copies (editing.md §5): inside one note its plain text; across notes Markdown,
    /// the first note with its marker only when the selection includes the note's start, and
    /// every later note with its marker and indent. Empty: nothing selected.
    pub fn copy_text(&mut self) -> String {
        if self.selection().is_none() {
            return String::new();
        }
        let fx = self.next_run(caretline::Msg::Copy);
        let text = fx.into_iter().find_map(|f| if let caretline::Effect::ClipboardSet { text } = f { Some(text) } else { None }).unwrap_or_default();
        self.engine.with_fields(text)
    }
}

// ---- wrapping and the view ------------------------------------------------------------------------

impl Doc {
    /// The wrap of line `i` at `w` columns, cached by (text, width).
    pub fn rows_of(&mut self, i: usize, w: usize) -> Vec<(usize, usize)> {
        self.engine.rows_of(i, w, &mut self.wraps)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{EditCmd, Motion};
    use super::*;

    const W: fn(&Line) -> usize = |_| 72;

    fn doc(lines: &[(usize, Kind, &str)]) -> Doc {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
        let mut d = Doc::new(Target::Journal { date: today }, None, &[], today);
        d.set_blocks(lines.iter().map(|(dep, k, t)| Line::new(*dep, *k, t)).collect());
        d
    }

    fn texts(d: &Doc) -> Vec<String> {
        d.lines().iter().map(|l| format!("{}{:?} {}", " ".repeat(l.depth), l.kind(), l.text)).collect()
    }

    fn at(d: &mut Doc, line: usize, byte: usize) {
        d.set_caret(BlockPos { line, byte });
    }

    #[test]
    fn typing_splitting_and_merging() {
        let mut d = doc(&[(0, Kind::Para, "")]);
        d.insert("Hello world");
        at(&mut d, 0, 6);
        d.newline(); // a line break in the paragraph
        assert_eq!(texts(&d), ["Para Hello \nworld"]);
        d.newline(); // a blank line: two notes
        assert_eq!(texts(&d), ["Para Hello ", "Para world"]);
        d.apply(EditCmd::Backspace, &W); // at the start of a paragraph: join, the break kept
        assert_eq!(texts(&d), ["Para Hello \nworld"]);
        assert_eq!(d.caret(), BlockPos { line: 0, byte: 7 });
        d.apply(EditCmd::Undo, &W);
        assert_eq!(texts(&d), ["Para Hello ", "Para world"]);
        d.apply(EditCmd::Redo, &W);
        assert_eq!(texts(&d), ["Para Hello \nworld"]);
    }

    #[test]
    fn list_forms_and_nesting() {
        let mut d = doc(&[(0, Kind::Para, "")]);
        d.insert("- ");
        assert_eq!(d.lines()[0].kind(), Kind::Bullet);
        d.insert("Plan");
        d.newline();
        d.insert("Book venue");
        d.apply(EditCmd::Indent, &W);
        d.newline();
        d.newline(); // an empty item ends the list: an empty paragraph line
        assert_eq!(texts(&d), ["Bullet Plan", " Bullet Book venue", "Para "]);
        at(&mut d, 0, 4);
        d.apply(EditCmd::TaskCycle, &W);
        assert_eq!(d.lines()[0].kind(), Kind::Task);
        assert_eq!(d.apply(EditCmd::TaskCycle, &W), crate::editor::Outcome::Completed);
        assert_eq!(d.lines()[0].status.as_deref(), Some("done"));
    }

    #[test]
    fn numbered_items_continue() {
        let mut d = doc(&[(0, Kind::Bullet, "1. First")]);
        let n = d.lines()[0].text.len();
        at(&mut d, 0, n);
        d.newline();
        assert_eq!(d.lines()[1].text, "2. ");
    }

    #[test]
    fn move_with_children_and_selection_delete() {
        let mut d = doc(&[(0, Kind::Bullet, "A"), (1, Kind::Bullet, "A1"), (0, Kind::Bullet, "B")]);
        at(&mut d, 2, 0);
        d.apply(EditCmd::MoveLine(-1), &W);
        assert_eq!(texts(&d), ["Bullet B", "Bullet A", " Bullet A1"]);
        assert_eq!(d.caret().line, 0);
        assert!(matches!(d.apply(EditCmd::MoveLine(-1), &W), crate::editor::Outcome::Nothing(_)));
        d.select_range(Some(BlockPos { line: 2, byte: 1 }), BlockPos { line: 0, byte: 1 });
        d.delete_selection();
        assert_eq!(texts(&d), ["Bullet B1"]);
    }

    #[test]
    fn repeats_are_short_in_the_meta() {
        assert_eq!(short_repeat("every month on the 1st"), "monthly");
        assert_eq!(short_repeat("every 3 days"), "3d");
        assert_eq!(short_repeat("every! 3 months"), "3mo!");
        assert_eq!(short_repeat("every weekday"), "weekdays");
        assert_eq!(short_repeat("every 2 weeks"), "2w");
    }

    #[test]
    fn save_plan_places_new_lines() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
        let mut d = Doc::new(Target::Journal { date: today }, Some("root".into()), &[], today);
        d.set_blocks(vec![Line::new(0, Kind::Bullet, "A"), Line::new(1, Kind::Task, "A1"), Line::new(0, Kind::Para, "")]);
        at(&mut d, 2, 0);
        let p = d.plan_save(false);
        let creates: Vec<(String, Option<String>, Option<String>)> = p
            .ops
            .iter()
            .filter_map(|o| match o {
                BlockOp::Create { text, parent, after, .. } => Some((text.clone(), parent.clone(), after.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(creates.len(), 2, "the empty caret line isn't a node yet");
        assert_eq!(creates[0], ("A".into(), None, None));
        assert_eq!(creates[1].1, Some(d.lines()[0].id.clone()), "A1 under A");
    }

    /// Characters as people see them (jank A1–A6): the caret, Backspace and Delete never
    /// split a cluster.
    #[test]
    fn graphemes_are_one_character() {
        for g in ["👨\u{200d}👩\u{200d}👧", "❤\u{fe0f}", "🇯🇵", "e\u{301}", "漢", "a"] {
            let mut d = doc(&[(0, Kind::Para, &format!("a{g}"))]);
            let n = d.lines()[0].text.len();
            at(&mut d, 0, n);
            d.apply(EditCmd::Backspace, &W);
            assert_eq!(d.lines()[0].text, "a", "⌫ removes all of {g:?}");
            let mut d = doc(&[(0, Kind::Para, &format!("a{g}"))]);
            at(&mut d, 0, 1);
            d.apply(EditCmd::Delete, &W);
            assert_eq!(d.lines()[0].text, "a", "⌦ removes all of {g:?}");
            let mut d = doc(&[(0, Kind::Para, &format!("a{g}b"))]);
            at(&mut d, 0, 1);
            d.apply(EditCmd::Move { motion: Motion::Right, select: false }, &W);
            assert_eq!(d.caret().byte, 1 + g.len(), "→ over {g:?}");
        }
    }

    #[test]
    fn save_plan_keeps_completion_alongside_an_unsaved_task_kind() {
        let mut d = doc(&[(0, Kind::Para, "alpha line")]);
        let l = &mut d.lines_mut()[0];
        l.is_new = false;
        l.saved = Some(l.text.clone());
        l.saved_kind = Some(Kind::Para);
        d.apply(EditCmd::TaskCycle, &W);
        let first = d.plan_save(true);
        assert!(first.ops.iter().any(|op| matches!(op, BlockOp::Kind { kind: Kind::Task, .. })));
        assert!(!first.ops.iter().any(|op| matches!(op, BlockOp::Status { .. })), "Kind(Task) already initializes todo");
        d.apply(EditCmd::TaskCycle, &W);
        let second = d.plan_save(true);
        let kind = second.ops.iter().position(|op| matches!(op, BlockOp::Kind { kind: Kind::Task, .. })).unwrap();
        let status = second.ops.iter().position(|op| matches!(op, BlockOp::Status { status, .. } if status == "done")).unwrap();
        assert!(kind < status, "the completed status follows the kind that initializes todo");
    }

    /// ⌃T cycles text → [ ] → [x] → text (writing.md A9); ⌫ takes the marker off a step at a
    /// time (A10); a selection cycles together (A11).
    #[test]
    fn task_cycle_and_marker_steps() {
        let mut d = doc(&[(0, Kind::Para, "call")]);
        d.apply(EditCmd::TaskCycle, &W);
        assert_eq!((d.lines()[0].kind(), d.lines()[0].status.as_deref()), (Kind::Task, Some("todo")));
        d.apply(EditCmd::TaskCycle, &W);
        assert_eq!(d.lines()[0].status.as_deref(), Some("done"));
        d.apply(EditCmd::TaskCycle, &W);
        assert_eq!((d.lines()[0].kind(), d.lines()[0].status.as_deref()), (Kind::Para, None));
        d.apply(EditCmd::TaskCycle, &W);
        at(&mut d, 0, 0);
        d.apply(EditCmd::Backspace, &W);
        assert_eq!((d.lines()[0].kind(), d.lines()[0].status.as_deref()), (Kind::Bullet, None));
        d.apply(EditCmd::Backspace, &W);
        assert_eq!((d.lines()[0].kind(), d.lines()[0].text.as_str()), (Kind::Para, "call"));
        let mut d = doc(&[(0, Kind::Para, "a"), (0, Kind::Para, "b"), (0, Kind::Para, "c")]);
        d.select_range(Some(BlockPos { line: 0, byte: 0 }), BlockPos { line: 2, byte: 1 });
        d.apply(EditCmd::TaskCycle, &W);
        assert!(d.lines().iter().all(|l| l.kind() == Kind::Task), "{:?}", texts(&d));
    }

    /// Enter is a line break in a paragraph; a blank line splits it (writing.md A2–A4); ⌫ at a
    /// paragraph's start joins (A5); a marker typed on a later line starts a note.
    #[test]
    fn paragraphs_are_plain_text() {
        let mut d = doc(&[(0, Kind::Para, "")]);
        d.insert("one");
        d.newline();
        d.insert("two");
        assert_eq!(texts(&d), ["Para one\ntwo"], "A2: one note, two lines");
        let mut d = doc(&[(0, Kind::Para, "")]);
        d.insert("one");
        d.newline();
        d.newline();
        d.insert("two");
        assert_eq!(texts(&d), ["Para one", "Para two"], "A3: two notes");
        let mut d = doc(&[(0, Kind::Para, "abcdef")]);
        let id = d.lines()[0].id.clone();
        at(&mut d, 0, 3);
        d.newline();
        d.newline();
        assert_eq!(texts(&d), ["Para abc", "Para def"], "A4: a split");
        assert_eq!(d.lines()[0].id, id, "the first part keeps the id");
        at(&mut d, 1, 0);
        d.apply(EditCmd::Backspace, &W);
        assert_eq!(texts(&d), ["Para abc\ndef"], "A5: a join keeps the break");
        assert_eq!(d.lines()[0].id, id);
        let mut d = doc(&[(0, Kind::Para, "")]);
        d.insert("intro");
        d.newline();
        for c in ["-", " ", "milk"] {
            d.insert(c);
        }
        assert_eq!(texts(&d), ["Para intro", "Bullet milk"], "a marker on a later line starts an item");
        d.newline();
        assert_eq!(d.lines()[2].kind(), Kind::Bullet, "A6: the next item");
        d.newline();
        assert_eq!((d.lines()[2].kind(), d.lines()[2].text.as_str()), (Kind::Para, ""), "A6: an empty item ends the list");
        let mut d = doc(&[(0, Kind::Para, "")]);
        for c in ["[", " ", "]", " ", "call"] {
            d.insert(c);
        }
        d.newline();
        assert_eq!((d.lines()[1].kind(), d.lines()[1].status.as_deref()), (Kind::Task, Some("todo")), "A8");
        let mut d = doc(&[(0, Kind::Para, "")]);
        for c in ["#", " ", "Title"] {
            d.insert(c);
        }
        d.newline();
        d.insert("text");
        assert_eq!(texts(&d), ["Para # Title", "Para text"], "a heading is one line");
    }
}
