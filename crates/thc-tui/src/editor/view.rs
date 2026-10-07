//! The document on screen: caretline lays it out, scrolls it, draws it into a `Frame` and says
//! what a click hits. thc gives the view its geometry (the text column, rows it draws after a
//! note: a meta on its own row, an image) and styles what the frame shows; it keeps no layout
//! of its own.

use super::doc::Doc;
use super::engine::list_len;
use super::BlockPos;
use caretline as cn;
use cn::layout::{Layout, RowPos};
use cn::state::{Follow, Scroll};
use cn::view::{Hit, RowInfo};
use cn::{MarkId, OutlineLayout, Viewport};
use std::collections::BTreeMap;

/// Columns before the hang: the marks (`≠`, `◆`, `◌`, `▸`), the outline layout's gutter.
pub const MARKS: u16 = 2;
/// The hang: a task's box, a bullet, a number or heading marker.
pub const HANG: u16 = 4;
/// Columns each depth indents.
pub const INDENT: u16 = 4;
/// The narrowest a nested note's text wraps at.
pub const MIN_COLUMN: u16 = 20;
/// Rows of the previous screen a page keeps (motion.md §4).
const PAGE_CONTEXT: u16 = 2;
/// Rows kept between the caret and the view's edges.
const SCROLLOFF: u16 = 2;
/// Typewriter scrolling keeps the caret's row here, in percent of the height.
const TYPEWRITER: u8 = 45;

/// How the document is laid out in its view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewGeometry {
    /// The view's size: from the marks column to the right edge, and the rows it shows.
    pub width: u16,
    pub height: u16,
    /// The text column at depth 0 (each depth is `INDENT` narrower, never under `MIN_COLUMN`).
    pub column: u16,
    /// Rows drawn after a note (line index → rows): a meta on its own row, an image.
    pub extra_rows: Vec<(usize, u16)>,
    /// Typewriter scrolling: the caret's row stays near the middle.
    pub typewriter: bool,
}

/// One row of the drawn document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocRow {
    /// Text of line `line`: bytes `start..end` of its text (a first row starts after a marker
    /// drawn in the hang); `shown` is the first byte drawn (later in a code block scrolled
    /// sideways). `x` is the column its text starts at.
    Text { line: usize, start: usize, end: usize, shown: usize, first: bool, x: u16 },
    /// The blank row before line `line`.
    Gap { line: usize },
    /// Row `index` of the rows drawn after line `line`.
    Extra { line: usize, index: u16 },
    /// Below the document.
    Past,
}

/// The document drawn: its rows and the caret's cell.
pub struct DocFrame {
    pub rows: Vec<DocRow>,
    pub cursor: Option<(u16, u16)>,
}

/// What a click at a cell of the view hits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocHit {
    /// Text: where the caret goes.
    Text(BlockPos),
    /// A note's hang; `task_box` when it's the task's box. `row` is where the row starts.
    Hang { line: usize, task_box: bool, row: BlockPos },
    /// A note's marks column. `row` is where the row starts.
    Marks { line: usize, row: BlockPos },
}

impl Doc {
    /// Lay the document out in a view of this geometry. The view follows the caret (unless it
    /// was scrolled freely).
    pub fn set_view(&mut self, g: &ViewGeometry) {
        self.engine.flush();
        let lines = self.engine.lines();
        let extra_rows: BTreeMap<MarkId, u16> = g.extra_rows.iter().filter_map(|&(i, n)| Some((MarkId(lines.get(i)?.mark?), n))).filter(|(_, n)| *n > 0).collect();
        let layout = OutlineLayout { gutter: MARKS, indent: INDENT, hang: HANG, column: g.column.max(1), min_column: MIN_COLUMN, extra_rows, hang_glyphs: false };
        let st = self.engine.state_mut();
        let v = &mut st.view;
        v.viewport = Viewport { width: g.width.max(1), height: g.height.max(1) };
        v.layout = Some(layout);
        v.config.status_bar = false;
        v.config.scrolloff = SCROLLOFF;
        v.config.page_overlap = PAGE_CONTEXT;
        v.config.follow = if g.typewriter { Follow::Typewriter { percent: TYPEWRITER } } else { Follow::Margin };
        if v.free {
            cn::layout::clamp_scroll(st);
        } else {
            cn::layout::ensure_caret_visible(st);
        }
    }

    /// The end (a byte of its text) of each listed line's first row, as the view wraps it.
    pub fn first_row_ends(&self, lines: &[usize]) -> Vec<usize> {
        let st = self.engine.state();
        let layout = Layout::of(&st.doc, &st.view);
        let o = st.doc.blocks().expect("an outline document");
        let rope = &st.doc.text;
        lines
            .iter()
            .map(|&i| {
                let b = &o.blocks[i];
                let cs = b.start + list_len(b);
                let line = b.first_line;
                let lf = layout.line_format(line);
                let mut end = if b.line_count > 1 { rope.line_to_char(line + 1) - 1 } else { b.end };
                for gr in layout.formatter_at_row(RowPos { line, row: lf.before }) {
                    if gr.line_idx != line || gr.visual_pos.row > 0 {
                        if gr.line_idx == line {
                            end = gr.char_idx;
                        }
                        break;
                    }
                }
                rope.char_to_byte(end.max(cs)) - rope.char_to_byte(cs)
            })
            .collect()
    }

    /// The document drawn into its view.
    pub fn frame(&self) -> DocFrame {
        let st = self.engine.state();
        let frame = cn::view::render(&st.doc, &st.view);
        let o = st.doc.blocks().expect("an outline document");
        let rope = &st.doc.text;
        let line_of = |m: MarkId| o.index_of(m);
        let w = frame.width as usize;
        let rows = frame
            .rows
            .iter()
            .enumerate()
            .map(|(y, r)| match r {
                RowInfo::Text { block: Some(m), chars, first, x, .. } => match line_of(*m) {
                    Some(i) => {
                        let b = &o.blocks[i];
                        let cs = b.start + list_len(b);
                        let byte = |c: usize| rope.char_to_byte(c.clamp(cs, b.end.max(cs))) - rope.char_to_byte(cs);
                        // The first char a cell of the row shows.
                        let shown = frame.cells[y * w + (*x as usize).min(w)..(y + 1) * w].iter().find_map(|c| c.char_idx).map_or(chars.start, |c| c as usize);
                        DocRow::Text { line: i, start: byte(chars.start), end: byte(chars.end), shown: byte(shown), first: *first, x: *x }
                    }
                    None => DocRow::Past,
                },
                RowInfo::Gap { before } => line_of(*before).map_or(DocRow::Past, |line| DocRow::Gap { line }),
                RowInfo::Extra { block, index } => line_of(*block).map_or(DocRow::Past, |line| DocRow::Extra { line, index: *index }),
                _ => DocRow::Past,
            })
            .collect();
        DocFrame { rows, cursor: frame.cursor }
    }

    /// What a click at cell (`col`, `row`) of the view hits. A blank row, or one a note draws
    /// after itself, is the end of the nearest text row above it (motion.md §4); below the
    /// text, nothing.
    pub fn hit(&self, col: u16, row: u16) -> Option<DocHit> {
        let st = self.engine.state();
        let o = st.doc.blocks().expect("an outline document");
        let line_of = |m: MarkId| o.index_of(m);
        match cn::view::hit(&st.doc, &st.view, col, row) {
            Hit::Text { pos } => Some(DocHit::Text(self.engine.pos_of(pos))),
            Hit::Hang { block, deco } => {
                let line = line_of(block)?;
                let x = Layout::of(&st.doc, &st.view).line_format(o.blocks[line].first_line).x;
                // Only the box's own three cells (`[ ]`) are its button; the gap after it is
                // margin.
                let in_box = (col as usize) < x.saturating_sub(HANG as usize) + 3;
                let row = self.row_start(x, row, line);
                Some(DocHit::Hang { line, task_box: in_box && deco.as_deref() == Some(super::tasks::BOX), row })
            }
            Hit::Gutter { block, .. } => {
                let line = line_of(block)?;
                let x = Layout::of(&st.doc, &st.view).line_format(o.blocks[line].first_line).x;
                Some(DocHit::Marks { line, row: self.row_start(x, row, line) })
            }
            Hit::Gap { .. } | Hit::Extra { .. } => {
                // The nearest text row above, at its end.
                (0..row).rev().find_map(|r| match cn::view::hit(&st.doc, &st.view, u16::MAX - 1, r) {
                    Hit::Text { pos } => Some(DocHit::Text(self.engine.pos_of(pos))),
                    _ => None,
                })
            }
            Hit::Past => None,
        }
    }

    /// Where screen row `row` of note `line` starts: the text cell at column `x`.
    fn row_start(&self, x: usize, row: u16, line: usize) -> BlockPos {
        let st = self.engine.state();
        match cn::view::hit(&st.doc, &st.view, x.min(u16::MAX as usize) as u16, row) {
            Hit::Text { pos } => self.engine.pos_of(pos),
            _ => BlockPos { line, byte: 0 },
        }
    }

    /// How many rows the document lays out to, and the first row on screen.
    pub fn scroll_rows(&self) -> (usize, usize) {
        let st = self.engine.state();
        let layout = Layout::of(&st.doc, &st.view);
        let start = RowPos { line: layout.visible_at_or_after(0).unwrap_or(0), row: 0 };
        let total = layout.rows_between(start, layout.end(), usize::MAX).max(0) as usize + 1;
        let top = layout.rows_between(start, layout.top(&st.view.scroll), usize::MAX).max(0) as usize;
        (total, top)
    }

    /// The first row on screen, as a row of the whole document.
    pub fn scroll(&self) -> usize {
        self.scroll_rows().1
    }

    /// Scroll so row `row` of the document is the first on screen. `free`: the view stays
    /// there until the caret moves (the wheel, the scrollbar); else it follows the caret again.
    pub fn set_scroll(&mut self, row: usize, free: bool) {
        self.engine.flush();
        let st = self.engine.state_mut();
        let layout = Layout::of(&st.doc, &st.view);
        let start = RowPos { line: layout.visible_at_or_after(0).unwrap_or(0), row: 0 };
        let (top, _) = layout.step_rows(start, row.min(isize::MAX as usize) as isize);
        st.view.scroll = Scroll { line: top.line, row: top.row, col: 0 };
        st.view.free = free;
        if free {
            cn::layout::clamp_scroll(st);
        } else {
            cn::layout::ensure_caret_visible(st);
        }
    }

    /// The wheel: the view moves `rows` (negative: up) and stays there until the caret moves.
    pub fn scroll_view(&mut self, rows: isize) {
        self.run(cn::Msg::ScrollView { rows: rows.clamp(i32::MIN as isize, i32::MAX as isize) as i32 });
    }

    /// The view was scrolled freely and doesn't follow the caret.
    pub fn scroll_free(&self) -> bool {
        self.engine.state().view.free
    }

    /// The view follows the caret again.
    pub fn follow_caret(&mut self) {
        let st = self.engine.state_mut();
        if st.view.free {
            st.view.free = false;
            cn::layout::ensure_caret_visible(st);
        }
    }
}
