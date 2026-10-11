//! Semantic tokens → ratatui styles, per docs/design/tui-handoff.md §1–2.
//! Themes: `ember-dark` / `ember-light` (truecolor), `redacted` / `newsprint` (Thought Control,
//! thought-control.md §3.1), their `-256` variants, and `ansi` (named colors only; the
//! terminal's palette decides hues). `NO_COLOR` strips colors but keeps modifiers.

use ratatui::style::{Color, Modifier, Style};
pub use thc_core::palette::{Accent, Palette, Token};
use thc_core::palette;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Ansi { no_color: bool },
    Truecolor { dark: bool },
    /// 256 colours (tui-handoff.md §1.4): ember with each token at its xterm-256 index.
    Indexed { dark: bool },
}

/// Serializable: a UI trace records the theme its frames were drawn in (session.rs `env`).
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct Theme {
    pub mode: Mode,
    pub palette: Palette,
    pub ascii: bool,
    no_dim: bool,
    /// The vault's colour (vaults.md §10): what ember means "here" everywhere. Status colours
    /// never change.
    pub accent: Accent,
}

fn hex(s: &str) -> Color {
    let v = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap_or(0);
    Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

impl Theme {
    /// `THC_THEME` = auto | ember-dark | ember-light | ember-dark-256 | ember-light-256 | redacted
    /// | newsprint | redacted-256 | newsprint-256 | ansi;
    /// `THC_GLYPHS` = auto | unicode | ascii. Auto: truecolor when COLORTERM says so, else 256
    /// colours when TERM does, else 16 (§1.2).
    pub fn detect() -> Theme {
        let env = |k: &str| std::env::var(k).unwrap_or_default();
        let no_color = std::env::var_os("NO_COLOR").is_some();
        let truecolor = matches!(env("COLORTERM").as_str(), "truecolor" | "24bit");
        let c256 = env("TERM").contains("256color");
        // THC_THEME for one run, else [theme] theme in the settings (a vault may set it).
        let theme_name = match env("THC_THEME") {
            t if !t.is_empty() => t,
            _ => thc_core::settings::current().str("theme.theme").unwrap_or("").to_string(),
        };
        let palette = match theme_name.as_str() {
            "redacted" | "newsprint" | "redacted-256" | "newsprint-256" => Palette::Redacted,
            _ => Palette::Ember,
        };
        let mode = match theme_name.as_str() {
            _ if no_color => Mode::Ansi { no_color: true },
            "redacted" => Mode::Truecolor { dark: true },
            "newsprint" => Mode::Truecolor { dark: false },
            "redacted-256" => Mode::Indexed { dark: true },
            "newsprint-256" => Mode::Indexed { dark: false },
            "ansi" | "16" => Mode::Ansi { no_color: false },
            "ember-light" | "light" => Mode::Truecolor { dark: false },
            "ember-dark" | "dark" | "ember" => Mode::Truecolor { dark: true },
            "ember-light-256" => Mode::Indexed { dark: false },
            "ember-dark-256" | "256" => Mode::Indexed { dark: true },
            _ if truecolor => Mode::Truecolor { dark: true },
            _ if c256 => Mode::Indexed { dark: true },
            _ => Mode::Ansi { no_color: false },
        };
        let locale = format!("{}{}{}", env("LC_ALL"), env("LC_CTYPE"), env("LANG"));
        let cjk = ["ja", "zh", "ko"].iter().any(|p| locale.starts_with(p));
        let ascii = match env("THC_GLYPHS").as_str() {
            "ascii" => true,
            "unicode" => false,
            _ => env("THC_ASCII") == "1" || cjk || (!locale.is_empty() && !locale.to_uppercase().contains("UTF-8") && !locale.to_uppercase().contains("UTF8")),
        };
        // The vault's accent: THC_ACCENT for one run, else [theme] accent in its settings.
        let eff = thc_core::settings::current();
        let accent = match env("THC_ACCENT").as_str() {
            "" => Accent::parse(eff.accent()),
            a => Accent::parse(a),
        };
        Theme { mode, palette, ascii, no_dim: env("THC_NO_DIM") == "1", accent }
    }

    pub fn is_ansi(&self) -> bool {
        matches!(self.mode, Mode::Ansi { .. })
    }

    pub fn s(&self, t: Token) -> Style {
        match self.mode {
            Mode::Ansi { no_color } => {
                let st = ansi(t, self.no_dim);
                if no_color { Style { fg: None, bg: None, ..st } } else { st }
            }
            // Redacted: its own grounds and accent; a vault with its own accent keeps it.
            Mode::Truecolor { dark } if self.palette == Palette::Redacted => {
                let st = redacted(t, dark);
                if self.accent == Accent::Ember {
                    return st;
                }
                let ((ad, al), (td, tl)) = self.accent.hexes();
                match t {
                    Token::Accent => st.fg(hex(if dark { ad } else { al })),
                    Token::AccentTint => st.bg(hex(if dark { td } else { tl })),
                    _ => st,
                }
            }
            Mode::Indexed { dark } if self.palette == Palette::Redacted => {
                let mut st = Theme { mode: Mode::Truecolor { dark }, ..*self }.s(t);
                st.fg = st.fg.map(nearest256);
                st.bg = st.bg.map(nearest256);
                st
            }
            Mode::Truecolor { dark } => {
                let st = ember(t, dark);
                let ((ad, al), (td, tl)) = self.accent.hexes();
                match t {
                    Token::Accent => st.fg(hex(if dark { ad } else { al })),
                    Token::AccentTint => st.bg(hex(if dark { td } else { tl })),
                    _ => st,
                }
            }
            Mode::Indexed { dark } => {
                let st = ember256(t, dark);
                let ((ad, al), (td, tl)) = self.accent.indexed();
                match t {
                    Token::Accent => st.fg(Color::Indexed(if dark { ad } else { al })),
                    Token::AccentTint => st.bg(Color::Indexed(if dark { td } else { tl })),
                    _ => st,
                }
            }
        }
    }

    /// Dark background (for the caret colour)?
    pub fn dark(&self) -> bool {
        matches!(self.mode, Mode::Truecolor { dark: true } | Mode::Indexed { dark: true })
    }

    /// Text + BOLD (headings, active tab, `!high`).
    pub fn strong(&self) -> Style {
        self.s(Token::Text).add_modifier(Modifier::BOLD)
    }

    /// Background fill for a tint/surface token; nothing in ANSI mode.
    pub fn fill(&self, t: Token) -> Style {
        if self.is_ansi() { Style::default() } else { Style::default().bg(self.s(t).bg.unwrap_or(Color::Reset)) }
    }

    pub fn glyphs(&self) -> &'static Glyphs {
        if self.ascii { &ASCII } else { &UNICODE }
    }
}

fn ansi(t: Token, no_dim: bool) -> Style {
    let s = Style::default();
    let dim = if no_dim { s.fg(Color::DarkGray) } else { s.add_modifier(Modifier::DIM) };
    match t {
        Token::Bg | Token::Surface | Token::Raised | Token::OverdueTint | Token::AgentTint => s,
        Token::Selection => s.add_modifier(Modifier::REVERSED),
        Token::Line | Token::Muted | Token::Waiting => dim,
        Token::Text | Token::Tag => s.fg(Color::Reset),
        Token::Accent => s.fg(Color::Reset).add_modifier(Modifier::BOLD),
        Token::Overdue => s.fg(Color::Red),
        Token::Today => s.fg(Color::Yellow),
        Token::Done => s.fg(Color::Green),
        Token::Doing => s.fg(Color::Cyan),
        Token::Agent => s.fg(Color::LightBlue),
        Token::Conflict => s.fg(Color::Magenta).add_modifier(Modifier::BOLD),
        Token::Link => s.fg(Color::Reset).add_modifier(Modifier::UNDERLINED),
        Token::ConflictTint => s.fg(Color::Magenta).add_modifier(Modifier::BOLD | Modifier::REVERSED),
        Token::AccentTint => s.add_modifier(Modifier::UNDERLINED),
    }
}

fn ember(t: Token, dark: bool) -> Style {
    from_table(Palette::Ember, t, dark)
}

/// A token's style from the shared hex table (thc_core::palette): grounds and tints are
/// backgrounds, the rest foregrounds; `link` underlines and `conflict` is bold in every palette.
fn from_table(p: Palette, t: Token, dark: bool) -> Style {
    let c = hex(palette::hex(p, dark, t));
    let s = Style::default();
    match t {
        Token::Bg | Token::Surface | Token::Raised | Token::Selection | Token::OverdueTint | Token::ConflictTint | Token::AgentTint | Token::AccentTint => s.bg(c),
        Token::Link => s.fg(c).add_modifier(Modifier::UNDERLINED),
        Token::Conflict => s.fg(c).add_modifier(Modifier::BOLD),
        _ => s.fg(c),
    }
}

/// Redacted (dark) and newsprint (light), thought-control.md §3.1. `today` and `link` aren't in
/// its table: today keeps ember's hue, link is text underlined.
fn redacted(t: Token, dark: bool) -> Style {
    from_table(Palette::Redacted, t, dark)
}

/// The nearest xterm-256 colour to an RGB one, by CIELAB distance (tui-handoff.md §1.4's rule),
/// over the 6×6×6 cube and the grey ramp (16–255; the first 16 are the terminal's to choose).
fn nearest256(c: Color) -> Color {
    let Color::Rgb(r, g, b) = c else { return c };
    let lab = |r: u8, g: u8, b: u8| {
        let lin = |v: u8| {
            let v = v as f64 / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        let (r, g, b) = (lin(r), lin(g), lin(b));
        let (x, y, z) = ((0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047, 0.2126 * r + 0.7152 * g + 0.0722 * b, (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883);
        let f = |t: f64| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
        let (fx, fy, fz) = (f(x), f(y), f(z));
        (116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz))
    };
    let target = lab(r, g, b);
    let rgb = |i: u8| -> (u8, u8, u8) {
        if i >= 232 {
            let v = 8 + (i - 232) * 10;
            (v, v, v)
        } else {
            let i = i - 16;
            let step = |k: u8| if k == 0 { 0 } else { 55 + k * 40 };
            (step(i / 36), step((i / 6) % 6), step(i % 6))
        }
    };
    let best = (16..=255u8)
        .min_by(|&a, &b| {
            let d = |i: u8| {
                let (r, g, b) = rgb(i);
                let (l, a, bb) = lab(r, g, b);
                (l - target.0).powi(2) + (a - target.1).powi(2) + (bb - target.2).powi(2)
            };
            d(a).total_cmp(&d(b))
        })
        .unwrap_or(16);
    Color::Indexed(best)
}

/// ember at 256 colours (tui-handoff.md §1.4): the nearest xterm index per token by CIELAB,
/// tints picked by hand so they don't collapse into `surface`. Modifiers as in truecolor.
fn ember256(t: Token, dark: bool) -> Style {
    let (d, l): (u8, u8) = match t {
        Token::Bg => (234, 255),
        Token::Surface => (235, 254),
        Token::Raised => (236, 231),
        Token::Selection => (237, 253),
        Token::Line => (238, 188),
        Token::Text | Token::Link => (254, 235),
        Token::Muted | Token::Waiting => (246, 59),
        Token::Accent => (209, 130),
        Token::Overdue => (210, 88),
        Token::Today => (221, 94),
        Token::Done => (150, 65),
        Token::Doing => (116, 23),
        Token::Agent => (110, 25),
        Token::Conflict => (182, 133),
        Token::Tag => (180, 95),
        Token::OverdueTint => (52, 224),
        Token::ConflictTint => (53, 225),
        Token::AgentTint => (17, 189),
        Token::AccentTint => (94, 223),
    };
    let c = Color::Indexed(if dark { d } else { l });
    let mut st = ember(t, dark);
    if st.fg.is_some() {
        st.fg = Some(c);
    }
    if st.bg.is_some() {
        st.bg = Some(c);
    }
    st
}

pub use thc_core::glyphs::{ASCII, Glyphs, UNICODE};

#[cfg(test)]
mod redacted_tests {
    use super::*;

    fn lum(c: Color) -> f64 {
        let Color::Rgb(r, g, b) = c else { panic!("{c:?}") };
        let lin = |v: u8| {
            let v = v as f64 / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    }

    fn contrast(a: Color, b: Color) -> f64 {
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// thought-control.md §3.1: every text token clears 4.5:1 on bg, surface, raised and
    /// selection, in both themes.
    #[test]
    fn redacted_and_newsprint_text_reads_on_every_ground() {
        let text = [Token::Text, Token::Muted, Token::Accent, Token::Overdue, Token::Today, Token::Done, Token::Doing, Token::Agent, Token::Conflict, Token::Tag];
        let grounds = [Token::Bg, Token::Surface, Token::Raised, Token::Selection];
        let mut low = Vec::new();
        for dark in [true, false] {
            for t in text {
                for g in grounds {
                    let (fg, bg) = (redacted(t, dark).fg.unwrap(), redacted(g, dark).bg.unwrap());
                    let r = contrast(fg, bg);
                    if r < 4.5 {
                        low.push(format!("{} {t:?} on {g:?}: {r:.2}", if dark { "redacted" } else { "newsprint" }));
                    }
                }
            }
        }
        assert!(low.is_empty(), "{low:#?}");
    }

    #[test]
    fn the_themes_are_chosen_by_name_and_fall_back_to_256_colours() {
        let t = |mode, palette| Theme { mode, palette, ascii: false, no_dim: false, accent: Accent::Ember };
        let dark = t(Mode::Truecolor { dark: true }, Palette::Redacted);
        assert_eq!(dark.s(Token::Accent).fg, Some(hex("#E4FF3A")));
        assert_eq!(t(Mode::Truecolor { dark: false }, Palette::Redacted).s(Token::Accent).fg, Some(hex("#4A5700")));
        // 256 colours: indexed, and black-ish ground maps near the bottom of the grey ramp.
        let bg = t(Mode::Indexed { dark: true }, Palette::Redacted).s(Token::Bg).bg;
        assert!(matches!(bg, Some(Color::Indexed(i)) if (232..=234).contains(&i) || i == 16), "{bg:?}");
        // A vault with its own accent keeps it under redacted.
        let rose = Theme { accent: Accent::Rose, ..dark };
        assert_eq!(rose.s(Token::Accent).fg, Some(hex("#E8769F")));
        // Ember is unchanged.
        assert_eq!(t(Mode::Truecolor { dark: true }, Palette::Ember).s(Token::Bg).bg, Some(hex("#1B1916")));
    }
}
