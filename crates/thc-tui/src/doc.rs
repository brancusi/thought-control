//! The document editor (docs/design/tui-editor.md): a page or a journal day as one buffer you
//! type into. Lines are nodes; the core `outline` module turns the buffer into block ops (one
//! transaction per save, `base` on every edit so a concurrent change becomes a conflict).
//!
//! This file is the pure part: thc's lines (a caretline block plus its save state), the save
//! diff and wrapping. The editing rules and undo are caretline's (`caretline::buffer`); drawing
//! and keys live in ui.rs / input.rs and call in here.

use caretline::buffer::{BlockLine, Buffer};
use std::collections::HashMap;
use std::time::Instant;
use thc_core::outline::{Block, BlockOp, Kind};

/// Crockford base32 as the core writes ids (FORMAT.md).
pub fn new_id() -> String {
    thc_core::id::new_id()
}

/// One line of the document: a node, its shape and its save state.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// The block itself (caretline's): id, depth, kind, status, text (soft breaks are `\n`) and
    /// fold. Its fields read and write through `Line` (Deref).
    pub block: caretline::Block<String>,
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
    /// The meta flashes accent until then (values settling after a save).
    pub flash_until: Option<Instant>,
    /// A save in flight since (◌ after 3 s), or the last save's error.
    pub saving_since: Option<Instant>,
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
}

impl std::ops::Deref for Line {
    type Target = caretline::Block<String>;
    fn deref(&self) -> &Self::Target {
        &self.block
    }
}

impl std::ops::DerefMut for Line {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.block
    }
}

impl Line {
    pub fn new(depth: usize, kind: Kind, text: &str) -> Line {
        Line {
            block: caretline::Block::new(new_id(), depth, kind, text),
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

    pub fn edited(&self) -> bool {
        self.is_new || self.saved.as_deref() != Some(self.text.as_str())
    }
}

impl BlockLine for Line {
    type Id = String;

    fn fresh(depth: usize, kind: Kind, text: &str) -> Line {
        Line::new(depth, kind, text)
    }

    fn is_new(&self) -> bool {
        self.is_new
    }

    fn keep_host_state(&mut self, cur: &Line) {
        self.base = cur.base.clone();
        self.saved = cur.saved.clone();
        self.saved_parent = cur.saved_parent.clone();
        self.saved_after = cur.saved_after.clone();
        self.saved_kind = cur.saved_kind;
        self.saved_status = cur.saved_status.clone();
        self.saved_gap = cur.saved_gap;
        self.is_new = cur.is_new;
        self.meta = cur.meta.clone();
        self.fields = cur.fields.clone();
        self.remote_text = cur.remote_text.clone();
        self.conflict = cur.conflict;
        self.conflict_with = cur.conflict_with.clone();
        self.saving_since = cur.saving_since;
    }

    fn revive(&mut self) {
        self.is_new = true;
        self.id = new_id();
        self.saving_since = None;
    }

    fn text_only(&mut self) {
        self.meta.clear();
        self.fields.clear();
    }

    fn fields(&self) -> &str {
        &self.fields
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

/// A caret position: a line and a byte offset in its text (caretline's).
pub use caretline::Pos;
// Text measured as drawn, and wrapping: caretline's.
pub use caretline::{next_char, prev_char, width, wrap, wrap_with};

/// What the document is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Page { id: String, title: String },
    Journal { date: chrono::NaiveDate },
}

pub struct Doc {
    pub target: Target,
    /// The document's root node (None: a journal day not created yet; its first save makes it).
    pub root: Option<String>,
    /// The lines, caret, selection, pending deletes and undo: caretline's buffer, whose rules
    /// edit it (`d.lines`, `d.caret`, `d.newline()` read through Deref).
    pub buf: Buffer<Line>,
    /// The first visual row on screen.
    pub scroll: usize,
    /// Content changes the buffer doesn't make (remote text, a line added for typing).
    host_revision: u64,
    /// Each line as its last save left it (what the vault has); see `Doc::undo`.
    last_saved: HashMap<String, Line>,
    wraps: HashMap<(u64, usize), Vec<(usize, usize)>>,
}

impl std::ops::Deref for Doc {
    type Target = Buffer<Line>;
    fn deref(&self) -> &Buffer<Line> {
        &self.buf
    }
}

impl std::ops::DerefMut for Doc {
    fn deref_mut(&mut self) -> &mut Buffer<Line> {
        &mut self.buf
    }
}

impl Doc {
    pub fn new(target: Target, root: Option<String>, blocks: &[Block], today: chrono::NaiveDate) -> Doc {
        let mut lines: Vec<Line> = blocks.iter().map(|b| Line::from_block(b, today)).collect();
        let mut before: Vec<(usize, String)> = Vec::new();
        for l in lines.iter_mut() {
            l.saved_after = place(l.depth, &before).1;
            before.push((l.depth, l.id.clone()));
        }
        Doc { target, root, buf: Buffer::new(lines), scroll: 0, host_revision: 0, last_saved: HashMap::new(), wraps: HashMap::new() }
    }

    /// Content generation, independent of caret motion and undo coalescing.
    pub fn revision(&self) -> u64 {
        self.buf.content_rev().wrapping_add(self.host_revision)
    }

    /// Invalidate content-derived inputs after a local edit or deferred remote text.
    pub fn touch_content(&mut self) {
        self.host_revision = self.host_revision.wrapping_add(1);
    }

    pub fn line(&self) -> &Line {
        &self.buf.lines[self.buf.caret.line]
    }

    /// Put the caret at the end of the document's last line, or on a fresh line when the last
    /// line has text (a journal day: ready to type).
    pub fn caret_to_end(&mut self, fresh_line: bool) {
        if self.buf.lines.is_empty() || (fresh_line && !self.buf.lines.last().unwrap().text.is_empty()) {
            let depth = 0;
            self.buf.lines.push(Line::new(depth, Kind::Para, ""));
            self.touch_content();
        }
        let i = self.buf.lines.len() - 1;
        self.buf.caret = Pos { line: i, byte: self.buf.lines[i].text.len() };
    }

    /// Undo one step. A line coming back while its delete is still pending is still in the
    /// vault: it takes its save state from the last save, not from the snapshot, which may be
    /// older than that save (fuzz, late saves: it came back "new" and was made twice).
    pub fn undo(&mut self) -> bool {
        self.step(Buffer::undo)
    }

    pub fn redo(&mut self) -> bool {
        self.step(Buffer::redo)
    }

    fn step(&mut self, f: fn(&mut Buffer<Line>) -> bool) -> bool {
        let pending: std::collections::HashSet<String> = self.buf.deleted.iter().cloned().collect();
        let here: std::collections::HashSet<String> = self.buf.lines.iter().map(|l| l.id.clone()).collect();
        if !f(&mut self.buf) {
            return false;
        }
        for l in self.buf.lines.iter_mut().filter(|l| pending.contains(&l.id) && !here.contains(&l.id)) {
            if let Some(s) = self.last_saved.get(&l.id) {
                l.base = s.base.clone();
                l.saved = s.saved.clone();
                l.saved_parent = s.saved_parent.clone();
                l.saved_after = s.saved_after.clone();
                l.saved_kind = s.saved_kind;
                l.saved_status = s.saved_status.clone();
                l.saved_gap = s.saved_gap;
            }
            l.is_new = false;
            l.saving_since = None;
        }
        true
    }
}

/// Where a line lives given the lines before it: its parent (None = the root) and the sibling
/// it follows (None = first).
pub fn place(depth: usize, before: &[(usize, String)]) -> (Option<String>, Option<String>) {
    let mut parent = None;
    let mut after = None;
    for (d, id) in before.iter().rev() {
        if *d == depth && after.is_none() && parent.is_none() {
            after = Some(id.clone());
        }
        if *d < depth {
            parent = Some(id.clone());
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
        let mut above: Option<usize> = None;
        for l in self.buf.lines.iter_mut() {
            if l.text.trim().is_empty() {
                continue;
            }
            let max = above.map_or(0, |d| d + 1);
            if l.depth > max {
                l.depth = max;
            }
            above = Some(l.depth);
        }
        let caret = self.buf.caret.line;
        // A saved note emptied to nothing (and left) goes: an empty line isn't a note, and an
        // empty node kept its old place, which other notes were then placed after (fuzz). The
        // line stays as a plain empty line; its children are placed by this save before the
        // delete runs (deletes go last).
        for (i, l) in self.buf.lines.iter_mut().enumerate() {
            if !l.is_new && l.text.trim().is_empty() && (all || i != caret) && !l.conflict {
                self.buf.deleted.push(std::mem::replace(&mut l.id, new_id()));
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
        let mut ops = Vec::new();
        let mut parsed = Vec::new();
        let mut afters = HashMap::new();
        let mut present: Vec<(usize, String)> = Vec::new();
        for (i, l) in self.buf.lines.iter().enumerate() {
            let skip = !all && i == caret;
            let (parent, after) = place(l.depth, &present);
            if l.is_new {
                // Blank (spaces only) isn't a note: capture refuses it, and a child placed under it
                // would be moved under a parent that never got made (fuzz).
                if l.text.trim().is_empty() || skip {
                    continue;
                }
                ops.push(BlockOp::Create { id: l.id.clone(), parent: parent.clone(), after: after.clone(), kind: l.kind, text: l.text.clone() });
                afters.insert(l.id.clone(), after);
                parsed.push(l.id.clone());
                present.push((l.depth, l.id.clone()));
                if let Some(st) = l.status.as_deref().filter(|s| *s != "todo" && l.kind == Kind::Task) {
                    ops.push(BlockOp::Status { id: l.id.clone(), status: st.into(), rev: None });
                }
                if l.gap.is_some() {
                    ops.push(BlockOp::Gap { id: l.id.clone(), gap: l.gap, rev: None });
                }
                continue;
            }
            // An empty line is never a parent, saved or not (the same rule as Tab's).
            if !l.text.trim().is_empty() {
                present.push((l.depth, l.id.clone()));
            }
            // The caret's line keeps its text (still being typed), never its place: a parent
            // deleted in this save takes its children with it, so the caret's line must move
            // out first (fuzz: it was deleted with its old parent).
            let reparented = parent != l.saved_parent && !(parent.is_none() && l.saved_parent == self.root);
            if reparented || after != l.saved_after {
                ops.push(BlockOp::Move { id: l.id.clone(), parent: parent.clone(), after: after.clone(), rev: None });
                afters.insert(l.id.clone(), after.clone());
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
            if Some(l.kind) != l.saved_kind {
                ops.push(BlockOp::Kind { id: l.id.clone(), kind: l.kind, rev: None });
            }
            if l.kind == Kind::Task && l.status.is_some() && l.status != l.saved_status && Some(l.kind) == l.saved_kind {
                ops.push(BlockOp::Status { id: l.id.clone(), status: l.status.clone().unwrap(), rev: None });
            }
        }
        for id in self.buf.deleted.drain(..) {
            ops.push(BlockOp::Delete { id, rev: None });
        }
        let sent = Sent {
            lines: self.buf.lines.iter().map(|l| (l.id.clone(), (l.text.clone(), l.status.clone()))).collect(),
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
        let mut messages = Vec::new();
        let caret_id = self.line().id.clone();
        // Lines edited since the plan (before any result here: one line can have several, a
        // create then its status): their text and status stay as they are now.
        let (moved_text, moved_status): (std::collections::HashSet<String>, std::collections::HashSet<String>) = {
            let mut t = std::collections::HashSet::new();
            let mut st = std::collections::HashSet::new();
            for l in &self.buf.lines {
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
        for r in results {
            let Some(i) = self.buf.lines.iter().position(|l| l.id == r.id) else {
                // Made by this save, gone from the buffer since: the vault has it now, so it goes.
                if r.state == "ok" && sent.created.contains(&r.id) && !self.buf.deleted.contains(&r.id) {
                    self.buf.deleted.push(r.id.clone());
                }
                continue;
            };
            let l = &mut self.buf.lines[i];
            l.saving_since = None;
            match r.state {
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
                        if self.buf.caret.line == i {
                            self.buf.caret.byte = self.buf.caret.byte.min(l.text.len());
                        }
                    }
                    if parsed.contains(&l.id) && l.meta != had_meta && !l.meta.is_empty() {
                        l.flash_until = Some(Instant::now() + std::time::Duration::from_millis(300));
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

/// A pasted line, and Markdown read into lines: caretline's.
pub use caretline::markdown::parse_paste;

impl Doc {
    /// What ⌘C copies (editing.md §5): inside one note its plain text; across notes Markdown,
    /// the first note with its marker only when the selection includes the note's start, and
    /// every later note with its marker and indent. Empty: nothing selected.
    pub fn copy_text(&self) -> String {
        let parts = self.selected_parts();
        match parts.len() {
            0 => String::new(),
            1 => parts[0].1.clone(),
            _ => {
                let md = markdown(&parts.iter().map(|(i, t)| (&self.buf.lines[*i], t.as_str())).collect::<Vec<_>>());
                // One trailing newline goes; a boundary at the end (an empty last note) stays.
                let md = md.strip_suffix('\n').unwrap_or(&md);
                let from_start = self.selection().is_some_and(|(s, _)| s.byte == 0);
                if from_start || self.buf.lines[parts[0].0].kind == Kind::Para {
                    return md.to_string();
                }
                // The first note from the middle: its text without the marker.
                let (first, rest) = md.split_once('\n').unwrap_or((md, ""));
                let t = first.trim_start();
                let t = t.strip_prefix("- ").unwrap_or(t);
                let t = ["[ ] ", "[x] ", "[/] ", "[w] ", "[-] "].iter().find_map(|c| t.strip_prefix(c)).unwrap_or(t);
                format!("{t}\n{rest}")
            }
        }
    }
}

/// Lines as Markdown (copy): the same form `thc edit` writes, fields re-attached with absolute
/// dates. Each entry is a line and the part of its text that's selected.
pub fn markdown(parts: &[(&Line, &str)]) -> String {
    let blocks: Vec<(&caretline::Block<String>, &str, &str)> = parts.iter().map(|(l, t)| (&l.block, *t, l.fields.as_str())).collect();
    caretline::markdown::to_markdown(&blocks)
}

// ---- wrapping and the view ------------------------------------------------------------------------

impl Doc {
    /// The wrap of line `i` at `w` columns, cached by (text, width).
    pub fn rows_of(&mut self, i: usize, w: usize) -> Vec<(usize, usize)> {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.buf.lines[i].text.hash(&mut h);
        let key = (h.finish(), w);
        if let Some(r) = self.wraps.get(&key) {
            return r.clone();
        }
        // A code block never wraps: one row per line (it scrolls sideways instead, §3.2).
        let r = if self.buf.lines[i].kind == Kind::Para && self.buf.lines[i].text.starts_with("```") {
            wrap(&self.buf.lines[i].text, usize::MAX / 2)
        } else {
            wrap_with(&self.buf.lines[i].text, w, width(&self.buf.lines[i].text[..crate::doc_ui::marker_len(&self.buf.lines[i])]))
        };
        if self.wraps.len() > 20_000 {
            self.wraps.clear();
        }
        self.wraps.insert(key, r.clone());
        r
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(lines: &[(usize, Kind, &str)]) -> Doc {
        let mut d = Doc::new(Target::Journal { date: chrono::NaiveDate::from_ymd_opt(2026, 10, 4).unwrap() }, None, &[], chrono::NaiveDate::from_ymd_opt(2026, 10, 4).unwrap());
        d.lines = lines.iter().map(|(dep, k, t)| Line::new(*dep, *k, t)).collect();
        d
    }

    fn texts(d: &Doc) -> Vec<String> {
        d.lines.iter().map(|l| format!("{}{:?} {}", " ".repeat(l.depth), l.kind, l.text)).collect()
    }

    #[test]
    fn typing_splitting_and_merging() {
        let mut d = doc(&[(0, Kind::Para, "")]);
        d.insert("Hello world");
        d.caret.byte = 5;
        d.newline(); // a line break in the paragraph
        assert_eq!(texts(&d), ["Para Hello\n world"]);
        d.newline(); // a blank line: two notes
        assert_eq!(texts(&d), ["Para Hello", "Para  world"]);
        d.backspace(); // at the start of a paragraph: join, the break kept
        assert_eq!(texts(&d), ["Para Hello\n world"]);
        assert_eq!(d.caret, Pos { line: 0, byte: 6 });
        assert!(d.undo());
        assert_eq!(texts(&d), ["Para Hello", "Para  world"]);
        assert!(d.redo());
        assert_eq!(texts(&d), ["Para Hello\n world"]);
    }

    #[test]
    fn list_forms_and_nesting() {
        let mut d = doc(&[(0, Kind::Para, "")]);
        d.insert("- ");
        assert_eq!(d.lines[0].kind, Kind::Bullet);
        d.insert("Plan");
        d.newline();
        d.insert("Book venue");
        assert!(d.nest(1));
        d.newline();
        d.newline(); // an empty item ends the list: an empty paragraph line
        assert_eq!(texts(&d), ["Bullet Plan", " Bullet Book venue", "Para "]);
        d.caret = Pos { line: 0, byte: 4 };
        assert_eq!(d.task_cycle(), "task");
        assert_eq!(d.task_cycle(), "done");
        assert_eq!(d.lines[0].status.as_deref(), Some("done"));
    }

    #[test]
    fn numbered_items_continue() {
        let mut d = doc(&[(0, Kind::Bullet, "1. First")]);
        d.caret.byte = d.lines[0].text.len();
        d.newline();
        assert_eq!(d.lines[1].text, "2. ");
    }

    #[test]
    fn move_with_children_and_selection_delete() {
        let mut d = doc(&[(0, Kind::Bullet, "A"), (1, Kind::Bullet, "A1"), (0, Kind::Bullet, "B")]);
        d.caret = Pos { line: 2, byte: 0 };
        d.move_line(-1).unwrap();
        assert_eq!(texts(&d), ["Bullet B", "Bullet A", " Bullet A1"]);
        assert_eq!(d.caret.line, 0);
        assert!(d.move_line(-1).is_err());
        d.caret = Pos { line: 0, byte: 1 };
        d.anchor = Some(Pos { line: 2, byte: 1 });
        d.delete_selection();
        assert_eq!(texts(&d), ["Bullet B1"]);
    }

    #[test]
    fn markdown_paste_reads_outlines() {
        let (lines, images) = parse_paste("Intro\nwrapped.\n\n- Plan\n  - [ ] Book venue due:2026-10-09\n  - notes ![x](a.png)\n1. First\n\n```\ncode due:fri\n```\n", false);
        let shape: Vec<String> = lines.iter().map(|(d, k, s, t)| format!("{d} {k:?} {} {t}", s.clone().unwrap_or_default())).collect();
        assert_eq!(shape, ["0 Para  Intro wrapped.", "0 Bullet  Plan", "1 Task todo Book venue due:2026-10-09", "1 Bullet  notes", "0 Bullet  1. First", "0 Para  ```\ncode due:fri\n```"]);
        assert_eq!(images, 1);
        let (plain, _) = parse_paste("a\nb\n\nc", true);
        assert_eq!(plain.iter().map(|l| l.3.clone()).collect::<Vec<_>>(), ["a\nb", "c"]);
    }

    #[test]
    fn copy_writes_markdown() {
        let mut a = Line::new(0, Kind::Bullet, "Plan");
        a.fields = String::new();
        let mut b = Line::new(1, Kind::Task, "Book venue");
        b.fields = " due:2026-10-09".into();
        let p = Line::new(0, Kind::Para, "Note");
        assert_eq!(markdown(&[(&p, "Note"), (&a, "Plan"), (&b, "Book venue")]), "Note\n\n- Plan\n  - [ ] Book venue due:2026-10-09\n");
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
    fn wrapping() {
        assert_eq!(wrap("one two three", 8), vec![(0, 8), (8, 13)]);
        assert_eq!(wrap("a\nb", 10), vec![(0, 1), (2, 3)]);
        assert_eq!(wrap("", 10), vec![(0, 0)]);
        assert_eq!(wrap("abc\n", 10), vec![(0, 3), (4, 4)], "a trailing soft break starts a row");
        let long = "x".repeat(20);
        assert_eq!(wrap(&long, 8).len(), 3);
    }

    #[test]
    fn save_plan_places_new_lines() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
        let mut d = Doc::new(Target::Journal { date: today }, Some("root".into()), &[], today);
        d.lines = vec![Line::new(0, Kind::Bullet, "A"), Line::new(1, Kind::Task, "A1"), Line::new(0, Kind::Para, "")];
        d.caret = Pos { line: 2, byte: 0 };
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
        assert_eq!(creates[1].1, Some(d.lines[0].id.clone()), "A1 under A");
    }

    /// Characters as people see them (jank A1–A6): width matches ratatui's per-cluster
    /// measure, and the caret, Backspace and Delete never split a cluster.
    #[test]
    fn graphemes_are_one_character() {
        let family = "👨\u{200d}👩\u{200d}👧";
        let heart = "❤\u{fe0f}";
        let flag = "🇯🇵";
        let accent = "e\u{301}";
        for (g, w) in [(family, 2), (heart, 2), (flag, 2), (accent, 1), ("漢", 2), ("a", 1)] {
            assert_eq!(width(g), w, "{g:?}");
            assert_eq!(width(g), ratatui::text::Span::raw(g).width(), "{g:?} as ratatui measures it");
            let s = format!("a{g}b");
            assert_eq!(next_char(&s, 1), 1 + g.len(), "→ over {g:?}");
            assert_eq!(prev_char(&s, 1 + g.len()), 1, "← over {g:?}");
            let mut d = doc(&[(0, Kind::Para, &format!("a{g}"))]);
            d.caret = Pos { line: 0, byte: d.lines[0].text.len() };
            d.backspace();
            assert_eq!(d.lines[0].text, "a", "⌫ removes all of {g:?}");
            let mut d = doc(&[(0, Kind::Para, &format!("a{g}"))]);
            d.caret = Pos { line: 0, byte: 1 };
            d.delete_forward();
            assert_eq!(d.lines[0].text, "a", "⌦ removes all of {g:?}");
        }
        // Wrapping counts clusters and never cuts one: 5 families are 10 columns.
        let five = family.repeat(5);
        let rows = wrap(&five, 8);
        assert_eq!(rows.len(), 2);
        assert!(five.is_char_boundary(rows[0].1) && unicode_segmentation::UnicodeSegmentation::graphemes(&five[..rows[0].1], true).count() == 4, "{rows:?}");
        // A heading's marker sits in the hang: the first row gets its columns back.
        assert_eq!(wrap_with("## abcdefgh ij", 8, 3), vec![(0, 12), (12, 14)]);
        assert_eq!(wrap("abcdefgh ij", 8), vec![(0, 9), (9, 11)], "a word filling the row stays on it");
    }

    /// ⌃T cycles text → [ ] → [x] → text (writing.md A9); ⌫ takes the marker off a step at a
    /// time (A10); a selection cycles together (A11).
    #[test]
    fn task_cycle_and_marker_steps() {
        let mut d = doc(&[(0, Kind::Para, "call")]);
        assert_eq!(d.task_cycle(), "task");
        assert_eq!(d.task_cycle(), "done");
        assert_eq!(d.task_cycle(), "text");
        assert_eq!((d.lines[0].kind, d.lines[0].status.as_deref()), (Kind::Para, None));
        d.task_cycle();
        d.caret = Pos { line: 0, byte: 0 };
        d.backspace();
        assert_eq!((d.lines[0].kind, d.lines[0].status.as_deref()), (Kind::Bullet, None));
        d.backspace();
        assert_eq!((d.lines[0].kind, d.lines[0].text.as_str()), (Kind::Para, "call"));
        let mut d = doc(&[(0, Kind::Para, "a"), (0, Kind::Para, "b"), (0, Kind::Para, "c")]);
        d.anchor = Some(Pos { line: 0, byte: 0 });
        d.caret = Pos { line: 2, byte: 1 };
        d.task_cycle();
        assert!(d.lines.iter().all(|l| l.kind == Kind::Task), "{:?}", texts(&d));
    }

    /// Enter is a line break in a paragraph; a blank line splits it (writing.md A2–A4); ⌫ at a
    /// paragraph's start joins (A5); a marker typed on a later line starts a note.
    #[test]
    fn paragraphs_are_plain_text() {
        let mut d = doc(&[(0, Kind::Para, "")]);
        d.caret = Pos { line: 0, byte: 0 };
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
        let id = d.lines[0].id.clone();
        d.caret = Pos { line: 0, byte: 3 };
        d.newline();
        d.newline();
        assert_eq!(texts(&d), ["Para abc", "Para def"], "A4: a split");
        assert_eq!(d.lines[0].id, id, "the first part keeps the id");
        d.caret = Pos { line: 1, byte: 0 };
        d.backspace();
        assert_eq!(texts(&d), ["Para abc\ndef"], "A5: a join keeps the break");
        assert_eq!(d.lines[0].id, id);
        let mut d = doc(&[(0, Kind::Para, "")]);
        d.insert("intro");
        d.newline();
        for c in ["-", " ", "milk"] {
            d.insert(c);
        }
        assert_eq!(texts(&d), ["Para intro", "Bullet milk"], "a marker on a later line starts an item");
        d.newline();
        assert_eq!(d.lines[2].kind, Kind::Bullet, "A6: the next item");
        d.newline();
        assert_eq!((d.lines[2].kind, d.lines[2].text.as_str()), (Kind::Para, ""), "A6: an empty item ends the list");
        let mut d = doc(&[(0, Kind::Para, "")]);
        for c in ["[", " ", "]", " ", "call"] {
            d.insert(c);
        }
        d.newline();
        assert_eq!((d.lines[1].kind, d.lines[1].status.as_deref()), (Kind::Task, Some("todo")), "A8");
        let mut d = doc(&[(0, Kind::Para, "")]);
        for c in ["#", " ", "Title"] {
            d.insert(c);
        }
        d.newline();
        d.insert("text");
        assert_eq!(texts(&d), ["Para # Title", "Para text"], "a heading is one line");
    }
}
