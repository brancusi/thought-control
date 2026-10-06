//! The document editor (docs/design/tui-editor.md): a page or a journal day as one buffer you
//! type into. Lines are nodes; the core `outline` module turns the buffer into block ops (one
//! transaction per save, `base` on every edit so a concurrent change becomes a conflict).
//!
//! This file is the pure part: the buffer, edits, undo, wrapping and the save diff. Drawing and
//! keys live in ui.rs / input.rs and call in here.

use std::collections::HashMap;
use std::time::Instant;
use thc_core::outline::{Block, BlockOp, Kind};
use unicode_segmentation::UnicodeSegmentation;

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
    /// Whether a blank line comes before this note (`gap`, writing.md §1); None: the default for
    /// its kind (Doc::effective_gap). Kind changes pin it so nothing moves.
    pub gap: Option<bool>,
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
            gap: None,
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

fn l_heading(l: &Line) -> bool {
    l.text.starts_with("# ")
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

/// One undo step: the lines' shape and text, the deletions pending and the caret.
#[derive(Clone)]
struct Snapshot {
    /// Shared with the snapshot before wherever a line didn't change: an undo step costs the
    /// lines it touched, not a copy of the document (500 steps of a 3,000-line page were GBs).
    lines: Vec<std::rc::Rc<Line>>,
    deleted: Vec<String>,
    caret: Pos,
    /// The selection's other end then: undo brings the selection back (editing.md §5).
    anchor: Option<Pos>,
    /// When it was taken (Doc::epoch): lines that arrived after it aren't its to remove.
    seq: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Typing,
    Other,
}

pub struct Doc {
    pub target: Target,
    /// The document's root node (None: a journal day not created yet; its first save makes it).
    pub root: Option<String>,
    pub lines: Vec<Line>,
    pub caret: Pos,
    /// The selection's other end, while selecting.
    pub anchor: Option<Pos>,
    /// The column ↑ / ↓ aim for.
    pub goal_col: Option<usize>,
    /// Nodes removed by merges and deletes, written on the next save.
    pub deleted: Vec<String>,
    /// The first visual row on screen.
    pub scroll: usize,
    undo: Vec<Snapshot>,
    /// Counts snapshots taken, so a line patched in from elsewhere knows which it postdates.
    epoch: u64,
    /// Content generation, independent of caret motion and undo coalescing.
    revision: u64,
    /// Notes patched in from another device or an agent, by the epoch they arrived at.
    arrived: HashMap<String, u64>,
    /// Each line as its last save left it (what the vault has): a line undo brings back while
    /// its delete is still pending takes its save state from here, not from the snapshot, which
    /// may be older than that save (fuzz, late saves: it came back "new" and was made twice).
    last_saved: HashMap<String, Line>,
    redo: Vec<Snapshot>,
    last_edit: Option<(EditKind, usize, Instant)>,
    /// When the buffer last changed (the 1.5 s idle save).
    pub changed_at: Option<Instant>,
    wraps: HashMap<(u64, usize), Vec<(usize, usize)>>,
}

impl Doc {
    pub fn new(target: Target, root: Option<String>, blocks: &[Block], today: chrono::NaiveDate) -> Doc {
        let mut lines: Vec<Line> = blocks.iter().map(|b| Line::from_block(b, today)).collect();
        let mut before: Vec<(usize, String)> = Vec::new();
        for l in lines.iter_mut() {
            l.saved_after = place(l.depth, &before).1;
            before.push((l.depth, l.id.clone()));
        }
        Doc { target, root, lines, caret: Pos::default(), anchor: None, goal_col: None, deleted: vec![], scroll: 0, undo: vec![], epoch: 0,
            revision: 0, arrived: HashMap::new(), last_saved: HashMap::new(), redo: vec![], last_edit: None, changed_at: None, wraps: HashMap::new() }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Invalidate content-derived inputs after a local edit or deferred remote text.
    pub fn touch_content(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn line(&self) -> &Line {
        &self.lines[self.caret.line]
    }

    /// Put the caret at the end of the document's last line, or on a fresh line when the last
    /// line has text (a journal day: ready to type).
    pub fn caret_to_end(&mut self, fresh_line: bool) {
        if self.lines.is_empty() || (fresh_line && !self.lines.last().unwrap().text.is_empty()) {
            let depth = 0;
            self.lines.push(Line::new(depth, Kind::Para, ""));
            self.touch_content();
        }
        let i = self.lines.len() - 1;
        self.caret = Pos { line: i, byte: self.lines[i].text.len() };
    }

    // ---- undo ---------------------------------------------------------------------------------

    fn snapshot(&mut self) -> Snapshot {
        let prev: HashMap<&str, &std::rc::Rc<Line>> = self.undo.last().or(self.redo.last()).map(|s| s.lines.iter().map(|l| (l.id.as_str(), l)).collect()).unwrap_or_default();
        let lines = self
            .lines
            .iter()
            .map(|l| match prev.get(l.id.as_str()) {
                Some(p) if ***p == *l => std::rc::Rc::clone(p),
                _ => std::rc::Rc::new(l.clone()),
            })
            .collect();
        self.epoch += 1;
        Snapshot { lines, deleted: self.deleted.clone(), caret: self.caret, anchor: self.anchor, seq: self.epoch }
    }

    /// Record an undo step before an edit. Typing on one line within 1.5 s is one step.
    fn begin(&mut self, kind: EditKind) {
        self.touch_content();
        // An empty selection (⇧→ at the end of the text: the anchor on the caret) ends with any
        // edit; left behind, it pointed past the text once the edit shortened it (fuzz).
        if self.anchor == Some(self.caret) {
            self.anchor = None;
        }
        let now = Instant::now();
        let coalesce = matches!(self.last_edit, Some((EditKind::Typing, line, at)) if kind == EditKind::Typing && line == self.caret.line && now.duration_since(at).as_millis() < 1500);
        if !coalesce {
            let snapshot = self.snapshot();
            self.undo.push(snapshot);
            if self.undo.len() > 500 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last_edit = Some((kind, self.caret.line, now));
        self.changed_at = Some(now);
    }

    /// A note patched in from elsewhere (see `restore`).
    pub fn mark_arrived(&mut self, id: &str) {
        self.arrived.insert(id.to_string(), self.epoch);
    }

    /// How many undo steps there are (a drop's ⌃Z checks nothing happened since).
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// One undo step before recovered lines go back in (recover.rs), or an attachment.
    pub fn begin_recovery(&mut self) {
        self.begin(EditKind::Other);
        self.last_edit = None;
    }

    pub fn undo(&mut self) -> bool {
        let Some(s) = self.undo.pop() else { return false };
        let snapshot = self.snapshot();
        self.redo.push(snapshot);
        self.restore(s);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(s) = self.redo.pop() else { return false };
        let snapshot = self.snapshot();
        self.undo.push(snapshot);
        self.restore(s);
        true
    }

    /// Back to a snapshot. Save state (base, saved) stays as the server has it, so undoing past
    /// a save edits the line back and saves that as a new transaction.
    fn restore(&mut self, s: Snapshot) {
        self.touch_content();
        // Deletions not saved yet: those notes are still in the vault (fuzz: undo recreated one
        // under a new id and dropped the pending delete, leaving the old one and a copy).
        let pending: std::collections::HashSet<String> = self.deleted.iter().cloned().collect();
        let server: HashMap<String, Line> = self.lines.drain(..).map(|l| (l.id.clone(), l)).collect();
        self.lines = s
            .lines
            .into_iter()
            .map(|rc| std::rc::Rc::try_unwrap(rc).unwrap_or_else(|rc| (*rc).clone()))
            .map(|mut l| {
                if let Some(cur) = server.get(&l.id) {
                    l.base = cur.base.clone();
                    l.saved = cur.saved.clone();
                    l.saved_parent = cur.saved_parent.clone();
                    l.saved_after = cur.saved_after.clone();
                    l.saved_kind = cur.saved_kind;
                    l.saved_status = cur.saved_status.clone();
                    l.saved_gap = cur.saved_gap;
                    l.is_new = cur.is_new;
                    l.meta = cur.meta.clone();
                    l.fields = cur.fields.clone();
                    // What came from elsewhere isn't undone: a held remote text, a ≠ (fuzz: an
                    // undo dropped one, and the buffer kept the old text for good).
                    l.remote_text = cur.remote_text.clone();
                    l.conflict = cur.conflict;
                    l.conflict_with = cur.conflict_with.clone();
                    l.saving_since = cur.saving_since;
                } else if pending.contains(&l.id) {
                    // Saved, removed, its delete not sent: back as the vault has it.
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
                } else {
                    // Gone from the buffer and not waiting to be deleted: it was deleted, or was
                    // unsaved then and its id has since been used and deleted (fuzz: a create
                    // with that id was refused). Bringing it back makes a new note.
                    l.is_new = true;
                    l.id = new_id();
                    l.saving_since = None;
                }
                l
            })
            .collect();
        let alive: std::collections::HashSet<&str> = self.lines.iter().map(|l| l.id.as_str()).collect();
        // Deletions of lines that are back are dropped; lines now gone are deleted.
        let mut deleted: Vec<String> = s.deleted.into_iter().filter(|d| !alive.contains(d.as_str())).collect();
        for (id, l) in &server {
            // A note that arrived from elsewhere after this snapshot isn't undone: it goes back in
            // from the vault (two-device soak: an undo here deleted the other device's note).
            if self.arrived.get(id).is_some_and(|&at| at >= s.seq) {
                continue;
            }
            if !alive.contains(id.as_str()) && !l.is_new && !deleted.contains(id) {
                deleted.push(id.clone());
            }
        }
        // A pending delete of a note that stays gone still goes.
        for id in pending {
            if !alive.contains(id.as_str()) && !deleted.contains(&id) {
                deleted.push(id);
            }
        }
        self.deleted = deleted;
        self.caret = s.caret;
        self.clamp();
        // The selection as it was (EI: undo restores the selection); selection() drops it if an
        // end no longer fits.
        self.anchor = s.anchor;
        self.last_edit = None;
        self.changed_at = Some(Instant::now());
    }

    fn clamp(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(Line::new(0, Kind::Para, ""));
        }
        self.caret.line = self.caret.line.min(self.lines.len() - 1);
        let t = &self.lines[self.caret.line].text;
        self.caret.byte = floor_char(t, self.caret.byte.min(t.len()));
    }

    // ---- selection ------------------------------------------------------------------------------

    /// The selection, ordered (start, end), when there is one.
    pub fn selection(&self) -> Option<(Pos, Pos)> {
        let a = self.anchor?;
        if a == self.caret {
            return None;
        }
        // Both ends must still be inside the text (a line or bytes an edit removed: no selection).
        let ok = |p: Pos| self.lines.get(p.line).is_some_and(|l| p.byte <= l.text.len() && l.text.is_char_boundary(p.byte));
        if !ok(a) || !ok(self.caret) {
            return None;
        }
        Some(if a < self.caret { (a, self.caret) } else { (self.caret, a) })
    }

    /// The selected lines with the part of each that's selected (copy).
    pub fn selected_parts(&self) -> Vec<(usize, String)> {
        let Some((s, e)) = self.selection() else { return vec![] };
        (s.line..=e.line)
            .map(|i| {
                let t = &self.lines[i].text;
                let a = if i == s.line { s.byte } else { 0 };
                let b = if i == e.line { e.byte } else { t.len() };
                (i, t[a..b].to_string())
            })
            .collect()
    }

    /// Remove the selection (one edit), joining its first and last lines.
    fn delete_selection_inner(&mut self) -> bool {
        let Some((s, e)) = self.selection() else { return false };
        let tail = self.lines[e.line].text[e.byte..].to_string();
        for i in (s.line + 1..=e.line).rev() {
            let gone = self.lines.remove(i);
            if !gone.is_new {
                self.deleted.push(gone.block.id);
            }
        }
        let first = &mut self.lines[s.line];
        first.text.truncate(s.byte);
        first.text.push_str(&tail);
        self.caret = s;
        self.anchor = None;
        true
    }

    pub fn delete_selection(&mut self) -> bool {
        if self.selection().is_none() {
            return false;
        }
        self.begin(EditKind::Other);
        self.delete_selection_inner()
    }

    // ---- typing -----------------------------------------------------------------------------------

    /// Typing: a marker typed at a line's start changes its kind, and that never moves another
    /// line (writing.md §1): gaps are pinned as for ⌃T.
    pub fn insert(&mut self, s: &str) {
        let before = self.near_caret();
        self.insert_inner(s);
        self.pin_near(&before);
    }

    /// Before an edit at the caret: its block's shape and the blank lines a change there can
    /// move: its own and the next block's (a default reads only a block and the one before).
    /// O(1): this runs on every key (a whole-document scan here cost 60 ms at 5,000 lines).
    pub fn near_caret(&self) -> (String, Kind, usize, Vec<(String, bool)>) {
        let i = self.caret.line.min(self.lines.len().saturating_sub(1));
        let l = &self.lines[i];
        let near = (i..(i + 2).min(self.lines.len())).map(|j| (self.lines[j].id.clone(), self.effective_gap(j))).collect();
        (l.id.clone(), l.kind, l.depth, near)
    }

    /// After it: if that block's kind or depth changed, the nearby blank lines stay as they were.
    pub fn pin_near(&mut self, before: &(String, Kind, usize, Vec<(String, bool)>)) {
        let (id, kind, depth, near) = before;
        let changed = self.lines.iter().position(|l| &l.id == id).is_some_and(|i| (self.lines[i].kind, self.lines[i].depth) != (*kind, *depth));
        if !changed {
            return;
        }
        for (nid, want) in near {
            if let Some(j) = self.lines.iter().position(|l| &l.id == nid) {
                if j > 0 && self.effective_gap(j) != *want {
                    self.lines[j].gap = Some(*want);
                }
            }
        }
    }

    fn insert_inner(&mut self, s: &str) {
        let had_selection = self.selection().is_some();
        self.begin(if had_selection || s.chars().count() > 1 { EditKind::Other } else { EditKind::Typing });
        self.delete_selection_inner();
        // A key typed over a selection starts a typing run: what follows is the same undo step
        // (editing.md §3, E24).
        if had_selection && s.chars().count() == 1 {
            self.last_edit = Some((EditKind::Typing, self.caret.line, Instant::now()));
        }
        let Pos { line, byte } = self.caret;
        self.lines[line].text.insert_str(byte, s);
        self.caret.byte += s.len();
        self.goal_col = None;
        self.typed_prefix();
    }

    /// `- ` and `[ ] ` typed at the start of a line make it a bullet or a task (consumed into
    /// the hang); `1. ` `# ` `> ` stay in the text (line forms). `1. ` / `1) ` also make the
    /// line a list item (a bullet whose text starts with its number, as paste makes it), so
    /// the number sits in the hang and Enter continues the list.
    fn typed_prefix(&mut self) {
        // A marker typed at the start of a later line of a paragraph starts a note there: the
        // paragraph keeps its id, the item is new (writing.md §1).
        {
            let i = self.caret.line;
            let l = &self.lines[i];
            if l.kind == Kind::Para && !l.text.starts_with("```") {
                let ls = l.text[..self.caret.byte].rfind('\n').map(|k| k + 1).unwrap_or(0);
                let line_now = &l.text[ls..self.caret.byte];
                let digits = line_now.bytes().take_while(u8::is_ascii_digit).count();
                let marker = ["- ", "* ", "+ ", "[ ] ", "[] ", "[x] ", "- [ ] ", "- [x] ", "# ", "## ", "### ", "> "].contains(&line_now)
                    || ((1..=3).contains(&digits) && (line_now[digits..] == *". " || line_now[digits..] == *") "));
                if ls > 0 && marker {
                    let tail = self.lines[i].text.split_off(ls);
                    self.lines[i].text.pop(); // the line break before the marker
                    // The item is the marker's line; lines after it stay a paragraph of their
                    // own (an item never starts with a line break: fuzz).
                    let (item, rest) = match tail.split_once('\n') {
                        Some((a, b)) => (a.to_string(), Some(b.to_string())),
                        None => (tail.clone(), None),
                    };
                    self.lines.insert(i + 1, Line::new(0, Kind::Para, &item));
                    if let Some(rest) = rest.filter(|r| !r.is_empty()) {
                        self.lines.insert(i + 2, Line::new(0, Kind::Para, &rest));
                    }
                    self.caret = Pos { line: i + 1, byte: self.caret.byte - ls };
                }
            }
        }
        let l = &mut self.lines[self.caret.line];
        if l.kind == Kind::Para && self.caret.byte >= 3 {
            let digits = l.text.bytes().take_while(u8::is_ascii_digit).count();
            if (1..=3).contains(&digits) && self.caret.byte == digits + 2 && (l.text[digits..].starts_with(". ") || l.text[digits..].starts_with(") ")) {
                l.kind = Kind::Bullet;
                return;
            }
        }
        let done = l.text.starts_with("[x] ") || l.text.starts_with("- [x] ");
        let (kind, cut) = if l.text.starts_with("- [ ] ") || l.text.starts_with("- [x] ") {
            (Kind::Task, 6)
        } else if l.text.starts_with("[x] ") {
            (Kind::Task, 4)
        } else if l.text.starts_with("- ") || l.text.starts_with("* ") || l.text.starts_with("+ ") {
            (Kind::Bullet, 2)
        } else if l.text.starts_with("[ ] ") {
            (Kind::Task, 4)
        } else if l.text.starts_with("[] ") {
            (Kind::Task, 3)
        } else {
            return;
        };
        // A marker typed at a line's start is always read, whatever the line was: the save
        // reads it so (the vault had `dash-y` while the buffer showed `[ ] dash-y`: fuzz).
        let explicit_todo = kind == Kind::Task && !done;
        l.text.replace_range(..cut, "");
        l.kind = kind;
        l.status = match kind {
            Kind::Task if done => Some("done".into()),
            Kind::Task if explicit_todo => Some("todo".into()),
            _ => None,
        };
        self.caret.byte = self.caret.byte.saturating_sub(cut);
    }

    /// A paragraph line that's one note per line: a heading or a quote (Enter ends it).
    fn line_note(l: &Line) -> bool {
        l.kind == Kind::Para && (l.text.starts_with("# ") || l.text.starts_with("## ") || l.text.starts_with("### ") || l.text.starts_with("> "))
    }

    fn is_fence(l: &Line) -> bool {
        l.kind == Kind::Para && l.text.starts_with("```")
    }

    /// Enter (writing.md §3): in a paragraph, a line break in the same note, and on an empty
    /// line (a blank line) the paragraph splits into two notes; in a list or task, the next item
    /// with the same marker and depth; on an empty item, the list ends (an empty paragraph
    /// line). A heading or quote is one line: Enter starts a paragraph after it.
    pub fn newline(&mut self) {
        self.begin(EditKind::Other);
        self.delete_selection_inner();
        let i = self.caret.line;
        // An item holding only its marker (or nothing) ends the list.
        let only_number = self.lines[i].kind == Kind::Bullet && numbered(&self.lines[i].text).is_some() && crate::doc_ui::marker_len(&self.lines[i]) == self.lines[i].text.len();
        if self.lines[i].kind != Kind::Para && (self.lines[i].text.is_empty() || only_number) {
            let l = &mut self.lines[i];
            l.text.clear();
            l.kind = Kind::Para;
            l.status = None;
            l.depth = 0;
            self.caret.byte = 0;
            return;
        }
        if self.lines[i].kind == Kind::Para && !Self::line_note(&self.lines[i]) {
            let b = self.caret.byte;
            let t = self.lines[i].text.clone();
            if Self::is_fence(&self.lines[i]) {
                self.lines[i].text.insert(b, '\n');
                self.caret.byte += 1;
                self.goal_col = None;
                return;
            }
            if b == 0 {
                // At a note's very start: a new empty note above; the note keeps its id.
                let depth = self.lines[i].depth;
                self.lines.insert(i, Line::new(depth, Kind::Para, ""));
                self.caret = Pos { line: i + 1, byte: 0 };
                self.goal_col = None;
                return;
            }
            if t[..b].ends_with('\n') {
                // Enter at the start of a later line makes a blank line: the note before keeps
                // the id, the text after is a new note (an empty line in between is dropped).
                let before = t[..b - 1].to_string();
                let after = t[b..].strip_prefix('\n').unwrap_or(&t[b..]).to_string();
                self.lines[i].text = before;
                let depth = self.lines[i].depth;
                self.lines.insert(i + 1, Line::new(depth, Kind::Para, &after));
                self.caret = Pos { line: i + 1, byte: 0 };
                self.goal_col = None;
                return;
            }
            if t[b..].starts_with('\n') {
                // Enter at the end of a line with more below: that would leave a blank line in
                // the note, and a blank line splits it (writing.md §1). The note keeps what's
                // before; what's after is a new note; the caret sits on the empty line between.
                let after = t[b + 1..].to_string();
                self.lines[i].text = t[..b].to_string();
                let depth = self.lines[i].depth;
                self.lines.insert(i + 1, Line::new(depth, Kind::Para, ""));
                self.lines.insert(i + 2, Line::new(depth, Kind::Para, &after));
                self.caret = Pos { line: i + 1, byte: 0 };
                self.goal_col = None;
                return;
            }
            self.lines[i].text.insert(b, '\n');
            self.caret.byte += 1;
            self.goal_col = None;
            return;
        }
        let cur = &mut self.lines[i];
        let tail = cur.text.split_off(self.caret.byte);
        let mut next = Line::new(cur.depth, cur.kind, &tail);
        // The next number after a numbered item.
        if tail.is_empty() {
            if let Some(n) = numbered(&cur.text) {
                next.text = format!("{}. ", n + 1);
            }
        }
        if cur.kind == Kind::Task {
            next.status = Some("todo".into());
        }
        let caret_byte = if tail.is_empty() { next.text.len() } else { 0 };
        self.lines.insert(i + 1, next);
        self.caret = Pos { line: i + 1, byte: caret_byte };
        self.goal_col = None;
    }

    /// ⌃J: a line break inside the note (a list item's second line; in a paragraph it's
    /// Enter, blank lines and all).
    pub fn soft_break(&mut self) {
        if self.lines[self.caret.line].kind == Kind::Para && !Self::line_note(&self.lines[self.caret.line]) {
            return self.newline();
        }
        self.begin(EditKind::Other);
        self.delete_selection_inner();
        let Pos { line, byte } = self.caret;
        self.lines[line].text.insert(byte, '\n');
        self.caret.byte += 1;
    }

    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        let Pos { line, byte } = self.caret;
        // Right after a numbered item's number (it sits in the hang): the number goes, and the
        // item becomes a paragraph line.
        if byte > 0 && self.lines[line].kind == Kind::Bullet && numbered(&self.lines[line].text).is_some() && byte == crate::doc_ui::marker_len(&self.lines[line]) {
            self.begin(EditKind::Other);
            self.lines[line].text.replace_range(..byte, "");
            self.lines[line].kind = Kind::Para;
            self.lines[line].depth = 0;
            self.caret.byte = 0;
            return;
        }
        if byte > 0 {
            self.begin(EditKind::Typing);
            let t = &mut self.lines[line].text;
            let prev = prev_char(t, byte);
            t.replace_range(prev..byte, "");
            self.caret.byte = prev;
            return;
        }
        // At the start: a bullet or task becomes a paragraph, plain text (a task stops being
        // one: its status goes too, or it came back as a task on save); a paragraph merges into
        // the line above.
        // The marker goes a step at a time, like deleting its characters: a task becomes a list
        // item (status gone), an item becomes a paragraph line (writing.md A10).
        if self.lines[line].kind == Kind::Task {
            self.begin(EditKind::Other);
            self.lines[line].kind = Kind::Bullet;
            self.lines[line].status = None;
            return;
        }
        if self.lines[line].kind == Kind::Bullet {
            self.begin(EditKind::Other);
            self.lines[line].kind = Kind::Para;
            self.lines[line].depth = 0;
            return;
        }
        if line == 0 {
            return;
        }
        self.begin(EditKind::Other);
        self.merge_into_previous(line);
    }

    /// Delete / ⌃D: the next character, or at the end of a line, merge the next line in.
    pub fn delete_forward(&mut self) {
        if self.delete_selection() {
            return;
        }
        let Pos { line, byte } = self.caret;
        let len = self.lines[line].text.len();
        if byte < len {
            self.begin(EditKind::Typing);
            let t = &mut self.lines[line].text;
            let next = next_char(t, byte);
            t.replace_range(byte..next, "");
            return;
        }
        if line + 1 < self.lines.len() {
            self.begin(EditKind::Other);
            self.merge_into_previous(line + 1);
        }
    }

    /// A join (writing.md §1): the upper note keeps its id, the lower goes in the same save. Two
    /// paragraphs join as one with the line break kept (deleting the blank line between them).
    fn merge_into_previous(&mut self, line: usize) {
        let gone = self.lines.remove(line);
        let both_para = gone.kind == Kind::Para && self.lines[line - 1].kind == Kind::Para && !Self::line_note(&gone) && !Self::line_note(&self.lines[line - 1]) && !self.lines[line - 1].text.is_empty() && !gone.text.is_empty();
        let prev = &mut self.lines[line - 1];
        if both_para {
            prev.text.push('\n');
        }
        let at = prev.text.len();
        prev.text.push_str(&gone.text);
        if !gone.is_new {
            self.deleted.push(gone.block.id);
        }
        self.caret = Pos { line: line - 1, byte: at };
    }

    /// Tab / ⇧Tab: nest under the line above / un-nest. Paragraphs don't nest.
    pub fn nest(&mut self, delta: isize) -> bool {
        let i = self.caret.line;
        if self.lines[i].kind == Kind::Para {
            return false;
        }
        // One deeper than the note above at most: an empty line above isn't a note (it's never
        // saved), so it can't hold this one (fuzz: a first item indented under nothing).
        let max = self.lines[..i].iter().rev().find(|l| !l.text.trim().is_empty()).map_or(0, |l| l.depth + 1);
        let d = (self.lines[i].depth as isize + delta).clamp(0, max as isize) as usize;
        if d == self.lines[i].depth {
            return false;
        }
        self.begin(EditKind::Other);
        self.lines[i].depth = d;
        true
    }

    /// ⌃T (writing.md §3): text → `[ ]` → `[x]` → text, on the caret's line or every line of the
    /// selection (they all take the first line's next state). A repeating task "done" advances
    /// on save instead of closing. Reaching text clears the status (and its dates, on save).
    /// A click on a task's box: open ⇄ done, never back to text (mouse.md §3; that's ⌃T's job).
    pub fn task_box(&mut self, line: usize) -> &'static str {
        if self.lines[line].kind != Kind::Task {
            return "";
        }
        self.begin(EditKind::Other);
        let l = &mut self.lines[line];
        if l.status.as_deref() == Some("done") {
            l.status = Some("todo".into());
            "reopened"
        } else {
            l.status = Some("done".into());
            "done"
        }
    }

    /// Select the word at `p` (Unicode word boundaries: a run of CJK is one word).
    pub fn select_word(&mut self, p: Pos) {
        let t = &self.lines[p.line].text;
        let (mut a, mut b) = (p.byte, p.byte);
        for (i, w) in t.split_word_bound_indices() {
            if p.byte >= i && p.byte < i + w.len() {
                a = i;
                b = i + w.len();
                break;
            }
        }
        self.anchor = Some(Pos { line: p.line, byte: a });
        self.caret = Pos { line: p.line, byte: b };
        self.goal_col = None;
    }

    /// Select the whole note at `line` (the paragraph, or the item's text).
    pub fn select_note(&mut self, line: usize) {
        self.anchor = Some(Pos { line, byte: 0 });
        self.caret = Pos { line, byte: self.lines[line].text.len() };
        self.goal_col = None;
    }

    /// The blank line before note `i` by its kind and its neighbour's, as thc always drew it: a
    /// paragraph keeps a blank line on either side (a `##` heading only above), lists stay tight.
    pub fn default_gap(&self, i: usize) -> bool {
        if i == 0 || i >= self.lines.len() {
            return false;
        }
        let (a, b) = (&self.lines[i - 1], &self.lines[i]);
        let para = |l: &Line| l.kind == Kind::Para;
        let sub = |l: &Line| l.text.starts_with("## ") || l.text.starts_with("### ");
        let after = para(a) && !sub(a);
        let before = para(b) && (l_heading(b) || sub(b));
        after || before || (para(b) && !para(a))
    }

    /// Whether a blank line is drawn before note `i` (writing.md §1): its `gap`, else the
    /// default. A paragraph right after a paragraph always has one (else they'd be one note).
    pub fn effective_gap(&self, i: usize) -> bool {
        if i == 0 || i >= self.lines.len() {
            return false;
        }
        if self.lines[i].kind == Kind::Para && self.lines[i - 1].kind == Kind::Para {
            return true;
        }
        self.lines[i].gap.unwrap_or_else(|| self.default_gap(i))
    }

    /// After a kind change: every note keeps the blank line it had (`before`, by id). Where the
    /// default would now differ, the gap is written explicitly (pinned), so nothing moves.
    pub fn pin_gaps(&mut self, before: &std::collections::HashMap<String, bool>) {
        for i in 1..self.lines.len() {
            let Some(&want) = before.get(&self.lines[i].id) else { continue };
            if self.effective_gap(i) != want {
                self.lines[i].gap = Some(want);
            }
        }
    }

    /// Every note's blank line before it, by id.
    pub fn gaps(&self) -> std::collections::HashMap<String, bool> {
        (0..self.lines.len()).map(|i| (self.lines[i].id.clone(), self.effective_gap(i))).collect()
    }

    pub fn task_cycle(&mut self) -> &'static str {
        self.begin(EditKind::Other);
        // ⌃T on a line inside a multi-line paragraph tasks that line only.
        if let Some(what) = self.split_for_task() {
            return what;
        }
        let gaps = self.gaps();
        let lines: Vec<usize> = match self.selection() {
            Some((a, b)) => (a.line..=b.line).collect(),
            None => vec![self.caret.line],
        };
        let first = &self.lines[lines[0]];
        let (kind, status, what): (Kind, Option<&str>, &'static str) = match (first.kind, first.status.as_deref()) {
            (Kind::Task, Some("done")) => (Kind::Para, None, "text"),
            (Kind::Task, _) => (Kind::Task, Some("done"), "done"),
            _ => (Kind::Task, Some("todo"), "task"),
        };
        for i in lines {
            let l = &mut self.lines[i];
            if Self::is_fence(l) {
                continue;
            }
            l.kind = kind;
            l.status = status.map(str::to_string);
            if kind == Kind::Para {
                l.depth = 0;
                // Text has no status or dates: the meta shows none now (the save clears them).
                l.meta.clear();
                l.fields.clear();
            }
        }
        // Un-tasking one line next to a paragraph with no blank line between: it joins it, the
        // exact reverse of the split (E81). Never across a gap.
        if kind == Kind::Para && self.selection().is_none() {
            let mut i = self.caret.line;
            let caret_byte = self.caret.byte;
            let joins = |d: &Doc, i: usize| i > 0 && d.lines[i - 1].kind == Kind::Para && !gaps.get(&d.lines[i].id).copied().unwrap_or(true) && d.lines[i - 1].depth == 0;
            if joins(self, i) {
                let at = self.lines[i - 1].text.len() + 1;
                self.lines[i - 1].text.push('\n');
                let gone = self.lines.remove(i);
                self.lines[i - 1].text.push_str(&gone.text);
                if !gone.is_new {
                    self.deleted.push(gone.block.id);
                }
                i -= 1;
                self.caret = Pos { line: i, byte: at + caret_byte };
            }
            if i + 1 < self.lines.len() && self.lines[i + 1].kind == Kind::Para && self.lines[i + 1].depth == 0 && !gaps.get(&self.lines[i + 1].id).copied().unwrap_or(true) {
                self.lines[i].text.push('\n');
                let gone = self.lines.remove(i + 1);
                self.lines[i].text.push_str(&gone.text);
                if !gone.is_new {
                    self.deleted.push(gone.block.id);
                }
            }
        }
        what
    }

    /// ⌃T inside a multi-line paragraph (writing.md §1, E70-E72): the caret's line (or each
    /// selected line) becomes its own task; the lines above and below stay paragraph pieces.
    /// The piece holding the first line keeps the note's id; the new pieces sit right against
    /// it (no blank line), so nothing moves. None when it isn't that case.
    fn split_for_task(&mut self) -> Option<&'static str> {
        let i = self.caret.line;
        let (s, e) = self.selection().unwrap_or((self.caret, self.caret));
        if s.line != i || e.line != i {
            return None;
        }
        let l = &self.lines[i];
        if l.kind != Kind::Para || !l.text.contains('\n') || Self::is_fence(l) {
            return None;
        }
        let text = l.text.clone();
        let segs: Vec<&str> = text.split('\n').collect();
        let mut starts = vec![0usize];
        for sg in &segs[..segs.len() - 1] {
            starts.push(starts.last().unwrap() + sg.len() + 1);
        }
        let seg_of = |b: usize| starts.iter().rposition(|&st| st <= b).unwrap_or(0);
        let (ka, kb) = (seg_of(s.byte), seg_of(e.byte));
        // Pieces: (text, is_task, the byte in the original where the piece starts).
        let mut pieces: Vec<(String, bool, usize)> = Vec::new();
        if ka > 0 {
            pieces.push((segs[..ka].join("\n"), false, 0));
        }
        for k in ka..=kb {
            pieces.push((segs[k].to_string(), true, starts[k]));
        }
        if kb + 1 < segs.len() {
            pieces.push((segs[kb + 1..].join("\n"), false, starts[kb + 1]));
        }
        let map = |b: usize, pieces: &[(String, bool, usize)]| -> Pos {
            let k = pieces.iter().rposition(|p| p.2 <= b).unwrap_or(0);
            Pos { line: i + k, byte: (b - pieces[k].2).min(pieces[k].0.len()) }
        };
        let caret = map(self.caret.byte, &pieces);
        let anchor = self.anchor.map(|a| map(a.byte, &pieces));
        let orig = self.lines[i].clone();
        let mut new_lines = Vec::with_capacity(pieces.len());
        for (k, (t, task, _)) in pieces.iter().enumerate() {
            let mut nl = if k == 0 {
                let mut o = orig.clone();
                o.text = t.clone();
                o
            } else {
                let mut n = Line::new(0, Kind::Para, t);
                n.gap = Some(false);
                n
            };
            if *task {
                nl.kind = Kind::Task;
                nl.status = Some("todo".into());
            }
            new_lines.push(nl);
        }
        self.lines.splice(i..=i, new_lines);
        self.caret = caret;
        self.anchor = anchor;
        self.goal_col = None;
        Some("task")
    }

    /// The line and its children: [i, end).
    fn subtree(&self, i: usize) -> usize {
        let d = self.lines[i].depth;
        let mut j = i + 1;
        while j < self.lines.len() && self.lines[j].depth > d {
            j += 1;
        }
        j
    }

    /// ⌥↑ / ⌥↓: swap the line (with its children) with its previous / next sibling.
    pub fn move_line(&mut self, delta: isize) -> Result<(), &'static str> {
        let i = self.caret.line;
        let d = self.lines[i].depth;
        let (upper, lower) = if delta < 0 {
            let mut j = i as isize - 1;
            while j >= 0 && self.lines[j as usize].depth > d {
                j -= 1;
            }
            if j < 0 || self.lines[j as usize].depth != d {
                return Err("first in its list · ⇧Tab to move out");
            }
            ((j as usize, i), (i, self.subtree(i)))
        } else {
            let e = self.subtree(i);
            if e >= self.lines.len() || self.lines[e].depth != d {
                return Err("last in its list · ⇧Tab to move out");
            }
            ((i, e), (e, self.subtree(e)))
        };
        self.begin(EditKind::Other);
        let lower_block: Vec<Line> = self.lines.drain(lower.0..lower.1).collect();
        let n = lower_block.len();
        for (k, l) in lower_block.into_iter().enumerate() {
            self.lines.insert(upper.0 + k, l);
        }
        let offset = self.caret.line - if delta < 0 { lower.0 } else { upper.0 };
        self.caret.line = if delta < 0 { upper.0 + offset } else { upper.0 + n + offset };
        Ok(())
    }

    /// How many lines sit under line i (its subtree, less itself).
    pub fn descendants(&self, i: usize) -> usize {
        self.subtree(i) - i - 1
    }

    // ---- caret movement (logical; visual-row moves are in `View`) -------------------------------

    pub fn left(&mut self, select: bool) {
        self.select(select);
        let Pos { line, byte } = self.caret;
        if byte > 0 {
            self.caret.byte = prev_char(&self.lines[line].text, byte);
        } else if line > 0 {
            self.caret = Pos { line: line - 1, byte: self.lines[line - 1].text.len() };
        }
        self.goal_col = None;
    }

    pub fn word_left(&mut self, select: bool) {
        self.select(select);
        let t = &self.lines[self.caret.line].text;
        let before: Vec<(usize, char)> = t[..self.caret.byte].char_indices().collect();
        let mut k = before.len();
        while k > 0 && !before[k - 1].1.is_alphanumeric() {
            k -= 1;
        }
        while k > 0 && before[k - 1].1.is_alphanumeric() {
            k -= 1;
        }
        if k == 0 && self.caret.byte == 0 && self.caret.line > 0 {
            return self.left(select);
        }
        self.caret.byte = before.get(k).map(|c| c.0).unwrap_or(0).min(self.caret.byte);
        if k == 0 {
            self.caret.byte = 0;
        }
    }

    pub fn select(&mut self, select: bool) {
        if select {
            if self.anchor.is_none() {
                self.anchor = Some(self.caret);
            }
        } else {
            self.anchor = None;
        }
    }

    /// ⌃W / ⌥Backspace: the word before the caret.
    pub fn delete_word_back(&mut self) {
        if self.delete_selection() {
            return;
        }
        let end = self.caret;
        self.word_left(false);
        let start = self.caret;
        // The undo step is taken with the caret where it was (EI5: undo puts it back there).
        self.caret = end;
        if start.line != end.line {
            return self.backspace();
        }
        if start.byte == end.byte {
            return;
        }
        self.begin(EditKind::Other);
        self.caret = start;
        self.lines[end.line].text.replace_range(start.byte..end.byte, "");
    }

    /// ⌃K: to the end of the line; at the end, merge the next line in.
    pub fn kill_to_end(&mut self) {
        // With a selection, only the selection goes (editing.md §3: line deletes never extend it).
        if self.delete_selection() {
            return;
        }
        let Pos { line, byte } = self.caret;
        let t = &self.lines[line].text;
        let end = t[byte..].find('\n').map(|k| byte + k).unwrap_or(t.len());
        if end == byte {
            return self.delete_forward();
        }
        self.begin(EditKind::Other);
        self.lines[line].text.replace_range(byte..end, "");
    }

    /// ⌃U: to the start of the line.
    pub fn kill_to_start(&mut self) {
        if self.delete_selection() {
            return;
        }
        let Pos { line, byte } = self.caret;
        let t = &self.lines[line].text;
        let start = t[..byte].rfind('\n').map(|k| k + 1).unwrap_or(0);
        if start == byte {
            return;
        }
        self.begin(EditKind::Other);
        self.lines[line].text.replace_range(start..byte, "");
        self.caret.byte = start;
    }

    /// Lines pasted (bracketed paste, already parsed): the current line splits at the caret;
    /// the first pasted line joins the text before it, the last the text after. One undo step.
    pub fn paste_lines(&mut self, pasted: Vec<(usize, Kind, Option<String>, String)>) {
        if pasted.is_empty() {
            return;
        }
        self.begin(EditKind::Other);
        self.delete_selection_inner();
        let i = self.caret.line;
        let base = if self.lines[i].kind == Kind::Para { 0 } else { self.lines[i].depth };
        let tail = self.lines[i].text.split_off(self.caret.byte);
        let before_empty = self.lines[i].text.is_empty();
        let mut at = i;
        for (k, (depth, kind, status, text)) in pasted.into_iter().enumerate() {
            let depth = if kind == Kind::Para { 0 } else { base + depth };
            if k == 0 {
                let l = &mut self.lines[i];
                if before_empty {
                    l.kind = kind;
                    l.status = status;
                    l.depth = depth;
                }
                l.text.push_str(&text);
            } else {
                let mut l = Line::new(depth, kind, &text);
                l.status = status;
                at += 1;
                self.lines.insert(at, l);
            }
        }
        let last = &mut self.lines[at];
        let caret = last.text.len();
        last.text.push_str(&tail);
        self.caret = Pos { line: at, byte: caret };
    }
}

/// The number of a `12. ` numbered line.
fn numbered(text: &str) -> Option<u64> {
    let digits: String = text.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let rest = &text[digits.len()..];
    (rest.starts_with(". ") || rest.starts_with(") ")).then(|| digits.parse().ok()).flatten()
}

fn floor_char(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
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
        for l in self.lines.iter_mut() {
            if l.text.trim().is_empty() {
                continue;
            }
            let max = above.map_or(0, |d| d + 1);
            if l.depth > max {
                l.depth = max;
            }
            above = Some(l.depth);
        }
        let caret = self.caret.line;
        // A saved note emptied to nothing (and left) goes: an empty line isn't a note, and an
        // empty node kept its old place, which other notes were then placed after (fuzz). The
        // line stays as a plain empty line; its children are placed by this save before the
        // delete runs (deletes go last).
        for (i, l) in self.lines.iter_mut().enumerate() {
            if !l.is_new && l.text.trim().is_empty() && (all || i != caret) && !l.conflict {
                self.deleted.push(std::mem::replace(&mut l.id, new_id()));
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
        for (i, l) in self.lines.iter().enumerate() {
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
        for id in self.deleted.drain(..) {
            ops.push(BlockOp::Delete { id, rev: None });
        }
        let sent = Sent {
            lines: self.lines.iter().map(|l| (l.id.clone(), (l.text.clone(), l.status.clone()))).collect(),
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
            for l in &self.lines {
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
            let Some(i) = self.lines.iter().position(|l| l.id == r.id) else {
                // Made by this save, gone from the buffer since: the vault has it now, so it goes.
                if r.state == "ok" && sent.created.contains(&r.id) && !self.deleted.contains(&r.id) {
                    self.deleted.push(r.id.clone());
                }
                continue;
            };
            let l = &mut self.lines[i];
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
                        if self.caret.line == i {
                            self.caret.byte = self.caret.byte.min(l.text.len());
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
                let md = markdown(&parts.iter().map(|(i, t)| (&self.lines[*i], t.as_str())).collect::<Vec<_>>());
                // One trailing newline goes; a boundary at the end (an empty last note) stays.
                let md = md.strip_suffix('\n').unwrap_or(&md);
                let from_start = self.selection().is_some_and(|(s, _)| s.byte == 0);
                if from_start || self.lines[parts[0].0].kind == Kind::Para {
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
        self.lines[i].text.hash(&mut h);
        let key = (h.finish(), w);
        if let Some(r) = self.wraps.get(&key) {
            return r.clone();
        }
        // A code block never wraps: one row per line (it scrolls sideways instead, §3.2).
        let r = if self.lines[i].kind == Kind::Para && self.lines[i].text.starts_with("```") {
            wrap(&self.lines[i].text, usize::MAX / 2)
        } else {
            wrap_with(&self.lines[i].text, w, width(&self.lines[i].text[..crate::doc_ui::marker_len(&self.lines[i])]))
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
        assert!(five.is_char_boundary(rows[0].1) && five[..rows[0].1].graphemes(true).count() == 4, "{rows:?}");
        // A heading's marker sits in the hang: the first row gets its columns back.
        assert_eq!(wrap_with("## abcdefgh ij", 8, 3), vec![(0, 12), (12, 14)]);
        assert_eq!(wrap("abcdefgh ij", 8), vec![(0, 9), (9, 11)], "a word filling the row stays on it");
    }

    /// Undo steps share the lines they didn't touch (jank: undo copied the whole document).
    #[test]
    fn undo_steps_share_untouched_lines() {
        let lines: Vec<(usize, Kind, String)> = (0..1000).map(|i| (0, Kind::Bullet, format!("line {i}"))).collect();
        let refs: Vec<(usize, Kind, &str)> = lines.iter().map(|(d, k, t)| (*d, *k, t.as_str())).collect();
        let mut d = doc(&refs);
        for i in 0..10 {
            d.caret = Pos { line: i, byte: d.lines[i].text.len() };
            d.insert("!");
        }
        assert_eq!(d.undo.len(), 10);
        assert!(std::rc::Rc::ptr_eq(&d.undo[0].lines[500], &d.undo[9].lines[500]), "an untouched line is one allocation");
        assert!(!std::rc::Rc::ptr_eq(&d.undo[0].lines[3], &d.undo[9].lines[3]), "an edited one isn't");
        for _ in 0..10 {
            assert!(d.undo());
        }
        assert!(d.lines.iter().take(10).all(|l| !l.text.ends_with('!')), "undo restores every step");
        while d.redo() {}
        assert!(d.lines.iter().take(10).all(|l| l.text.ends_with('!')), "and redo replays them");
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
