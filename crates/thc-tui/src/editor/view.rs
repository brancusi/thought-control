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
use std::collections::{BTreeMap, HashMap};
use std::hash::{BuildHasher, Hash, Hasher};

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
        let mut layout = OutlineLayout::default();
        (layout.gutter, layout.indent, layout.hang, layout.column, layout.min_column, layout.extra_rows, layout.hang_glyphs) = (MARKS, INDENT, HANG, g.column.max(1), MIN_COLUMN, extra_rows, false);
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
        // (A text row names its document line: its block is a lookup, not a search.)
        let rows = frame
            .rows
            .iter()
            .enumerate()
            .map(|(y, r)| match r {
                RowInfo::Text { block: Some(_), line, chars, first, x, .. } => match Some(o.index_of_line(*line)) {
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

    /// How many rows the document lays out to, and the first row on screen. Each note's rows
    /// come from the row index ([`RowIndex`]): only notes that changed are laid out again.
    pub fn scroll_rows(&self) -> (usize, usize) {
        let rev = self.revision();
        let st = self.engine.state();
        let layout = Layout::of(&st.doc, &st.view);
        let mut idx = self.rows.borrow_mut();
        idx.update(rev, st, &layout);
        let total = idx.prefix.last().copied().unwrap_or(0) as usize;
        let top = layout.top(&st.view.scroll);
        let o = st.doc.blocks().expect("an outline document");
        let b = o.index_of_line(top.line);
        let within: usize = (o.blocks[b].first_line..top.line).map(|l| layout.line_rows(l)).sum();
        (total, idx.prefix[b] as usize + within + top.row)
    }

    /// A stamp of where the view is: its scroll, folds and whether it follows the caret.
    pub fn view_stamp(&self) -> u64 {
        let v = &self.engine.state().view;
        let mut h = foldhash::fast::FixedState::with_seed(0).build_hasher();
        (v.scroll.line, v.scroll.row, v.scroll.col, v.free, v.viewport.width, v.viewport.height).hash(&mut h);
        for m in &v.folds {
            m.0.hash(&mut h);
        }
        h.finish()
    }

    /// How many rows the document lays out to, counting no further than `cap` (cheap for a
    /// long document when only a few rows matter).
    pub fn rows_capped(&self, cap: usize) -> usize {
        let st = self.engine.state();
        let layout = Layout::of(&st.doc, &st.view);
        let start = RowPos { line: layout.visible_at_or_after(0).unwrap_or(0), row: 0 };
        (layout.rows_between(start, layout.end(), cap).max(0) as usize + 1).min(cap)
    }

    /// Where the view starts, as the engine keeps it (line, row): no layout.
    pub fn scroll_anchor(&self) -> (usize, usize) {
        let s = &self.engine.state().view.scroll;
        (s.line, s.row)
    }

    /// The first row on screen, as a row of the whole document.
    pub fn scroll(&self) -> usize {
        // At the top there's nothing to count (and a view never laid out needn't index its
        // rows to say so: opening a 5,000-line page beside, vw384).
        if self.scroll_anchor() == (0, 0) {
            return 0;
        }
        self.scroll_rows().1
    }

    /// Scroll so row `row` of the document is the first on screen. `free`: the view stays
    /// there until the caret moves (the wheel, the scrollbar); else it follows the caret again.
    pub fn set_scroll(&mut self, row: usize, free: bool) {
        self.engine.flush();
        let rev = self.revision();
        let st = self.engine.state_mut();
        let layout = Layout::of(&st.doc, &st.view);
        // The note row `row` is in (by the row index), then the line in it.
        let mut idx = self.rows.borrow_mut();
        idx.update(rev, st, &layout);
        let o = st.doc.blocks().expect("an outline document");
        let n = o.blocks.len();
        let b = idx.prefix.partition_point(|&p| (p as usize) <= row).saturating_sub(1).min(n.saturating_sub(1));
        let mut left = row.saturating_sub(idx.prefix[b] as usize);
        let mut top = RowPos { line: o.blocks[b].first_line, row: 0 };
        for l in o.blocks[b].first_line..=o.blocks[b].last_line() {
            let r = layout.line_rows(l);
            top = RowPos { line: l, row: left.min(r.saturating_sub(1)) };
            if left < r {
                break;
            }
            left -= r;
        }
        drop(idx);
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

    /// The view stays where it is until the caret moves by a key or the text changes: a click
    /// puts the caret where the pointer is, and never scrolls (mouse.md, interaction.md §3).
    pub fn hold_view(&mut self) {
        self.engine.flush();
        let st = self.engine.state_mut();
        st.view.free = true;
        cn::layout::clamp_scroll(st);
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

/// Each note's rows in the view, remembered by what decides them (its text and shape, its
/// blank row and the rows drawn after it, the view's geometry), and the running sums for the
/// document as it is now. An edit lays out again only the notes it changed; a frame, a clock
/// tick or a caret move lays out none.
#[derive(Default)]
pub(crate) struct RowIndex {
    by_content: HashMap<u64, u32, foldhash::fast::FixedState>,
    /// What `prefix` was summed for: the text, the marks, the view's geometry and folds.
    stamp: Option<u64>,
    /// `prefix[i]`: the rows before note `i` (one more entry: all of them).
    pub(crate) prefix: Vec<u32>,
    /// Notes laid out since the document opened (tests: an edit lays out what it changed).
    pub(crate) laid_out: usize,
}

impl RowIndex {
    fn update(&mut self, rev: u64, st: &cn::State, layout: &Layout) {
        let seed = foldhash::fast::FixedState::with_seed(0);
        let v = &st.view;
        let mut g = seed.build_hasher();
        v.viewport.width.hash(&mut g);
        if let Some(l) = &v.layout {
            (l.gutter, l.indent, l.hang, l.column, l.min_column, l.hang_glyphs).hash(&mut g);
            for (m, n) in &l.extra_rows {
                (m.0, n).hash(&mut g);
            }
        }
        for m in &v.folds {
            m.0.hash(&mut g);
        }
        let geometry = g.finish();
        let mut s = seed.build_hasher();
        (geometry, rev, st.doc.rev, st.doc.text.len_chars(), st.doc.marks.len()).hash(&mut s);
        let stamp = s.finish();
        if self.stamp == Some(stamp) {
            return;
        }
        let o = st.doc.blocks().expect("an outline document");
        let rope = &st.doc.text;
        let extra = v.layout.as_ref().map(|l| &l.extra_rows);
        self.prefix.clear();
        self.prefix.reserve(o.blocks.len() + 1);
        let mut sum = 0u32;
        let mut buf = String::new();
        for b in &o.blocks {
            self.prefix.push(sum);
            if !v.folds.is_empty() && layout.is_hidden(b.first_line) {
                continue;
            }
            let mut k = seed.build_hasher();
            (geometry, b.gap, b.fence, extra.and_then(|e| e.get(&b.id)).copied().unwrap_or(0)).hash(&mut k);
            // The text as one string: the rope's chunks move with edits nearby, the text doesn't.
            buf.clear();
            for c in rope.slice(b.start..b.end).chunks() {
                buf.push_str(c);
            }
            buf.hash(&mut k);
            let key = k.finish();
            let rows = match self.by_content.get(&key) {
                Some(&r) => r,
                None => {
                    let r = (b.first_line..=b.last_line()).map(|l| layout.line_rows(l)).sum::<usize>() as u32;
                    self.laid_out += 1;
                    if self.by_content.len() > 50_000 {
                        self.by_content.clear();
                    }
                    self.by_content.insert(key, r);
                    r
                }
            };
            sum += rows;
        }
        self.prefix.push(sum);
        self.stamp = Some(stamp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Target;

    fn big(n: usize) -> Doc {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 7).unwrap();
        let blocks: Vec<thc_core::outline::Block> = (0..n)
            .map(|i| serde_json::from_value(serde_json::json!({"id": format!("n{i}"), "parent": null, "depth": 0, "kind": "bullet", "text": format!("the quick brown fox jumps over the lazy dog and keeps running past the old fence {i}"), "text_rev": "r"})).unwrap())
            .collect();
        let mut d = Doc::new(Target::Page { id: "p".into(), title: "Big".into() }, Some("p".into()), &blocks, today);
        d.set_view(&ViewGeometry { width: 100, height: 40, column: 72, extra_rows: Vec::new(), typewriter: false });
        d
    }

    /// The engine's own count of the document's rows, walking all of it.
    fn engine_total(d: &Doc) -> usize {
        let st = d.engine.state();
        let layout = Layout::of(&st.doc, &st.view);
        layout.rows_between(RowPos { line: 0, row: 0 }, layout.end(), usize::MAX) as usize + 1
    }

    /// The view's place in the whole document comes from remembered rows: a caret move, a
    /// frame or asking again lays out no note, and typing lays out only the note typed in. (A
    /// whole-document layout per message once made typing on a 5,000-line page 160 ms a key.)
    #[test]
    fn only_changed_notes_are_laid_out_again() {
        let mut d = big(2000);
        let (total, top) = d.scroll_rows();
        assert_eq!(top, 0);
        assert_eq!(total, engine_total(&d));
        assert!(total >= 4000, "each note wraps to two rows: {total}");
        let warm = d.rows.borrow().laid_out;
        assert!(warm >= 1, "the first look lays the document out");
        d.run_command("move.down");
        d.scroll_rows();
        let _ = d.frame();
        d.scroll_rows();
        assert_eq!(d.rows.borrow().laid_out, warm, "nothing changed: nothing laid out");
        d.insert("typed ");
        let (total2, _) = d.scroll_rows();
        let more = d.rows.borrow().laid_out - warm;
        assert!(more <= 1, "only the note typed in: {more}");
        assert_eq!(total2, engine_total(&d), "the rows agree with the engine's walk");
    }

    /// Scrolling to a row and reading it back agree, anywhere in the document.
    #[test]
    fn set_scroll_and_scroll_agree() {
        let mut d = big(300);
        for row in [0, 1, 7, 100, 333] {
            d.set_scroll(row, true);
            assert_eq!(d.scroll(), row, "row {row}");
        }
    }
}
