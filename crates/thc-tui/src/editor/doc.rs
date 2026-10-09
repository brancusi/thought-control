//! The document editor (docs/design/tui-editor.md): a page or a journal day as one buffer you
//! type into. Lines are nodes; the core `outline` module turns the buffer into block ops (one
//! transaction per save, `base` on every edit so a concurrent change becomes a conflict).
//!
//! This file is the pure part: thc's lines (a block's node id and save state, with a copy of
//! its shape and text) and the save diff. The text, the selection, the editing rules and undo
//! are caretline's (`engine.rs`). The rest of thc-tui reaches it only through `editor/mod.rs`.

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
    /// The meta flashes accent until then (values settling after a save), in ms on the UI's logical clock (`UiState::now_ms`).
    pub flash_until: Option<u64>,
    /// A save in flight since (◌ after 3 s), in ms on the UI's logical clock (`UiState::now_ms`).
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
    /// The engine's document (text, selection, folds, undo) and thc's lines beside it.
    pub(super) engine: Box<super::engine::Engine>,
    /// Content changes the buffer doesn't make (remote text, a line added for typing).
    host_revision: u64,
    /// Each line as its last save left it (what the vault has); see `Doc::undo`.
    pub(super) last_saved: HashMap<String, Line>,
    /// The word count at a revision (the footer shows it every frame).
    words: std::cell::Cell<Option<(u64, usize)>>,
    /// Each line's words by its content version (`Engine::line_versions`): a keystroke counts
    /// only the line it changed (vw384).
    line_words: std::cell::RefCell<(Option<u64>, Vec<u64>, Vec<u32>)>,
    /// Each note's rows in the view, remembered (`view::RowIndex`): where the view is in the
    /// whole document, without laying the whole document out every frame.
    /// One per view: a page beside itself is laid out at two widths (a shared index re-summed
    /// the whole page at each).
    pub(super) rows: std::cell::RefCell<HashMap<u32, super::view::RowIndex>>,
    /// Each note's rows by what decides them alone (`view::note_key`), shared by every view
    /// and filled ahead in idle time (`Doc::prewarm_rows`).
    pub(super) row_cache: std::cell::RefCell<super::view::RowCache>,
    /// The clock as the runtime last gave it ([`Doc::tick`], ms on the UI's logical clock, `UiState::now_ms`).
    pub(super) now_ms: u64,
    /// A change from elsewhere to the caret's line waits until the caret leaves it (the view
    /// you type in). False for a view nobody types in now (a panel without the keyboard).
    pub hold_caret_line: bool,
    /// A save took empty lines out above the caret: the next frame keeps the caret on the
    /// screen row it was on (the view scrolls by what went), as a reflow does.
    pub repin: bool,
    /// The fresh line the document arrived with (`caret_to_end`), by its id while it's there:
    /// it goes when it's left empty (`Doc::drop_fresh_end`).
    pub(super) fresh_end: Option<String>,
}

impl Doc {
    pub fn new(target: Target, root: Option<String>, blocks: &[Block], today: chrono::NaiveDate) -> Doc {
        let mut lines: Vec<Line> = blocks.iter().map(|b| Line::from_block(b, today)).collect();
        // A blank page or day opens on an empty bullet note, as a typed line is one (the Logseq
        // model, writing.md §1); the engine would read the empty text as a paragraph.
        if lines.is_empty() {
            lines.push(Line::new(0, Kind::Bullet, ""));
        }
        // The notes that can still be a predecessor, depths increasing (see `plan_save`).
        let mut before: Vec<(usize, String)> = Vec::new();
        for l in lines.iter_mut() {
            l.saved_after = place(l.depth, &before).1;
            while before.last().is_some_and(|(d, _)| *d >= l.depth) {
                before.pop();
            }
            before.push((l.depth, l.id.clone()));
        }
        let engine = Box::new(super::engine::Engine::load(lines));
        Doc { target, root, engine, host_revision: 0, last_saved: HashMap::new(), words: Default::default(), line_words: Default::default(), rows: Default::default(), row_cache: Default::default(), now_ms: 0, hold_caret_line: true, repin: false, fresh_end: None }
    }

    /// Content generation, independent of caret motion and undo coalescing.
    pub fn revision(&self) -> u64 {
        self.engine.rev().wrapping_add(self.host_revision)
    }

    /// How many ids the document wants handed in before input ([`Doc::fill_ids`]): the model
    /// mints none.
    pub fn ids_wanted(&self) -> usize {
        super::engine::POOL.saturating_sub(self.engine.pool_len())
    }

    /// Ids minted by the runtime, for the notes the next edits make.
    pub fn fill_ids(&mut self, ids: Vec<String>) {
        self.engine.pool().fill(ids);
    }

    /// An id for a note this document makes (from the pool).
    pub(super) fn take_id(&mut self) -> String {
        self.engine.pool().take()
    }

    /// The runtime's clock (ms on the UI's logical clock, `UiState::now_ms`), before it hands the document input: idle saves,
    /// flashes and the engine's typing runs read it; the model reads no clock itself.
    pub fn tick(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
        self.engine.now_ms = now_ms;
    }

    /// Keep composed document changes for text anchors (shared across this Doc's views).
    pub fn track_changes(&mut self, on: bool) {
        if !on && self.engine.track { let _ = self.engine.take_changes(); }
        self.engine.track = on;
    }

    pub fn tracking(&self) -> bool { self.engine.track }

    pub fn take_changes(&mut self) -> Option<caretline::ChangeSet> {
        self.engine.take_changes()
    }

    pub fn cn_doc(&self) -> &caretline::Document { self.engine.cn_doc() }

    /// The engine's text (tests: the lines against it).
    #[cfg(test)]
    pub fn engine_text(&self) -> String {
        self.engine.text()
    }

    /// The document's words (its own lines), remembered for the revision.
    pub fn word_count(&self) -> usize {
        let rev = self.revision();
        if let Some((_, n)) = self.words.get().filter(|(r, _)| *r == rev) {
            return n;
        }
        let (epoch, vers) = self.engine.line_versions();
        let lines = self.lines();
        let mut c = self.line_words.borrow_mut();
        let (seen, kept, counts) = &mut *c;
        let count = |l: &Line| l.text.split_whitespace().count() as u32;
        if *seen == Some(epoch) && kept.len() == vers.len() && vers.len() == lines.len() {
            for (i, l) in lines.iter().enumerate() {
                if kept[i] != vers[i] {
                    kept[i] = vers[i];
                    counts[i] = count(l);
                }
            }
        } else {
            *counts = lines.iter().map(count).collect();
            *kept = vers.to_vec();
            *seen = (vers.len() == lines.len()).then_some(epoch);
        }
        let n = counts.iter().map(|&c| c as usize).sum();
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
        &self.lines()[self.engine.selection().1.line]
    }

    /// The selection, ordered (start, end), when there is one.
    pub fn selection(&self) -> Option<(BlockPos, BlockPos)> {
        let (a, c) = self.engine.selection();
        let a = a?;
        Some(if a < c { (a, c) } else { (c, a) })
    }

    /// A message for caretline, the vault's save state at hand (for lines an undo brings back).
    pub(super) fn run(&mut self, msg: caretline::Msg) -> Vec<caretline::Effect> {
        let Doc { engine, last_saved, .. } = self;
        engine.run(last_saved, msg)
    }

    pub fn insert(&mut self, s: &str) {
        self.run(caretline::Msg::InsertText { text: s.to_string() });
    }

    /// Enter at the caret, as the key does (`Doc::run_command("edit.newline")`).
    pub fn newline(&mut self) {
        self.run_command("edit.newline");
    }

    pub fn delete_selection(&mut self) -> bool {
        if self.selection().is_none() {
            return false;
        }
        self.run(caretline::Msg::DeleteBackward);
        true
    }

    /// A click on a task's box: its next status.
    pub fn task_box(&mut self, line: usize) -> &'static str {
        let Some(l) = self.lines().get(line) else { return "" };
        let (Some(mark), Kind::Task) = (l.mark, l.kind()) else { return "" };
        let done = l.status.as_deref() == Some("done");
        let ch = if done { ' ' } else { 'x' };
        self.run(caretline::Msg::Command { name: super::tasks::SET_STATUS.into(), args: serde_json::json!({ "id": mark, "status": ch.to_string() }) });
        if done { "reopened" } else { "done" }
    }

    /// The host's changes in `f` (recovered lines, an attachment) as one undo step: the engine
    /// takes them in together when `f` returns.
    pub fn undo_step<R>(&mut self, f: impl FnOnce(&mut Doc) -> R) -> R {
        self.engine.begin_undo_step();
        let r = f(self);
        self.engine.end_undo_step();
        r
    }

    /// A double-click: the word at `p`.
    pub fn select_word_at(&mut self, p: BlockPos) {
        let pos = self.char_of(p);
        self.run(caretline::Msg::SelectWordAt { pos });
    }

    /// A triple-click: the whole note at line `line`.
    pub fn select_block(&mut self, line: usize) {
        if let Some(m) = self.lines().get(line).and_then(|l| l.mark) {
            self.run(caretline::Msg::SelectBlock { id: caretline::MarkId(m) });
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

    /// Whether a blank row comes before line `i` (tests: the engine draws it).
    #[cfg(test)]
    pub fn effective_gap(&self, i: usize) -> bool {
        self.engine.effective_gap(i)
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
            self.engine.lines_mut().push(Line::new(0, Kind::Bullet, ""));
            self.touch_content();
            if fresh_line {
                self.engine.flush();
                self.fresh_end = self.lines().last().map(|l| l.id.clone());
            }
        }
        let i = self.lines().len() - 1;
        let byte = self.lines()[i].text.len();
        self.engine.select(None, BlockPos { line: i, byte });
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

/// Siblings in the vault's order, as a linked list per parent: what a save's creates and moves
/// do to it (`after`: right after that sibling; none: first), followed op by op.
struct VaultOrder {
    root: String,
    parent: HashMap<String, String>,
    prev: HashMap<String, Option<String>>,
    next: HashMap<String, Option<String>>,
    first: HashMap<String, Option<String>>,
}

impl VaultOrder {
    fn key(&self, p: Option<&str>) -> String {
        p.filter(|p| *p != self.root).unwrap_or("").to_string()
    }

    /// From each saved note's (id, parent, after). None when they don't make one chain per
    /// parent (two after the same note, an `after` that isn't a sibling here).
    fn of<'a>(notes: impl Iterator<Item = (&'a str, Option<&'a str>, Option<&'a str>)>, root: Option<&str>) -> Option<VaultOrder> {
        let mut o = VaultOrder { root: root.unwrap_or("").to_string(), parent: HashMap::new(), prev: HashMap::new(), next: HashMap::new(), first: HashMap::new() };
        let notes: Vec<_> = notes.collect();
        for (id, p, _) in &notes {
            let k = o.key(*p);
            o.parent.insert(id.to_string(), k);
        }
        for (id, p, after) in &notes {
            let k = o.key(*p);
            o.prev.insert(id.to_string(), after.map(str::to_string));
            match after {
                Some(a) => {
                    if o.parent.get(*a) != Some(&k) || o.next.get(*a).is_some_and(|n| n.is_some()) {
                        return None;
                    }
                    o.next.insert(a.to_string(), Some(id.to_string()));
                }
                None => {
                    if o.first.get(&k).is_some_and(|f| f.is_some()) {
                        return None;
                    }
                    o.first.insert(k, Some(id.to_string()));
                }
            }
        }
        Some(o)
    }

    /// `id` is where the vault would have it: under `parent`, right after `after`.
    fn is_at(&self, id: &str, parent: Option<&str>, after: Option<&str>) -> bool {
        self.parent.get(id) == Some(&self.key(parent)) && self.prev.get(id).map(|p| p.as_deref()) == Some(after)
    }

    fn unlink(&mut self, id: &str) {
        let Some(k) = self.parent.remove(id) else { return };
        let p = self.prev.remove(id).flatten();
        let n = self.next.remove(id).flatten();
        match &p {
            Some(p) => {
                self.next.insert(p.clone(), n.clone());
            }
            None => {
                self.first.insert(k, n.clone());
            }
        }
        if let Some(n) = n {
            self.prev.insert(n, p);
        }
    }

    fn insert(&mut self, id: &str, parent: Option<&str>, after: Option<&str>) {
        let k = self.key(parent);
        let n = match after {
            Some(a) => self.next.get(a).cloned().flatten(),
            None => self.first.get(&k).cloned().flatten(),
        };
        match after {
            Some(a) => {
                self.next.insert(a.to_string(), Some(id.to_string()));
            }
            None => {
                self.first.insert(k.clone(), Some(id.to_string()));
            }
        }
        if let Some(n) = &n {
            self.prev.insert(n.clone(), Some(id.to_string()));
        }
        self.parent.insert(id.to_string(), k);
        self.prev.insert(id.to_string(), after.map(str::to_string));
        self.next.insert(id.to_string(), n);
    }
}

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
    /// Lines this save leaves where they are whose note before them in the vault changes (a
    /// note created or moved in just above): what they follow once it lands.
    pub shifted: Vec<(String, Option<String>)>,
}

impl Doc {
    /// The ops that bring the server to the buffer: every changed line except the caret's
    /// (unless `all`), in document order; `raw` saves the caret's line as text only (the idle
    /// save).
    pub fn plan_save(&mut self, all: bool) -> SavePlan {
        let caret = self.caret().line;
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
        // An empty line away from the caret isn't a note and is never saved: it goes now,
        // with the save, so the page you see is the page you reopen (h8vsn). The caret's own
        // empty line stays (it's where you're about to type); so does a line in conflict.
        // The fresh line a document arrives with at its end stays too: it's in the state
        // (`fresh_end`), so it reopens as it shows, and ↓ from the last note goes to it.
        let fresh = self.fresh_end.clone();
        let last = self.engine.lines().len().saturating_sub(1);
        let gone = |i: usize, l: &Line| l.is_new && l.text.trim().is_empty() && i != caret && !l.conflict && !(i == last && fresh.as_deref() == Some(l.id.as_str()));
        let mut caret = caret;
        if self.engine.lines().len() > 1 && self.engine.lines().iter().enumerate().any(|(i, l)| gone(i, l)) {
            let keep: Vec<bool> = self.engine.lines().iter().enumerate().map(|(i, l)| !gone(i, l)).collect();
            let above = keep[..caret].iter().filter(|k| !**k).count();
            caret -= above;
            self.repin |= above > 0;
            let mut k = keep.iter();
            self.engine.lines_mut().retain(|_| *k.next().expect("one flag a line"));
        }
        let mut ops = Vec::new();
        let mut parsed = Vec::new();
        let mut afters = HashMap::new();
        let lines = self.engine.lines();
        // The notes placed so far that can still be a parent or a predecessor: (depth, index
        // into `lines`), depths increasing. `place` over every note placed would give the same.
        let mut present: Vec<(usize, usize)> = Vec::new();
        // Notes this save moves or creates.
        let mut placed: HashSet<&str> = HashSet::new();
        // The vault's sibling order as this save's ops change it, to move only notes whose
        // place in it differs (ymh1g: a paste above 5,000 notes moved every one of them).
        // None: the saved places don't chain up (a delete pending, a line from elsewhere):
        // the cautious rule below then moves every note after a moved one.
        let mut order = if self.engine.deleted().is_empty() { VaultOrder::of(lines.iter().filter(|l| !l.is_new).map(|l| (l.id.as_str(), l.saved_parent.as_deref(), l.saved_after.as_deref())), self.root.as_deref()) } else { None };
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
                placed.insert(l.id.as_str());
                if let Some(o) = order.as_mut() {
                    o.insert(&l.id, parent.as_deref(), after.as_deref());
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
            // A note after one this save moves goes with it: the vault places it by its own
            // order, not by what comes before it (fuzz: ⌥↓ moved two notes, the second stayed).
            let follows_moved = after.is_some_and(|a| placed.contains(a));
            let moves = match order.as_mut() {
                Some(o) => !o.is_at(&l.id, parent, after),
                None => reparented || after != l.saved_after.as_deref() || follows_moved,
            };
            if moves {
                if let Some(o) = order.as_mut() {
                    o.unlink(&l.id);
                    o.insert(&l.id, parent, after);
                }
                placed.insert(l.id.as_str());
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
        let shifted: Vec<(String, Option<String>)> = match &order {
            Some(o) => lines.iter().filter(|l| !l.is_new && !afters.contains_key(&l.id)).filter_map(|l| o.prev.get(&l.id).filter(|p| p.as_deref() != l.saved_after.as_deref()).map(|p| (l.id.clone(), p.clone()))).collect(),
            None => Vec::new(),
        };
        for id in std::mem::take(self.engine.deleted_mut()).into_iter() {
            ops.push(BlockOp::Delete { id, rev: None });
        }
        // What was sent, to tell later edits apart: nothing to send, nothing to remember.
        let sent = Sent {
            lines: self.engine.lines().iter().filter(|_| !ops.is_empty()).map(|l| (l.id.clone(), (l.text.clone(), l.status.clone()))).collect(),
            created: ops.iter().filter_map(|o| if let BlockOp::Create { id, .. } = o { Some(id.clone()) } else { None }).collect(),
            shifted,
        };
        self.engine.settle();
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
        // Where each line is (the results change fields, never the lines' order): a lookup, not
        // a search per result (a 1,000-note save searched 5,000 lines a result).
        let at: HashMap<String, usize> = self.engine.lines().iter().enumerate().map(|(i, l)| (l.id.clone(), i)).collect();
        for r in results {
            let latest = last.get(r.id.as_str()) == Some(&r.index);
            let Some(&i) = at.get(&r.id) else {
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
        // Every op landed: the lines it left in place follow what the vault has above them now.
        if results.iter().all(|r| r.state == "ok") {
            for (id, after) in &sent.shifted {
                if let Some(l) = self.engine.lines_mut().iter_mut().find(|l| l.id == *id && !l.is_new) {
                    l.saved_after = after.clone();
                }
            }
        }
        self.engine.settle();
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
        let fx = self.run(caretline::Msg::Copy);
        let text = fx.into_iter().find_map(|f| if let caretline::Effect::ClipboardSet { text } = f { Some(text) } else { None }).unwrap_or_default();
        self.engine.with_fields(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // The Logseq model (writing.md §1): a typed line is a bullet note; Enter splits it,
        // ⇧Enter breaks the line, ⌫ at a note's start joins the note above.
        let mut d = doc(&[(0, Kind::Bullet, "")]);
        d.insert("Hello world");
        at(&mut d, 0, 6);
        d.run_command("edit.soft_break");
        assert_eq!(texts(&d), ["Bullet Hello \nworld"]);
        d.run_command("edit.backspace");
        d.newline();
        assert_eq!(texts(&d), ["Bullet Hello ", "Bullet world"]);
        d.run_command("edit.backspace");
        assert_eq!(texts(&d), ["Bullet Hello world"]);
        assert_eq!(d.caret(), BlockPos { line: 0, byte: 6 });
        d.run_command("history.undo");
        assert_eq!(texts(&d), ["Bullet Hello ", "Bullet world"]);
        d.run_command("history.redo");
        assert_eq!(texts(&d), ["Bullet Hello world"]);
        // A paragraph (imported Markdown) splits too.
        let mut d = doc(&[(0, Kind::Para, "Hello world")]);
        at(&mut d, 0, 5);
        d.newline();
        assert_eq!(texts(&d), ["Para Hello", "Para world"]);
    }

    #[test]
    fn list_forms_and_nesting() {
        let mut d = doc(&[(0, Kind::Bullet, "")]);
        d.insert("- ");
        assert_eq!(texts(&d), ["Bullet "], "`- ` on a bullet is the bullet it is");
        d.insert("Plan");
        d.newline();
        d.insert("Book venue");
        d.run_command("structure.indent");
        d.newline();
        d.newline(); // an empty nested note comes out a level
        assert_eq!(texts(&d), ["Bullet Plan", " Bullet Book venue", "Bullet "]);
        d.newline(); // at the top, Enter on an empty note does nothing
        assert_eq!(texts(&d), ["Bullet Plan", " Bullet Book venue", "Bullet "]);
        at(&mut d, 0, 4);
        d.run_command("thc.task_cycle");
        assert_eq!(d.lines()[0].kind(), Kind::Task);
        assert_eq!(d.run_command("thc.task_cycle"), crate::editor::Outcome::Completed);
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
        d.run_command("structure.move_up");
        assert_eq!(texts(&d), ["Bullet B", "Bullet A", " Bullet A1"]);
        assert_eq!(d.caret().line, 0);
        assert!(matches!(d.run_command("structure.move_up"), crate::editor::Outcome::Nothing(_)));
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
            d.run_command("edit.backspace");
            assert_eq!(d.lines()[0].text, "a", "⌫ removes all of {g:?}");
            let mut d = doc(&[(0, Kind::Para, &format!("a{g}"))]);
            at(&mut d, 0, 1);
            d.run_command("edit.delete_forward");
            assert_eq!(d.lines()[0].text, "a", "⌦ removes all of {g:?}");
            let mut d = doc(&[(0, Kind::Para, &format!("a{g}b"))]);
            at(&mut d, 0, 1);
            d.run_command("move.right");
            assert_eq!(d.caret().byte, 1 + g.len(), "→ over {g:?}");
        }
    }

    /// An empty line left behind goes with the save (h8vsn), and only it: the other notes keep
    /// their ids, and the save sends nothing for them (no moves, no edits).
    #[test]
    fn an_empty_line_left_goes_and_nothing_else_moves() {
        let mut d = doc(&[(0, Kind::Para, "alpha"), (0, Kind::Para, ""), (0, Kind::Para, "beta"), (0, Kind::Para, "gamma")]);
        let mut prev: Option<String> = None;
        for l in d.lines_mut().iter_mut().filter(|l| !l.text.is_empty()) {
            l.is_new = false;
            l.saved = Some(l.text.clone());
            l.saved_kind = Some(Kind::Para);
            l.saved_after = prev.replace(l.id.clone());
        }
        let ids: Vec<String> = d.lines().iter().filter(|l| !l.text.is_empty()).map(|l| l.id.clone()).collect();
        d.set_caret(BlockPos { line: 3, byte: 2 });
        let plan = d.plan_save(false);
        assert_eq!(d.lines().iter().map(|l| l.id.clone()).collect::<Vec<_>>(), ids, "the empty line went, every note kept its id");
        assert!(plan.ops.is_empty(), "nothing to send: {:?}", plan.ops);
        assert_eq!(d.caret(), BlockPos { line: 2, byte: 2 }, "the caret stays on its text");
        assert!(d.repin, "the next frame keeps the caret's row");
    }

    #[test]
    fn save_plan_keeps_completion_alongside_an_unsaved_task_kind() {
        let mut d = doc(&[(0, Kind::Para, "alpha line")]);
        let l = &mut d.lines_mut()[0];
        l.is_new = false;
        l.saved = Some(l.text.clone());
        l.saved_kind = Some(Kind::Para);
        d.run_command("thc.task_cycle");
        let first = d.plan_save(true);
        assert!(first.ops.iter().any(|op| matches!(op, BlockOp::Kind { kind: Kind::Task, .. })));
        assert!(!first.ops.iter().any(|op| matches!(op, BlockOp::Status { .. })), "Kind(Task) already initializes todo");
        d.run_command("thc.task_cycle");
        let second = d.plan_save(true);
        let kind = second.ops.iter().position(|op| matches!(op, BlockOp::Kind { kind: Kind::Task, .. })).unwrap();
        let status = second.ops.iter().position(|op| matches!(op, BlockOp::Status { status, .. } if status == "done")).unwrap();
        assert!(kind < status, "the completed status follows the kind that initializes todo");
    }

    #[test]
    fn task_cycle_and_marker_steps() {
        let mut d = doc(&[(0, Kind::Bullet, "call")]);
        d.run_command("thc.task_cycle");
        assert_eq!((d.lines()[0].kind(), d.lines()[0].status.as_deref()), (Kind::Task, Some("todo")));
        d.run_command("thc.task_cycle");
        assert_eq!(d.lines()[0].status.as_deref(), Some("done"));
        d.run_command("thc.task_cycle");
        assert_eq!((d.lines()[0].kind(), d.lines()[0].status.as_deref()), (Kind::Bullet, None), "back to text: a bullet note");
        d.run_command("thc.task_cycle");
        at(&mut d, 0, 0);
        d.run_command("edit.backspace");
        assert_eq!((d.lines()[0].kind(), d.lines()[0].status.as_deref()), (Kind::Bullet, None));
        d.run_command("edit.backspace");
        assert_eq!((d.lines()[0].kind(), d.lines()[0].text.as_str()), (Kind::Bullet, "call"), "the first note stays a note");
        let mut d = doc(&[(0, Kind::Para, "a"), (0, Kind::Bullet, "b"), (0, Kind::Bullet, "c")]);
        d.select_range(Some(BlockPos { line: 0, byte: 0 }), BlockPos { line: 2, byte: 1 });
        d.run_command("thc.task_cycle");
        assert!(d.lines().iter().all(|l| l.kind() == Kind::Task), "{:?}", texts(&d));
    }

    #[test]
    fn composed_text_changes_cover_edits_through_both_views() {
        let mut d = doc(&[(0, Kind::Bullet, "hello")]);
        d.track_changes(true);
        let mut before = d.cn_doc().text.clone();
        d.insert("🙂");
        d.add_view(1);
        assert!(d.use_view(1));
        d.insert("λ");
        let changes = d.take_changes().expect("both view edits are observed");
        assert!(changes.apply(&mut before));
        assert_eq!(before.to_string(), d.cn_doc().text.to_string());
        assert!(d.take_changes().is_none());
        d.track_changes(false);
        d.insert("off");
        assert!(!d.tracking());
        assert!(d.take_changes().is_none());
    }

    #[test]
    fn notes_are_bullets_and_paragraphs_keep_working() {
        let mut d = doc(&[(0, Kind::Bullet, "")]);
        d.insert("one");
        d.newline();
        d.insert("two");
        assert_eq!(texts(&d), ["Bullet one", "Bullet two"], "Enter: two notes");
        let mut d = doc(&[(0, Kind::Bullet, "")]);
        d.insert("one");
        d.run_command("edit.soft_break");
        d.insert("two");
        assert_eq!(texts(&d), ["Bullet one\ntwo"], "⇧Enter: one note, two lines");
        let mut d = doc(&[(0, Kind::Para, "abcdef")]);
        let id = d.lines()[0].id.clone();
        at(&mut d, 0, 3);
        d.newline();
        assert_eq!(texts(&d), ["Para abc", "Para def"], "a paragraph splits");
        assert_eq!(d.lines()[0].id, id, "the first part keeps the id");
        at(&mut d, 1, 0);
        d.run_command("edit.backspace");
        assert_eq!(texts(&d), ["Para abc\ndef"], "paragraphs join keeping the break");
        assert_eq!(d.lines()[0].id, id);
        let mut d = doc(&[(0, Kind::Para, "intro")]);
        at(&mut d, 0, 5);
        d.newline();
        d.insert("milk");
        assert_eq!(texts(&d), ["Para intro", "Bullet milk"], "after a paragraph, a typed line is a bullet");
        let mut d = doc(&[(0, Kind::Bullet, "")]);
        for c in ["[", " ", "]", " ", "call"] {
            d.insert(c);
        }
        d.newline();
        assert_eq!((d.lines()[1].kind(), d.lines()[1].status.as_deref()), (Kind::Task, Some("todo")), "A8");
        let mut d = doc(&[(0, Kind::Bullet, "")]);
        for c in ["#", " ", "Title"] {
            d.insert(c);
        }
        d.newline();
        d.insert("text");
        assert_eq!(texts(&d), ["Para # Title", "Bullet text"], "a heading, then a bullet");
    }
}
