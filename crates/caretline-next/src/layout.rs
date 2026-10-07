//! Layout on top of Helix's `DocumentFormatter`: visual rows, the caret's place on screen,
//! and keeping it in view. Shared by `update` (motion, scrolling) and `view` (drawing).

use std::cell::RefCell;
use std::sync::Arc;

use crate::helix::chars::char_is_line_ending;
use crate::helix::doc_formatter::{DocumentFormatter, TextFormat};
use crate::helix::text_annotations::TextAnnotations;
use crate::helix::transaction::{ChangeSet, Operation};
use crate::helix::{Rope, RopeSlice};
use crate::outline::Outline;
use crate::state::{Config, Scroll, State};

/// Soft-wrapped lines at least this long remember where their rows start (see
/// [`WrapCache`]); shorter lines are laid out from their start every time.
pub const LONG_LINE_CHARS: usize = 256;

/// How many long lines the cache remembers.
const CACHED_LINES: usize = 4;

/// The first grapheme of a visual row: its char index and column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowStart {
    char_idx: usize,
    col: usize,
}

/// The text format fields that decide where rows wrap.
type FmtKey = (u16, u16, u16, u16);

fn fmt_key(fmt: &TextFormat) -> FmtKey {
    (fmt.viewport_width, fmt.tab_width, fmt.max_wrap, fmt.max_indent_retain)
}

/// The known row starts of one long line, in order. Row 0 is the line's start.
#[derive(Debug, Clone)]
struct LineRows {
    fmt: FmtKey,
    line: usize,
    rows: Vec<RowStart>,
    /// The line's indent level, as the formatter reports it once known.
    indent: Option<usize>,
    /// Every row is known.
    complete: bool,
    /// When complete: the char index just past the line's break, or `None` when the line
    /// runs to the end of the text.
    next_line: Option<usize>,
}

impl LineRows {
    fn start(&self) -> usize {
        self.rows[0].char_idx
    }
}

/// A memo of where the rows of long soft-wrapped lines start, kept in the [`State`] so that
/// laying out a long line near the caret costs about one row instead of the whole line.
/// Typing at the end of a long paragraph is then linear overall rather than quadratic.
///
/// It is derived data: it never changes what layout computes, is not serialized, and every
/// state compares equal whatever it holds. `update` prunes it on every text change
/// ([`WrapCache::edited`]); a state built any other way starts with it empty.
#[derive(Clone, Default)]
pub struct WrapCache(Vec<LineRows>);

impl PartialEq for WrapCache {
    fn eq(&self, _: &WrapCache) -> bool {
        true
    }
}

impl std::fmt::Debug for WrapCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WrapCache({} lines)", self.0.len())
    }
}

impl WrapCache {
    /// Drops what `changes` (applied to the text the cache describes) may have moved. Row
    /// starts well before the first changed char stay: a row's start depends only on the
    /// text before it and on the row after it.
    pub fn edited(&mut self, changes: &ChangeSet) {
        let mut first = 0usize;
        let mut changed = false;
        for op in changes.changes() {
            match op {
                Operation::Retain(n) => first += n,
                _ => {
                    changed = true;
                    break;
                }
            }
        }
        if !changed {
            return;
        }
        self.0.retain_mut(|e| {
            if e.start() >= first {
                return false;
            }
            if e.complete && e.next_line.is_some_and(|next| first >= next) {
                return true;
            }
            let before = e.rows.iter().take_while(|r| r.char_idx < first).count();
            e.rows.truncate(before.saturating_sub(2).max(1));
            if e.rows.len() == 1 {
                // The indent comes from the first row, which may have changed.
                e.indent = None;
            }
            e.complete = false;
            e.next_line = None;
            true
        });
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }
}

/// What a lookup needs known about a line's rows.
#[derive(Debug, Clone, Copy)]
enum Need {
    /// The row holding this char.
    Pos(usize),
    /// The start of this row.
    Row(usize),
    /// Every row.
    All,
}

impl Need {
    fn met(self, e: &LineRows) -> bool {
        e.complete
            || match self {
                Need::Pos(p) => e.rows.last().is_some_and(|r| r.char_idx > p),
                Need::Row(r) => e.rows.len() > r,
                Need::All => false,
            }
    }

    /// The known row to start formatting from.
    fn row(self, e: &LineRows) -> usize {
        match self {
            Need::Pos(p) => e.rows.iter().rposition(|r| r.char_idx <= p).unwrap_or(0),
            Need::Row(r) => r.min(e.rows.len() - 1),
            Need::All => e.rows.len() - 1,
        }
    }
}

/// A place in the layout: a document line and a visual row within it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RowPos {
    pub line: usize,
    pub row: usize,
}

/// The text format for a viewport width. Mirrors how Helix derives it: wrapping needs
/// more than 10 columns, and the wrap and indent limits scale with the width.
pub fn text_format(config: &Config, width: u16, wrap: bool) -> TextFormat {
    let width = width.max(1);
    TextFormat {
        soft_wrap: wrap && config.soft_wrap && width > 10,
        tab_width: config.tab_width.max(1),
        max_wrap: 20.min(width / 4),
        max_indent_retain: 40.min(width * 2 / 5),
        wrap_indicator: Box::from(""),
        wrap_indicator_highlight: None,
        viewport_width: width,
        soft_wrap_at_text_width: false,
    }
}

/// Layout context for one state. Holds its own (cheap, shared) copy of the rope so the
/// state can change while a layout of the old text is in use.
pub struct Layout {
    rope: Rope,
    pub fmt: TextFormat,
    pub annotations: TextAnnotations<'static>,
    /// A copy of the state's [`WrapCache`], extended as lookups need.
    cache: RefCell<WrapCache>,
    /// An outline document's blocks: a gapped block's first line has a virtual (blank) row
    /// before its text rows. Rows of a line count from that virtual row.
    outline: Option<Arc<Outline>>,
}

impl Layout {
    pub fn new(state: &State) -> Self {
        Layout {
            rope: state.text.clone(),
            fmt: text_format(&state.config, state.viewport.width, true),
            annotations: TextAnnotations::default(),
            cache: RefCell::new(state.wrap.clone()),
            outline: state.blocks(),
        }
    }

    /// The same document laid out without soft wrap (for logical-line motion).
    pub fn unwrapped(state: &State) -> Self {
        Layout {
            rope: state.text.clone(),
            fmt: text_format(&state.config, state.viewport.width, false),
            annotations: TextAnnotations::default(),
            cache: RefCell::new(WrapCache::default()),
            outline: state.blocks(),
        }
    }

    /// Virtual rows before line `line`'s text: 1 for a gapped block's first line, else 0.
    pub fn gap(&self, line: usize) -> usize {
        match &self.outline {
            Some(o) => o.gap_before_line(line) as usize,
            None => 0,
        }
    }

    /// Whether `at` is a virtual row (never a caret stop).
    pub fn is_virtual(&self, at: RowPos) -> bool {
        at.row < self.gap(at.line)
    }

    /// Hands what this layout learned about long lines back to `state`, whose text must be
    /// the text this layout was made from.
    pub fn store(self, state: &mut State) {
        state.wrap = self.cache.into_inner();
    }

    /// Whether line `line` is laid out through the [`WrapCache`].
    fn is_long(&self, line: usize) -> bool {
        if !self.fmt.soft_wrap {
            return false;
        }
        let text = self.text();
        let start = text.line_to_char(line);
        let end = if line < self.last_line() { text.line_to_char(line + 1) } else { text.len_chars() };
        end - start >= LONG_LINE_CHARS
    }

    /// Makes sure the cache knows what `need` asks of long line `line`, and returns the row
    /// to start formatting from with its start and the line's indent level.
    fn known_row(&self, line: usize, need: Need) -> (usize, RowStart, Option<usize>) {
        let key = fmt_key(&self.fmt);
        let start = self.text().line_to_char(line);
        let mut cache = self.cache.borrow_mut();
        let entries = &mut cache.0;
        let text = self.text();
        let next_line = (line < self.last_line()).then(|| text.line_to_char(line + 1));
        // A cheap consistency check: the line still starts (and, when known, ends) where the
        // entry says.
        let valid = |e: &LineRows| {
            e.fmt == key && e.line == line && e.start() == start && (!e.complete || e.next_line == next_line)
        };
        let i = match entries.iter().position(valid) {
            Some(i) => i,
            None => {
                entries.retain(|e| !(e.fmt == key && e.line == line));
                if entries.len() >= CACHED_LINES {
                    entries.remove(0);
                }
                entries.push(LineRows {
                    fmt: key,
                    line,
                    rows: vec![RowStart { char_idx: start, col: 0 }],
                    indent: None,
                    complete: false,
                    next_line: None,
                });
                entries.len() - 1
            }
        };
        let e = &mut entries[i];
        if !need.met(e) {
            self.extend(e, need);
        }
        let row = need.row(e);
        (row, e.rows[row], e.indent)
    }

    /// Formats `e` on from its last known row until `need` is met.
    fn extend(&self, e: &mut LineRows, need: Need) {
        let last = e.rows.len() - 1;
        let mut formatter = self.formatter_from(e.line, last, e.rows[last], e.indent);
        while let Some(g) = formatter.next() {
            if g.line_idx != e.line {
                e.complete = true;
                e.next_line = Some(g.char_idx);
                return;
            }
            if e.indent.is_none() {
                e.indent = formatter.indent_level();
            }
            if g.visual_pos.row == e.rows.len() {
                e.rows.push(RowStart { char_idx: g.char_idx, col: g.visual_pos.col });
                if need.met(e) {
                    return;
                }
            }
        }
        e.complete = true;
        e.next_line = None;
    }

    fn formatter_from(&self, line: usize, row: usize, at: RowStart, indent: Option<usize>) -> DocumentFormatter<'_> {
        if row == 0 {
            DocumentFormatter::new_at_prev_checkpoint(self.text(), &self.fmt, &self.annotations, at.char_idx)
        } else {
            DocumentFormatter::resume_at_row(
                self.text(),
                &self.fmt,
                &self.annotations,
                at.char_idx,
                line,
                row,
                at.col,
                indent,
            )
        }
    }

    /// A formatter for `line` that starts at its row `need` names (or an earlier one).
    fn formatter_for(&self, line: usize, need: Need) -> DocumentFormatter<'_> {
        if self.is_long(line) {
            let (row, at, indent) = self.known_row(line, need);
            self.formatter_from(line, row, at, indent)
        } else {
            let start = self.text().line_to_char(line);
            DocumentFormatter::new_at_prev_checkpoint(self.text(), &self.fmt, &self.annotations, start)
        }
    }

    /// A formatter that starts at or before visual row `at` and yields every grapheme from
    /// there on, with rows numbered from the start of `at.line`.
    pub fn formatter_at_row(&self, at: RowPos) -> DocumentFormatter<'_> {
        let row = at.row.saturating_sub(self.gap(at.line));
        self.formatter_for(at.line, Need::Row(row))
    }

    pub fn text(&self) -> RopeSlice<'_> {
        self.rope.slice(..)
    }

    pub fn wraps(&self) -> bool {
        self.fmt.soft_wrap
    }

    pub fn last_line(&self) -> usize {
        self.text().len_lines().saturating_sub(1)
    }

    /// The number of visual rows document line `line` takes (its virtual row included).
    pub fn line_rows(&self, line: usize) -> usize {
        self.gap(line) + self.text_rows_of(line)
    }

    /// The rows line `line`'s text takes.
    fn text_rows_of(&self, line: usize) -> usize {
        if !self.fmt.soft_wrap || self.fits_one_row(line) {
            return 1;
        }
        if self.is_long(line) {
            self.known_row(line, Need::All);
            let cache = self.cache.borrow();
            let key = fmt_key(&self.fmt);
            if let Some(e) = cache.0.iter().find(|e| e.fmt == key && e.line == line) {
                return e.rows.len();
            }
        }
        self.formatted_rows(line)
    }

    /// The rows the formatter gives line `line`.
    fn formatted_rows(&self, line: usize) -> usize {
        let start = self.text().line_to_char(line);
        let formatter =
            DocumentFormatter::new_at_prev_checkpoint(self.text(), &self.fmt, &self.annotations, start);
        let mut rows = 1;
        for g in formatter {
            if g.line_idx != line {
                break;
            }
            rows = g.visual_pos.row + 1;
        }
        rows
    }

    /// Whether a line surely fits on one row, by an upper bound on its width (every char
    /// at least one cell, a tab a full stop, plus a cell for the line break or the end of
    /// the text): the formatter only wraps once a row reaches the viewport width. Saves
    /// formatting the common short line.
    fn fits_one_row(&self, line: usize) -> bool {
        use unicode_width::UnicodeWidthChar;
        let width = self.fmt.viewport_width as usize;
        let tab = self.fmt.tab_width as usize;
        let mut sum = 1;
        for c in self.text().line(line).chars() {
            sum += match c {
                '\t' => tab,
                c if char_is_line_ending(c) => 0,
                c if c.is_ascii() => 1,
                c => c.width().unwrap_or(0).max(1),
            };
            if sum >= width {
                return false;
            }
        }
        true
    }

    /// The visual place of char position `pos`: its row position and column.
    pub fn pos_coords(&self, pos: usize) -> (RowPos, usize) {
        let pos = pos.min(self.text().len_chars());
        let line = self.text().char_to_line(pos);
        // As Helix's `visual_offset_from_block`, from the nearest known row.
        let mut formatter = self.formatter_for(line, Need::Pos(pos));
        let mut last = crate::helix::Position::default();
        while let Some(g) = formatter.next() {
            last = g.visual_pos;
            if formatter.next_char_pos() > pos {
                break;
            }
        }
        (RowPos { line, row: last.row + self.gap(line) }, last.col)
    }

    /// The char position on visual row `at` closest to column `col` (Helix's rule: the
    /// grapheme covering the column, else the row's last grapheme).
    pub fn pos_at(&self, at: RowPos, col: usize) -> usize {
        // A virtual row: the line's start.
        let gap = self.gap(at.line);
        if at.row < gap {
            return self.text().line_to_char(at.line);
        }
        let at = RowPos { line: at.line, row: at.row - gap };
        // Search within the one line so a row past its end can't spill into the next line.
        let end = if at.line < self.last_line() {
            self.text().line_to_char(at.line + 1)
        } else {
            self.text().len_chars()
        };
        self.char_at_row_col(at, col).min(end)
    }

    /// Helix's `char_idx_at_visual_block_offset` from the nearest known row: the grapheme
    /// covering `col` on row `at.row`, else that row's last grapheme.
    fn char_at_row_col(&self, at: RowPos, col: usize) -> usize {
        use std::cmp::Ordering;
        let mut formatter = self.formatter_for(at.line, Need::Row(at.row));
        let mut last_char_idx = formatter.next_char_pos();
        let mut found_non_virtual_on_row = false;
        for g in &mut formatter {
            match g.visual_pos.row.cmp(&at.row) {
                Ordering::Equal => {
                    if g.visual_pos.col + g.width() > col {
                        if !g.is_virtual() {
                            return g.char_idx;
                        } else if found_non_virtual_on_row {
                            return last_char_idx;
                        }
                    } else if !g.is_virtual() {
                        found_non_virtual_on_row = true;
                        last_char_idx = g.char_idx;
                    }
                }
                Ordering::Greater => return last_char_idx,
                Ordering::Less => {
                    if !g.is_virtual() {
                        last_char_idx = g.char_idx;
                    }
                }
            }
        }
        formatter.next_char_pos()
    }

    /// The row position `n` rows after (positive) or before (negative) `at`, clamped to
    /// the document. Returns the position and how many rows were actually moved.
    pub fn step_rows(&self, at: RowPos, n: isize) -> (RowPos, isize) {
        let mut pos = at;
        let mut moved: isize = 0;
        if n >= 0 {
            let mut left = n as usize;
            while left > 0 {
                let rows = self.line_rows(pos.line);
                if pos.row + left < rows {
                    pos.row += left;
                    moved += left as isize;
                    left = 0;
                } else if pos.line < self.last_line() {
                    let step = rows - pos.row;
                    left -= step;
                    moved += step as isize;
                    pos = RowPos { line: pos.line + 1, row: 0 };
                } else {
                    let step = rows - 1 - pos.row;
                    moved += step as isize;
                    pos.row = rows - 1;
                    left = 0;
                }
            }
        } else {
            let mut left = n.unsigned_abs();
            while left > 0 {
                if pos.row >= left {
                    pos.row -= left;
                    moved -= left as isize;
                    left = 0;
                } else if pos.line > 0 {
                    let step = pos.row + 1;
                    left -= step;
                    moved -= step as isize;
                    pos.line -= 1;
                    pos.row = self.line_rows(pos.line) - 1;
                } else {
                    moved -= pos.row as isize;
                    pos.row = 0;
                    left = 0;
                }
            }
        }
        (pos, moved)
    }

    /// Rows from `from` down to `to` (negative when `to` is above), counting no further
    /// than `limit` rows in either direction.
    pub fn rows_between(&self, from: RowPos, to: RowPos, limit: usize) -> isize {
        if to < from {
            return -1 - self.rows_between(to, from, limit);
        }
        let mut sum = 0usize;
        let mut line = from.line;
        let mut row = from.row;
        while line < to.line {
            sum += self.line_rows(line).saturating_sub(row);
            row = 0;
            line += 1;
            if sum > limit {
                return limit as isize + 1;
            }
        }
        (sum + to.row).saturating_sub(row) as isize
    }

    /// The top of the view as a row position, clamped to the document.
    pub fn top(&self, scroll: &Scroll) -> RowPos {
        let line = scroll.line.min(self.last_line());
        let row = scroll.row.min(self.line_rows(line) - 1);
        RowPos { line, row }
    }

    /// The char position under screen cell (`col`, `row`) of the text area.
    pub fn pos_at_screen(&self, scroll: &Scroll, col: u16, row: u16) -> usize {
        let top = self.top(scroll);
        let (at, moved) = self.step_rows(top, row as isize);
        if moved < row as isize {
            // Below the last row: the end of the document.
            return self.text().len_chars();
        }
        let col = col as usize + if self.wraps() { 0 } else { scroll.col };
        self.pos_at(at, col)
    }
}

/// Moves the view the least needed for the primary caret to sit in it, `scrolloff` rows
/// from the edges where possible.
pub fn ensure_caret_visible(state: &mut State) {
    let layout = Layout::new(state);
    let h = state.text_rows();
    let w = state.viewport.width as usize;
    let (caret, col) = layout.pos_coords(state.caret());
    let mut top = layout.top(&state.scroll);
    if h > 0 {
        let so = (state.config.scrolloff as usize).min((h - 1) / 2);
        let dist = layout.rows_between(top, caret, h + so);
        if dist < so as isize {
            top = layout.step_rows(caret, -(so as isize)).0;
        } else if dist > (h - 1 - so) as isize {
            top = layout.step_rows(caret, -((h - 1 - so) as isize)).0;
        }
        // Don't leave empty rows below the end of the document. Every line takes at least
        // one row, so this can only happen when the last line is fewer than `h` lines below
        // the top; skipping the walk otherwise keeps updates cheap in long documents.
        if layout.last_line().saturating_sub(top.line) < h {
            let end = layout.pos_coords(layout.text().len_chars()).0;
            let max_top = layout.step_rows(end, -(h as isize - 1)).0;
            if top > max_top && max_top <= caret {
                top = max_top;
            }
        }
    } else {
        top = caret;
    }
    let mut scroll_col = state.scroll.col;
    if layout.wraps() {
        scroll_col = 0;
    } else if col < scroll_col {
        scroll_col = col;
    } else if col >= scroll_col + w {
        scroll_col = col + 1 - w;
    }
    state.scroll = Scroll {
        line: top.line,
        row: top.row,
        col: scroll_col,
    };
    layout.store(state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Viewport;

    /// The short-line shortcut never claims one row for a line the formatter wraps.
    #[test]
    fn fits_one_row_agrees_with_the_formatter() {
        let pieces = ["a", "word ", "\t", "界", "🙂", "👨‍👩‍👧", "🇫🇷", "e\u{301}", "\u{1}", "  ", "long-unbroken-token"];
        let mut seed = 0x2545_f491_u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for _ in 0..3000 {
            let mut line = String::new();
            for _ in 0..(next() % 40) {
                line.push_str(pieces[next() as usize % pieces.len()]);
            }
            let text = format!("{line}\nnext\n");
            let width = 11 + (next() % 60) as u16;
            let state = State::new(&text, None, Viewport { width, height: 10 });
            let layout = Layout::new(&state);
            if layout.fits_one_row(0) {
                assert_eq!(layout.formatted_rows(0), 1, "{line:?} at width {width}");
            }
        }
    }

    fn xorshift(mut seed: u32) -> impl FnMut() -> u32 {
        move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        }
    }

    /// Layout straight from Helix's functions, with no cache: the reference.
    fn reference_coords(layout: &Layout, pos: usize) -> (RowPos, usize) {
        use crate::helix::visual_offset_from_block;
        let line = layout.text().char_to_line(pos);
        let (p, _) = visual_offset_from_block(layout.text(), pos, pos, &layout.fmt, &layout.annotations);
        (RowPos { line, row: p.row }, p.col)
    }

    fn reference_pos_at(layout: &Layout, at: RowPos, col: usize) -> usize {
        use crate::helix::position::char_idx_at_visual_block_offset;
        let start = layout.text().line_to_char(at.line);
        let end = if at.line < layout.last_line() {
            layout.text().line_to_char(at.line + 1)
        } else {
            layout.text().len_chars()
        };
        let (pos, _) =
            char_idx_at_visual_block_offset(layout.text(), start, at.row, col, &layout.fmt, &layout.annotations);
        pos.min(end)
    }

    /// `CARETLINE_WRAP_SEEDS` runs more seeds than the default.
    fn seeds() -> u32 {
        std::env::var("CARETLINE_WRAP_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(6)
    }

    /// Long lines laid out through the wrap cache, kept across random edits, agree with
    /// layout from scratch: the same states, frames, coordinates and row counts.
    #[test]
    fn wrap_cache_agrees_with_layout_from_scratch() {
        use crate::msg::{By, Dir, Msg};
        use crate::update::update;
        use crate::view::view;
        let pieces = [
            "a", "word ", "words and more ", "\t", "界", "🙂", "e\u{301}", "  ", "long-unbroken-token-that-goes-on",
            "x", "x", "x", " ",
        ];
        for seed in 1..=seeds() {
            let mut next = xorshift(0x9e37_79b9 ^ seed.wrapping_mul(0x85eb_ca6b));
            let mut text = String::new();
            for l in 0..(1 + next() % 3) {
                if next().is_multiple_of(2) {
                    text.push_str(["", "  ", "\t", "        "][next() as usize % 4]);
                }
                let n = if l == 0 || next().is_multiple_of(2) { 300 + next() % 600 } else { next() % 20 };
                for _ in 0..n {
                    text.push_str(pieces[next() as usize % pieces.len()]);
                }
                text.push('\n');
            }
            let width = [11, 17, 40, 80, 8][next() as usize % 5];
            let mut warm = State::new(&text, None, Viewport { width, height: 12 });
            for step in 0..300u32 {
                let msg = match next() % 16 {
                    0..=4 => Msg::InsertText { text: pieces[next() as usize % pieces.len()].to_string() },
                    5 => Msg::InsertNewline,
                    6 | 7 => Msg::DeleteBackward,
                    8 => Msg::DeleteForward,
                    9 => Msg::Undo,
                    10 => Msg::Redo,
                    11 => Msg::Move {
                        dir: if next().is_multiple_of(2) { Dir::Forward } else { Dir::Backward },
                        by: [By::VisualLine, By::Page, By::LineEnd, By::LineStart, By::Word, By::DocEnd][next() as usize % 6],
                        extend: next().is_multiple_of(4),
                    },
                    12 => Msg::Click { col: (next() % 90) as u16, row: (next() % 12) as u16, extend: false },
                    13 => Msg::Scroll { rows: (next() % 21) as i32 - 10 },
                    14 => Msg::Resize { width: [11, 17, 40, 80, 8][next() as usize % 5], height: 12 },
                    _ => Msg::Move { dir: Dir::Backward, by: By::Grapheme, extend: false },
                };
                let mut cold = warm.clone();
                cold.wrap.clear();
                update(&mut warm, msg.clone());
                update(&mut cold, msg.clone());
                let ctx = format!("seed {seed} step {step} {msg:?}");
                assert_eq!(warm, cold, "{ctx}: state");
                assert_eq!(warm.scroll, cold.scroll, "{ctx}: scroll");
                assert_eq!(view(&warm), view(&cold), "{ctx}: frame");
                if step.is_multiple_of(8) {
                    let layout = Layout::new(&warm);
                    let len = layout.text().len_chars();
                    for _ in 0..6 {
                        let pos = next() as usize % (len + 1);
                        let pos = crate::helix::graphemes::ensure_grapheme_boundary_prev(layout.text(), pos);
                        let at = layout.pos_coords(pos);
                        assert_eq!(at, reference_coords(&layout, pos), "{ctx}: coords of {pos}");
                        let col = next() as usize % 90;
                        assert_eq!(layout.pos_at(at.0, col), reference_pos_at(&layout, at.0, col), "{ctx}: pos_at");
                    }
                    for line in 0..=layout.last_line() {
                        let rows = if !layout.fmt.soft_wrap { 1 } else { layout.formatted_rows(line) };
                        assert_eq!(layout.line_rows(line), rows, "{ctx}: rows of line {line}");
                    }
                }
            }
        }
    }
}
