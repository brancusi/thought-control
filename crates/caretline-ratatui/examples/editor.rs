//! A whole editor in one file: caretline for the editing, caretline-ratatui for the drawing.
//!
//!     cargo run -p caretline-ratatui --example editor -- notes.md
//!
//! Type; Enter, ⌫, Delete, Tab / ⇧Tab, ⌃T (text → [ ] → [x]), arrows (⇧ selects), ⌥↑ / ⌥↓ move a
//! line, ⌃Z / ⌃Y undo and redo, ⌃A selects all, a click places the caret. ⌃S writes the file as
//! Markdown; ⌃Q quits.

use caretline::buffer::BlockLine;
use caretline::doc::{Doc, Rect, View};
use caretline::{Block, Command, Kind, Motion};
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Position};
use ratatui::widgets::Paragraph;

/// The host's line: just a block (a host would add its own state, an id from its store…).
#[derive(Clone, PartialEq)]
struct Line(Block<u32>);

impl std::ops::Deref for Line {
    type Target = Block<u32>;
    fn deref(&self) -> &Block<u32> {
        &self.0
    }
}

impl std::ops::DerefMut for Line {
    fn deref_mut(&mut self) -> &mut Block<u32> {
        &mut self.0
    }
}

impl BlockLine for Line {
    type Id = u32;
    fn fresh(depth: usize, kind: Kind, text: &str) -> Self {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
        Line(Block::new(N.fetch_add(1, std::sync::atomic::Ordering::Relaxed), depth, kind, text))
    }
    fn is_new(&self) -> bool {
        true
    }
    fn keep_host_state(&mut self, _: &Self) {}
    fn revive(&mut self) {}
}

/// A key as a caretline command (None: not an editing key).
fn command(code: KeyCode, m: KeyModifiers) -> Option<Command> {
    let (shift, ctrl, alt) = (m.contains(KeyModifiers::SHIFT), m.contains(KeyModifiers::CONTROL), m.contains(KeyModifiers::ALT));
    let mv = |motion| Some(Command::Move { motion, select: shift });
    match code {
        KeyCode::Enter if shift => Some(Command::SoftBreak),
        KeyCode::Enter => Some(Command::Newline),
        KeyCode::Backspace if alt => Some(Command::DeleteWordBack),
        KeyCode::Backspace => Some(Command::Backspace),
        KeyCode::Delete => Some(Command::Delete),
        KeyCode::Tab => Some(Command::Indent),
        KeyCode::BackTab => Some(Command::Outdent),
        KeyCode::Up if alt => Some(Command::MoveLine(-1)),
        KeyCode::Down if alt => Some(Command::MoveLine(1)),
        KeyCode::Left if alt => mv(Motion::WordLeft),
        KeyCode::Right if alt => mv(Motion::WordRight),
        KeyCode::Up => mv(Motion::Up),
        KeyCode::Down => mv(Motion::Down),
        KeyCode::Left => mv(Motion::Left),
        KeyCode::Right => mv(Motion::Right),
        KeyCode::Home => mv(Motion::Home),
        KeyCode::End => mv(Motion::End),
        KeyCode::PageUp => mv(Motion::Page(-10)),
        KeyCode::PageDown => mv(Motion::Page(10)),
        KeyCode::Char('t') if ctrl => Some(Command::TaskCycle),
        KeyCode::Char('z') if ctrl => Some(Command::Undo),
        KeyCode::Char('y') if ctrl => Some(Command::Redo),
        KeyCode::Char('a') if ctrl => Some(Command::SelectAll),
        KeyCode::Char('k') if ctrl => Some(Command::KillToEnd),
        KeyCode::Char('u') if ctrl => Some(Command::KillToStart),
        _ => None,
    }
}

/// The document as Markdown, every block whole.
fn markdown(doc: &Doc<Line>) -> String {
    let parts: Vec<(&Block<u32>, &str, &str)> = doc.lines().iter().map(|l| (&l.0, l.text.as_str(), "")).collect();
    caretline::markdown::to_markdown(&parts)
}

fn main() -> std::io::Result<()> {
    let path = std::env::args().nth(1);
    let mut doc = Doc::new(vec![Line::fresh(0, Kind::Para, "")]);
    let mut view = View::new(Rect { x: 0, y: 0, width: 80, height: 24 });
    view.focused = true;
    // An existing file is read the way a paste is: Markdown into blocks, in one undo step.
    if let Some(text) = path.as_deref().and_then(|p| std::fs::read_to_string(p).ok()) {
        let _ = doc.paste(&mut view, text.trim_end(), false);
        let _ = doc.apply(&mut view, Command::Move { motion: Motion::DocStart, select: false }, &|_| 80);
    }
    let mut status = String::from("⌃S save · ⌃Q quit");
    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnableMouseCapture)?;
    let result = loop {
        // Layout first: the view's rect is where it draws and how it wraps.
        let size = terminal.size()?;
        let [body, bar] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(ratatui::layout::Rect::new(0, 0, size.width, size.height));
        view.rect = Rect { x: body.x, y: body.y, width: body.width, height: body.height };
        doc.scroll_to_caret(&mut view);
        if let Err(e) = terminal.draw(|f| {
            let caret = caretline_ratatui::render(&doc, &view, f.buffer_mut(), &caretline_ratatui::Theme::default());
            f.render_widget(Paragraph::new(format!(" {}  {status}", path.as_deref().unwrap_or("(no file)"))).style(ratatui::style::Style::default().add_modifier(ratatui::style::Modifier::DIM)), bar);
            if let Some((x, y)) = caret {
                f.set_cursor_position(Position::new(x, y));
            }
        }) {
            break Err(e);
        }
        // The text column of each line: the view's width less its block's indent.
        let width = view.rect.width as usize;
        let width_of = |l: &Line| width.saturating_sub(caretline::doc::indent(&l.0)).max(1);
        match event::read()? {
            Event::Key(k) if k.kind != KeyEventKind::Release => {
                let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
                match k.code {
                    KeyCode::Char('q') if ctrl => break Ok(()),
                    KeyCode::Char('s') if ctrl => {
                        status = match &path {
                            Some(p) => match std::fs::write(p, markdown(&doc) + "\n") {
                                Ok(()) => format!("saved {p}"),
                                Err(e) => format!("not saved: {e}"),
                            },
                            None => "no file to save to: run with a path".into(),
                        };
                    }
                    KeyCode::Char(c) if !ctrl => {
                        let _ = doc.insert(&mut view, &c.to_string());
                        status.clear();
                    }
                    code => {
                        if let Some(cmd) = command(code, k.modifiers) {
                            status = match doc.apply(&mut view, cmd, &width_of) {
                                Ok((caretline::Outcome::Nothing(why), _)) => why.to_string(),
                                Ok(_) => String::new(),
                                Err(e) => format!("{e:?}"),
                            };
                        }
                    }
                }
            }
            Event::Paste(text) => {
                let _ = doc.paste(&mut view, &text, false);
            }
            Event::Mouse(m) if m.kind == MouseEventKind::Down(MouseButton::Left) => {
                if let Some(p) = caretline_ratatui::hit(&doc, &view, m.column, m.row) {
                    view.caret = p;
                    view.anchor = None;
                }
            }
            _ => {}
        }
    };
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}
