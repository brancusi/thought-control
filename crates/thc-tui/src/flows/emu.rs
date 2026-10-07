//! A terminal as a person's terminal shows it: the cells ratatui's diff sends, applied with a
//! real terminal's rules for wide characters. Writing over either half of a wide character
//! erases the whole of it; a wide character covers the cell to its right. The live TUI draws
//! frame after frame into one terminal, so a cell the diff forgot (a stale cell) or a wide
//! character torn by a later write shows up here and not in a fresh render.

use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Size};
use unicode_width::UnicodeWidthStr;

/// The right half of a wide character.
pub const TRAIL: &str = "\u{0}";

pub struct Emu {
    pub w: u16,
    pub h: u16,
    pub cells: Vec<String>,
    pub cursor: Position,
    pub visible: bool,
    /// Wide characters written where they don't fit (the last column).
    pub cut: Vec<(u16, u16, String)>,
}

impl Emu {
    pub fn new(w: u16, h: u16) -> Emu {
        Emu { w, h, cells: vec![" ".to_string(); w as usize * h as usize], cursor: Position::new(0, 0), visible: false, cut: Vec::new() }
    }

    fn idx(&self, x: u16, y: u16) -> usize {
        y as usize * self.w as usize + x as usize
    }

    fn width(s: &str) -> usize {
        if s == TRAIL { 0 } else { UnicodeWidthStr::width(s) }
    }

    /// One cell written as a terminal writes it.
    pub fn put(&mut self, x: u16, y: u16, sym: &str) {
        if x >= self.w || y >= self.h {
            return;
        }
        let sym = if sym.is_empty() { " " } else { sym };
        let i = self.idx(x, y);
        // Over the right half of a wide character: the whole character goes.
        if self.cells[i] == TRAIL && x > 0 {
            self.cells[i - 1] = " ".into();
        }
        // Over the left half of one: its right half goes too.
        if Self::width(&self.cells[i]) == 2 && x + 1 < self.w {
            self.cells[i + 1] = " ".into();
        }
        self.cells[i] = sym.to_string();
        if Self::width(sym) == 2 {
            if x + 1 >= self.w {
                self.cut.push((x, y, sym.to_string()));
                return;
            }
            if Self::width(&self.cells[i + 1]) == 2 && x + 2 < self.w {
                self.cells[i + 2] = " ".into();
            }
            self.cells[i + 1] = TRAIL.into();
        }
    }

    /// What a buffer looks like on a terminal that drew it whole: wide characters cover the
    /// cell to their right.
    pub fn visual(buf: &Buffer) -> Vec<String> {
        let (w, h) = (buf.area.width, buf.area.height);
        let mut out = Vec::with_capacity(w as usize * h as usize);
        for y in 0..h {
            let mut skip = 0;
            for x in 0..w {
                if skip > 0 {
                    skip -= 1;
                    out.push(TRAIL.to_string());
                    continue;
                }
                let s = buf[(x, y)].symbol();
                let s = if s.is_empty() { " " } else { s };
                skip = Self::width(s).saturating_sub(1);
                out.push(s.to_string());
            }
        }
        out
    }

    /// The cells where this terminal differs from `buf` drawn whole: (x, y, here, intended).
    pub fn diff(&self, buf: &Buffer) -> Vec<(u16, u16, String, String)> {
        let want = Self::visual(buf);
        let mut out = Vec::new();
        if (buf.area.width, buf.area.height) != (self.w, self.h) {
            return vec![(0, 0, format!("{}x{}", self.w, self.h), format!("{}x{}", buf.area.width, buf.area.height))];
        }
        for (i, (a, b)) in self.cells.iter().zip(want.iter()).enumerate() {
            if a != b {
                out.push(((i % self.w as usize) as u16, (i / self.w as usize) as u16, a.clone(), b.clone()));
            }
        }
        out
    }
}

impl Backend for Emu {
    type Error = core::convert::Infallible;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        for (x, y, c) in content {
            self.put(x, y, c.symbol());
        }
        Ok(())
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.visible = false;
        Ok(())
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.visible = true;
        Ok(())
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        Ok(self.cursor)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.cursor = position.into();
        Ok(())
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.cells.iter_mut().for_each(|c| *c = " ".into());
        Ok(())
    }

    fn clear_region(&mut self, _clear_type: ClearType) -> Result<(), Self::Error> {
        self.clear()
    }

    fn size(&self) -> Result<Size, Self::Error> {
        Ok(Size::new(self.w, self.h))
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        Ok(WindowSize { columns_rows: Size::new(self.w, self.h), pixels: Size::new(self.w * 8, self.h * 16) })
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
