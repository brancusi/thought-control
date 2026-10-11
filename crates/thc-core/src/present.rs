//! Words and labels every front end draws the same way: a row's meta items (dates relative to
//! today, `↻ weekly`), where a node lives (`¶ Home`, `§ Oct 2`), and what a transaction or a
//! reviewed change did, in words. Pure: the store, the glyph set and `today` in, text out. Styles
//! are tokens (thc_core::palette); each front end turns them into its own spans.

use crate::dates::{self, DateVal};
use crate::glyphs::Glyphs;
use crate::model::{HistoryEntry, Node};
use crate::palette::Token;
use crate::store::Store;
use chrono::NaiveDate;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Cut `s` to `max` cells, ending in `ell`. Cells, not bytes or chars (tui-handoff.md §4.2).
pub fn truncate_str(s: &str, max: usize, ell: &str) -> String {
    if UnicodeWidthStr::width(s) <= max {
        return s.to_string();
    }
    let budget = max.saturating_sub(UnicodeWidthStr::width(ell));
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if used + cw > budget {
            break;
        }
        used += cw;
        out.push(c);
    }
    out.push_str(ell);
    out
}

/// One date meta item (tui-handoff.md §4.1) as styled parts: `mon 09:00`, `due Oct 13 (in 10d)`,
/// `due Oct 1 (2d ago)`, then ` ◎` when `alert`. Past due is `overdue`, today `today`, else
/// `muted`. Under an agenda day heading (`under_day`) the date part is dropped: `due` / `sched` /
/// the time remain, and a lone scheduled date disappears entirely (None).
#[allow(clippy::too_many_arguments)]
pub fn date_item(g: &Glyphs, today: NaiveDate, raw: &str, is_due: bool, open: bool, alert: bool, under_day: Option<NaiveDate>, has_other: bool) -> Option<Vec<(String, Token)>> {
    let Some(dv) = DateVal::from_stored(raw) else { return Some(vec![(raw.to_string(), Token::Text)]) };
    let d = dv.date();
    let days = (d - today).num_days();
    let time = dv.time().map(|t| t.format("%H:%M").to_string());
    let text = if under_day == Some(d) {
        match (is_due, has_other, &time) {
            (true, _, Some(t)) => format!("due {t}"),
            (true, _, None) => "due".into(),
            (false, true, Some(t)) => format!("sched {t}"),
            (false, true, None) => "sched".into(),
            (false, false, Some(t)) => t.clone(),
            (false, false, None) => {
                return if alert { Some(vec![(g.alert.to_string(), Token::Muted)]) } else { None };
            }
        }
    } else {
        let t = time.map(|t| format!(" {t}")).unwrap_or_default();
        let body = if (0..7).contains(&days) {
            format!("{}{t}", dates::relative(d, today))
        } else if days < 0 {
            format!("{}{t} ({})", d.format("%b %-d"), dates::relative(d, today))
        } else {
            format!("{}{t} (in {days}d)", d.format("%b %-d"))
        };
        if is_due { format!("due {body}") } else { body }
    };
    let tone = if open && is_due && d < today {
        Token::Overdue
    } else if open && d == today {
        Token::Today
    } else {
        Token::Muted
    };
    let mut v = vec![(text, tone)];
    if alert {
        v.push((format!(" {}", g.alert), Token::Muted));
    }
    Some(v)
}

/// A node as a place something is in: `§ today`, `§ Oct 2`, `¶ Home`, else its text cut to 24
/// cells (the log's `in …`, the Tasks table's `where`).
pub fn place_label(store: &Store, g: &Glyphs, p: &Node, today: NaiveDate) -> String {
    if p.journal.is_some() {
        return match p.journal.as_deref().and_then(DateVal::from_stored).map(|d| d.date()) {
            Some(d) if d == today => format!("{} today", g.journal),
            Some(d) => format!("{} {}", g.journal, d.format("%b %-d")),
            None => g.journal.to_string(),
        };
    }
    if p.parent.is_none() && p.title.is_some() {
        return format!("{} {}", g.page, p.label());
    }
    truncate_str(&store.render_text(&p.label()), 24, g.ellipsis)
}

/// A Tasks-table date (tui-handoff.md §5.3), fitting its 11-cell column: `yesterday`, `today`,
/// `tue 09:00`, else `Oct 15` (`Oct 15 27` in another year), toned as in §4.1.
pub fn table_date(today: NaiveDate, raw: &str, is_due: bool, open: bool) -> (String, Token) {
    use chrono::Datelike;
    let Some(dv) = DateVal::from_stored(raw) else { return (raw.to_string(), Token::Text) };
    let d = dv.date();
    let days = (d - today).num_days();
    let word = if (-1..7).contains(&days) {
        dates::relative(d, today)
    } else if d.year() == today.year() {
        d.format("%b %-d").to_string()
    } else {
        d.format("%b %-d %y").to_string()
    };
    let t = dv.time().map(|t| format!(" {}", t.format("%H:%M"))).unwrap_or_default();
    let text = if UnicodeWidthStr::width(word.as_str()) + UnicodeWidthStr::width(t.as_str()) <= 11 { format!("{word}{t}") } else { word };
    let tone = if open && is_due && d < today {
        Token::Overdue
    } else if open && d == today {
        Token::Today
    } else {
        Token::Muted
    };
    (text, tone)
}

/// A transaction's first log line, as data: its verb (`created`, `completed`, `tagged`…), the
/// event naming the node a person cares about (not a tag node), and how many nodes it touched.
pub fn tx_main(entries: &[HistoryEntry]) -> Option<(&'static str, &HistoryEntry, usize)> {
    let main = entries.iter().rev().find(|e| e.op.starts_with("node.")).or(entries.first())?;
    let verb = match main.op.as_str() {
        "node.create" if main.body.get("title").is_some() && main.body.get("props").and_then(|p| p.get("tag")).is_some() => "tagged",
        "node.create" => "created",
        "node.text" => "edited",
        "node.set" => "set",
        "node.move" => "moved",
        "node.complete" => "completed",
        "node.skip" => "skipped",
        "node.delete" => "deleted",
        "node.restore" => "restored",
        "alert.add" => "alert",
        "alert.snooze" => "snoozed",
        "alert.ack" => "acked",
        "edge.add" | "edge.remove" if main.body.get("rel").and_then(|r| r.as_str()) == Some("tag") => "tagged",
        "edge.add" | "edge.remove" => "linked",
        _ => "changed",
    };
    let target = entries
        .iter()
        .rev()
        .find(|e| e.op == "node.create" && e.body.get("props").and_then(|p| p.get("tag")).is_none())
        .or_else(|| entries.iter().rev().find(|e| e.entity.len() == 12))
        .unwrap_or(main);
    let n = entries.iter().filter(|e| e.op.starts_with("node.")).map(|e| &e.entity).collect::<std::collections::HashSet<_>>().len();
    Some((verb, target, n))
}

/// What a run of a node's text is, for styling: plain text, a `#tag`, the `[[` `]]` of a link,
/// or the link's title (tui-handoff.md §4: tags `tag`, brackets `muted`, title `link`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Piece {
    Text,
    Tag,
    Bracket,
    Link,
}

/// A node's (rendered) text split into pieces. Quoted text is never parsed (writing.md §1).
pub fn text_pieces(text: &str) -> Vec<(String, Piece)> {
    let quoted = crate::capture::quoted_ranges(text);
    if !quoted.is_empty() {
        let mut out = Vec::new();
        let mut from = 0;
        for (a, b) in quoted {
            if a > from {
                out.extend(text_pieces(&text[from..a]));
            }
            out.push((text[a..b].to_string(), Piece::Text));
            from = b;
        }
        if from < text.len() {
            out.extend(text_pieces(&text[from..]));
        }
        return out;
    }
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(start) = rest.find("[[")
            && let Some(end) = rest[start..].find("]]")
        {
            tag_pieces(&rest[..start], &mut out);
            out.push(("[[".into(), Piece::Bracket));
            out.push((rest[start + 2..start + end].to_string(), Piece::Link));
            out.push(("]]".into(), Piece::Bracket));
            rest = &rest[start + end + 2..];
            continue;
        }
        tag_pieces(rest, &mut out);
        break;
    }
    out
}

fn tag_pieces(s: &str, out: &mut Vec<(String, Piece)>) {
    let mut buf = String::new();
    for (i, word) in s.split(' ').enumerate() {
        if i > 0 {
            buf.push(' ');
        }
        if crate::capture::tag_of(word).is_some() {
            if !buf.is_empty() {
                out.push((std::mem::take(&mut buf), Piece::Text));
            }
            out.push((word.to_string(), Piece::Tag));
        } else {
            buf.push_str(word);
        }
    }
    if !buf.is_empty() {
        out.push((buf, Piece::Text));
    }
}

/// Where a node lives, from its root (a page or a journal day): `¶ Q4 Planning`, `§ Oct 2`, and
/// `§ today` only when `include_today` (Today's own rows don't name today's journal).
pub fn root_label(g: &Glyphs, root: &Node, today: NaiveDate, include_today: bool) -> Option<String> {
    let t = today.format("%Y-%m-%d").to_string();
    match &root.journal {
        Some(j) if *j == t => include_today.then(|| format!("{} today", g.journal)),
        Some(j) => DateVal::from_stored(j).map(|d| format!("{} {}", g.journal, d.date().format("%b %-d"))),
        None if root.title.is_some() => Some(format!("{} {}", g.page, root.label())),
        None => None,
    }
}

/// Second log line: what the transaction did, in words a person would say.
/// e.g. `in § today · due tomorrow · links ¶ Reading List · tagged #reading`
pub fn tx_detail(store: &Store, entries: &[HistoryEntry], g: &Glyphs, today: NaiveDate) -> String {
    let place = |id: &str| -> String {
        match store.node(id).ok().flatten() {
            Some(p) => place_label(store, g, &p, today),
            None => "?".into(),
        }
    };
    let pretty = |v: &str| -> String {
        crate::dates::DateVal::from_stored(v)
            .map(|d| {
                let rel = crate::dates::relative(d.date(), today);
                let t = d.time().map(|t| format!(" {}", t.format("%H:%M"))).unwrap_or_default();
                if (0..7).contains(&(d.date() - today).num_days()) { format!("{rel}{t}") } else { format!("{}{t}", d.date().format("%b %-d")) }
            })
            .unwrap_or_else(|| v.to_string())
    };
    let prop_words = |k: &str, v: &serde_json::Value| -> Option<String> {
        let val = match v {
            serde_json::Value::Null => return Some(format!("{k} cleared")),
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Object(o) => o.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string(),
            serde_json::Value::Bool(_) => return None,
            o => o.to_string(),
        };
        Some(match k {
            "due" => format!("due {}", pretty(&val)),
            "scheduled" => format!("scheduled {}", pretty(&val)),
            "status" => format!("status {val}"),
            "priority" => format!("!{val}"),
            "repeat" => format!("{} {val}", g.repeat),
            "done_at" | "journal" | "tag" => return None,
            "title" => format!("title “{val}”"),
            _ => format!("{k} {} {val}", g.arrow),
        })
    };
    let mut parts: Vec<String> = Vec::new();
    for e in entries.iter().rev() {
        match e.op.as_str() {
            "node.create" => {
                if e.body.get("props").and_then(|p| p.get("tag")).is_some() || e.body.get("props").and_then(|p| p.get("journal")).is_some() {
                    continue;
                }
                if let Some(p) = e.body.get("parent").and_then(|v| v.as_str()) {
                    parts.push(format!("in {}", place(p)));
                } else if e.body.get("title").is_some() {
                    parts.push("new page".into());
                } else {
                    parts.push("in inbox".into());
                }
                if let Some(props) = e.body.get("props").and_then(|v| v.as_object()) {
                    for (k, v) in props {
                        if k != "status" {
                            parts.extend(prop_words(k, v));
                        }
                    }
                }
            }
            "node.set" => {
                if let Some(props) = e.body.get("props").and_then(|v| v.as_object()) {
                    for (k, v) in props {
                        parts.extend(prop_words(k, v));
                    }
                }
            }
            "node.move" => {
                let dest = e.body.get("parent").and_then(|v| v.as_str()).map(place).unwrap_or_else(|| "inbox".into());
                parts.push(format!("{} {dest}", g.arrow));
            }
            "node.complete" => {
                if let Some(n) = e.body.get("next").and_then(|n| n.get("scheduled").or(n.get("due"))).and_then(|v| v.as_str()) {
                    parts.push(format!("{} next {}", g.repeat, pretty(n)));
                }
            }
            "edge.add" | "edge.remove" => {
                let rel = e.body.get("rel").and_then(|v| v.as_str()).unwrap_or("");
                let dst = e.body.get("dst").and_then(|v| v.as_str()).unwrap_or("");
                let label = store.node(dst).ok().flatten().map(|n| n.label()).unwrap_or_default();
                let add = e.op == "edge.add";
                parts.push(match rel {
                    "tag" if add => format!("tagged #{label}"),
                    "tag" => format!("untagged #{label}"),
                    "mention" if add => format!("links {} {label}", g.page),
                    "mention" => format!("unlinks {} {label}", g.page),
                    r => format!("{} {r} {label}", if add { "+" } else { "-" }),
                });
            }
            "alert.add" => parts.push(format!("{} alert", g.alert)),
            _ => {}
        }
    }
    parts.dedup();
    parts.join(&format!(" {} ", g.sep))
}

/// A review value in words: dates relative, `!high`, `↻ every week`, `§ today`.
pub fn review_value(v: &serde_json::Value, key: &str, g: &Glyphs, today: NaiveDate) -> String {
    use serde_json::Value;
    match (key, v) {
        (_, Value::Null) => "—".into(),
        ("priority", Value::String(p)) => format!("!{p}"),
        ("repeat", Value::String(t)) => format!("{} {t}", g.repeat),
        ("parent", Value::String(p)) => match p.strip_prefix("§ ").and_then(crate::dates::DateVal::from_stored) {
            Some(d) if d.date() == today => format!("{} today", g.journal),
            Some(d) => format!("{} {}", g.journal, d.date().format("%b %-d")),
            None => p.replacen("§ ", &format!("{} ", g.journal), 1).replacen("¶ ", &format!("{} ", g.page), 1),
        },
        (_, Value::String(s)) => match crate::dates::DateVal::from_stored(s) {
            Some(d) => {
                let rel = crate::dates::relative(d.date(), today);
                let t = d.time().map(|t| format!(" {}", t.format("%H:%M"))).unwrap_or_default();
                if (0..7).contains(&(d.date() - today).num_days()) { format!("{rel}{t}") } else { format!("{}{t}", d.date().format("%b %-d")) }
            }
            None => s.clone(),
        },
        (_, other) => other.to_string(),
    }
}

fn review_set(f: &serde_json::Value, prefix: &str, minus: &str) -> String {
    let mut parts = Vec::new();
    for (side, sign) in [("add", "+"), ("remove", minus)] {
        for v in f.get(side).and_then(|a| a.as_array()).into_iter().flatten() {
            parts.push(format!("{sign}{prefix}{}", v.as_str().unwrap_or("")));
        }
    }
    parts.join(" ")
}

/// `(key, old, new)` lines for one change, in display order (agents.md §1.6 detail pane).
pub fn review_field_lines(c: &crate::review::Change, g: &Glyphs, today: NaiveDate) -> Vec<(String, Option<String>, String)> {
    let mut keys: Vec<&String> = c.fields.keys().filter(|k| k.as_str() != "done_at").collect();
    keys.sort_by_key(|k| crate::review::field_rank(k));
    if c.change == "create" {
        keys.sort_by_key(|k| k.as_str() != "parent");
    }
    keys.into_iter()
        .map(|k| {
            let f = &c.fields[k];
            let label = match k.as_str() {
                "parent" => "in".to_string(),
                "scheduled" => "sched".to_string(),
                _ => k.clone(),
            };
            if k == "tags" || k == "links" {
                let minus = if g.arrow == "→" { "−" } else { "-" };
                return (label, None, review_set(f, if k == "tags" { "#" } else { "" }, minus));
            }
            let old = match f.get("from") {
                Some(v) => Some(review_value(v, k, g, today)),
                None if c.change != "create" => Some(review_value(&serde_json::Value::Null, k, g, today)),
                None => None,
            };
            let new = review_value(f.get("to").unwrap_or(&serde_json::Value::Null), k, g, today);
            (label, old, new)
        })
        .collect()
}

/// The second line of a review row: the first change's marker, node and what changed.
pub fn review_summary(store: &Store, it: &crate::review::Item, g: &Glyphs, today: NaiveDate) -> String {
    let Some(c) = it.changes.first() else { return String::new() };
    if it.changes.len() > 1 {
        // A batch: `+ 3 created · ~ 1 changed · ✓ 1 done · → 1 moved`.
        let mut parts: Vec<String> = crate::review::counts(&it.changes).iter().map(|(k, n)| format!("{} {n} {}", review_marker(k, g), crate::review::count_word(k))).collect();
        let links = crate::review::link_count(&it.changes);
        if links > 0 {
            parts.push(format!("{links} link{}", if links == 1 { "" } else { "s" }));
        }
        return parts.join(&format!(" {} ", g.sep));
    }
    let mark = review_marker(c.change, g);
    let more = if it.changes.len() > 1 { format!(" {} +{} more", g.sep, it.changes.len() - 1) } else { String::new() };
    let flag = if it.conflict { format!(" {} {} conflict", g.sep, g.conflict) } else if it.later_changed_by_human { format!(" {} you changed it afterwards", g.sep) } else { String::new() };
    // The node ID only when the transaction touched several nodes.
    let id = if it.changes.len() > 1 { format!("{} ", c.short) } else { String::new() };
    format!("{mark} {id}{}{more}{flag}", change_words(store, c, g, today))
}

/// `done · ↻ next Tue Oct 6`, `moved from … to …`, or the field lines joined.
pub fn change_words(store: &Store, c: &crate::review::Change, g: &Glyphs, today: NaiveDate) -> String {
    let to = |k: &str| c.fields.get(k).and_then(|f| f.get("to")).cloned().unwrap_or(serde_json::Value::Null);
    match c.change {
        "complete" => {
            // A repeating completion moves its dates (status stays todo).
            {
                let next = ["scheduled", "due"].iter().find_map(|k| to(k).as_str().and_then(crate::dates::DateVal::from_stored)).map(|d| d.date());
                match next {
                    Some(d) => format!("done {} {} next {}", g.sep, g.repeat, d.format("%a %b %-d")),
                    None => "done".into(),
                }
            }
        }
        "delete" => match c.children.filter(|k| *k > 0) {
            Some(k) => format!("deleted with {k} children"),
            None => "deleted".into(),
        },
        "restore" => "restored".into(),
        _ => review_fields_text(store, c, g, today),
    }
}

fn review_fields_text(store: &Store, c: &crate::review::Change, g: &Glyphs, today: NaiveDate) -> String {
    let create = c.change == "create";
    let mut lines = review_field_lines(c, g, today);
    if create {
        // `in § today · due tomorrow · #reading`: place first, no status (the checkbox shows it).
        lines.retain(|(k, _, _)| k != "text" && k != "status");
    }
    let fields: Vec<String> = lines
        .into_iter()
        .map(|(k, old, new)| {
            let (old, new) = if k == "text" { (old.map(|o| store.render_text(&o)), store.render_text(&new)) } else { (old, new) };
            match old {
                Some(o) => format!("{k} {o} {} {new}", g.arrow),
                None if k == "in" => format!("in {new}"),
                None if create && k == "tags" => new.replace('+', ""),
                None if create && matches!(k.as_str(), "priority" | "repeat") => new,
                None => format!("{k} {new}"),
            }
        })
        .collect();
    fields.join(&format!(" {} ", g.sep))
}

pub fn review_marker(change: &str, g: &Glyphs) -> &'static str {
    let ascii = g.arrow != "→";
    match change {
        "create" | "restore" => "+",
        "move" => if ascii { ">" } else { "→" },
        "complete" => if ascii { "x" } else { "✓" },
        "delete" => if ascii { "-" } else { "−" },
        "alert" => if ascii { "(o)" } else { "◎" },
        _ => "~",
    }
}

pub fn short_repeat(rule: &str) -> String {
    let rule = rule.trim();
    let from_done = rule.starts_with("every!");
    let short = short_repeat_rule(rule.trim_start_matches("every!").trim_start_matches("every").trim());
    if from_done { format!("{short}!") } else { short }
}

fn short_repeat_rule(r: &str) -> String {
    let words: Vec<&str> = r.split_whitespace().collect();
    let unit = |w: &str| -> Option<&'static str> {
        Some(match w.trim_end_matches('s') {
            "day" => "d",
            "week" => "w",
            "month" => "mo",
            "year" => "y",
            _ => return None,
        })
    };
    match words.as_slice() {
        [] => "repeats".into(),
        ["day", ..] | ["daily", ..] => "daily".into(),
        ["weekday", ..] | ["weekdays", ..] => "weekdays".into(),
        ["week", ..] | ["weekly", ..] => "weekly".into(),
        ["month", ..] | ["monthly", ..] => "monthly".into(),
        ["year", ..] | ["yearly", ..] => "yearly".into(),
        [n, u, ..] if n.parse::<u32>().is_ok() && unit(u).is_some() => format!("{n}{}", unit(u).unwrap()),
        [one] if one.len() <= 10 => (*one).to_string(),
        _ => r.chars().take(12).collect(),
    }
}
