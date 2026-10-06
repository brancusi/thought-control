//! Styled snapshot output for design review: `THC_TUI_SNAPSHOT_FORMAT=ansi|html`.

use crate::theme::{Mode, Theme};
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use unicode_width::UnicodeWidthStr;

fn sgr_color(c: Color, bg: bool) -> String {
    let base = if bg { 40 } else { 30 };
    match c {
        Color::Reset => (if bg { 49 } else { 39 }).to_string(),
        Color::Black => base.to_string(),
        Color::Red => (base + 1).to_string(),
        Color::Green => (base + 2).to_string(),
        Color::Yellow => (base + 3).to_string(),
        Color::Blue => (base + 4).to_string(),
        Color::Magenta => (base + 5).to_string(),
        Color::Cyan => (base + 6).to_string(),
        Color::Gray => (base + 7).to_string(),
        Color::DarkGray => (base + 60).to_string(),
        Color::LightRed => (base + 61).to_string(),
        Color::LightGreen => (base + 62).to_string(),
        Color::LightYellow => (base + 63).to_string(),
        Color::LightBlue => (base + 64).to_string(),
        Color::LightMagenta => (base + 65).to_string(),
        Color::LightCyan => (base + 66).to_string(),
        Color::White => (base + 67).to_string(),
        Color::Rgb(r, g, b) => format!("{};2;{r};{g};{b}", if bg { 48 } else { 38 }),
        Color::Indexed(i) => format!("{};5;{i}", if bg { 48 } else { 38 }),
    }
}

/// ANSI escape sequences, one line per row, reset at each line end.
pub fn ansi(buf: &Buffer) -> String {
    let mut out = String::new();
    for y in 0..buf.area.height {
        let mut last: Option<(Color, Color, Modifier)> = None;
        let mut skip = 0;
        for x in 0..buf.area.width {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let cell = &buf[(x, y)];
            let style = (cell.fg, cell.bg, cell.modifier);
            if last != Some(style) {
                let mut codes = vec!["0".to_string()];
                for (m, code) in [(Modifier::BOLD, "1"), (Modifier::DIM, "2"), (Modifier::ITALIC, "3"), (Modifier::UNDERLINED, "4"), (Modifier::REVERSED, "7"), (Modifier::CROSSED_OUT, "9")] {
                    if cell.modifier.contains(m) {
                        codes.push(code.into());
                    }
                }
                if cell.fg != Color::Reset {
                    codes.push(sgr_color(cell.fg, false));
                }
                if cell.bg != Color::Reset {
                    codes.push(sgr_color(cell.bg, true));
                }
                out.push_str(&format!("\x1b[{}m", codes.join(";")));
                last = Some(style);
            }
            let sym = cell.symbol();
            skip = UnicodeWidthStr::width(sym).saturating_sub(1);
            out.push_str(sym);
        }
        out.push_str("\x1b[0m\n");
    }
    out
}

fn css_color(c: Color, dark: bool) -> Option<String> {
    // A neutral xterm-like palette for named colors (only used by the ansi theme).
    let named = |d: &str, l: &str| Some(if dark { d } else { l }.to_string());
    match c {
        Color::Reset => None,
        Color::Rgb(r, g, b) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
        // xterm-256: the 6x6x6 cube and the grey ramp (the themes use only those).
        Color::Indexed(i @ 16..=231) => {
            let lv = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            let n = i - 16;
            Some(format!("#{:02x}{:02x}{:02x}", lv(n / 36), lv((n / 6) % 6), lv(n % 6)))
        }
        Color::Indexed(i @ 232..=255) => {
            let g = 8 + (i - 232) * 10;
            Some(format!("#{g:02x}{g:02x}{g:02x}"))
        }
        Color::Red => named("#e06c75", "#b3262f"),
        Color::Green => named("#98c379", "#3a7036"),
        Color::Yellow => named("#e5c07b", "#7f5b00"),
        Color::Blue | Color::LightBlue => named("#88b1ec", "#2d5fa3"),
        Color::Magenta => named("#d49ad0", "#874083"),
        Color::Cyan => named("#6cc5ce", "#1c6c77"),
        Color::DarkGray => named("#7f7f7f", "#7f7f7f"),
        Color::White | Color::Gray => named("#ece5d8", "#29241f"),
        Color::Black => named("#1b1916", "#1b1916"),
        _ => None,
    }
}

/// A self-contained HTML page with one span per style run.
pub fn html(buf: &Buffer, theme: &Theme) -> String {
    let dark = !matches!(theme.mode, Mode::Truecolor { dark: false } | Mode::Indexed { dark: false });
    let (bg, fg) = if dark { ("#1B1916", "#ECE5D8") } else { ("#F7F3EC", "#29241F") };
    let mut out = format!(
        "<!doctype html><meta charset=utf-8><title>thc snapshot</title>\n<style>body{{margin:0;background:{bg}}}pre{{margin:16px;font:13px/1.35 'IBM Plex Mono',ui-monospace,Menlo,monospace;color:{fg}}}</style>\n<pre>"
    );
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    for y in 0..buf.area.height {
        let mut run = String::new();
        let mut run_style: Option<(Color, Color, Modifier)> = None;
        let mut skip = 0;
        let flush = |run: &mut String, st: Option<(Color, Color, Modifier)>, out: &mut String| {
            if run.is_empty() {
                return;
            }
            let Some((cf, cb, m)) = st else { return };
            let (mut cf, mut cb) = (css_color(cf, dark), css_color(cb, dark));
            if m.contains(Modifier::REVERSED) {
                let f2 = cf.clone().unwrap_or(fg.to_string());
                let b2 = cb.clone().unwrap_or(bg.to_string());
                cf = Some(b2);
                cb = Some(f2);
            }
            let mut css = Vec::new();
            if let Some(c) = cf {
                css.push(format!("color:{c}"));
            }
            if let Some(c) = cb {
                css.push(format!("background:{c}"));
            }
            if m.contains(Modifier::BOLD) {
                css.push("font-weight:700".into());
            }
            if m.contains(Modifier::DIM) {
                css.push("opacity:.62".into());
            }
            let mut deco = Vec::new();
            if m.contains(Modifier::UNDERLINED) {
                deco.push("underline");
            }
            if m.contains(Modifier::CROSSED_OUT) {
                deco.push("line-through");
            }
            if !deco.is_empty() {
                css.push(format!("text-decoration:{}", deco.join(" ")));
            }
            if css.is_empty() {
                out.push_str(&esc(run));
            } else {
                out.push_str(&format!("<span style=\"{}\">{}</span>", css.join(";"), esc(run)));
            }
            run.clear();
        };
        for x in 0..buf.area.width {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let cell = &buf[(x, y)];
            let st = Some((cell.fg, cell.bg, cell.modifier));
            if st != run_style {
                flush(&mut run, run_style, &mut out);
                run_style = st;
            }
            let sym = cell.symbol();
            skip = UnicodeWidthStr::width(sym).saturating_sub(1);
            run.push_str(sym);
        }
        flush(&mut run, run_style, &mut out);
        out.push('\n');
    }
    out.push_str("</pre>\n");
    out
}
