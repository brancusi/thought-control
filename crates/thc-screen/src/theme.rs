//! thc's themes as fictty themes: ember-dark, ember-light, redacted and newsprint. The hexes are
//! thc_core::palette's (the one table the TUI reads too); this file only maps them onto fictty's
//! token names and the parts of fictty's components (tui-handoff.md §1).

use fictty::model::{Style, ThemeSpec, Variant};
use std::collections::BTreeMap;
use thc_core::palette::{self, Palette, Token};

/// The themes by name, in the order `{"theme": "next"}` cycles them.
pub const NAMES: [&str; 4] = ["ember-dark", "ember-light", "redacted", "newsprint"];

fn which(name: &str) -> Option<(Palette, bool)> {
    Some(match name {
        "ember-dark" => (Palette::Ember, true),
        "ember-light" => (Palette::Ember, false),
        "redacted" => (Palette::Redacted, true),
        "newsprint" => (Palette::Redacted, false),
        _ => return None,
    })
}

/// One theme, or None for a name thc doesn't have.
pub fn theme(name: &str) -> Option<ThemeSpec> {
    let (p, dark) = which(name)?;
    let hex = |t: Token| palette::hex(p, dark, t).to_string();
    // Every design token by its own name (`overdue-tint`), so markup can say `<overdue>`.
    let mut tokens: BTreeMap<String, String> = Token::ALL.iter().map(|t| (t.name().to_string(), hex(*t))).collect();
    // fictty's own names for the screen's colours.
    for (k, t) in [("bg", Token::Bg), ("fg", Token::Text), ("border", Token::Line), ("accent", Token::Accent), ("dim", Token::Muted), ("pos", Token::Done), ("neg", Token::Overdue), ("hover", Token::Raised), ("panel", Token::Raised), ("ring", Token::Accent)] {
        tokens.insert(k.into(), hex(t));
    }
    let parts: BTreeMap<String, String> = [
        // The focused pane's cursor row on `selection`; the other pane shows no background.
        ("row.selected", "on-selection"),
        ("row.selected.blur", "text"),
        ("table.header", "muted"),
        ("tree.guide", "muted"),
        ("title", "text+b"),
        ("title.focused", "text+b"),
        ("border", "line"),
        ("border.focused", "line"),
        ("tabs.active", "text+b"),
        ("tabs.idle", "muted"),
        ("tabs.underline", "accent"),
        ("empty", "muted"),
        ("status", "muted"),
        ("status.error", "overdue+on-overdue-tint"),
        ("editor.caret", "r"),
        ("editor.selection", "on-selection"),
        ("input.placeholder", "muted"),
        ("md.heading", "text+b"),
        ("md.bullet", "muted"),
        ("md.quote", "muted+i"),
        ("md.code", "tag"),
        ("md.task", "text"),
        ("md.done", "muted+s"),
        // Toasts in the bar (§7.3): a confirmation on the bar's surface, an agent's change on
        // the agent tint, an error on the overdue tint.
        ("toast", "text+on-surface"),
        ("toast.text", "text+on-surface"),
        ("toast.agent", "text+on-agent-tint"),
        ("toast.neg", "overdue+on-overdue-tint"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let style = |fg: Option<Token>, bg: Option<Token>| Style { fg: fg.map(|t| t.name().to_string()), bg: bg.map(|t| t.name().to_string()), ..Style::default() };
    let variants: BTreeMap<String, Variant> = [
        // The bottom bar (§3.3).
        ("bar", style(Some(Token::Text), Some(Token::Surface))),
        // A focused input row: the Pages finder, Search, the Tasks filter (§5.3–5.6).
        ("input", style(Some(Token::Text), Some(Token::AccentTint))),
        // Floating panels (§7.2).
        ("overlay", style(Some(Token::Text), Some(Token::Raised))),
        // The conflict banner (§7.4).
        ("banner", style(Some(Token::Conflict), Some(Token::ConflictTint))),
    ]
    .into_iter()
    .map(|(k, s)| (k.to_string(), Variant { style: Some(s), ..Variant::default() }))
    .collect();
    Some(ThemeSpec { tokens, parts, variants })
}

/// All of thc's themes, by name.
pub fn themes() -> BTreeMap<String, ThemeSpec> {
    NAMES.iter().filter_map(|n| Some((n.to_string(), theme(n)?))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes_read_the_shared_table() {
        let t = themes();
        assert_eq!(t.len(), 4);
        assert_eq!(t["ember-dark"].tokens["overdue-tint"], palette::hex(Palette::Ember, true, Token::OverdueTint));
        assert_eq!(t["newsprint"].tokens["accent"], "#4A5700");
        assert_eq!(t["ember-light"].tokens["bg"], "#F7F3EC");
        assert!(theme("solarized").is_none());
    }
}
