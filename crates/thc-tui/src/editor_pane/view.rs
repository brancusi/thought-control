//! Drawing the document (tui-editor.md §3): marks, hang, a text column wrapped at
//! min(72, available), the meta right-aligned (or on its own row), the journal header and the
//! page title. The caret is the terminal's cursor; the selection is the `sel` fill.
//!
//! caretline lays the document out, scrolls it and draws it (`Doc::set_view`, `Doc::frame`):
//! which rows show, what each holds, where the caret is. thc gives it the geometry (the text
//! column, rows after a note for a meta or an image) and styles what the frame shows.

use crate::app::App;
use crate::ui::RenderOutput;
use crate::editor::{DocRow, Line, Target, ViewGeometry};
use crate::text::width;
use crate::theme::Token;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span};
use ratatui::widgets::Paragraph;
use thc_core::outline::Kind;
use thc_core::tui_config::{El, FocusSet};

const MARKS: usize = crate::editor::MARKS as usize;
/// `Line::conflict_with` for a line moved here because its parent was deleted elsewhere.
pub const MOVED_HERE: &str = "\u{1}moved here";
const HANG: usize = crate::editor::HANG as usize;
const META: usize = 24;
const BLOCK: usize = MARKS + HANG + 72 + 2 + META;

/// The content width a document gets: the full width, or less the detail pane when it's open
/// on a wide terminal.
pub fn pane_width(screen: u16, show_detail: bool) -> u16 {
    if show_detail && screen >= 130 { screen - 36 } else { screen }
}

/// The text column's width for a line at `depth` (each level indents 4).
pub fn text_width(ctx: DocContext, screen: u16, show_detail: bool, depth: usize) -> usize {
    let w = (pane_width(screen, show_detail) as usize).saturating_sub(rail(ctx));
    let col = focus(ctx).map_or_else(|| col_beside(ctx, w), |f| f.width);
    let base = col.min(w.saturating_sub(MARKS + HANG + 2));
    base.saturating_sub(depth * 4).max(20)
}

/// The rail's width beside the document, its rule included (0: no rail; navigation.md §3).
pub const RAIL_W: usize = 23;

fn rail(ctx: DocContext) -> usize {
    ctx.rail
}

/// The text column outside Focus: 72, or with the rail beside it, as much as fits the meta
/// too, but no less than 60 (72 again from 132 columns).
fn col_beside(ctx: DocContext, w: usize) -> usize {
    let col = if rail(ctx) == 0 { 72 } else { w.saturating_sub(MARKS + HANG + 2 + META).clamp(60, 72) };
    ctx.cap.map_or(col, |c| col.min(c))
}

/// The text column plus marks, hang and meta, outside Focus, in a document area `w` wide.
fn block_w(ctx: DocContext, w: usize) -> usize {
    MARKS + HANG + col_beside(ctx, w) + 2 + META
}

/// The month beside the text (tui-editor.md §8.3) is 20 columns, 2 apart.
const MONTH_W: usize = 20;

/// What Focus shows while it's on, for the drawing helpers (set at the top of `draw`).
#[derive(Clone, Copy)]
pub(crate) struct FocusView {
    pub set: FocusSet,
    pub width: usize,
    pub journal: bool,
}

impl FocusView {
    fn has(&self, e: El) -> bool {
        self.set.has(e)
    }

    /// The text column plus the meta (when on): what's centred.
    fn block(&self) -> usize {
        MARKS + HANG + self.width + if self.has(El::Meta) { 2 + META } else { 0 }
    }

    /// The month fits beside the block (journal only, 110 columns or more).
    fn month_fits(&self, w: usize) -> bool {
        self.has(El::Month) && self.journal && w >= 110 && self.block() + 2 + MONTH_W + 2 <= w
    }
}

fn focus(ctx: DocContext) -> Option<FocusView> {
    ctx.focus
}

fn left_edge(ctx: DocContext, w: usize) -> usize {
    // Focus centres the text column (and the meta when it's on); the month, when it fits,
    // joins the centred block on the right.
    if let Some(f) = focus(ctx) {
        let month = if f.month_fits(w) { 2 + MONTH_W } else { 0 };
        return w.saturating_sub(f.block() + month) / 2;
    }
    if rail(ctx) > 0 {
        return (w.saturating_sub(block_w(ctx, w)) / 2).max(1);
    }
    if w >= 110 { (w - BLOCK) / 2 } else { 1 }
}

/// The marker at the start of a line's text that's drawn in the hang instead (§3.1): a
/// numbered item's `12. `, a heading's `## `, a quote's `> `. Its length in bytes.
pub fn marker_len(l: &Line) -> usize {
    let t = l.text.as_str();
    if l.kind() == Kind::Para {
        for m in ["### ", "## ", "# ", "> "] {
            if t.starts_with(m) {
                return m.len();
            }
        }
        return 0;
    }
    if l.kind() == Kind::Bullet {
        let digits = t.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && digits <= 3 && (t[digits..].starts_with(". ") || t[digits..].starts_with(") ")) {
            return digits + 2;
        }
    }
    0
}

pub fn is_code(l: &Line) -> bool {
    l.kind() == Kind::Para && l.text.starts_with("```")
}

/// What a line looks like: its hang and text style (§3.2).
pub(crate) struct Form {
    pub hang: String,
    pub hang_style: Style,
    pub text_style: Style,
}

pub(crate) fn form(app: &App, l: &Line) -> Form {
    let th = app.theme;
    let dim = th.s(Token::Muted).add_modifier(Modifier::DIM);
    let mut f = Form { hang: String::new(), hang_style: th.s(Token::Muted), text_style: th.s(Token::Text) };
    match l.kind() {
        Kind::Task => {
            let (cell, tok) = match l.status.as_deref() {
                Some("done") => ("[x]", Token::Done),
                Some("doing") => ("[/]", Token::Doing),
                Some("waiting") => ("[w]", Token::Waiting),
                Some("cancelled") => ("[-]", Token::Muted),
                _ => ("[ ]", Token::Text),
            };
            f.hang = format!("{cell} ");
            f.hang_style = th.s(if l.conflict { Token::Conflict } else { tok });
            if matches!(l.status.as_deref(), Some("done") | Some("cancelled")) {
                f.text_style = dim;
            }
        }
        Kind::Bullet => {
            let m = marker_len(l);
            if m > 0 {
                // The number hangs where the bullet would.
                f.hang = format!("{} ", l.text[..m].trim_end());
            } else {
                f.hang = "  · ".into();
                // Document mode is for prose: the bullets step back.
                if app.ui.document_mode {
                    f.hang_style = dim;
                }
            }
        }
        Kind::Para => {
            let t = l.text.as_str();
            let m = marker_len(l);
            if m > 0 {
                f.hang = format!("{} ", t[..m].trim_end());
                f.hang_style = dim;
            }
            if t.starts_with("# ") {
                f.text_style = th.strong();
            } else if t.starts_with("## ") || t.starts_with("### ") {
                f.text_style = th.strong();
            } else if t.starts_with("> ") {
                f.text_style = th.s(Token::Muted);
            }
        }
    }
    f
}

/// A line that's only an attachment, `![caption](files/…)`: (caption, path).
pub fn image_line(text: &str) -> Option<(String, String)> {
    let t = text.trim();
    if !(t.starts_with("![") && t.ends_with(')')) {
        return None;
    }
    let mut r = thc_core::attach::refs(t);
    (r.len() == 1).then(|| r.remove(0))
}

pub fn human_bytes(b: u64) -> String {
    match b {
        b if b >= 1024 * 1024 => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
        b if b >= 1024 => format!("{} KB", b.div_ceil(1024)),
        b => format!("{b} B"),
    }
}

/// One row on screen, from the engine's frame: (line index, text byte range).
#[derive(Clone, Copy)]
struct Row {
    line: usize,
    start: usize,
    end: usize,
    /// The first byte the frame shows (a code block scrolled sideways starts later).
    shown: usize,
    first: bool,
    /// The meta's own row under the line (narrow, or the text runs into it).
    meta_row: bool,
    blank: bool,
    /// Rows reserved under an attachment's chip for its image (attachments.md §3): (index,
    /// rows, cols). Not caret stops; drawn blank, then the image goes over them.
    image: Option<(u16, u16, u16)>,
}

/// What a note draws after its text: its meta on its own row, an image's rows (rows, cols).
#[derive(Clone, Copy, Default)]
struct After {
    meta_row: bool,
    image: Option<(u16, u16)>,
}

/// The document laid out by the engine in a view `w` wide and `h` high: the rows on screen
/// (blank spacer rows included), the caret's cell, the lines whose meta has its own row, and
/// (all rows, the first on screen) for the scrollbar.
/// The open document's view in an area `w` wide and `h` high, before the rows its notes draw
/// after them (`layout`).
pub(crate) fn view_geometry(app: &App, w: usize, h: u16) -> ViewGeometry {
    let ctx = DocContext::from_app(app);
    let left = left_edge(ctx, w);
    let typewriter = ctx.focus.map_or(app.tui_prefs.typewriter, |f| f.has(El::Typewriter));
    ViewGeometry { width: w.saturating_sub(left).min(u16::MAX as usize) as u16, height: h, column: text_width(ctx, if app.in_panel.is_some() { w as u16 } else { app.screen_width }, app.in_panel.is_none() && app.detail_shows(), 0).min(u16::MAX as usize) as u16, extra_rows: Vec::new(), typewriter }
}

fn pane_focused(app: &App) -> bool {
    match app.in_panel.as_ref() {
        Some(key) => app.ui.focus == crate::app::Focus::Sidebar && app.ui.sidebar.active_key().as_ref() == Some(key),
        None => app.ui.focus != crate::app::Focus::Sidebar,
    }
}

fn layout(app: &mut App, w: usize, h: u16) -> (Vec<Row>, Option<(u16, u16)>, std::collections::HashSet<usize>, (usize, usize), caretline::Frame, usize) {
    let ctx = DocContext::from_app(app);
    let left = left_edge(ctx, w);
    let app_vault = app.vault.paths.vault.clone();
    let inline_images = app.derived.inline_images;
    let attachments = &app.derived.attachments;
    let sw = if app.in_panel.is_some() { w as u16 } else { app.screen_width };
    let detail = app.in_panel.is_none() && app.detail_shows();
    let mut g = view_geometry(app, w, h);
    let pending = app.doc_pending_scroll.take();
    let focused = pane_focused(app);
    let d = app.doc.as_mut().unwrap();
    if focused { d.set_view(&g); } else { d.set_view_unfocused(&g); }
    // Whether the meta gets its own row follows the saved meta, never the live chip: lines
    // below don't jump while a token is typed (the chip may run into the margin instead).
    let with_meta: Vec<(usize, String)> = d.blocks().iter().enumerate().filter(|(_, l)| l.conflict || !l.meta.is_empty()).map(|(i, l)| (i, meta_of(ctx, l, None))).filter(|(_, m)| !m.is_empty()).collect();
    let ends = d.first_row_ends(&with_meta.iter().map(|(i, _)| *i).collect::<Vec<_>>());
    let mut after: std::collections::HashMap<usize, After> = std::collections::HashMap::new();
    for ((i, meta), end) in with_meta.iter().zip(ends) {
        let l = &d.blocks()[*i];
        let last_row_end_col = width(&l.text[..end.min(l.text.len())]);
        let own = w < 60 || MARKS + HANG + l.depth * 4 + last_row_end_col + 2 > w.saturating_sub(1).saturating_sub(left).saturating_sub(width(meta));
        if own {
            after.entry(*i).or_default().meta_row = true;
        }
    }
    // An image attachment: rows under its chip, reserved whether or not the caret's on it,
    // so nothing moves while you type (attachments.md §3).
    if inline_images {
        for (i, l) in d.blocks().iter().enumerate() {
            let Some((_, path)) = image_line(&l.text) else { continue };
            if !thc_core::attach::is_image(&path) {
                continue;
            }
            if let Some((iw, ih)) = attachments.get(&(app_vault.clone(), path.clone())).and_then(|a| a.dimensions) {
                let tw = text_width(ctx, sw, detail, l.depth);
                let (cols, n) = crate::images::cells(iw, ih, tw.min(u16::MAX as usize) as u16);
                if n > 0 {
                    after.entry(i).or_default().image = Some((n, cols));
                }
            }
        }
    }
    if !after.is_empty() {
        g.extra_rows = after.iter().map(|(&i, a)| (i, a.meta_row as u16 + a.image.map_or(0, |(n, _)| n))).collect();
        g.extra_rows.sort();
        if focused { d.set_view(&g); } else { d.set_view_unfocused(&g); }
    }
    // A remembered scroll goes on now the view has its width (`App::doc_pending_scroll`).
    if let Some((row, free)) = pending {
        d.set_scroll(row, free);
    }
    let f = d.frame();
    let rows = f
        .rows
        .iter()
        .filter_map(|r| {
            let blank = Row { line: 0, start: 0, end: 0, shown: 0, first: false, meta_row: false, blank: true, image: None };
            Some(match *r {
                DocRow::Text { line, start, end, first, shown, .. } => Row { line, start, end, shown, first, ..blank }.text(),
                DocRow::Gap { line } => Row { line, ..blank },
                DocRow::Extra { line, index } => {
                    let a = after.get(&line).copied().unwrap_or_default();
                    if a.meta_row && index == 0 {
                        Row { line, meta_row: true, ..blank }.text()
                    } else {
                        let k = index - a.meta_row as u16;
                        let (n, cols) = a.image.unwrap_or((1, 0));
                        Row { line, image: Some((k, n, cols)), ..blank }.text()
                    }
                }
                DocRow::Past => return None,
            })
        })
        .collect();
    let own = after.iter().filter(|(_, a)| a.meta_row).map(|(&i, _)| i).collect();
    (rows, f.cursor, own, d.scroll_rows(), f.cn, left)
}

impl Row {
    fn text(self) -> Row {
        Row { blank: false, ..self }
    }
}

/// The meta to show: the chip while the caret's line has tokens, else the saved meta.
fn meta_of(ctx: DocContext, l: &Line, chip: Option<&str>) -> String {
    if focus(ctx).is_some_and(|f| !f.has(El::Meta)) {
        return String::new();
    }
    if let Some(chip) = chip {
        return chip.to_owned();
    }
    if l.conflict {
        // In Write `c` types: the way in is Esc, then c.
        let key = if ctx.writing { "⌃O" } else { "c" };
        // A line moved here (its parent was deleted elsewhere) is reviewed, not compared.
        if l.conflict_with.as_deref() == Some(MOVED_HERE) {
            return format!("≠ moved here · {key} review");
        }
        return format!("≠ {} · {key} compare", l.conflict_with.as_deref().unwrap_or("changed elsewhere"));
    }
    // The meta is the saved note's; a done time it shows goes as soon as the line isn't done
    // (⌃T back to text, a reopen), not when the save lands.
    if l.status.as_deref() != Some("done") && l.meta.starts_with("done") {
        return l.meta.split_once(" · ").map_or(String::new(), |(_, rest)| rest.to_string());
    }
    l.meta.clone()
}

/// The parse chip: what the line's tokens will set (`due fri · !high ✓`), or what can't be read.
pub fn chip(text: &str, today: chrono::NaiveDate) -> Option<String> {
    if !text.split_whitespace().any(|w| w.contains(':') || w.starts_with('!')) {
        return None;
    }
    let (cap, bad) = thc_core::capture::parse_lenient_text(text, today).ok()?;
    if let Some(b) = bad.first() {
        let v = b.split_once(':').map(|x| x.1).unwrap_or(b).trim_matches('"');
        return Some(format!("can't read \"{v}\""));
    }
    let mut parts = Vec::new();
    if let Some(s) = &cap.scheduled {
        parts.push(s.fmt());
    }
    if let Some(d) = &cap.due {
        let date = d.date();
        let n = (date - today).num_days();
        parts.push(format!("due {}", match n {
            0 => "today".into(),
            1 => "tomorrow".into(),
            2..=6 => date.format("%a").to_string().to_lowercase(),
            _ => date.format("%b %d").to_string().to_lowercase(),
        }));
    }
    if let Some(p) = &cap.priority {
        parts.push(format!("!{p}"));
    }
    if let Some(r) = &cap.repeat {
        parts.push(format!("↻ {}", r.text.trim_start_matches("every ")));
    }
    // Two tokens for one field (writing.md §1): the last one is used; say so.
    let dupes: Vec<String> = thc_core::capture::duplicates(text, today)
        .into_iter()
        .map(|(field, toks)| format!("{} {field} · using {}", toks.len(), toks.last().cloned().unwrap_or_default()))
        .collect();
    (!parts.is_empty()).then(|| {
        let mut s = format!("{} ✓", parts.join(" · "));
        for d in dupes {
            s.push_str(&format!(" · {d}"));
        }
        s
    })
}

/// The journal header (date line, day strip, rule) or the page title (title, rule).
fn header(ctx: DocContext, days: &mut Vec<(u16, u16, u16, chrono::NaiveDate)>, app: &App, w: usize) -> Vec<TLine<'static>> {
    let th = app.theme;
    let left = " ".repeat(left_edge(ctx, w) + MARKS + HANG);
    let fv = focus(ctx);
    let rule_w = match fv {
        Some(f) => f.width + if f.has(El::Meta) { 2 + META } else { 0 },
        None => col_beside(ctx, w) + 2 + META,
    }
    .min(w.saturating_sub(left.len()));
    // In Focus, each part is an element: the date (or title) and rule, the strip. The month
    // replaces the strip where it fits; where it doesn't, the strip stands in for it.
    let show_title = fv.is_none_or(|f| f.has(El::Header));
    let show_strip = fv.is_none_or(|f| !f.month_fits(w) && (f.has(El::Strip) || f.has(El::Month)));
    if !show_title && !show_strip {
        return vec![TLine::raw("")];
    }
    let mut out = Vec::new();
    match &app.doc.as_ref().unwrap().target {
        Target::Journal { date } => {
            let mut l = vec![Span::raw(left.clone()), Span::styled(date.format("%a %d %b %Y").to_string().to_uppercase(), th.strong())];
            if *date == app.today {
                l.push(Span::styled(" · TODAY", th.s(Token::Today).add_modifier(Modifier::BOLD)));
            } else if *date == app.today - chrono::Duration::days(1) {
                l.push(Span::styled(" · YESTERDAY", th.s(Token::Muted)));
            }
            if show_title {
                out.push(TLine::from(l));
            }
            let mut strip = vec![Span::raw(left.clone())];
            let start = *date - chrono::Duration::days(3);
            let row = out.len() as u16;
            for k in 0..7 {
                let d = start + chrono::Duration::days(k);
                let label = d.format("%a %d").to_string().to_uppercase();
                // Each day is clickable (mouse.md §3): recorded relative, placed by draw.
                let x0 = strip.iter().map(|s| width(&s.content)).sum::<usize>() as u16;
                days.push((x0, x0 + width(&label) as u16 + 2, row, d));
                let has = app.derived.data.document.populated_days.contains(&d);
                if d == *date {
                    strip.push(Span::styled(format!("[{label}"), th.s(Token::Text)));
                    if has {
                        strip.push(Span::styled(" •", th.s(Token::Accent)));
                    }
                    strip.push(Span::styled("]   ", th.s(Token::Text)));
                } else {
                    strip.push(Span::styled(label, th.s(Token::Muted)));
                    strip.push(Span::styled(if has { " •   " } else { "   " }, th.s(Token::Muted)));
                }
            }
            if show_strip {
                out.push(TLine::from(strip));
            }
        }
        Target::Page { title, .. } if show_title => {
            // An issue (issues.md §1): a task opened as a document. Its header is the task line,
            // with its cell, and the meta: owner, priority, when it was opened.
            let chrome = &app.derived.data.document;
            match chrome.issue_status.as_deref() {
                Some(status) => {
                    let cell = thc_core::outline::checkbox(Some(status));
                    let done = matches!(status, "done" | "cancelled");
                    let title_style = if done { th.s(Token::Muted) } else { th.strong() };
                    let meta = &chrome.issue_meta;
                    let mut l = vec![Span::raw(left.clone()), Span::styled(cell.to_string(), th.s(Token::Muted)), Span::styled(title.clone(), title_style)];
                    if !meta.is_empty() {
                        l.push(Span::styled(format!("   {}", meta.join(" · ")), th.s(Token::Muted)));
                    }
                    out.push(TLine::from(l));
                }
                None => out.push(TLine::from(vec![Span::raw(left.clone()), Span::styled(title.clone(), th.strong())])),
            }
        }
        Target::Page { .. } => return vec![TLine::raw("")],
    }
    if show_title {
        out.push(TLine::from(vec![Span::raw(left), Span::styled("─".repeat(rule_w), th.s(Token::Line))]));
    }
    out.push(TLine::raw(""));
    out
}

/// The month beside the text (tui-editor.md §8.3): days with entries in text, the rest dim,
/// the shown day reversed in accent, today bold.
fn month(app: &App, date: chrono::NaiveDate) -> Vec<TLine<'static>> {
    use chrono::Datelike;
    let th = app.theme;
    let first = date.with_day(1).unwrap();
    let days = {
        let next = if first.month() == 12 { chrono::NaiveDate::from_ymd_opt(first.year() + 1, 1, 1) } else { chrono::NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1) };
        (next.unwrap() - first).num_days() as u32
    };
    let has = |d: chrono::NaiveDate| app.derived.data.document.populated_days.contains(&d);
    let mut out = vec![
        TLine::styled(first.format("%B %Y").to_string().to_uppercase(), th.s(Token::Muted)),
        TLine::styled("Mo Tu We Th Fr Sa Su", th.s(Token::Muted).add_modifier(Modifier::DIM)),
    ];
    let mut row: Vec<Span<'static>> = vec![Span::raw("   ".repeat(first.weekday().num_days_from_monday() as usize))];
    for n in 1..=days {
        let d = first.with_day(n).unwrap();
        let mut st = if has(d) { th.s(Token::Text) } else { th.s(Token::Muted).add_modifier(Modifier::DIM) };
        if d == app.today {
            st = st.add_modifier(Modifier::BOLD);
        }
        if d == date {
            st = st.add_modifier(Modifier::REVERSED).fg(th.s(Token::Accent).fg.unwrap_or_default());
        }
        row.push(Span::styled(format!("{n:>2}"), st));
        if d.weekday() == chrono::Weekday::Sun || n == days {
            out.push(TLine::from(std::mem::take(&mut row)));
        } else {
            row.push(Span::raw(" "));
        }
    }
    out
}

/// The document's words (its own lines), for the `wordcount` element.
pub fn word_count(app: &App) -> usize {
    app.doc.as_ref().map_or(0, |d| d.word_count())
}

/// One visible row of the document on screen, as drawn: which line and bytes it shows, where its
/// text starts, where its hang is. Clicks go through the engine's hit-testing ([`hit`]); these
/// say where thc's own marks (`≠`) are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HitRow {
    pub y: u16,
    pub line: usize,
    /// The bytes drawn on this row (a first row starts after a marker drawn in the hang).
    pub start: usize,
    pub end: usize,
    pub first: bool,
    /// The column where the row's text starts, and where the hang starts.
    pub text_x: u16,
    pub hang_x: u16,
}

/// Layout inputs shared by motion and drawing. No frame-local global state.
#[derive(Clone, Copy)]
pub(crate) struct DocContext {
    focus: Option<FocusView>,
    writing: bool,
    rail: usize,
    /// With the sidebar beside it, the text column the document has without it: the sidebar
    /// only takes room, it never gives the text more (mgmm8: at 120 columns the rail gave way
    /// and the column widened 66 → 72, so every paragraph reflowed when a panel opened).
    cap: Option<usize>,
}

impl DocContext {
    pub fn from_app(app: &App) -> Self {
        if app.in_panel.is_some() {
            return Self { focus: None, writing: app.main.write, rail: 0, cap: None };
        }
        let journal = app.doc.as_ref().is_some_and(|d| matches!(d.target, Target::Journal { .. }));
        Self {
            focus: app.focus_mode.then_some(FocusView { set: app.focus_cfg.set, width: app.focus_cfg.width() as usize, journal }),
            writing: app.main.write,
            rail: if app.rail_shows() { RAIL_W } else { 0 },
            cap: (app.sidebar_col.is_some() && !app.focus_mode).then(|| {
                // The terminal's width, the rail as it shows there, the detail pane as it would.
                let rail = if app.term_width >= 120 && app.tui_prefs.page_rail && app.doc.is_some() { RAIL_W } else { 0 };
                let w = (pane_width(app.term_width, app.show_detail) as usize).saturating_sub(rail);
                let ctx = DocContext { focus: None, writing: app.main.write, rail, cap: None };
                col_beside(ctx, w).min(w.saturating_sub(MARKS + HANG + 2))
            }),
        }
    }
}

/// A content fingerprint also catches in-place edits (including folds and same-size text
/// replacements), not just saves: the document's revision, caret and view. It reads no clock,
/// store, or interior cache, and costs the same on a page of any length.
fn source_revision(app: &App) -> u64 {
    use std::hash::{BuildHasher, Hash, Hasher};
    // A fast fixed-seed hash: this runs over every line twice a frame.
    let mut h = foldhash::fast::FixedState::with_seed(0).build_hasher();
    app.vault.paths.vault.hash(&mut h);
    app.screen_width.hash(&mut h);
    app.term_width.hash(&mut h);
    app.sidebar_col.is_some().hash(&mut h);
    app.detail_shows().hash(&mut h);
    app.focus_mode.hash(&mut h);
    format!("{:?}", app.focus_cfg).hash(&mut h);
    app.main.write.hash(&mut h);
    app.rail_shows().hash(&mut h);
    app.crumb_shows().hash(&mut h);
    app.today.hash(&mut h);
    if let Some(d) = &app.doc {
        // The document's revision moves with every change to its lines (text, shape, meta,
        // save state, from the engine or the host): no walk over the lines.
        format!("{:?}", d.target).hash(&mut h);
        d.root.hash(&mut h);
        d.revision().hash(&mut h);
        d.caret().hash(&mut h);
        d.view_stamp().hash(&mut h);
    }
    h.finish()
}

/// A prepared layout. Cache filling never changes presentation scroll.
pub struct PreparedDoc {
    area: Rect,
    revision: u64,
    doc_revision: u64,
    /// The rows on screen.
    rows: Vec<Row>,
    /// The caret's cell in the view (from the marks column, the first row on screen).
    cursor: Option<(u16, u16)>,
    /// Lines whose meta has its own row.
    own_meta: std::collections::HashSet<usize>,
    /// All the document's rows, and the first on screen (the scrollbar).
    scroll: (usize, usize),
    header: Vec<TLine<'static>>,
    days: Vec<(u16, u16, u16, chrono::NaiveDate)>,
    body: Rect,
    crumb: Option<CrumbAt>,
    ctx: DocContext,
    popup_area: Rect,
    caret: crate::editor::BlockPos,
    selection: Option<(crate::editor::BlockPos, crate::editor::BlockPos)>,
    folds: std::collections::HashSet<String>,
    chip: Option<String>,
    link: Option<(String, Vec<(String, String)>, Option<String>)>,
    /// Shared engine-frame seams consumed by the teaching overlay, not another layout.
    pub frame: caretline::Frame,
    pub frame_at: (u16, u16),
}

pub(crate) fn prepare(app: &mut App, area: Rect) {
    let ctx = DocContext::from_app(app);
    let w = area.width as usize;
    let mut days = Vec::new();
    let mut header = if app.in_panel.is_some() { Vec::new() } else { header(ctx, &mut days, app, w) };
    // Without the rail, the crumb leads the title's own row (navigation.md §3), so the text
    // starts on the same row with or without it: opening the sidebar, which hides the rail,
    // moves nothing down (interaction.md §2).
    let crumb = if app.in_panel.is_none() && app.crumb_shows() { crumb(app, ctx, w) } else { None };
    let crumb = match crumb {
        Some(c) if header.first().is_some_and(|l| !l.spans.is_empty()) && title_shows(ctx) => {
            let first = &mut header[0];
            let at = usize::from(first.spans.first().is_some_and(|s| s.content.trim().is_empty()));
            for (i, s) in c.spans.iter().cloned().enumerate() {
                first.spans.insert(at + i, s);
            }
            Some(CrumbAt { row: 0, x0: c.list_x, x1: c.list_x + c.list_w, action: c.action })
        }
        Some(c) => {
            // No title row to share (Focus without the header): the crumb has its own row.
            let indent = " ".repeat(left_edge(ctx, w) + MARKS + HANG);
            let mut spans = vec![Span::raw(indent)];
            spans.extend(c.spans);
            header.insert(0, TLine::from(spans));
            for d in days.iter_mut() {
                d.2 += 1;
            }
            Some(CrumbAt { row: 0, x0: c.list_x, x1: c.list_x + c.list_w, action: c.action })
        }
        None => None,
    };
    let head_h = (header.len() as u16).min(area.height);
    let body = Rect { y: area.y + head_h, height: area.height.saturating_sub(head_h), ..area };
    let (mut rows, mut cursor, mut own_meta, mut scroll, mut frame, mut left) = layout(app, w, body.height);
    // The caret line stays on its screen row when the geometry changes under it (the sidebar
    // opens or closes, the rail comes or goes, the terminal resizes): the view scrolls by what
    // the reflow moved it (interaction.md §2.2).
    let here = pin_key(app, body, w);
    let repin = app.doc.as_mut().is_some_and(|d| std::mem::take(&mut d.repin));
    if let (Some(pin), Some((_, row))) = (app.caret_pin.clone().filter(|_| pane_focused(app)), cursor) {
        let d = app.doc.as_ref().unwrap();
        // (Or a save took empty lines out above the caret: h8vsn.)
        let same_place = pin.doc == here.doc && (pin.caret == d.caret() && pin.geometry != here.geometry || repin);
        let follows = !d.scroll_free() && !ctx.focus.map_or(app.tui_prefs.typewriter, |f| f.has(El::Typewriter));
        let now_y = body.y + row;
        if same_place && follows && now_y != pin.y && pin.y >= body.y && pin.y < body.bottom() {
            let top = d.scroll() as isize + now_y as isize - pin.y as isize;
            app.doc.as_mut().unwrap().set_scroll(top.max(0) as usize, false);
            (rows, cursor, own_meta, scroll, frame, left) = layout(app, w, body.height);
        }
    }
    if pane_focused(app) { app.caret_pin = cursor.map(|(_, row)| CaretPin { y: body.y + row, ..here }); }
    let d = app.doc.as_ref().unwrap();
    let caret = d.caret();
    let selection = d.selection();
    let folds = rows.iter().filter_map(|r| d.blocks().get(r.line)).filter(|l| d.is_folded(&l.id)).map(|l| l.id.clone()).collect();
    let chip = if app.in_panel.is_some() { chip(&d.caret_block().text, app.today) } else { app.derived.caret_chip.clone() };
    let link = app.main.link_open.then(|| app.link_query()).flatten().and_then(|(_, q)| {
        app.derived.data.overlay.link.as_ref().filter(|(query, _, _)| *query == q).cloned()
    });
    app.derived.doc = Some(PreparedDoc { area, revision: source_revision(app), doc_revision: d.revision(), rows, cursor, own_meta, scroll, header, days, body, crumb, ctx, popup_area: area, caret, selection, folds, chip, link, frame, frame_at: (body.x + left as u16, body.y) });
}

/// Where the main view's caret was drawn, for the next layout (`prepare`): its document and
/// caret, the geometry it was laid out in, and its screen row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaretPin {
    doc: String,
    caret: crate::editor::BlockPos,
    geometry: (Rect, usize, usize),
    y: u16,
}

#[cfg(test)]
impl CaretPin {
    /// The screen row the caret was drawn on.
    pub fn row(&self) -> u16 {
        self.y
    }
}

fn pin_key(app: &App, body: Rect, w: usize) -> CaretPin {
    let ctx = DocContext::from_app(app);
    let d = app.doc.as_ref().unwrap();
    CaretPin {
        doc: format!("{:?}{:?}", d.target, d.root),
        caret: d.caret(),
        geometry: (body, text_width(ctx, app.screen_width, app.detail_shows(), 0), left_edge(ctx, w)),
        y: 0,
    }
}

/// The header's title row shows (outside Focus, or Focus with the header element).
fn title_shows(ctx: DocContext) -> bool {
    focus(ctx).is_none_or(|f| f.has(El::Header))
}

/// The crumb's clickable first part, on a header row.
#[derive(Clone, Copy)]
struct CrumbAt {
    row: u16,
    x0: u16,
    x1: u16,
    action: &'static str,
}

struct Crumb {
    spans: Vec<Span<'static>>,
    /// The list's name: where it starts after the indent, and how wide it is.
    list_x: u16,
    list_w: u16,
    action: &'static str,
}

/// The crumb, when the rail doesn't show: `¶ Pages › ` (or the page an issue lives in, or
/// `§ Journal › `), its first part a click to the list. Not home: the vault leads, in its accent
/// (vaults.md §8).
fn crumb(app: &App, _ctx: DocContext, _w: usize) -> Option<Crumb> {
    let th = app.theme;
    let g = th.glyphs();
    let (sym, list, action) = match &app.doc.as_ref()?.target {
        // A node inside a page (an issue): the page it lives in, not the Pages list.
        Target::Page { .. } => match &app.derived.data.document.parent_label {
            Some(parent) => (g.page, parent.clone(), "doc.done"),
            None => (g.page, "Pages".to_string(), "go.pages"),
        },
        Target::Journal { .. } => (g.journal, "Journal".to_string(), "go.journal"),
    };
    let vault = if app.vault_home { String::new() } else { format!("{} › ", app.vault_name) };
    let lead = format!("{vault}{sym} ");
    Some(Crumb {
        spans: vec![
            Span::styled(vault, th.s(Token::Accent)),
            Span::styled(format!("{sym} "), th.s(Token::Muted)),
            Span::styled(list.clone(), th.s(Token::Muted).add_modifier(Modifier::UNDERLINED)),
            Span::styled(" › ", th.s(Token::Muted)),
        ],
        list_x: width(&lead) as u16,
        list_w: width(&list) as u16,
        action,
    })
}

fn meta_targets(render: &mut RenderOutput, mut x: u16, y: u16, line: usize, meta: &str) {
    let target = |field| crate::ui::Click::Meta { line, field };
    if meta.starts_with("↗") || meta.starts_with("new page") {
        crate::ui::target(render, x, x + width(meta) as u16, y, target("open"));
    } else if meta.starts_with('≠') {
        crate::ui::target(render, x, x + width(meta) as u16, y, target("conflict"));
    } else {
        for segment in meta.split(" · ") {
            let w = width(segment) as u16;
            let field = if segment.starts_with("due") { Some("due") } else if segment.starts_with(['!', '↻', '+']) || segment.starts_with("done") { None } else { Some("sched") };
            if let Some(field) = field { crate::ui::target(render, x, x + w, y, target(field)); }
            x += w + 3;
        }
    }
}

struct DocRead<'a> {
    doc: &'a crate::editor::Doc,
    prepared: &'a PreparedDoc,
}
impl std::ops::Deref for DocRead<'_> {
    type Target = crate::editor::Doc;
    fn deref(&self) -> &Self::Target { self.doc }
}
impl DocRead<'_> {
    fn caret(&self) -> crate::editor::BlockPos { self.prepared.caret }
    fn selection(&self) -> Option<(crate::editor::BlockPos, crate::editor::BlockPos)> { self.prepared.selection }
    fn is_folded(&self, id: &str) -> bool { self.prepared.folds.contains(id) }
}
struct PaneRead<'a> {
    host: &'a App,
    main: &'a super::EditorState,
    doc: Option<DocRead<'a>>,
    screen_width: u16,
    focus_mode: bool,
    doc_footer: Option<&'a (String, Vec<crate::doc_app::FooterRow>)>,
    panel: bool,
}
impl std::ops::Deref for PaneRead<'_> {
    type Target = App;
    fn deref(&self) -> &Self::Target { self.host }
}
impl PaneRead<'_> {
    fn detail_shows(&self) -> bool { !self.panel && self.host.detail_shows() }
}

/// Measure through the very same layout, restoring all view geometry and scroll afterwards.
/// It fills row caches but cannot move any pane's caret or consume its remembered scroll.
pub(crate) fn natural_rows(app: &mut App, width: u16, cap: usize) -> usize {
    let Some(d) = app.doc.as_ref() else { return 1 };
    let view = d.view_snapshot();
    let pending = app.doc_pending_scroll.take();
    layout(app, width as usize, cap.min(u16::MAX as usize) as u16);
    let d = app.doc.as_mut().unwrap();
    let rows = d.rows_capped(cap);
    d.restore_view_snapshot(view);
    app.doc_pending_scroll = pending;
    rows
}

impl PreparedDoc {
    pub fn current(&self, app: &App) -> bool { self.revision == source_revision(app) }

    /// Visible line anchors from this exact shared layout, in screen coordinates.
    pub(crate) fn line_rows(&self, doc: &crate::editor::Doc) -> Vec<(u16, u16, u16, String)> {
        self.rows.iter().enumerate().filter_map(|(dy, row)| {
            if row.blank || row.meta_row || row.image.is_some() { return None; }
            let line = doc.blocks().get(row.line)?;
            let tx = (self.frame_at.0 as usize + MARKS + line.depth * 4 + HANG).min(u16::MAX as usize) as u16;
            let x0 = if row.first { tx.saturating_sub(HANG as u16) } else { tx };
            let start = if row.first { row.shown.max(marker_len(line)) } else { row.shown }.min(row.end);
            Some((self.body.y + dy as u16, x0, tx - x0 + (width(&line.text[start..row.end]) as u16).max(1), line.id.clone()))
        }).collect()
    }

    pub(crate) fn popup_bounds(&mut self, area: Rect) { self.popup_area = area; }
    pub(crate) fn view_rect(&self) -> Rect {
        let left = left_edge(self.ctx, self.area.width as usize) as u16;
        Rect { x: self.body.x + left, width: self.body.width.saturating_sub(left), ..self.body }
    }
    pub(crate) fn cursor_cell(&self) -> Option<(u16, u16)> {
        let r = self.view_rect();
        self.cursor.filter(|(_, y)| *y < r.height && r.width > 0).map(|(x, y)| (r.x + x.min(r.width - 1), r.y + y))
    }
}

pub fn draw(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect) {
    let Some(prepared) = app.derived.doc.as_ref() else { return };
    if prepared.area != area || prepared.revision != source_revision(app) { return }
    let Some(doc) = app.doc.as_ref() else { return };
    draw_pane(render, f, app, prepared, doc, &app.main, app.ui.focus != crate::app::Focus::Sidebar, false);
}

/// One body renderer, used by main and panels. Its caret, selection and folds are the
/// prepared view's, not whichever view currently owns the shared document borrow.
pub(crate) fn draw_pane(render: &mut RenderOutput, f: &mut Frame, host: &App, prepared: &PreparedDoc, doc: &crate::editor::Doc, editor: &super::EditorState, focused: bool, panel: bool) {
    if prepared.doc_revision != doc.revision() { return; }
    let app = PaneRead { host, main: editor, doc: Some(DocRead { doc, prepared }), screen_width: if panel { prepared.area.width } else { host.screen_width }, focus_mode: !panel && host.focus_mode, doc_footer: if panel { None } else { host.doc_footer.as_ref() }, panel };
    let app = &app;
    let area = prepared.area;
    let ctx = prepared.ctx;
    let fv = focus(ctx);
    let w = area.width as usize;
    if let Some(c) = prepared.crumb {
        let x0 = area.x + (left_edge(ctx, w) + MARKS + HANG) as u16;
        crate::ui::target(render, x0 + c.x0, x0 + c.x1, area.y + c.row, crate::ui::Click::Action(c.action));
    }
    let head_h = (prepared.header.len() as u16).min(area.height);
    for &(x0, x1, row, date) in &prepared.days {
        if row < head_h {
            crate::ui::target(render, area.x + x0, area.x + x1, area.y + row, crate::ui::Click::Day(date));
        }
    }
    f.render_widget(Paragraph::new(prepared.header.clone()), Rect { height: head_h, ..area });
    let body = prepared.body;
    let rows = &prepared.rows;
    let th = app.theme;
    let h = body.height as usize;
    let d = app.doc.as_ref().unwrap();
    let sel = d.selection();
    let left = left_edge(ctx, w);
    // The last column is the scrollbar's, kept whether or not it shows, so a page growing
    // long enough for one moves nothing and the meta is never drawn under it.
    let meta_right = fv.map_or(left + if rail(ctx) > 0 { block_w(ctx, w) } else { BLOCK }, |f| left + f.block()).min(w.saturating_sub(1));
    let mut lines: Vec<TLine<'static>> = Vec::new();
    let mut hits: Vec<HitRow> = Vec::new();
    let mut places: Vec<crate::images::Place> = Vec::new();
    for r in rows.iter().take(h) {
        if let Some((k, n, cols)) = r.image {
            // The image goes over its reserved rows, when they're all on screen.
            if k == 0 && lines.len() + n as usize <= h {
                if let Some((_, path)) = image_line(&d.blocks()[r.line].text) {
                    let indent = d.blocks()[r.line].depth * 4;
                    places.push(crate::images::Place {
                        x: body.x + (left + MARKS + indent + HANG) as u16,
                        y: body.y + lines.len() as u16,
                        cols,
                        rows: n,
                        path: app.vault.paths.vault.join(&path),
                    });
                }
            }
            lines.push(TLine::raw(""));
            continue;
        }
        if r.blank {
            lines.push(TLine::raw(""));
            continue;
        }
        let l = &d.blocks()[r.line];
        let fm = form(app, l);
        let indent = l.depth * 4;
        let mut spans: Vec<Span<'static>> = vec![Span::raw(" ".repeat(left))];
        let in_vsel = app.main.vsel.is_some_and(|v| r.line >= v.min(d.caret().line) && r.line <= v.max(d.caret().line));
        let navigate_here = (!app.main.write && r.line == d.caret().line && app.main.footer_cur.is_none()) || in_vsel;
        let row_fill = if navigate_here { th.fill(Token::Selection) } else { Style::default() };
        if r.meta_row {
            let meta = meta_of(ctx, l, (r.line == d.caret().line).then_some(prepared.chip.as_deref()).flatten());
            let pad = meta_right.saturating_sub(left + width(&meta));
            spans.push(Span::styled(" ".repeat(pad), row_fill));
            meta_targets(render, body.x + (left + pad) as u16, body.y + lines.len() as u16, r.line, &meta);
            spans.push(Span::styled(meta, th.s(Token::Muted).add_modifier(Modifier::DIM)));
            lines.push(TLine::from(spans));
            continue;
        }
        // Marks.
        let mark = if l.conflict {
            Span::styled("≠ ", th.s(Token::Conflict))
        } else if l.remote_text.is_some() {
            Span::styled("◆ ", th.s(Token::Agent))
        } else if l.save_error.is_some() {
            Span::styled("◌ ", th.s(Token::Muted))
        } else if r.first && d.is_folded(&l.id) {
            Span::styled("▸ ", th.s(Token::Muted))
        } else {
            Span::raw("  ")
        };
        spans.push(if r.first && !app.focus_mode { mark } else { Span::raw("  ") });
        spans.push(Span::styled(" ".repeat(indent), row_fill));
        spans.push(if r.first { Span::styled(format!("{:>4}", fm.hang), fm.hang_style.patch(row_fill)) } else { Span::styled("    ", row_fill) });
        // Text, with the selection filled (a first row's marker is in the hang).
        let start = if r.first { r.start.max(marker_len(l)).min(r.end) } else { r.start };
        let r = &Row { start, ..*r };
        // A code block: cut to the column, from where the engine scrolled it sideways to keep
        // the caret in view, `→` where it runs on.
        let tw_here = text_width(ctx, app.screen_width, app.detail_shows(), l.depth);
        let (r, more) = if is_code(l) {
            let a = r.shown.clamp(r.start, r.end);
            let (mut b, mut cw) = (a, 0);
            while b < r.end {
                let g = crate::text::next_char(&l.text[..r.end], b);
                let w1 = width(&l.text[b..g]);
                if cw + w1 > tw_here.saturating_sub(1) {
                    break;
                }
                cw += w1;
                b = g;
            }
            (Row { start: a, end: b, ..*r }, b < r.end)
        } else {
            (*r, false)
        };
        let r = &r;
        {
            let y = body.y + lines.len() as u16;
            let text_x = body.x + (left + MARKS + indent + HANG) as u16;
            if r.first && l.kind() == Kind::Task {
                crate::ui::target(render, body.x + (left + MARKS + indent) as u16, body.x + (left + MARKS + indent) as u16 + 3, y, crate::ui::Click::Box);
            }
            let vis = &l.text[r.start..r.end];
            let quoted = thc_core::capture::quoted_ranges(&l.text);
            let mut from = 0;
            while let Some(a) = vis[from..].find("[[").map(|k| from + k) {
                let Some(b) = vis[a..].find("]]").map(|k| a + k + 2) else { break };
                // A quoted `[[…]]` is text, not a link (writing.md §1).
                if quoted.iter().any(|(q0, q1)| r.start + a > *q0 && r.start + a < *q1) {
                    from = b;
                    continue;
                }
                let x0 = text_x + width(&vis[..a]) as u16;
                crate::ui::target(render, x0, x0 + width(&vis[a..b]) as u16, y, crate::ui::Click::Link);
                from = b;
            }
        }
        hits.push(HitRow {
            y: body.y + lines.len() as u16,
            line: r.line,
            start: r.start,
            end: r.end,
            first: r.first,
            text_x: body.x + (left + MARKS + indent + HANG) as u16,
            hang_x: body.x + (left + MARKS + indent) as u16,
        });
        let text = &l.text[r.start..r.end];
        let (sa, sb) = match sel {
            Some((s, e)) if r.line >= s.line && r.line <= e.line => {
                let a = if r.line == s.line { s.byte.clamp(r.start, r.end) } else { r.start };
                let b = if r.line == e.line { e.byte.clamp(r.start, r.end) } else { r.end };
                (a - r.start, b - r.start)
            }
            _ => (0, 0),
        };
        let code = l.kind() == Kind::Para && l.text.starts_with("```");
        let mut base = if code { fm.text_style.patch(th.fill(Token::Raised)) } else { fm.text_style.patch(row_fill) };
        // Focus dimming (the `dim` element): every line but the caret's in dim.
        if fv.is_some_and(|f| f.has(El::Dim)) && r.line != d.caret().line {
            base = base.add_modifier(Modifier::DIM);
        }
        // While the caret's line is written, every token is underlined in its hue (§5).
        let typing = app.main.write && r.line == d.caret().line;
        let token_style = |word: &str| -> Option<Style> {
            if !typing {
                return None;
            }
            let lower = word.to_lowercase();
            let hue = if word.starts_with('#') && word.len() > 1 && !word[1..].starts_with('#') {
                Token::Tag
            } else if ["!high", "!med", "!medium", "!low", "!h", "!m", "!l"].contains(&lower.as_str()) {
                Token::Text
            } else if let Some((k, v)) = lower.split_once(':') {
                if v.is_empty() || !["due", "deadline", "sched", "scheduled", "start", "at", "on", "every", "every!", "repeat"].contains(&k) {
                    return None;
                }
                if !app.derived.data.input.valid_token(&l.text, word) { Token::Overdue } else if k.starts_with("every") || k == "repeat" { Token::Muted } else { Token::Today }
            } else {
                return None;
            };
            let st = th.s(hue).patch(row_fill).add_modifier(Modifier::UNDERLINED).underline_color(th.s(hue).fg.unwrap_or_default());
            Some(if lower.starts_with('!') { st.add_modifier(Modifier::BOLD) } else { st })
        };
        // Quoted text is never parsed (writing.md §1): words inside quotes keep the line's style.
        let quoted = thc_core::capture::quoted_ranges(&l.text);
        // A token a later one of the same field overrides is a plain word (writing.md §1).
        let shadowed = if typing { app.derived.data.input.shadowed(&l.text) } else { vec![] };
        let line_start = l.text.as_ptr() as usize;
        let tagged = |s: &str| -> Vec<Span<'static>> {
            // #tags in tag colour, the rest in the line's style.
            let mut out = Vec::new();
            let mut at = (s.as_ptr() as usize).saturating_sub(line_start);
            for (k, word) in s.split(' ').enumerate() {
                if k > 0 {
                    out.push(Span::styled(" ", base));
                    at += 1;
                }
                let inside = quoted.iter().any(|(a, b)| at > *a && at < *b) || word.starts_with(['"', '`']) && !word.contains(':') || shadowed.iter().any(|(a, _)| *a == at);
                at += word.len();
                let st = if inside {
                    base
                } else {
                    token_style(word).unwrap_or(if word.starts_with('#') && word.len() > 1 && !word[1..].starts_with('#') { th.s(Token::Tag).patch(row_fill) } else { base })
                };
                out.push(Span::styled(word.to_string(), st));
            }
            out
        };
        let image = image_line(&l.text).filter(|_| !(app.main.write && r.line == d.caret().line));
        if let Some((caption, path)) = image {
            // An attachment's line (attachments.md §3): a chip, `▣ caption · 1280×720 · ⌃O open`.
            // The caret's line shows the Markdown, which is what's edited.
            if r.first {
                let attachment = app.derived.attachment(&app.vault.paths.vault, &path);
                let dims = attachment.map(|a| a.label.as_str()).unwrap_or("");
                let missing = attachment.is_none_or(|a| a.missing);
                spans.push(Span::styled("▣ ", th.s(Token::Muted)));
                let shown = if caption.trim().is_empty() { path.rsplit('/').next().unwrap_or(&path).to_string() } else { caption.clone() };
                spans.push(Span::styled(shown, base));
                let rest = if missing { " · missing".to_string() } else { format!("{dims} · ⌃O open") };
                spans.push(Span::styled(rest, th.s(Token::Muted)));
            }
        } else if l.kind() == Kind::Para && (l.text == "---" || l.text == "***") && !(app.main.write && r.line == d.caret().line) {
            // A rule: a line across the text column (the text is still `---`).
            let tw = text_width(ctx, app.screen_width, app.detail_shows(), l.depth);
            spans.push(Span::styled("─".repeat(tw), th.s(Token::Line)));
        } else if sa < sb {
            spans.extend(tagged(&text[..sa]));
            spans.push(Span::styled(text[sa..sb].to_string(), base.patch(th.fill(Token::Selection))));
            spans.extend(tagged(&text[sb..]));
        } else {
            spans.extend(tagged(text));
        }
        if more {
            spans.push(Span::styled("→", th.s(Token::Muted).add_modifier(Modifier::DIM)));
        }
        // Meta on the first row, right-aligned.
        if r.first {
            let mut meta = meta_of(ctx, l, (r.line == d.caret().line).then_some(prepared.chip.as_deref()).flatten());
            // While the caret is in a link: the ↗ open chip (mouse.md §3).
            if app.main.write && r.line == d.caret().line && crate::doc_app::link_at(&l.text, d.caret().byte).is_some() {
                meta = "↗ open".to_string();
            }
            // On an attachment's line: ⌃O opens it.
            if app.main.write && r.line == d.caret().line && image_line(&l.text).is_some() {
                meta = "⌃O open".to_string();
            }
            // The near-miss chip (writing.md §5), for 3 s after the save.
            if let Some((_, typed, existing, _, since)) = app.main.near_miss.as_ref().filter(|n| n.0 == l.id && app.ui.age(n.4).as_secs() < 3) {
                let _ = since;
                meta = format!("new page \"{typed}\" · ⌃O {existing}?");
            }
            if fv.is_none() && d.is_folded(&l.id) {
                let n = d.descendants(r.line);
                meta = if meta.is_empty() { format!("+{n}") } else { format!("{meta} · +{n}") };
            }
            let own_row = prepared.own_meta.contains(&r.line);
            if !meta.is_empty() && !own_row {
                let used = left + MARKS + indent + HANG + width(text);
                let pad = meta_right.saturating_sub(used + width(&meta)).max(2);
                spans.push(Span::styled(" ".repeat(pad), row_fill));
                // The chips are buttons (mouse.md §3): ↗ open, the near miss and ≠ act as one;
                // a date opens its editor.
                let y = body.y + lines.len() as u16;
                meta_targets(render, body.x + (used + pad) as u16, y, r.line, &meta);
                let flashing = l.flash_until.is_some_and(|t| t > app.ui.now_ms);
                let st = if l.conflict {
                    th.s(Token::Conflict)
                } else if flashing {
                    th.s(Token::Accent)
                } else if meta.starts_with("can't read") {
                    th.s(Token::Overdue)
                } else {
                    th.s(Token::Muted)
                };
                spans.push(Span::styled(meta, st));
            } else if navigate_here {
                let used = left + MARKS + indent + HANG + width(text);
                spans.push(Span::styled(" ".repeat(meta_right.saturating_sub(used)), row_fill));
            }
        }
        let line = TLine::from(spans);
        let live = !th.is_ansi() && app.ui.flashes.get(&l.id).is_some_and(|(at, _)| app.ui.now_ms.saturating_sub(*at) < 3000);
        lines.push(if live { line.patch_style(th.fill(Token::AgentTint)) } else { line });
    }
    // The footer after the document's own lines (read-only; the caret never goes there).
    let shown_rows = rows.len();
    // In Focus, `also today` and `linked from` are elements.
    let footer_on = |title: &str| match fv {
        None => true,
        Some(f) => if title.starts_with("also today") { f.has(El::AlsoToday) } else { f.has(El::LinkedFrom) },
    };
    if let Some((title, items)) = app.doc_footer.as_ref().filter(|(t, _)| footer_on(t)) {
        if shown_rows + 2 < h {
            lines.push(TLine::raw(""));
            let span_w = fv.map_or(col_beside(ctx, w) + 2 + META, |f| f.width + if f.has(El::Meta) { 2 + META } else { 0 }).min(w.saturating_sub(left + MARKS + HANG));
            let label = format!(" {title} ");
            let side = span_w.saturating_sub(width(&label)) / 2;
            lines.push(TLine::from(vec![
                Span::raw(" ".repeat(left + MARKS + HANG)),
                Span::styled("─".repeat(side), th.s(Token::Line)),
                Span::styled(label.clone(), th.s(Token::Muted)),
                Span::styled("─".repeat(span_w.saturating_sub(side + width(&label))), th.s(Token::Line)),
            ]));
            for (fi, it) in items.iter().enumerate().take(h.saturating_sub(shown_rows + 2)) {
                let here = app.main.footer_cur == Some(fi);
                let fill = if here { th.fill(Token::Selection) } else { Style::default() };
                let (cell, tok) = match it.status.as_deref() {
                    Some("done") => ("[x] ", Token::Done),
                    Some("doing") => ("[/] ", Token::Doing),
                    Some(_) => ("[ ] ", Token::Text),
                    None => ("  · ", Token::Muted),
                };
                let tw = text_width(ctx, app.screen_width, app.detail_shows(), 0);
                let text: String = it.text.chars().take(tw).collect();
                let used = left + MARKS + HANG + width(&text);
                let meta: String = it.meta.chars().take(META + 8).collect();
                let pad = meta_right.saturating_sub(used + width(&meta)).max(2);
                // The mouse reaches these rows (keymap.md §0): the box toggles it, the text opens
                // it where it lives.
                let y = body.y + lines.len() as u16;
                let bx = body.x + (left + MARKS) as u16;
                if it.status.is_some() {
                    crate::ui::target(render, bx, bx + 3, y, crate::ui::Click::FooterBox(it.id.clone()));
                }
                crate::ui::target(render, bx + 4, body.x + used as u16, y, crate::ui::Click::Node(it.id.clone()));
                lines.push(TLine::from(vec![
                    Span::raw(" ".repeat(left + MARKS)),
                    Span::styled(cell, th.s(tok).patch(fill)),
                    Span::styled(text, th.s(if it.status.as_deref() == Some("done") { Token::Muted } else { Token::Text }).patch(fill)),
                    Span::styled(" ".repeat(pad), fill),
                    Span::styled(meta, th.s(Token::Muted).patch(fill)),
                ]));
            }
        }
    }
    f.render_widget(Paragraph::new(lines), body);
    // The scrollbar (mouse.md §5): one column at the right edge, only when the document overflows
    // (not in Focus, not under 60 columns). Clicking the track pages; dragging the thumb scrolls.
    app_scrollbar_draw(render, f, app_view(fv), panel, body, prepared.scroll.0, prepared.scroll.1, &th);
    if let Some(fv) = fv {
        // The month, top-aligned with the header, right of the text (and the meta).
        if let (true, Some(Target::Journal { date })) = (fv.month_fits(w), app.doc.as_ref().map(|d| &d.target)) {
            let x = left_edge(ctx, w) + fv.block() + 2;
            let m = month(app, *date);
            let r = Rect { x: area.x + x as u16, y: area.y, width: MONTH_W as u16 + 1, height: (m.len() as u16).min(area.height) };
            f.render_widget(Paragraph::new(m), r);
            // Each day is a button (mouse.md §3): rows start after the title and weekday line.
            {
                use chrono::Datelike;
                let first = date.with_day(1).unwrap();
                let offset = first.weekday().num_days_from_monday() as u16;
                let mut d = first;
                while d.month() == first.month() {
                    let i = offset + d.day() as u16 - 1;
                    let (row, col) = (i / 7, i % 7);
                    let y = r.y + 2 + row;
                    if y < r.bottom() {
                        crate::ui::target(render, r.x + col * 3, r.x + col * 3 + 2, y, crate::ui::Click::Day(d));
                    }
                    d += chrono::Duration::days(1);
                }
            }
        }
        // With the keys footer off (ui.rs draws it when it's on): a live message, or a failed
        // save, on the bottom row; the word count and the clock alone in the corner.
        if !fv.has(El::Footer) {
            let bottom = area.bottom().saturating_sub(1);
            let failed = app.doc.as_ref().is_some_and(|d| d.blocks().iter().any(|l| l.save_error.is_some()));
            let msg = app.toast.as_ref().filter(|t| t.alive_at(app.ui.now_ms)).map(|t| (t.parts.iter().map(|(s, _)| s.as_str()).collect::<String>(), Token::Muted));
            let msg = msg.or(failed.then(|| ("not saved · :retry".to_string(), Token::Overdue)));
            if let Some((text, tok)) = msg {
                let x = left_edge(ctx, w) + MARKS + HANG;
                let r = Rect { x: area.x + x.min(w) as u16, y: bottom, width: (w.saturating_sub(x)) as u16, height: 1 };
                f.render_widget(Paragraph::new(TLine::styled(text, th.s(tok))), r);
            }
            let mut corner = Vec::new();
            if fv.has(El::Wordcount) {
                let n = word_count(app);
                corner.push(format!("{n} word{}", if n == 1 { "" } else { "s" }));
            }
            if fv.has(El::Clock) {
                corner.push(app.derived.clock.clone());
            }
            if !corner.is_empty() {
                let text = corner.join("  ");
                let cw = width(&text) as u16 + 1;
                let r = Rect { x: area.right().saturating_sub(cw + 1), y: bottom, width: cw, height: 1 };
                f.render_widget(Paragraph::new(TLine::styled(text, th.s(Token::Muted).add_modifier(Modifier::DIM))), r);
            }
        }
    }
    if app.main.write {
        if let Some((col, row)) = prepared.cursor.filter(|&(_, row)| (row as usize) < h && body.width > 0) {
            let cx = (body.x as usize + left + col as usize).min(body.right() as usize - 1) as u16;
            let cy = body.y + row;
            // Under help, the leader's panel or any overlay the caret would show through it
            // An overlay that takes text places its own.
            if !focused {
                // The keyboard is in a panel: this view keeps its caret as a cell (sidebar.md §5.1).
                crate::sidebar_ui::unfocused_caret(f, &app.theme, cx, cy);
            } else if crate::ui::caret_allowed(app) {
                f.set_cursor_position((cx, cy));
            }
            if app.main.link_open {
                if focused && !panel { link_popup(render, f, host, prepared, editor, prepared.popup_area, cx, cy); }
            }
        }
    }
    render.doc_view = Some((body.x + left as u16, body.y, body.width.saturating_sub(left as u16), body.height));
    render.doc_hits = hits;
    render.image_places.extend(places);
}

/// A click on the marks column (`≠`) of a conflicted line: that line.
pub fn conflict_mark_at(app: &App, x: u16, y: u16) -> Option<usize> {
    let r = app.render.doc_hits.iter().find(|h| h.y == y && h.first)?;
    let l = app.doc.as_ref()?.blocks().get(r.line)?;
    (l.conflict && x + (MARKS as u16) >= r.hang_x && x < r.hang_x).then_some(r.line)
}

/// Where a click at (x, y) lands in the document (mouse.md §3), as the engine hit-tests its
/// view: the line and byte of the grapheme under it (the right half of a wide one goes after
/// it), the row's end past its text, a blank or meta row's the end of the text row above. `hang`
/// is true when the click was on a task's box.
pub fn hit(app: &App, x: u16, y: u16) -> Option<(usize, usize, bool)> {
    hit_at(app, &app.render, x, y)
}

/// [`hit`] in the view a render drew.
pub(crate) fn hit_at(app: &App, render: &RenderOutput, x: u16, y: u16) -> Option<(usize, usize, bool)> {
    use crate::editor::DocHit;
    let (vx, vy, vw, vh) = render.doc_view?;
    if y < vy || y >= vy + vh || x >= vx + vw {
        return None;
    }
    let d = app.doc.as_ref()?;
    let (col, row) = (x.saturating_sub(vx), y - vy);
    match d.hit(col, row)? {
        DocHit::Text(p) => Some((p.line, p.byte, false)),
        DocHit::Hang { line, task_box: true, .. } => Some((line, 0, true)),
        // The hang or the marks: where the row starts.
        DocHit::Hang { row, .. } | DocHit::Marks { row, .. } => Some((row.line, row.byte, false)),
    }
}

pub(crate) fn draw_popup(render: &mut RenderOutput, f: &mut Frame, app: &App, prepared: &PreparedDoc, editor: &super::EditorState) {
    if editor.write && editor.link_open {
        if let Some((cx, cy)) = prepared.cursor_cell() {
            link_popup(render, f, app, prepared, editor, prepared.popup_area, cx, cy);
        }
    }
}

/// The `[[` popup: 40 columns, up to 8 rows, under the caret (above when there's no room).
fn link_popup(render: &mut RenderOutput, f: &mut Frame, app: &App, prepared: &PreparedDoc, editor: &super::EditorState, area: Rect, cx: u16, cy: u16) {
    use ratatui::widgets::{Block, Borders, Clear};
    let th = app.theme;
    let Some((_, m, create)) = prepared.link.as_ref() else { return };
    let (m, create) = (m.clone(), create.clone());
    let mut rows: Vec<(String, bool)> = m.iter().map(|(l, _)| (l.clone(), false)).collect();
    if let Some(c) = &create {
        rows.push((format!("+ new page \"{c}\""), true));
    }
    if rows.is_empty() {
        rows.push(("no page matches".into(), true));
    }
    let h = (rows.len() as u16 + 2).min(10).min(area.height);
    let w = 40u16.min(area.width);
    let below = cy + 1 + h <= area.bottom();
    let y = if below { cy + 1 } else { cy.saturating_sub(h).max(area.y) };
    let x = cx.saturating_sub(2).min(area.right().saturating_sub(w)).max(area.x);
    let r = Rect { x, y, width: w, height: h };
    f.render_widget(Clear, r);
    let block = Block::default().borders(Borders::ALL).border_style(th.s(Token::Line)).style(th.fill(Token::Raised));
    let inner = block.inner(r);
    f.render_widget(block, r);
    // Rows are a menu (mouse.md): hover selects, a click inserts. The popup takes the clicks
    // over its box, nothing passes to the text under it.
    let live = create.is_some() || !m.is_empty();
    for i in 0..rows.len().min(inner.height as usize) {
        crate::ui::target(render, inner.x, inner.right(), inner.y + i as u16, if live { crate::ui::Click::LinkRow(i) } else { crate::ui::Click::Text });
    }
    let lines: Vec<TLine<'static>> = rows
        .into_iter()
        .enumerate()
        .map(|(i, (label, muted))| {
            let on = editor.link_sel == Some(i);
            let st = if on { th.s(Token::Text).patch(th.fill(Token::Selection)) } else if muted { th.s(Token::Muted) } else { th.s(Token::Text) };
            let text: String = label.chars().take(inner.width as usize).collect();
            let pad = (inner.width as usize).saturating_sub(width(&text));
            TLine::from(vec![Span::styled(text, st), Span::styled(" ".repeat(pad), st)])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn app_view(fv: Option<FocusView>) -> bool {
    fv.is_none()
}

fn app_scrollbar_draw(render: &mut RenderOutput, f: &mut Frame, show: bool, compact: bool, body: Rect, total: usize, scroll: usize, th: &crate::theme::Theme) {
    let h = body.height as usize;
    if !show || total <= h || !compact && body.width < 60 || h == 0 || body.width == 0 {
        return;
    }
    let x = body.right() - 1;
    let thumb_h = (h * h / total).max(1);
    let thumb_y = (scroll * h / total).min(h - thumb_h);
    let buf = f.buffer_mut();
    for i in 0..h {
        let y = body.y + i as u16;
        let on = i >= thumb_y && i < thumb_y + thumb_h;
        buf[(x, y)].set_symbol(if on { "┃" } else { "│" }).set_style(th.s(if on { Token::Muted } else { Token::Line }));
        crate::ui::target(render, x, x + 1, y, crate::ui::Click::Scroll(i as u16));
    }
    render.doc_scrollbar = Some((body.y, h as u16, total, thumb_y as u16, thumb_h as u16));
}
