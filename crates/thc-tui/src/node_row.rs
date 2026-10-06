//! One widget renders every list row (tui-handoff §4):
//!
//! ```text
//! col 0  gutter (▌ cursor / • live / ≠ conflict)
//! col 2-6  short id (muted)        col 9-11  status [ ] [/] [w] [x] [-] or " · "
//! col 13…  text (min 24 cells)     meta right-aligned ending at w-2; col w-1 blank
//! ```

use crate::theme::{Theme, Token};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

pub const MIN_TEXT: usize = 24;

pub fn width(spans: &[Span]) -> usize {
    spans.iter().map(|s| UnicodeWidthStr::width(s.content.as_ref())).sum()
}

/// Truncate spans to `max` cells, ending in `ell` (which inherits the cut span's style).
pub fn truncate_spans(spans: &[Span<'static>], max: usize, ell: &str) -> Vec<Span<'static>> {
    if width(spans) <= max {
        return spans.to_vec();
    }
    let budget = max.saturating_sub(UnicodeWidthStr::width(ell));
    let mut out = Vec::new();
    let mut used = 0;
    for s in spans {
        let mut buf = String::new();
        for c in s.content.chars() {
            let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            if used + cw > budget {
                buf.push_str(ell);
                out.push(Span::styled(buf, s.style));
                return out;
            }
            used += cw;
            buf.push(c);
        }
        out.push(Span::styled(buf, s.style));
    }
    out
}

/// Pick the first meta variant that fits next to the text; otherwise use the last variant and
/// truncate the text (never below MIN_TEXT), then the meta's tail.
pub fn fit(text: &[Span<'static>], metas: &[Vec<Span<'static>>], avail: usize, ell: &str) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
    let tw = width(text);
    for m in metas {
        let mw = width(m);
        if tw + if mw > 0 { 2 + mw } else { 0 } <= avail {
            return (text.to_vec(), m.clone());
        }
    }
    let meta = metas.last().cloned().unwrap_or_default();
    let mw = width(&meta);
    let gap = if mw > 0 { 2 } else { 0 };
    let text_budget = avail.saturating_sub(gap + mw).max(MIN_TEXT.min(avail.saturating_sub(gap + mw.min(4))));
    let text = truncate_spans(text, text_budget, ell);
    let room = avail.saturating_sub(width(&text) + gap);
    let meta = if mw > room { truncate_spans(&meta, room, ell) } else { meta };
    (text, meta)
}

pub struct Meta {
    /// Variants from fullest to most compact (tui-handoff §4.2).
    pub variants: Vec<Vec<Span<'static>>>,
}

pub enum Gutter {
    None,
    Cursor { focused: bool },
    Live { agent: bool },
    Conflict,
}

pub struct RowSpec {
    pub gutter: Gutter,
    pub selected: bool,
    pub id: Option<String>,
    /// Extra cells before the id (outline indent).
    pub indent: usize,
    pub fold: Option<bool>,
    pub status: Span<'static>,
    pub text: Vec<Span<'static>>,
    pub meta: Meta,
    /// Outline rows without IDs (tui-handoff §10.5): a fixed fold column, so status cells line up
    /// whether or not a row folds. Gutter x0, indent, fold x0+2, status x0+4, text x0+8.
    pub fold_col: bool,
}

fn gutter_span(th: &Theme, gutter: &Gutter) -> Span<'static> {
    let g = th.glyphs();
    match gutter {
        Gutter::Conflict => Span::styled(g.conflict.to_string(), th.s(Token::Conflict)),
        Gutter::Cursor { focused } => {
            let st = th.s(Token::Accent).add_modifier(Modifier::BOLD);
            Span::styled(g.cursor.to_string(), if *focused { st } else { st.remove_modifier(Modifier::empty()) })
        }
        Gutter::Live { agent } => Span::styled(g.live.to_string(), if *agent { th.s(Token::Agent) } else { th.s(Token::Accent) }),
        Gutter::None => Span::raw(" "),
    }
}

/// The cells before a row's text: gutter, fold, indent, id, status, and one space.
pub fn prefix(th: &Theme, spec: &RowSpec) -> Vec<Span<'static>> {
    let g = th.glyphs();
    let mut left: Vec<Span<'static>> = Vec::new();
    if spec.fold_col {
        left.push(gutter_span(th, &spec.gutter));
        left.push(Span::raw(" ".repeat(1 + spec.indent)));
        let fold = match spec.fold {
            Some(true) => g.fold_closed,
            Some(false) => g.fold_open,
            None => " ",
        };
        left.push(Span::styled(format!("{fold} "), th.s(Token::Muted)));
        left.push(spec.status.clone());
        left.push(Span::raw(" "));
        return left;
    }
    left.push(gutter_span(th, &spec.gutter));
    // Top-level outline rows with children carry the fold glyph in the gap column.
    match (spec.indent, spec.fold) {
        (0, Some(collapsed)) => left.push(Span::styled(if collapsed { g.fold_closed } else { g.fold_open }.to_string(), th.s(Token::Muted))),
        _ => left.push(Span::raw(" ")),
    }
    if spec.indent > 0 {
        let fold = match spec.fold {
            Some(true) => g.fold_closed,
            Some(false) => g.fold_open,
            None => " ",
        };
        let pad = spec.indent.saturating_sub(2);
        left.push(Span::raw(" ".repeat(pad)));
        left.push(Span::styled(format!("{fold} "), th.s(Token::Muted)));
    }
    match &spec.id {
        Some(id) => left.push(Span::styled(format!("{id:<5}  "), th.s(Token::Muted))),
        None => {}
    }
    left.push(spec.status.clone());
    left.push(Span::raw(" "));
    left
}

pub fn render(th: &Theme, spec: RowSpec, w: usize) -> Line<'static> {
    let g = th.glyphs();
    let mut left = prefix(th, &spec);
    let used = width(&left);
    let avail = w.saturating_sub(used + 1);
    let (text, meta) = fit(&spec.text, &spec.meta.variants, avail, g.ellipsis);
    let mw = width(&meta);
    let pad = w.saturating_sub(used + width(&text) + mw + 1);
    left.extend(text);
    left.push(Span::raw(" ".repeat(pad)));
    left.extend(meta);
    left.push(Span::raw(" "));
    finish(th, left, spec.selected, w)
}

/// Apply the cursor-row treatment: ANSI strips colors and reverses; truecolor paints `selection`.
pub fn finish(th: &Theme, mut spans: Vec<Span<'static>>, selected: bool, w: usize) -> Line<'static> {
    if !selected {
        return Line::from(spans);
    }
    let used = width(&spans);
    if used < w {
        spans.push(Span::raw(" ".repeat(w - used)));
    }
    if th.is_ansi() {
        let spans: Vec<Span> = spans
            .into_iter()
            .enumerate()
            .map(|(i, s)| {
                // Keep the gutter glyph readable; strip colors elsewhere.
                let base = Style { fg: None, bg: None, ..s.style };
                Span::styled(s.content, if i == 0 { base } else { base.add_modifier(Modifier::REVERSED) })
            })
            .collect();
        Line::from(spans)
    } else {
        Line::from(spans).patch_style(th.fill(Token::Selection))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(s: &str) -> Vec<Span<'static>> {
        vec![Span::raw(s.to_string())]
    }

    fn text_of(v: &[Span]) -> String {
        v.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn full_meta_when_it_fits() {
        let (t, m) = fit(&plain("Water plants"), &[plain("today · ↻ every 3 days · ¶ Home")], 66, "…");
        assert_eq!(text_of(&t), "Water plants");
        assert_eq!(text_of(&m), "today · ↻ every 3 days · ¶ Home");
    }

    #[test]
    fn drops_context_before_truncating_text() {
        let metas = [plain("due tomorrow · ◆ claude · ¶ Reading List"), plain("due tomorrow · ◆ claude")];
        // 80 cols: avail = 80 - 13 - 1 = 66
        let (t, m) = fit(&plain("Summarize unread newsletters into [[Reading List]]"), &metas, 66, "…");
        assert_eq!(text_of(&m), "due tomorrow · ◆ claude");
        assert_eq!(text_of(&t), "Summarize unread newsletters into [[Read…");
        assert!(width(&t) + 2 + width(&m) <= 66);
    }

    #[test]
    fn text_never_below_min_and_meta_tail_truncates() {
        let (t, m) = fit(&plain("A very long task title that keeps going"), &[plain("due Oct 10 (in 7d) · !high · ↻ every month on the 15th")], 40, "…");
        assert_eq!(width(&t), MIN_TEXT);
        assert!(width(&t) + 2 + width(&m) <= 40, "{} + {}", width(&t), width(&m));
        assert!(text_of(&m).starts_with("due Oct 10"));
    }
}
