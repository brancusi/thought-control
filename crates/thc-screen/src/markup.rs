//! thc's words as fictty template markup (`<tag>#work</>`): the spans a row's text and meta are
//! drawn with, styled by theme tokens. What the pieces are is thc_core::present's; this file only
//! says how each piece looks, so the TUI and the screen can't disagree about where a tag ends.

use thc_core::palette::Token;
use thc_core::present::{self, Piece};

/// A value made safe to read as markup: `<` is written `<<` (a literal `<`), and a tab (which
/// would right-align the rest of the line) is a space.
pub fn esc(s: &str) -> String {
    s.replace('<', "<<").replace('\t', " ")
}

/// `text` in a style (a token name, then modifiers: `text+b`).
pub fn styled(spec: &str, text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    format!("<{spec}>{}</>", esc(text))
}

pub fn tok(t: Token, text: &str) -> String {
    styled(t.name(), text)
}

/// A node's text as spans (tui-handoff.md §4): plain text in the row's own style, `#tags` in
/// `tag`, `[[` `]]` in `muted` and the title in `link` (underlined). Closed rows (done,
/// cancelled) are one plain run: the row's muted style covers tags and links too. With `terms`,
/// the words searched for are bold on `accent-tint` (§5.6).
pub fn text(s: &str, closed: bool, terms: &str) -> String {
    if closed {
        return esc(s);
    }
    present::text_pieces(s)
        .into_iter()
        .map(|(t, p)| match p {
            Piece::Text => highlight(&t, terms),
            Piece::Tag => tok(Token::Tag, &t),
            // `=` replaces the row's style: the brackets are muted even in a bold title.
            Piece::Bracket => styled("=muted", &t),
            Piece::Link => styled("link+u", &t),
        })
        .collect()
}

/// Case-insensitive occurrences of each search word (2+ chars) in bold on `accent-tint`.
fn highlight(s: &str, terms: &str) -> String {
    let words: Vec<String> = terms.split_whitespace().map(str::to_lowercase).filter(|w| w.chars().count() >= 2).collect();
    if words.is_empty() {
        return esc(s);
    }
    let lower = s.to_lowercase();
    // Lowercasing can change byte lengths; fall back to plain text when it does.
    if lower.len() != s.len() {
        return esc(s);
    }
    let mut out = String::new();
    let (mut i, mut last) = (0, 0);
    while i < s.len() {
        match words.iter().find(|w| lower[i..].starts_with(w.as_str())) {
            Some(w) => {
                out.push_str(&esc(&s[last..i]));
                out.push_str(&styled("b+on-accent-tint", &s[i..i + w.len()]));
                i += w.len();
                last = i;
            }
            None => i += s[i..].chars().next().map_or(1, char::len_utf8),
        }
    }
    out.push_str(&esc(&s[last..]));
    out
}

/// Meta items (each one or more styled parts) joined by ` · ` in `muted` (§4.1).
pub fn join(items: &[Vec<(String, String)>]) -> String {
    let sep = tok(Token::Muted, " · ");
    items
        .iter()
        .map(|parts| parts.iter().map(|(t, spec)| styled(spec, t)).collect::<String>())
        .collect::<Vec<_>>()
        .join(&sep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_spans_tags_links_and_escapes() {
        assert_eq!(text("Call #health about [[Q4 Planning]]", false, ""), "Call <tag>#health</> about <=muted>[[</><link+u>Q4 Planning</><=muted>]]</>");
        assert_eq!(text("done #x", true, ""), "done #x");
        assert_eq!(esc("a <b> c <3 x\ty"), "a <<b> c <<3 x y");
        assert_eq!(text("Water plants", false, "plant"), "Water <b+on-accent-tint>plant</>s");
        // Data can't restyle the screen: its angle brackets come out as written.
        assert_eq!(fictty::model::fill("{x|markup}", &serde_json::json!({"x": text("a <b> #t", false, "")})), "a <b> #t");
    }
}
