//! ⌘-click, where the terminal can't say ⌘ (mouse.md "Links beside").
//!
//! The SGR mouse report (1006) has bits for ⇧ ⌥ ⌃ and none for ⌘ (super), so WezTerm and Ghostty
//! send a ⌘-click byte for byte as a plain click. In WezTerm, thc's `thc_keys.lua` (`thc setup
//! wezterm`) catches the ⌘-*release* while thc is the pane's program (a `mouse_reporting`
//! binding on `Up`; the press still reaches thc) and sends [`MARKER`] instead: kitty's encoding
//! of ⌘F35, a key no keyboard has. This turns it back into what it was: the release of the
//! press thc already has, with ⌘ held (`mods: "d"`).

use crate::session::{Mouse, MouseKind, Msg};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

/// What thc_keys.lua sends for a ⌘-release, as written in Lua: ⌘F35 in the kitty keyboard
/// protocol (code 57398, modifiers 1 + 8). The pty flow in tests/ reads it back through the
/// real terminal parser.
pub const MARKER: &str = "\\x1b[57398;9u";

/// The left button's press in progress: where the pointer is while it's held.
#[derive(Default)]
pub(crate) struct CmdRelease {
    held: Option<(u16, u16)>,
}

impl CmdRelease {
    /// The key is the marker (⌘F35).
    pub fn is_marker(&self, k: &KeyEvent) -> bool {
        k.code == KeyCode::F(35) && k.modifiers.contains(KeyModifiers::SUPER)
    }

    /// Follow the left button from the terminal's own reports.
    pub fn saw(&mut self, m: &MouseEvent) {
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left) => self.held = Some((m.column, m.row)),
            MouseEventKind::Up(_) | MouseEventKind::Down(_) => self.held = None,
            _ => {}
        }
    }

    /// The marker arrived: the held press ends where the pointer is, with ⌘. Without a press
    /// in progress it means nothing (a stray key).
    pub fn release(&mut self) -> Option<Msg> {
        let (x, y) = self.held.take()?;
        Some(Msg::Mouse { mouse: Mouse { kind: MouseKind::Up, x, y, mods: "d".into(), clicks: None } })
    }
}

/// Whether the pointer at (`x`, `y`) rests on a link's title: in the main view (its link
/// targets), or in a panel's document.
pub(crate) fn link_under(app: &mut crate::app::App, x: u16, y: u16, at: Option<&crate::ui::Click>) -> bool {
    if matches!(at, Some(crate::ui::Click::Link)) {
        return true;
    }
    let over = app.render.panel_views.iter().find(|(_, r)| x >= r.x && x < r.right() && y >= r.y && y < r.bottom()).map(|(k, _)| k.clone());
    let Some(k) = over else { return false };
    app.with_panel(&k, |a| {
        let Some((line, byte, false)) = crate::doc_ui::hit(a, x, y) else { return false };
        a.doc.as_ref().is_some_and(|d| crate::doc_app::on_link_title(&d.blocks()[line].text, byte))
    })
    .unwrap_or(false)
}

/// ⇧-click on a link, where the terminal keeps ⇧ for its own selection (Ghostty, xterm: they
/// honour XTSHIFTESCAPE). While the pointer rests on a link's title, thc asks for ⇧ with mouse
/// reports (`CSI > 1 s`); off it, it gives ⇧ back (`CSI > 0 s`), so ⇧-drag elsewhere is still
/// the terminal's own selection. WezTerm always keeps ⇧ (bypass_mouse_reporting_modifiers):
/// there ⌘-, ⌃- or middle-click open beside.
pub(crate) struct ShiftCapture {
    supported: bool,
    sent: bool,
}

impl ShiftCapture {
    pub fn detect() -> ShiftCapture {
        let term = std::env::var("TERM").unwrap_or_default();
        let program = std::env::var("TERM_PROGRAM").unwrap_or_default().to_lowercase();
        let supported = program == "ghostty" || term == "xterm-ghostty" || std::env::var_os("XTERM_VERSION").is_some();
        ShiftCapture { supported, sent: false }
    }

    /// The bytes to send for the pointer's place now (nothing when nothing changes).
    pub fn update(&mut self, on_link: bool) -> Option<&'static [u8]> {
        let want = self.supported && on_link;
        if want == self.sent {
            return None;
        }
        self.sent = want;
        Some(if want { b"\x1b[>1s" } else { b"\x1b[>0s" })
    }

    /// On the way out: ⇧ back to the terminal, if thc had it.
    pub fn release(&mut self) -> Option<&'static [u8]> {
        self.update(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyEventKind;

    fn mouse(kind: MouseEventKind, x: u16, y: u16) -> MouseEvent {
        MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE }
    }

    fn marker() -> KeyEvent {
        KeyEvent::new_with_kind(KeyCode::F(35), KeyModifiers::SUPER, KeyEventKind::Press)
    }

    #[test]
    fn the_marker_is_the_release_of_the_press_in_progress_with_cmd() {
        let mut c = CmdRelease::default();
        assert!(c.is_marker(&marker()));
        assert!(!c.is_marker(&KeyEvent::new(KeyCode::F(35), KeyModifiers::NONE)));
        c.saw(&mouse(MouseEventKind::Down(MouseButton::Left), 10, 4));
        c.saw(&mouse(MouseEventKind::Drag(MouseButton::Left), 12, 5));
        match c.release() {
            Some(Msg::Mouse { mouse: m }) => assert_eq!((m.kind, m.x, m.y, m.mods.as_str()), (MouseKind::Up, 12, 5, "d")),
            other => panic!("{other:?}"),
        }
        // Once: a second marker, or one with no press, is nothing.
        assert!(c.release().is_none());
        c.saw(&mouse(MouseEventKind::Down(MouseButton::Left), 1, 1));
        c.saw(&mouse(MouseEventKind::Up(MouseButton::Left), 1, 1));
        assert!(c.release().is_none());
    }

    #[test]
    fn shift_is_asked_for_only_over_a_link_and_only_once() {
        let mut s = ShiftCapture { supported: true, sent: false };
        assert_eq!(s.update(false), None);
        assert_eq!(s.update(true), Some(&b"\x1b[>1s"[..]));
        assert_eq!(s.update(true), None);
        assert_eq!(s.update(false), Some(&b"\x1b[>0s"[..]));
        assert_eq!(s.release(), None);
        s.update(true);
        assert_eq!(s.release(), Some(&b"\x1b[>0s"[..]));
        let mut off = ShiftCapture { supported: false, sent: false };
        assert_eq!(off.update(true), None);
    }
}
