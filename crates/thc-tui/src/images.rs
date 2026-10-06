//! Inline images under attachment chips (attachments.md §3): the iTerm2 protocol (WezTerm,
//! iTerm2) or kitty's graphics protocol (kitty, Ghostty), drawn after the frame into rows the
//! layout reserved, so text never reflows while you type. Elsewhere, over SSH, or with
//! `[tui] images = "chips"`, only the chip shows.

use std::io::Write;

/// How this terminal draws images.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proto {
    /// OSC 1337 File=…: WezTerm and iTerm2.
    Iterm,
    /// The kitty graphics protocol (APC G): kitty and Ghostty.
    Kitty,
}

/// One image to draw: where (cells), how big (cells), and the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub x: u16,
    pub y: u16,
    pub cols: u16,
    pub rows: u16,
    pub path: std::path::PathBuf,
}

/// The protocol, from `[tui] images` (auto, chips, off) and the terminal. `auto` is chips over
/// SSH (images cost bandwidth) and in snapshots.
pub fn proto() -> Option<Proto> {
    // Forced (tests, or a terminal thc doesn't recognise): THC_TUI_IMAGES=iterm|kitty.
    match std::env::var("THC_TUI_IMAGES").as_deref() {
        Ok("iterm") => return Some(Proto::Iterm),
        Ok("kitty") => return Some(Proto::Kitty),
        _ => {}
    }
    // Snapshots draw text only, whatever terminal runs them.
    if crate::SNAPSHOT.with(|s| s.get()) {
        return None;
    }
    let setting = thc_core::settings::current().str("tui.images").unwrap_or("auto").to_string();
    let setting = std::env::var("THC_TUI_IMAGES").unwrap_or(setting);
    if setting == "chips" || setting == "off" {
        return None;
    }
    if setting == "auto" && (std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some()) {
        return None;
    }
    let tp = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let term = std::env::var("TERM").unwrap_or_default();
    match tp.as_str() {
        "WezTerm" | "iTerm.app" => return Some(Proto::Iterm),
        "ghostty" => return Some(Proto::Kitty),
        _ => {}
    }
    if term == "xterm-kitty" || std::env::var_os("KITTY_WINDOW_ID").is_some() {
        return Some(Proto::Kitty);
    }
    if std::env::var_os("WEZTERM_EXECUTABLE").is_some() || std::env::var_os("WEZTERM_PANE").is_some() {
        return Some(Proto::Iterm);
    }
    None
}

/// The cells an image of `w`×`h` pixels takes in a column `max_cols` wide: at most 16 rows,
/// cells counted twice as tall as wide.
pub fn cells(w: u32, h: u32, max_cols: u16) -> (u16, u16) {
    if w == 0 || h == 0 || max_cols == 0 {
        return (0, 0);
    }
    let mut cols = max_cols as f64;
    let mut rows = (cols * h as f64 / w as f64 / 2.0).ceil();
    if rows > 16.0 {
        rows = 16.0;
        cols = (rows * 2.0 * w as f64 / h as f64).ceil().min(max_cols as f64);
    }
    (cols.max(1.0) as u16, rows.max(1.0) as u16)
}

/// Draw the images at their places (after the frame). Files are read once and kept encoded.
pub fn draw(out: &mut impl Write, proto: Proto, places: &[Place]) -> std::io::Result<()> {
    for p in places {
        let Some(b64) = encoded(&p.path) else { continue };
        write!(out, "\x1b7\x1b[{};{}H", p.y + 1, p.x + 1)?;
        match proto {
            Proto::Iterm => {
                write!(out, "\x1b]1337;File=inline=1;width={};height={};preserveAspectRatio=1:{}\x07", p.cols, p.rows, b64)?;
            }
            Proto::Kitty => {
                // Transmit and display a PNG in chunks of 4096, sized in cells; no cursor move.
                let chunks: Vec<&[u8]> = b64.as_bytes().chunks(4096).collect();
                for (i, c) in chunks.iter().enumerate() {
                    let more = if i + 1 < chunks.len() { 1 } else { 0 };
                    if i == 0 {
                        write!(out, "\x1b_Gf=100,a=T,q=2,C=1,c={},r={},m={more};", p.cols, p.rows)?;
                    } else {
                        write!(out, "\x1b_Gm={more};")?;
                    }
                    out.write_all(c)?;
                    write!(out, "\x1b\\")?;
                }
            }
        }
        write!(out, "\x1b8")?;
    }
    out.flush()
}

/// Kitty keeps images until deleted: remove them all before drawing again.
pub fn clear_kitty(out: &mut impl Write) -> std::io::Result<()> {
    write!(out, "\x1b_Ga=d,q=2\x1b\\")?;
    out.flush()
}

fn encoded(path: &std::path::Path) -> Option<String> {
    thread_local! {
        static CACHE: std::cell::RefCell<std::collections::HashMap<std::path::PathBuf, String>> = std::cell::RefCell::new(std::collections::HashMap::new());
    }
    if let Some(hit) = CACHE.with(|c| c.borrow().get(path).cloned()) {
        return Some(hit);
    }
    let data = std::fs::read(path).ok()?;
    let b = base64(&data);
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 64 {
            c.clear();
        }
        c.insert(path.to_path_buf(), b.clone());
    });
    Some(b)
}

/// Standard base64 with padding.
pub fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_and_cells() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        // A wide screenshot at 72 columns: 72 × 20 rows would be too tall: 16 rows, narrower.
        assert_eq!(cells(1280, 720, 72), (57, 16));
        assert_eq!(cells(1280, 360, 72), (72, 11));
        assert_eq!(cells(4, 3, 72), (43, 16));
    }
}
