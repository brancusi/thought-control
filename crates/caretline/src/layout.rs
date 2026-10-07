//! Layout on top of Helix's `DocumentFormatter`: visual rows, the caret's place on screen,
//! and keeping it in view. Shared by `update` (motion, scrolling) and `view` (drawing).

use std::cell::RefCell;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::marks::MarkId;
use std::sync::Arc;

use crate::helix::chars::char_is_line_ending;
use crate::helix::doc_formatter::{DocumentFormatter, TextFormat};
use crate::helix::text_annotations::TextAnnotations;
use crate::helix::transaction::{ChangeSet, Operation};
use crate::helix::{Rope, RopeSlice};
use crate::outline::Outline;
use crate::state::{Config, Document, Follow, Scroll, State, View};

/// The outline layout a host gives a view: column geometry as data. With it, an outline
/// document's markers move out of the text into a hang, nested blocks get their own
/// columns, and folds hide children (see `docs/caretline/structure.md`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutlineLayout {
    /// Columns before everything: the gutter, where a decoration's `gutter` text goes.
    #[serde(alias = "marks")]
    pub gutter: u16,
    /// Columns per depth.
    pub indent: u16,
    /// Columns of the hang, before the content: where a block's marker glyph or decoration goes.
    pub hang: u16,
    /// The wrap width of depth-0 content (`min(72, available)` is a good choice).
    pub column: u16,
    /// The narrowest a nested block's content wraps at.
    pub min_column: u16,
    /// Rows a host draws after a block (anything of its own: an inline image, a form).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extra_rows: BTreeMap<MarkId, u16>,
    /// Draw a plain glyph in each hang (`•`, `1.`, `[ ]`, `#`), for a host that draws none.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hang_glyphs: bool,
}

impl Default for OutlineLayout {
    fn default() -> Self {
        OutlineLayout { gutter: 2, indent: 4, hang: 4, column: 72, min_column: 20, extra_rows: BTreeMap::new(), hang_glyphs: false }
    }
}

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
        hang_spaces: false,
    }
}

/// The text format of an outline block's content: prose wrapping. A word moves to the next
/// row whole unless it's longer than a row (with `hang`; words end at whitespace); the space after a word that reaches the row's end
/// stays on that row (the next row starts with the next word); a word that ends exactly at the
/// row's end before a line break or the end stays.
/// `hang`: there's a cell right of the column for the caret, so a space (or a word that ends
/// exactly there) may sit at the row's end; without one, the Helix rule (the next row takes
/// it) keeps the caret on screen.
pub fn prose_format(config: &Config, width: u16, wrap: bool, hang: bool) -> TextFormat {
    let mut f = text_format(config, width, wrap);
    if hang {
        // Words wrap whole; only a word longer than a row breaks (at the row's end).
        f.max_wrap = f.viewport_width;
    }
    f.hang_spaces = hang;
    f.soft_wrap_at_text_width = hang;
    f
}

/// How one document line is laid out: what of it is drawn, where, at what width, and the
/// virtual rows around it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineFormat {
    /// Chars at the line's start that are not drawn (a block's indentation and marker, in an
    /// outline layout). Never a caret stop.
    pub skip: usize,
    /// The screen column its text starts at (before horizontal scrolling).
    pub x: usize,
    /// Under a folded block: it takes no rows.
    pub hidden: bool,
    /// Virtual rows before its text (a block's blank row).
    pub before: usize,
    /// Virtual rows after its text (rows a host draws after a block).
    pub after: usize,
    /// Which text format it wraps with: 0 the layout's own, else an outline depth's.
    fmt: usize,
}

/// Layout context for one view of a document. Holds its own (cheap, shared) copy of the
/// rope so the document can change while a layout of the old text is in use.
///
/// Every line has a [`LineFormat`]. A plain document's lines all start at column 0 with the
/// view's width. An outline document adds a blank row before gapped blocks and hides the
/// children of folded ones; with an [`OutlineLayout`], each block's marker moves out of the
/// text into a hang, its content starts at its depth's column and wraps at its depth's width,
/// and a host's extra rows follow it. Motion, paging, scrolling, hit-testing and drawing all go
/// through the same rows.
pub struct Layout {
    rope: Rope,
    /// The text format of a line with no outline layout: the view's width.
    pub fmt: TextFormat,
    pub annotations: TextAnnotations<'static>,
    /// A copy of the view's [`WrapCache`], extended as lookups need.
    cache: RefCell<WrapCache>,
    /// An outline document's blocks: a gapped block's first line has a virtual (blank) row
    /// before its text rows. Rows of a line count from that virtual row.
    outline: Option<Arc<Outline>>,
    /// The view's outline layout, for an outline document.
    geometry: Option<OutlineLayout>,
    /// The lines folds hide, as sorted `[from, to)` ranges.
    hidden: Vec<(usize, usize)>,
    /// With a geometry: the text format of each depth, wrapping and not (`2·depth + nowrap`).
    depth_fmts: Vec<TextFormat>,
    /// A line that doesn't wrap, scrolled sideways to show the caret: the line and how far.
    offset: Option<(usize, usize)>,
    /// The view's width.
    width: u16,
}

impl Layout {
    pub fn new(state: &State) -> Self {
        Layout::of(&state.doc, &state.view)
    }

    /// The layout of `view` on `doc`.
    pub fn of(doc: &Document, view: &View) -> Self {
        Layout::build(doc, view, true)
    }

    /// The same document laid out without soft wrap (for logical-line motion).
    pub fn unwrapped(state: &State) -> Self {
        let mut l = Layout::build(&state.doc, &state.view, false);
        l.cache = RefCell::new(WrapCache::default());
        l
    }

    fn build(doc: &Document, view: &View, wrap: bool) -> Self {
        let outline = doc.blocks();
        let geometry = outline.as_ref().and(view.layout.clone());
        let hidden = match &outline {
            Some(o) => crate::views::hidden_lines(o, &view.folds),
            None => Vec::new(),
        };
        let width = view.viewport.width;
        let mut depth_fmts = Vec::new();
        if let (Some(g), Some(o)) = (&geometry, &outline) {
            let max_depth = o.blocks.iter().map(|b| b.depth as usize).max().unwrap_or(0);
            // Past the width every deeper column is the same: one column at the right edge.
            let cap = (width as usize / g.indent.max(1) as usize) + 1;
            for d in 0..=max_depth.min(cap) {
                let w = depth_width(g, d, width);
                // A space hangs past the column only where there's a cell for the caret there.
                let room = (width as usize).saturating_sub(block_x(g, d, width));
                let hang = (w as usize) < room;
                depth_fmts.push(prose_format(&doc.config, w, wrap, hang));
                depth_fmts.push(prose_format(&doc.config, w, false, hang));
            }
        }
        let mut layout = Layout {
            rope: doc.text.clone(),
            fmt: text_format(&doc.config, width, wrap),
            annotations: TextAnnotations::default(),
            cache: RefCell::new(view.wrap.clone()),
            outline,
            geometry,
            hidden,
            depth_fmts,
            offset: None,
            width,
        };
        // A long line that doesn't wrap (a fence in an outline layout) scrolls sideways on
        // its own while the caret is on it.
        if layout.geometry.is_some() {
            let caret = view.caret().min(layout.text().len_chars());
            let line = layout.text().char_to_line(caret);
            let lf = layout.line_format(line);
            let fmt = layout.fmt_of(&lf);
            if !fmt.soft_wrap {
                let col = layout.pos_coords(caret).1 - lf.x;
                let w = fmt.viewport_width as usize;
                if col >= w {
                    layout.offset = Some((line, col + 1 - w));
                }
            }
        }
        layout
    }

    /// How line `line` is laid out.
    pub fn line_format(&self, line: usize) -> LineFormat {
        let plain = LineFormat { skip: 0, x: 0, hidden: false, before: 0, after: 0, fmt: 0 };
        let Some(o) = &self.outline else { return plain };
        let b = o.block_of_line(line);
        let first = b.first_line == line;
        let hidden = crate::views::hidden_range(&self.hidden, line).is_some();
        let before = (first && b.gap) as usize;
        let Some(g) = &self.geometry else { return LineFormat { hidden, before, ..plain } };
        let d = b.depth as usize;
        let after = if line == b.last_line() { g.extra_rows.get(&b.id).copied().unwrap_or(0) as usize } else { 0 };
        let level = d.min(self.depth_fmts.len() / 2 - 1);
        LineFormat {
            skip: if first { b.prefix_len } else { 0 },
            x: block_x(g, d, self.width),
            hidden,
            before,
            after,
            fmt: 1 + 2 * level + b.fence as usize,
        }
    }

    fn fmt_of(&self, lf: &LineFormat) -> &TextFormat {
        match lf.fmt {
            0 => &self.fmt,
            i => &self.depth_fmts[i - 1],
        }
    }

    /// The text format line `line` wraps with.
    pub fn line_text_format(&self, line: usize) -> &TextFormat {
        let lf = self.line_format(line);
        self.fmt_of(&lf)
    }

    /// The view's outline layout, when it has one.
    pub fn geometry(&self) -> Option<&OutlineLayout> {
        self.geometry.as_ref()
    }

    /// The document's blocks, for an outline document.
    pub fn outline(&self) -> Option<&Arc<Outline>> {
        self.outline.as_ref()
    }

    /// How far line `line` is scrolled sideways (a long line that doesn't wrap, with the
    /// caret on it).
    pub fn line_offset(&self, line: usize) -> usize {
        match self.offset {
            Some((l, n)) if l == line => n,
            _ => 0,
        }
    }

    /// Where line `line`'s drawn text starts: after its skipped chars.
    pub fn content_start(&self, line: usize) -> usize {
        self.text().line_to_char(line) + self.line_format(line).skip
    }

    /// Virtual rows before line `line`'s text: 1 for a gapped block's first line, else 0.
    pub fn gap(&self, line: usize) -> usize {
        self.line_format(line).before
    }

    /// Whether line `line` is hidden under a fold.
    pub fn is_hidden(&self, line: usize) -> bool {
        crate::views::hidden_range(&self.hidden, line).is_some()
    }

    /// The first line at or after `line` that isn't hidden.
    pub fn visible_at_or_after(&self, line: usize) -> Option<usize> {
        let l = match crate::views::hidden_range(&self.hidden, line) {
            Some((_, b)) => b,
            None => line,
        };
        (l <= self.last_line()).then_some(l)
    }

    /// The last line at or before `line` that isn't hidden.
    pub fn visible_at_or_before(&self, line: usize) -> Option<usize> {
        match crate::views::hidden_range(&self.hidden, line) {
            Some((a, _)) => a.checked_sub(1),
            None => Some(line),
        }
    }

    /// Whether `at` is a virtual row (never a caret stop): a blank row before a block's text,
    /// or a host's row after it.
    pub fn is_virtual(&self, at: RowPos) -> bool {
        let lf = self.line_format(at.line);
        at.row < lf.before || (lf.after > 0 && at.row >= lf.before + self.text_rows_of(at.line))
    }

    /// Hands what this layout learned about long lines back to `state`, whose text must be
    /// the text this layout was made from.
    pub fn store(self, state: &mut State) {
        state.view.wrap = self.cache.into_inner();
    }

    /// The char range `[start, end)` of line `line`'s drawn text (its break excluded).
    fn content_range(&self, line: usize, lf: &LineFormat) -> (usize, usize) {
        let text = self.text();
        let start = text.line_to_char(line);
        let end = crate::helix::line_ending::line_end_char_index(&text, line);
        ((start + lf.skip).min(end), end)
    }

    /// Whether line `line` is laid out through the [`WrapCache`].
    fn is_long(&self, line: usize, lf: &LineFormat) -> bool {
        if !self.fmt_of(lf).soft_wrap {
            return false;
        }
        let (start, end) = self.content_range(line, lf);
        end - start >= LONG_LINE_CHARS
    }

    /// Makes sure the cache knows what `need` asks of long line `line`, and returns the row
    /// to start formatting from with its start and the line's indent level.
    fn known_row(&self, line: usize, lf: &LineFormat, need: Need) -> (usize, RowStart, Option<usize>) {
        let key = fmt_key(self.fmt_of(lf));
        let start = self.content_range(line, lf).0;
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
            self.extend(e, lf, need);
        }
        let row = need.row(e);
        (row, e.rows[row], e.indent)
    }

    /// Formats `e` on from its last known row until `need` is met.
    fn extend(&self, e: &mut LineRows, lf: &LineFormat, need: Need) {
        let last = e.rows.len() - 1;
        let mut formatter = self.formatter_from(e.line, lf, last, e.rows[last], e.indent);
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

    fn formatter_from(&self, line: usize, lf: &LineFormat, row: usize, at: RowStart, indent: Option<usize>) -> DocumentFormatter<'_> {
        let fmt = self.fmt_of(lf);
        if row == 0 && lf.skip == 0 {
            DocumentFormatter::new_at_prev_checkpoint(self.text(), fmt, &self.annotations, at.char_idx)
        } else {
            DocumentFormatter::resume_at_row(self.text(), fmt, &self.annotations, at.char_idx, line, row, at.col, indent)
        }
    }

    /// A formatter for `line` that starts at its row `need` names (or an earlier one).
    fn formatter_for(&self, line: usize, lf: &LineFormat, need: Need) -> DocumentFormatter<'_> {
        if self.is_long(line, lf) {
            let (row, at, indent) = self.known_row(line, lf, need);
            self.formatter_from(line, lf, row, at, indent)
        } else {
            let start = self.content_range(line, lf).0;
            self.formatter_from(line, lf, 0, RowStart { char_idx: start, col: 0 }, None)
        }
    }

    /// A formatter that starts at or before visual row `at` of `at.line` and yields every
    /// grapheme of that line from there on, with rows numbered from its first text row and
    /// columns from its content column (add [`LineFormat::x`]).
    pub fn formatter_at_row(&self, at: RowPos) -> DocumentFormatter<'_> {
        let lf = self.line_format(at.line);
        let row = at.row.saturating_sub(lf.before);
        self.formatter_for(at.line, &lf, Need::Row(row))
    }

    pub fn text(&self) -> RopeSlice<'_> {
        self.rope.slice(..)
    }

    /// Whether lines soft-wrap at the view's width (an outline layout wraps each line at its
    /// own width, and scrolls sideways one line at a time).
    pub fn wraps(&self) -> bool {
        self.fmt.soft_wrap || self.geometry.is_some()
    }

    pub fn last_line(&self) -> usize {
        self.text().len_lines().saturating_sub(1)
    }

    /// The number of visual rows document line `line` takes (its virtual rows included). A
    /// hidden line takes none.
    pub fn line_rows(&self, line: usize) -> usize {
        let lf = self.line_format(line);
        if lf.hidden {
            return 0;
        }
        lf.before + self.text_rows_with(line, &lf) + lf.after
    }

    /// A cursor for counting the rows of neighbouring lines one after another (see
    /// [`LineWalk`]).
    fn walk(&self) -> LineWalk<'_> {
        LineWalk { layout: self, lines: None }
    }

    /// The rows line `line`'s text takes.
    pub fn text_rows_of(&self, line: usize) -> usize {
        let lf = self.line_format(line);
        if lf.hidden {
            return 0;
        }
        self.text_rows_with(line, &lf)
    }

    fn text_rows_with(&self, line: usize, lf: &LineFormat) -> usize {
        if !self.fmt_of(lf).soft_wrap || self.fits_one_row(line, lf) {
            return 1;
        }
        if self.is_long(line, lf) {
            self.known_row(line, lf, Need::All);
            let cache = self.cache.borrow();
            let key = fmt_key(self.fmt_of(lf));
            if let Some(e) = cache.0.iter().find(|e| e.fmt == key && e.line == line) {
                return e.rows.len();
            }
        }
        self.formatted_rows_with(line, lf)
    }

    /// The rows the formatter gives line `line`.
    #[cfg(test)]
    fn formatted_rows(&self, line: usize) -> usize {
        let lf = self.line_format(line);
        self.formatted_rows_with(line, &lf)
    }

    fn formatted_rows_with(&self, line: usize, lf: &LineFormat) -> usize {
        let start = self.content_range(line, lf).0;
        let formatter = self.formatter_from(line, lf, 0, RowStart { char_idx: start, col: 0 }, None);
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
    fn fits_one_row(&self, line: usize, lf: &LineFormat) -> bool {
        use unicode_width::UnicodeWidthChar;
        let fmt = self.fmt_of(lf);
        let width = fmt.viewport_width as usize;
        let tab = fmt.tab_width as usize;
        // `Rope::line` skips the full-slice bookkeeping `RopeSlice::line` pays.
        let text = self.rope.line(line);
        let rest = if lf.skip == 0 { text } else { text.slice(lf.skip.min(text.len_chars())..) };
        if surely_fits(rest, width, tab) {
            return true;
        }
        let mut sum = 1;
        for c in rest.chars() {
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

    /// The visual place of char position `pos`: its row position and its screen column
    /// (before horizontal scrolling). A position inside a line's skipped chars is placed at
    /// its text's start; one inside a folded block, at the fold's end.
    pub fn pos_coords(&self, pos: usize) -> (RowPos, usize) {
        let pos = pos.min(self.text().len_chars());
        let line = self.text().char_to_line(pos);
        let lf = self.line_format(line);
        if lf.hidden {
            if let Some(owner) = self.visible_at_or_before(line) {
                let end = crate::helix::line_ending::line_end_char_index(&self.text(), owner);
                return self.pos_coords(end);
            }
        }
        let pos = pos.max(self.content_range(line, &lf).0);
        // As Helix's `visual_offset_from_block`, from the nearest known row.
        let mut formatter = self.formatter_for(line, &lf, Need::Pos(pos));
        let mut last = crate::helix::Position::default();
        while let Some(g) = formatter.next() {
            last = g.visual_pos;
            if formatter.next_char_pos() > pos {
                break;
            }
        }
        (RowPos { line, row: last.row + lf.before }, last.col + lf.x)
    }

    /// The char position on visual row `at` closest to screen column `col` (Helix's rule: the
    /// grapheme covering the column, else the row's last grapheme). A virtual row before a
    /// line's text gives its text's start; one after it, its end.
    pub fn pos_at(&self, at: RowPos, col: usize) -> usize {
        let lf = self.line_format(at.line);
        let (start, end) = self.content_range(at.line, &lf);
        if at.row < lf.before {
            return start;
        }
        let row = at.row - lf.before;
        if lf.after > 0 && row >= self.text_rows_with(at.line, &lf) {
            return end;
        }
        let col = col.saturating_sub(lf.x).saturating_add(self.line_offset(at.line));
        // Search within the one line so a row past its end can't spill into the next line.
        let next = if at.line < self.last_line() { self.text().line_to_char(at.line + 1) } else { self.text().len_chars() };
        self.char_at_row_col(at.line, &lf, row, col).min(next).max(start)
    }

    /// Helix's `char_idx_at_visual_block_offset` from the nearest known row: the grapheme
    /// covering `col` on text row `row`, else that row's last grapheme.
    fn char_at_row_col(&self, line: usize, lf: &LineFormat, row: usize, col: usize) -> usize {
        use std::cmp::Ordering;
        let mut formatter = self.formatter_for(line, lf, Need::Row(row));
        let mut last_char_idx = formatter.next_char_pos();
        let mut found_non_virtual_on_row = false;
        for g in &mut formatter {
            match g.visual_pos.row.cmp(&row) {
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
    /// the document. Hidden lines take no rows. Returns the position and how many rows were
    /// actually moved.
    pub fn step_rows(&self, at: RowPos, n: isize) -> (RowPos, isize) {
        let mut walk = self.walk();
        let mut pos = at;
        let mut moved: isize = 0;
        if n >= 0 {
            let mut left = n as usize;
            while left > 0 {
                let rows = walk.rows(pos.line).max(1);
                if pos.row + left < rows {
                    pos.row += left;
                    moved += left as isize;
                    left = 0;
                } else if let Some(next) = (pos.line < self.last_line()).then(|| self.visible_at_or_after(pos.line + 1)).flatten() {
                    let step = rows - pos.row;
                    left -= step;
                    moved += step as isize;
                    pos = RowPos { line: next, row: 0 };
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
                } else if let Some(prev) = pos.line.checked_sub(1).and_then(|l| self.visible_at_or_before(l)) {
                    let step = pos.row + 1;
                    left -= step;
                    moved -= step as isize;
                    pos.line = prev;
                    pos.row = walk.rows(prev).max(1) - 1;
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
        let mut walk = self.walk();
        while line < to.line {
            sum += walk.rows(line).saturating_sub(row);
            row = 0;
            line += 1;
            if let Some((_, b)) = crate::views::hidden_range(&self.hidden, line) {
                line = b.min(to.line);
            }
            if sum > limit {
                return limit as isize + 1;
            }
        }
        (sum + to.row).saturating_sub(row) as isize
    }

    /// The top of the view as a row position, clamped to the document and off hidden lines.
    pub fn top(&self, scroll: &Scroll) -> RowPos {
        let line = scroll.line.min(self.last_line());
        let line = self.visible_at_or_after(line).or_else(|| self.visible_at_or_before(line)).unwrap_or(0);
        let row = scroll.row.min(self.line_rows(line).max(1) - 1);
        RowPos { line, row }
    }

    /// The document's last row.
    pub fn end(&self) -> RowPos {
        let line = self.visible_at_or_before(self.last_line()).unwrap_or(0);
        RowPos { line, row: self.line_rows(line).max(1) - 1 }
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

/// The screen column of a block's content at depth `d`. Deep blocks in a narrow view stop
/// indenting where their content would have fewer than `min_column` columns (or the whole
/// view, when it is narrower).
pub(crate) fn block_x(g: &OutlineLayout, d: usize, width: u16) -> usize {
    let x = g.gutter as usize + d * g.indent as usize + g.hang as usize;
    let keep = (g.min_column as usize).min(width as usize);
    x.min((width as usize).saturating_sub(keep))
}

/// The wrap width of a block's content at depth `d`: its depth's column, at least the
/// narrowest, and never past the view's right edge.
fn depth_width(g: &OutlineLayout, d: usize, width: u16) -> u16 {
    let column = (g.column as usize).saturating_sub(d * g.indent as usize).max(g.min_column as usize);
    let room = (width as usize).saturating_sub(block_x(g, d, width));
    column.min(room).max(1) as u16
}

/// Keeps a freely scrolled view's top inside the document, without following the caret.
pub fn clamp_scroll(state: &mut State) {
    let layout = Layout::new(state);
    let top = layout.top(&state.view.scroll);
    let col = if layout.wraps() { 0 } else { state.view.scroll.col };
    state.view.scroll = Scroll { line: top.line, row: top.row, col };
    layout.store(state);
}

/// Moves the view the least needed for the primary caret to sit in it, `scrolloff` rows
/// from the edges where possible.
pub fn ensure_caret_visible(state: &mut State) {
    let layout = Layout::new(state);
    let h = state.text_rows();
    let w = state.view.viewport.width as usize;
    let (caret, col) = layout.pos_coords(state.caret());
    let mut top = layout.top(&state.view.scroll);
    if let (Follow::Typewriter { percent }, true) = (state.view.config.follow, h > 0) {
        // The caret's row sits at `percent` of the height (the top clamps at the start).
        let row = ((h - 1) * percent.min(100) as usize + 50) / 100;
        top = layout.step_rows(caret, -(row as isize)).0;
    } else if h > 0 {
        let so = (state.view.config.scrolloff as usize).min((h - 1) / 2);
        let dist = layout.rows_between(top, caret, h + so);
        if dist < so as isize {
            top = layout.step_rows(caret, -(so as isize)).0;
        } else if dist > (h - 1 - so) as isize {
            top = layout.step_rows(caret, -((h - 1 - so) as isize)).0;
        }
        // Don't leave empty rows below the end of the document. Every visible line takes at
        // least one row, so without folds this can only happen when the last line is fewer
        // than `h` lines below the top; skipping the walk otherwise keeps updates cheap in
        // long documents.
        if layout.last_line().saturating_sub(top.line) < h || !layout.hidden.is_empty() {
            let max_top = layout.step_rows(layout.end(), -(h as isize - 1)).0;
            if top > max_top && max_top <= caret {
                top = max_top;
            }
        }
    } else {
        top = caret;
    }
    let mut scroll_col = state.view.scroll.col;
    if layout.wraps() {
        scroll_col = 0;
    } else if col < scroll_col {
        scroll_col = col;
    } else if col >= scroll_col + w {
        scroll_col = col + 1 - w;
    }
    state.view.scroll = Scroll {
        line: top.line,
        row: top.row,
        col: scroll_col,
    };
    layout.store(state);
}

/// Counts the rows of lines visited one next to another (stepping or measuring rows). In
/// plain text (no outline, no folds) it keeps a lines iterator beside the last line it saw,
/// so a neighbour costs no tree descent, and only a line the width bound can't place on one
/// row is formatted. Otherwise it is [`Layout::line_rows`].
struct LineWalk<'a> {
    layout: &'a Layout,
    /// The iterator and the line its `next` returns.
    lines: Option<(crate::helix::ropey::iter::Lines<'a>, usize)>,
}

impl LineWalk<'_> {
    fn rows(&mut self, line: usize) -> usize {
        let l = self.layout;
        if l.outline.is_some() || !l.hidden.is_empty() {
            return l.line_rows(line);
        }
        if !l.fmt.soft_wrap {
            return 1;
        }
        let slice = match &mut self.lines {
            Some((it, at)) if *at == line => {
                *at += 1;
                it.next()
            }
            Some((it, at)) if *at == line + 1 => {
                *at = line;
                it.prev()
            }
            _ => {
                let mut it = l.rope.lines_at(line);
                let s = it.next();
                self.lines = Some((it, line + 1));
                s
            }
        };
        match slice {
            Some(s) if surely_fits(s, l.fmt.viewport_width as usize, l.fmt.tab_width as usize) => 1,
            _ => l.line_rows(line),
        }
    }
}

/// Whether a line's text surely fits on one row of `width` cells, by a bound that needs no
/// char walk: no char but a tab is wider than its UTF-8 bytes (a double-width char is at least
/// 3 bytes), so the bytes plus the tabs' extra cells, plus a cell for the line end, bound the
/// width. `false` means "maybe not": format the line to know.
fn surely_fits(line: RopeSlice<'_>, width: usize, tab: usize) -> bool {
    let mut bytes = 0;
    let mut tabs = 0;
    for chunk in line.chunks() {
        bytes += chunk.len();
        if chunk.contains('\t') {
            tabs += chunk.matches('\t').count();
        }
        if bytes >= width {
            return false;
        }
    }
    1 + bytes + tabs * tab.saturating_sub(1) < width
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
            if layout.fits_one_row(0, &layout.line_format(0)) {
                assert_eq!(layout.formatted_rows(0), 1, "{line:?} at width {width}");
            }
        }
    }

    /// The line walk (an iterator beside the last line, and the width bound) counts the
    /// same rows as asking each line on its own, stepping down, up and measuring.
    #[test]
    fn the_line_walk_counts_what_line_rows_counts() {
        let pieces = ["a", "word ", "\t", "界", "🙂", "e\u{301}", "  ", "long-unbroken-token", "\n"];
        let mut next = xorshift(0x9e37_79b9);
        for _ in 0..200 {
            let mut text = String::new();
            for _ in 0..(next() % 300) {
                text.push_str(pieces[next() as usize % pieces.len()]);
            }
            let width = 11 + (next() % 40) as u16;
            let state = State::new(&text, None, Viewport { width, height: 10 });
            let layout = Layout::new(&state);
            let last = layout.last_line();
            let rows: Vec<usize> = (0..=last).map(|l| layout.line_rows(l)).collect();
            let a = (next() as usize) % (last + 1);
            let b = (next() as usize) % (last + 1);
            let (a, b) = (a.min(b), a.max(b));
            let want: usize = rows[a..b].iter().sum();
            let from = RowPos { line: a, row: 0 };
            let to = RowPos { line: b, row: 0 };
            assert_eq!(layout.rows_between(from, to, usize::MAX / 2), want as isize, "{text:?} at {width}");
            assert_eq!(layout.step_rows(from, want as isize), (to, want as isize), "{text:?} at {width}");
            assert_eq!(layout.step_rows(to, -(want as isize)), (from, -(want as isize)), "{text:?} at {width}");
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
                cold.view.wrap.clear();
                update(&mut warm, msg.clone());
                update(&mut cold, msg.clone());
                let ctx = format!("seed {seed} step {step} {msg:?}");
                assert_eq!(warm, cold, "{ctx}: state");
                assert_eq!(warm.view.scroll, cold.view.scroll, "{ctx}: scroll");
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
