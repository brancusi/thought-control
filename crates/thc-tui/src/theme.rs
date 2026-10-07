//! Semantic tokens → ratatui styles, per docs/design/tui-handoff.md §1–2.
//! Themes: `ember-dark` / `ember-light` (truecolor), `redacted` / `newsprint` (Thought Control,
//! thought-control.md §3.1), their `-256` variants, and `ansi` (named colors only; the
//! terminal's palette decides hues). `NO_COLOR` strips colors but keeps modifiers.

use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Token {
    Bg,
    Surface,
    Raised,
    Selection,
    Line,
    Text,
    Muted,
    Accent,
    Overdue,
    Today,
    Done,
    Doing,
    Waiting,
    Agent,
    Conflict,
    Tag,
    Link,
    OverdueTint,
    ConflictTint,
    AgentTint,
    AccentTint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Ansi { no_color: bool },
    Truecolor { dark: bool },
    /// 256 colours (tui-handoff.md §1.4): ember with each token at its xterm-256 index.
    Indexed { dark: bool },
}

/// The colour set a truecolor or 256-colour theme draws with: ember, or redacted (dark) and
/// newsprint (light). Status meanings are the same in both; the grounds and the accent move.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Palette {
    #[default]
    Ember,
    Redacted,
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

/// A vault's accent (vaults.md §10.1). Ember is the home vault's, always.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Accent {
    #[default]
    Ember,
    Rose,
    Sea,
    Iris,
    Graphite,
}

impl Accent {
    pub fn parse(s: &str) -> Accent {
        match s {
            "rose" => Accent::Rose,
            "sea" => Accent::Sea,
            "iris" => Accent::Iris,
            "graphite" => Accent::Graphite,
            _ => Accent::Ember,
        }
    }

    /// (dark, light) hex for the accent, and for its tint.
    fn hexes(self) -> ((&'static str, &'static str), (&'static str, &'static str)) {
        match self {
            Accent::Ember => (("#E8834F", "#A8461A"), ("#3A281E", "#F5E2D6")),
            Accent::Rose => (("#E8769F", "#A33462"), ("#2F2529", "#F3E3E8")),
            Accent::Sea => (("#4DBFA9", "#17705F"), ("#1D3330", "#D7EEE8")),
            Accent::Iris => (("#9D8FF2", "#5340B0"), ("#2B2836", "#E6E3EF")),
            Accent::Graphite => (("#ECE5D8", "#29241F"), ("#352E28", "#EDE1CF")),
        }
    }

    /// xterm-256 indices (dark, light) for the accent and its tint.
    fn indexed(self) -> ((u8, u8), (u8, u8)) {
        match self {
            Accent::Ember => ((209, 130), (94, 223)),
            Accent::Rose => ((175, 131), (236, 254)),
            Accent::Sea => ((73, 23), (23, 194)),
            Accent::Iris => ((141, 61), (60, 183)),
            Accent::Graphite => ((254, 235), (237, 253)),
        }
    }

    /// The caret colour for OSC 12, `#rrggbb`.
    pub fn caret_hex(self, dark: bool) -> &'static str {
        let ((d, l), _) = self.hexes();
        if dark { d } else { l }
    }
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
    let c = |dk: &str, light: &str| hex(if dark { dk } else { light });
    let s = Style::default();
    match t {
        Token::Bg => s.bg(c("#1B1916", "#F7F3EC")),
        Token::Surface => s.bg(c("#24211D", "#EFE9DE")),
        Token::Raised => s.bg(c("#2C2824", "#FFFDF8")),
        Token::Selection => s.bg(c("#352E28", "#EDE1CF")),
        Token::Line => s.fg(c("#3A352F", "#DDD4C6")),
        Token::Text | Token::Link => {
            let st = s.fg(c("#ECE5D8", "#29241F"));
            if t == Token::Link { st.add_modifier(Modifier::UNDERLINED) } else { st }
        }
        Token::Muted | Token::Waiting => s.fg(c("#A0968A", "#655C51")),
        Token::Accent => s.fg(c("#E8834F", "#A8461A")),
        Token::Overdue => s.fg(c("#F2766E", "#B3262F")),
        Token::Today => s.fg(c("#E2B54A", "#7F5B00")),
        Token::Done => s.fg(c("#8FC487", "#3A7036")),
        Token::Doing => s.fg(c("#6CC5CE", "#1C6C77")),
        Token::Agent => s.fg(c("#88B1EC", "#2D5FA3")),
        Token::Conflict => s.fg(c("#D49AD0", "#874083")).add_modifier(Modifier::BOLD),
        Token::Tag => s.fg(c("#CCA884", "#77583A")),
        Token::OverdueTint => s.bg(c("#3A2220", "#F6E1DC")),
        Token::ConflictTint => s.bg(c("#36233A", "#F2E1F0")),
        Token::AgentTint => s.bg(c("#1F2A3A", "#E2E9F4")),
        Token::AccentTint => s.bg(c("#3A281E", "#F5E2D6")),
    }
}

/// Redacted (dark) and newsprint (light), thought-control.md §3.1. `today` and `link` aren't in
/// its table: today keeps ember's hue, link is text underlined.
fn redacted(t: Token, dark: bool) -> Style {
    let c = |dk: &str, light: &str| hex(if dark { dk } else { light });
    let s = Style::default();
    match t {
        Token::Bg => s.bg(c("#0C0C0B", "#ECE8DF")),
        Token::Surface => s.bg(c("#161614", "#E2DDD1")),
        Token::Raised => s.bg(c("#1F1F1C", "#F5F2EA")),
        Token::Selection => s.bg(c("#26261F", "#DAD4C4")),
        Token::Line => s.fg(c("#34332F", "#CFC9BC")),
        Token::Text | Token::Link => {
            let st = s.fg(c("#ECE8DF", "#0C0C0B"));
            if t == Token::Link { st.add_modifier(Modifier::UNDERLINED) } else { st }
        }
        Token::Muted | Token::Waiting => s.fg(c("#9C978C", "#544F47")),
        Token::Accent => s.fg(c("#E4FF3A", "#4A5700")),
        Token::Overdue => s.fg(c("#FF5A3C", "#9E280B")),
        Token::Today => s.fg(c("#E2B54A", "#6B4D00")),
        Token::Done => s.fg(c("#5FCB8A", "#16614A")),
        Token::Doing => s.fg(c("#4FC3D9", "#0C5B68")),
        Token::Agent => s.fg(c("#8AA9FF", "#284CA6")),
        Token::Conflict => s.fg(c("#E08BE0", "#7E347E")).add_modifier(Modifier::BOLD),
        Token::Tag => s.fg(c("#CBBF9F", "#62502A")),
        Token::OverdueTint => s.bg(c("#3A1712", "#F5DCD3")),
        Token::ConflictTint => s.bg(c("#34203A", "#EEDDEE")),
        Token::AgentTint => s.bg(c("#161E33", "#DCE3F3")),
        Token::AccentTint => s.bg(c("#23260F", "#E6EDB5")),
    }
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

pub struct Glyphs {
    pub brand_l: &'static str,
    pub brand_dot: &'static str,
    pub brand_r: &'static str,
    pub cursor: &'static str,
    pub live: &'static str,
    pub conflict: &'static str,
    pub agent: &'static str,
    pub agent_sep: &'static str,
    pub repeat: &'static str,
    pub alert: &'static str,
    pub page: &'static str,
    pub journal: &'static str,
    pub sep: &'static str,
    pub note: &'static str,
    pub fold_closed: &'static str,
    pub fold_open: &'static str,
    pub backlink: &'static str,
    pub arrow: &'static str,
    pub prompt: &'static str,
    pub ellipsis: &'static str,
    pub daemon_live: &'static str,
    pub daemon_local: &'static str,
    pub rule: &'static str,
    pub rule_heavy: &'static str,
    pub vsep: &'static str,
    pub tee: &'static str,
}

pub static UNICODE: Glyphs = Glyphs {
    brand_l: "[",
    brand_dot: "•",
    brand_r: "]",
    cursor: "▌",
    live: "•",
    conflict: "≠",
    agent: "◆",
    agent_sep: " ",
    repeat: "↻",
    alert: "◎",
    page: "¶",
    journal: "§",
    sep: "·",
    note: "·",
    fold_closed: "▸",
    fold_open: "▾",
    backlink: "←",
    arrow: "→",
    prompt: "›",
    ellipsis: "…",
    daemon_live: "●",
    daemon_local: "○",
    rule: "─",
    rule_heavy: "━",
    vsep: "│",
    tee: "┬",
};

pub static ASCII: Glyphs = Glyphs {
    brand_l: "[",
    brand_dot: "*",
    brand_r: "]",
    cursor: ">",
    live: "*",
    conflict: "!=",
    agent: "@",
    agent_sep: "",
    repeat: "~",
    alert: "(o)",
    page: "P",
    journal: "J",
    sep: "-",
    note: "-",
    fold_closed: "+",
    fold_open: "-",
    backlink: "<-",
    arrow: "->",
    prompt: ">",
    ellipsis: "~",
    daemon_live: "*",
    daemon_local: "o",
    rule: "-",
    rule_heavy: "=",
    vsep: "|",
    tee: "+",
};

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
