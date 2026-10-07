//! Positions and diffs over a document's text.
//!
//! The engine counts in chars (Unicode scalar values). Agents count in lines and columns, both
//! 1-based, columns in chars: `{line: 1, col: 1}` is the first char of the document.

use serde_json::{json, Value};

/// A line and column, both 1-based; the column counts chars.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

impl Pos {
    pub fn json(self) -> Value {
        json!({ "line": self.line, "col": self.col })
    }
}

/// The char index where each line starts. A text ending in a line break has an empty last
/// line after it.
pub fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (i, c) in text.chars().enumerate() {
        if c == '\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// The number of lines an agent sees: a final line break doesn't start another line.
pub fn line_count(text: &str) -> usize {
    let n = line_starts(text).len();
    if text.ends_with('\n') { n - 1 } else { n }.max(1)
}

/// The chars of line `i` (0-based) without its line break.
fn line_len(text_chars: &[char], starts: &[usize], i: usize) -> usize {
    let start = starts[i];
    let mut end = starts.get(i + 1).map(|&s| s - 1).unwrap_or(text_chars.len());
    if end > start && text_chars[end - 1] == '\r' {
        end -= 1;
    }
    end - start
}

/// The char index of a line and column. The column may be one past the line's last char
/// (the end of the line); the line may be one past the last line when the text ends in a
/// line break (where a new last line would go).
pub fn to_char(text: &str, pos: Pos) -> Result<usize, String> {
    let chars: Vec<char> = text.chars().collect();
    let starts = line_starts(text);
    if pos.line == 0 || pos.col == 0 {
        return Err(format!("line {}, col {}: lines and columns start at 1", pos.line, pos.col));
    }
    let i = pos.line - 1;
    if i >= starts.len() {
        return Err(format!("line {} is past the end: the document has {} lines", pos.line, line_count(text)));
    }
    let len = line_len(&chars, &starts, i);
    if pos.col - 1 > len {
        return Err(format!(
            "line {} has {len} chars, so col {} is past its end (col {} is the end of the line)",
            pos.line,
            pos.col,
            len + 1
        ));
    }
    Ok(starts[i] + pos.col - 1)
}

/// The line and column of a char index (clamped to the text).
pub fn to_pos(text: &str, at: usize) -> Pos {
    let starts = line_starts(text);
    let at = at.min(text.chars().count());
    let i = match starts.binary_search(&at) {
        Ok(i) => i,
        Err(i) => i - 1,
    };
    Pos { line: i + 1, col: at - starts[i] + 1 }
}

/// Where `needle` occurs in `text`, as char ranges.
pub fn find_all(text: &str, needle: &str) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return Vec::new();
    }
    let n = needle.chars().count();
    let mut out = Vec::new();
    let mut byte = 0;
    while let Some(found) = text[byte..].find(needle) {
        let b = byte + found;
        let from = text[..b].chars().count();
        out.push((from, from + n));
        byte = b + needle.len();
    }
    out
}

/// Lines `from..=to` (1-based, clamped) of `text`.
pub fn slice_lines(text: &str, from: usize, to: usize) -> (usize, usize, String) {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let count = line_count(text);
    let from = from.clamp(1, count);
    let to = to.clamp(from, count);
    let body: String = lines.iter().skip(from - 1).take(to - from + 1).copied().collect();
    (from, to, body)
}

/// Lines with their numbers, `   12│ text`.
pub fn numbered(text: &str, first: usize) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let n = if text.ends_with('\n') { lines.len() - 1 } else { lines.len() };
    let width = (first + n).to_string().len();
    lines[..n]
        .iter()
        .enumerate()
        .map(|(i, l)| format!("{:>width$}│ {l}\n", first + i))
        .collect()
}

/// What changed between two texts, by lines: the first and last line that differ on each
/// side, and those lines (at most `max` each way).
pub fn diff(old: &str, new: &str, max: usize) -> Option<Value> {
    if old == new {
        return None;
    }
    let a: Vec<&str> = old.split('\n').collect();
    let b: Vec<&str> = new.split('\n').collect();
    let pre = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let rest = a.len().min(b.len()) - pre;
    let suf = a.iter().rev().zip(b.iter().rev()).take(rest).take_while(|(x, y)| x == y).count();
    let removed = &a[pre..a.len() - suf];
    let added = &b[pre..b.len() - suf];
    let show = |ls: &[&str]| -> Vec<String> {
        let mut v: Vec<String> = ls.iter().take(max).map(|s| s.to_string()).collect();
        if ls.len() > max {
            v.push(format!("… {} more lines", ls.len() - max));
        }
        v
    };
    let summary = match (removed.len(), added.len()) {
        (0, n) => format!("{n} line(s) inserted at line {}", pre + 1),
        (n, 0) => format!("{n} line(s) removed at line {}", pre + 1),
        (r, n) if r == n => format!("line(s) {}-{} changed", pre + 1, pre + n),
        (r, n) => format!("{r} line(s) at line {} replaced by {n}", pre + 1),
    };
    Some(json!({
        "summary": summary,
        "first_line": pre + 1,
        "old_lines": show(removed),
        "new_lines": show(added),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_round_trip() {
        let t = "héllo\nwörld\n";
        assert_eq!(to_char(t, Pos { line: 1, col: 1 }), Ok(0));
        assert_eq!(to_char(t, Pos { line: 2, col: 2 }), Ok(7));
        assert_eq!(to_char(t, Pos { line: 2, col: 6 }), Ok(11));
        assert!(to_char(t, Pos { line: 2, col: 7 }).is_err());
        assert_eq!(to_char(t, Pos { line: 3, col: 1 }), Ok(12));
        assert!(to_char(t, Pos { line: 4, col: 1 }).is_err());
        for i in 0..=12 {
            assert_eq!(to_char(t, to_pos(t, i)), Ok(i));
        }
        assert_eq!(line_count(t), 2);
        assert_eq!(line_count(""), 1);
    }

    #[test]
    fn finds_in_chars() {
        assert_eq!(find_all("ä wrold wrold", "wrold"), vec![(2, 7), (8, 13)]);
        assert!(find_all("abc", "").is_empty());
    }

    #[test]
    fn diffs_by_line() {
        let d = diff("a\nb\nc\n", "a\nB\nc\n", 5).unwrap();
        assert_eq!(d["summary"], "line(s) 2-2 changed");
        assert_eq!(d["old_lines"][0], "b");
        assert_eq!(d["new_lines"][0], "B");
        assert!(diff("x", "x", 5).is_none());
        assert_eq!(diff("a\n", "a\nb\n", 5).unwrap()["summary"], "1 line(s) inserted at line 2");
    }

    #[test]
    fn numbers_lines() {
        assert_eq!(numbered("a\nb\n", 9), " 9│ a\n10│ b\n");
        assert_eq!(slice_lines("a\nb\nc", 2, 9), (2, 3, "b\nc".to_string()));
    }
}
