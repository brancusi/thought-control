//! Markdown in and out (caretline SPEC §6): a paste read into blocks, and blocks written back
//! as Markdown for copy. The engine's own parser: it owns the round trip, including the one
//! CommonMark deviation (a single newline inside a block is a soft break).

use crate::{Block, Kind};

/// A pasted line: (depth, kind, status, text).
pub type Pasted = (usize, Kind, Option<String>, String);

/// Markdown read as outline lines (tui-editor.md §6, editor.md §3.2): paragraphs (lines joined),
/// `- * +` bullets nested by the first indent, numbered items, `- [ ]` tasks, headings, quotes
/// and rules as text, fences / tables / front matter as one soft-broken paragraph. Images are
/// left out and counted. `plain`: a paragraph per block, line breaks kept.
pub fn parse_paste(input: &str, plain: bool) -> (Vec<Pasted>, usize) {
    let text = input.replace("\r\n", "\n").replace('\r', "\n");
    let mut raw: Vec<String> = text.split('\n').map(|l| l.replace('\t', "    ")).collect();
    while raw.last().is_some_and(|l| l.trim().is_empty()) {
        raw.pop();
    }
    while raw.first().is_some_and(|l| l.trim().is_empty()) {
        raw.remove(0);
    }
    let mut out: Vec<Pasted> = Vec::new();
    if plain {
        let mut block: Vec<String> = Vec::new();
        for l in raw.into_iter().chain(std::iter::once(String::new())) {
            if l.trim().is_empty() {
                if !block.is_empty() {
                    out.push((0, Kind::Para, None, block.join("\n")));
                    block.clear();
                }
            } else {
                block.push(l);
            }
        }
        return (out, 0);
    }
    let mut images = 0;
    for l in raw.iter_mut() {
        while let (Some(a), true) = (l.find("!["), l.contains("](")) {
            let Some(close) = l[a..].find(')').map(|k| a + k) else {
                break;
            };
            l.replace_range(a..=close, "");
            images += 1;
        }
        let trimmed = l.trim_end().len();
        l.truncate(trimmed);
    }
    let item = |l: &str| -> Option<(usize, String, String)> {
        let indent = l.len() - l.trim_start().len();
        let rest = l.trim_start();
        let (marker, body) = if let Some(b) = rest
            .strip_prefix("- ")
            .or_else(|| rest.strip_prefix("* "))
            .or_else(|| rest.strip_prefix("+ "))
        {
            ("-".to_string(), b.to_string())
        } else if ["[ ] ", "[x] ", "[X] ", "[/] ", "[-] ", "[w] "]
            .iter()
            .any(|p| rest.starts_with(p))
        {
            // A bare checkbox is a task item too (`[ ] call`, as typed); the save no longer
            // reads markers out of a line's text.
            ("-".to_string(), rest.to_string())
        } else {
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            let after = &rest[digits.len()..];
            if !digits.is_empty() && (after.starts_with(". ") || after.starts_with(") ")) {
                (format!("{digits}{}", &after[..1]), after[2..].to_string())
            } else {
                return None;
            }
        };
        Some((indent, marker, body))
    };
    let unit = raw
        .iter()
        .filter_map(|l| item(l))
        .map(|(i, _, _)| i)
        .find(|i| *i > 0)
        .unwrap_or(2);
    let mut para: Vec<String> = Vec::new();
    let flush = |para: &mut Vec<String>, out: &mut Vec<Pasted>| {
        if !para.is_empty() {
            out.push((0, Kind::Para, None, para.join(" ")));
            para.clear();
        }
    };
    let mut i = 0;
    if raw.first().map(String::as_str) == Some("---")
        && let Some(close) = raw.iter().skip(1).position(|l| l == "---").map(|k| k + 1)
    {
        let mut body = vec!["```".to_string()];
        body.extend(raw[1..close].iter().cloned());
        body.push("```".into());
        out.push((0, Kind::Para, None, body.join("\n")));
        i = close + 1;
    }
    let mut last_indent: Option<usize> = None;
    while i < raw.len() {
        let l = raw[i].clone();
        let t = l.trim();
        if t.is_empty() {
            flush(&mut para, &mut out);
            last_indent = None;
            i += 1;
            continue;
        }
        if t.starts_with("```") {
            flush(&mut para, &mut out);
            let mut body = vec![t.to_string()];
            i += 1;
            while i < raw.len() {
                body.push(raw[i].clone());
                i += 1;
                if raw[i - 1].trim().starts_with("```") {
                    break;
                }
            }
            out.push((0, Kind::Para, None, body.join("\n")));
            last_indent = None;
            continue;
        }
        if t.starts_with('|') {
            flush(&mut para, &mut out);
            let mut rows = Vec::new();
            while i < raw.len() && raw[i].trim().starts_with('|') {
                rows.push(raw[i].trim().to_string());
                i += 1;
            }
            out.push((0, Kind::Para, None, rows.join("\n")));
            last_indent = None;
            continue;
        }
        if let Some((indent, marker, body)) = item(&l) {
            flush(&mut para, &mut out);
            let mut kind = Kind::Bullet;
            let mut status = None;
            let mut text = body;
            for (p, s) in [
                ("[ ] ", "todo"),
                ("[x] ", "done"),
                ("[X] ", "done"),
                ("[/] ", "doing"),
                ("[-] ", "cancelled"),
                ("[w] ", "waiting"),
            ] {
                if let Some(rest) = text.strip_prefix(p) {
                    kind = Kind::Task;
                    status = Some(s.to_string());
                    text = rest.to_string();
                    break;
                }
            }
            if kind == Kind::Bullet && marker != "-" {
                text = format!("{marker} {text}");
            }
            out.push((indent / unit, kind, status, text));
            last_indent = Some(indent);
            i += 1;
            continue;
        }
        if let (Some(li), Some(last)) = (last_indent, out.last_mut())
            && l.len() - l.trim_start().len() > li
        {
            last.3.push(' ');
            last.3.push_str(t);
            i += 1;
            continue;
        }
        last_indent = None;
        if t.starts_with('#') || t.starts_with('>') || t == "---" || t == "***" {
            flush(&mut para, &mut out);
            out.push((0, Kind::Para, None, t.to_string()));
        } else {
            para.push(t.to_string());
        }
        i += 1;
    }
    flush(&mut para, &mut out);
    // Depth never jumps more than one below the line above.
    let mut prev: isize = -1;
    for l in out.iter_mut() {
        if l.1 == Kind::Para {
            l.0 = 0;
            prev = -1;
            continue;
        }
        l.0 = l.0.min((prev + 1) as usize);
        prev = l.0 as isize;
    }
    (out, images)
}

/// The bytes of a block's marker drawn in the hang, not as text: a heading's `## `, a quote's
/// `> `, a numbered item's `12. `.
pub fn marker_len<Id>(l: &Block<Id>) -> usize {
    let t = l.text.as_str();
    if l.kind == Kind::Para {
        for m in ["### ", "## ", "# ", "> "] {
            if t.starts_with(m) {
                return m.len();
            }
        }
        return 0;
    }
    if l.kind == Kind::Bullet {
        let digits = t.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && digits <= 3 && (t[digits..].starts_with(". ") || t[digits..].starts_with(") ")) {
            return digits + 2;
        }
    }
    0
}

/// A task's cell as Markdown: `[ ] ` open, `[/] ` doing, `[w] ` waiting, `[x] ` done, `[-] `
/// cancelled.
pub fn checkbox(status: Option<&str>) -> &'static str {
    match status {
        Some("todo") => "[ ] ",
        Some("doing") => "[/] ",
        Some("waiting") => "[w] ",
        Some("done") => "[x] ",
        Some("cancelled") => "[-] ",
        _ => "",
    }
}

/// Blocks as Markdown (copy). Each entry is a block, the part of its text that's selected, and a
/// suffix the host adds after the text (thc: its fields as tokens, ` due:2026-10-09 !high`).
pub fn to_markdown<Id>(parts: &[(&Block<Id>, &str, &str)]) -> String {
    let base = parts
        .iter()
        .filter(|(l, _, _)| l.kind != Kind::Para)
        .map(|(l, _, _)| l.depth)
        .min()
        .unwrap_or(0);
    let mut out = String::new();
    let mut prev_para: Option<bool> = None;
    for (l, text, suffix) in parts {
        let para = l.kind == Kind::Para;
        if let Some(p) = prev_para
            && (para || p)
        {
            out.push('\n');
        }
        if para {
            out.push_str(text);
            out.push_str(suffix);
        } else {
            let pad = "  ".repeat(l.depth.saturating_sub(base));
            let cell = if l.kind == Kind::Task {
                checkbox(l.status.as_deref())
            } else {
                ""
            };
            let mut rows = text.split('\n');
            out.push_str(&format!(
                "{pad}- {cell}{}{}",
                rows.next().unwrap_or(""),
                suffix
            ));
            for r in rows {
                out.push_str(&format!("\n{pad}  {r}"));
            }
        }
        out.push('\n');
        prev_para = Some(para);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_reads_an_outline_and_copy_writes_it_back() {
        let md = "Intro line\ncontinued\n\n- one\n  - [ ] two\n- [x] three\n";
        let (lines, images) = parse_paste(md, false);
        assert_eq!(images, 0);
        let shape: Vec<(usize, Kind, Option<&str>, &str)> = lines
            .iter()
            .map(|(d, k, s, t)| (*d, *k, s.as_deref(), t.as_str()))
            .collect();
        assert_eq!(
            shape[0],
            (0, Kind::Para, None, "Intro line continued"),
            "a paragraph's lines are joined"
        );
        assert_eq!(shape[1], (0, Kind::Bullet, None, "one"));
        assert_eq!(shape[2].0, 1);
        assert_eq!(
            (shape[2].1, shape[2].2, shape[2].3),
            (Kind::Task, Some("todo"), "two")
        );
        assert_eq!((shape[3].1, shape[3].2), (Kind::Task, Some("done")));
        let blocks: Vec<Block<u32>> = lines
            .iter()
            .enumerate()
            .map(|(i, (d, k, s, t))| Block {
                id: i as u32,
                depth: *d,
                kind: *k,
                status: s.clone(),
                text: t.clone(),
                folded: false,
                gap: None,
            })
            .collect();
        let parts: Vec<(&Block<u32>, &str, &str)> =
            blocks.iter().map(|b| (b, b.text.as_str(), "")).collect();
        let out = to_markdown(&parts);
        assert!(
            out.contains("- [ ] two")
                && out.contains("- [x] three")
                && out.starts_with("Intro line continued"),
            "{out}"
        );
        assert_eq!(parse_paste(&out, false).0, lines, "round trip");
    }
}
