//! The terminal backend, quieter (jank C1–C3): ratatui shows and moves the cursor on every
//! draw, even when no cell changed. Idle redraws then restart the cursor's blink or flash it,
//! and every repaint goes out unsynchronized. This wrapper:
//! - skips cursor show/hide/move when nothing was drawn and the cursor is already there;
//! - brackets a frame that drew cells in synchronized output (`CSI ?2026h` … `l`), so a
//!   terminal that has it (WezTerm, kitty, Ghostty, iTerm2, tmux 3.4) shows the frame whole;
//! - sets the cursor shape on request (a bar while writing, the person's own shape otherwise).

use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::crossterm::{cursor::SetCursorStyle, queue, terminal};
use ratatui::layout::{Position, Size};
use std::io::Stdout;

pub struct Quiet {
    inner: CrosstermBackend<Stdout>,
    /// Cells went out since the last flush (so the cursor moved and must be put back).
    drew: bool,
    synced: bool,
    cursor: Option<Position>,
    visible: Option<bool>,
    bar: Option<bool>,
}

impl Quiet {
    /// The terminal itself, for what isn't cells (inline images).
    pub fn raw(&mut self) -> &mut CrosstermBackend<Stdout> {
        &mut self.inner
    }

    pub fn new(out: Stdout) -> Quiet {
        Quiet { inner: CrosstermBackend::new(out), drew: false, synced: false, cursor: None, visible: None, bar: None }
    }

    /// A bar cursor while writing, the terminal's own shape otherwise (sent only on change).
    pub fn cursor_bar(&mut self, bar: bool) -> std::io::Result<()> {
        if self.bar != Some(bar) {
            self.bar = Some(bar);
            let style = if bar { SetCursorStyle::SteadyBar } else { SetCursorStyle::DefaultUserShape };
            queue!(self.inner, style)?;
            Backend::flush(&mut self.inner)?;
        }
        Ok(())
    }

}

impl Backend for Quiet {
    type Error = std::io::Error;

    fn draw<'a, I>(&mut self, content: I) -> std::io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let mut content = content.peekable();
        if content.peek().is_none() {
            return Ok(());
        }
        if !self.synced {
            queue!(self.inner, terminal::BeginSynchronizedUpdate)?;
            self.synced = true;
        }
        self.drew = true;
        self.cursor = None;
        self.inner.draw(content)
    }

    fn append_lines(&mut self, n: u16) -> std::io::Result<()> {
        self.cursor = None;
        self.inner.append_lines(n)
    }

    fn hide_cursor(&mut self) -> std::io::Result<()> {
        if self.visible == Some(false) {
            return Ok(());
        }
        self.visible = Some(false);
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> std::io::Result<()> {
        // Drawing cells moves the cursor but never hides it.
        if self.visible == Some(true) {
            return Ok(());
        }
        self.visible = Some(true);
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> std::io::Result<Position> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> std::io::Result<()> {
        let p = position.into();
        if self.cursor == Some(p) && !self.drew {
            return Ok(());
        }
        self.cursor = Some(p);
        self.inner.set_cursor_position(p)
    }

    fn clear(&mut self) -> std::io::Result<()> {
        self.cursor = None;
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> std::io::Result<()> {
        self.cursor = None;
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> std::io::Result<Size> {
        self.inner.size()
    }

    fn window_size(&mut self) -> std::io::Result<WindowSize> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.synced {
            queue!(self.inner, terminal::EndSynchronizedUpdate)?;
            self.synced = false;
        }
        self.drew = false;
        Backend::flush(&mut self.inner)
    }
}

/// Snapshots: a TestBackend that remembers whether the cursor is shown, so the cursor marker
/// (`THC_TUI_SNAPSHOT_CURSOR`) is drawn only where a person would see a cursor.
pub struct Snap {
    pub inner: ratatui::backend::TestBackend,
    pub visible: bool,
}

impl Backend for Snap {
    type Error = core::convert::Infallible;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.inner.draw(content)
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.visible = false;
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.visible = true;
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> Result<Size, Self::Error> {
        self.inner.size()
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush()
    }
}
