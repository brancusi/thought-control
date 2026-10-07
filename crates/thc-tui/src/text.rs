//! Text measured as a terminal draws it: grapheme clusters, their cell widths, and wrapping
//! into rows by words. thc-tui's own copy, so the TUI's chrome and layout don't reach into
//! the editor engine for them.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// The start of the character before `i`: a grapheme cluster, what a person sees as one
/// character (👨‍👩‍👧, 🇯🇵, e + ◌́). The caret, Backspace and Delete step by these (jank A3–A5).
pub fn prev_char(s: &str, i: usize) -> usize {
    s[..i]
        .grapheme_indices(true)
        .next_back()
        .map(|(k, _)| k)
        .unwrap_or(0)
}

/// The end of the character starting at `i` (a grapheme cluster).
pub fn next_char(s: &str, i: usize) -> usize {
    s[i..]
        .graphemes(true)
        .next()
        .map(|g| i + g.len())
        .unwrap_or(i)
}

/// One character's columns, as ratatui draws it: the cluster's width (👨‍👩‍👧 and ❤️ are 2).
pub fn gwidth(g: &str) -> usize {
    if g == "\n" {
        0
    } else {
        UnicodeWidthStr::width(g)
    }
}

/// Display width of a string (soft breaks excluded).
pub fn width(s: &str) -> usize {
    // Per cluster, the way ratatui measures what it draws: anything else puts the caret, the
    // wrap and the meta off by the difference (jank A1, A2, A6–A8).
    s.graphemes(true).map(gwidth).sum()
}

/// Wrap text at `w` columns by words; returns byte ranges of the visual rows. A soft break
/// starts a new row. Long words break, between characters (never inside a cluster).
pub fn wrap(text: &str, w: usize) -> Vec<(usize, usize)> {
    wrap_with(text, w, 0)
}

/// [`wrap`], with `first_extra` more columns on the very first row: a heading's `## ` or a
/// numbered item's `12. ` is drawn in the hang, so the text beside it gets that room back.
pub fn wrap_with(text: &str, w: usize, first_extra: usize) -> Vec<(usize, usize)> {
    let w = w.max(8);
    let mut rows = Vec::new();
    let mut start = 0;
    for seg in text.split_inclusive('\n') {
        let seg_end = start + seg.len();
        let body_end = if seg.ends_with('\n') {
            seg_end - 1
        } else {
            seg_end
        };
        let mut row_start = start;
        let mut col = 0;
        let mut last_space: Option<usize> = None;
        let mut i = start;
        while i < body_end {
            let g = text[i..body_end].graphemes(true).next().unwrap();
            let cw = gwidth(g);
            let limit = if row_start == 0 { w + first_extra } else { w };
            // A space past the edge ends the row there (a word that fills the row exactly
            // stays on it).
            if col + cw > limit && g == " " {
                rows.push((row_start, i + 1));
                row_start = i + 1;
                col = 0;
                last_space = None;
                i += 1;
                continue;
            }
            if col + cw > limit {
                let cut = match last_space {
                    Some(sp) if sp > row_start => sp + 1,
                    _ => i,
                };
                rows.push((row_start, cut.min(body_end)));
                row_start = cut;
                col = width(&text[row_start..i]);
                last_space = None;
                continue;
            }
            if g == " " {
                last_space = Some(i);
            }
            col += cw;
            i += g.len();
        }
        rows.push((row_start, body_end));
        start = seg_end;
    }
    // A soft break at the end starts an empty row: the caret goes there (⇧Enter / ⌃J at the
    // end of a line used to leave it on the line above).
    if rows.is_empty() || text.ends_with('\n') {
        rows.push((text.len(), text.len()));
    }
    rows
}
