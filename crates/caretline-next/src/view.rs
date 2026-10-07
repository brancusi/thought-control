//! `view`: a pure function from the state to a grid of cells, plus text and ANSI renderers
//! for snapshots. A terminal front end copies the grid to the screen.

use crate::helix::graphemes::{grapheme_width, Grapheme};
use crate::helix::Tendril;
use crate::layout::Layout;
use crate::state::State;
use unicode_segmentation::UnicodeSegmentation;

/// What a cell shows, by meaning. Front ends pick the colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Text,
    Selection,
    Status,
    /// The dirty marker and other emphasis in the status bar.
    StatusAccent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// The grapheme drawn here. Empty for the second cell of a wide grapheme. Stored inline
    /// (no allocation) for anything up to 23 bytes.
    pub symbol: Tendril,
    pub role: Role,
}

/// A rendered screen: `height` rows of `width` cells, and where the caret is (if visible).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u16,
    pub height: u16,
    pub cells: Vec<Cell>,
    pub cursor: Option<(u16, u16)>,
}

impl Frame {
    fn new(width: u16, height: u16) -> Frame {
        Frame {
            width,
            height,
            cells: vec![
                Cell {
                    symbol: " ".into(),
                    role: Role::Text
                };
                width as usize * height as usize
            ],
            cursor: None,
        }
    }

    pub fn cell(&self, x: u16, y: u16) -> &Cell {
        &self.cells[y as usize * self.width as usize + x as usize]
    }

    /// Writes one grapheme of display width `w` at (`x`, `y`). Clipped at the right edge:
    /// a wide grapheme that doesn't fit is drawn as spaces.
    fn put(&mut self, x: usize, y: usize, symbol: &str, w: usize, role: Role) {
        let width = self.width as usize;
        if y >= self.height as usize || x >= width {
            return;
        }
        let row = y * width;
        if w == 0 {
            return;
        }
        if x + w > width {
            for cx in x..width {
                self.cells[row + cx] = Cell {
                    symbol: " ".into(),
                    role,
                };
            }
            return;
        }
        self.cells[row + x] = Cell {
            symbol: Tendril::from(symbol),
            role,
        };
        for cx in x + 1..x + w {
            self.cells[row + cx] = Cell {
                symbol: Tendril::new(),
                role,
            };
        }
    }

    /// Writes a string from column `x`, stopping before `limit`. Returns the column after it.
    fn put_str(&mut self, x: usize, y: usize, s: &str, limit: usize, role: Role) -> usize {
        let mut cx = x;
        for g in s.graphemes(true) {
            let w = display_width(g);
            if cx + w > limit {
                break;
            }
            self.put(cx, y, printable(g), w, role);
            cx += w;
        }
        cx
    }

    /// The rows as plain text, trailing spaces trimmed.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for y in 0..self.height {
            let mut line = String::new();
            for x in 0..self.width {
                line.push_str(&self.cell(x, y).symbol);
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }

    /// The rows with ANSI styling: the selection in reverse video, the status bar
    /// highlighted, and the caret as an underlined reverse cell (a snapshot has no
    /// terminal cursor).
    pub fn to_ansi(&self) -> String {
        let mut out = String::new();
        for y in 0..self.height {
            let mut current: Option<(Role, bool)> = None;
            for x in 0..self.width {
                let cell = self.cell(x, y);
                let is_cursor = self.cursor == Some((x, y));
                let key = (cell.role, is_cursor);
                if current != Some(key) {
                    out.push_str("\x1b[0m");
                    out.push_str(sgr(cell.role, is_cursor));
                    current = Some(key);
                }
                out.push_str(&cell.symbol);
            }
            out.push_str("\x1b[0m\n");
        }
        out
    }
}

fn sgr(role: Role, cursor: bool) -> &'static str {
    match (role, cursor) {
        (_, true) => "\x1b[7;4m",
        (Role::Text, _) => "",
        (Role::Selection, _) => "\x1b[30;46m",
        (Role::Status, _) => "\x1b[30;47m",
        (Role::StatusAccent, _) => "\x1b[1;30;47m",
    }
}

/// A grapheme's display width, matching the formatter's rule (at least one cell).
pub fn display_width(g: &str) -> usize {
    if g.is_empty() {
        0
    } else {
        grapheme_width(g)
    }
}

/// Control characters would drive the terminal, and zero-width clusters would take no
/// cell where the layout gives them one; both draw as a placeholder.
fn printable(g: &str) -> &str {
    use unicode_width::UnicodeWidthStr;
    if g.chars().any(|c| c.is_control()) || UnicodeWidthStr::width(g) == 0 {
        "\u{FFFD}"
    } else {
        g
    }
}

/// Renders the state. Pure: the same state always gives the same frame.
pub fn view(state: &State) -> Frame {
    let width = state.viewport.width.max(1);
    let height = state.viewport.height.max(1);
    let mut frame = Frame::new(width, height);
    let text_rows = state.text_rows();
    let layout = Layout::new(state);
    let top = layout.top(&state.scroll);
    let hscroll = if layout.wraps() { 0 } else { state.scroll.col };
    let caret = state.caret();
    let ranges: Vec<(usize, usize)> = state
        .selection
        .iter()
        .filter(|r| !r.is_empty())
        .map(|r| (r.from(), r.to()))
        .collect();
    let selected = |pos: usize| ranges.iter().any(|&(f, t)| f <= pos && pos < t);

    if text_rows > 0 {
        let formatter = layout.formatter_at_row(top);
        // Rows count from the top line's first row, virtual rows (an outline block's blank
        // row before it) included: the formatter's rows skip them.
        let mut line = top.line;
        let mut virtual_rows = layout.gap(top.line);
        for g in formatter {
            while line < g.line_idx {
                line += 1;
                virtual_rows += layout.gap(line);
            }
            let row = g.visual_pos.row + virtual_rows;
            if row < top.row {
                continue;
            }
            let y = row - top.row;
            if y >= text_rows {
                break;
            }
            if g.is_virtual() {
                continue;
            }
            let col = g.visual_pos.col as isize - hscroll as isize;
            let w = g.width();
            let role = if selected(g.char_idx) {
                Role::Selection
            } else {
                Role::Text
            };
            if g.char_idx == caret && col >= 0 && (col as usize) < width as usize {
                frame.cursor = Some((col as u16, y as u16));
            }
            if col < 0 {
                continue;
            }
            let x = col as usize;
            match &g.raw {
                Grapheme::Newline => {
                    if role == Role::Selection {
                        frame.put(x, y, " ", 1, role);
                    }
                }
                Grapheme::Tab { width: tw } => {
                    for i in 0..*tw {
                        frame.put(x + i, y, " ", 1, role);
                    }
                }
                Grapheme::Other { g: s } => {
                    if g.source.is_eof() {
                        continue;
                    }
                    frame.put(x, y, printable(s), w, role);
                }
            }
        }
    }

    if state.config.status_bar {
        draw_status(state, &mut frame, height as usize - 1);
    }
    frame
}

/// The status bar: the file name and dirty marker on the left, the message in the middle,
/// and `line:col` (plus the selection size) on the right.
fn draw_status(state: &State, frame: &mut Frame, y: usize) {
    let width = frame.width as usize;
    for x in 0..width {
        frame.put(x, y, " ", 1, Role::Status);
    }
    let text = state.text.slice(..);
    let caret = state.caret();
    let line = text.char_to_line(caret);
    let line_start = text.line_to_char(line);
    let prefix = text.slice(line_start..caret);
    // Within a line, ASCII text has one grapheme per char (CRLF only ends a line), so a
    // long ASCII line needs no segmentation.
    let col = if prefix.len_bytes() == prefix.len_chars() {
        prefix.len_chars()
    } else {
        prefix.to_string().graphemes(true).count()
    };
    let selected: usize = state.selection.iter().map(|r| r.len()).sum();
    let mut right = format!("{}:{} ", line + 1, col + 1);
    if selected > 0 {
        right = format!("{selected} sel  {right}");
    }
    let right_w: usize = right.graphemes(true).map(display_width).sum();
    let right_x = width.saturating_sub(right_w);
    frame.put_str(right_x, y, &right, width, Role::Status);

    let mut x = frame.put_str(1, y, &state.name(), right_x.saturating_sub(1), Role::Status);
    if state.dirty {
        x = frame.put_str(x, y, " [+]", right_x.saturating_sub(1), Role::StatusAccent);
    }
    if let Some(msg) = &state.status {
        frame.put_str(x + 2, y, msg, right_x.saturating_sub(1), Role::Status);
    }
}
