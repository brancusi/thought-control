//! A [caretline](https://docs.rs/caretline) `View`, drawn into a ratatui `Buffer`: each row of
//! the view's layout in its rect, the block's marker in the hang (`- `, `[ ] `, `[x] `), the
//! selection, and where the caret is. Hit-testing maps a cell back to a position, from the same
//! layout, so what's drawn and where a click lands can't disagree.
//!
//! ```
//! use caretline::buffer::BlockLine;
//! use caretline::doc::{Doc, Rect, View};
//! use caretline::{Block, Kind};
//! use ratatui::buffer::Buffer;
//!
//! #[derive(Clone, PartialEq)]
//! struct Line(Block<u32>);
//! impl std::ops::Deref for Line { type Target = Block<u32>; fn deref(&self) -> &Block<u32> { &self.0 } }
//! impl std::ops::DerefMut for Line { fn deref_mut(&mut self) -> &mut Block<u32> { &mut self.0 } }
//! impl BlockLine for Line {
//!     type Id = u32;
//!     fn fresh(depth: usize, kind: Kind, text: &str) -> Self {
//!         static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
//!         Line(Block::new(N.fetch_add(1, std::sync::atomic::Ordering::Relaxed), depth, kind, text))
//!     }
//!     fn is_new(&self) -> bool { true }
//!     fn keep_host_state(&mut self, _: &Self) {}
//!     fn revive(&mut self) {}
//! }
//!
//! let doc = Doc::new(vec![Line::fresh(0, Kind::Task, "Buy milk")]);
//! let mut view = View::new(Rect { x: 0, y: 0, width: 30, height: 3 });
//! view.focused = true;
//! let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 30, 3));
//! let caret = caretline_ratatui::render(&doc, &view, &mut buf, &caretline_ratatui::Theme::default());
//! assert_eq!(caret, Some((4, 0)));
//! assert_eq!(buf[(0, 0)].symbol(), "[");
//! ```

use caretline::buffer::BlockLine;
use caretline::doc::{Doc, Row, View};
use caretline::{Kind, Pos};
use ratatui::buffer::Buffer;
use ratatui::style::{Modifier, Style};
use unicode_segmentation::UnicodeSegmentation;

/// The styles a view is drawn with.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub text: Style,
    /// The hang: `- `, `[ ] `.
    pub marker: Style,
    /// A done task's text.
    pub done: Style,
    /// Selected text (focused view).
    pub selection: Style,
    /// The caret's cell in a view without focus (a focused one gets the terminal cursor).
    pub caret_unfocused: Style,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            text: Style::default(),
            marker: Style::default().add_modifier(Modifier::DIM),
            done: Style::default().add_modifier(Modifier::DIM | Modifier::CROSSED_OUT),
            selection: Style::default().add_modifier(Modifier::REVERSED),
            caret_unfocused: Style::default().add_modifier(Modifier::UNDERLINED),
        }
    }
}

/// The marker a block's first row shows in its hang.
fn marker<Id>(b: &caretline::Block<Id>) -> &'static str {
    match (b.kind, b.status.as_deref()) {
        (Kind::Task, Some("done")) => "[x] ",
        (Kind::Task, Some("cancelled")) => "[-] ",
        (Kind::Task, _) => "[ ] ",
        (Kind::Bullet, _) => "- ",
        (Kind::Para, _) => "",
    }
}

/// The rows on screen: (layout row, its y).
fn visible<L: BlockLine>(doc: &Doc<L>, view: &View<L::Id>) -> Vec<(Row, u16)> {
    let r = view.rect;
    doc.layout(view).into_iter().skip(view.scroll).take(r.height as usize).enumerate().map(|(k, row)| (row, r.y + k as u16)).collect()
}

/// Draw `view` into `buf` (inside its rect only). Returns the caret's cell when the view has
/// focus and the caret is on screen, for the host to put the terminal's cursor there.
pub fn render<L: BlockLine>(doc: &Doc<L>, view: &View<L::Id>, buf: &mut Buffer, theme: &Theme) -> Option<(u16, u16)> {
    let r = view.rect;
    if r.width == 0 || r.height == 0 {
        return None;
    }
    let right = r.x + r.width;
    let sel = view.anchor.filter(|a| *a != view.caret).map(|a| if a < view.caret { (a, view.caret) } else { (view.caret, a) });
    let lines = doc.lines();
    for (row, y) in visible(doc, view) {
        let l = &lines[row.line];
        let x0 = r.x + row.x as u16;
        if row.start == 0 {
            let m = marker(l);
            let hx = x0.saturating_sub(m.len() as u16).max(r.x);
            if hx < right {
                buf.set_stringn(hx, y, m, (right - hx) as usize, theme.marker);
            }
        }
        let style = if l.status.as_deref() == Some("done") { theme.done } else { theme.text };
        let mut x = x0;
        let text = &l.text[row.start..row.end];
        for (i, g) in text.grapheme_indices(true) {
            if g == "\n" {
                continue;
            }
            let w = caretline::gwidth(g).max(1) as u16;
            if x + w > right {
                break;
            }
            let at = Pos { line: row.line, byte: row.start + i };
            let s = if sel.is_some_and(|(a, b)| at >= a && at < b) { style.patch(theme.selection) } else { style };
            buf.set_string(x, y, g, s);
            x += w;
        }
    }
    let cell = caret_cell(doc, view)?;
    if view.focused {
        Some(cell)
    } else {
        if let Some(c) = buf.cell_mut(cell) {
            c.set_style(theme.caret_unfocused);
        }
        None
    }
}

/// The cell the caret is drawn in, when it's on screen.
pub fn caret_cell<L: BlockLine>(doc: &Doc<L>, view: &View<L::Id>) -> Option<(u16, u16)> {
    let c = view.caret;
    let lines = doc.lines();
    let rows = visible(doc, view);
    let (row, y) = rows.iter().rev().find(|(r, _)| r.line == c.line && r.start <= c.byte)?;
    let t = &lines[row.line].text;
    let x = view.rect.x as usize + row.x + caretline::width(&t[row.start..c.byte.min(row.end).max(row.start)]);
    (x < (view.rect.x + view.rect.width) as usize).then_some((x as u16, *y))
}

/// The position a cell maps to (a click): the grapheme under it, or the row's end past its
/// text. None outside the view's rows.
pub fn hit<L: BlockLine>(doc: &Doc<L>, view: &View<L::Id>, x: u16, y: u16) -> Option<Pos> {
    let r = view.rect;
    if x < r.x || x >= r.x + r.width {
        return None;
    }
    let (row, _) = visible(doc, view).into_iter().find(|(_, ry)| *ry == y)?;
    let t = &doc.lines()[row.line].text;
    let mut col = r.x as usize + row.x;
    for (i, g) in t[row.start..row.end].grapheme_indices(true) {
        if g == "\n" {
            break;
        }
        let w = caretline::gwidth(g).max(1);
        if (x as usize) < col + w {
            return Some(Pos { line: row.line, byte: row.start + i });
        }
        col += w;
    }
    let end = if row.end < t.len() && t[..row.end].ends_with('\n') { row.end - 1 } else { row.end };
    Some(Pos { line: row.line, byte: end.max(row.start) })
}
