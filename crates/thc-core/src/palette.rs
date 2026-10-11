//! The colour tokens and their hexes (tui-handoff.md §1.1, thought-control.md §3.1): the one
//! table every front end reads. thc-tui turns them into ratatui styles (plus modifiers and its
//! ANSI and 256-colour forms); thc-screen turns them into fictty themes.

/// A semantic colour. Names are the design's (`overdue-tint`), see [`Token::name`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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

impl Token {
    pub const ALL: [Token; 21] = [
        Token::Bg,
        Token::Surface,
        Token::Raised,
        Token::Selection,
        Token::Line,
        Token::Text,
        Token::Muted,
        Token::Accent,
        Token::Overdue,
        Token::Today,
        Token::Done,
        Token::Doing,
        Token::Waiting,
        Token::Agent,
        Token::Conflict,
        Token::Tag,
        Token::Link,
        Token::OverdueTint,
        Token::ConflictTint,
        Token::AgentTint,
        Token::AccentTint,
    ];

    /// The design's name for the token (`overdue-tint`).
    pub fn name(self) -> &'static str {
        match self {
            Token::Bg => "bg",
            Token::Surface => "surface",
            Token::Raised => "raised",
            Token::Selection => "selection",
            Token::Line => "line",
            Token::Text => "text",
            Token::Muted => "muted",
            Token::Accent => "accent",
            Token::Overdue => "overdue",
            Token::Today => "today",
            Token::Done => "done",
            Token::Doing => "doing",
            Token::Waiting => "waiting",
            Token::Agent => "agent",
            Token::Conflict => "conflict",
            Token::Tag => "tag",
            Token::Link => "link",
            Token::OverdueTint => "overdue-tint",
            Token::ConflictTint => "conflict-tint",
            Token::AgentTint => "agent-tint",
            Token::AccentTint => "accent-tint",
        }
    }
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

/// A token's hex in a palette, dark or light. Every token has one: `waiting` is `muted`, `link`
/// is `text` (its underline is a modifier, not a colour).
pub fn hex(palette: Palette, dark: bool, t: Token) -> &'static str {
    let (d, l) = match palette {
        Palette::Ember => ember(t),
        Palette::Redacted => redacted(t),
    };
    if dark { d } else { l }
}

/// ember-dark / ember-light (tui-handoff.md §1.1).
fn ember(t: Token) -> (&'static str, &'static str) {
    match t {
        Token::Bg => ("#1B1916", "#F7F3EC"),
        Token::Surface => ("#24211D", "#EFE9DE"),
        Token::Raised => ("#2C2824", "#FFFDF8"),
        Token::Selection => ("#352E28", "#EDE1CF"),
        Token::Line => ("#3A352F", "#DDD4C6"),
        Token::Text | Token::Link => ("#ECE5D8", "#29241F"),
        Token::Muted | Token::Waiting => ("#A0968A", "#655C51"),
        Token::Accent => ("#E8834F", "#A8461A"),
        Token::Overdue => ("#F2766E", "#B3262F"),
        Token::Today => ("#E2B54A", "#7F5B00"),
        Token::Done => ("#8FC487", "#3A7036"),
        Token::Doing => ("#6CC5CE", "#1C6C77"),
        Token::Agent => ("#88B1EC", "#2D5FA3"),
        Token::Conflict => ("#D49AD0", "#874083"),
        Token::Tag => ("#CCA884", "#77583A"),
        Token::OverdueTint => ("#3A2220", "#F6E1DC"),
        Token::ConflictTint => ("#36233A", "#F2E1F0"),
        Token::AgentTint => ("#1F2A3A", "#E2E9F4"),
        Token::AccentTint => ("#3A281E", "#F5E2D6"),
    }
}

/// Redacted (dark) and newsprint (light), thought-control.md §3.1. `today` and `link` aren't in
/// its table: today keeps ember's hue, link is text.
fn redacted(t: Token) -> (&'static str, &'static str) {
    match t {
        Token::Bg => ("#0C0C0B", "#ECE8DF"),
        Token::Surface => ("#161614", "#E2DDD1"),
        Token::Raised => ("#1F1F1C", "#F5F2EA"),
        Token::Selection => ("#26261F", "#DAD4C4"),
        Token::Line => ("#34332F", "#CFC9BC"),
        Token::Text | Token::Link => ("#ECE8DF", "#0C0C0B"),
        Token::Muted | Token::Waiting => ("#9C978C", "#544F47"),
        Token::Accent => ("#E4FF3A", "#4A5700"),
        Token::Overdue => ("#FF5A3C", "#9E280B"),
        Token::Today => ("#E2B54A", "#6B4D00"),
        Token::Done => ("#5FCB8A", "#16614A"),
        Token::Doing => ("#4FC3D9", "#0C5B68"),
        Token::Agent => ("#8AA9FF", "#284CA6"),
        Token::Conflict => ("#E08BE0", "#7E347E"),
        Token::Tag => ("#CBBF9F", "#62502A"),
        Token::OverdueTint => ("#3A1712", "#F5DCD3"),
        Token::ConflictTint => ("#34203A", "#EEDDEE"),
        Token::AgentTint => ("#161E33", "#DCE3F3"),
        Token::AccentTint => ("#23260F", "#E6EDB5"),
    }
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
    pub fn hexes(self) -> ((&'static str, &'static str), (&'static str, &'static str)) {
        let tint = |t| (ember(t).0, ember(t).1);
        match self {
            Accent::Ember => (ember(Token::Accent), tint(Token::AccentTint)),
            Accent::Rose => (("#E8769F", "#A33462"), ("#2F2529", "#F3E3E8")),
            Accent::Sea => (("#4DBFA9", "#17705F"), ("#1D3330", "#D7EEE8")),
            Accent::Iris => (("#9D8FF2", "#5340B0"), ("#2B2836", "#E6E3EF")),
            // Graphite is ember's text on ember's selection.
            Accent::Graphite => (ember(Token::Text), ember(Token::Selection)),
        }
    }

    /// xterm-256 indices (dark, light) for the accent and its tint.
    pub fn indexed(self) -> ((u8, u8), (u8, u8)) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_token_has_a_hex_in_every_palette_and_a_unique_name() {
        let mut names: Vec<&str> = Token::ALL.iter().map(|t| t.name()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), Token::ALL.len());
        for p in [Palette::Ember, Palette::Redacted] {
            for dark in [true, false] {
                for t in Token::ALL {
                    let h = hex(p, dark, t);
                    assert!(h.len() == 7 && h.starts_with('#'), "{p:?} {t:?} {h}");
                }
            }
        }
        assert_eq!(hex(Palette::Ember, true, Token::Bg), "#1B1916");
        assert_eq!(Accent::Ember.hexes().0, ("#E8834F", "#A8461A"));
        assert_eq!(Accent::Graphite.hexes(), (("#ECE5D8", "#29241F"), ("#352E28", "#EDE1CF")));
    }
}
