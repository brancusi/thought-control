//! Block marks: numeric ids pinned to line starts, mapped through every edit.
//!
//! A mark is a [`MarkId`] at a char position that is always the start of a line. Hosts use
//! marks as block identity: the outline layer ([`crate::outline`]) puts one on the first line
//! of every block, and an embedder keys its own data (a database row, a node id) by them.
//! The engine only ever hands out plain numbers (`MarkId(n)`, from a counter in [`Marks`]);
//! anything richer is the host's.
//!
//! **Mapping.** After every transaction each mark is mapped with [`Assoc::After`] and then
//! snapped to the start of its line. A mark is **removed** when the edit deleted the line
//! break in front of it (so its line joined the line above), with one refinement for pure
//! deletions of whole lines:
//!
//! - A deletion `[a, b)` that starts and ends at line starts (whole lines, nothing inserted
//!   in their place) removes the marks in `[a, b)`; a mark at `b` survives and moves up to
//!   `a`. Cutting whole lines therefore takes exactly their marks.
//! - Any other deletion removes the marks in `(a, b]`; a mark at `a` survives (typing over
//!   a selection that starts at a block's start keeps the block). A mark at `b` survives
//!   too when the text typed in the deletion's place ends with a line break (its line is
//!   still a line of its own).
//!
//! If two surviving marks land on one line, the one that was earlier keeps it and the
//! later one is removed. Every removal is reported, so undo can bring it back.
//!
//! **Undo and redo.** History revisions don't record marks, so each revision gets a
//! [`MarkDelta`]: what to fix up after mapping through the revision's inversion (undo) or
//! its transaction (redo). The fix-ups are computed when the revision is made, by comparing
//! the plain mapping with the marks the edit actually left, so undo and redo restore every
//! mark, id and attribute exactly.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::helix::{Assoc, ChangeSet, Operation, RopeSlice};

/// A block's identity: a plain number, unique within a document's lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MarkId(pub u64);

/// What a mark carries beside its position: attributes that live beside the text, never in
/// it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkAttrs {
    /// A blank row before the block: `Some(true)` always, `Some(false)` never, `None` the
    /// default for its kind and its neighbour's (see [`crate::outline`]). Drawn as a virtual
    /// row, never stored as text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<bool>,
    /// The host's payload: any JSON value. The engine never reads it; it travels with the
    /// mark through edits, cut and paste, undo and redo, changes from elsewhere and JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl MarkAttrs {
    /// Attributes with only a blank-row setting.
    pub fn gap(gap: Option<bool>) -> MarkAttrs {
        MarkAttrs { gap, data: None }
    }
}

impl MarkAttrs {
    pub fn is_default(&self) -> bool {
        *self == MarkAttrs::default()
    }
}

/// One mark: an id at a line start, with its attributes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    /// A char index; always the start of a line.
    pub pos: usize,
    pub id: MarkId,
    #[serde(default, skip_serializing_if = "MarkAttrs::is_default")]
    pub attrs: MarkAttrs,
}

impl Mark {
    pub fn new(pos: usize, id: MarkId) -> Mark {
        Mark { pos, id, attrs: MarkAttrs::default() }
    }
}

/// The marks of a document: sorted by position, at most one per line, ids unique.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marks {
    #[serde(default)]
    marks: Vec<Mark>,
    /// The next id [`Marks::mint`] hands out. Never decreases, so an id is never reused.
    #[serde(default)]
    next: u64,
}

/// Why [`Marks::insert`] refused a mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InsertError {
    /// Another mark already owns that line.
    LineTaken(MarkId),
    /// The id is already in use elsewhere.
    IdLive(usize),
}

impl Marks {
    pub fn new() -> Marks {
        Marks::default()
    }

    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    pub fn len(&self) -> usize {
        self.marks.len()
    }

    /// The marks in position order.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Mark> + ExactSizeIterator + '_ {
        self.marks.iter()
    }

    pub fn as_slice(&self) -> &[Mark] {
        &self.marks
    }

    /// The id the next [`Marks::mint`] returns.
    pub fn next_id(&self) -> MarkId {
        MarkId(self.next)
    }

    fn index_of(&self, pos: usize) -> Result<usize, usize> {
        self.marks.binary_search_by(|m| m.pos.cmp(&pos))
    }

    /// The mark owning the line that starts at `pos`.
    pub fn at(&self, pos: usize) -> Option<MarkId> {
        self.index_of(pos).ok().map(|i| self.marks[i].id)
    }

    /// The mark at `pos`, with its attributes.
    pub fn mark_at(&self, pos: usize) -> Option<&Mark> {
        self.index_of(pos).ok().map(|i| &self.marks[i])
    }

    /// The marks with `from <= pos < to`.
    pub fn in_range(&self, from: usize, to: usize) -> &[Mark] {
        let a = self.marks.partition_point(|m| m.pos < from);
        let b = self.marks.partition_point(|m| m.pos < to);
        &self.marks[a..b.max(a)]
    }

    /// The last mark at or before `pos`.
    pub fn at_or_before(&self, pos: usize) -> Option<&Mark> {
        let i = self.marks.partition_point(|m| m.pos <= pos);
        i.checked_sub(1).map(|i| &self.marks[i])
    }

    pub fn get(&self, id: MarkId) -> Option<&Mark> {
        self.marks.iter().find(|m| m.id == id)
    }

    pub fn pos(&self, id: MarkId) -> Option<usize> {
        self.get(id).map(|m| m.pos)
    }

    pub fn contains(&self, id: MarkId) -> bool {
        self.get(id).is_some()
    }

    /// Puts a new mark at `pos` (a line start) and returns its id. If the line already
    /// has a mark, returns that one instead.
    pub fn mint(&mut self, pos: usize) -> MarkId {
        self.mint_with(pos, MarkAttrs::default())
    }

    /// As [`Marks::mint`], with attributes for a new mark.
    pub fn mint_with(&mut self, pos: usize, attrs: MarkAttrs) -> MarkId {
        match self.index_of(pos) {
            Ok(i) => self.marks[i].id,
            Err(i) => {
                let id = MarkId(self.next);
                self.next += 1;
                self.marks.insert(i, Mark { pos, id, attrs });
                id
            }
        }
    }

    /// Inserts a mark with a given id (a restored or carried mark).
    pub fn insert(&mut self, mark: Mark) -> Result<(), InsertError> {
        if let Some(p) = self.pos(mark.id) {
            return Err(InsertError::IdLive(p));
        }
        match self.index_of(mark.pos) {
            Ok(i) => Err(InsertError::LineTaken(self.marks[i].id)),
            Err(i) => {
                self.next = self.next.max(mark.id.0 + 1);
                self.marks.insert(i, mark);
                Ok(())
            }
        }
    }

    /// Many marks at once (a host loading a document): the same as [`Marks::insert`] of each
    /// in order, in about linear time when none is refused.
    pub fn insert_all(&mut self, marks: Vec<Mark>) -> Result<(), InsertError> {
        let mut all = self.marks.clone();
        all.extend(marks.iter().cloned());
        let mut ids = std::collections::HashSet::with_capacity(all.len());
        let unique_ids = all.iter().all(|m| ids.insert(m.id));
        all.sort_by_key(|m| m.pos);
        if unique_ids && all.windows(2).all(|w| w[0].pos != w[1].pos) {
            self.next = self.next.max(all.iter().map(|m| m.id.0 + 1).max().unwrap_or(0));
            self.marks = all;
            return Ok(());
        }
        // Something is refused: one at a time, to the refusal.
        for m in marks {
            self.insert(m)?;
        }
        Ok(())
    }

    /// Removes a mark; returns it as it was.
    pub fn remove(&mut self, id: MarkId) -> Option<Mark> {
        let i = self.marks.iter().position(|m| m.id == id)?;
        Some(self.marks.remove(i))
    }

    /// Removes the mark at `pos`, if any.
    pub fn remove_at(&mut self, pos: usize) -> Option<Mark> {
        let i = self.index_of(pos).ok()?;
        Some(self.marks.remove(i))
    }

    /// Removes every mark with `from <= pos < to`; returns them in order.
    pub fn remove_range(&mut self, from: usize, to: usize) -> Vec<Mark> {
        let a = self.marks.partition_point(|m| m.pos < from);
        let b = self.marks.partition_point(|m| m.pos < to).max(a);
        self.marks.drain(a..b).collect()
    }

    pub fn attrs(&self, id: MarkId) -> MarkAttrs {
        self.get(id).map(|m| m.attrs.clone()).unwrap_or_default()
    }

    /// Sets a mark's attributes; returns the old ones (`None` if there is no such mark).
    pub fn set_attrs(&mut self, id: MarkId, attrs: MarkAttrs) -> Option<MarkAttrs> {
        let m = self.marks.iter_mut().find(|m| m.id == id)?;
        Some(std::mem::replace(&mut m.attrs, attrs))
    }

    /// Sets a mark's blank-row attribute, keeping its payload; returns the old setting.
    pub fn set_gap(&mut self, id: MarkId, gap: Option<bool>) -> Option<Option<bool>> {
        let m = self.marks.iter_mut().find(|m| m.id == id)?;
        Some(std::mem::replace(&mut m.attrs.gap, gap))
    }

    /// Sets a mark's payload, keeping its other attributes; returns the old payload.
    pub fn set_data(&mut self, id: MarkId, data: Option<serde_json::Value>) -> Option<Option<serde_json::Value>> {
        let m = self.marks.iter_mut().find(|m| m.id == id)?;
        Some(std::mem::replace(&mut m.attrs.data, data))
    }

    /// Maps every mark through `changes`, which turned `old` into `new`. Returns the marks
    /// it removed, at their positions in `old` (see the module docs for the rules).
    pub fn map(&mut self, old: RopeSlice, new: RopeSlice, changes: &ChangeSet) -> Vec<Mark> {
        if self.marks.is_empty() {
            return Vec::new();
        }
        // Deletions in old positions (with whether text was inserted in their place), and
        // the old span the changes touch.
        let ops = changes.changes();
        let mut dels: Vec<(usize, usize, bool, bool)> = Vec::new();
        let mut at = 0usize;
        let mut first: Option<usize> = None;
        let mut last_end = 0usize;
        for (i, op) in ops.iter().enumerate() {
            match op {
                Operation::Retain(n) => at += n,
                Operation::Delete(n) => {
                    let ins = match (i.checked_sub(1).and_then(|j| ops.get(j)), ops.get(i + 1)) {
                        (Some(Operation::Insert(s)), _) | (_, Some(Operation::Insert(s))) => Some(s),
                        _ => None,
                    };
                    // What was typed in its place ends with a line break: the line after the
                    // deletion stays a line of its own.
                    let keeps_next = ins.is_some_and(|s| s.ends_with('\n'));
                    first.get_or_insert(at);
                    dels.push((at, at + n, ins.is_some(), keeps_next));
                    at += n;
                    last_end = at;
                }
                Operation::Insert(_) => {
                    first.get_or_insert(at);
                    last_end = at;
                }
            }
        }
        let Some(first) = first else { return Vec::new() };
        let delta = new.len_chars() as isize - old.len_chars() as isize;
        let start = self.marks.partition_point(|m| m.pos < first);
        let end = self.marks.partition_point(|m| m.pos <= last_end).max(start);

        let mut removed = Vec::new();
        let mut middle: Vec<Mark> = Vec::with_capacity(end - start);
        for m in &self.marks[start..end] {
            let gone = dels.iter().any(|&(a, b, replaced, keeps_next)| {
                let whole = !replaced && a < b && is_line_start(old, a) && is_line_start(old, b);
                if whole {
                    a <= m.pos && m.pos < b
                } else {
                    a < m.pos && (m.pos < b || (m.pos == b && !keeps_next))
                }
            });
            if gone {
                removed.push(m.clone());
            } else {
                middle.push(m.clone());
            }
        }
        let mut positions: Vec<usize> = middle.iter().map(|m| m.pos).collect();
        changes.update_positions(positions.iter_mut().map(|p| (p, Assoc::After)));

        let suffix: Vec<Mark> = self.marks[end..].to_vec();
        self.marks.truncate(start);
        for (m, p) in middle.into_iter().zip(positions) {
            let p = line_start_at(new, p);
            if self.marks.last().is_some_and(|last| last.pos == p) {
                removed.push(m);
                continue;
            }
            self.marks.push(Mark { pos: p, ..m });
        }
        for m in suffix {
            self.marks.push(Mark { pos: (m.pos as isize + delta) as usize, ..m });
        }
        removed.sort_by_key(|m| m.pos);
        removed
    }

    /// Drops what can't be right in `text`: marks past the end or not at a line start,
    /// a second mark on one line, a repeated id. Keeps the id counter past every id.
    pub fn repair(&mut self, text: RopeSlice) {
        let len = text.len_chars();
        if let Some(max) = self.marks.iter().map(|m| m.id.0).max() {
            self.next = self.next.max(max + 1);
        }
        let mut seen = HashSet::new();
        self.marks.sort_by_key(|m| m.pos);
        let mut last: Option<usize> = None;
        self.marks.retain(|m| {
            let ok = m.pos <= len && is_line_start(text, m.pos) && last != Some(m.pos) && seen.insert(m.id);
            if ok {
                last = Some(m.pos);
            }
            ok
        });
    }

    /// No marks, and no id ever handed out.
    pub fn is_unused(&self) -> bool {
        self.marks.is_empty() && self.next == 0
    }

    /// The fix-up that turns `self` (a plain mapping) into `target`. Both describe the same
    /// text.
    pub fn diff(&self, target: &Marks) -> Fixup {
        let (a, b) = (&self.marks, &target.marks);
        let pre = a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
        let suf = a[pre..].iter().rev().zip(b[pre..].iter().rev()).take_while(|(x, y)| x == y).count();
        let (am, bm) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
        if am.is_empty() && bm.is_empty() {
            return Fixup::default();
        }
        let mut naive: HashMap<MarkId, &Mark> = am.iter().map(|m| (m.id, m)).collect();
        let mut set = Vec::new();
        for m in bm {
            if naive.remove(&m.id) != Some(m) {
                set.push(m.clone());
            }
        }
        let mut drop: Vec<MarkId> = naive.into_keys().collect();
        drop.sort();
        Fixup { set, drop }
    }

    /// Applies a fix-up made by [`Marks::diff`].
    pub fn apply(&mut self, fixup: &Fixup) {
        if fixup.is_empty() {
            return;
        }
        let gone: HashSet<MarkId> = fixup.drop.iter().copied().chain(fixup.set.iter().map(|m| m.id)).collect();
        self.marks.retain(|m| !gone.contains(&m.id));
        for m in &fixup.set {
            match self.index_of(m.pos) {
                // A fix-up made for this text never collides; if a state was edited by hand,
                // the mark already there wins.
                Ok(_) => {}
                Err(i) => self.marks.insert(i, m.clone()),
            }
            self.next = self.next.max(m.id.0 + 1);
        }
    }
}

/// Whether `pos` starts a line of `text`.
pub fn is_line_start(text: RopeSlice, pos: usize) -> bool {
    pos == 0 || (pos <= text.len_chars() && text.char(pos - 1) == '\n')
}

/// The start of the line holding `pos`.
pub fn line_start_at(text: RopeSlice, pos: usize) -> usize {
    let pos = pos.min(text.len_chars());
    text.line_to_char(text.char_to_line(pos))
}

/// Marks to put back or move, and marks to drop: what turns a plain mapping into the exact
/// marks an edit left.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fixup {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub set: Vec<Mark>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drop: Vec<MarkId>,
}

impl Fixup {
    pub fn is_empty(&self) -> bool {
        self.set.is_empty() && self.drop.is_empty()
    }
}

/// What one history revision did to the marks: the fix-ups after mapping through its
/// inversion (`undo`) and its transaction (`redo`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkDelta {
    #[serde(default, skip_serializing_if = "Fixup::is_empty")]
    pub undo: Fixup,
    #[serde(default, skip_serializing_if = "Fixup::is_empty")]
    pub redo: Fixup,
}

impl MarkDelta {
    pub fn is_empty(&self) -> bool {
        self.undo.is_empty() && self.redo.is_empty()
    }
}

/// A mark carried in the clipboard register: its offset into the copied text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipMark {
    pub offset: usize,
    pub id: MarkId,
    #[serde(default, skip_serializing_if = "MarkAttrs::is_default")]
    pub attrs: MarkAttrs,
}

/// The clipboard register: the last copy or cut, with the marks a cut took (so pasting it
/// back re-creates the same ids). Serializes as a plain string when it carries no marks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Clipboard {
    /// The text as it was in the buffer.
    pub text: String,
    /// What went to the system clipboard (Markdown in an outline), when it differs from
    /// `text`. A paste of exactly this text is a paste of the register.
    pub external: Option<String>,
    pub marks: Vec<ClipMark>,
    /// Whole blocks of an outline (their lines with markers and indentation, ending in a line
    /// break): a paste puts them in as blocks of their own.
    pub blocks: bool,
}

impl Clipboard {
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Whether `text` (from the system clipboard) is this register's own copy.
    pub fn is_own(&self, text: &str) -> bool {
        !self.text.is_empty() && (self.external.as_deref().unwrap_or(&self.text) == text || self.text == text)
    }
}

impl From<&str> for Clipboard {
    fn from(text: &str) -> Clipboard {
        Clipboard { text: text.to_string(), external: None, marks: Vec::new(), blocks: false }
    }
}

impl From<String> for Clipboard {
    fn from(text: String) -> Clipboard {
        Clipboard { text, external: None, marks: Vec::new(), blocks: false }
    }
}

impl PartialEq<&str> for Clipboard {
    fn eq(&self, other: &&str) -> bool {
        self.text == *other
    }
}

impl PartialEq<str> for Clipboard {
    fn eq(&self, other: &str) -> bool {
        self.text == other
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum ClipboardRepr {
    Text(String),
    Full {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        external: Option<String>,
        #[serde(default)]
        marks: Vec<ClipMark>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        blocks: bool,
    },
}

impl Serialize for Clipboard {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.marks.is_empty() && self.external.is_none() && !self.blocks {
            ClipboardRepr::Text(self.text.clone()).serialize(s)
        } else {
            ClipboardRepr::Full { text: self.text.clone(), external: self.external.clone(), marks: self.marks.clone(), blocks: self.blocks }
                .serialize(s)
        }
    }
}

impl<'de> Deserialize<'de> for Clipboard {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Clipboard, D::Error> {
        Ok(match ClipboardRepr::deserialize(d)? {
            ClipboardRepr::Text(text) => Clipboard::from(text),
            ClipboardRepr::Full { text, external, marks, blocks } => Clipboard { text, external, marks, blocks },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helix::{Rope, Transaction};

    fn run(text: &str, marks: &[usize], change: (usize, usize, Option<&str>)) -> (String, Vec<(usize, u64)>, Vec<u64>) {
        let mut rope = Rope::from(text);
        let old = rope.clone();
        let mut m = Marks::new();
        for &p in marks {
            m.mint(p);
        }
        let txn = Transaction::change(&rope, std::iter::once((change.0, change.1, change.2.map(Into::into))));
        txn.apply(&mut rope);
        let removed = m.map(old.slice(..), rope.slice(..), txn.changes());
        (
            rope.to_string(),
            m.iter().map(|m| (m.pos, m.id.0)).collect(),
            removed.iter().map(|m| m.id.0).collect(),
        )
    }

    #[test]
    fn inserting_all_at_once_is_inserting_each() {
        let m = |pos: usize, id: u64| Mark { pos, id: MarkId(id), attrs: MarkAttrs::default() };
        for batch in [vec![m(10, 3), m(0, 1), m(5, 2)], vec![m(0, 1), m(5, 1)], vec![m(0, 1), m(0, 2)], vec![m(7, 9)]] {
            let (mut a, mut b) = (Marks::new(), Marks::new());
            a.insert(m(3, 7)).unwrap();
            b.insert(m(3, 7)).unwrap();
            let ra = a.insert_all(batch.clone());
            let rb = batch.iter().try_for_each(|x| b.insert(x.clone()));
            assert_eq!((ra, &a), (rb, &b), "{batch:?}");
            assert_eq!(a.next_id(), b.next_id());
        }
    }

    #[test]
    fn typing_at_a_line_start_keeps_the_mark_there() {
        assert_eq!(run("ab\ncd", &[0, 3], (3, 3, Some("x"))), ("ab\nxcd".into(), vec![(0, 0), (3, 1)], vec![]));
    }

    #[test]
    fn a_break_inserted_at_a_mark_moves_it_down() {
        assert_eq!(run("ab\ncd", &[0, 3], (3, 3, Some("\n"))), ("ab\n\ncd".into(), vec![(0, 0), (4, 1)], vec![]));
    }

    #[test]
    fn deleting_the_break_before_a_mark_removes_it() {
        assert_eq!(run("ab\ncd", &[0, 3], (2, 3, None)), ("abcd".into(), vec![(0, 0)], vec![1]));
    }

    #[test]
    fn deleting_whole_lines_takes_their_marks_and_keeps_the_next() {
        assert_eq!(
            run("ab\ncd\nef", &[0, 3, 6], (3, 6, None)),
            ("ab\nef".into(), vec![(0, 0), (3, 2)], vec![1])
        );
    }

    #[test]
    fn replacing_from_a_mark_keeps_it() {
        assert_eq!(
            run("ab\ncd\nef", &[0, 3, 6], (0, 4, Some("X"))),
            ("Xd\nef".into(), vec![(0, 0), (3, 2)], vec![1])
        );
    }

    #[test]
    fn the_counter_never_reuses_an_id() {
        let mut m = Marks::new();
        let a = m.mint(0);
        m.remove(a);
        assert_ne!(m.mint(0), a);
    }

    #[test]
    fn clipboard_serializes_as_a_string_without_marks() {
        let c = Clipboard::from("abc");
        assert_eq!(serde_json::to_string(&c).unwrap(), "\"abc\"");
        let back: Clipboard = serde_json::from_str("\"abc\"").unwrap();
        assert_eq!(back, c);
        let full = Clipboard { text: "a\nb".into(), external: None, marks: vec![ClipMark { offset: 2, id: MarkId(4), attrs: MarkAttrs::default() }], blocks: false };
        let json = serde_json::to_string(&full).unwrap();
        assert_eq!(serde_json::from_str::<Clipboard>(&json).unwrap(), full);
    }
}
