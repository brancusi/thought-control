//! Inline syntax at human edges: capture strings, `drop/` files and the `$EDITOR` format.
//! Parsed once into typed fields; never re-parsed from stored text.
//!
//! Tokens: `[ ]` / `[x]` / `todo` prefix, `due:X`, `sched:X`, `at:X`, `every:X` / `every!:X`,
//! `!high|!med|!low`, `#tag`, `[[Page Title]]`. Values with spaces may be quoted: `due:"nov 1 9am"`.

use crate::dates::{self, DateVal};
use crate::recur::Repeat;
use anyhow::{Result, anyhow};
use chrono::NaiveDate;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Capture {
    /// Display text, with field tokens removed (tags and links are kept).
    pub text: String,
    pub status: Option<String>,
    pub scheduled: Option<DateVal>,
    pub due: Option<DateVal>,
    pub priority: Option<String>,
    pub repeat: Option<Repeat>,
    pub tags: Vec<String>,
    pub links: Vec<String>,
}

pub const STATUSES: [&str; 5] = ["todo", "doing", "waiting", "done", "cancelled"];

pub fn normalize_priority(p: &str) -> Option<&'static str> {
    Some(match p.to_lowercase().as_str() {
        "high" | "h" | "1" | "a" => "high",
        "med" | "medium" | "m" | "2" | "b" => "med",
        "low" | "l" | "3" | "c" => "low",
        _ => return None,
    })
}

/// A word of capture text: the whitespace before it (kept, so soft breaks and spacing survive
/// a parse) and whether it's code (inside backticks or a fence, never read for tokens or tags).
struct Tok {
    ws: String,
    text: String,
    code: bool,
}

/// Split respecting `key:"quoted value"` tokens and `` `code` `` spans.
fn tokenize(s: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut ws = String::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut in_code = false;
    let mut code = false;
    for c in s.chars() {
        match c {
            '`' => {
                in_code = !in_code;
                code = true;
                cur.push(c);
            }
            '"' if !in_code => {
                in_quote = !in_quote;
                cur.push(c);
            }
            c if c.is_whitespace() && !in_quote => {
                if !cur.is_empty() {
                    out.push(Tok { ws: std::mem::take(&mut ws), text: std::mem::take(&mut cur), code });
                }
                code = in_code;
                ws.push(c);
            }
            c => {
                code |= in_code;
                cur.push(c);
            }
        }
    }
    if !cur.is_empty() {
        out.push(Tok { ws, text: cur, code });
    }
    out
}

/// Kept words back into text, with their original spacing (the first word's dropped).
fn join(kept: &[Tok]) -> String {
    let mut out = String::new();
    for (i, t) in kept.iter().enumerate() {
        if i > 0 {
            out.push_str(if t.ws.is_empty() { " " } else { &t.ws });
        }
        out.push_str(&t.text);
    }
    out
}

/// The tag a word carries, if any: `#work` → `work`. A run of `#` (a Markdown heading marker)
/// and numbers (`#1`) aren't tags.
pub fn tag_of(tok: &str) -> Option<String> {
    let tag = tok.strip_prefix('#')?;
    let tag = tag.trim_end_matches([',', '.', ';', ':', ')', '!', '?']);
    if tag.is_empty() || tag.starts_with('#') || tag.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(tag.to_lowercase())
}

fn unquote(v: &str) -> &str {
    v.trim_matches('"')
}

/// What a lenient capture (the daemon's `capture`, the TUI box, ThoughtBar) would save, for live
/// previews. The daemon's `parse` method and `thc parse` return exactly this, so a preview can
/// never disagree with what gets written.
pub fn preview(input: &str, today: NaiveDate) -> Result<serde_json::Value> {
    let (cap, bad) = parse_lenient(input, today)?;
    // Why each bad token didn't parse, for previews that explain it (`"30m" is ambiguous …`).
    let why: Vec<String> = bad
        .iter()
        .map(|t| match parse(&format!("x {t}"), today) {
            Err(e) => {
                let m = e.to_string();
                m.strip_prefix("invalid: ").unwrap_or(&m).to_string()
            }
            Ok(_) => String::new(),
        })
        .collect();
    let kind = match (&cap.status, &cap.scheduled) {
        (Some(_), _) => "task",
        (None, Some(DateVal::DateTime(_))) => "event",
        _ => "note",
    };
    let mut v = serde_json::json!({
        "text": cap.text,
        "kind": kind,
        "status": cap.status,
        "scheduled": cap.scheduled.map(|d| d.fmt()),
        "due": cap.due.map(|d| d.fmt()),
        "priority": cap.priority,
        "repeat": cap.repeat.map(|r| r.text),
        "tags": cap.tags,
        "links": cap.links,
        "bad": bad,
        "bad_why": why,
    });
    if let Some(m) = v.as_object_mut() {
        m.retain(|_, x| !x.is_null());
    }
    Ok(v)
}

/// The field a single token sets, in words for "two …" (writing.md §1, "Two tokens for one
/// field"), and its value, normalized: `!high` → ("priorities", "high").
fn field_of(tok: &str, today: NaiveDate) -> Option<(&'static str, String)> {
    if !(tok.contains(':') || tok.starts_with('!')) {
        return None;
    }
    let c = parse_tokens(&format!("x {tok}"), today).ok()?;
    if let Some(p) = c.priority {
        return Some(("priorities", p));
    }
    if let Some(d) = c.due {
        return Some(("due dates", d.fmt()));
    }
    if let Some(d) = c.scheduled {
        return Some(("scheduled dates", d.fmt()));
    }
    if let Some(r) = c.repeat {
        return Some(("repeats", r.text));
    }
    None
}

/// Fields set by more than one token with different values: (field, the tokens in order). The
/// same value twice (`!high … !high`) isn't a conflict.
pub fn duplicates(input: &str, today: NaiveDate) -> Vec<(&'static str, Vec<String>)> {
    let mut seen: Vec<(&'static str, Vec<(String, String)>)> = Vec::new();
    for t in tokenize(input.trim()) {
        if t.code {
            continue;
        }
        let Some((field, value)) = field_of(&t.text, today) else { continue };
        match seen.iter_mut().find(|(f, _)| *f == field) {
            Some((_, v)) => v.push((t.text.clone(), value)),
            None => seen.push((field, vec![(t.text.clone(), value)])),
        }
    }
    seen.into_iter()
        .filter(|(_, v)| v.iter().any(|(_, x)| *x != v[0].1))
        .map(|(f, v)| (f, v.into_iter().map(|(t, _)| t).collect()))
        .collect()
}

/// Byte ranges in `input` of the field tokens a later token of the same field overrides (with a
/// different value): drawn as plain words, since what's coloured is what applies.
pub fn shadowed(input: &str, today: NaiveDate) -> Vec<(usize, usize)> {
    let quoted = quoted_ranges(input);
    let mut toks: Vec<(&'static str, String, usize, usize)> = Vec::new();
    let mut at = 0;
    for w in input.split(' ') {
        let (a, b) = (at, at + w.len());
        at = b + 1;
        if w.is_empty() || quoted.iter().any(|(qa, qb)| a > *qa && a < *qb) {
            continue;
        }
        if let Some((f, v)) = field_of(w, today) {
            toks.push((f, v, a, b));
        }
    }
    let mut out = Vec::new();
    for (i, (f, _, a, b)) in toks.iter().enumerate() {
        let later: Vec<&(&str, String, usize, usize)> = toks[i + 1..].iter().filter(|t| t.0 == *f).collect();
        let all_same = toks.iter().filter(|t| t.0 == *f).all(|t| t.1 == toks[i].1);
        if !later.is_empty() && !all_same {
            out.push((*a, *b));
        }
    }
    out
}

/// The refusal for two tokens for one field, as the CLI, `thc apply` and `thc import` give it.
fn refuse_duplicates(input: &str, today: NaiveDate) -> Result<()> {
    if let Some((field, toks)) = duplicates(input, today).into_iter().next() {
        let n = match toks.len() {
            2 => "two".to_string(),
            3 => "three".to_string(),
            n => n.to_string(),
        };
        return Err(crate::error::invalid(format!("{n} {field} ({}) · keep one, or quote them to keep them as text", toks.join(", "))));
    }
    Ok(())
}

/// Like `parse`, but a token whose value doesn't parse (e.g. `due:fryday`) stays in the text
/// as literal words instead of failing. Returns the bad tokens. Two tokens for one field: the
/// last wins, and the earlier ones stay in the text as plain words (writing.md §1).
pub fn parse_lenient(input: &str, today: NaiveDate) -> Result<(Capture, Vec<String>)> {
    let mut bad = Vec::new();
    let mut kept: Vec<Tok> = Vec::new();
    // The last token of each field with differing values: the earlier ones are words.
    let dupes = duplicates(input, today);
    let mut remaining: Vec<(&str, usize)> = dupes.iter().map(|(f, t)| (*f, t.len())).collect();
    for mut tok in tokenize(input.trim()) {
        if tok.code {
            kept.push(tok);
            continue;
        }
        if let Some((field, _)) = field_of(&tok.text, today) {
            if let Some(r) = remaining.iter_mut().find(|(f, _)| *f == field) {
                r.1 -= 1;
                if r.1 > 0 {
                    tok.text = neutralize(&tok.text);
                    kept.push(tok);
                    continue;
                }
            }
        }
        let probe = format!("x {}", tok.text);
        if parse(&probe, today).is_err() && tok.text.contains(':') {
            bad.push(tok.text.clone());
            tok.text = tok.text.replacen(':', "\u{2236}", 1); // ratio sign: no longer a field token
        }
        kept.push(tok);
    }
    let mut cap = parse_tokens(&join(&kept), today)?;
    cap.text = cap.text.replace('\u{2236}', ":").replace(NOT_PRIORITY, "!");
    Ok((cap, bad))
}

/// A lookalike of `!` that marks no priority (put back after parsing).
const NOT_PRIORITY: char = '\u{01C3}';

/// A field token made into plain words for the parser (restored after): `due:fri` → `due∶fri`.
fn neutralize(tok: &str) -> String {
    if let Some(rest) = tok.strip_prefix('!') {
        return format!("{NOT_PRIORITY}{rest}");
    }
    tok.replacen(':', "\u{2236}", 1)
}

/// For the editor: like `parse_lenient`, but a leading marker (`- `, `[x] `, `TODO `, `DONE `)
/// is text, not a kind or a status. A document's lines carry those themselves, so "DONE with
/// the report" stays a sentence, not a done task "with the report" (fuzz).
pub fn parse_lenient_text(input: &str, today: NaiveDate) -> Result<(Capture, Vec<String>)> {
    const GUARD: char = '\u{2063}'; // invisible separator: no marker matches after it
    let guarded = format!("{GUARD}{}", input.trim_start());
    let (mut cap, bad) = parse_lenient(&guarded, today)?;
    cap.text = cap.text.replacen(GUARD, "", 1).trim_start().to_string();
    Ok((cap, bad))
}

/// Parse capture text. Two tokens for one field with different values exit 6 (the strict
/// edges: the CLI, `thc apply`, `thc import`); the lenient ones use `parse_lenient`.
pub fn parse(input: &str, today: NaiveDate) -> Result<Capture> {
    refuse_duplicates(input, today)?;
    parse_tokens(input, today)
}

/// The parse itself: each field token sets its field (a later one replaces an earlier one).
fn parse_tokens(input: &str, today: NaiveDate) -> Result<Capture> {
    let mut cap = Capture::default();
    let mut s = input.trim();
    // Leading bullet / checkbox / TODO marker.
    if let Some(rest) = s.strip_prefix("- ").or_else(|| s.strip_prefix("* ")) {
        s = rest.trim_start();
    }
    for (prefix, status) in [
        ("[ ] ", "todo"),
        ("[x] ", "done"),
        ("[X] ", "done"),
        ("[/] ", "doing"),
        ("[-] ", "cancelled"),
        ("[w] ", "waiting"),
        ("TODO ", "todo"),
        ("todo: ", "todo"),
        ("DOING ", "doing"),
        ("DONE ", "done"),
    ] {
        if let Some(rest) = s.strip_prefix(prefix) {
            cap.status = Some(status.into());
            s = rest.trim_start();
            break;
        }
    }
    let mut kept: Vec<Tok> = Vec::new();
    for t in tokenize(s) {
        if t.code {
            kept.push(t);
            continue;
        }
        let tok = t.text.clone();
        if let Some(colon) = tok.find(':') {
            let k = tok[..colon].to_lowercase();
            let raw_v = unquote(&tok[colon + 1..]).to_string();
            let v = raw_v.to_lowercase();
            let v = v.as_str();
            if !raw_v.is_empty() && !raw_v.starts_with("//") {
                match k.as_str() {
                    "due" | "deadline" => {
                        cap.due = Some(dates::parse(&raw_v, today)?);
                        continue;
                    }
                    "sched" | "scheduled" | "start" => {
                        cap.scheduled = Some(dates::parse(&raw_v, today)?);
                        continue;
                    }
                    "at" | "on" => {
                        cap.scheduled = Some(dates::parse(&raw_v, today)?);
                        continue;
                    }
                    "every" => {
                        cap.repeat = Some(Repeat::parse(&format!("every {raw_v}"), None)?);
                        continue;
                    }
                    "every!" => {
                        cap.repeat = Some(Repeat::parse(&format!("every! {raw_v}"), None)?);
                        continue;
                    }
                    "repeat" => {
                        cap.repeat = Some(Repeat::parse(&raw_v, None)?);
                        continue;
                    }
                    "status" if STATUSES.contains(&v) => {
                        cap.status = Some(v.to_string());
                        continue;
                    }
                    _ => {}
                }
            }
        }
        if let Some(p) = tok.strip_prefix('!') {
            if let Some(p) = normalize_priority(p) {
                cap.priority = Some(p.into());
                continue;
            }
        }
        if let Some(tag) = tag_of(&tok) {
            if !cap.tags.contains(&tag) {
                cap.tags.push(tag);
            }
        }
        kept.push(t);
    }
    cap.text = join(&kept);
    cap.links = extract_links(&cap.text);
    let is_task_shaped = cap.due.is_some() || cap.priority.is_some() || cap.repeat.is_some();
    if cap.status.is_none() && is_task_shaped {
        cap.status = Some("todo".into());
    }
    if cap.text.is_empty() {
        return Err(anyhow!("nothing to capture: text is empty"));
    }
    Ok(cap)
}

/// Byte ranges of quoted text: inside "double quotes" or `backticks` nothing is parsed (writing.md
/// §1, "quoted text is never parsed"): not tokens, not tags, not `[[links]]`.
pub fn quoted_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut open: Option<(char, usize)> = None;
    for (i, c) in text.char_indices() {
        match (open, c) {
            (None, '"' | '`') => open = Some((c, i)),
            (Some((q, start)), c) if c == q => {
                out.push((start, i + c.len_utf8()));
                open = None;
            }
            _ => {}
        }
    }
    out
}

/// `[[Title]]` and `[[id|Title]]` targets, in order of appearance (never inside quotes).
pub fn extract_links(text: &str) -> Vec<String> {
    let quoted = quoted_ranges(text);
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        let at = text.len() - rest.len() + start;
        if quoted.iter().any(|(a, b)| at > *a && at < *b) {
            rest = &rest[start + 2..];
            continue;
        }
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else { break };
        let inner = &after[..end];
        let target = inner.split('|').next().unwrap_or(inner).trim();
        // `[[vault:…]]` is reserved for links between vaults (FORMAT.md): kept as text.
        let reserved = target.get(..6).is_some_and(|p| p.eq_ignore_ascii_case("vault:"));
        if !target.is_empty() && !reserved && !out.iter().any(|t: &String| t == target) {
            out.push(target.to_string());
        }
        rest = &after[end + 2..];
    }
    out
}

/// Tags present in stored text (used when text is edited).
pub fn extract_tags(text: &str) -> Vec<String> {
    let mut tags = Vec::new();
    for tok in tokenize(text) {
        if tok.code {
            continue;
        }
        if let Some(tag) = tag_of(&tok.text) {
            if !tags.contains(&tag) {
                tags.push(tag);
            }
        }
    }
    tags
}

#[cfg(test)]
mod text_tests {
    use super::*;

    #[test]
    fn the_editor_keeps_leading_markers_as_text() {
        let d = NaiveDate::from_ymd_opt(2026, 10, 3).unwrap();
        for t in ["DONE with the report", "- [x] done item", "TODO later", "[ ] box", "* star"] {
            let (c, _) = parse_lenient_text(t, d).unwrap();
            assert_eq!(c.text, t, "{t}");
            assert_eq!(c.status, None, "{t}");
        }
        let (c, _) = parse_lenient_text("pay rent due:2026-10-09 #home", d).unwrap();
        assert_eq!(c.text, "pay rent #home");
        assert!(c.due.is_some());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 3).unwrap()
    }

    #[test]
    fn task_with_fields() {
        let c = parse("Call dentist due:fri #health !high", today()).unwrap();
        assert_eq!(c.text, "Call dentist #health");
        assert_eq!(c.status.as_deref(), Some("todo"));
        assert_eq!(c.due.unwrap().fmt(), "2026-10-09");
        assert_eq!(c.priority.as_deref(), Some("high"));
        assert_eq!(c.tags, vec!["health"]);
    }

    #[test]
    fn plain_note_and_quoted_dates() {
        let c = parse("idea: sync reading list", today()).unwrap();
        assert_eq!(c.status, None);
        assert_eq!(c.text, "idea: sync reading list");
        let c = parse(r#"Dentist at:"tue 2pm" [[Health]]"#, today()).unwrap();
        assert_eq!(c.scheduled.unwrap().fmt(), "2026-10-06T14:00");
        assert_eq!(c.status, None);
        assert_eq!(c.links, vec!["Health"]);
    }

    #[test]
    fn checkbox_and_repeat() {
        let c = parse("- [ ] water plants every:1w", today()).unwrap();
        assert_eq!(c.status.as_deref(), Some("todo"));
        assert_eq!(c.repeat.unwrap().rule, "FREQ=WEEKLY");
        let c = parse("[x] renew passport", today()).unwrap();
        assert_eq!(c.status.as_deref(), Some("done"));
    }

    #[test]
    fn lenient_keeps_bad_tokens_literal() {
        let (c, bad) = parse_lenient("Book flu shot due:fryday #health", today()).unwrap();
        assert_eq!(bad, vec!["due:fryday"]);
        assert_eq!(c.text, "Book flu shot due:fryday #health");
        assert!(c.due.is_none());
    }

    #[test]
    fn heading_markers_are_not_tags() {
        let c = parse("## Sub #work", today()).unwrap();
        assert_eq!(c.tags, vec!["work"]);
        assert_eq!(c.text, "## Sub #work");
        assert!(parse("# Title", today()).unwrap().tags.is_empty());
        assert!(extract_tags("### Notes ##").is_empty());
    }

    #[test]
    fn code_is_never_parsed() {
        let (c, bad) = parse_lenient("see `#include due:fri` and `a b:c` due:fri", today()).unwrap();
        assert!(bad.is_empty(), "{bad:?}");
        assert!(c.tags.is_empty());
        assert_eq!(c.text, "see `#include due:fri` and `a b:c`");
        assert!(c.due.is_some());
        let (c, bad) = parse_lenient("```\nlet x = 1 due:fryday #tag\n```", today()).unwrap();
        assert!(bad.is_empty() && c.tags.is_empty() && c.due.is_none());
        assert_eq!(c.text, "```\nlet x = 1 due:fryday #tag\n```", "soft breaks survive");
        assert!(extract_tags("`#nope` #yes").contains(&"yes".to_string()));
        assert_eq!(extract_tags("`#nope` #yes").len(), 1);
    }

    #[test]
    fn urls_are_not_fields() {
        let c = parse("read https://example.com/post", today()).unwrap();
        assert_eq!(c.text, "read https://example.com/post");
    }
}

#[cfg(test)]
mod quoted_tests {
    use super::*;

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 5).unwrap()
    }

    /// writing.md §1, "quoted text is never parsed": inside "double quotes" or `backticks`
    /// nothing is a token (`!high`, `#tag`, `due:`, `[[…]]`); a quoted *value* still is.
    #[test]
    fn quoted_text_is_never_parsed() {
        let c = parse(r##"Priority is written "!high !med !low" in capture"##, day()).unwrap();
        assert_eq!(c.priority, None);
        assert_eq!(c.text, r##"Priority is written "!high !med !low" in capture"##);
        let c = parse(r##"the tag "#work" and the link "[[Page]]" stay text"##, day()).unwrap();
        assert!(c.tags.is_empty() && c.links.is_empty(), "{c:?}");
        let c = parse("run `thc set x due:fri !high` later", day()).unwrap();
        assert!(c.due.is_none() && c.priority.is_none(), "{c:?}");
        // A quoted value is still a token.
        let c = parse(r##"Dentist due:"nov 1 9am""##, day()).unwrap();
        assert!(c.due.is_some() && c.text == "Dentist", "{c:?}");
        // Outside quotes, tokens work as before.
        let c = parse(r##"Fix "it" !high #bug"##, day()).unwrap();
        assert_eq!(c.priority.as_deref(), Some("high"));
        assert_eq!(c.tags, ["bug"]);
        // A quoted link isn't a link when written either (no stub page): builder::canonicalize.
        let dir = std::env::temp_dir().join(format!("thc-quote-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = crate::store::Store::open(&dir.join("s.db")).unwrap();
        let mut b = crate::builder::TxBuilder::new(&store, day());
        let (text, links) = b.canonicalize(r##"see "[[Lisbon]]" only, and [[Porto]]"##).unwrap();
        assert_eq!(links.len(), 1, "only the bare link");
        assert!(text.contains(r##""[[Lisbon]]""##), "{text}");
        assert_eq!(b.stubs.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
        // The seeding bug's line, quoted, keeps every token as text.
        let c = parse(r##"Tags: "#bug #polish #feature". Priority: "!high !med !low"."##, day()).unwrap();
        assert!(c.priority.is_none() && c.tags.is_empty() && c.status.is_none(), "{c:?}");
    }
}

#[cfg(test)]
mod two_tokens_tests {
    use super::*;

    fn d() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 5).unwrap()
    }

    /// (writing.md §1, "Two tokens for one field"): strict edges refuse, lenient ones use
    /// the last and keep the earlier as words; the same value twice is fine.
    #[test]
    fn two_tokens_for_one_field() {
        let e = parse("call !high about it !low", d()).unwrap_err().to_string();
        assert!(e.contains("two priorities (!high, !low) · keep one, or quote them to keep them as text"), "{e}");
        assert!(parse("x due:fri due:mon", d()).unwrap_err().to_string().contains("two due dates (due:fri, due:mon)"));
        assert!(parse("x sched:fri at:mon", d()).unwrap_err().to_string().contains("two scheduled dates"));
        assert_eq!(parse("x !high y !high", d()).unwrap().priority.as_deref(), Some("high"));
        let (c, bad) = parse_lenient("call !high about it !low", d()).unwrap();
        assert_eq!(c.priority.as_deref(), Some("low"));
        assert_eq!(c.text, "call !high about it");
        assert!(bad.is_empty());
        let (c, _) = parse_lenient("ship due:fri or due:mon", d()).unwrap();
        assert_eq!(c.due.map(|x| x.fmt()), Some("2026-10-05".into()), "mon is today: the last one wins");
        assert_eq!(c.text, "ship due:fri or");
        assert_eq!(duplicates("a !high b !low", d()), vec![("priorities", vec!["!high".to_string(), "!low".to_string()])]);
        // Quoted, it's text.
        assert_eq!(parse("say \"!high\" then !low", d()).unwrap().priority.as_deref(), Some("low"));
    }
}

#[cfg(test)]
mod shadowed_tests {
    use super::*;

    #[test]
    fn earlier_tokens_of_a_field_are_shadowed() {
        let d = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        let t = "call !high about due:fri it !low";
        assert_eq!(shadowed(t, d), vec![(5, 10)]);
        assert!(shadowed("x !high y !high", d).is_empty());
        assert!(shadowed("x \"!high\" !low", d).is_empty());
    }
}
