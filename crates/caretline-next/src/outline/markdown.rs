//! Markdown in and out of an outline: pasted Markdown read into blocks, a selection written as
//! Markdown for the clipboard, and a whole outline document read from and written to a
//! Markdown file.

use crate::marks::BlockAttrs;
use crate::outline::{default_gap, derive, numbered_marker, parse_str, BlockInfo, Hang, Kind, NewBlock, Outline, OutlineConfig};
use crate::state::{State, Viewport};

/// Markdown read as blocks: paragraphs (their lines joined), `-` `*` `+` bullets nested by
/// their first indent, numbered items, `- [ ]` tasks (a bare `[ ] ` too), headings, quotes
/// and rules as one-line paragraphs, and fences, tables and front matter as one paragraph
/// with its line breaks. Images are left out and counted. `plain`: a paragraph per block of
/// lines, line breaks kept, nothing read as a list.
pub fn parse_markdown(input: &str, plain: bool) -> (Vec<NewBlock>, usize) {
    let text = input.replace("\r\n", "\n").replace('\r', "\n");
    let mut raw: Vec<String> = text.split('\n').map(|l| l.replace('\t', "    ")).collect();
    while raw.last().is_some_and(|l| l.trim().is_empty()) {
        raw.pop();
    }
    while raw.first().is_some_and(|l| l.trim().is_empty()) {
        raw.remove(0);
    }
    let mut out: Vec<NewBlock> = Vec::new();
    if plain {
        let mut block: Vec<String> = Vec::new();
        for l in raw.into_iter().chain(std::iter::once(String::new())) {
            if l.trim().is_empty() {
                if !block.is_empty() {
                    out.push(NewBlock::para(&block.join("\n")));
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
            let Some(close) = l[a..].find(')').map(|k| a + k) else { break };
            l.replace_range(a..=close, "");
            images += 1;
        }
        let trimmed = l.trim_end().len();
        l.truncate(trimmed);
    }
    // (indent, marker, body) of a list line.
    let item = |l: &str| -> Option<(usize, String, String)> {
        let indent = l.len() - l.trim_start().len();
        let rest = l.trim_start();
        if let Some(b) = rest.strip_prefix("- ").or_else(|| rest.strip_prefix("* ")).or_else(|| rest.strip_prefix("+ ")) {
            return Some((indent, "-".into(), b.to_string()));
        }
        if CHECKBOXES.iter().any(|(p, _)| rest.starts_with(p)) {
            return Some((indent, "-".into(), rest.to_string()));
        }
        let n = numbered_marker(rest)?;
        Some((indent, rest[..n - 1].to_string(), rest[n..].to_string()))
    };
    let unit = raw.iter().filter_map(|l| item(l)).map(|(i, _, _)| i).find(|i| *i > 0).unwrap_or(2);
    let mut para: Vec<String> = Vec::new();
    let flush = |para: &mut Vec<String>, out: &mut Vec<NewBlock>| {
        if !para.is_empty() {
            out.push(NewBlock::para(&para.join(" ")));
            para.clear();
        }
    };
    let mut i = 0;
    if raw.first().map(String::as_str) == Some("---") {
        if let Some(close) = raw.iter().skip(1).position(|l| l == "---").map(|k| k + 1) {
            let mut body = vec!["```".to_string()];
            body.extend(raw[1..close].iter().cloned());
            body.push("```".into());
            out.push(NewBlock::para(&body.join("\n")));
            i = close + 1;
        }
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
            out.push(NewBlock::para(&body.join("\n")));
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
            out.push(NewBlock::para(&rows.join("\n")));
            last_indent = None;
            continue;
        }
        if let Some((indent, marker, body)) = item(&l) {
            flush(&mut para, &mut out);
            let mut nb = NewBlock { depth: (indent / unit) as u16, kind: Kind::Bullet, status: None, text: body, gap: None, mark: None };
            for (p, c) in CHECKBOXES {
                if let Some(rest) = nb.text.strip_prefix(p) {
                    nb.kind = Kind::Task;
                    nb.status = Some(c);
                    nb.text = rest.to_string();
                    break;
                }
            }
            if nb.kind == Kind::Bullet && marker != "-" {
                nb.text = format!("{marker} {}", nb.text);
            }
            out.push(nb);
            last_indent = Some(indent);
            i += 1;
            continue;
        }
        if let (Some(li), Some(last)) = (last_indent, out.last_mut()) {
            if l.len() - l.trim_start().len() > li {
                last.text.push(' ');
                last.text.push_str(t);
                i += 1;
                continue;
            }
        }
        last_indent = None;
        if t.starts_with('#') || t.starts_with('>') || t == "---" || t == "***" {
            flush(&mut para, &mut out);
            out.push(NewBlock::para(t));
        } else {
            para.push(t.to_string());
        }
        i += 1;
    }
    flush(&mut para, &mut out);
    // Depth never jumps more than one below the item above.
    let mut prev: isize = -1;
    for b in out.iter_mut() {
        if b.kind == Kind::Para {
            b.depth = 0;
            prev = -1;
            continue;
        }
        b.depth = b.depth.min((prev + 1) as u16);
        prev = b.depth as isize;
    }
    (out, images)
}

const CHECKBOXES: [(&str, char); 6] = [("[ ] ", ' '), ("[x] ", 'x'), ("[X] ", 'x'), ("[/] ", '/'), ("[-] ", '-'), ("[w] ", 'w')];

/// The selection `[from, to)` as Markdown (a copy). Inside one block: its plain text. Across
/// blocks: the first block's text from `from` (with its marker only when `from` is its
/// content start), then every later block with its marker and indentation, a blank line
/// around paragraphs and none between list items. Soft breaks are line breaks.
pub fn to_markdown(state: &State, o: &Outline, from: usize, to: usize) -> String {
    let text = state.text.slice(..);
    let piece = |a: usize, b: usize| text.slice(a..b.max(a)).to_string().replace("\r\n", "\n");
    let parts: Vec<(&BlockInfo, String)> = o
        .indices_between(text, from, to)
        .map(|i| {
            let b = &o.blocks[i];
            (b, piece(from.max(b.content_start()), to.min(b.end)))
        })
        .collect();
    if parts.len() == 1 {
        return parts[0].1.clone();
    }
    let base = parts.iter().filter(|(b, _)| b.kind != Kind::Para).map(|(b, _)| b.depth).min().unwrap_or(0);
    let first = parts[0].0;
    let from_start = from <= first.content_start();
    let mut out = String::new();
    let mut prev_para: Option<bool> = None;
    for (k, (b, part)) in parts.iter().enumerate() {
        let para = b.kind == Kind::Para;
        if prev_para.is_some_and(|p| para || p) {
            out.push('\n');
        }
        let with_marker = k > 0 || from_start;
        if para {
            if with_marker && matches!(b.hang, Hang::Heading(_) | Hang::Quote) {
                out.push_str(&piece(b.start + b.indent, b.content_start()));
            }
            out.push_str(part);
        } else {
            let pad = "  ".repeat(b.depth.saturating_sub(base) as usize);
            let marker = match (b.kind, b.hang, b.status) {
                (Kind::Task, _, Some(c)) => format!("- [{c}] "),
                (_, Hang::Number(_), _) => piece(b.start + b.indent, b.content_start()),
                _ => "- ".into(),
            };
            let mut rows = part.split('\n');
            if with_marker {
                out.push_str(&pad);
                out.push_str(&marker);
            }
            out.push_str(rows.next().unwrap_or(""));
            for r in rows {
                out.push('\n');
                out.push_str(&pad);
                out.push_str("  ");
                out.push_str(r);
            }
        }
        out.push('\n');
        prev_para = Some(para);
    }
    out.strip_suffix('\n').unwrap_or(&out).to_string()
}

/// The continuation indent of a block in a Markdown file: past an item's marker (for a task,
/// past its `- `), or the paragraph's indentation.
fn continuation_indent(b: &BlockInfo) -> usize {
    match b.kind {
        Kind::Para => b.indent,
        Kind::Task => b.indent + 2,
        Kind::Bullet => b.prefix_len,
    }
}

/// The whole outline document as a Markdown file: a blank line where a block has a blank row
/// before it, continuation lines indented under their item, and a final line break. Empty
/// paragraphs (a fresh line to type on) are left out.
pub fn to_file(state: &State) -> String {
    let Some(o) = state.blocks() else { return state.text.to_string() };
    let text = state.text.slice(..);
    let mut lines: Vec<String> = Vec::new();
    for b in &o.blocks {
        if b.kind == Kind::Para && b.hang == Hang::None && b.is_empty() && b.line_count == 1 {
            continue;
        }
        if b.gap && !lines.is_empty() {
            lines.push(String::new());
        }
        let pad = " ".repeat(continuation_indent(b));
        for (k, line) in (b.first_line..=b.last_line()).enumerate() {
            let s = text.line(line).to_string();
            let s = s.trim_end_matches(['\n', '\r']);
            if k == 0 || b.fence || s.is_empty() {
                lines.push(s.to_string());
            } else {
                lines.push(format!("{pad}{s}"));
            }
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Reads a Markdown file into outline buffer text and its block starts: `(text, starts)`,
/// each start a line index and whether a blank line came before it. Blank lines separate
/// blocks (outside fences), a marker line starts a block, and any other line continues the
/// block above (its continuation indent removed). Line breaks become `\n`.
pub fn from_file(md: &str, cfg: &OutlineConfig) -> (String, Vec<(usize, bool)>) {
    let md = md.replace("\r\n", "\n");
    let mut out: Vec<String> = Vec::new();
    let mut starts: Vec<(usize, bool)> = Vec::new();
    let mut blank = false;
    let mut in_fence = false;
    let mut cont = 0usize;
    for raw in md.split('\n') {
        if in_fence {
            out.push(raw.to_string());
            if raw.trim_start().starts_with("```") {
                in_fence = false;
            }
            continue;
        }
        if raw.trim().is_empty() {
            blank = true;
            continue;
        }
        let p = parse_str(raw, cfg);
        if out.is_empty() || blank || p.marker {
            starts.push((out.len(), blank && !out.is_empty()));
            out.push(raw.to_string());
            cont = match p.kind {
                Kind::Para => p.indent,
                Kind::Task => p.indent + 2,
                Kind::Bullet => p.len,
            };
            in_fence = p.fence;
        } else {
            let strip = raw.chars().take(cont).take_while(|c| *c == ' ').count();
            out.push(raw.chars().skip(strip).collect());
        }
        blank = false;
    }
    (out.join("\n"), starts)
}

/// An outline document from a Markdown file's text: blocks, marks and blank rows as the file
/// has them, a fresh history, and the document counted as saved.
pub fn load(md: &str, path: Option<String>, viewport: Viewport, cfg: OutlineConfig) -> State {
    let (text, starts) = from_file(md, &cfg);
    let mut state = State::new(&text, path, viewport);
    for &(line, _) in &starts {
        let pos = state.text.line_to_char(line);
        state.marks.mint(pos);
    }
    let o = derive(state.text.slice(..), &state.marks, &cfg);
    let blank: std::collections::HashMap<usize, bool> = starts.into_iter().collect();
    for (i, b) in o.blocks.iter().enumerate() {
        if i == 0 {
            continue;
        }
        let want = blank.get(&b.first_line).copied().unwrap_or(false);
        let default = default_gap(o.blocks.get(i - 1), b);
        if want != default {
            state.marks.set_attrs(b.id, BlockAttrs { gap: Some(want) });
        }
    }
    state.enable_outline(cfg);
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_reads_an_outline() {
        let (blocks, images) = parse_markdown("Intro line\ncontinued\n\n- one\n  - [ ] two\n- [x] three\n1. first\n![x](y.png)\n", false);
        assert_eq!(images, 1);
        let shape: Vec<(u16, Kind, Option<char>, &str)> = blocks.iter().map(|b| (b.depth, b.kind, b.status, b.text.as_str())).collect();
        assert_eq!(
            shape,
            [
                (0, Kind::Para, None, "Intro line continued"),
                (0, Kind::Bullet, None, "one"),
                (1, Kind::Task, Some(' '), "two"),
                (0, Kind::Task, Some('x'), "three"),
                (0, Kind::Bullet, None, "1. first"),
            ]
        );
    }

    #[test]
    fn a_file_round_trips() {
        let md = "# Trip\n\nBooked the flat.\nIt faces the river.\n\n- [ ] Pay the deposit\n  - ask about the desk\n    on two lines\n- [x] Book flights\n\n```\n- not a list\n\n```\n";
        let s = load(md, None, Viewport { width: 80, height: 24 }, OutlineConfig::default());
        assert_eq!(to_file(&s), md);
        let o = s.blocks().unwrap();
        let kinds: Vec<Kind> = o.blocks.iter().map(|b| b.kind).collect();
        assert_eq!(kinds, [Kind::Para, Kind::Para, Kind::Task, Kind::Bullet, Kind::Task, Kind::Para]);
        assert_eq!(o.blocks[3].line_count, 2, "the continuation line belongs to its item");
    }
}
