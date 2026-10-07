//! Drawing the document (tui-editor.md §3): marks, hang, a text column wrapped at
//! min(72, available), the meta right-aligned (or on its own row), the journal header and the
//! page title. The caret is the terminal's cursor; the selection is the `sel` fill.

use crate::app::App;
use crate::ui::RenderOutput;
use crate::doc::{Line, Target};
use crate::text::width;
use crate::theme::Token;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span};
use ratatui::widgets::Paragraph;
use thc_core::outline::Kind;
use thc_core::tui_config::{El, FocusSet};

const MARKS: usize = 2;
/// `Line::conflict_with` for a line moved here because its parent was deleted elsewhere.
pub const MOVED_HERE: &str = "\u{1}moved here";
const HANG: usize = 4;
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
    if rail(ctx) == 0 { 72 } else { w.saturating_sub(MARKS + HANG + 2 + META).clamp(60, 72) }
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

/// How far a code block is scrolled sideways for a caret at `caret_col` in a `tw`-wide column.
fn code_offset(caret_col: usize, tw: usize) -> usize {
    (caret_col + 2).saturating_sub(tw)
}

/// What a line looks like: its hang and text style (§3.2).
struct Form {
    hang: String,
    hang_style: Style,
    text_style: Style,
}

fn form(app: &App, l: &Line) -> Form {
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

/// One visual row ready to draw: (line index, text byte range, row index within the line).
#[derive(Clone, Copy)]
struct Row {
    line: usize,
    start: usize,
    end: usize,
    first: bool,
    /// The meta's own row under the line (narrow, or the text runs into it).
    meta_row: bool,
    blank: bool,
    /// Rows reserved under an attachment's chip for its image (attachments.md §3): (index,
    /// rows, cols). Not caret stops; drawn blank, then the image goes over them.
    image: Option<(u16, u16, u16)>,
}

/// The document's visual rows (blank spacer rows included), and the caret's (row, col).
fn layout(app: &mut App, w: usize) -> (Vec<Row>, Option<(usize, isize)>) {
    let ctx = DocContext::from_app(app);
    let app_vault = app.vault.paths.vault.clone();
    let inline_images = app.derived.inline_images;
    let attachments = &app.derived.attachments;
    let sw = app.screen_width;
    let detail = app.show_detail;
    let d = app.doc.as_mut().unwrap();
    let mut rows = Vec::new();
    let mut caret = None;
    let n = d.lines().len();
    for i in 0..n {
        // A blank row before the note: its `gap`, else the default for its kind (Doc::effective_gap).
        if d.effective_gap(i) && !rows.last().is_some_and(|r: &Row| r.blank) {
            rows.push(Row { line: i, start: 0, end: 0, first: false, meta_row: false, blank: true, image: None });
        }
        if folded_hidden(d.lines(), i, &d.view.folds) {
            continue;
        }
        let tw = text_width(ctx, sw, detail, d.lines()[i].depth);
        let wr = d.rows_of(i, tw);
        // Whether the meta gets its own row follows the saved meta, never the live chip: lines
        // below don't jump while a token is typed (the chip may run into the margin instead).
        let meta = meta_of(ctx, &d.lines()[i], None);
        let last_row_end_col = width(&d.lines()[i].text[wr[0].0..wr[0].1]);
        let own_row = !meta.is_empty() && (w < 60 || (MARKS + HANG + d.lines()[i].depth * 4 + last_row_end_col + 2 > w.saturating_sub(left_edge(ctx, w)).saturating_sub(width(&meta))));
        for (k, (s, e)) in wr.iter().enumerate() {
            if i == d.view.caret.line && d.view.caret.byte >= *s && (d.view.caret.byte < *e || (d.view.caret.byte == *e && (k + 1 == wr.len() || d.lines()[i].text.as_bytes().get(*e) == Some(&b'\n')))) && caret.is_none() {
                // On the first row the marker is drawn in the hang: columns count from after it,
                // and a caret inside it sits in the hang (negative).
                let m = if k == 0 { marker_len(&d.lines()[i]) } else { 0 };
                let t = &d.lines()[i].text;
                let mut col = if d.view.caret.byte >= s + m { width(&t[s + m..d.view.caret.byte]) as isize } else { -(width(&t[d.view.caret.byte..s + m]) as isize) };
                // A code block scrolls sideways to keep the caret in view.
                if is_code(&d.lines()[i]) {
                    col -= code_offset(width(&t[*s..d.view.caret.byte]), tw) as isize;
                }
                caret = Some((rows.len(), col));
            }
            rows.push(Row { line: i, start: *s, end: *e, first: k == 0, meta_row: false, blank: false, image: None });
        }
        if own_row {
            rows.push(Row { line: i, start: 0, end: 0, first: false, meta_row: true, blank: false, image: None });
        }
        // An image attachment: rows under its chip, reserved whether or not the caret's on it,
        // so nothing moves while you type (attachments.md §3).
        if let (true, Some((_, path))) = (inline_images, image_line(&d.lines()[i].text)) {
            if thc_core::attach::is_image(&path) {
                if let Some((w, h)) = attachments.get(&(app_vault.clone(), path.clone())).and_then(|a| a.dimensions) {
                    let (cols, n) = crate::images::cells(w, h, tw.min(u16::MAX as usize) as u16);
                    for k in 0..n {
                        rows.push(Row { line: i, start: 0, end: 0, first: false, meta_row: false, blank: false, image: Some((k, n, cols)) });
                    }
                }
            }
        }
    }
    (rows, caret)
}

/// Line `i` is hidden under a folded parent above (folds are the view's).
pub(crate) fn folded_hidden(lines: &[Line], i: usize, folds: &std::collections::HashSet<String>) -> bool {
    if folds.is_empty() {
        return false;
    }
    let mut depth = lines[i].depth;
    for l in lines[..i].iter().rev() {
        if l.depth < depth {
            if folds.contains(&l.id) {
                return true;
            }
            depth = l.depth;
        }
        if depth == 0 {
            break;
        }
    }
    false
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
    app.doc.as_ref().map_or(0, |d| d.lines().iter().map(|l| l.text.split_whitespace().count()).sum())
}

/// One visible row of the document on screen, for the mouse (mouse.md §3): which line and bytes
/// it shows, where its text starts, where its hang is. The renderer writes these every frame, so
/// a click maps to exactly what was drawn.
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
}

impl DocContext {
    pub fn from_app(app: &App) -> Self {
        let journal = app.doc.as_ref().is_some_and(|d| matches!(d.target, Target::Journal { .. }));
        Self {
            focus: app.focus_mode.then_some(FocusView { set: app.focus_cfg.set, width: app.focus_cfg.width() as usize, journal }),
            writing: app.doc_write,
            rail: if app.rail_shows() { RAIL_W } else { 0 },
        }
    }
}

/// A content fingerprint also catches in-place edits (including folds and same-size text
/// replacements), not just saves. It reads no clock, store, or interior cache.
fn source_revision(app: &App) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    app.vault.paths.vault.hash(&mut h);
    app.screen_width.hash(&mut h);
    app.show_detail.hash(&mut h);
    app.focus_mode.hash(&mut h);
    format!("{:?}", app.focus_cfg).hash(&mut h);
    app.doc_write.hash(&mut h);
    app.rail_shows().hash(&mut h);
    app.crumb_shows().hash(&mut h);
    app.today.hash(&mut h);
    if let Some(d) = &app.doc {
        format!("{:?}", d.target).hash(&mut h);
        d.root.hash(&mut h);
        d.view.caret.line.hash(&mut h);
        d.view.caret.byte.hash(&mut h);
        d.lines().len().hash(&mut h);
        for l in d.lines() {
            l.id.hash(&mut h);
            l.text.hash(&mut h);
            l.depth.hash(&mut h);
            l.kind().hash(&mut h);
            l.status.hash(&mut h);
            l.gap.hash(&mut h);
            d.view.folds.contains(&l.id).hash(&mut h);
            l.meta.hash(&mut h);
            l.conflict.hash(&mut h);
            l.conflict_with.hash(&mut h);
        }
    }
    h.finish()
}

/// A prepared layout. Cache filling never changes presentation scroll.
pub struct PreparedDoc {
    area: Rect,
    revision: u64,
    rows: Vec<Row>,
    caret: Option<(usize, isize)>,
    header: Vec<TLine<'static>>,
    days: Vec<(u16, u16, u16, chrono::NaiveDate)>,
    body: Rect,
}

impl PreparedDoc {
    pub(crate) fn viewport(&self, app: &App) -> Option<crate::update::Viewport> {
        let ctx = DocContext::from_app(app);
        Some(crate::update::Viewport::Document {
            identity: crate::runtime_effects::document_identity(app)?, rows: self.rows.len(),
            caret: self.caret.map(|(row, _)| row), height: self.body.height as usize,
            free: app.doc_scroll_free, typewriter: ctx.focus.map_or(app.tui_prefs.typewriter, |f| f.has(El::Typewriter)),
        })
    }
}

pub(crate) fn prepare(app: &mut App, mut area: Rect) {
    let source_area = area;
    let ctx = DocContext::from_app(app);
    let w = area.width as usize;
    if app.crumb_shows() && area.height > 4 {
        area.y += 1;
        area.height -= 1;
    }
    let mut days = Vec::new();
    let header = header(ctx, &mut days, app, w);
    let head_h = (header.len() as u16).min(area.height);
    let body = Rect { y: area.y + head_h, height: area.height.saturating_sub(head_h), ..area };
    let (rows, caret) = layout(app, w);
    app.derived.doc = Some(PreparedDoc { area: source_area, revision: source_revision(app), rows, caret, header, days, body });
}

pub fn draw(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect) {
    let Some(prepared) = app.derived.doc.as_ref() else { return };
    // Fall back to an empty document until the runtime prepares the new inputs. In
    // particular, never slice changed text using byte ranges from an older layout.
    if prepared.area != area || prepared.revision != source_revision(app) { return }
    let ctx = DocContext::from_app(app);
    let fv = focus(ctx);
    let w = area.width as usize;
    // Narrower than the rail needs: a crumb above the title, its first part a click to the
    // list (navigation.md §3).
    let area = if app.crumb_shows() && area.height > 4 {
        let th = app.theme;
        let g = th.glyphs();
        let x0 = left_edge(ctx, w) + MARKS + HANG;
        let (sym, list, here, action) = match &app.doc.as_ref().unwrap().target {
            // A node inside a page (an issue): the page it lives in, not the Pages list.
            Target::Page { .. } => match &app.derived.data.document.parent_label {
                Some(parent) => (g.page, parent.clone(), app.derived.data.document.label.clone(), "doc.done"),
                None => (g.page, "Pages".to_string(), app.derived.data.document.label.clone(), "go.pages"),
            },
            Target::Journal { date } => (g.journal, "Journal".to_string(), date.format("%a %d %b").to_string(), "go.journal"),
        };
        // Not home: the vault leads, in its accent (vaults.md §8): `acme › ¶ Pages › Health`.
        let vault = if app.vault_home { String::new() } else { format!("{} › ", app.vault_name) };
        let lead = format!("{vault}{sym} ");
        let line = TLine::from(vec![
            Span::raw(" ".repeat(x0)),
            Span::styled(vault.clone(), th.s(Token::Accent)),
            Span::styled(format!("{sym} "), th.s(Token::Muted)),
            Span::styled(list.clone(), th.s(Token::Muted).add_modifier(Modifier::UNDERLINED)),
            Span::styled(format!(" › {here}"), th.s(Token::Muted)),
        ]);
        let lx = area.x + (x0 + width(&lead)) as u16;
        crate::ui::target(render, lx, lx + width(&list) as u16, area.y, crate::ui::Click::Action(action));
        f.render_widget(Paragraph::new(line), Rect { height: 1, ..area });
        Rect { y: area.y + 1, height: area.height - 1, ..area }
    } else {
        area
    };
    let head_h = (prepared.header.len() as u16).min(area.height);
    for &(x0, x1, row, date) in &prepared.days {
        if row < head_h {
            crate::ui::target(render, area.x + x0, area.x + x1, area.y + row, crate::ui::Click::Day(date));
        }
    }
    f.render_widget(Paragraph::new(prepared.header.clone()), Rect { height: head_h, ..area });
    let body = prepared.body;
    let rows = &prepared.rows;
    let caret = prepared.caret;
    let th = app.theme;
    let h = body.height as usize;
    render.doc_view_rows = h;
    let d = app.doc.as_ref().unwrap();
    let sel = d.selection();
    let left = left_edge(ctx, w);
    let meta_right = fv.map_or(left + if rail(ctx) > 0 { block_w(ctx, w) } else { BLOCK }, |f| left + f.block()).min(w);
    let mut lines: Vec<TLine<'static>> = Vec::new();
    let mut hits: Vec<HitRow> = Vec::new();
    let mut places: Vec<crate::images::Place> = Vec::new();
    for r in rows.iter().skip(d.scroll).take(h) {
        if let Some((k, n, cols)) = r.image {
            // The image goes over its reserved rows, when they're all on screen.
            if k == 0 && lines.len() + n as usize <= h {
                if let Some((_, path)) = image_line(&d.lines()[r.line].text) {
                    let indent = d.lines()[r.line].depth * 4;
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
        let l = &d.lines()[r.line];
        let fm = form(app, l);
        let indent = l.depth * 4;
        let mut spans: Vec<Span<'static>> = vec![Span::raw(" ".repeat(left))];
        let in_vsel = app.doc_vsel.is_some_and(|v| r.line >= v.min(d.view.caret.line) && r.line <= v.max(d.view.caret.line));
        let navigate_here = (!app.doc_write && r.line == d.view.caret.line && app.doc_footer_cur.is_none()) || in_vsel;
        let row_fill = if navigate_here { th.fill(Token::Selection) } else { Style::default() };
        if r.meta_row {
            let meta = meta_of(ctx, l, (r.line == d.view.caret.line).then_some(app.derived.caret_chip.as_deref()).flatten());
            let pad = meta_right.saturating_sub(left + width(&meta));
            spans.push(Span::styled(" ".repeat(pad), row_fill));
            spans.push(Span::styled(meta, th.s(Token::Muted).add_modifier(Modifier::DIM)));
            lines.push(TLine::from(spans));
            continue;
        }
        // Marks.
        let mark = if l.conflict {
            Span::styled("≠ ", th.s(Token::Conflict))
        } else if l.remote_text.is_some() {
            Span::styled("◆ ", th.s(Token::Agent))
        } else if l.save_error.is_some() || l.saving_since.is_some_and(|t| app.derived.age(t).as_secs() >= 3) {
            Span::styled("◌ ", th.s(Token::Muted))
        } else if r.first && d.view.folds.contains(&l.id) {
            Span::styled("▸ ", th.s(Token::Muted))
        } else {
            Span::raw("  ")
        };
        spans.push(if r.first && !app.focus_mode { mark } else { Span::raw("  ") });
        spans.push(Span::styled(" ".repeat(indent), row_fill));
        spans.push(if r.first { Span::styled(format!("{:>4}", fm.hang), fm.hang_style.patch(row_fill)) } else { Span::styled("    ", row_fill) });
        // Text, with the selection filled (a first row's marker is in the hang).
        let start = if r.first { (r.start + marker_len(l)).min(r.end) } else { r.start };
        let r = &Row { start, ..*r };
        // A code block: cut to the column, scrolled to the caret, `→` where it runs on.
        let tw_here = text_width(ctx, app.screen_width, app.show_detail, l.depth);
        let (r, more) = if is_code(l) {
            let off = if r.line == d.view.caret.line && app.doc_write {
                let caret_row = d.view.caret.byte >= r.start && d.view.caret.byte <= r.end;
                if caret_row { code_offset(width(&l.text[r.start..d.view.caret.byte]), tw_here) } else { 0 }
            } else {
                0
            };
            let (mut a, mut col) = (r.start, 0);
            while a < r.end && col < off {
                let g = crate::text::next_char(&l.text[..r.end], a);
                col += width(&l.text[a..g]);
                a = g;
            }
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
        if fv.is_some_and(|f| f.has(El::Dim)) && r.line != d.view.caret.line {
            base = base.add_modifier(Modifier::DIM);
        }
        // While the caret's line is written, every token is underlined in its hue (§5).
        let typing = app.doc_write && r.line == d.view.caret.line;
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
        let image = image_line(&l.text).filter(|_| !(app.doc_write && r.line == d.view.caret.line));
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
        } else if l.kind() == Kind::Para && (l.text == "---" || l.text == "***") && !(app.doc_write && r.line == d.view.caret.line) {
            // A rule: a line across the text column (the text is still `---`).
            let tw = text_width(ctx, app.screen_width, app.show_detail, l.depth);
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
            let mut meta = meta_of(ctx, l, (r.line == d.view.caret.line).then_some(app.derived.caret_chip.as_deref()).flatten());
            // While the caret is in a link: the ↗ open chip (mouse.md §3).
            if app.doc_write && r.line == d.view.caret.line && crate::doc_app::link_at(&l.text, d.view.caret.byte).is_some() {
                meta = "↗ open".to_string();
            }
            // On an attachment's line: ⌃O opens it.
            if app.doc_write && r.line == d.view.caret.line && image_line(&l.text).is_some() {
                meta = "⌃O open".to_string();
            }
            // The near-miss chip (writing.md §5), for 3 s after the save.
            if let Some((_, typed, existing, _, since)) = app.near_miss.as_ref().filter(|n| n.0 == l.id && app.derived.age(n.4).as_secs() < 3) {
                let _ = since;
                meta = format!("new page \"{typed}\" · ⌃O {existing}?");
            }
            if fv.is_none() && d.view.folds.contains(&l.id) {
                let n = d.descendants(r.line);
                meta = if meta.is_empty() { format!("+{n}") } else { format!("{meta} · +{n}") };
            }
            let own_row = rows.iter().any(|x| x.meta_row && x.line == r.line);
            if !meta.is_empty() && !own_row {
                let used = left + MARKS + indent + HANG + width(text);
                let pad = meta_right.saturating_sub(used + width(&meta)).max(2);
                spans.push(Span::styled(" ".repeat(pad), row_fill));
                // The chips are buttons (mouse.md §3): ↗ open, the near miss and ≠ act as one;
                // a date opens its editor.
                let y = body.y + lines.len() as u16;
                let mut x = body.x + (used + pad) as u16;
                let whole = |field: &'static str| crate::ui::Click::Meta { line: r.line, field };
                if meta.starts_with("↗") || meta.starts_with("new page") {
                    crate::ui::target(render, x, x + width(&meta) as u16, y, whole("open"));
                } else if meta.starts_with('≠') {
                    crate::ui::target(render, x, x + width(&meta) as u16, y, whole("conflict"));
                } else {
                    for seg in meta.split(" · ") {
                        let sw = width(seg) as u16;
                        let field = if seg.starts_with("due") { Some("due") } else if seg.starts_with('!') || seg.starts_with('↻') || seg.starts_with('+') || seg.starts_with("done") { None } else { Some("sched") };
                        if let Some(f) = field {
                            crate::ui::target(render, x, x + sw, y, whole(f));
                        }
                        x += sw + 3;
                    }
                }
                let flashing = l.flash_until.is_some_and(|t| t > app.derived.now);
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
        lines.push(TLine::from(spans));
    }
    // The footer after the document's own lines (read-only; the caret never goes there).
    let shown_rows = rows.len().saturating_sub(d.scroll);
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
                let here = app.doc_footer_cur == Some(fi);
                let fill = if here { th.fill(Token::Selection) } else { Style::default() };
                let (cell, tok) = match it.status.as_deref() {
                    Some("done") => ("[x] ", Token::Done),
                    Some("doing") => ("[/] ", Token::Doing),
                    Some(_) => ("[ ] ", Token::Text),
                    None => ("  · ", Token::Muted),
                };
                let tw = text_width(ctx, app.screen_width, app.show_detail, 0);
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
    app_scrollbar_draw(render, f, app_view(fv), body, rows.len(), d.scroll, &th);
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
            let failed = app.doc.as_ref().is_some_and(|d| d.lines().iter().any(|l| l.save_error.is_some()));
            let msg = app.toast.as_ref().filter(|t| t.alive_at(app.derived.now)).map(|t| (t.parts.iter().map(|(s, _)| s.as_str()).collect::<String>(), Token::Muted));
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
    if app.doc_write {
        if let Some((cr, col)) = caret {
            if cr >= d.scroll && cr < d.scroll + h {
                let l = &d.lines()[rows[cr].line];
                let x = (left + MARKS + l.depth * 4 + HANG) as isize + col;
                let cx = ((body.x as isize + x).max(0) as usize).min(body.right() as usize - 1) as u16;
                let cy = body.y + (cr - d.scroll) as u16;
                // Under help, the leader's panel or any overlay the caret would show through it
                // An overlay that takes text places its own.
                if crate::ui::caret_allowed(app) {
                    f.set_cursor_position((cx, cy));
                }
                if app.link_open {
                    link_popup(render, f, app, area, cx, cy);
                }
            }
        }
    }
    render.doc_hits = hits;
    render.image_places = places;
}

/// A click on the marks column (`≠`) of a conflicted line: that line.
pub fn conflict_mark_at(app: &App, x: u16, y: u16) -> Option<usize> {
    let r = app.render.doc_hits.iter().find(|h| h.y == y && h.first)?;
    let l = app.doc.as_ref()?.lines().get(r.line)?;
    (l.conflict && x + (MARKS as u16) >= r.hang_x && x < r.hang_x).then_some(r.line)
}

/// Where a click at (x, y) lands in the document (mouse.md §3): the line and byte of the
/// grapheme under it (the right half of a wide one goes after it), the row's end past its text,
/// the line's start in the hang. `hang` is true when the click was in the hang.
pub fn hit(app: &App, x: u16, y: u16) -> Option<(usize, usize, bool)> {
    hit_rows(app, &app.render.doc_hits, x, y)
}

pub(crate) fn hit_rows(app: &App, hits: &[HitRow], x: u16, y: u16) -> Option<(usize, usize, bool)> {
    let d = app.doc.as_ref()?;
    let Some(r) = hits.iter().find(|h| h.y == y) else {
        // A row between the text's rows that isn't a stop (a gap, a meta on its own row): the
        // end of the nearest stop row above it (motion.md §4). Below the text, the footers and
        // the rest keep their own clicks.
        if !hits.iter().any(|h| h.y > y) {
            return None;
        }
        let r = hits.iter().filter(|h| h.y < y).max_by_key(|h| h.y)?;
        let text = &d.lines().get(r.line)?.text;
        return Some((r.line, row_last(text, r.end), false));
    };
    let text = &d.lines().get(r.line)?.text;
    // The layout is the last frame's: if the line changed under it since (a remote change, a
    // reopen), the click lands at the line's end rather than past it (sync soak: a panic).
    if r.end > text.len() || !text.is_char_boundary(r.start) || !text.is_char_boundary(r.end) {
        return Some((r.line, text.len(), false));
    }
    if x < r.text_x {
        // Only the box's own three cells (`[ ]`) are its button; the gap after it is margin.
        return Some((r.line, if r.first { 0 } else { r.start }, r.first && x >= r.hang_x && x < r.hang_x + 3));
    }
    let mut col = r.text_x;
    let mut b = r.start;
    while b < r.end {
        let next = crate::text::next_char(&text[..r.end], b);
        let w = width(&text[b..next]).max(1) as u16;
        if x < col + w {
            // The right half of a wide character puts the caret after it.
            let after = w > 1 && x >= col + w / 2;
            return Some((r.line, if !after { b } else if next == r.end { row_last(text, next) } else { next }, false));
        }
        col += w;
        b = next;
    }
    Some((r.line, row_last(text, r.end), false))
}

/// The last position a row owns, given the byte its range ends at: where the next row starts
/// belongs to that row (motion.md §2), so a wrapped row's own end is just before it. A row ending
/// at a soft break or the note's end owns its end.
fn row_last(text: &str, end: usize) -> usize {
    if end >= text.len() || text.as_bytes().get(end) == Some(&b'\n') {
        return end.min(text.len());
    }
    crate::text::prev_char(text, end)
}

/// The `[[` popup: 40 columns, up to 8 rows, under the caret (above when there's no room).
fn link_popup(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect, cx: u16, cy: u16) {
    use ratatui::widgets::{Block, Borders, Clear};
    let Some((_, q)) = app.link_query() else { return };
    let th = app.theme;
    let (m, create) = app.link_matches(&q);
    let mut rows: Vec<(String, bool)> = m.iter().map(|(l, _)| (l.clone(), false)).collect();
    if let Some(c) = &create {
        rows.push((format!("+ new page \"{c}\""), true));
    }
    if rows.is_empty() {
        rows.push(("no page matches".into(), true));
    }
    let h = (rows.len() as u16 + 2).min(10);
    let w = 40u16.min(area.width);
    let below = cy + 1 + h <= area.bottom();
    let y = if below { cy + 1 } else { cy.saturating_sub(h) };
    let x = cx.saturating_sub(2).min(area.right().saturating_sub(w));
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
            let on = app.link_sel == Some(i);
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

fn app_scrollbar_draw(render: &mut RenderOutput, f: &mut Frame, show: bool, body: Rect, total: usize, scroll: usize, th: &crate::theme::Theme) {
    let h = body.height as usize;
    if !show || total <= h || body.width < 60 || h == 0 {
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
