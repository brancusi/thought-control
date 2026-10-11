//! Lists (owner decision, 2026-10-11): a **list** is a run of consecutive sibling items (a
//! paragraph between them ends it). Its style is the `list` prop on the run's **first item**,
//! meaning "the style of the list that starts here"; it's never an item's own numbering. Numbers
//! come from order, so a reorder renumbers by itself and no number is ever stored. When the first
//! item moves or goes, the run's new first item takes the prop (writers re-derive it from the
//! text), and a prop left on an item that no longer starts a run means nothing.
//!
//! **Styles** are the first item's marker, as the editor (fictty `lists.rs`) writes it: a bullet
//! `-` `*` `+`, or an ordered count (`1` numbers, `a`/`A` letters, `i`/`I` roman) with `.` or `)`.
//! The stored value is that template (`"1."`, `"a)"`, `"I."`, `"*"`), so case and delimiter come
//! back exactly as typed.
//!
//! **Default** (no `list` prop): bullets, except inside an ordered list, where the next level
//! takes the depth sequence `1.` → `a.` → `i.` (then again). Depth is counted along the run of
//! ordered lists a list sits in, not from the page: a bullet list breaks the run, so every existing
//! bullet list (and a bullet list under a numbered item) stays bullets with no prop written, and a
//! numbered list started anywhere is depth 1. Only a style that differs from the default is
//! written. A plain property: no op type and no format change.

use crate::store::Store;

/// The property on a list's first item that holds the list's style.
pub const LIST: &str = "list";

/// How an ordered marker counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Count {
    Number,
    Lower,
    Upper,
    Roman,
    RomanUpper,
}

/// A list's style: a bullet (its character), or an ordered count and its delimiter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ListStyle {
    Bullet(char),
    Ordered(Count, char),
}

impl ListStyle {
    pub const BULLET: ListStyle = ListStyle::Bullet('-');

    /// The depth sequence of ordered lists: depth 1 `1.`, 2 `a.`, 3 `i.`, then again.
    pub fn for_depth(depth: usize) -> ListStyle {
        let count = [Count::Number, Count::Lower, Count::Roman][depth.saturating_sub(1) % 3];
        ListStyle::Ordered(count, '.')
    }

    pub fn is_ordered(self) -> bool {
        matches!(self, ListStyle::Ordered(..))
    }

    /// A stored style: the template a first item would show (`"1."`, `"A)"`, `"-"`).
    pub fn parse(s: &str) -> Option<ListStyle> {
        let line = format!("{} x", s.trim());
        marker(&line).filter(|(_, rest)| *rest == "x").map(|(style, _)| style)
    }

    /// The stored form: the first item's marker.
    pub fn as_str(self) -> String {
        self.marker(1)
    }

    /// The marker of the `n`th item (from 1).
    pub fn marker(self, n: usize) -> String {
        match self {
            ListStyle::Bullet(c) => c.to_string(),
            ListStyle::Ordered(count, delim) => {
                let body = match count {
                    Count::Number => n.to_string(),
                    Count::Lower => letters(n),
                    Count::Upper => letters(n).to_uppercase(),
                    Count::Roman => roman(n),
                    Count::RomanUpper => roman(n).to_uppercase(),
                };
                format!("{body}{delim}")
            }
        }
    }
}

/// 1 → a, 26 → z, 27 → aa (as the editor counts).
fn letters(mut n: usize) -> String {
    let mut out = Vec::new();
    while n > 0 {
        n -= 1;
        out.push((b'a' + (n % 26) as u8) as char);
        n /= 26;
    }
    out.iter().rev().collect()
}

fn roman(mut n: usize) -> String {
    const R: [(usize, &str); 13] =
        [(1000, "m"), (900, "cm"), (500, "d"), (400, "cd"), (100, "c"), (90, "xc"), (50, "l"), (40, "xl"), (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i")];
    let mut out = String::new();
    for (v, s) in R {
        while n >= v {
            out.push_str(s);
            n -= v;
        }
    }
    out
}

/// A line's list marker (the line without its indent): its style and the text after it. The
/// same reading as the editor's: `- ` `* ` `+ `, or up to six letters or digits, `.` or `)`, a
/// space. A bare `-` is an empty bullet. A count is roman when all its letters are roman
/// numerals (`i`, `iv`, `c`), else a letter; later items' counts don't matter (the list's style
/// is its first item's).
pub fn marker(line: &str) -> Option<(ListStyle, &str)> {
    if line == "-" {
        return Some((ListStyle::BULLET, ""));
    }
    let b = line.as_bytes();
    if b.len() >= 2 && matches!(b[0], b'-' | b'*' | b'+') && b[1] == b' ' {
        return Some((ListStyle::Bullet(b[0] as char), line[2..].trim_start()));
    }
    let n = b.iter().take_while(|c| c.is_ascii_alphanumeric()).count();
    if n == 0 || n > 6 || !matches!(b.get(n), Some(b'.' | b')')) || b.get(n + 1) != Some(&b' ') {
        return None;
    }
    let token = &line[..n];
    let count = if token.bytes().all(|c| c.is_ascii_digit()) {
        Count::Number
    } else if token.chars().all(|c| "ivxlcdm".contains(c)) {
        Count::Roman
    } else if token.chars().all(|c| "IVXLCDM".contains(c)) {
        Count::RomanUpper
    } else if token.bytes().all(|c| c.is_ascii_lowercase()) {
        Count::Lower
    } else if token.bytes().all(|c| c.is_ascii_uppercase()) {
        Count::Upper
    } else {
        return None;
    };
    Some((ListStyle::Ordered(count, b[n] as char), line[n + 2..].trim_start()))
}

/// A paragraph line that would read as a list item, escaped as CommonMark does (`\- `, `1\. `),
/// so a paragraph that starts `1. ` stays a paragraph through a round trip.
pub fn escape(line: &str) -> String {
    match marker(line) {
        Some((ListStyle::Bullet(_), _)) => format!("\\{line}"),
        Some((ListStyle::Ordered(..), _)) => {
            let n = line.bytes().take_while(|c| c.is_ascii_alphanumeric()).count();
            format!("{}\\{}", &line[..n], &line[n..])
        }
        None => line.to_string(),
    }
}

/// The reverse of `escape`, for a line that isn't a list item.
pub fn unescape(line: &str) -> String {
    if let Some(rest) = line.strip_prefix('\\') {
        if marker(rest).is_some() {
            return rest.to_string();
        }
    }
    let n = line.bytes().take_while(|c| c.is_ascii_alphanumeric()).count();
    if n > 0 && line[n..].starts_with('\\') {
        let plain = format!("{}{}", &line[..n], &line[n + 1..]);
        if marker(&plain).is_some() {
            return plain;
        }
    }
    line.to_string()
}

/// A list level: the style its items show, and how deep it is in a run of ordered lists (0 for
/// bullets).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level {
    pub style: ListStyle,
    pub depth: usize,
}

impl Level {
    /// What a top-level note (a page, a day, an inbox note) sits in: no list.
    pub const NONE: Level = Level { style: ListStyle::BULLET, depth: 0 };

    /// The style of a list nested in this one when it has no `list` prop.
    pub fn default_inside(self) -> ListStyle {
        if self.depth == 0 { ListStyle::BULLET } else { ListStyle::for_depth(self.depth + 1) }
    }

    /// A list nested in this one, shown in `style`.
    pub fn inside(self, style: ListStyle) -> Level {
        Level { style, depth: if style.is_ordered() { self.depth + 1 } else { 0 } }
    }

    /// A list nested in this one, with its explicit style if it has one.
    pub fn child(self, explicit: Option<ListStyle>) -> Level {
        self.inside(explicit.unwrap_or(self.default_inside()))
    }
}

/// A node's explicit `list` prop, if it has one that reads.
pub fn explicit(store: &Store, id: &str) -> Option<ListStyle> {
    store.props_of(id).ok()?.get(LIST)?.as_str().and_then(ListStyle::parse)
}

/// Where a note stands among its siblings: a list item (its list's style and its number), or a
/// paragraph (None); and `inner`, the level its own children's lists sit in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    pub item: Option<(ListStyle, usize)>,
    pub inner: Level,
}

impl Place {
    /// The marker it's written with (None for a paragraph).
    pub fn marker(self) -> Option<String> {
        self.item.map(|(style, n)| style.marker(n))
    }
}

/// Each sibling's place, given whether it's a paragraph and its own `list` prop, and the level
/// the siblings' lists sit in. A run of items is one list: its style is its first item's prop,
/// else the default; a paragraph ends it, and its children start afresh (it isn't an item).
pub fn places(enclosing: Level, kids: impl IntoIterator<Item = (bool, Option<ListStyle>)>) -> Vec<Place> {
    let mut run: Option<(Level, usize)> = None;
    kids.into_iter()
        .map(|(para, explicit)| {
            if para {
                run = None;
                return Place { item: None, inner: Level::NONE };
            }
            let (level, n) = match run {
                Some((level, n)) => (level, n + 1),
                None => (enclosing.child(explicit), 1),
            };
            run = Some((level, n));
            Place { item: Some((level.style, n)), inner: level }
        })
        .collect()
}

/// The places of `parent`'s children (in order) from the store, given the level they sit in.
pub fn places_under(store: &Store, parent: &str, enclosing: Level) -> anyhow::Result<Vec<(crate::model::Node, Place)>> {
    let kids = store.children(parent)?;
    let p = places(enclosing, kids.iter().map(|c| (crate::edit::is_paragraph(store, c), explicit(store, &c.id))));
    Ok(kids.into_iter().zip(p).collect())
}

/// A note's place among its siblings, from the store (None for a note with no parent).
pub fn place_of(store: &Store, id: &str) -> Option<Place> {
    let parent = store.node(id).ok().flatten()?.parent?;
    let enclosing = enclosing(store, &parent);
    places_under(store, &parent, enclosing).ok()?.into_iter().find(|(n, _)| n.id == id).map(|(_, p)| p)
}

/// The level the lists among `id`'s children sit in: a top-level note's (a page's, a day's)
/// start afresh, and so do a paragraph's.
pub fn enclosing(store: &Store, id: &str) -> Level {
    place_of(store, id).map_or(Level::NONE, |p| p.inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_read_as_the_editor_reads_them() {
        let style = |l: &str| marker(l).map(|(s, _)| s.as_str());
        assert_eq!(style("1. one").as_deref(), Some("1."));
        assert_eq!(style("12) one").as_deref(), Some("1)"));
        assert_eq!(style("a. one").as_deref(), Some("a."));
        assert_eq!(style("B) one").as_deref(), Some("A)"));
        assert_eq!(style("iv. one").as_deref(), Some("i."));
        assert_eq!(style("I. one").as_deref(), Some("I."));
        assert_eq!(style("* one").as_deref(), Some("*"));
        assert_eq!(style("-").as_deref(), Some("-"));
        for prose in ["Dr. Who", "1.5 litres", "e.g. this", "1.", "#tag"] {
            assert!(marker(prose).is_none(), "{prose}");
        }
        assert_eq!(ListStyle::parse("a.").unwrap().marker(27), "aa.");
        assert_eq!(ListStyle::parse("I)").unwrap().marker(9), "IX)");
        assert!(ListStyle::parse("nope").is_none() && ListStyle::parse("").is_none());
    }

    #[test]
    fn escape_round_trips() {
        for l in ["1. not a list", "- not a bullet", "iv) no", "plain", "a.b"] {
            assert!(marker(&escape(l)).is_none(), "{l}");
            assert_eq!(unescape(&escape(l)), l);
        }
    }

    #[test]
    fn the_default_follows_the_run_of_ordered_lists() {
        let top = Level::NONE.child(None);
        assert_eq!(top.style, ListStyle::BULLET);
        let one = Level::NONE.child(ListStyle::parse("1."));
        let two = one.child(None);
        let three = two.child(None);
        let four = three.child(None);
        assert_eq!([two.style, three.style, four.style].map(|s| s.as_str()), ["a.", "i.", "1."]);
        // A bullet level breaks the run: under it, bullets again.
        assert_eq!(one.child(Some(ListStyle::BULLET)).child(None).style, ListStyle::BULLET);
    }
}
