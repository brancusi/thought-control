//! Layout on top of Helix's `DocumentFormatter`: visual rows, the caret's place on screen,
//! and keeping it in view. Shared by `update` (motion, scrolling) and `view` (drawing).

use crate::helix::chars::char_is_line_ending;
use crate::helix::doc_formatter::{DocumentFormatter, TextFormat};
use crate::helix::position::char_idx_at_visual_block_offset;
use crate::helix::text_annotations::TextAnnotations;
use crate::helix::{visual_offset_from_block, Rope, RopeSlice};
use crate::state::{Config, Scroll, State};

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
}

impl Layout {
    pub fn new(state: &State) -> Self {
        Layout {
            rope: state.text.clone(),
            fmt: text_format(&state.config, state.viewport.width, true),
            annotations: TextAnnotations::default(),
        }
    }

    /// The same document laid out without soft wrap (for logical-line motion).
    pub fn unwrapped(state: &State) -> Self {
        Layout {
            rope: state.text.clone(),
            fmt: text_format(&state.config, state.viewport.width, false),
            annotations: TextAnnotations::default(),
        }
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

    /// The number of visual rows document line `line` takes.
    pub fn line_rows(&self, line: usize) -> usize {
        if !self.fmt.soft_wrap || self.fits_one_row(line) {
            return 1;
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
        let (p, _) = visual_offset_from_block(self.text(), pos, pos, &self.fmt, &self.annotations);
        (RowPos { line, row: p.row }, p.col)
    }

    /// The char position on visual row `at` closest to column `col` (Helix's rule: the
    /// grapheme covering the column, else the row's last grapheme).
    pub fn pos_at(&self, at: RowPos, col: usize) -> usize {
        let start = self.text().line_to_char(at.line);
        // Search within the one line so a row past its end can't spill into the next line.
        let end = if at.line < self.last_line() {
            self.text().line_to_char(at.line + 1)
        } else {
            self.text().len_chars()
        };
        let (pos, _) = char_idx_at_visual_block_offset(
            self.text(),
            start,
            at.row,
            col,
            &self.fmt,
            &self.annotations,
        );
        pos.min(end)
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
}
