//! Editor instances: one [`Doc`] per document, any number of [`View`]s onto it
//! (engine-extraction.md §1a). The doc holds the blocks, identity and undo; a view holds where
//! you are in it (caret, selection, goal column, scroll, folds, the rect it draws into, whether
//! it may edit, whether it has focus). There's no global state: the host owns docs, views and
//! focus. Read-only is enforced here, so a host can't forget it. Two views on one doc share
//! the text, the undo and the save; an edit through one rebases the others, by block identity,
//! the same way a remote edit does.

use crate::buffer::{BlockLine, Buffer};
use crate::{Command, Motion, Outcome, Pos, Stop};
use std::collections::HashSet;

/// Where a view draws, in cells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

/// One place a document is shown.
#[derive(Clone, Debug)]
pub struct View<Id> {
    pub caret: Pos,
    pub anchor: Option<Pos>,
    pub goal: Option<usize>,
    /// The first layout row drawn.
    pub scroll: usize,
    /// Blocks whose children this view hides (another view of the same doc may not).
    pub folds: HashSet<Id>,
    pub rect: Rect,
    pub read_only: bool,
    pub focused: bool,
}

impl<Id> View<Id> {
    pub fn new(rect: Rect) -> Self {
        View { caret: Pos::default(), anchor: None, goal: None, scroll: 0, folds: HashSet::new(), rect, read_only: false, focused: false }
    }

    pub fn read_only(mut self, on: bool) -> Self {
        self.read_only = on;
        self
    }
}

/// A position that survives edits and serialization: a block by identity and a byte offset
/// in its text, on a grapheme boundary. `Pos` indices are working values; store anchors.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Anchor<Id> {
    pub id: Id,
    pub byte: usize,
}

/// Why a command wasn't run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The view may not edit (a panel, say): motion, selection and copy still work.
    ReadOnly,
}

/// What an edit changed, for rebasing the other views: the blocks as they were, by identity.
/// `None`: nothing to follow (a motion, or no other view to rebase), so a rebase only clamps.
pub struct Edit<Id> {
    before: Option<Vec<(Id, String)>>,
}

impl<Id> Edit<Id> {
    /// An edit that moves nothing.
    pub fn none() -> Self {
        Edit { before: None }
    }
}

/// The caret stops of a view as its host draws it (the host's own wrap, markers and folds), for
/// motion. Called only for motion commands, so typing never lays the document out.
pub type HostStops<'a, L> = &'a mut dyn FnMut(&[L], &View<<L as BlockLine>::Id>) -> Vec<Stop>;

/// A laid-out row of a view: the block, the bytes it draws and the cell its text starts at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub line: usize,
    pub start: usize,
    pub end: usize,
    pub x: usize,
}

/// A document: its blocks and undo, shared by every view of it.
pub struct Doc<L: BlockLine> {
    buf: Buffer<L>,
    rev: u64,
}

/// Cells a block's text is indented by: four per level, and four for its hang (marker, box).
pub fn indent<Id>(b: &crate::Block<Id>) -> usize {
    b.depth * 4 + 4
}

impl<L: BlockLine> Doc<L> {
    pub fn new(lines: Vec<L>) -> Self {
        Doc { buf: Buffer::new(lines), rev: 0 }
    }

    /// The engine reads the time from `clock` (typing runs that undo as one step, `changed_at`),
    /// never the wall clock: a host that replays its messages passes each message's time.
    pub fn with_clock(mut self, clock: impl Fn() -> std::time::Instant + Send + Sync + 'static) -> Self {
        self.buf.set_clock(std::sync::Arc::new(clock));
        self
    }

    /// New lines (Enter, a split, a paste, ⌃T in a paragraph) come from `mint(depth, kind, text)`,
    /// not `BlockLine::fresh`: a host that replays mints ids from the message, not at random.
    pub fn with_lines(mut self, mint: impl FnMut(usize, crate::Kind, &str) -> L + Send + 'static) -> Self {
        self.buf.set_mint(std::sync::Arc::new(std::sync::Mutex::new(mint)));
        self
    }

    /// When the document last changed, by the host's clock (a host's idle commit).
    pub fn changed_at(&self) -> Option<std::time::Instant> {
        self.buf.changed_at
    }

    /// The host committed what changed (saved it): `changed_at` starts over.
    pub fn mark_committed(&mut self) {
        self.buf.changed_at = None;
    }

    pub fn lines(&self) -> &[L] {
        &self.buf.lines
    }

    /// The blocks, for the host's own bookkeeping (what it saved, what it shows beside them).
    /// Views aren't rebased: a change to text or shape that other views must follow goes
    /// through [`Doc::external_edit`].
    pub fn lines_mut(&mut self) -> &mut Vec<L> {
        self.rev += 1;
        &mut self.buf.lines
    }

    /// The pending deletes, for the host's own bookkeeping (a line re-created under a new id
    /// leaves its old one to delete).
    pub fn deleted_mut(&mut self) -> &mut Vec<L::Id> {
        &mut self.buf.deleted
    }

    /// `view`'s selection, ordered (start, end), when it has one inside the doc.
    pub fn selection(&self, view: &View<L::Id>) -> Option<(Pos, Pos)> {
        let a = view.anchor?;
        let ok = |p: Pos| self.buf.lines.get(p.line).is_some_and(|l| p.byte <= l.text.len() && l.text.is_char_boundary(p.byte));
        (a != view.caret && ok(a) && ok(view.caret)).then(|| if a < view.caret { (a, view.caret) } else { (view.caret, a) })
    }

    /// Bumped by every change: a view drawn at an older rev re-lays out.
    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// The anchor for a position (None when the position isn't in the doc).
    pub fn anchor(&self, p: Pos) -> Option<Anchor<L::Id>> {
        let l = self.buf.lines.get(p.line)?;
        Some(Anchor { id: l.id.clone(), byte: self.clamp(p).byte })
    }

    /// Where an anchor is now: its block found by identity, the byte kept inside the text on
    /// a grapheme boundary. None when the block is gone.
    pub fn resolve(&self, a: &Anchor<L::Id>) -> Option<Pos> {
        let line = self.buf.lines.iter().position(|l| l.id == a.id)?;
        let t = &self.buf.lines[line].text;
        let mut byte = a.byte.min(t.len());
        while !t.is_char_boundary(byte) {
            byte -= 1;
        }
        // On a grapheme boundary, not just a char one.
        use unicode_segmentation::UnicodeSegmentation;
        let g = t.grapheme_indices(true).map(|(i, _)| i).take_while(|&i| i <= byte).last().unwrap_or(0);
        let byte = if byte == t.len() { byte } else { g };
        Some(Pos { line, byte })
    }

    /// Whether a blank line comes before block `i` (its `gap`, else its kind's default).
    pub fn effective_gap(&self, i: usize) -> bool {
        self.buf.effective_gap(i)
    }

    pub fn default_gap(&self, i: usize) -> bool {
        self.buf.default_gap(i)
    }

    /// Every block's blank line before it, by id.
    pub fn gaps(&self) -> std::collections::HashMap<L::Id, bool> {
        self.buf.gaps()
    }

    /// Set a block's gap (a host loading what it stored).
    pub fn set_gap(&mut self, i: usize, gap: Option<bool>) {
        if let Some(l) = self.buf.lines.get_mut(i) {
            l.gap = gap;
        }
    }

    pub fn undo_depth(&self) -> usize {
        self.buf.undo_depth()
    }

    /// Blocks removed by edits since the last commit, for the host to delete.
    pub fn take_deleted(&mut self) -> Vec<L::Id> {
        std::mem::take(&mut self.buf.deleted)
    }

    fn snapshot(&self) -> Edit<L::Id> {
        Edit { before: Some(self.buf.lines.iter().map(|l| (l.id.clone(), l.text.clone())).collect()) }
    }

    /// Blocks removed by edits and not committed yet (the host's next save deletes them).
    pub fn deleted(&self) -> &[L::Id] {
        &self.buf.deleted
    }

    fn load(&mut self, view: &View<L::Id>) {
        self.buf.caret = view.caret;
        self.buf.anchor = view.anchor;
        self.buf.goal_col = view.goal;
    }

    fn store(&self, view: &mut View<L::Id>) {
        view.caret = self.clamp(self.buf.caret);
        view.anchor = self.buf.anchor.map(|a| self.clamp(a));
        view.goal = self.buf.goal_col;
    }

    /// Run the buffer's own rules at `view`'s caret, for what [`Command`] doesn't name (a paste
    /// of parsed lines, a marker click, a recovery step). Refused on a read-only view; the
    /// returned [`Edit`] rebases the doc's other views.
    pub fn edit_at<R>(&mut self, view: &mut View<L::Id>, f: impl FnOnce(&mut Buffer<L>) -> R) -> Result<(R, Edit<L::Id>), Refused> {
        if view.read_only {
            return Err(Refused::ReadOnly);
        }
        let edit = self.snapshot();
        self.load(view);
        let r = f(&mut self.buf);
        self.store(view);
        self.rev += 1;
        Ok((r, edit))
    }

    /// The buffer at `view`'s caret, for what reads or moves without editing (the selection,
    /// a word or block selected). Works on a read-only view; editing here is a host bug.
    pub fn at<R>(&mut self, view: &mut View<L::Id>, f: impl FnOnce(&mut Buffer<L>) -> R) -> R {
        let rev = self.buf.content_rev();
        self.load(view);
        let r = f(&mut self.buf);
        self.store(view);
        debug_assert_eq!(rev, self.buf.content_rev(), "Doc::at edited the doc: use Doc::edit_at");
        r
    }

    /// Run `cmd` at `view`'s caret. Editing commands on a read-only view are refused and change
    /// nothing. The returned [`Edit`] rebases the doc's other views ([`Doc::rebase`]); hosts use
    /// [`Doc::apply_and_rebase`], which does both.
    pub fn apply(&mut self, view: &mut View<L::Id>, cmd: Command, width_of: &dyn Fn(&L) -> usize) -> Result<(Outcome, Edit<L::Id>), Refused> {
        let _ = width_of;
        self.apply_core(view, cmd, None, true)
    }

    /// [`Doc::apply`] with the host's layout for motion ([`HostStops`]).
    pub fn apply_with(&mut self, view: &mut View<L::Id>, cmd: Command, stops: HostStops<'_, L>) -> Result<(Outcome, Edit<L::Id>), Refused> {
        self.apply_core(view, cmd, Some(stops), true)
    }

    fn apply_core(&mut self, view: &mut View<L::Id>, cmd: Command, stops: Option<HostStops<'_, L>>, want_edit: bool) -> Result<(Outcome, Edit<L::Id>), Refused> {
        if view.read_only && edits(&cmd) {
            return Err(Refused::ReadOnly);
        }
        // (A motion moves no text: nothing for another view to follow.)
        let edit = if want_edit && edits(&cmd) { self.snapshot() } else { Edit::none() };
        self.load(view);
        // A kind or depth change (Tab, ⇧Tab, a marker deleted, ⌃T) never moves another line:
        // every block keeps the blank line it had. (Enter leaving a list isn't one of them: its
        // paragraph takes a paragraph's blank line.)
        // ⌫ / Delete run on every key: only the caret's neighbourhood, O(1). ⌃T and Tab may
        // touch a whole selection: every block, O(n), on a rare key. Motions check nothing.
        let out = match cmd {
            Command::Backspace | Command::Delete => {
                let before = self.buf.near_caret();
                let out = self.run(cmd, stops, view);
                self.buf.pin_near(&before);
                out
            }
            Command::Indent | Command::Outdent | Command::TaskCycle => {
                let gaps = self.buf.gaps();
                let out = self.run(cmd, stops, view);
                self.buf.pin_gaps(&gaps);
                out
            }
            _ => self.run(cmd, stops, view),
        };
        // (Undo puts back the caret of whoever made that step: inside this doc, on a boundary.)
        self.store(view);
        if edits(&cmd) {
            self.rev += 1;
        }
        Ok((out, edit))
    }

    /// Type text at `view`'s caret (over its selection).
    pub fn insert(&mut self, view: &mut View<L::Id>, text: &str) -> Result<Edit<L::Id>, Refused> {
        if view.read_only {
            return Err(Refused::ReadOnly);
        }
        let edit = self.snapshot();
        self.buf.caret = view.caret;
        self.buf.anchor = view.anchor;
        self.buf.insert(text);
        view.caret = self.buf.caret;
        view.anchor = self.buf.anchor;
        view.goal = None;
        self.rev += 1;
        Ok(edit)
    }

    /// What a copy takes from `view` (editing.md §5): inside one block its plain text; across
    /// blocks Markdown, the first with its marker only when the selection includes its start.
    /// Empty: nothing selected.
    pub fn copy(&mut self, view: &View<L::Id>) -> String {
        self.buf.caret = view.caret;
        self.buf.anchor = view.anchor;
        // (A selection starting at the very end of a block copies an empty first part: the line
        // break that says "a block boundary here". Pasting it back over the same selection then
        // changes nothing, EI8; dropping it would merge two blocks.)
        let parts = self.buf.selected_parts();
        let from_start = self.buf.selection().is_some_and(|(s, _)| s.byte == 0);
        match parts.len() {
            0 => String::new(),
            1 => parts[0].1.clone(),
            _ => {
                let blocks: Vec<(&crate::Block<L::Id>, &str, &str)> = parts.iter().map(|(i, t)| (&*self.buf.lines[*i], t.as_str(), self.buf.lines[*i].fields())).collect();
                let md = crate::markdown::to_markdown(&blocks);
                let md = md.strip_suffix('\n').unwrap_or(&md).to_string();
                if from_start || self.buf.lines[parts[0].0].kind == crate::Kind::Para {
                    return md;
                }
                let (first, rest) = md.split_once('\n').unwrap_or((&md, ""));
                let t = first.trim_start();
                let t = t.strip_prefix("- ").unwrap_or(t);
                let t = ["[ ] ", "[x] ", "[/] ", "[w] ", "[-] "].iter().find_map(|c| t.strip_prefix(c)).unwrap_or(t);
                format!("{t}\n{rest}")
            }
        }
    }

    /// Cut: what a copy takes, then the selection deleted.
    pub fn cut(&mut self, view: &mut View<L::Id>) -> Result<(String, Edit<L::Id>), Refused> {
        if view.read_only {
            return Err(Refused::ReadOnly);
        }
        let text = self.copy(view);
        let edit = self.snapshot();
        if !text.is_empty() {
            self.buf.delete_selection();
            view.caret = self.clamp(self.buf.caret);
            view.anchor = None;
            self.rev += 1;
        }
        Ok((text, edit))
    }

    /// Paste text at `view`'s caret: one line types in; Markdown lines become blocks (a list
    /// stays a list), one undo step. `plain`: as text, no Markdown read.
    pub fn paste(&mut self, view: &mut View<L::Id>, text: &str, plain: bool) -> Result<Edit<L::Id>, Refused> {
        if view.read_only {
            return Err(Refused::ReadOnly);
        }
        if !text.contains('\n') {
            return self.insert(view, text);
        }
        let edit = self.snapshot();
        self.buf.caret = view.caret;
        self.buf.anchor = view.anchor;
        let (lines, _) = crate::markdown::parse_paste(text, plain);
        self.buf.paste_lines(lines);
        view.caret = self.clamp(self.buf.caret);
        view.anchor = None;
        view.goal = None;
        self.rev += 1;
        Ok(edit)
    }

    /// The one write path a host needs: `cmd` through `acting`, then every other view of this
    /// doc rebased, so none can be forgotten.
    pub fn apply_and_rebase<'v>(&mut self, acting: &mut View<L::Id>, others: impl IntoIterator<Item = &'v mut View<L::Id>>, cmd: Command, width_of: &dyn Fn(&L) -> usize) -> Result<Outcome, Refused>
    where
        L::Id: 'v,
    {
        let _ = width_of;
        self.apply_and_rebase_core(acting, others, cmd, None)
    }

    /// [`Doc::apply_and_rebase`] with the host's layout for motion ([`HostStops`]).
    pub fn apply_and_rebase_with<'v>(&mut self, acting: &mut View<L::Id>, others: impl IntoIterator<Item = &'v mut View<L::Id>>, cmd: Command, stops: HostStops<'_, L>) -> Result<Outcome, Refused>
    where
        L::Id: 'v,
    {
        self.apply_and_rebase_core(acting, others, cmd, Some(stops))
    }

    fn apply_and_rebase_core<'v>(&mut self, acting: &mut View<L::Id>, others: impl IntoIterator<Item = &'v mut View<L::Id>>, cmd: Command, stops: Option<HostStops<'_, L>>) -> Result<Outcome, Refused>
    where
        L::Id: 'v,
    {
        // With no other view, nothing to rebase: skip the snapshot (a copy of every block's text,
        // on every key).
        let mut others = others.into_iter().peekable();
        let want = others.peek().is_some();
        let (out, edit) = self.apply_core(acting, cmd, stops, want)?;
        for v in others {
            self.rebase(v, &edit);
        }
        Ok(out)
    }

    /// Typing, with the same guarantee as [`Doc::apply_and_rebase`].
    pub fn insert_and_rebase<'v>(&mut self, acting: &mut View<L::Id>, others: impl IntoIterator<Item = &'v mut View<L::Id>>, text: &str) -> Result<(), Refused>
    where
        L::Id: 'v,
    {
        let edit = self.insert(acting, text)?;
        for v in others {
            self.rebase(v, &edit);
        }
        Ok(())
    }

    /// A change from elsewhere (sync, an agent): `f` edits the blocks; every view is then
    /// rebased with the returned [`Edit`], exactly as for a local edit.
    pub fn external_edit(&mut self, f: impl FnOnce(&mut Vec<L>)) -> Edit<L::Id> {
        let edit = self.snapshot();
        f(&mut self.buf.lines);
        self.rev += 1;
        edit
    }

    /// Move `view` to where it was in the doc before `edit`: each position follows its block by
    /// identity, and its byte follows the text around it. A block that's gone puts the caret at
    /// the end of the nearest block before it that's still there. Folds of gone blocks drop.
    pub fn rebase(&self, view: &mut View<L::Id>, edit: &Edit<L::Id>) {
        let map = |p: Pos| self.map_pos(p, edit);
        view.caret = map(view.caret);
        view.anchor = view.anchor.map(map).filter(|a| *a != view.caret);
        let ids: HashSet<&L::Id> = self.buf.lines.iter().map(|l| &l.id).collect();
        view.folds.retain(|id| ids.contains(id));
        let rows = self.layout(view).len();
        view.scroll = view.scroll.min(rows.saturating_sub(1));
    }

    fn map_pos(&self, p: Pos, edit: &Edit<L::Id>) -> Pos {
        let lines = &self.buf.lines;
        if lines.is_empty() {
            return Pos::default();
        }
        let find = |id: &L::Id| lines.iter().position(|l| &l.id == id);
        let Some(before) = &edit.before else { return self.clamp(p) };
        let Some((id, old)) = before.get(p.line) else { return self.clamp(Pos { line: lines.len() - 1, byte: usize::MAX }) };
        match find(id) {
            Some(i) => {
                let new = &lines[i].text;
                let pre = common_prefix(old, new);
                let suf = common_suffix(&old[pre..], &new[pre..]);
                let byte = if p.byte <= pre {
                    p.byte
                } else if p.byte >= old.len() - suf {
                    p.byte + new.len() - old.len()
                } else {
                    pre
                };
                self.clamp(Pos { line: i, byte })
            }
            None => {
                // Gone (joined into the block above, deleted): the nearest surviving block before.
                for (oid, _) in before[..p.line].iter().rev() {
                    if let Some(i) = find(oid) {
                        return self.clamp(Pos { line: i, byte: usize::MAX });
                    }
                }
                self.clamp(Pos { line: 0, byte: 0 })
            }
        }
    }

    /// A position inside the doc, on a char boundary.
    fn clamp(&self, p: Pos) -> Pos {
        let lines = &self.buf.lines;
        if lines.is_empty() {
            return Pos::default();
        }
        let line = p.line.min(lines.len() - 1);
        let t = &lines[line].text;
        let mut byte = p.byte.min(t.len());
        while !t.is_char_boundary(byte) {
            byte -= 1;
        }
        Pos { line, byte }
    }

    /// Whether a block is hidden in `view`: under a folded ancestor.
    fn hidden(&self, view: &View<L::Id>, i: usize) -> bool {
        let lines = &self.buf.lines;
        let mut depth = lines[i].depth;
        for j in (0..i).rev() {
            if lines[j].depth < depth {
                if view.folds.contains(&lines[j].id) {
                    return true;
                }
                depth = lines[j].depth;
                if depth == 0 {
                    break;
                }
            }
        }
        false
    }

    /// The rows `view` draws, wrapped to its rect's width (any size, 0×0 included).
    pub fn layout(&self, view: &View<L::Id>) -> Vec<Row> {
        let w = view.rect.width as usize;
        let mut rows = Vec::new();
        for (i, l) in self.buf.lines.iter().enumerate() {
            if self.hidden(view, i) {
                continue;
            }
            let x = indent(l).min(w.saturating_sub(1));
            let tw = w.saturating_sub(x).max(1);
            // (Word wrap keeps 8 columns at least; a narrower rect cuts between graphemes, so
            // every row still fits.)
            let wrapped = if tw >= 8 { crate::wrap(&l.text, tw) } else { hard_wrap(&l.text, tw) };
            for (s, e) in wrapped {
                rows.push(Row { line: i, start: s, end: e, x });
            }
        }
        rows
    }

    /// The layout row the caret is on.
    pub fn caret_row(&self, view: &View<L::Id>) -> Option<usize> {
        let rows = self.layout(view);
        let c = view.caret;
        rows.iter().rposition(|r| r.line == c.line && r.start <= c.byte)
    }

    /// Scroll `view` so its caret's row is inside its rect.
    pub fn scroll_to_caret(&self, view: &mut View<L::Id>) {
        let h = view.rect.height as usize;
        let Some(r) = self.caret_row(view) else { return };
        if h == 0 {
            view.scroll = r;
        } else if r < view.scroll {
            view.scroll = r;
        } else if r >= view.scroll + h {
            view.scroll = r + 1 - h;
        }
    }

    fn run(&mut self, cmd: Command, stops: Option<HostStops<'_, L>>, view: &View<L::Id>) -> Outcome {
        let b = &mut self.buf;
        match cmd {
            Command::Newline => b.newline(),
            Command::SoftBreak => b.soft_break(),
            Command::Backspace => b.backspace(),
            Command::Delete => b.delete_forward(),
            Command::DeleteWordBack => b.delete_word_back(),
            Command::KillToEnd => b.kill_to_end(),
            Command::KillToStart => b.kill_to_start(),
            Command::Indent => {
                if !b.nest(1) {
                    return Outcome::Nothing("paragraphs don't nest");
                }
            }
            Command::Outdent => {
                b.nest(-1);
            }
            Command::TaskCycle => {
                if b.task_cycle() == "done" {
                    return Outcome::Completed;
                }
            }
            Command::MoveLine(n) => {
                if let Err(m) = b.move_line(n) {
                    return Outcome::Nothing(m);
                }
            }
            Command::SelectAll => {
                if !b.lines.is_empty() {
                    let last = b.lines.len() - 1;
                    b.anchor = Some(Pos { line: 0, byte: 0 });
                    b.caret = Pos { line: last, byte: b.lines[last].text.len() };
                }
            }
            Command::Undo => return if b.undo() { Outcome::Restored } else { Outcome::Nothing("nothing to undo") },
            Command::Redo => return if b.redo() { Outcome::Restored } else { Outcome::Nothing("nothing to redo") },
            Command::Move { motion, select } => self.motion(motion, select, stops, view),
        }
        Outcome::Done
    }

    /// A caret motion over the view's own layout (its width, its folds).
    fn motion(&mut self, m: Motion, select: bool, stops: Option<HostStops<'_, L>>, view: &View<L::Id>) {
        let collapse = if select { None } else { self.buf.selection() };
        self.buf.select(select);
        if let Some((s, e)) = collapse {
            match m {
                Motion::Left => {
                    self.buf.caret = s;
                    self.buf.goal_col = None;
                    return;
                }
                Motion::Right => {
                    self.buf.caret = e;
                    self.buf.goal_col = None;
                    return;
                }
                Motion::Up | Motion::WordLeft | Motion::Home | Motion::DocStart | Motion::NoteUp => self.buf.caret = s,
                Motion::Page(n) if n < 0 => self.buf.caret = s,
                _ => self.buf.caret = e,
            }
            self.buf.goal_col = None;
        }
        let stops: Vec<Stop> = match stops {
            Some(f) => f(&self.buf.lines, view),
            None => self
                .layout(view)
                .chunk_by(|a, b| a.line == b.line)
                .flat_map(|rows| {
                    let i = rows[0].line;
                    let ranges: Vec<(usize, usize)> = rows.iter().map(|r| (r.start, r.end)).collect();
                    crate::note_stops(i, &self.buf.lines[i].text, &ranges, 0, rows[0].x)
                })
                .collect(),
        };
        if stops.is_empty() {
            return;
        }
        let l = crate::Layout { texts: self.buf.lines.iter().map(|l| l.text.as_str()).collect(), stops };
        let (caret, goal) = (l.valid(self.buf.caret), self.buf.goal_col);
        let (c, g) = match m {
            Motion::Up => l.vertical(caret, goal, -1),
            Motion::Down => l.vertical(caret, goal, 1),
            Motion::Page(n) => l.vertical(caret, goal, n),
            Motion::Left => (l.left(caret), None),
            Motion::Right => (l.right(caret), None),
            Motion::WordLeft => (l.word_left(caret), None),
            Motion::WordRight => (l.word_right(caret), None),
            Motion::Home => (l.home_end(caret, false), None),
            Motion::End => (l.home_end(caret, true), None),
            Motion::DocStart => (l.start(), None),
            Motion::DocEnd => (l.end(), None),
            Motion::NoteUp | Motion::NoteDown => (l.note_step(caret, m == Motion::NoteDown), None),
        };
        self.buf.caret = c;
        self.buf.goal_col = g;
    }
}

/// Whether a command changes the document (a read-only view refuses these).
pub fn edits(cmd: &Command) -> bool {
    !matches!(cmd, Command::Move { .. } | Command::SelectAll)
}

/// Rows of at most `w` columns, cut between graphemes; a soft break starts a row.
fn hard_wrap(text: &str, w: usize) -> Vec<(usize, usize)> {
    use unicode_segmentation::UnicodeSegmentation;
    let mut rows = Vec::new();
    let mut start = 0;
    let mut col = 0;
    for (i, g) in text.grapheme_indices(true) {
        if g == "\n" {
            rows.push((start, i));
            start = i + 1;
            col = 0;
            continue;
        }
        let cw = crate::gwidth(g).max(1);
        if col + cw > w && i > start {
            rows.push((start, i));
            start = i;
            col = 0;
        }
        col += cw;
    }
    rows.push((start, text.len()));
    rows
}

fn common_prefix(a: &str, b: &str) -> usize {
    let mut n = 0;
    for (x, y) in a.char_indices().zip(b.char_indices()) {
        if x.1 != y.1 {
            break;
        }
        n = x.0 + x.1.len_utf8();
    }
    n
}

fn common_suffix(a: &str, b: &str) -> usize {
    let mut n = 0;
    for (x, y) in a.chars().rev().zip(b.chars().rev()) {
        if x != y {
            break;
        }
        n += x.len_utf8();
    }
    n
}
