//! The outline layer: blocks on top of the text, bounded by [marks](crate::marks).
//!
//! An outline document is still one text buffer. **One text line is one row**, and a block is
//! one or more lines:
//!
//! - **A block's first line** is `indent marker content`. `indent` is
//!   [`OutlineConfig::indent`] spaces per depth. The marker is `- ` (or `* `, `+ `) for a
//!   bullet, `- [c] ` for a task (`c` from the task vocabulary), `12. ` (or `12) `) for a
//!   numbered item, or nothing for a paragraph. A paragraph's first line may start with a
//!   heading (`# `, `## `, `### `), a quote (`> `) or a code fence (three backticks).
//! - **Every other line is a continuation** of the block above (a soft break inside it). A
//!   continuation holds plain content, without indentation.
//! - **Block starts** are: the first line, every line with a mark, and every line outside a
//!   fence that starts with a marker (after optional indentation). A start without a mark
//!   gets one in the same update, so typing `- ` at the start of a continuation line starts a
//!   new block there. Inside a fence (from an opening fence line to the closing one) no marker
//!   starts a block.
//! - **Block identity** is the mark on its first line. Node ids, due dates and anything else a
//!   host keeps per block are keyed by [`MarkId`].
//! - **The blank row before a block** is an attribute ([`BlockAttrs::gap`]), drawn as a
//!   virtual row and never stored as text. Unset, it follows the defaults in
//!   [`default_gap`].
//!
//! Shape (depth, kind, status) is a pure function of the text, the marks and the config:
//! [`derive`]. The rules that edit outlines (Enter, Backspace at block edges, Tab, the task
//! cycle, moving blocks, pasting Markdown) are in [`rules`]; Markdown in and out is in
//! [`markdown`].

pub mod markdown;
pub mod rules;

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::helix::{RopeSlice};
use crate::marks::{BlockAttrs, MarkId, Marks};
use crate::state::{Document, State};

/// One entry of a task vocabulary: the character between the brackets and its name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskMarker {
    pub ch: char,
    pub name: String,
}

/// How an outline document reads and edits its text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutlineConfig {
    /// Spaces per depth on a block's first line.
    pub indent: u8,
    /// The characters a task's box may hold (`- [c] `), with their names.
    pub task_markers: Vec<TaskMarker>,
    /// The task cycle (⌃T): text → `[cycle[0]]` → `[cycle[1]]` → text. `cycle[1]` is "done".
    pub cycle: [char; 2],
    /// A block whose whole content is one Markdown image (`![caption](path)`) is atomic: the
    /// caret never stops inside it, and reaching it selects it whole.
    pub atomic_images: bool,
    /// `12. ` and `12) ` start numbered items; Enter continues the number.
    pub numbered: bool,
}

impl Default for OutlineConfig {
    fn default() -> Self {
        let m = |ch: char, name: &str| TaskMarker { ch, name: name.into() };
        OutlineConfig {
            indent: 2,
            task_markers: vec![m(' ', "todo"), m('x', "done"), m('/', "doing"), m('w', "waiting"), m('-', "cancelled")],
            cycle: [' ', 'x'],
            atomic_images: true,
            numbered: true,
        }
    }
}

impl OutlineConfig {
    pub fn is_task_char(&self, c: char) -> bool {
        self.task_markers.iter().any(|m| m.ch == c)
    }

    pub fn indent_str(&self, depth: u16) -> String {
        " ".repeat(self.indent.max(1) as usize * depth as usize)
    }
}

/// What a block is, by its marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Para,
    Bullet,
    Task,
}

/// What a host draws in a block's hang (the gutter before its content).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hang {
    None,
    Bullet,
    Number(u32),
    Task(char),
    Heading(u8),
    Quote,
    Fence,
}

/// One block, derived from the text and the marks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockInfo {
    /// The mark on its first line.
    pub id: MarkId,
    /// The char index of its first line's start.
    pub start: usize,
    /// The char index of its content's end: the end of its last line, before the break.
    pub end: usize,
    pub first_line: usize,
    pub line_count: usize,
    pub depth: u16,
    pub kind: Kind,
    /// A task's box character.
    pub status: Option<char>,
    /// Chars of indentation and marker on the first line: never a caret stop.
    pub prefix_len: usize,
    /// Spaces of indentation on the first line.
    pub indent: usize,
    pub hang: Hang,
    /// A code fence (its lines are all continuations).
    pub fence: bool,
    /// One line whose whole content is an image: one unit for the caret.
    pub atomic: bool,
    /// Whether a blank row comes before it (its attribute, else the default).
    pub gap: bool,
    /// Its attributes as set (`gap` unset means the default).
    pub attrs: BlockAttrs,
}

impl BlockInfo {
    /// Where its content starts: after the indentation and marker.
    pub fn content_start(&self) -> usize {
        self.start + self.prefix_len
    }

    pub fn last_line(&self) -> usize {
        self.first_line + self.line_count - 1
    }

    /// A list item (bullet, numbered or task).
    pub fn is_item(&self) -> bool {
        self.kind != Kind::Para
    }

    /// A paragraph that is plain text: no heading, quote or fence.
    pub fn is_plain_para(&self) -> bool {
        self.kind == Kind::Para && self.hang == Hang::None && !self.fence
    }

    pub fn is_empty(&self) -> bool {
        self.content_start() >= self.end
    }
}

/// The blocks of a document, with the block of every line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outline {
    pub blocks: Vec<BlockInfo>,
    line_block: Vec<u32>,
}

impl Outline {
    /// The index of the block holding line `line`.
    pub fn index_of_line(&self, line: usize) -> usize {
        self.line_block.get(line).or(self.line_block.last()).map_or(0, |&b| b as usize)
    }

    pub fn block_of_line(&self, line: usize) -> &BlockInfo {
        &self.blocks[self.index_of_line(line)]
    }

    /// The index of the block holding char position `pos`.
    pub fn index_at(&self, text: RopeSlice, pos: usize) -> usize {
        self.index_of_line(text.char_to_line(pos.min(text.len_chars())))
    }

    pub fn block_at(&self, text: RopeSlice, pos: usize) -> &BlockInfo {
        &self.blocks[self.index_at(text, pos)]
    }

    pub fn index_of(&self, id: MarkId) -> Option<usize> {
        self.blocks.iter().position(|b| b.id == id)
    }

    pub fn get(&self, id: MarkId) -> Option<&BlockInfo> {
        self.blocks.iter().find(|b| b.id == id)
    }

    /// Whether a blank row is drawn before line `line` (only a gapped block's first line).
    pub fn gap_before_line(&self, line: usize) -> bool {
        let b = self.block_of_line(line);
        b.first_line == line && b.gap
    }

    /// The blocks a range `[from, to]` touches, as indices.
    pub fn indices_between(&self, text: RopeSlice, from: usize, to: usize) -> std::ops::RangeInclusive<usize> {
        self.index_at(text, from)..=self.index_at(text, to)
    }

    /// The block's subtree: it and the blocks after it that are deeper, as `[i, end)`.
    pub fn subtree_end(&self, i: usize) -> usize {
        let d = self.blocks[i].depth;
        let mut j = i + 1;
        while j < self.blocks.len() && self.blocks[j].depth > d {
            j += 1;
        }
        j
    }
}

/// A block's first-line prefix, parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Prefix {
    pub indent: usize,
    pub kind: Kind,
    pub status: Option<char>,
    pub hang: Hang,
    /// Chars of indentation plus marker.
    pub len: usize,
    /// A list, heading, quote or fence marker: the line starts a block wherever it is.
    pub marker: bool,
    pub fence: bool,
}

impl Prefix {
    fn fence_content() -> Prefix {
        Prefix { indent: 0, kind: Kind::Para, status: None, hang: Hang::None, len: 0, marker: false, fence: true }
    }
}

/// Parses the start of a line: indentation, then a marker.
pub(crate) fn parse_prefix(line: RopeSlice, cfg: &OutlineConfig) -> Prefix {
    let mut chars = line.chars();
    let mut indent = 0usize;
    let mut first = None;
    for c in chars.by_ref() {
        if c == ' ' {
            indent += 1;
        } else {
            first = Some(c);
            break;
        }
    }
    let mut head: Vec<char> = Vec::with_capacity(14);
    if let Some(c) = first {
        head.push(c);
        head.extend(chars.take(13));
    }
    parse_head(indent, &head, cfg)
}

pub(crate) fn parse_str(line: &str, cfg: &OutlineConfig) -> Prefix {
    let indent = line.chars().take_while(|&c| c == ' ').count();
    let head: Vec<char> = line.chars().skip(indent).take(14).collect();
    parse_head(indent, &head, cfg)
}

fn parse_head(indent: usize, h: &[char], cfg: &OutlineConfig) -> Prefix {
    let para = Prefix { indent, kind: Kind::Para, status: None, hang: Hang::None, len: indent, marker: false, fence: false };
    let at = |i: usize| h.get(i).copied();
    match (at(0), at(1)) {
        (Some('-' | '*' | '+'), Some(' ')) => {
            if at(2) == Some('[') && at(4) == Some(']') && at(5) == Some(' ') {
                if let Some(c) = at(3).filter(|&c| cfg.is_task_char(c)) {
                    return Prefix { kind: Kind::Task, status: Some(c), hang: Hang::Task(c), len: indent + 6, marker: true, ..para };
                }
            }
            return Prefix { kind: Kind::Bullet, hang: Hang::Bullet, len: indent + 2, marker: true, ..para };
        }
        (Some('#'), _) => {
            let n = h.iter().take_while(|&&c| c == '#').count();
            if (1..=3).contains(&n) && at(n) == Some(' ') {
                return Prefix { hang: Hang::Heading(n as u8), len: indent + n + 1, marker: true, ..para };
            }
        }
        (Some('>'), Some(' ')) => return Prefix { hang: Hang::Quote, len: indent + 2, marker: true, ..para },
        (Some('`'), Some('`')) if at(2) == Some('`') => {
            return Prefix { hang: Hang::Fence, marker: true, fence: true, ..para };
        }
        (Some(c), _) if c.is_ascii_digit() && cfg.numbered => {
            let n = h.iter().take_while(|c| c.is_ascii_digit()).count();
            if n <= 9 && matches!(at(n), Some('.' | ')')) && at(n + 1) == Some(' ') {
                let num: String = h[..n].iter().collect();
                let num = num.parse().unwrap_or(0);
                return Prefix { kind: Kind::Bullet, hang: Hang::Number(num), len: indent + n + 2, marker: true, ..para };
            }
        }
        _ => {}
    }
    para
}

fn starts_fence(line: RopeSlice) -> bool {
    let mut chars = line.chars().skip_while(|&c| c == ' ');
    chars.next() == Some('`') && chars.next() == Some('`') && chars.next() == Some('`')
}

fn line_ending_len(line: RopeSlice) -> usize {
    crate::helix::line_ending::get_line_ending(&line).map_or(0, |le| le.len_chars())
}

/// The blank row before `b` by default, given the block `a` before it: a paragraph keeps a
/// blank row on either side (an `##` or `###` heading only above it), list items stay tight,
/// and the first block has none.
pub fn default_gap(a: Option<&BlockInfo>, b: &BlockInfo) -> bool {
    let Some(a) = a else { return false };
    let para = |l: &BlockInfo| l.kind == Kind::Para;
    let sub = |l: &BlockInfo| matches!(l.hang, Hang::Heading(2 | 3));
    let heading = |l: &BlockInfo| matches!(l.hang, Hang::Heading(_));
    let after = para(a) && !sub(a);
    let before = para(b) && heading(b);
    after || before || (para(b) && !para(a))
}

/// Derives the blocks of `text` from its marks. Lines that start a block but have no mark
/// get [`MarkId`]`(u64::MAX)` (the update loop gives them a mark before anyone sees them).
pub fn derive(text: RopeSlice, marks: &Marks, cfg: &OutlineConfig) -> Outline {
    let mut blocks: Vec<BlockInfo> = Vec::new();
    let mut line_block: Vec<u32> = Vec::with_capacity(text.len_lines());
    let mut in_fence = false;
    let mut line_start = 0usize;
    let mut next_mark = marks.iter().peekable();
    for (i, line) in text.lines().enumerate() {
        let len = line.len_chars();
        let content_end = line_start + len - line_ending_len(line);
        while next_mark.peek().is_some_and(|m| m.pos < line_start) {
            next_mark.next();
        }
        let mark = next_mark.peek().filter(|m| m.pos == line_start).copied().copied();
        let prefix = if in_fence { None } else { Some(parse_prefix(line, cfg)) };
        let starts = i == 0 || mark.is_some() || prefix.is_some_and(|p| p.marker);
        let mut opened = false;
        if starts {
            let p = prefix.unwrap_or_else(Prefix::fence_content);
            let depth = (p.indent / cfg.indent.max(1) as usize) as u16;
            if p.hang == Hang::Fence {
                in_fence = true;
                opened = true;
            }
            blocks.push(BlockInfo {
                id: mark.map_or(MarkId(u64::MAX), |m| m.id),
                start: line_start,
                end: content_end,
                first_line: i,
                line_count: 1,
                depth,
                kind: p.kind,
                status: p.status,
                prefix_len: p.len.min(content_end - line_start),
                indent: p.indent,
                hang: p.hang,
                fence: p.fence,
                atomic: false,
                gap: false,
                attrs: mark.map(|m| m.attrs).unwrap_or_default(),
            });
        } else if let Some(b) = blocks.last_mut() {
            b.line_count += 1;
            b.end = content_end;
        }
        if in_fence && !opened && starts_fence(line) {
            in_fence = false;
        }
        line_block.push((blocks.len() - 1) as u32);
        line_start += len;
    }
    for i in 0..blocks.len() {
        let gap = {
            let (before, rest) = blocks.split_at(i);
            let b = &rest[0];
            match b.attrs.gap {
                Some(g) if i > 0 => g,
                _ => default_gap(before.last(), b),
            }
        };
        let b = &mut blocks[i];
        b.gap = gap;
        if cfg.atomic_images && b.line_count == 1 && !b.fence {
            b.atomic = is_image(text.slice(b.content_start()..b.end));
        }
    }
    Outline { blocks, line_block }
}

/// Whether `content` is exactly one Markdown image: `![caption](target)`.
pub fn is_image(content: RopeSlice) -> bool {
    let n = content.len_chars();
    if n < 5 || content.char(0) != '!' || content.char(1) != '[' || content.char(n - 1) != ')' {
        return false;
    }
    let s: String = content.chars().collect();
    match s.find("](") {
        Some(k) => !s[2..k].contains(']') && !s[k + 2..s.len() - 1].contains(')'),
        None => false,
    }
}

/// A memo of the state's outline: derived data, never serialized, equal to any other.
#[derive(Default)]
pub struct OutlineCache(Mutex<Option<Arc<Outline>>>);

impl Clone for OutlineCache {
    fn clone(&self) -> Self {
        OutlineCache(Mutex::new(self.0.lock().map(|g| g.clone()).unwrap_or(None)))
    }
}

impl PartialEq for OutlineCache {
    fn eq(&self, _: &OutlineCache) -> bool {
        true
    }
}

impl std::fmt::Debug for OutlineCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OutlineCache")
    }
}

impl OutlineCache {
    pub fn clear(&mut self) {
        *self.0.get_mut().unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn get(&self) -> Option<Arc<Outline>> {
        self.0.lock().ok().and_then(|g| g.clone())
    }

    fn put(&self, o: Arc<Outline>) {
        if let Ok(mut g) = self.0.lock() {
            *g = Some(o);
        }
    }
}

impl Document {
    /// The outline, when this is an outline document (`outline` is set). Derived from the
    /// text and the marks, and remembered until they change.
    pub fn blocks(&self) -> Option<Arc<Outline>> {
        let cfg = self.outline.as_ref()?;
        if let Some(o) = self.derived.get() {
            let lines = self.text.len_lines();
            if o.line_block.len() == lines && o.blocks.len() <= self.marks.len().max(1) + lines {
                return Some(o);
            }
        }
        let o = Arc::new(derive(self.text.slice(..), &self.marks, cfg));
        self.derived.put(o.clone());
        Some(o)
    }
}

impl State {
    /// The outline, when this is an outline document (see [`Document::blocks`]).
    pub fn blocks(&self) -> Option<Arc<Outline>> {
        self.doc.blocks()
    }

    /// Makes this an outline document: derives its blocks and gives every block a mark
    /// (outside the undo history).
    pub fn enable_outline(&mut self, cfg: OutlineConfig) {
        self.doc.outline = Some(cfg);
        self.doc.derived.clear();
        mint_missing(&mut self.doc);
        rules::normalize(self, &self.view.selection.clone(), &crate::Msg::Tick { now_ms: self.doc.now_ms });
    }

    /// Call after changing `text` or `marks` directly (not through `update`).
    pub fn outline_changed(&mut self) {
        self.doc.derived.clear();
        if self.doc.outline.is_some() {
            mint_missing(&mut self.doc);
            rules::normalize(self, &self.view.selection.clone(), &crate::Msg::Tick { now_ms: self.doc.now_ms });
        }
    }
}

/// Gives every block start without a mark a new one. Returns whether it added any.
pub(crate) fn mint_missing(doc: &mut Document) -> bool {
    let Some(o) = doc.blocks() else { return false };
    let missing: Vec<usize> = o.blocks.iter().filter(|b| b.id == MarkId(u64::MAX)).map(|b| b.start).collect();
    if missing.is_empty() {
        return false;
    }
    for pos in missing {
        doc.marks.mint(pos);
    }
    doc.derived.clear();
    true
}

/// A block's content as text: its lines after the prefix, joined with `\n`.
pub fn content(doc: &Document, id: MarkId) -> Option<String> {
    let o = doc.blocks()?;
    let b = o.get(id)?;
    Some(doc.text.slice(b.content_start()..b.end).to_string().replace("\r\n", "\n"))
}

/// A new block for [`crate::Msg::InsertBlocks`] and Markdown paste: its shape and content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewBlock {
    #[serde(default)]
    pub depth: u16,
    pub kind: Kind,
    /// A task's box character.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<char>,
    /// The content, with `\n` for soft breaks. A bullet whose text starts with `12. ` is a
    /// numbered item; a paragraph may start with a heading or quote marker.
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<bool>,
    /// An id to reuse (when it isn't in use); otherwise the block gets a new mark.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark: Option<MarkId>,
}

impl NewBlock {
    pub fn para(text: &str) -> NewBlock {
        NewBlock { depth: 0, kind: Kind::Para, status: None, text: text.into(), gap: None, mark: None }
    }

    /// The block's lines as buffer text (prefix on the first line, `\n` between lines).
    pub fn to_lines(&self, cfg: &OutlineConfig) -> String {
        let indent = if self.kind == Kind::Para { String::new() } else { cfg.indent_str(self.depth) };
        let marker = match self.kind {
            Kind::Task => format!("- [{}] ", self.status.unwrap_or(cfg.cycle[0])),
            Kind::Bullet if numbered_marker(&self.text).is_some() => String::new(),
            Kind::Bullet => "- ".into(),
            Kind::Para => String::new(),
        };
        format!("{indent}{marker}{}", self.text)
    }
}

/// The length of a `12. ` / `12) ` marker at the start of `text`.
pub(crate) fn numbered_marker(text: &str) -> Option<usize> {
    let n = text.chars().take_while(|c| c.is_ascii_digit()).count();
    let rest = &text[n..];
    (n > 0 && n <= 9 && (rest.starts_with(". ") || rest.starts_with(") "))).then_some(n + 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helix::Rope;

    fn kinds(text: &str) -> Vec<(usize, Kind, u16, usize)> {
        let rope = Rope::from(text);
        let o = derive(rope.slice(..), &Marks::new(), &OutlineConfig::default());
        o.blocks.iter().map(|b| (b.first_line, b.kind, b.depth, b.line_count)).collect()
    }

    #[test]
    fn markers_start_blocks_and_other_lines_continue_them() {
        assert_eq!(
            kinds("Intro\nmore\n- a\n  - [ ] b\nnote\n1. one\n# Head"),
            [
                (0, Kind::Para, 0, 2),
                (2, Kind::Bullet, 0, 1),
                (3, Kind::Task, 1, 2),
                (5, Kind::Bullet, 0, 1),
                (6, Kind::Para, 0, 1)
            ]
        );
    }

    #[test]
    fn nothing_starts_a_block_inside_a_fence() {
        assert_eq!(kinds("```\n- a\n```\n- b"), [(0, Kind::Para, 0, 3), (3, Kind::Bullet, 0, 1)]);
    }

    #[test]
    fn a_whole_line_image_is_atomic() {
        let rope = Rope::from("![shot](files/a.png)\n![x](y) and text");
        let o = derive(rope.slice(..), &Marks::new(), &OutlineConfig::default());
        assert_eq!(o.blocks.len(), 1, "the second line continues the first");
        assert!(!o.blocks[0].atomic, "two lines are not one image");
        let rope = Rope::from("![shot](files/a.png)");
        let o = derive(rope.slice(..), &Marks::new(), &OutlineConfig::default());
        assert!(o.blocks[0].atomic);
    }
}
