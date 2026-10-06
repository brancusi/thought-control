//! The document buffer (caretline SPEC §3, §6, §7): blocks, the caret and selection, the
//! editing rules (typing, Enter and Backspace, nesting, the task cycle, moving blocks, pasting
//! outlines), pending deletions and undo. A host's line type carries the block plus whatever
//! the host keeps beside it (its save state), through [`BlockLine`].

use crate::{Block, Kind, Pos, next_char, prev_char};
use std::collections::HashMap;
use std::time::Instant;
use unicode_segmentation::UnicodeSegmentation;

/// A host's line: a [`Block`] (through `Deref`) plus the host's own state, and the few things
/// the buffer needs from it.
pub trait BlockLine: Clone + PartialEq + std::ops::Deref<Target = Block<Self::Id>> + std::ops::DerefMut {
    type Id: Clone + Eq + std::hash::Hash;
    /// A new, unsaved line with a new id.
    fn fresh(depth: usize, kind: Kind, text: &str) -> Self;
    /// Never saved: removing it deletes nothing.
    fn is_new(&self) -> bool;
    /// Undo restored this line's text and shape: what the host knows about it now (`cur`) stays.
    fn keep_host_state(&mut self, cur: &Self);
    /// Undo brought back a line that's gone (and isn't waiting to be deleted): make it new.
    fn revive(&mut self);
    /// The task cycle made it plain text: clear what the host shows for a task.
    fn text_only(&mut self) {}
    /// Fields the host keeps beside the text that a copy re-attaches (thc: ` due:fri !high`).
    fn fields(&self) -> &str {
        ""
    }
}

/// The caret's block and the blank lines near it, before an edit ([`Buffer::near_caret`]).
pub struct Near<Id> {
    id: Id,
    kind: Kind,
    depth: usize,
    gaps: Vec<(Id, bool)>,
}

/// One undo step: the lines' shape and text, the deletions pending and the caret.
struct Snapshot<L: BlockLine> {
    /// Shared with the snapshot before wherever a line didn't change: an undo step costs the
    /// lines it touched, not a copy of the document (500 steps of a 3,000-line page were GBs).
    lines: Vec<std::rc::Rc<L>>,
    deleted: Vec<L::Id>,
    caret: Pos,
    /// The selection's other end then: undo brings the selection back.
    anchor: Option<Pos>,
    /// When it was taken (Doc::epoch): lines that arrived after it aren't its to remove.
    seq: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Typing,
    Other,
}

/// The buffer: lines, caret, selection, the deletions a save will write, and undo.
pub struct Buffer<L: BlockLine> {
    pub lines: Vec<L>,
    pub caret: Pos,
    /// The selection's other end, while selecting.
    pub anchor: Option<Pos>,
    /// The column ↑ / ↓ aim for.
    pub goal_col: Option<usize>,
    /// Blocks removed by merges and deletes, for the host's next commit.
    pub deleted: Vec<L::Id>,
    undo: Vec<Snapshot<L>>,
    /// Counts snapshots taken, so a line patched in from elsewhere knows which it postdates.
    epoch: std::cell::Cell<u64>,
    /// Blocks patched in from elsewhere (another device, an agent), by the epoch they arrived at.
    arrived: HashMap<L::Id, u64>,
    redo: Vec<Snapshot<L>>,
    last_edit: Option<(EditKind, usize, Instant)>,
    /// When the buffer last changed (a host's idle commit).
    pub changed_at: Option<Instant>,
    /// Bumped by every edit and every undo or redo (not by motion): a host keys caches of what
    /// it derives from the text (wraps, highlights) on it.
    content_rev: u64,
    /// The host's clock (typing runs, changed_at): `Instant::now` unless the host replays.
    clock: Clock,
    /// The host's new lines (Enter, a split, a paste): `L::fresh` unless the host mints ids.
    mint: Mint<L>,
}

/// Where the engine reads the time. A host that replays (a pure update loop, a test) passes
/// the message's own time instead of the wall clock.
pub type Clock = std::sync::Arc<dyn Fn() -> Instant + Send + Sync>;

/// Where the engine gets a new line: (depth, kind, text). A host that replays mints ids from
/// the message instead of at random.
pub type Mint<L> = std::sync::Arc<std::sync::Mutex<dyn FnMut(usize, Kind, &str) -> L + Send>>;

impl<L: BlockLine> Buffer<L> {
    pub fn new(lines: Vec<L>) -> Self {
        Buffer {
            lines,
            caret: Pos::default(),
            anchor: None,
            goal_col: None,
            deleted: vec![],
            undo: vec![],
            epoch: std::cell::Cell::new(0),
            arrived: HashMap::new(),
            redo: vec![],
            last_edit: None,
            changed_at: None,
            content_rev: 0,
            clock: std::sync::Arc::new(Instant::now),
            mint: std::sync::Arc::new(std::sync::Mutex::new(|d: usize, k: Kind, t: &str| L::fresh(d, k, t))),
        }
    }

    /// Read the time from the host's clock from now on.
    pub fn set_clock(&mut self, clock: Clock) {
        self.clock = clock;
    }

    /// Make new lines with the host's minter from now on.
    pub fn set_mint(&mut self, mint: Mint<L>) {
        self.mint = mint;
    }

    /// The time, by the host's clock.
    pub fn now(&self) -> Instant {
        (self.clock)()
    }

    /// A new line, from the host's minter.
    pub fn fresh(&self, depth: usize, kind: Kind, text: &str) -> L {
        (self.mint.lock().unwrap_or_else(|e| e.into_inner()))(depth, kind, text)
    }

    /// Put the caret at the end of the document's last line, or on a fresh line when the last
    /// line has text (a journal day: ready to type).
    pub fn caret_to_end(&mut self, fresh_line: bool) {
        if self.lines.is_empty() || (fresh_line && !self.lines.last().unwrap().text.is_empty()) {
            let depth = 0;
            self.lines.push(self.fresh(depth, Kind::Para, ""));
        }
        let i = self.lines.len() - 1;
        self.caret = Pos { line: i, byte: self.lines[i].text.len() };
    }

    // ---- undo ---------------------------------------------------------------------------------

    fn snapshot(&self) -> Snapshot<L> {
        let prev: HashMap<&L::Id, &std::rc::Rc<L>> = self.undo.last().or(self.redo.last()).map(|s| s.lines.iter().map(|l| (&l.id, l)).collect()).unwrap_or_default();
        let lines = self
            .lines
            .iter()
            .map(|l| match prev.get(&l.id) {
                Some(p) if ***p == *l => std::rc::Rc::clone(p),
                _ => std::rc::Rc::new(l.clone()),
            })
            .collect();
        self.epoch.set(self.epoch.get() + 1);
        Snapshot { lines, deleted: self.deleted.clone(), caret: self.caret, anchor: self.anchor, seq: self.epoch.get() }
    }

    /// Record an undo step before an edit. Typing on one line within 1.5 s is one step.
    fn begin(&mut self, kind: EditKind) {
        self.content_rev = self.content_rev.wrapping_add(1);
        // An empty selection (⇧→ at the end of the text: the anchor on the caret) ends with any
        // edit; left behind, it pointed past the text once the edit shortened it (fuzz).
        if self.anchor == Some(self.caret) {
            self.anchor = None;
        }
        let now = self.now();
        let coalesce = matches!(self.last_edit, Some((EditKind::Typing, line, at)) if kind == EditKind::Typing && line == self.caret.line && now.duration_since(at).as_millis() < 1500);
        if !coalesce {
            self.undo.push(self.snapshot());
            if self.undo.len() > 500 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last_edit = Some((kind, self.caret.line, now));
        self.changed_at = Some(now);
    }

    /// The content revision: changes with every edit, undo and redo, never with motion.
    pub fn content_rev(&self) -> u64 {
        self.content_rev
    }

    /// A note patched in from elsewhere (see `restore`).
    pub fn mark_arrived(&mut self, id: &L::Id) {
        self.arrived.insert(id.clone(), self.epoch.get());
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
        self.redo.push(self.snapshot());
        self.restore(s);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(s) = self.redo.pop() else { return false };
        self.undo.push(self.snapshot());
        self.restore(s);
        true
    }

    /// Back to a snapshot. Save state (base, saved) stays as the server has it, so undoing past
    /// a save edits the line back and saves that as a new transaction.
    fn restore(&mut self, s: Snapshot<L>) {
        self.content_rev = self.content_rev.wrapping_add(1);
        // Deletions not saved yet: those notes are still in the vault (fuzz: undo recreated one
        // under a new id and dropped the pending delete, leaving the old one and a copy).
        let pending: std::collections::HashSet<L::Id> = self.deleted.iter().cloned().collect();
        let server: HashMap<L::Id, L> = self.lines.drain(..).map(|l| (l.id.clone(), l)).collect();
        self.lines = s
            .lines
            .into_iter()
            .map(|rc| std::rc::Rc::try_unwrap(rc).unwrap_or_else(|rc| (*rc).clone()))
            .map(|mut l| {
                if let Some(cur) = server.get(&l.id) {
                    l.keep_host_state(cur);
                } else if !pending.contains(&l.id) {
                    // Gone from the buffer and not waiting to be deleted: it was deleted, or was
                    // unsaved then and its id has since been used and deleted (fuzz: a create
                    // with that id was refused). Bringing it back makes a new note.
                    l.revive();
                }
                l
            })
            .collect();
        let alive: std::collections::HashSet<&L::Id> = self.lines.iter().map(|l| &l.id).collect();
        // Deletions of lines that are back are dropped; lines now gone are deleted.
        let mut deleted: Vec<L::Id> = s.deleted.into_iter().filter(|d| !alive.contains(d)).collect();
        for (id, l) in &server {
            // A note that arrived from elsewhere after this snapshot isn't undone: it goes back in
            // from the vault (two-device soak: an undo here deleted the other device's note).
            if self.arrived.get(id).is_some_and(|&at| at >= s.seq) {
                continue;
            }
            if !alive.contains(id) && !l.is_new() && !deleted.contains(id) {
                deleted.push(id.clone());
            }
        }
        // A pending delete of a note that stays gone still goes.
        for id in pending {
            if !alive.contains(&id) && !deleted.contains(&id) {
                deleted.push(id);
            }
        }
        self.deleted = deleted;
        self.caret = s.caret;
        self.clamp();
        // The selection as it was; selection() drops it if an end no longer fits.
        self.anchor = s.anchor;
        self.last_edit = None;
        self.changed_at = Some(self.now());
    }

    fn clamp(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(self.fresh(0, Kind::Para, ""));
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
            if !gone.is_new() {
                self.deleted.push(gone.id.clone());
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

    /// Typing. A marker typed at a line's start changes its kind, and that never moves another
    /// line: gaps are pinned as for ⌃T.
    pub fn insert(&mut self, s: &str) {
        let before = self.near_caret();
        self.insert_inner(s);
        self.pin_near(&before);
    }

    /// Before an edit at the caret: its block's shape and the blank lines a change there can
    /// move, its own and the next block's (a default reads only a block and the one before).
    /// O(1): it runs on every key.
    pub fn near_caret(&self) -> Near<L::Id> {
        let i = self.caret.line.min(self.lines.len().saturating_sub(1));
        let l = &self.lines[i];
        let gaps = (i..(i + 2).min(self.lines.len())).map(|j| (self.lines[j].id.clone(), self.effective_gap(j))).collect();
        Near { id: l.id.clone(), kind: l.kind, depth: l.depth, gaps }
    }

    /// After it: if that block's kind or depth changed, the nearby blank lines stay as they were.
    pub fn pin_near(&mut self, before: &Near<L::Id>) {
        let changed = self.lines.iter().position(|l| l.id == before.id).is_some_and(|i| (self.lines[i].kind, self.lines[i].depth) != (before.kind, before.depth));
        if !changed {
            return;
        }
        for (id, want) in &before.gaps {
            if let Some(j) = self.lines.iter().position(|l| &l.id == id) {
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
        // A key typed over a selection starts a typing run: what follows is the same undo step.
        if had_selection && s.chars().count() == 1 {
            self.last_edit = Some((EditKind::Typing, self.caret.line, self.now()));
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
                    self.lines.insert(i + 1, self.fresh(0, Kind::Para, &item));
                    if let Some(rest) = rest.filter(|r| !r.is_empty()) {
                        self.lines.insert(i + 2, self.fresh(0, Kind::Para, &rest));
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
    fn line_note(l: &L) -> bool {
        l.kind == Kind::Para && (l.text.starts_with("# ") || l.text.starts_with("## ") || l.text.starts_with("### ") || l.text.starts_with("> "))
    }

    fn is_fence(l: &L) -> bool {
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
        let only_number = self.lines[i].kind == Kind::Bullet && numbered(&self.lines[i].text).is_some() && crate::markdown::marker_len(&self.lines[i]) == self.lines[i].text.len();
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
                self.lines.insert(i, self.fresh(depth, Kind::Para, ""));
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
                self.lines.insert(i + 1, self.fresh(depth, Kind::Para, &after));
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
                self.lines.insert(i + 1, self.fresh(depth, Kind::Para, ""));
                self.lines.insert(i + 2, self.fresh(depth, Kind::Para, &after));
                self.caret = Pos { line: i + 1, byte: 0 };
                self.goal_col = None;
                return;
            }
            self.lines[i].text.insert(b, '\n');
            self.caret.byte += 1;
            self.goal_col = None;
            return;
        }
        let byte = self.caret.byte;
        let tail = self.lines[i].text.split_off(byte);
        let mut next = self.fresh(self.lines[i].depth, self.lines[i].kind, &tail);
        let cur = &mut self.lines[i];
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
        if byte > 0 && self.lines[line].kind == Kind::Bullet && numbered(&self.lines[line].text).is_some() && byte == crate::markdown::marker_len(&self.lines[line]) {
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
        if !gone.is_new() {
            self.deleted.push(gone.id.clone());
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

    /// The blank line before block `i` by its kind and its neighbour's: a paragraph keeps a
    /// blank line on either side (a `##` heading only above), lists stay tight.
    pub fn default_gap(&self, i: usize) -> bool {
        if i == 0 || i >= self.lines.len() {
            return false;
        }
        let (a, b) = (&self.lines[i - 1], &self.lines[i]);
        let para = |l: &L| l.kind == Kind::Para;
        let sub = |l: &L| l.text.starts_with("## ") || l.text.starts_with("### ");
        let after = para(a) && !sub(a);
        let before = para(b) && (b.text.starts_with("# ") || sub(b));
        after || before || (para(b) && !para(a))
    }

    /// Whether a blank line comes before block `i`: its `gap`, else the default. A paragraph
    /// right after a paragraph always has one (else they'd be one block).
    pub fn effective_gap(&self, i: usize) -> bool {
        if i == 0 || i >= self.lines.len() {
            return false;
        }
        if self.lines[i].kind == Kind::Para && self.lines[i - 1].kind == Kind::Para {
            return true;
        }
        self.lines[i].gap.unwrap_or_else(|| self.default_gap(i))
    }

    /// After a kind change: every block keeps the blank line it had (`before`, by id), written
    /// explicitly where the default would now differ, so nothing moves.
    pub fn pin_gaps(&mut self, before: &HashMap<L::Id, bool>) {
        for i in 1..self.lines.len() {
            let Some(&want) = before.get(&self.lines[i].id) else { continue };
            if self.effective_gap(i) != want {
                self.lines[i].gap = Some(want);
            }
        }
    }

    /// Every block's blank line before it, by id.
    pub fn gaps(&self) -> HashMap<L::Id, bool> {
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
                l.text_only();
            }
        }
        // Un-tasking one line next to a paragraph with no blank line between: it joins it, the
        // exact reverse of the split. Never across a gap.
        if kind == Kind::Para && self.selection().is_none() {
            let mut i = self.caret.line;
            let caret_byte = self.caret.byte;
            let no_gap = |gaps: &HashMap<L::Id, bool>, l: &L| !gaps.get(&l.id).copied().unwrap_or(true);
            if i > 0 && self.lines[i - 1].kind == Kind::Para && self.lines[i - 1].depth == 0 && no_gap(&gaps, &self.lines[i]) {
                let at = self.lines[i - 1].text.len() + 1;
                self.lines[i - 1].text.push('\n');
                let gone = self.lines.remove(i);
                self.lines[i - 1].text.push_str(&gone.text);
                if !gone.is_new() {
                    self.deleted.push(gone.id.clone());
                }
                i -= 1;
                self.caret = Pos { line: i, byte: at + caret_byte };
            }
            if i + 1 < self.lines.len() && self.lines[i + 1].kind == Kind::Para && self.lines[i + 1].depth == 0 && no_gap(&gaps, &self.lines[i + 1]) {
                self.lines[i].text.push('\n');
                let gone = self.lines.remove(i + 1);
                self.lines[i].text.push_str(&gone.text);
                if !gone.is_new() {
                    self.deleted.push(gone.id.clone());
                }
            }
        }
        what
    }

    /// ⌃T inside a multi-line paragraph: the caret's line (or each selected line) becomes its
    /// own task; the lines above and below stay paragraph pieces. The piece holding the first
    /// line keeps the block's id; the new pieces sit right against it (no blank line).
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
                let mut n = self.fresh(0, Kind::Para, t);
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
        let lower_block: Vec<L> = self.lines.drain(lower.0..lower.1).collect();
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
        // The undo step is taken with the caret where it was (undo puts it back there).
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
        // With a selection, only the selection goes (line deletes never extend it).
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
                let mut l = self.fresh(depth, kind, &text);
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
pub fn numbered(text: &str) -> Option<u64> {
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


#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, PartialEq)]
    struct L(Block<usize>);
    impl std::ops::Deref for L {
        type Target = Block<usize>;
        fn deref(&self) -> &Block<usize> {
            &self.0
        }
    }
    impl std::ops::DerefMut for L {
        fn deref_mut(&mut self) -> &mut Block<usize> {
            &mut self.0
        }
    }
    impl BlockLine for L {
        type Id = usize;
        fn fresh(depth: usize, kind: Kind, text: &str) -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1_000_000);
            L(Block::new(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed), depth, kind, text))
        }
        fn is_new(&self) -> bool {
            false
        }
        fn keep_host_state(&mut self, _: &Self) {}
        fn revive(&mut self) {}
    }

    /// Undo steps share the lines they didn't touch (jank: undo copied the whole document).
    #[test]
    fn undo_steps_share_untouched_lines() {
        let mut b = Buffer::new((0..1000).map(|i| L(Block::new(i, 0, Kind::Bullet, &format!("line {i}")))).collect());
        let rev = b.content_rev();
        for i in 0..10 {
            b.caret = Pos { line: i, byte: b.lines[i].text.len() };
            b.insert("!");
        }
        assert_ne!(b.content_rev(), rev, "edits change the content revision");
        assert_eq!(b.undo.len(), 10);
        assert!(std::rc::Rc::ptr_eq(&b.undo[0].lines[500], &b.undo[9].lines[500]), "an untouched line is one allocation");
        assert!(!std::rc::Rc::ptr_eq(&b.undo[0].lines[3], &b.undo[9].lines[3]), "an edited one isn't");
        for _ in 0..10 {
            let rev = b.content_rev();
            assert!(b.undo());
            assert_ne!(b.content_rev(), rev, "and so does undo");
        }
        assert!(b.lines.iter().take(10).all(|l| !l.text.ends_with('!')), "undo restores every step");
        while b.redo() {}
        assert!(b.lines.iter().take(10).all(|l| l.text.ends_with('!')), "and redo replays them");
        let rev = b.content_rev();
        b.caret = Pos { line: 5, byte: 0 };
        b.select(true);
        assert_eq!(b.content_rev(), rev, "motion doesn't");
    }
}
