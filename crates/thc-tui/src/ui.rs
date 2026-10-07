//! Drawing, per docs/design/tui-handoff.md. Frame: header · rule · [banner] · content ·
//! [capture drawer] · bar. No boxes except the floating overlays.

use crate::app::{App, Focus, Overlay, Row, ToastKind, View, VIEWS, fuzzy_positions};
use crate::node_row::{self, Gutter, Meta, RowSpec, width};
use crate::theme::{Theme, Token};
use chrono::{Datelike, Duration, NaiveDate};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use thc_core::dates::{self, DateVal};
use thc_core::model::Node;
use unicode_width::UnicodeWidthStr;

pub const SPLIT_AT: u16 = 120;

fn w(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

pub fn truncate_str(s: &str, max: usize, ell: &str) -> String {
    let spans = node_row::truncate_spans(&[Span::raw(s.to_string())], max, ell);
    spans.iter().map(|s| s.content.as_ref()).collect()
}

/// List width when the screen splits, per view (§3.4). None = single pane.
/// A document, with the rail of pages or days beside it when it fits (navigation.md §3).
fn draw_document(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect) {
    let rw = crate::doc_ui::RAIL_W as u16;
    if !app.rail_shows() || area.width < rw + 60 {
        return crate::doc_ui::draw(render, f, app, area);
    }
    draw_rail(render, f, app, Rect { width: rw - 1, ..area });
    let th = app.theme;
    let rule: Vec<Line> = (0..area.height).map(|_| Line::styled(th.glyphs().vsep, th.s(Token::Line))).collect();
    f.render_widget(Paragraph::new(rule), Rect { x: area.x + rw - 1, width: 1, ..area });
    crate::doc_ui::draw(render, f, app, Rect { x: area.x + rw, width: area.width - rw, ..area });
}

/// The rail: `PAGES` (or `DAYS`), a row per page or day, the current one marked `▌`, and a
/// last row that opens the list. Every row is a click.
fn draw_rail(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect) {
    use crate::app::RailItem;
    let th = app.theme;
    let g = th.glyphs();
    let w = area.width as usize;
    let page = matches!(app.doc.as_ref().map(|d| &d.target), Some(crate::editor::Target::Page { .. }));
    let open_id = app.page_open.clone();
    let mut lines: Vec<Line> = Vec::new();
    let head = if page { "PAGES" } else { "DAYS" };
    lines.push(Line::from(Span::styled(format!(" {head}"), th.s(Token::Muted).add_modifier(Modifier::BOLD))));
    target(render, area.x, area.x + 1 + head.len() as u16, area.y, Click::Action(if page { "go.pages" } else { "go.journal" }));
    let room = (area.height as usize).saturating_sub(2);
    let total = app.rail.len();
    for (i, item) in app.rail.iter().take(room).enumerate() {
        let y = area.y + 1 + i as u16;
        let (label, count, current, click) = match item {
            RailItem::Page { id, title, open } => (title.clone(), *open, open_id.as_deref() == Some(id.as_str()), Click::Node(id.clone())),
            RailItem::Day { date, count } => {
                let l = if *date == app.today { format!("today · {}", date.format("%a %d")) } else { date.format("%a %d %b").to_string() };
                (l, *count, *date == app.journal_date, Click::Day(*date))
            }
        };
        let num = if count > 0 { count.to_string() } else { String::new() };
        let name_w = w.saturating_sub(2 + num.len() + 1);
        let name = truncate_str(&label, name_w, g.ellipsis);
        let pad = w.saturating_sub(1 + crate::text::width(&name) + num.len() + 1);
        let mark = if current { Span::styled("▌", th.s(Token::Accent)) } else { Span::raw(" ") };
        lines.push(Line::from(vec![
            mark,
            Span::styled(name, th.s(if current { Token::Text } else { Token::Muted })),
            Span::raw(" ".repeat(pad)),
            Span::styled(num, th.s(Token::Muted)),
            Span::raw(" "),
        ]));
        target(render, area.x, area.x + area.width, y, click);
    }
    // The last row opens the rest: the Pages list, or the ⌃O finder for any day (navigation.md §3).
    {
        let (label, action) = if page { (format!(" all pages: {total} "), "go.pages") } else { (" any day: ⌃O ".to_string(), "finder.open") };
        let side = w.saturating_sub(crate::text::width(&label)) / 2;
        let row = Line::from(vec![
            Span::styled("─".repeat(side), th.s(Token::Line)),
            Span::styled(label.clone(), th.s(Token::Muted)),
            Span::styled("─".repeat(w.saturating_sub(side + crate::text::width(&label))), th.s(Token::Line)),
        ]);
        let y = area.y + lines.len() as u16;
        if y < area.y + area.height {
            target(render, area.x, area.x + area.width, y, Click::Action(action));
            lines.push(row);
        }
    }
    f.render_widget(Paragraph::new(lines), area);
}

fn f_width(app: &App) -> u16 {
    app.screen_width
}

pub(crate) fn split_width(app: &App, total: u16) -> Option<u16> {
    if total < SPLIT_AT || !app.show_detail {
        return None;
    }
    let std = (total as u32 * 60 / 100).min(96) as u16;
    // A document keeps its full width unless the terminal is wide (tui-editor.md §3.1).
    if app.doc.is_some() {
        let w = crate::doc_ui::pane_width(total, true);
        return (w < total).then_some(w);
    }
    match app.view {
        View::Tasks => None,
        // Nothing to select (an empty Today, the first-run welcome): no pane, the message
        // takes the full width.
        View::Today if !app.rows.iter().any(|r| matches!(r, Row::Node { .. })) => None,
        // With no matches the preview has nothing to show: the finder takes the full width,
        // so "No page matches …" and "Enter creates ¶ …" read in full.
        View::Pages if app.page_open.is_none() => app.rows.iter().any(|r| matches!(r, Row::Node { .. })).then_some(34),
        View::Journal => Some(total - 36),
        _ => Some(std),
    }
}

/// What a click on a drawn thing does (mouse.md §1: a click runs an action, the same as its key).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Click {
    View(View),
    Key(ratatui::crossterm::event::KeyCode, ratatui::crossterm::event::KeyModifiers),
    Day(NaiveDate),
    Row(usize),
    /// A row of an overlay's list (mouse.md "Overlays are menus"): hover selects it, a click
    /// selects it and acts as Enter.
    Menu(usize),
    /// An overlay's text field, starting at x0: a click places the caret there.
    Caret { x0: u16 },
    /// A row of the `[[` popup: a click inserts it.
    LinkRow(usize),
    /// Text in an overlay that does nothing when clicked (a heading, a body), declared so the
    /// sweep (overlays.rs) can tell it from a row someone forgot to make clickable.
    Text,
    /// A document line's meta: the ↗ open chip, a field (due / scheduled), a ≠ chip.
    Meta { line: usize, field: &'static str },
    /// The document's scrollbar, a row of it (0 = top).
    Scroll(u16),
    /// Drawn only so hover can light them; the document handles their clicks.
    Box,
    Link,
    /// A node named in the detail pane (a path segment, a blocker, a child, a backlink):
    /// open it where it lives.
    Node(String),
    /// A detail-pane field value: its prompt (`due`, `sched`, `priority`, `tags`, `status`).
    Field(&'static str),
    /// A list's scrollbar, a row of it (0 = top).
    ListScroll(u16),
    /// A list row's field (a Tasks column, a `due …` chip): select the row, then its prompt.
    RowField(usize, &'static str),
    /// A detail-pane history row: the node's full history, the cursor on that tx.
    HistoryTx(String),
    /// The task box of an `also today` / `linked from` row under a document: toggle it.
    FooterBox(String),
    /// A generated footer hint: run that action (keymap.rs), not a replayed key.
    Action(&'static str),
}

/// A clickable span on screen, recorded while drawing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub x0: u16,
    pub x1: u16,
    pub y: u16,
    pub what: Click,
}

/// Geometry and hit regions produced by one frame. The runtime retains this value for input.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenderOutput {
    pub click_targets: Vec<Target>,
    pub overlay_rect: Option<Rect>,
    pub doc_hits: Vec<crate::doc_ui::HitRow>,
    pub image_places: Vec<crate::images::Place>,
    pub doc_view_rows: usize,
    pub list_height: usize,
    pub doc_scrollbar: Option<(u16, u16, usize, u16, u16)>,
    pub list_scrollbar: Option<(u16, u16, usize, u16, u16)>,
    pub help_max_scroll: u16,
    /// About's section starts, search hits and scroll limit (about.rs).
    pub about: crate::about::Out,
    pub size: Rect,
    pub cells: Option<ratatui::buffer::Buffer>,
    hint_actions: Vec<(String, &'static str)>,
    drawing_row: Option<usize>,
}

impl RenderOutput {
    pub fn cells_at(&self, x: u16, y: u16, before: u16, after: u16) -> String {
        let Some(buf) = self.cells.as_ref() else { return String::new() };
        (x.saturating_sub(before)..=x.saturating_add(after))
            .filter(|cx| buf.area.contains((*cx, y).into()))
            .map(|cx| buf[(cx, y)].symbol().to_string()).collect()
    }
}

/// Record a clickable span [x0, x1) on row y.
pub(crate) fn target(render: &mut RenderOutput, x0: u16, x1: u16, y: u16, what: Click) {
    render.click_targets.push(Target { x0, x1, y, what });
}

pub(crate) fn inert(render: &mut RenderOutput, inner: Rect, y: u16) {
    target(render, inner.x, inner.right(), y, Click::Text);
}

fn set_overlay(render: &mut RenderOutput, r: Rect) {
    render.overlay_rect = Some(r);
}

pub(crate) fn set_overlay_rect(render: &mut RenderOutput, r: Rect) {
    set_overlay(render, r);
}

pub fn draw(f: &mut Frame, app: &App) -> RenderOutput {
    let mut output = RenderOutput { size: f.area(), ..RenderOutput::default() };
    let render = &mut output;
    draw_frame(render, f, app);
    // Hover (mouse.md §6): the target under the pointer turns accent; colour only, never layout.
    if let Some((hx, hy)) = app.hover {
        let accent = app.theme.s(Token::Accent).fg;
        {
            if let Some(tg) = render.click_targets.iter().rev().find(|t| t.y == hy && hx >= t.x0 && hx < t.x1 && !matches!(t.what, Click::Row(_) | Click::Scroll(_) | Click::ListScroll(_) | Click::Menu(_) | Click::RowField(..))) {
                let buf = f.buffer_mut();
                for x in tg.x0..tg.x1.min(buf.area.width) {
                    if tg.y >= buf.area.height {
                        break;
                    }
                    // 16 colours have no accent hue: underline instead.
                    match accent {
                        Some(c) => {
                            buf[(x, tg.y)].set_fg(c);
                        }
                        None => {
                            let st = buf[(x, tg.y)].style().add_modifier(Modifier::UNDERLINED);
                            buf[(x, tg.y)].set_style(st);
                        }
                    }
                }
            }
        }
    }
    // A link's title under the pointer underlines in accent: a click follows it (sidebar.md §7).
    if let (Some((hx, hy)), true) = (app.hover, app.overlay.is_none() && app.prompt.is_none()) {
        if let Some((line, byte, false)) = crate::doc_ui::hit_rows(app, &render.doc_hits, hx, hy) {
            let text = &app.doc.as_ref().unwrap().blocks()[line].text;
            if let Some(r) = crate::doc_app::link_title_range(text, byte) {
                let accent = app.theme.s(Token::Accent).fg;
                let buf = f.buffer_mut();
                // (Past a row's end every cell maps to the same byte: only a cell starting a new
                // byte is text.)
                let mut prev = None;
                for x in 0..buf.area.width {
                    let at = crate::doc_ui::hit_rows(app, &render.doc_hits, x, hy);
                    let fresh = at != prev;
                    prev = at;
                    if fresh && matches!(at, Some((l, b, false)) if l == line && r.contains(&b)) {
                        let mut st = buf[(x, hy)].style().add_modifier(Modifier::UNDERLINED);
                        if let Some(c) = accent {
                            st = st.underline_color(c);
                        }
                        buf[(x, hy)].set_style(st);
                    }
                }
            }
        }
    }
    render.cells = Some(f.buffer_mut().clone());
    output
}

fn draw_frame(render: &mut RenderOutput, f: &mut Frame, app: &App) {
    let area = f.area();
    let th = app.theme;
    if area.width < 60 || area.height < 24 {
        let msg = format!("thc needs at least 60×24 · now {}×{}", area.width, area.height);
        let y = area.height / 2;
        let x = area.width.saturating_sub(w(&msg) as u16) / 2;
        f.render_widget(Paragraph::new(Line::styled(msg, th.s(Token::Muted))), Rect { x, y, width: area.width - x, height: 1 });
        return;
    }
    // Focus: the document alone (its day strip stays), no tabs, no bar.
    // Overlays (help, palette, move) and prompts still show: focus hides chrome, not answers.
    // The tabs and the keys footer are Focus elements (tui-editor.md §8.1); the footer has
    // no fill there.
    if app.focus_mode && app.doc.is_some() {
        use thc_core::tui_config::El;
        let tabs = app.focus_cfg.has(El::Tabs) as u16;
        let bar = (app.focus_cfg.has(El::Footer) || app.prompt.is_some()) as u16;
        let [top, doc, bottom] = focus_areas(app, area);
        if tabs > 0 {
            draw_header(render, f, app, top);
        }
        draw_document(render, f, app, doc);
        if bar > 0 {
            draw_bar(render, f, app, bottom, true);
        }
        draw_overlays(render, f, app, area);
        return;
    }
    let banner = if app.conflicts.is_empty() { 0 } else { 1 };
    let drawer = if matches!(app.overlay, Some(Overlay::Capture { .. })) { 3 } else { 0 };
    let [header, rule, banner_r, content, drawer_r, bar] = normal_areas(app, area);
    let split = split_width(app, area.width);
    let tab = draw_header(render, f, app, header);
    draw_rule(f, app, rule, tab, split);
    if banner > 0 {
        draw_banner(f, app, banner_r);
    }
    match split {
        Some(lw) => {
            let list = Rect { width: lw, ..content };
            let sep = Rect { x: content.x + lw, width: 1, ..content };
            let detail = Rect { x: content.x + lw + 2, width: area.width.saturating_sub(lw + 2), ..content };
            let lines: Vec<Line> = (0..sep.height).map(|_| Line::styled(th.glyphs().vsep, th.s(Token::Line))).collect();
            f.render_widget(Paragraph::new(lines), sep);
            if app.doc.is_some() { draw_document(render, f, app, list) } else { draw_content(render, f, app, list) }
            draw_side(render, f, app, detail);
        }
        None if app.doc.is_some() => draw_document(render, f, app, content),
        None => draw_content(render, f, app, content),
    }
    if drawer > 0 {
        draw_drawer(render, f, app, drawer_r);
    }
    draw_bar(render, f, app, bar, false);
    draw_which_key(render, f, app, Rect { height: area.height.saturating_sub(1), ..area });
    draw_overlays(render, f, app, area);
}

/// Which-key (keymap.md §5.1): while a prefix is pending, a full-width panel above the footer
/// lists what can follow, groups ending in `+`. At most 8 rows, in columns; every entry is a
/// button for its key.
fn draw_which_key(render: &mut RenderOutput, f: &mut Frame, app: &App, above: Rect) {
    if app.overlay.is_some() || app.prompt.is_some() || !crate::keymap::popup_due_at(app, app.derived.now) {
        return;
    }
    let th = app.theme;
    let items = app.derived.data.bindings.continuations.clone();
    if items.is_empty() {
        return;
    }
    let cell = |h: &crate::keymap::Hint| {
        let group = h.actions.iter().all(|(_, a)| a.is_empty());
        (h.keys.clone(), if group { format!("+{}", h.label) } else { h.label.to_string() })
    };
    let cells: Vec<(String, String)> = items.iter().map(cell).collect();
    let col_w = cells.iter().map(|(k, l)| w(k) + 2 + w(l)).max().unwrap_or(10).max(14) + 3;
    let cols = (above.width as usize / col_w).max(1);
    let rows = cells.len().div_ceil(cols).min(8);
    let h = rows as u16 + 1;
    let r = Rect { x: above.x, y: above.bottom().saturating_sub(h), width: above.width, height: h };
    f.render_widget(Clear, r);
    let crumb = format!(" {} ", crate::keymap::display_seq(&app.pending_keys));
    let rule = format!("{}{crumb}{}", th.glyphs().rule, th.glyphs().rule.repeat((r.width as usize).saturating_sub(w(&crumb) + 1)));
    let mut lines = vec![Line::styled(rule, th.s(Token::Line))];
    for row in 0..rows {
        let mut spans = vec![Span::raw(" ")];
        let mut x = r.x + 1;
        for c in 0..cols {
            let Some((k, l)) = cells.get(c * rows + row) else { break };
            let y = r.y + 1 + row as u16;
            if let Some((code, mods)) = key_of(k) {
                target(render, x, x + (w(k) + 2 + w(l)) as u16, y, Click::Key(code, mods));
            }
            let used = w(k) + 2 + w(l);
            spans.push(Span::styled(k.clone(), th.s(Token::Accent)));
            spans.push(Span::raw("  "));
            spans.push(Span::styled(l.clone(), if l.starts_with('+') { th.s(Token::Text) } else { th.s(Token::Muted) }));
            spans.push(Span::raw(" ".repeat(col_w.saturating_sub(used))));
            x += col_w as u16;
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines).style(th.fill(Token::Surface)), r);
}

fn draw_overlays(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect) {
    if let Some(ov) = app.overlay.clone() {
        match ov {
            Overlay::Help { all, scroll } => draw_help(render, f, app, area, all, scroll),
            Overlay::Focus => draw_focus(render, f, app, area),
            Overlay::Palette { input, sel } => draw_palette(render, f, app, area, &input.buf, input.cur, sel),
            Overlay::Move { node, input, sel } => draw_move(render, f, app, area, &node, &input.buf, input.cur, sel),
            Overlay::Compare { detail } => draw_compare(render, f, app, area, &detail),
            Overlay::Finder { input, sel } => draw_finder(render, f, app, area, &input.buf, input.cur, sel),
            Overlay::Vaults { rows, sel, naming } => draw_vaults(render, f, app, area, &rows, sel, naming.as_ref()),
            Overlay::History { sel } => draw_history(render, f, app, area, sel),
            Overlay::Scope { sel, picked } => draw_scope(render, f, app, area, sel, &picked),
            Overlay::Recipe { name } => draw_recipe(render, f, app, area, &name),
            Overlay::About(a) => crate::about::draw(render, f, app, area, &a),
            Overlay::Capture { .. } => {}
        }
    }
}

// ---- frame ------------------------------------------------------------------------------------

/// Returns (label_start, label_len) of the active tab.
fn draw_header(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect) -> (u16, u16) {
    let th = app.theme;
    let g = th.glyphs();
    let wide = area.width >= 110;
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(g.brand_l, th.s(Token::Muted)),
        Span::styled(g.brand_dot, th.s(Token::Accent).add_modifier(Modifier::BOLD)),
        Span::styled(g.brand_r, th.s(Token::Muted)),
    ];
    // The vault's name in its accent, in place of `thc`, always (home too: vaults.md §8). In 16
    // colours there's no accent hue, so the name is a reversed chip. When the header doesn't
    // fit, in order: tab gaps 3 → 2, the clock goes (the date stays), the vault's own short
    // name (`[vault] name_short`), then the name middle-truncated to 12 (`thought…-lab`).
    let badge_count = app.vault.readonly.is_some() as usize + app.context_name().is_some() as usize;
    let badges_w = {
        let mut b: Vec<String> = Vec::new();
        if app.vault.readonly.is_some() {
            b.push("read-only".into());
        }
        if let Some(n) = app.context_name() {
            b.push(format!("@{n}"));
        }
        if b.is_empty() { 0 } else { w(&b.join(&format!(" {} ", g.sep))) + w(g.sep) + 2 }
    };
    let tabs_w = |gap: usize| -> usize {
        let mut t = 0;
        for (i, v) in VIEWS.iter().enumerate() {
            t += w(v.name()) + if i > 0 { gap } else { 0 };
            if *v == View::Inbox && app.inbox_count > 0 {
                t += w(&format!(" {}", app.inbox_count));
            }
            if *v == View::Log && app.to_review > 0 {
                t += w(&format!(" {}", app.to_review));
            }
        }
        t
    };
    let date_w = |clock: bool| w(&app.today.format("%a %b %-d").to_string()) + if clock { 3 + 5 } else { 0 };
    let chip = if th.is_ansi() { 2 } else { 0 };
    let short = app.derived.data.presentation.short_name.as_ref().map(|s| s.chars().take(12).collect::<String>());
    let full = app.vault_name.clone();
    let mid = |s: &str| middle_truncate(s, 12, g.ellipsis);
    let room = area.width as usize;
    let need = |name: &str, gap: usize, clock: bool| 5 + w(name) + chip + gap + tabs_w(gap) + 3 + badges_w + date_w(clock);
    let first_gap = if (badge_count > 0 && area.width < 100) || area.width < 90 { 2 } else { 3 };
    let mut tries: Vec<(String, usize, bool)> = vec![(full.clone(), first_gap, area.width >= 100), (full.clone(), 2, area.width >= 100), (full.clone(), 2, false)];
    if let Some(sh) = &short {
        tries.push((sh.clone(), 2, false));
    }
    tries.push((mid(short.as_deref().unwrap_or(&full)), 2, false));
    let (name, gap, clock) = tries.iter().find(|(n, gp, c)| need(n, *gp, *c) <= room).cloned().unwrap_or_else(|| tries.last().cloned().unwrap());
    spans.push(Span::raw(" "));
    if th.is_ansi() {
        spans.push(Span::styled(format!(" {name} "), Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)));
    } else {
        spans.push(Span::styled(name, th.s(Token::Accent)));
    }
    let used = width(&spans);
    let start = (used + gap).max(if wide { 11 } else { 7 });
    spans.push(Span::raw(" ".repeat(start - used)));
    let mut x = start as u16;
    let mut active = (start as u16, 5);
    for (i, v) in VIEWS.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" ".repeat(gap)));
            x += gap as u16;
        }
        let style = if *v == app.view { th.strong() } else { th.s(Token::Muted) };
        let label = v.name().to_string();
        let lw = w(&label) as u16;
        spans.push(Span::styled(label, style));
        let mut total = lw;
        if *v == View::Inbox && app.inbox_count > 0 {
            let c = format!(" {}", app.inbox_count);
            total += w(&c) as u16;
            spans.push(Span::styled(c, if *v == app.view { th.strong() } else { th.s(Token::Text) }));
        }
        // `Log 4`: agent changes waiting for review, the digit in `agent` (agents.md §1.6).
        if *v == View::Log && app.to_review > 0 {
            let c = format!(" {}", app.to_review);
            total += w(&c) as u16;
            spans.push(Span::styled(c, th.s(Token::Agent)));
        }
        if *v == app.view {
            active = (x, total);
        }
        target(render, area.x + x, area.x + x + total, area.y, Click::View(*v));
        x += total;
    }
    let mut date = app.today.format("%a %b %-d").to_string();
    if clock {
        date.push_str(&format!(" {} {}", g.sep, app.derived.clock));
    }
    // Before the date: `read-only · @work · Sat Oct 3` (policy.md §2.2, views.md §2.3).
    // Each badge is (muted prefix, text); neither is ever hidden.
    let mut badges: Vec<(&str, String)> = Vec::new();
    if app.vault.readonly.is_some() {
        badges.push(("", "read-only".to_string()));
    }
    if let Some(n) = app.context_name() {
        badges.push(("@", n.clone()));
    }
    let used = width(&spans);
    let room = area.width as usize;
    let join = |b: &[(&str, String)]| b.iter().map(|(p, t)| format!("{p}{t}")).collect::<Vec<_>>().join(&format!(" {} ", g.sep));
    let joined = join(&badges);
    let ctx_w = if badges.is_empty() { 0 } else { w(&joined) + w(g.sep) + 2 };
    // Narrow: the date goes first, the badges stay (`… Log 6   @work`). A filter is never hidden.
    let show_date = used + w(&date) + ctx_w + 3 <= room;
    // Without the date only the badges need room (no trailing separator).
    let show_ctx = !badges.is_empty() && (show_date || used + w(&joined) + 2 <= room);
    if show_date || show_ctx {
        let tail = if show_date { w(&date) + ctx_w } else { ctx_w - w(g.sep) - 2 };
        spans.push(Span::raw(" ".repeat(room - used - tail - 1)));
        if show_ctx {
            for (i, (p, t)) in badges.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::styled(format!(" {} ", g.sep), th.s(Token::Muted)));
                }
                if !p.is_empty() {
                    spans.push(Span::styled(p.to_string(), th.s(Token::Muted)));
                }
                spans.push(Span::styled(t.clone(), th.s(Token::Text)));
            }
            if show_date {
                spans.push(Span::styled(format!(" {} ", g.sep), th.s(Token::Muted)));
            }
        }
        if show_date {
            spans.push(Span::styled(date, th.s(Token::Muted)));
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    active
}

/// `thought…-lab`: the start and the distinctive end, `max` cells in all.
fn middle_truncate(s: &str, max: usize, ell: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(w(ell));
    let head = (keep * 3).div_ceil(5);
    let tail = keep - head;
    format!("{}{ell}{}", chars[..head].iter().collect::<String>(), chars[chars.len() - tail..].iter().collect::<String>())
}

fn draw_rule(f: &mut Frame, app: &App, area: Rect, (start, len): (u16, u16), split: Option<u16>) {
    let th = app.theme;
    let g = th.glyphs();
    let (h0, h1) = (start.saturating_sub(1), start + len);
    let mut spans = Vec::new();
    for col in 0..area.width {
        let (sym, st) = if col >= h0 && col <= h1 {
            (g.rule_heavy, th.s(Token::Accent))
        } else if Some(col) == split {
            (g.tee, th.s(Token::Line))
        } else {
            (g.rule, th.s(Token::Line))
        };
        spans.push(Span::styled(sym, st));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_banner(f: &mut Frame, app: &App, area: Rect) {
    let th = app.theme;
    let g = th.glyphs();
    // Lines moved here (their parent was deleted elsewhere) count apart: `≠ 3 conflicts · 1
    // moved here · c review` (daemon.md §4.0a).
    let moved = app.conflicts.iter().filter(|c| c.2 == "rehomed").count();
    let n = app.conflicts.len() - moved;
    let first = app.conflicts.iter().find(|c| c.2 != "rehomed").or(app.conflicts.first()).unwrap();
    let (_, node, field, _) = first;
    let short = app.derived.data.rows.short(&app.vault.paths.vault, node);
    let sentence = match field.as_str() {
        "parent" => format!("  {short} a move was skipped (it would loop) {} ", g.sep),
        "rehomed" => format!("  {short} its parent was deleted elsewhere {} ", g.sep),
        _ => format!("  {short} text was edited on two devices {} ", g.sep),
    };
    let base = th.s(Token::Conflict).remove_modifier(Modifier::BOLD);
    let mut head = Vec::new();
    if n > 0 {
        head.push(format!("{n} conflict{}", if n == 1 { "" } else { "s" }));
    }
    if moved > 0 {
        head.push(format!("{moved} moved here"));
    }
    let line = Line::from(vec![
        Span::styled(format!(" {} {}", g.conflict, head.join(&format!(" {} ", g.sep))), th.s(Token::Conflict)),
        Span::styled(sentence, base),
        // In Write `c` types: the way in is Esc, then c.
        Span::styled(if app.doc.is_some() && app.doc_write { "⌃O" } else { "c" }, th.s(Token::Conflict)),
        Span::styled(if n == 0 { " review" } else { " compare" }, base),
    ]);
    // ANSI: the whole row (padding too) is Magenta + BOLD + REVERSED; truecolor: tint background.
    let row_style = if th.is_ansi() { th.s(Token::ConflictTint) } else { th.fill(Token::ConflictTint) };
    f.render_widget(Paragraph::new(line).style(row_style), area);
}

/// THC_TUI_TRACE=1 (or 2): timings on screen.
fn trace(app: &App) -> bool {
    app.derived.trace
}

fn hints(th: &Theme, pairs: &[(&str, &str)]) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    for (i, (k, l)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push(Span::raw("  "));
        }
        out.push(Span::styled(k.to_string(), th.s(Token::Text)));
        out.push(Span::styled(format!(" {l}"), th.s(Token::Muted)));
    }
    out
}



/// Generated hints as `key label` spans. `actions`: their clicks run the action IDs (lists,
/// documents, toasts); overlays keep key clicks, since their own handlers read keys.
fn hint_spans(render: &mut RenderOutput, th: &Theme, items: &[crate::keymap::Hint], actions: bool) -> Vec<Span<'static>> {
    {
        let h = &mut render.hint_actions;
        h.clear();
        if actions {
            for it in items {
                h.push((it.keys.clone(), it.actions[0].1));
                for (k, a) in &it.actions {
                    h.push((k.clone(), a));
                }
            }
        }
    }
    let pairs: Vec<(&str, &str)> = items.iter().map(|i| (i.keys.as_str(), i.label.as_str())).collect();
    hints(th, &pairs)
}

/// Render a bar: left spans, right spans, background. The left side is truncated so at
/// least 2 spaces remain before the right side (§3.3).
fn bar_line(f: &mut Frame, area: Rect, th: &Theme, left: Vec<Span<'static>>, right: Vec<Span<'static>>, bg: Style, render: &mut RenderOutput) {
    let width_total = area.width as usize;
    // Every `key label` pair on the right is a button that runs the key (mouse.md §5, M7).
    {
        let mut x = area.x + width_total.saturating_sub(width(&right) + 1) as u16;
        let key_style = th.s(Token::Text);
        for (i, sp) in right.iter().enumerate() {
            let sw = w(&sp.content) as u16;
            let label = right.get(i + 1).filter(|n| n.content.starts_with(' ') && n.style != key_style).map(|n| w(&n.content) as u16);
            if sp.style == key_style && label.is_some() {
                let parts: Vec<&str> = sp.content.split(' ').collect();
                let mut px = x;
                for (k, part) in parts.iter().enumerate() {
                    let pw = w(part) as u16;
                    let end = if k + 1 == parts.len() { x + sw + label.unwrap_or(0) } else { px + pw };
                    let action = render.hint_actions.iter().find(|(k, _)| k == part || k.as_str() == sp.content.as_ref()).map(|(_, a)| *a);
                    if let Some(a) = action {
                        target(render, px, end, area.y, Click::Action(a));
                    } else if let Some((code, mods)) = key_of(part) {
                        target(render, px, end, area.y, Click::Key(code, mods));
                    }
                    px += pw + 1;
                }
            }
            x += sw;
        }
    }
    let max_left = width_total.saturating_sub(width(&right) + 3);
    let mut left = node_row::truncate_spans(&left, max_left, th.glyphs().ellipsis);
    let pad = width_total.saturating_sub(width(&left) + width(&right) + 1);
    left.push(Span::raw(" ".repeat(pad)));
    left.extend(right);
    left.push(Span::raw(" "));
    f.render_widget(Paragraph::new(Line::from(left)).style(bg), area);
    render.hint_actions.clear();
}

/// A footer key's text as the key it names (`⌃T`, `Esc`, `F1`, `x`), for its button.
fn key_of(k: &str) -> Option<(ratatui::crossterm::event::KeyCode, ratatui::crossterm::event::KeyModifiers)> {
    use ratatui::crossterm::event::{KeyCode as C, KeyModifiers as M};
    Some(match k {
        "Esc" => (C::Esc, M::NONE),
        "Enter" => (C::Enter, M::NONE),
        "Tab" => (C::Tab, M::NONE),
        "space" => (C::Char(' '), M::NONE),
        "F1" => (C::F(1), M::NONE),
        "↑" => (C::Up, M::NONE),
        "↓" => (C::Down, M::NONE),
        "↑↓" => (C::Down, M::NONE),
        "⇧Tab" => (C::BackTab, M::SHIFT),
        "⌫" => (C::Backspace, M::NONE),
        _ if k.starts_with('⌃') && k.chars().count() == 2 => (C::Char(k.chars().nth(1)?.to_ascii_lowercase()), M::CONTROL),
        _ if k.starts_with('⌥') && k.chars().count() == 2 => (C::Char(k.chars().nth(1)?.to_ascii_lowercase()), M::ALT),
        _ if k.chars().count() == 1 => (C::Char(k.chars().next()?), M::NONE),
        _ => return None,
    })
}

/// The bar. Whoever owns it (drawer, overlay, prompt, toast, error) shows only its own keys;
/// otherwise the view summary, daemon state and view hints (§3.3).
fn draw_bar(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect, plain: bool) {
    let th = app.theme;
    let g = th.glyphs();
    let width_total = area.width as usize;
    // `plain`: the keys footer in Focus, one muted row with no fill.
    let surface = if plain { Style::default() } else { th.fill(Token::Surface) };
    let lead = || vec![Span::raw(" ")];
    // The right side comes from the keymap (keymap.md §6): the contexts that own the bar now.
    let table_hints = |render: &mut RenderOutput, actions: bool| hint_spans(render, &th, &app.derived.data.bindings.footer, actions);

    // Tasks filter with a bad query.
    if let (Some((crate::app::PromptKind::Filter, _)), Some((msg, _, fix))) = (&app.prompt, &app.tasks_error) {
        let mut left = lead();
        left.push(Span::styled(msg.clone(), th.s(Token::Overdue)));
        let mut pairs = vec![("Esc", "clear")];
        if fix.is_some() {
            pairs.insert(0, ("Tab", "fix"));
        }
        return bar_line(f, area, &th, left, hints(&th, &pairs), th.fill(Token::OverdueTint), render);
    }
    // Bottom-bar prompts (inline ones are drawn in their own row).
    if let Some((kind, input)) = app.prompt.as_ref().filter(|(k, _)| !k.inline()) {
        let label = format!(" {} {} ", kind.label(), g.prompt);
        let left = vec![Span::styled(label.clone(), th.s(Token::Accent).add_modifier(Modifier::BOLD)), Span::styled(input.buf.clone(), th.s(Token::Text))];
        bar_line(f, area, &th, left, table_hints(render, false), th.fill(Token::AccentTint), render);
        let cx = area.x + w(&label) as u16 + w_chars(&input.buf, input.cur) as u16;
        f.set_cursor_position((cx.min(area.right().saturating_sub(1)), area.y));
        return;
    }
    // THC_NOW (testing only) shifts every date a write computes: say so, always (except in the
    // guide's renders, THC_TUI_RENDER=1, which pin the clock on purpose).
    if app.overlay.is_none() && !app.derived.render_mode {
        if let Some(w) = app.derived.pinned_warning.as_ref() {
            return bar_line(f, area, &th, vec![Span::raw(" "), Span::styled(w.clone(), th.s(Token::Overdue))], vec![], th.fill(Token::OverdueTint), render);
        }
    }
    // In-place editing owns the bar (tui-handoff §10.1).
    if app.edit.is_some() && app.overlay.is_none() {
        if let Some(t) = app.toast.as_ref().filter(|t| t.kind != crate::app::ToastKind::Error && t.alive_at(app.derived.now)) {
            let mut left = lead();
            left.extend(t.parts.iter().map(|(text, tok)| Span::styled(text.clone(), th.s(*tok))));
            return bar_line(f, area, &th, left, vec![], surface, render);
        }
        let mut left = lead();
        left.push(Span::styled("editing", th.s(Token::Text)));
        left.push(Span::styled(format!(" {} ", g.sep), th.s(Token::Muted)));
        left.extend(key_line(&th, &format!("Enter next line  {sep}  Tab nest  {sep}  Esc done", sep = g.sep)));
        return bar_line(f, area, &th, left, vec![], surface, render);
    }
    // Overlays and the capture drawer own the bar.
    match &app.overlay {
        Some(Overlay::Capture { input, .. }) => {
            let mut left = lead();
            // A refused write (veto, read-only) shows here while the drawer keeps the text
            // (policy.md §3.4: a veto never loses what a person typed).
            if let Some(t) = app.toast.as_ref().filter(|t| t.kind == crate::app::ToastKind::Error && t.alive_at(app.derived.now)) {
                left.extend(t.parts.iter().map(|(text, _)| Span::styled(text.clone(), th.s(Token::Overdue))));
                return bar_line(f, area, &th, left, hints(&th, &[("Enter", "retry"), ("Esc", "")]), th.fill(Token::OverdueTint), render);
            }
            match app.derived.data.input.lenient(&input.buf) {
                // Nothing typed yet: no complaint, just the hints.
                _ if input.buf.trim().is_empty() => {}
                Ok((c, bad)) if !bad.is_empty() => {
                    left.push(Span::styled(format!("Enter saves \"{}\" as plain text", bad.join(" ")), th.s(Token::Muted)));
                    let _ = c;
                }
                Ok((c, _)) => {
                    // Chips show each token as typed → what it became.
                    let mut chips: Vec<String> = Vec::new();
                    let toks: Vec<&str> = input.buf.split_whitespace().collect();
                    let find = |prefixes: &[&str]| -> Option<String> {
                        toks.iter().find(|t| prefixes.iter().any(|p| t.to_lowercase().starts_with(p))).map(|t| t.to_string())
                    };
                    if c.status.is_some() {
                        let typed = find(&["[ ]", "[x]", "todo"]).unwrap_or_else(|| "[ ]".into());
                        chips.push(format!("{typed}{}{}", g.arrow, c.status.clone().unwrap_or_default()));
                    }
                    if let Some(d) = c.scheduled {
                        chips.push(format!("{}{}{}", find(&["sched:", "scheduled:", "start:", "at:", "on:"]).unwrap_or("sched".into()), g.arrow, short_date(&d)));
                    }
                    if let Some(d) = c.due {
                        chips.push(format!("{}{}{}", find(&["due:", "deadline:"]).unwrap_or("due".into()), g.arrow, short_date(&d)));
                    }
                    if let Some(r) = &c.repeat {
                        chips.push(format!("{}{}{} {}", find(&["every", "repeat:"]).unwrap_or("every".into()), g.arrow, g.repeat, r.text.trim_start_matches("every ")));
                    }
                    left.push(Span::styled(chips.join("  "), th.s(Token::Muted)));
                }
                Err(e) => left.push(Span::styled(e, th.s(Token::Overdue))),
            }
            return bar_line(f, area, &th, left, table_hints(render, false), surface, render);
        }
        Some(Overlay::About(_)) => {
            let mut left = lead();
            left.push(Span::styled("about", th.s(Token::Text)));
            let right = hints(&th, &[("1 2 3", "sections"), ("/", "search"), ("n N", "next"), ("e", "config"), ("Esc", "close")]);
            return bar_line(f, area, &th, left, right, surface, render);
        }
        Some(Overlay::Focus) => {
            let mut left = lead();
            left.push(Span::styled("focus", th.s(Token::Text)));
            // The element letters aren't one key: they lead, then the table's keys.
            let mut right = hints(&th, &[("letters", "show · hide")]);
            right.push(Span::raw("  "));
            right.extend(table_hints(render, false));
            return bar_line(f, area, &th, left, right, surface, render);
        }
        Some(Overlay::Help { .. }) => {
            let mut left = lead();
            left.push(Span::styled("help", th.s(Token::Text)));
            return bar_line(f, area, &th, left, table_hints(render, false), surface, render);
        }
        Some(Overlay::Palette { .. }) => {
            let mut left = lead();
            left.push(Span::styled("command", th.s(Token::Text)));
            return bar_line(f, area, &th, left, table_hints(render, false), surface, render);
        }
        Some(Overlay::Finder { .. }) => {
            let mut left = lead();
            left.push(Span::styled("open", th.s(Token::Text)));
            return bar_line(f, area, &th, left, table_hints(render, false), surface, render);
        }
        // The overlay's border carries the keys; the bar only names it.
        Some(Overlay::Vaults { .. }) => {
            let mut left = lead();
            left.push(Span::styled("vaults", th.s(Token::Text)));
            return bar_line(f, area, &th, left, vec![], surface, render);
        }
        Some(Overlay::History { .. }) => {
            let mut left = lead();
            left.push(Span::styled("history", th.s(Token::Text)));
            return bar_line(f, area, &th, left, vec![], surface, render);
        }
        Some(Overlay::Recipe { .. }) => {
            let mut left = lead();
            left.push(Span::styled("how it's built", th.s(Token::Text)));
            return bar_line(f, area, &th, left, vec![], surface, render);
        }
        Some(Overlay::Scope { .. }) => {
            let mut left = lead();
            left.push(Span::styled("scope", th.s(Token::Text)));
            return bar_line(f, area, &th, left, vec![], surface, render);
        }
        Some(Overlay::Move { .. }) => {
            let mut left = lead();
            left.push(Span::styled("move", th.s(Token::Text)));
            return bar_line(f, area, &th, left, table_hints(render, false), surface, render);
        }
        // In a document the overlay carries the keys; the bar only names it.
        Some(Overlay::Compare { detail }) if app.doc.is_some() && detail.kind == "text" => {
            let left = vec![Span::raw(" "), Span::styled(format!("{} compare {} {}", g.conflict, g.sep, app.derived.data.rows.short(&app.vault.paths.vault, &detail.node)), th.s(Token::Conflict))];
            return bar_line(f, area, &th, left, vec![], th.fill(Token::ConflictTint), render);
        }
        Some(Overlay::Compare { .. }) => {
            let mut left = lead();
            left.push(Span::styled(format!("{} compare", g.conflict), th.s(Token::Conflict)));
            return bar_line(f, area, &th, left, table_hints(render, false), th.fill(Token::ConflictTint), render);
        }
        None => {}
    }
    // The update applying owns the whole bar; a failure says why (tui-editor.md §11).
    if app.reexec {
        return bar_line(f, area, &th, vec![Span::raw(" "), Span::styled("updating… · back in a moment", th.s(Token::Text))], vec![], surface, render);
    }
    if let crate::app::UpdateState::Failed(why) = &app.update_state {
        if app.toast.as_ref().is_none_or(|t| !t.alive_at(app.derived.now)) {
            let left = vec![Span::raw(" "), Span::styled("update ", th.s(Token::Text)), Span::styled("failed", th.s(Token::Overdue)), Span::styled(format!(": {why} · :update to retry"), th.s(Token::Muted))];
            return bar_line(f, area, &th, left, vec![], th.fill(Token::OverdueTint), render);
        }
    }
    // A pending prefix owns the bar (keymap.md §6): `p…` and what can follow. Its hints click as
    // keys, finishing the sequence.
    if let Some((crumb, mut next)) = app.derived.data.bindings.prefix.clone() {
        let left = vec![Span::raw(" "), Span::styled(crumb, th.s(Token::Accent))];
        // With the which-key popup open, its keys aren't repeated here: just how to
        // step back or leave.
        if crate::keymap::popup_due_at(app, app.derived.now) && app.overlay.is_none() {
            next.clear();
        }
        // Groups show as words (`f find`); the fitting rule drops from the end.
        loop {
            let mut right = hint_spans(render, &th, &next, false);
            right.push(Span::raw("  "));
            if crate::keymap::display_seq(&app.pending_keys).starts_with("space") {
                right.extend(hints(&th, &[("⌫", "back"), ("Esc", "cancel")]));
            } else {
                right.extend(hints(&th, &[("Esc", "cancel")]));
            }
            if next.is_empty() || width(&left) + width(&right) + 3 <= width_total {
                return bar_line(f, area, &th, left, right, surface, render);
            }
            next.pop();
        }
    }
    // Toasts own the bar while alive.
    if let Some(t) = app.toast.as_ref().filter(|t| t.alive_at(app.derived.now)) {
        let mut left = lead();
        for (txt, tok) in &t.parts {
            left.push(Span::styled(txt.clone(), th.s(*tok)));
        }
        let (right, bg): (Vec<Span<'static>>, Style) = match t.kind {
            ToastKind::Confirm | ToastKind::Alert => (table_hints(render, true), surface),
            ToastKind::Agent => (table_hints(render, true), th.fill(Token::AgentTint)),
            ToastKind::Error => (hints(&th, &[("Esc", "")]), th.fill(Token::OverdueTint)),
            ToastKind::Info => (vec![], surface),
            ToastKind::Notice => (vec![], th.fill(Token::OverdueTint)),
        };
        return bar_line(f, area, &th, left, right, bg, render);
    }

    // In a document, the keys footer (tui-editor.md §4.5): where you are, that saving is
    // automatic, and the few keys you need now. `?` leads with the same keys and words.
    if let (Some(d), true) = (app.doc.as_ref(), app.overlay.is_none()) {
        let what = match &d.target {
            crate::editor::Target::Journal { date } => format!("§ {}", date.format("%a %d %b").to_string().to_lowercase()),
            crate::editor::Target::Page { title, .. } => format!("{} {title}", g.page),
        };
        let failed = d.blocks().iter().any(|l| l.save_error.is_some());
        let late = d.blocks().iter().any(|l| l.saving_since.is_some_and(|t| crate::editor::ms(app.derived.now).saturating_sub(t) >= 3000));
        // (text, token, all is well): `autosaved` is steady; only a problem changes it.
        // The very first journal, still blank: `just type`.
        let blank = d.blocks().iter().all(|l| l.text.trim().is_empty());
        let save: (String, Token, bool) = if app.doc_first_ever && blank && app.doc_write {
            ("just type".into(), Token::Muted, false)
        } else if failed {
            ("not saved · :retry".into(), Token::Overdue, false)
        } else if late {
            ("◌ saving…".into(), Token::Muted, false)
        } else {
            ("autosaved".into(), Token::Muted, true)
        };
        // writing.md §4: `§ sun 04 oct · 412 words · autosaved   ⌃T task  ⌃O open  ⌃P ⌃N day  Esc done
        // F1 keys`, fitted by measure: first `F1 keys` goes, then the document's name. A page has
        // no day keys. While the `[[` popup is open it shows the popup's own keys.
        let journal = matches!(d.target, crate::editor::Target::Journal { .. });
        let sep = || Span::styled(format!(" {} ", g.sep), th.s(Token::Muted));
        let words = crate::doc_ui::word_count(app);
        let (text, tok, ok) = save.clone();
        // The table's `write` ranks (or the link popup's): `⌃T task  ⌃O open  ⌃P ⌃N day  Esc done
        // F1 keys`, the day keys only in a journal.
        let _ = journal;
        let mut keys = app.derived.data.bindings.footer.clone();
        let done_only: Vec<crate::keymap::Hint> = keys.iter().filter(|h| h.label == "done").cloned().collect();
        for level in [3u8, 2, 1, 0] {
            if level < 3 && !app.link_open && keys.last().is_some_and(|k| k.label == "keys") {
                keys.pop();
            }
            let mut left = lead();
            if level >= 2 {
                left.push(Span::styled(what.clone(), th.s(Token::Text)));
                left.push(sep());
            }
            if level >= 1 {
                left.push(Span::styled(format!("{words} word{}", if words == 1 { "" } else { "s" }), th.s(Token::Muted)));
                left.push(sep());
            }
            if level >= 1 || !ok {
                left.push(Span::styled(text.clone(), th.s(tok)));
            }
            let mut right = hint_spans(render, &th, if level == 0 && !app.link_open { &done_only } else { &keys }, true);
            if app.focus_mode && app.focus_cfg.has(thc_core::tui_config::El::Clock) {
                right.push(Span::raw("   "));
                right.push(Span::styled(app.derived.clock.clone(), th.s(Token::Muted)));
            }
            if level == 0 || width(&left) + width(&right) + 3 <= width_total {
                // The version (or a newer thc installed under this window), when it fits.
                let mut version = None;
                match app.installed.as_deref() {
                    Some(v) => {
                        let notice = vec![Span::raw("   "), Span::styled(format!("thc {v} installed"), th.s(Token::Accent)), Span::styled(format!(" {} :update reloads here", g.sep), th.s(Token::Muted))];
                        if width(&left) + width(&right) + width(&notice) + 3 <= width_total {
                            right.extend(notice);
                        }
                    }
                    None => version = version_slot(app, &th, &left, &mut right, &[], width_total),
                }
                bar_line(f, area, &th, left, right, surface, render);
                if let Some(vw) = version {
                    version_target(render, area, vw);
                }
                return;
            }
        }
    }
    // The view owns the bar: summary · daemon state · hints.
    let n_nodes = app.rows.iter().filter(|r| matches!(r, Row::Node { .. })).count();
    let mut left = lead();
    let summary: Vec<(String, Token)> = match app.view {
        View::Today if app.agenda_mode => vec![("agenda".into(), Token::Text), (format!(" {} 7 days {} {n_nodes} items", g.sep, g.sep), Token::Muted)],
        View::Today => {
            let mut v = vec![("today".to_string(), Token::Text)];
            // Across vaults: `today · 3 vaults · 13 open · 2 inbox` (vaults.md §3.5).
            let (oo, oi, od) = app.others_counts;
            if !app.others.is_empty() {
                v.push((format!(" {} {} vaults", g.sep, app.others.len() + 1), Token::Muted));
            }
            match app.context_name() {
                Some(n) => {
                    let shown = n_nodes;
                    v.push((format!(" {} @{n}", g.sep), Token::Text));
                    v.push((format!(" {} {shown} of {} shown", g.sep, shown + app.context_hidden), Token::Muted));
                }
                None => v.push((format!(" {} {} open", g.sep, app.open_count + oo), Token::Muted)),
            }
            if app.overdue_count + od > 0 {
                v.push((format!(" {} ", g.sep), Token::Muted));
                v.push((format!("{} overdue", app.overdue_count + od), Token::Overdue));
            }
            if app.inbox_count + oi > 0 {
                v.push((format!(" {} {} inbox", g.sep, app.inbox_count + oi), Token::Muted));
            }
            if width_total >= SPLIT_AT as usize {
                let done = app.rows.iter().filter(|r| matches!(r, Row::Node { node, .. } if node.status.as_deref() == Some("done"))).count();
                if done > 0 {
                    v.push((format!(" {} {done} done", g.sep), Token::Muted));
                }
            }
            v
        }
        View::Inbox => vec![("inbox".into(), Token::Text), (format!(" {} {n_nodes} to triage", g.sep), Token::Muted)],
        // Every list bar starts with the context (views.md §2.3).
        View::Tasks => match app.context_name() {
            Some(n) => vec![
                ("tasks".into(), Token::Text),
                (format!(" {} @{n}", g.sep), Token::Text),
                (format!(" {} {n_nodes} of {} shown {} {}", g.sep, n_nodes + app.context_hidden, g.sep, app.tasks_filter), Token::Muted),
            ],
            None => vec![("tasks".into(), Token::Text), (format!(" {} {}", g.sep, app.tasks_filter), Token::Muted)],
        },
        View::Pages => match &app.page_open {
            Some(p) => vec![(format!("{} {}", g.page, node_label(app, p)), Token::Text), (format!(" {} outline", g.sep), Token::Muted)],
            None if width_total >= SPLIT_AT as usize && app.selected_node().is_some() => {
                let label = app.selected_node().map(|n| n.label()).unwrap_or_default();
                vec![(format!("{} {label}", g.page), Token::Text), (format!(" {} preview", g.sep), Token::Muted)]
            }
            None => vec![("pages".into(), Token::Text), (format!(" {} {n_nodes}", g.sep), Token::Muted)],
        },
        View::Journal => {
            let own = app.rows.iter().filter(|r| matches!(r, Row::Node { outline: true, depth: 0, .. })).count();
            // The journal is a record of the day; contexts don't filter it, and the bar says so.
            match app.context_name() {
                Some(n) => vec![("journal".into(), Token::Text), (format!(" {} @{n} not applied here", g.sep), Token::Muted)],
                None => vec![("journal".into(), Token::Text), (format!(" {} {own} entr{}", g.sep, if own == 1 { "y" } else { "ies" }), Token::Muted)],
            }
        }
        View::Search => vec![("search".into(), Token::Text), (format!(" {} full text + titles", g.sep), Token::Muted)],
        // The review lane (agents.md §1): what's waiting, by whom.
        View::Log if app.review_lane && app.log_node.is_none() => {
            let mut by: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
            for r in &app.rows {
                if let Row::Tx { entries, .. } = r {
                    if let Some(e) = entries.first() {
                        *by.entry(actor_word(&e.actor)).or_default() += 1;
                    }
                }
            }
            let mut v = vec![("to review".to_string(), Token::Text)];
            if by.is_empty() {
                v.push((format!(" {} nothing waiting", g.sep), Token::Muted));
            }
            for (who, n) in by {
                v.push((format!(" {} {n} by {who}", g.sep), Token::Muted));
            }
            v
        }
        View::Log => match (&app.log_node, &app.log_actor) {
            (Some(n), _) => vec![("log".into(), Token::Text), (format!(" {} {}", g.sep, node_label(app, n)), Token::Muted)],
            (None, Some(a)) => vec![("log".into(), Token::Text), (format!(" {} {}", g.sep, actor_word(a)), Token::Muted)],
            (None, None) => vec![("log".into(), Token::Text), (format!(" {} undo makes a new tx", g.sep), Token::Muted)],
        },
    };
    left.extend(summary.into_iter().map(|(t, tok)| Span::styled(t, th.s(tok))));
    if !app.conflicts.is_empty() {
        left.push(Span::styled(format!(" {} ", g.sep), th.s(Token::Muted)));
        left.push(Span::styled(format!("{} conflict{}", app.conflicts.len(), if app.conflicts.len() == 1 { "" } else { "s" }), th.s(Token::Conflict)));
    }
    let mut hint_list = app.derived.data.bindings.footer.clone();
    // The compare over a document: yours and theirs, not current and other (tui-editor.md §9).
    if let (Some(crate::app::Overlay::Compare { .. }), true) = (&app.overlay, app.doc.is_some()) {
        let who = app.derived.data.detail.theirs_name.clone();
        for h in hint_list.iter_mut() {
            match h.label.as_str() {
                "keep current" => h.label = "keep yours".into(),
                "keep other" => h.label = format!("keep {who}'s"),
                _ => {}
            }
        }
    }
    // A parked document (navigation.md §6.1) leads with how to start and how to move on.
    if app.doc.is_some() && app.doc_parked && app.overlay.is_none() && app.prompt.is_none() {
        hint_list.insert(0, crate::keymap::Hint { keys: "Tab".into(), label: "next view".into(), actions: vec![] });
        hint_list.insert(0, crate::keymap::Hint { keys: "type".into(), label: "to write".into(), actions: vec![] });
    }
    // An update: downloading, or available (lists and Navigate only; never while writing).
    let update: Vec<Span<'static>> = match (&app.update_state, &app.update_available) {
        (crate::app::UpdateState::Downloading { version }, _) => vec![Span::styled(format!("updating to {version} {} downloading   ", g.sep), th.s(Token::Muted))],
        // A newer thc was installed under this window (thc update elsewhere).
        (crate::app::UpdateState::Idle, _) if app.installed.is_some() => vec![
            Span::styled(format!("thc {} installed", app.installed.as_deref().unwrap_or("")), th.s(Token::Accent)),
            Span::styled(format!(" {} :update reloads here   ", g.sep), th.s(Token::Muted)),
        ],
        (crate::app::UpdateState::Idle, Some(v)) => vec![Span::styled("update", th.s(Token::Accent)), Span::styled(format!(" {v} {} :update   ", g.sep), th.s(Token::Muted))],
        _ => vec![],
    };
    // Offline (§7.5): `○ local · daemon offline   :daemon start` replaces the view hints.
    if !app.daemon_live {
        let right = vec![
            Span::styled(g.daemon_local, th.s(Token::Muted)),
            Span::styled(format!(" local {} daemon offline   ", g.sep), th.s(Token::Muted)),
            Span::styled(":daemon start", th.s(Token::Text)),
        ];
        // Narrow: `○ offline   :daemon start` before the left counts get cut.
        let short = vec![Span::styled(g.daemon_local, th.s(Token::Muted)), Span::styled(" offline   ", th.s(Token::Muted)), Span::styled(":daemon start", th.s(Token::Text))];
        // An update to apply goes first: the long offline words give way to it.
        let mut right = if width(&left) + width(&right) + width(&update) + 3 > width_total { short } else { right };
        if width(&left) + width(&right) + width(&update) + 3 <= width_total {
            let mut r = update.clone();
            r.extend(right);
            right = r;
        }
        let version = version_slot(app, &th, &left, &mut right, &update, width_total);
        bar_line(f, area, &th, left, right, surface, render);
        if let Some(vw) = version {
            version_target(render, area, vw);
        }
        return;
    }
    // Drop hints from the end until the right side fits, keeping the daemon dot.
    let right = loop {
        let mut right = update.clone();
        right.extend([Span::styled(g.daemon_live, th.s(Token::Done)), Span::styled(" live", th.s(Token::Muted))]);
        if !hint_list.is_empty() {
            right.push(Span::raw("   "));
            right.extend(hint_spans(render, &th, &hint_list, true));
        }
        if width(&left) + width(&right) + 3 <= width_total || hint_list.is_empty() {
            break right;
        }
        hint_list.pop();
    };
    let mut right = right;
    let version = version_slot(app, &th, &left, &mut right, &update, width_total);
    bar_line(f, area, &th, left, right, surface, render);
    // The version opens About (about.md §3): a click on `thc 0.9.55` at the far right.
    if let Some(vw) = version {
        version_target(render, area, vw);
    }
}

/// The footer's version (keymap.md §6): `thc 0.9.46` in dim at the far right,
/// three spaces after the last hint, only when it fits (the first thing dropped). While a newer
/// thc is installed under this window, the update notice stands in for it. `THC_TUI_VERSION`
/// pins it in snapshots, so goldens don't change with every release.
/// The footer version opens About (about.md §3). bar_line ends the bar with a space.
fn version_target(render: &mut RenderOutput, area: Rect, vw: u16) {
    let x1 = area.right().saturating_sub(1);
    target(render, x1.saturating_sub(vw), x1, area.y, Click::Action("about"));
}

/// Until About is opened on a new version it reads `thc 0.9.55 · new` in accent (about.md §3).
/// The width it took, when it fits (the caller makes it a click target).
#[must_use]
fn version_slot(app: &App, th: &Theme, left: &[Span<'static>], right: &mut Vec<Span<'static>>, update: &[Span<'static>], total: usize) -> Option<u16> {
    if !update.is_empty() {
        return None;
    }
    let v = &app.derived.version;
    let slot = if app.about_new { Span::styled(format!("   thc {v} · new"), th.s(Token::Accent)) } else { Span::styled(format!("   thc {v}"), th.s(Token::Muted).add_modifier(Modifier::DIM)) };
    let sw = width(std::slice::from_ref(&slot));
    if width(left) + width(right) + sw + 3 <= total {
        right.push(slot);
        return Some(sw.saturating_sub(3) as u16);
    }
    None
}

fn short_date(d: &DateVal) -> String {
    let t = d.time().map(|t| format!(" {}", t.format("%H:%M"))).unwrap_or_default();
    format!("{}{t}", d.date().format("%a %b %-d"))
}

// ---- content ----------------------------------------------------------------------------------

/// Fixed rows a view draws above its list (input rows, headers, day strip).
fn top_rows(app: &App, wd: usize) -> (Vec<Line<'static>>, Option<(u16, u16)>) {
    let th = app.theme;
    let g = th.glyphs();
    let mut cursor = None;
    let inline_prompt = app.prompt.as_ref().filter(|(k, _)| k.inline());
    let right_align = |left: Vec<Span<'static>>, right: Vec<Span<'static>>| -> Line<'static> {
        let pad = wd.saturating_sub(width(&left) + width(&right) + 1);
        let mut v = left;
        v.push(Span::raw(" ".repeat(pad)));
        v.extend(right);
        Line::from(v)
    };
    let lines = match app.view {
        View::Tasks => {
            let (text, focused) = match inline_prompt {
                Some((crate::app::PromptKind::Filter, input)) => {
                    cursor = Some((4 + w_chars(&input.buf, input.cur) as u16, 0));
                    (input.buf.clone(), true)
                }
                _ => (app.tasks_filter.clone(), false),
            };
            let mut left = vec![Span::raw(" "), Span::styled(format!("q{} ", g.prompt), th.s(Token::Accent).add_modifier(Modifier::BOLD))];
            let bad = app.tasks_error.as_ref().map(|(_, b, _)| b.clone()).unwrap_or_default();
            left.extend(query_spans(&th, &text).into_iter().map(|sp| {
                if !bad.is_empty() && (bad.ends_with(sp.content.as_ref()) && !sp.content.trim().is_empty()) {
                    Span::styled(sp.content, th.s(Token::Overdue).add_modifier(Modifier::UNDERLINED))
                } else {
                    sp
                }
            }));
            // `q› @work  status:open #work sort:due`: a view's expansion, muted beside its name.
            if let Some(v) = text.trim().strip_prefix('@').and_then(|n| app.saved_views.iter().find(|v| v.name == n)) {
                left.push(Span::styled(format!("  {}", v.query), th.s(Token::Muted)));
            }
            let n = app.rows.iter().filter(|r| matches!(r, Row::Node { .. })).count();
            // Timings are for THC_TUI_TRACE=1; people see the count.
            let ms = if trace(app) { format!(" {} {:.1} ms", g.sep, app.tasks_ms) } else { String::new() };
            let right = vec![Span::styled(format!("{n} task{}{ms}", if n == 1 { "" } else { "s" }), th.s(Token::Muted))];
            let mut l = right_align(left, right);
            if focused {
                l = l.patch_style(th.fill(Token::AccentTint));
            }
            // Row 3 says what the filter means while you type it (views.md §3.1).
            let meaning = match (focused, &app.derived.data.input.filter_meaning) {
                (true, Some(m)) => {
                    let m = m.clone();
                    let room = wd.saturating_sub(5);
                    let m = if m.chars().count() > room { format!("{}…", m.chars().take(room.saturating_sub(1)).collect::<String>()) } else { m };
                    Line::from(vec![Span::raw("    "), Span::styled(m, th.s(Token::Muted))])
                }
                _ => Line::raw(""),
            };
            vec![l, meaning]
        }
        View::Pages if app.page_open.is_none() => {
            let (text, focused) = match inline_prompt {
                Some((crate::app::PromptKind::PagesFilter, input)) => {
                    cursor = Some((3 + w_chars(&input.buf, input.cur) as u16, 0));
                    (input.buf.clone(), true)
                }
                _ => (app.pages_filter.clone(), false),
            };
            let body = if text.is_empty() {
                Span::styled("type to find a page, tag or day", th.s(Token::Muted))
            } else {
                Span::styled(text, th.s(Token::Text))
            };
            let mut l = Line::from(vec![Span::raw(" "), Span::styled(format!("{} ", g.prompt), th.s(Token::Accent).add_modifier(Modifier::BOLD)), body]);
            if focused || !app.pages_filter.is_empty() {
                l = right_align(l.spans, vec![]).patch_style(th.fill(Token::AccentTint));
            }
            vec![l, Line::raw("")]
        }
        View::Pages => {
            let pid = app.page_open.clone().unwrap_or_default();
            let title = node_label(app, &pid);
            let open = app.derived.data.rows.node(&app.vault.paths.vault, &pid).map_or(0, |n| n.open);
            let back = app.derived.data.rows.node(&app.vault.paths.vault, &pid).map_or(0, |n| n.backlinks);
            let left = vec![Span::raw(" "), Span::styled(format!("{} {title}", g.page), th.strong())];
            let mut bits = vec![app.derived.data.rows.short(&app.vault.paths.vault, &pid)];
            if open > 0 {
                bits.push(format!("{open} open"));
            }
            if back > 0 {
                bits.push(format!("{} {back}", g.backlink));
            }
            let right = vec![Span::styled(bits.join(&format!(" {} ", g.sep)), th.s(Token::Muted))];
            vec![right_align(left, right), Line::raw("")]
        }
        View::Journal => {
            let d = app.journal_date;
            let mut left = vec![Span::raw(" "), Span::styled(format!("{} {}", g.journal, d.format("%A, %B %-d %Y")), th.strong())];
            let rel = dates::relative(d, app.today);
            left.push(Span::styled(format!(" {} {rel}", g.sep), if d == app.today { th.s(Token::Today) } else { th.s(Token::Muted) }));
            let right = hints(&th, &[("[", "prev"), ("]", "next"), ("T", "today")]);
            let header = right_align(left, right);
            // Day strip: 7 days centered on the shown day.
            let mut strip: Vec<Span> = vec![Span::raw("   ")];
            let mut under = (0usize, 0usize);
            for i in -3..=3 {
                let day = d + Duration::days(i);
                let has = app.derived.data.detail.calendar.days.contains(&day);
                if i > -3 {
                    strip.push(Span::raw("   "));
                }
                let label = format!("{} {}", day.format("%a"), day.day());
                let start = width(&strip);
                let st = if i == 0 { th.strong() } else { th.s(Token::Muted) };
                strip.push(Span::styled(label.clone(), st));
                let mut lw = w(&label);
                if has {
                    strip.push(Span::styled(format!(" {}", g.live), if i == 0 { th.s(Token::Accent) } else { th.s(Token::Muted) }));
                    lw += 1 + w(g.live);
                }
                if i == 0 {
                    under = (start.saturating_sub(1), lw + 2);
                }
            }
            let underline = Line::from(vec![Span::raw(" ".repeat(under.0)), Span::styled(g.rule_heavy.repeat(under.1), th.s(Token::Accent))]);
            vec![header, Line::from(strip), underline, Line::raw("")]
        }
        View::Search => {
            let (text, focused) = match inline_prompt {
                Some((crate::app::PromptKind::Search, input)) => {
                    cursor = Some((3 + w_chars(&input.buf, input.cur) as u16, 0));
                    (input.buf.clone(), true)
                }
                _ => (app.search_terms.clone(), false),
            };
            let left = vec![Span::raw(" "), Span::styled("/ ", th.s(Token::Accent).add_modifier(Modifier::BOLD)), Span::styled(text, th.s(Token::Text))];
            let nodes = app.rows.iter().filter(|r| matches!(r, Row::Node { node, .. } if node.parent.is_some() || node.title.is_none())).count();
            let pages = app.rows.iter().filter(|r| matches!(r, Row::Node { node, .. } if node.parent.is_none() && node.title.is_some())).count();
            let tags = app.rows.iter().filter(|r| matches!(r, Row::Tag { .. })).count();
            let pl = |n: usize, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
            let right = vec![Span::styled(
                {
                    let ms = if trace(app) { format!(" {} {:.0} ms", g.sep, app.search_ms.max(1.0)) } else { String::new() };
                    format!("{} {} {} {} {}{ms}", pl(nodes, "node"), g.sep, pl(pages, "page"), g.sep, pl(tags, "tag"))
                },
                th.s(Token::Muted),
            )];
            let mut l = right_align(left, right);
            if focused {
                l = l.patch_style(th.fill(Token::AccentTint));
            }
            vec![l, Line::raw("")]
        }
        View::Log if app.review_lane && app.log_node.is_none() => {
            let n = app.rows.iter().filter(|r| matches!(r, Row::Tx { .. })).count();
            let mut left = vec![Span::raw(" "), Span::styled("Log", th.strong()), Span::styled(format!(" {} to review  ", g.sep), th.s(Token::Muted))];
            left.push(Span::styled(n.to_string(), th.s(Token::Agent)));
            if let Some(a) = &app.log_actor {
                left.push(Span::styled(format!(" {} {}", g.sep, actor_word(a)), th.s(Token::Muted)));
            }
            let mut right = vec![Span::styled("oldest first   ", th.s(Token::Muted))];
            right.extend(hints(&th, &[("r", "all changes")]));
            vec![right_align(left, right), Line::raw("")]
        }
        View::Log => {
            let txs = app.rows.iter().filter(|r| matches!(r, Row::Tx { .. })).count();
            let mut agents: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
            for r in &app.rows {
                if let Row::Tx { entries, .. } = r {
                    if let Some(e) = entries.first().filter(|e| e.actor.starts_with("agent")) {
                        *agents.entry(e.actor.trim_start_matches("agent:").to_string()).or_default() += 1;
                    }
                }
            }
            let mut summary = format!(" {} {txs} tx", g.sep);
            for (a, n) in &agents {
                summary.push_str(&format!(" {} {n} by {a}", g.sep));
            }
            let mut left = vec![Span::raw(" "), Span::styled("Log", th.strong()), Span::styled(summary, th.s(Token::Muted))];
            if let Some(n) = &app.log_node {
                left.push(Span::styled(format!(" {} {}", g.sep, truncate_str(&node_label(app, n), 30, g.ellipsis)), th.s(Token::Text)));
            }
            let who = match &app.log_actor {
                Some(a) => actor_word(a),
                None => "everyone".into(),
            };
            let right = if app.log_node.is_some() {
                hints(&th, &[("Esc", "everyone")])
            } else {
                let mut v = vec![Span::styled(format!("last 24h {} {who}   ", g.sep), th.s(Token::Muted))];
                if app.to_review > 0 {
                    v.extend(hints(&th, &[("r", "to review"), ("@", "actor")]));
                } else {
                    v.extend(hints(&th, &[("@", "actor")]));
                }
                v
            };
            vec![right_align(left, right), Line::raw("")]
        }
        // Today across vaults (view-explain.md §2): its scope, and how it's built.
        View::Today if !app.agenda_mode && !app.others.is_empty() => {
            let now = &app.derived.data.presentation.scope_current;
            let default = &app.derived.data.presentation.scope_default;
            let mut scope_style = th.s(Token::Text);
            if now != default {
                scope_style = scope_style.add_modifier(Modifier::UNDERLINED);
            }
            let mut left = vec![Span::raw(" "), Span::styled("Today", th.strong()), Span::styled(format!(" {} ", g.sep), th.s(Token::Muted)), Span::styled(app.derived.data.presentation.scope_current_text.clone(), scope_style)];
            if now != default {
                left.push(Span::styled(format!(" {} default {}", g.sep, app.derived.data.presentation.scope_default_text.clone()), th.s(Token::Muted).add_modifier(Modifier::DIM)));
            }
            let right = vec![Span::styled("*", th.s(Token::Accent)), Span::styled(" scope   ", th.s(Token::Muted)), Span::styled("?", th.s(Token::Accent)), Span::styled(" how it's built", th.s(Token::Muted))];
            vec![right_align(left, right)]
        }
        _ => vec![],
    };
    (lines, cursor)
}

/// Today's header buttons (view-explain.md §2): the scope and `* scope` open the picker,
/// `? how it's built` the recipe. Placed from the line as drawn.
fn today_header_targets(render: &mut RenderOutput, app: &App, area: Rect, line: &Line) {
    if app.view != View::Today || app.agenda_mode || app.others.is_empty() {
        return;
    }
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    let col = |needle: &str| text.find(needle).map(|b| area.x + w(&text[..b]) as u16);
    let star = ratatui::crossterm::event::KeyCode::Char('*');
    let none = ratatui::crossterm::event::KeyModifiers::NONE;
    let scope = app.derived.data.presentation.scope_current_text.clone();
    if let Some(x) = col(&scope) {
        target(render, x, x + w(&scope) as u16, area.y, Click::Key(star, none));
    }
    if let Some(x) = col("* scope") {
        target(render, x, x + 7, area.y, Click::Key(star, none));
    }
    if let Some(x) = col("? how it's built") {
        target(render, x, x + 16, area.y, Click::Key(ratatui::crossterm::event::KeyCode::Char('?'), none));
    }
}

fn w_chars(s: &str, n: usize) -> usize {
    w(&s.chars().take(n).collect::<String>())
}

/// `status:open #work` with keys muted and values in text.
fn query_spans(th: &Theme, q: &str) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    for (i, tok) in q.split(' ').enumerate() {
        if i > 0 {
            out.push(Span::raw(" "));
        }
        match tok.find([':', '<', '>', '=']) {
            Some(p) if p > 0 => {
                let end = tok[p..].find(|c: char| !matches!(c, ':' | '<' | '>' | '=')).map(|e| p + e).unwrap_or(tok.len());
                out.push(Span::styled(tok[..end].to_string(), th.s(Token::Muted)));
                out.push(Span::styled(tok[end..].to_string(), th.s(Token::Text)));
            }
            _ => out.push(Span::styled(tok.to_string(), if tok.starts_with('#') { th.s(Token::Tag) } else { th.s(Token::Text) })),
        }
    }
    out
}

/// Capture syntax as typed, each token underlined in its colour (bad ones in `overdue`):
/// the capture drawer and the in-place editing row (tui-handoff §10.2).
fn token_spans(app: &App, buf: &str, bad: &[String]) -> Vec<Span<'static>> {
    let th = app.theme;
    let mut out: Vec<Span<'static>> = Vec::new();
    // Quoted text is never parsed (writing.md §1): no token colours inside quotes.
    let quoted = thc_core::capture::quoted_ranges(buf);
    // A token a later one of the same field overrides is a plain word (writing.md §1).
    let shadowed = app.derived.data.input.shadowed(buf);
    let mut at = 0usize;
    for (i, tok) in buf.split(' ').enumerate() {
        if i > 0 {
            out.push(Span::styled(" ", th.s(Token::Text)));
            at += 1;
        }
        let start = at;
        at += tok.len();
        let inside = quoted.iter().any(|(a, b)| start > *a && start < *b) || (tok.starts_with(['"', '`']) && !tok.contains(':')) || shadowed.iter().any(|(a, _)| *a == start);
        let lower = tok.to_lowercase();
        let u = Modifier::UNDERLINED;
        let st = if inside {
            th.s(Token::Text)
        } else if bad.iter().any(|b| b == tok) {
            th.s(Token::Overdue).add_modifier(u)
        } else if ["due:", "deadline:", "sched:", "scheduled:", "start:", "at:", "on:"].iter().any(|p| lower.starts_with(p)) {
            // Date colours as in row meta (§4.1): past overdue, today today, later muted.
            let value = tok.split_once(':').map(|(_, v)| v.trim_matches('"')).unwrap_or("");
            match app.derived.data.input.date(buf, value) {
                Some(d) if d < app.today => th.s(Token::Overdue).add_modifier(u),
                Some(d) if d == app.today => th.s(Token::Today).add_modifier(u),
                _ => th.s(Token::Muted).add_modifier(u),
            }
        } else if lower.starts_with("every") || lower.starts_with("repeat:") {
            th.s(Token::Muted).add_modifier(u)
        } else if thc_core::capture::tag_of(tok).is_some() {
            th.s(Token::Tag).add_modifier(u)
        } else if tok.starts_with('!') && thc_core::capture::normalize_priority(&tok[1..]).is_some() {
            th.strong().add_modifier(u)
        } else if tok.starts_with("[[") || tok.ends_with("]]") {
            th.s(Token::Link)
        } else {
            th.s(Token::Text)
        };
        out.push(Span::styled(tok.to_string(), st));
    }
    out
}

/// What the edited line's tokens will set, for the meta area (tui-handoff §10.2):
/// `due Tue Oct 6 ✓`, or `can't read "fryday"` in `overdue`, or a refusal from the last save.
/// Only what the tokens change shows (an existing task with `due:fri` reads `due Fri Oct 9 ✓`,
/// not `task · …`), and a line with no tokens shows nothing at all.
fn edit_chips(app: &App, buf: &str, error: Option<&str>, node: Option<&Node>) -> Vec<Span<'static>> {
    let th = app.theme;
    if let Some(e) = error {
        return vec![Span::styled(e.to_string(), th.s(Token::Overdue))];
    }
    if buf.trim().is_empty() {
        return vec![];
    }
    match app.derived.data.input.lenient(buf) {
        Ok((_, bad)) if !bad.is_empty() => {
            let t = &bad[0];
            let v = t.split_once(':').map(|(_, v)| v.trim_matches('"')).unwrap_or(t);
            vec![Span::styled(format!("can't read \"{v}\""), th.s(Token::Overdue))]
        }
        Ok((c, _)) => {
            if c.status.is_none() && c.scheduled.is_none() && c.due.is_none() && c.priority.is_none() && c.repeat.is_none() {
                return vec![];
            }
            let day = |d: &thc_core::dates::DateVal| {
                let t = d.time().map(|t| t.format(" %H:%M").to_string()).unwrap_or_default();
                format!("{}{t}", d.date().format("%a %b %-d"))
            };
            let mut bits: Vec<String> = Vec::new();
            if c.status.is_some() && node.is_none_or(|n| n.status.is_none()) {
                bits.push("task".into());
            }
            if let Some(d) = c.scheduled.as_ref().filter(|d| node.is_none_or(|n| n.scheduled.as_deref() != Some(d.fmt().as_str()))) {
                bits.push(format!("sched {}", day(d)));
            }
            if let Some(d) = c.due.as_ref().filter(|d| node.is_none_or(|n| n.due.as_deref() != Some(d.fmt().as_str()))) {
                bits.push(format!("due {}", day(d)));
            }
            if let Some(p) = c.priority.as_ref().filter(|p| node.is_none_or(|n| n.priority.as_deref() != Some(p.as_str()))) {
                bits.push(format!("!{p}"));
            }
            if let Some(r) = c.repeat.as_ref().filter(|r| node.is_none_or(|n| n.repeat.as_ref().and_then(|x| x.get("rule")).and_then(|x| x.as_str()) != Some(r.rule.as_str()))) {
                bits.push(format!("{} {}", th.glyphs().repeat, r.text));
            }
            let mut v = Vec::new();
            if !bits.is_empty() {
                v.push(Span::styled(format!("{} ", bits.join(&format!(" {} ", th.glyphs().sep))), th.s(Token::Muted)));
            }
            v.push(Span::styled("✓", th.s(Token::Muted)));
            v
        }
        Err(e) => vec![Span::styled(e, th.s(Token::Overdue))],
    }
}

/// Spans cut to display columns [from, from + len).
fn slice_cols(spans: &[Span<'static>], from: usize, len: usize) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut col = 0;
    for s in spans {
        let mut buf = String::new();
        for ch in s.content.chars() {
            let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if col >= from && col + cw <= from + len {
                buf.push(ch);
            }
            col += cw;
        }
        if !buf.is_empty() {
            out.push(Span::styled(buf, s.style));
        }
    }
    out
}

/// The row being edited in place (tui-handoff §10.2): its gutter, indent, id and status,
/// then the input on `accent-tint` to the pane edge, with the chips in the meta area.
/// Returns the line and the caret's column.
fn edit_row_line(app: &App, row: &Row, wd: usize) -> (Line<'static>, u16) {
    let th = app.theme;
    let g = th.glyphs();
    let Some(e) = &app.edit else { return (Line::raw(""), 0) };
    let (id, status, depth, fold) = match row {
        Row::Node { node, depth, has_children, collapsed, .. } => (Some(data_for_row(app, row).map(|data| data.short.clone()).unwrap_or_else(|| node.id.chars().take(5).collect())), status_span(&th, node), *depth, has_children.then_some(*collapsed)),
        _ => (Some(String::new()), Span::styled(format!(" {} ", g.note), th.s(Token::Muted)), e.depth, None),
    };
    let hidden = !app.outline_ids();
    let id = if hidden { None } else { id };
    let spec = RowSpec { gutter: Gutter::Cursor { focused: true }, selected: false, id, indent: depth * 2, fold, status, text: vec![], meta: Meta { variants: vec![] }, fold_col: hidden };
    let left = node_row::prefix(&th, &spec);
    let used = width(&left);
    let editing_node = match row {
        Row::Node { node, .. } => Some(node),
        _ => None,
    };
    let chips = edit_chips(app, &e.input.buf, e.error.as_deref(), editing_node);
    let cw = width(&chips);
    // ⌃J line breaks show as ↵ in the one-line input.
    let shown: String = e.input.buf.replace('\n', "↵");
    let bad = app.derived.data.input.lenient(&e.input.buf).map(|(_, b)| b).unwrap_or_default();
    let text = token_spans(app, &shown, &bad);
    // The input gets the full text width, up to a 2-cell gap before the chips.
    let avail = wd.saturating_sub(used + cw + if cw > 0 { 3 } else { 1 }).max(8);
    let caret_col: usize = shown.chars().take(e.input.cur).map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)).sum();
    // Long text scrolls so the caret stays visible, with `…` at the left edge when it does;
    // rows never wrap while editing.
    let from = (caret_col + 1).saturating_sub(avail);
    let (visible, from) = if from > 0 {
        let from = from + 1;
        let mut v = vec![Span::styled(g.ellipsis.to_string(), th.s(Token::Muted))];
        v.extend(slice_cols(&text, from, avail - 1));
        (v, from - 1)
    } else {
        (slice_cols(&text, 0, avail), 0)
    };
    let mut line: Vec<Span<'static>> = left;
    let mut input: Vec<Span<'static>> = visible;
    let iw = width(&input);
    input.push(Span::raw(" ".repeat(wd.saturating_sub(used + iw + cw + 1))));
    input.extend(chips);
    input.push(Span::raw(" "));
    let tint = th.fill(Token::AccentTint);
    line.extend(input.into_iter().map(|s| s.patch_style(tint)));
    (Line::from(line), (used + caret_col - from) as u16)
}

/// Word-wrap styled spans to `width` cells (long words are cut).
fn wrap_spans(spans: &[Span<'static>], width: usize) -> Vec<Vec<Span<'static>>> {
    let width = width.max(8);
    // Words with their style, keeping the spaces that follow them.
    let mut words: Vec<(String, ratatui::style::Style)> = Vec::new();
    for s in spans {
        let mut cur = String::new();
        for ch in s.content.chars() {
            cur.push(ch);
            if ch == ' ' {
                words.push((std::mem::take(&mut cur), s.style));
            }
        }
        if !cur.is_empty() {
            words.push((cur, s.style));
        }
    }
    let mut lines: Vec<Vec<Span<'static>>> = vec![vec![]];
    let mut col = 0;
    for (w, st) in words {
        let ww = unicode_width::UnicodeWidthStr::width(w.trim_end());
        if col > 0 && col + ww > width {
            lines.push(vec![]);
            col = 0;
        }
        let mut w = w;
        while unicode_width::UnicodeWidthStr::width(w.as_str()) > width && col == 0 {
            let cut: String = w.chars().take(width).collect();
            w = w.chars().skip(width).collect();
            lines.last_mut().unwrap().push(Span::styled(cut, st));
            lines.push(vec![]);
        }
        col += unicode_width::UnicodeWidthStr::width(w.as_str());
        lines.last_mut().unwrap().push(Span::styled(w, st));
    }
    lines
}

/// The cells before a paragraph's text: the bullets' text column, with no `·` (§10.8).
fn prose_prefix(app: &App, row: &Row, selected: bool, focused: bool) -> Vec<Span<'static>> {
    let th = app.theme;
    let Row::Node { node, .. } = row else { return vec![] };
    let gutter = gutter_for(app, &node.id, selected, focused);
    let spec = RowSpec {
        gutter,
        selected: false,
        id: if app.outline_ids() { Some(data_for_row(app, row).map(|data| data.short.clone()).unwrap_or_else(|| node.id.chars().take(5).collect())) } else { None },
        indent: 0,
        fold: None,
        status: Span::raw("   "),
        text: vec![],
        meta: Meta { variants: vec![] },
        fold_col: !app.outline_ids(),
    };
    node_row::prefix(&th, &spec)
}

/// A top-level note on a page or day as a paragraph: wrapped to the pane, continuation
/// lines at the text column, ⌃J breaks kept (§10.8).
fn prose_lines(app: &App, row: &Row, selected: bool, focused: bool, wd: usize) -> Vec<Line<'static>> {
    let th = app.theme;
    let Row::Node { node, .. } = row else { return vec![] };
    let left = prose_prefix(app, row, selected, focused);
    let pw = width(&left);
    let avail = wd.saturating_sub(pw + 1);
    let text = data_for_row(app, row).map(|data| data.text.as_str()).unwrap_or(&node.text);
    let mut wrapped: Vec<Vec<Span<'static>>> = Vec::new();
    for para in text.split('\n') {
        wrapped.extend(wrap_spans(&text_spans(&th, para, th.s(Token::Text), ""), avail));
    }
    if let Some(a) = node.created_by.strip_prefix("agent:") {
        let tag = format!("  {}{}{a}", th.glyphs().agent, th.glyphs().agent_sep);
        let last = wrapped.last_mut().unwrap();
        if width(last) + w(&tag) <= avail {
            last.push(Span::raw(" ".repeat(avail - width(last) - w(&tag) + 2)));
            last.push(Span::styled(tag.trim_start().to_string(), th.s(Token::Agent)));
        }
    }
    wrapped
        .into_iter()
        .enumerate()
        .map(|(i, l)| {
            let mut spans = if i == 0 { left.clone() } else { vec![Span::raw(" ".repeat(pw))] };
            spans.extend(l);
            node_row::finish(&th, spans, selected && focused, wd)
        })
        .collect()
}

/// Editing a paragraph wraps too (§10.8): the input grows downward up to 8 rows, then scrolls
/// inside itself. Returns the lines and the caret (column, line offset).
fn edit_prose_lines(app: &App, row: &Row, wd: usize) -> (Vec<Line<'static>>, (u16, u16)) {
    let th = app.theme;
    let Some(e) = &app.edit else { return (vec![], (0, 0)) };
    let left = match row {
        Row::Node { .. } => prose_prefix(app, row, true, true),
        _ => {
            let mut v = vec![Span::styled(th.glyphs().cursor.to_string(), th.s(Token::Accent).add_modifier(Modifier::BOLD))];
            v.push(Span::raw(" ".repeat(7)));
            v
        }
    };
    let pw = width(&left);
    let avail = wd.saturating_sub(pw + 1).max(8);
    // Lay the text out with each line's starting character index, so the caret's line is the
    // last one starting at or before it.
    let chars: Vec<char> = e.input.buf.chars().collect();
    let mut layout: Vec<(usize, Vec<char>)> = vec![(0, vec![])];
    let mut last_space: Option<usize> = None;
    for (i, ch) in chars.iter().enumerate() {
        if *ch == '\n' {
            layout.push((i + 1, vec![]));
            last_space = None;
            continue;
        }
        let full = {
            let cur = &layout.last().unwrap().1;
            cur.iter().map(|c| unicode_width::UnicodeWidthChar::width(*c).unwrap_or(0)).sum::<usize>() >= avail
        };
        if full {
            let (start, cur) = layout.last_mut().unwrap();
            let start = *start;
            match last_space {
                // Carry the word being typed down to the next line.
                Some(ls) if ls < cur.len() => {
                    let carry = cur.split_off(ls);
                    layout.push((start + ls, carry));
                }
                _ => layout.push((i, vec![])),
            }
            last_space = None;
        }
        let cur = &mut layout.last_mut().unwrap().1;
        cur.push(*ch);
        if *ch == ' ' {
            last_space = Some(cur.len());
        }
    }
    let li = layout.iter().rposition(|(start, _)| *start <= e.input.cur).unwrap_or(0);
    let caret = (li, (e.input.cur - layout[li].0).min(layout[li].1.len()));
    let lines: Vec<String> = layout.into_iter().map(|(_, l)| l.into_iter().collect()).collect();
    // At most 8 rows, scrolled so the caret's line shows.
    let first = (caret.0 + 1).saturating_sub(8);
    let tint = th.fill(Token::AccentTint);
    let bad = app.derived.data.input.lenient(&e.input.buf).map(|(_, b)| b).unwrap_or_default();
    let out: Vec<Line<'static>> = lines
        .iter()
        .enumerate()
        .skip(first)
        .take(8)
        .map(|(i, l)| {
            let mut spans = if i == first { left.clone() } else { vec![Span::raw(" ".repeat(pw))] };
            let mut input = token_spans(app, l, &bad);
            let iw = width(&input);
            input.push(Span::raw(" ".repeat(wd.saturating_sub(pw + iw))));
            spans.extend(input.into_iter().map(|s| s.patch_style(tint)));
            Line::from(spans)
        })
        .collect();
    let col: usize = lines[caret.0].chars().take(caret.1).map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)).sum();
    (out, ((pw + col) as u16, (caret.0 - first) as u16))
}

/// Whether a row is edited as a paragraph: a prose row, or a new top-level line on a page or day.
fn edits_as_prose(app: &App, row: &Row) -> bool {
    match row {
        Row::Editing => app.edit.as_ref().is_some_and(|e| e.para && e.depth == 0) && (app.view == View::Journal || (app.view == View::Pages && app.page_open.is_some())),
        r => app.is_prose(r),
    }
}

/// How many lines a row takes (paragraphs wrap; the empty state is two).
fn row_height(app: &App, row: &Row, wd: usize, editing: bool) -> usize {
    match row {
        Row::Empty { .. } => 2,
        r if editing && edits_as_prose(app, r) => edit_prose_lines(app, r, wd).0.len().max(1),
        r if app.is_prose(r) => prose_lines(app, r, false, false, wd).len().max(1),
        _ => 1,
    }
}

fn draw_content(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect) {
    let Some(prepared) = app.derived.list.as_ref() else { return };
    if prepared.area != area || prepared.revision != list_revision(app) { return }
    let wd = area.width as usize;
    let top = prepared.top.clone();
    let cursor = prepared.cursor;
    let top_h = top.len() as u16;
    if let Some(l) = top.first() {
        today_header_targets(render, app, area, l);
    }
    if top_h > 0 {
        f.render_widget(Paragraph::new(top), Rect { height: top_h.min(area.height), ..area });
        if let Some((cx, cy)) = cursor {
            if caret_allowed(app) {
                f.set_cursor_position((area.x + cx.min(area.width.saturating_sub(1)), area.y + cy));
            }
        }
    }
    let list = Rect { y: area.y + top_h, height: area.height.saturating_sub(top_h), ..area };
    let table = app.view == View::Tasks && f.area().width >= SPLIT_AT;
    if table {
        f.render_widget(Paragraph::new(table_header(app)), Rect { height: 1, ..list });
    }
    let list = prepared.list;
    let h = list.height as usize;
    render.list_height = h;
    // Tasks: when the list overflows, the saved-filters row stays pinned at the bottom, so the
    // rows scroll in one line less (or the cursor's row hides behind it).
    let pinned = prepared.pinned;
    let h = prepared.height;
    let is_edit_row = |app: &App, r: &Row| {
        app.edit.as_ref().is_some_and(|e| match r {
            Row::Editing => e.node.is_none(),
            Row::Node { node, .. } => e.node.as_deref() == Some(node.id.as_str()),
            _ => false,
        })
    };
    let mut lines = Vec::with_capacity(h);
    let mut edit_cursor: Option<(u16, u16)> = None;
    if top_h == 0 && matches!(app.rows.first(), Some(Row::Empty { .. })) {
        lines.push(Line::raw(""));
        lines.push(Line::raw(""));
    }
    let focused = app.focus == Focus::List;
    let selected_tx = match app.rows.get(app.cursor) {
        Some(Row::Tx { tx, .. }) => Some(tx.clone()),
        _ => None,
    };
    let end = pinned.unwrap_or(app.rows.len());
    for i in app.scroll..end {
        if lines.len() >= h {
            break;
        }
        let selected = i == app.cursor;
        // A row is clickable (select; double-click opens; its box toggles): mouse.md §5. Every
        // line it draws is its target, and a Log detail line is its transaction's (a click
        // under a row's first line once selected some other row).
        let owner = match &app.rows[i] {
            Row::TxDetail { tx, .. } => (0..i).rev().find(|&j| matches!(&app.rows[j], Row::Tx { tx: t, .. } if t == tx)).unwrap_or(i),
            _ => i,
        };
        let first = lines.len();
        if !matches!(app.rows[i], Row::Blank) {
            target(render, area.x, area.right(), list.y + first as u16, Click::Row(owner));
        }
        match &app.rows[i] {
            // The first-run welcome: `key<TAB>what` lines, a centred block.
            Row::Empty { l1, l2 } if l2.contains('\t') => {
                let th = &app.theme;
                let rows: Vec<(String, String)> = l2.lines().map(|l| l.split_once('\t').map(|(k, v)| (k.to_string(), v.to_string())).unwrap_or_default()).collect();
                let kw = rows.iter().map(|(k, _)| w(k)).max().unwrap_or(0);
                let block = rows.iter().map(|(_, v)| kw + 3 + w(v)).max().unwrap_or(0).max(w(l1));
                let pad = " ".repeat(wd.saturating_sub(block) / 2);
                lines.push(Line::raw(""));
                lines.push(Line::from(vec![Span::raw(pad.clone()), Span::styled(l1.clone(), th.strong())]));
                for (k, v) in rows {
                    lines.push(Line::from(vec![Span::raw(pad.clone()), Span::styled(format!("{k:<kw$}   "), th.s(Token::Text)), Span::styled(v, th.s(Token::Muted))]));
                }
            }
            Row::Empty { l1, l2 } => {
                lines.push(Line::from(vec![Span::raw("   "), Span::styled(l1.clone(), app.theme.s(Token::Text))]));
                lines.push(Line::from(vec![Span::raw("   ")].into_iter().chain(key_line(&app.theme, l2)).collect::<Vec<_>>()));
            }
            Row::TxDetail { tx, text } => {
                let sel = selected_tx.as_deref() == Some(tx.as_str());
                let s = format!("{}{}", " ".repeat(28), truncate_str(text, wd.saturating_sub(30), app.theme.glyphs().ellipsis));
                lines.push(node_row::finish(&app.theme, vec![Span::styled(s, app.theme.s(Token::Muted))], sel && focused, wd));
            }
            row if is_edit_row(app, row) && edits_as_prose(app, row) => {
                let (pl, (cx, cy)) = edit_prose_lines(app, row, wd);
                edit_cursor = Some((cx, lines.len() as u16 + cy));
                lines.extend(pl);
            }
            row if app.is_prose(row) && !is_edit_row(app, row) => {
                lines.extend(prose_lines(app, row, selected, focused, wd));
            }
            row if is_edit_row(app, row) => {
                let (line, cx) = edit_row_line(app, row, wd);
                edit_cursor = Some((cx, lines.len() as u16));
                lines.push(line);
            }
            row => {
                render.drawing_row = Some(i);
                let line = if table { table_row(render, app, row, selected, focused, wd) } else { row_line(render, app, row, selected, focused, wd) };
                render.drawing_row = None;
                let y = list.y + lines.len() as u16;
                if table && matches!(row, Row::Node { .. }) {
                    for (c, field) in [(2, "due"), (3, "sched"), (4, "priority")] {
                        target(render, area.x + COLS[c] as u16, area.x + COLS[c + 1] as u16 - 1, y, Click::RowField(i, field));
                    }
                } else if !table && matches!(row, Row::Node { .. }) {
                    // The meta chips: `due fri` and `!high` open their prompts (mouse.md §5).
                    let mut x = area.x;
                    for sp in &line.spans {
                        let sw = w(&sp.content) as u16;
                        let t = sp.content.trim();
                        let field = if t.starts_with("due ") { Some("due") } else if t.starts_with('!') && t.len() > 1 && t[1..].chars().all(|c| c.is_ascii_alphabetic()) { Some("priority") } else { None };
                        if let Some(field) = field {
                            target(render, x, x + sw, y, Click::RowField(i, field));
                        }
                        x += sw;
                    }
                }
                // A bad query keeps the last good results, muted (§5.3).
                let muted = app.view == View::Tasks && app.tasks_error.is_some();
                lines.push(if muted { mute_line(&app.theme, line) } else { line });
            }
        }
        if !matches!(app.rows[i], Row::Blank) {
            for y in first + 1..lines.len().min(h) {
                target(render, area.x, area.right(), list.y + y as u16, Click::Row(owner));
            }
        }
    }
    if let Some(p) = pinned {
        while lines.len() < h {
            lines.push(Line::raw(""));
        }
        let row = &app.rows[p];
        render.drawing_row = Some(p);
        lines.push(if table { table_row(render, app, row, false, focused, wd) } else { row_line(render, app, row, false, focused, wd) });
        render.drawing_row = None;
    }
    f.render_widget(Paragraph::new(lines), list);
    list_scrollbar(render, f, app, Rect { height: h as u16, ..list }, h);
    if let Some((cx, cy)) = edit_cursor {
        if caret_allowed(app) {
            f.set_cursor_position((list.x + cx.min(list.width.saturating_sub(1)), list.y + cy));
        }
    }
}

/// A list's scrollbar (mouse.md §5): the right column, by rows, only when the list overflows and
/// at 60 columns or more. Its track pages; its thumb drags.
fn list_scrollbar(render: &mut RenderOutput, f: &mut Frame, app: &App, list: Rect, h: usize) {
    let total = app.rows.len();
    render.list_scrollbar = None;
    if total <= h || list.width < 60 || h == 0 {
        return;
    }
    let th = &app.theme;
    let x = list.right() - 1;
    let thumb_h = (h * h / total).max(1);
    let thumb_y = (app.scroll * h / total).min(h - thumb_h);
    let buf = f.buffer_mut();
    for i in 0..h {
        let y = list.y + i as u16;
        let on = i >= thumb_y && i < thumb_y + thumb_h;
        buf[(x, y)].set_symbol(if on { "┃" } else { "│" }).set_fg(th.s(if on { Token::Muted } else { Token::Line }).fg.unwrap_or_default());
        target(render, x, x + 1, y, Click::ListScroll(i as u16));
    }
    render.list_scrollbar = Some((list.y, h as u16, total, thumb_y as u16, thumb_h as u16));
}



/// Every span in `muted` (colours stripped; DIM in ANSI), keeping backgrounds and the cursor.
fn mute_line(th: &Theme, line: Line<'static>) -> Line<'static> {
    let m = th.s(Token::Muted);
    let spans = line
        .spans
        .into_iter()
        .map(|sp| {
            let mut st = Style { fg: m.fg.or(Some(ratatui::style::Color::Reset)), ..sp.style };
            st = st.add_modifier(m.add_modifier);
            Span::styled(sp.content, st)
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

/// "a capture  ·  w agenda": single-letter keys in text, the rest muted.
fn key_line(th: &Theme, s: &str) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    for (i, part) in s.split("  ·  ").enumerate() {
        if i > 0 {
            out.push(Span::styled("  ·  ", th.s(Token::Muted)));
        }
        match part.split_once(' ') {
            Some((k, rest)) if k.chars().count() <= 2 && !k.is_empty() => {
                out.push(Span::styled(k.to_string(), th.s(Token::Text)));
                out.push(Span::styled(format!(" {rest}"), th.s(Token::Muted)));
            }
            _ => out.push(Span::styled(part.to_string(), th.s(Token::Muted))),
        }
    }
    out
}

fn status_span(th: &Theme, n: &Node) -> Span<'static> {
    let g = th.glyphs();
    match n.status.as_deref() {
        Some("todo") => Span::styled("[ ]", th.s(Token::Text)),
        Some("doing") => Span::styled("[/]", th.s(Token::Doing)),
        Some("waiting") => Span::styled("[w]", th.s(Token::Waiting)),
        Some("done") => Span::styled("[x]", th.s(Token::Done)),
        Some("cancelled") => Span::styled("[-]", th.s(Token::Muted)),
        _ if n.parent.is_none() && n.title.is_some() => Span::styled(format!(" {} ", g.page), th.s(Token::Muted)),
        _ if n.journal.is_some() => Span::styled(format!(" {} ", g.journal), th.s(Token::Muted)),
        _ => Span::styled(format!(" {} ", g.note), th.s(Token::Muted)),
    }
}

/// Node text: `#tag` in tag color, `[[` `]]` muted around a link-styled title.
pub fn text_spans(th: &Theme, text: &str, base: Style, highlight: &str) -> Vec<Span<'static>> {
    let closed = base == th.s(Token::Muted) || base.add_modifier.contains(Modifier::CROSSED_OUT);
    if closed {
        // Done and cancelled rows: everything muted, tags and links included.
        return vec![Span::styled(text.to_string(), base)];
    }
    // Quoted text is never parsed (writing.md §1): quoted runs are plain text.
    let quoted = thc_core::capture::quoted_ranges(text);
    if !quoted.is_empty() {
        let mut out = Vec::new();
        let mut from = 0;
        for (a, b) in quoted {
            if a > from {
                out.extend(text_spans(th, &text[from..a], base, ""));
            }
            out.push(Span::styled(text[a..b].to_string(), base));
            from = b;
        }
        if from < text.len() {
            out.extend(text_spans(th, &text[from..], base, ""));
        }
        return if highlight.trim().is_empty() || th.is_ansi() && highlight.is_empty() { out } else { highlight_words(th, out, highlight) };
    }
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(start) = rest.find("[[") {
            if let Some(end) = rest[start..].find("]]") {
                out.extend(tag_spans(th, &rest[..start], base));
                out.push(Span::styled("[[", th.s(Token::Muted)));
                out.push(Span::styled(rest[start + 2..start + end].to_string(), th.s(Token::Link).patch(Style::default().add_modifier(base.add_modifier))));
                out.push(Span::styled("]]", th.s(Token::Muted)));
                rest = &rest[start + end + 2..];
                continue;
            }
        }
        out.extend(tag_spans(th, rest, base));
        break;
    }
    if highlight.trim().is_empty() || th.is_ansi() && highlight.is_empty() {
        return out;
    }
    highlight_words(th, out, highlight)
}

fn tag_spans(th: &Theme, s: &str, base: Style) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut buf = String::new();
    for (i, word) in s.split(' ').enumerate() {
        if i > 0 {
            buf.push(' ');
        }
        if thc_core::capture::tag_of(word).is_some() {
            if !buf.is_empty() {
                out.push(Span::styled(std::mem::take(&mut buf), base));
            }
            out.push(Span::styled(word.to_string(), th.s(Token::Tag).patch(Style::default().add_modifier(base.add_modifier))));
        } else {
            buf.push_str(word);
        }
    }
    if !buf.is_empty() {
        out.push(Span::styled(buf, base));
    }
    out
}

/// Bold + accent-tint on case-insensitive occurrences of each search word.
fn highlight_words(th: &Theme, spans: Vec<Span<'static>>, terms: &str) -> Vec<Span<'static>> {
    let words: Vec<String> = terms.split_whitespace().map(|t| t.to_lowercase()).filter(|t| t.len() >= 2).collect();
    if words.is_empty() {
        return spans;
    }
    let hl = th.s(Token::AccentTint).add_modifier(Modifier::BOLD);
    let mut out = Vec::new();
    for s in spans {
        let text = s.content.to_string();
        let lower = text.to_lowercase();
        if lower.len() != text.len() {
            out.push(s);
            continue;
        }
        let mut i = 0;
        let mut last = 0;
        while i < lower.len() {
            if let Some(wd) = words.iter().find(|wd| lower[i..].starts_with(wd.as_str())) {
                if last < i {
                    out.push(Span::styled(text[last..i].to_string(), s.style));
                }
                out.push(Span::styled(text[i..i + wd.len()].to_string(), s.style.patch(hl)));
                i += wd.len();
                last = i;
            } else {
                i += lower[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
            }
        }
        if last < text.len() {
            out.push(Span::styled(text[last..].to_string(), s.style));
        }
    }
    out
}

/// One date meta item. Under an agenda day heading (`under_day`), the date part is dropped:
/// `due` / `sched` / the time remain, and a lone scheduled date disappears entirely.
fn date_label(th: &Theme, today: NaiveDate, raw: &str, is_due: bool, open: bool, alert: bool, under_day: Option<NaiveDate>, has_other: bool) -> Option<Vec<Span<'static>>> {
    let g = th.glyphs();
    let Some(dv) = DateVal::from_stored(raw) else { return Some(vec![Span::raw(raw.to_string())]) };
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
                return if alert { Some(vec![Span::styled(g.alert.to_string(), th.s(Token::Muted))]) } else { None };
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
    let st = if open && is_due && d < today {
        th.s(Token::Overdue)
    } else if open && d == today {
        th.s(Token::Today)
    } else {
        th.s(Token::Muted)
    };
    let mut v = vec![Span::styled(text, st)];
    if alert {
        v.push(Span::styled(format!(" {}", g.alert), th.s(Token::Muted)));
    }
    Some(v)
}

/// `14:02` for an epoch-ms time, local.
fn ms_hhmm(app: &App, ms: i64) -> String {
    app.derived.data.clock.format(ms, "%H:%M")
}

fn hhmm(stamp: &str) -> String {
    stamp.split('T').nth(1).unwrap_or("").to_string()
}

fn node_label(app: &App, id: &str) -> String {
    app.derived.data.rows.node(&app.vault.paths.vault, id).map(|data| data.label.clone()).unwrap_or_default()
}

fn row_data<'a>(app: &'a App, row: Option<usize>, id: &str) -> Option<&'a crate::row_snapshot::NodeData> {
    let vault = row.and_then(|r| app.row_from(r)).and_then(|i| app.others.get(i))
        .map_or(&app.vault.paths.vault, |other| &other.vault.paths.vault);
    app.derived.data.rows.node(vault, id)
}

fn data_for_row<'a>(app: &'a App, row: &Row) -> Option<&'a crate::row_snapshot::NodeData> {
    let node = row.node()?;
    let index = app.rows.iter().position(|r| std::ptr::eq(r, row));
    row_data(app, index, &node.id)
}

/// Meta variants (§4.1–4.2): full, without context, short repeat, without alert.
fn node_meta(render: &RenderOutput, app: &App, n: &Node, show_context: bool, under_day: Option<NaiveDate>) -> Meta {
    let th = app.theme;
    let g = th.glyphs();
    let open = n.is_open() || n.status.is_none();
    // A row from another vault reads its own store, and its meta starts with that vault's
    // name in that vault's accent (vaults.md §3.5); rows from here carry no name.
    let row = render.drawing_row;
    let data = row_data(app, row, &n.id);
    // A list across vaults: every row ends with its vault, right-aligned in one column, in
    // its accent (view-explain.md §1). It's the last thing to drop.
    let col = (row.is_some() && app.derived.data.presentation.spans_vaults).then(|| {
        let (name, accent) = app.derived.data.presentation.vault_labels.get(&row.and_then(|r| app.row_from(r))).cloned().unwrap_or_else(|| (app.vault_name.clone(), th.accent));
        let w = app.derived.data.presentation.vault_width;
        let name: String = name.chars().take(w).collect();
        let mut vt = th;
        vt.accent = accent;
        let style = if th.is_ansi() { th.s(Token::Text).add_modifier(Modifier::BOLD) } else { vt.s(Token::Accent) };
        vec![Span::raw(format!("   {}", " ".repeat(w.saturating_sub(self::w(&name))))), Span::styled(name, style)]
    });
    let from = row.filter(|_| col.is_none()).and_then(|r| app.row_from(r)).and_then(|i| app.others.get(i)).map(|o| {
        let mut vt = th;
        vt.accent = o.accent;
        let style = if th.is_ansi() { th.s(Token::Text).add_modifier(Modifier::BOLD) } else { vt.s(Token::Accent) };
        Span::styled(o.name.clone(), style)
    });
    let has_alert = data.is_some_and(|data| data.has_alert);
    // In Today only because its alert fires today: `◎ 09:00` in `today`, not muted (§3.3).
    let alert_only = (app.view == View::Today && !app.agenda_mode).then(|| data.and_then(|d| d.alert_only.as_ref())).flatten();
    let has_alert = has_alert && alert_only.is_none();
    let sep = || Span::styled(format!(" {} ", g.sep), th.s(Token::Muted));
    // Done rows: Today shows the done time; Journal adds where it came from (§5.1, §5.5).
    if n.status.as_deref() == Some("done") && matches!(app.view, View::Today | View::Journal) {
        let time = n.done_at.as_deref().map(hhmm).unwrap_or_default();
        let mut v = Vec::new();
        if let Some(f) = &from {
            v.push(f.clone());
            v.push(sep());
        }
        if app.view == View::Journal {
            if let Some(c) = context_label(render, app, n, true) {
                v.push(Span::styled(c, th.s(Token::Muted)));
                v.push(sep());
            }
        }
        v.push(Span::styled(time, th.s(Token::Muted)));
        let mut with = v.clone();
        if let Some(c) = &col {
            with.extend(c.clone());
        }
        return Meta { variants: vec![with, v] };
    }
    let build = |ctx: bool, short_repeat: bool, alert: bool| -> Vec<Span<'static>> {
        let mut items: Vec<Vec<Span<'static>>> = Vec::new();
        if let Some(f) = &from {
            items.push(vec![f.clone()]);
        }
        if let Some(t) = &alert_only {
            items.push(vec![Span::styled(format!("{} {t}", g.alert), th.s(Token::Today))]);
        }
        let timed_sched = n.scheduled.as_deref().is_some_and(|s| s.contains('T'));
        let both = n.scheduled.is_some() && n.due.is_some();
        if let Some(s) = &n.scheduled {
            items.extend(date_label(&th, app.today, s, false, open, alert && (timed_sched || n.due.is_none()), under_day, both));
        }
        if let Some(d) = &n.due {
            items.extend(date_label(&th, app.today, d, true, n.is_open(), alert && !timed_sched && n.scheduled.is_none(), under_day, both));
        }
        if alert && n.scheduled.is_none() && n.due.is_none() {
            items.push(vec![Span::styled(g.alert.to_string(), th.s(Token::Muted))]);
        }
        if let Some(p) = &n.priority {
            items.push(vec![Span::styled(format!("!{p}"), if p == "high" { th.strong() } else { th.s(Token::Muted) })]);
        }
        if let Some(r) = n.repeat.as_ref().and_then(|r| r.get("text")).and_then(|t| t.as_str()) {
            let t = if short_repeat { g.repeat.to_string() } else { format!("{} {}", g.repeat, crate::editor::short_repeat(r)) };
            items.push(vec![Span::styled(t, th.s(Token::Muted))]);
        }
        if n.created_by.starts_with("agent") && app.view != View::Log {
            items.push(vec![Span::styled(format!("{}{}{}", g.agent, g.agent_sep, n.created_by.trim_start_matches("agent:")), th.s(Token::Agent))]);
        }
        if items.is_empty() && app.view == View::Inbox {
            let when = app.derived.data.clock.format(n.created_ms, "%b %-d %H:%M");
            items.push(vec![Span::styled(when, th.s(Token::Muted))]);
        }
        if ctx {
            // Search results come from anywhere, so they name today's journal too.
            if let Some(c) = context_label(render, app, n, app.view == View::Search) {
                // group:parent: the heading already says where (views.md §3.2).
                let norm = |l: &str| l.replace(g.page, "¶").replace(g.journal, "§");
                let repeats = app.view == View::Tasks && app.tasks_group.as_deref() == Some("parent") && section_above(app, &n.id).is_some_and(|t| norm(&t) == norm(&c));
                if !repeats {
                    items.push(vec![Span::styled(c, th.s(Token::Muted))]);
                }
            }
        }
        let mut out = Vec::new();
        for (i, it) in items.into_iter().enumerate() {
            if i > 0 {
                out.push(sep());
            }
            out.extend(it);
        }
        out
    };
    let mut variants = vec![build(show_context, false, has_alert), build(false, false, has_alert), build(false, true, has_alert), build(false, true, false)];
    if let Some(c) = &col {
        // Every variant keeps the vault; only the last, narrowest one goes without it.
        let last = variants.last().cloned().unwrap_or_default();
        for v in variants.iter_mut() {
            v.extend(c.clone());
        }
        variants.push(last);
    }
    Meta { variants }
}



/// The nearest page (`¶ Q4 Planning`) or journal day (`§ Oct 2`) a node lives in, unless it is
/// the view's own subject. Today's journal is omitted unless `include_today`.
fn context_label(render: &RenderOutput, app: &App, n: &Node, include_today: bool) -> Option<String> {
    let g = app.theme.glyphs();
    let root = row_data(app, render.drawing_row, &n.id)?.root.as_ref()?;
    if app.view == View::Pages && app.page_open.as_deref() == Some(root.id.as_str()) {
        return None;
    }
    let today = app.today.format("%Y-%m-%d").to_string();
    match &root.journal {
        Some(j) if app.view == View::Journal && *j == app.journal_date.format("%Y-%m-%d").to_string() => None,
        Some(j) if *j == today => include_today.then(|| format!("{} today", g.journal)),
        Some(j) => DateVal::from_stored(j).map(|d| format!("{} {}", g.journal, d.date().format("%b %-d"))),
        None if root.title.is_some() => Some(format!("{} {}", root_glyph(&root, g), root.label())),
        None => None,
    }
}

fn root_glyph(_n: &Node, g: &crate::theme::Glyphs) -> &'static str {
    g.page
}

/// Tasks table `where`: the direct parent (as the canvas draws it).
fn parent_label(render: &RenderOutput, app: &App, n: &Node) -> Option<String> {
    let g = app.theme.glyphs();
    let data = row_data(app, render.drawing_row, &n.id)?;
    let p = data.parent.as_ref()?;
    let today = app.today.format("%Y-%m-%d").to_string();
    match &p.journal {
        Some(j) if *j == today => Some(format!("{} today", g.journal)),
        Some(j) => DateVal::from_stored(j).map(|d| format!("{} {}", g.journal, d.date().format("%b %-d"))),
        None if p.parent.is_none() && p.title.is_some() => Some(format!("{} {}", g.page, p.label())),
        None => Some(truncate_str(&data.parent_text, 24, g.ellipsis)),
    }
}

fn gutter_for(app: &App, id: &str, selected: bool, focused: bool) -> Gutter {
    if app.conflicts.iter().any(|(_, n, _, _)| n == id) {
        Gutter::Conflict
    } else if selected {
        Gutter::Cursor { focused }
    } else if let Some((_, agent)) = app.flashes.get(id).filter(|(t, _)| app.derived.age(*t).as_secs() < 3) {
        Gutter::Live { agent: *agent }
    } else {
        Gutter::None
    }
}

fn row_line(render: &mut RenderOutput, app: &App, row: &Row, selected: bool, focused: bool, wd: usize) -> Line<'static> {
    let th = app.theme;
    let g = th.glyphs();
    let sel_bg = selected && focused;
    match row {
        Row::Blank => Line::raw(""),
        Row::Muted(s) => Line::from(vec![Span::raw(" "), Span::styled(truncate_str(s, wd.saturating_sub(2), g.ellipsis), th.s(Token::Muted))]),
        Row::ViewItem { name, query, count } => {
            let gutter = if selected { Span::styled(g.cursor, th.s(Token::Accent).add_modifier(Modifier::BOLD)) } else { Span::raw(" ") };
            let left = vec![gutter, Span::raw(" "), Span::styled(format!("@{name:<10} "), th.s(Token::Text)), Span::styled(truncate_str(query, wd.saturating_sub(24), g.ellipsis), th.s(Token::Muted))];
            let right = Span::styled(format!("{count} "), th.s(Token::Muted));
            let pad = wd.saturating_sub(width(&left) + width(std::slice::from_ref(&right)));
            let mut spans = left;
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(right);
            node_row::finish(&th, spans, selected && focused, wd)
        }
        Row::Empty { .. } | Row::TxDetail { .. } | Row::Editing => Line::raw(""),
        Row::NewPage { title } => {
            let gutter = if selected { Span::styled(g.cursor, th.s(Token::Accent).add_modifier(Modifier::BOLD)) } else { Span::raw(" ") };
            let spans = vec![gutter, Span::raw("  "), Span::styled("+ new page \"", th.s(Token::Muted)), Span::styled(title.clone(), th.s(Token::Text)), Span::styled("\"", th.s(Token::Muted))];
            node_row::finish(&th, spans, selected && focused, wd)
        }
        Row::Note { parts, right } => {
            let rs = right.as_ref().map(|r| key_line(&th, r)).unwrap_or_default();
            // Entries (the saved filters' `  1 Work …`) are cut whole: the ones that fit, then
            // `+2 more`, never half an entry.
            let avail = wd.saturating_sub(1 + width(&rs) + 2);
            let pw = |ps: &[(String, Token)]| ps.iter().map(|(t, _)| w(t)).sum::<usize>();
            let mut parts = parts.clone();
            let starts: Vec<usize> = parts.iter().enumerate().filter(|(_, (t, tok))| *tok == Token::Text && t.starts_with("  ")).map(|(i, _)| i).collect();
            if pw(&parts) > avail && !starts.is_empty() {
                for k in (0..starts.len()).rev() {
                    let tail = (format!("  +{} more", starts.len() - k), Token::Muted);
                    if pw(&parts[..starts[k]]) + w(&tail.0) <= avail || k == 0 {
                        parts.truncate(starts[k]);
                        parts.push(tail);
                        break;
                    }
                }
            }
            let mut left: Vec<Span> = vec![Span::raw(" ")];
            left.extend(parts.iter().map(|(t, tok)| Span::styled(t.clone(), th.s(*tok))));
            let mut left = node_row::truncate_spans(&left, wd.saturating_sub(1), g.ellipsis);
            if right.is_some() {
                let pad = wd.saturating_sub(width(&left) + width(&rs) + 1);
                left.push(Span::raw(" ".repeat(pad)));
                left.extend(rs);
            }
            Line::from(left)
        }
        Row::Section { title, count, token, note } => {
            let mut left = vec![Span::raw(" "), Span::styled(title.clone(), th.s(*token).add_modifier(Modifier::BOLD))];
            if let Some(c) = count {
                let of = app.section_totals.get(title).map(|t| format!(" of {t}")).unwrap_or_default();
                left.push(Span::styled(format!("  {c}{of}"), th.s(Token::Muted)));
            }
            if let Some(n) = note {
                let right = key_line(&th, n);
                let pad = wd.saturating_sub(width(&left) + width(&right) + 1);
                left.push(Span::raw(" ".repeat(pad)));
                left.extend(right);
            }
            Line::from(left)
        }
        Row::Node { node, depth, outline, has_children, collapsed, child_count, under_day } => {
            let data = row_data(app, render.drawing_row, &node.id);
            let closed = matches!(node.status.as_deref(), Some("done" | "cancelled"));
            let is_page = node.parent.is_none() && node.title.is_some();
            let base = if is_page {
                th.strong()
            } else if node.status.as_deref() == Some("cancelled") {
                th.s(Token::Muted).add_modifier(Modifier::CROSSED_OUT)
            } else if closed {
                th.s(Token::Muted)
            } else {
                th.s(Token::Text)
            };
            let rendered = data.map(|d| d.text.as_str()).unwrap_or(&node.text);
            let mut first = if node.title.is_some() || node.journal.is_some() { node.label() } else { rendered.lines().next().unwrap_or("").to_string() };
            if rendered.lines().count() > 1 {
                first.push_str(&format!(" {}", g.ellipsis));
            }
            let hl = if app.view == View::Search { app.search_terms.clone() } else { String::new() };
            let mut text = text_spans(&th, &first, base, &hl);
            if *collapsed && *child_count > 0 {
                text.push(Span::styled(format!("  {child_count}"), th.s(Token::Muted)));
            }
            let finder = is_page && app.view == View::Pages && app.page_open.is_none();
            let meta = if finder || (is_page && app.view == View::Search) {
                let kids = data.map_or(0, |d| d.children);
                let open = data.map_or(0, |d| d.open);
                let back = data.map_or(0, |d| d.backlinks);
                let mut v = Vec::new();
                if kids > 0 {
                    v.push(format!("{kids} children"));
                }
                if open > 0 {
                    v.push(format!("{open} open"));
                }
                if back > 0 {
                    v.push(format!("{} {back}", g.backlink));
                }
                let compact = if open > 0 { format!("{open} open") } else { String::new() };
                Meta {
                    variants: vec![
                        vec![Span::styled(v.join(&format!(" {} ", g.sep)), th.s(Token::Muted))],
                        vec![Span::styled(compact, th.s(Token::Muted))],
                        vec![],
                    ],
                }
            } else {
                node_meta(render, app, node, !*outline || app.view == View::Today, *under_day)
            };
            let querying_wide = finder && !app.pages_filter.is_empty() && split_width(app, f_width(app)).is_some();
            let (status, mut text) = if finder {
                // `g0b6v  ¶ Atomic Habits`: the ¶ sits right after the id gap.
                (Span::styled(g.page.to_string(), th.s(Token::Muted)), text)
            } else {
                (status_span(&th, node), text)
            };
            if querying_wide {
                let kids = data.map_or(0, |d| d.children);
                let back = data.map_or(0, |d| d.backlinks);
                let _ = &mut text;
                let m = if back > 0 { format!("{kids} {} {} {back}", g.sep, g.backlink) } else { kids.to_string() };
                let spec = RowSpec {
                    gutter: gutter_for(app, &node.id, selected, focused),
                    selected: sel_bg,
                    id: None,
                    indent: 0,
                    fold: None,
                    status,
                    text,
                    meta: Meta { variants: vec![vec![Span::styled(m, th.s(Token::Muted))]] }, fold_col: false };
                return node_row::render(&th, spec, wd);
            }
            let spec = RowSpec {
                gutter: gutter_for(app, &node.id, selected, focused),
                selected: sel_bg,
                // Pages and days hide IDs unless `.` shows them (§10.5); lists always show them.
                id: if *outline && !app.outline_ids() { None } else { Some(data.map(|d| d.short.clone()).unwrap_or_else(|| node.id.chars().take(5).collect())) },
                indent: if *outline { 2 * depth } else { 0 },
                fold: if *has_children { Some(*collapsed) } else { None },
                status,
                text,
                meta,
                fold_col: *outline && !app.outline_ids(),
            };
            let line = node_row::render(&th, spec, wd);
            // A row an agent (or another device) just changed gets a 3 s tint (§7.3).
            if !selected && app.flashes.get(&node.id).is_some_and(|(t, _)| app.derived.age(*t).as_secs() < 3) && !th.is_ansi() {
                node_row::finish(&th, line.spans, false, wd).patch_style(th.fill(Token::AgentTint))
            } else {
                line
            }
        }
        Row::Tag { id, name, count } => {
            let spec = RowSpec {
                gutter: if selected { Gutter::Cursor { focused } } else { Gutter::None },
                selected: sel_bg,
                id: Some(app.derived.data.rows.short(&app.vault.paths.vault, id)),
                indent: 0,
                fold: None,
                status: Span::styled(" # ", th.s(Token::Muted)),
                text: vec![Span::styled(name.clone(), th.s(Token::Tag))],
                meta: Meta { variants: vec![vec![Span::styled(format!("{count} node{}", if *count == 1 { "" } else { "s" }), th.s(Token::Muted))], vec![Span::styled(count.to_string(), th.s(Token::Muted))]] }, fold_col: false };
            node_row::render(&th, spec, wd)
        }
        Row::Tx { tx, entries } => {
            let e = &entries[0];
            let when = app.derived.data.clock.format(e.ms, "%H:%M");
            let agent = e.actor.starts_with("agent");
            let actor = if agent { actor_name(g, &e.actor) } else { "  you".to_string() };
            let gutter = if selected {
                Span::styled(g.cursor, th.s(Token::Accent).add_modifier(Modifier::BOLD))
            } else {
                Span::raw(" ")
            };
            let summary = tx_summary(app, entries);
            let mut spans = vec![
                gutter,
                Span::raw(" "),
                Span::styled(format!("{when:<5}  "), th.s(Token::Muted)),
                Span::styled(format!("{:<8}", truncate_str(&actor, 8, g.ellipsis)), if agent { th.s(Token::Agent) } else { th.s(Token::Muted) }),
                Span::raw("  "),
                Span::styled(format!("{:<6}", tx[tx.len().saturating_sub(6)..].to_string()), th.s(Token::Muted)),
                Span::raw("   "),
            ];
            let used = width(&spans);
            spans.extend(node_row::truncate_spans(&summary, wd.saturating_sub(used + 1), g.ellipsis));
            node_row::finish(&th, spans, sel_bg, wd)
        }
    }
}

/// First log line: verb + short id + text.
fn tx_summary(app: &App, entries: &[thc_core::model::HistoryEntry]) -> Vec<Span<'static>> {
    let th = app.theme;
    let main = entries.iter().rev().find(|e| e.op.starts_with("node.")).unwrap_or(&entries[0]);
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
    // Prefer the node the user cares about (not tag nodes).
    let target = entries
        .iter()
        .rev()
        .find(|e| e.op == "node.create" && e.body.get("props").and_then(|p| p.get("tag")).is_none())
        .or_else(|| entries.iter().rev().find(|e| e.entity.len() == 12))
        .unwrap_or(main);
    let label = app.derived.data.rows.node(&app.vault.paths.vault, &target.entity).map(|data| {
        match data.node.journal.as_deref().and_then(DateVal::from_stored) {
            Some(d) => format!("{} {}", th.glyphs().journal, d.date().format("%b %-d")),
            None => data.label.clone(),
        }
    }).unwrap_or_default();
    let n = entries.iter().filter(|e| e.op.starts_with("node.")).map(|e| &e.entity).collect::<std::collections::HashSet<_>>().len();
    let mut v = vec![
        Span::styled(format!("{verb} "), th.s(Token::Text)),
        Span::styled(format!("{} ", if target.entity.len() == 12 { app.derived.data.rows.short(&app.vault.paths.vault, &target.entity) } else { String::new() }), th.s(Token::Muted)),
        Span::styled(label, th.s(Token::Text)),
    ];
    if n > 1 {
        v.push(Span::styled(format!("  +{} more", n - 1), th.s(Token::Muted)));
    }
    v
}

// ---- tasks table (120+) -----------------------------------------------------------------------

const COLS: [usize; 7] = [2, 13, 58, 76, 88, 95, 110];

fn place(cells: &[(usize, Vec<Span<'static>>)], wd: usize) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut x = 0;
    for (i, (col, spans)) in cells.iter().enumerate() {
        if *col > x {
            out.push(Span::raw(" ".repeat(col - x)));
            x = *col;
        }
        // Clip to the next column (one cell gap), or the row width.
        let limit = cells.get(i + 1).map(|(c, _)| c.saturating_sub(1)).unwrap_or(wd.saturating_sub(1));
        let max = limit.saturating_sub(x).min(wd.saturating_sub(x + 1));
        let s = node_row::truncate_spans(spans, max, "…");
        x += width(&s);
        out.extend(s);
    }
    out
}

/// Table dates fit the 11-cell column: `yesterday`, `today`, `tue 09:00`, else `Oct 15`.
/// The `(in 12d)` / `1d ago` detail belongs to the list view.
fn table_date(th: &Theme, today: NaiveDate, raw: &str, is_due: bool, open: bool) -> Vec<Span<'static>> {
    let Some(dv) = DateVal::from_stored(raw) else { return vec![Span::raw(raw.to_string())] };
    let d = dv.date();
    let days = (d - today).num_days();
    let word = if (-1..7).contains(&days) { dates::relative(d, today) } else if d.year() == today.year() { d.format("%b %-d").to_string() } else { d.format("%b %-d %y").to_string() };
    let t = dv.time().map(|t| format!(" {}", t.format("%H:%M"))).unwrap_or_default();
    let text = if w(&word) + w(&t) <= 11 { format!("{word}{t}") } else { word };
    let st = if open && is_due && d < today { th.s(Token::Overdue) } else if open && d == today { th.s(Token::Today) } else { th.s(Token::Muted) };
    vec![Span::styled(text, st)]
}

/// Under `group:parent`, the group heading a table row sits under.
fn section_above(app: &App, id: &str) -> Option<String> {
    let i = app.rows.iter().position(|r| matches!(r, Row::Node { node, .. } if node.id == id))?;
    app.rows[..i].iter().rev().find_map(|r| match r {
        Row::Section { title, .. } => Some(title.clone()),
        _ => None,
    })
}

fn table_header(app: &App) -> Line<'static> {
    let th = app.theme;
    let names = ["id", "task", "due", "sched", "pri", "where", "by"];
    let cells: Vec<(usize, Vec<Span<'static>>)> = COLS.iter().zip(names).map(|(c, n)| (*c, vec![Span::styled(n, th.s(Token::Muted))])).collect();
    Line::from(place(&cells, 200))
}

fn table_row(render: &mut RenderOutput, app: &App, row: &Row, selected: bool, focused: bool, wd: usize) -> Line<'static> {
    let th = app.theme;
    let g = th.glyphs();
    let Row::Node { node, .. } = row else { return row_line(render, app, row, selected, focused, wd) };
    let data = row_data(app, render.drawing_row, &node.id);
    let dash = || vec![Span::styled("—", th.s(Token::Muted))];
    let mut task = vec![status_span(&th, node), Span::raw(" ")];
    let base = if matches!(node.status.as_deref(), Some("done" | "cancelled")) { th.s(Token::Muted) } else { th.s(Token::Text) };
    let mut label = data.map(|d| d.text.as_str()).unwrap_or(&node.text).lines().next().unwrap_or("").to_string();
    if node.repeat.is_some() {
        label.push_str(&format!(" {}", g.repeat));
    }
    task.extend(node_row::truncate_spans(&text_spans(&th, &label, base, ""), 41, g.ellipsis));
    let due = node.due.as_deref().map(|d| table_date(&th, app.today, d, true, node.is_open())).unwrap_or_else(dash);
    let sched = node.scheduled.as_deref().map(|d| table_date(&th, app.today, d, false, true)).unwrap_or_else(dash);
    let pri = node.priority.as_deref().map(|p| vec![Span::styled(p.to_string(), if p == "high" { th.strong() } else { th.s(Token::Muted) })]).unwrap_or_else(dash);
    let mut place_ = parent_label(render, app, node).map(|c| vec![Span::styled(c, th.s(Token::Muted))]).unwrap_or_else(dash);
    // In group:parent mode a `where` that repeats its heading is left blank.
    if app.view == View::Tasks && app.tasks_group.as_deref() == Some("parent") {
        let here = parent_label(render, app, node).map(|l| l.replace(g.page, "¶").replace(g.journal, "§"));
        if here.is_some() && here == section_above(app, &node.id) {
            place_ = vec![];
        }
    }
    let by = if node.created_by.starts_with("agent") {
        vec![Span::styled(format!("{}{}{}", g.agent, g.agent_sep, node.created_by.trim_start_matches("agent:")), th.s(Token::Agent))]
    } else {
        vec![Span::styled("you", th.s(Token::Muted))]
    };
    let gutter = gutter_for(app, &node.id, selected, focused);
    let gspan = match gutter {
        Gutter::Cursor { .. } => Span::styled(g.cursor, th.s(Token::Accent).add_modifier(Modifier::BOLD)),
        Gutter::Conflict => Span::styled(g.conflict, th.s(Token::Conflict)),
        Gutter::Live { agent } => Span::styled(g.live, if agent { th.s(Token::Agent) } else { th.s(Token::Accent) }),
        Gutter::None => Span::raw(" "),
    };
    let cells = vec![
        (0, vec![gspan]),
        (COLS[0], vec![Span::styled(data.map(|d| d.short.clone()).unwrap_or_else(|| node.id.chars().take(5).collect()), th.s(Token::Muted))]),
        (COLS[1] - 4, task),
        (COLS[2], due),
        (COLS[3], sched),
        (COLS[4], pri),
        (COLS[5], node_row::truncate_spans(&place_, 14, g.ellipsis)),
        (COLS[6], by),
    ];
    node_row::finish(&th, place(&cells, wd), selected && focused, wd)
}

// ---- right-hand pane --------------------------------------------------------------------------

fn draw_side(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect) {
    match app.view {
        View::Inbox => draw_triage(render, f, app, area),
        View::Journal => draw_calendar(f, app, area),
        View::Log if app.review_lane && app.log_node.is_none() => draw_review_detail(f, app, area),
        View::Log => draw_tx_detail(f, app, area),
        View::Pages if app.page_open.is_none() => draw_page_preview(render, f, app, area),
        _ => draw_detail(render, f, app, area),
    }
}

fn kv(th: &Theme, k: &str, v: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::styled(format!("{k:<11}"), th.s(Token::Muted))];
    spans.extend(v);
    Line::from(spans)
}

fn draw_detail(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect) {
    let th = app.theme;
    let g = th.glyphs();
    let wd = area.width.saturating_sub(1) as usize;
    let Some(n) = app.selected_node().cloned() else {
        let msg = match app.rows.get(app.cursor) {
            Some(Row::Tag { name, count, .. }) => format!("#{name} · {count} items · Enter lists them in Tasks"),
            _ => "nothing selected".into(),
        };
        f.render_widget(Paragraph::new(Line::styled(msg, th.s(Token::Muted))), area);
        return;
    };
    // A row from another vault: its details come from that vault's store.
    let Some(detail) = app.derived.data.detail.node.as_ref().filter(|d| d.id == n.id) else { return };
    let s = &detail.data;
    let mut lines: Vec<Line> = Vec::new();
    // Clickable spans, by line: (line, x from, x to, what) (mouse.md §5, the detail pane).
    let mut clicks: Vec<(usize, u16, u16, Click)> = Vec::new();
    let rendered = s.render_text(&n.text);
    let title = if n.title.is_some() || n.journal.is_some() { n.label() } else { rendered.lines().next().unwrap_or("").to_string() };
    let mut head = vec![status_span(&th, &n), Span::raw(" ")];
    head.extend(node_row::truncate_spans(&text_spans(&th, &title, th.strong(), ""), wd.saturating_sub(4), g.ellipsis));
    lines.push(Line::from(head));
    let mut path = Vec::new();
    let mut path_ids: Vec<String> = Vec::new();
    for p in &detail.ancestors {
        path_ids.push(p.id.clone());
        path.push(if let Some(d) = p.journal.as_deref().and_then(|j| chrono::NaiveDate::parse_from_str(j, "%Y-%m-%d").ok()) {
            // `§ today` / `§ Oct 3`, as the rows say it.
            let day = if (d - app.today).num_days().abs() <= 1 { dates::relative(d, app.today) } else { d.format("%b %-d").to_string() };
            format!("{} {day}", g.journal)
        } else if p.journal.is_some() {
            format!("{} {}", g.journal, p.label())
        } else if p.parent.is_none() && p.title.is_some() {
            format!("{} {}", g.page, p.label())
        } else {
            truncate_str(&s.render_text(&p.label()), 20, g.ellipsis)
        });
    }
    path.reverse();
    path_ids.reverse();
    path.push(s.short(&n.id));
    {
        // Each ancestor in the path opens where it lives (while the whole path fits).
        let mut x = 0u16;
        if w(&path.join(" › ")) <= wd {
            for (seg, id) in path.iter().zip(&path_ids) {
                clicks.push((lines.len(), x, x + w(seg) as u16, Click::Node(id.clone())));
                x += w(seg) as u16 + 3;
            }
        }
    }
    lines.push(Line::styled(truncate_str(&path.join(" › "), wd, g.ellipsis), th.s(Token::Muted)));
    for extra in rendered.lines().skip(1) {
        lines.push(Line::from(text_spans(&th, extra, th.s(Token::Text), "")));
    }
    lines.push(Line::raw(""));
    let fmt_date = |raw: &str, due: bool| -> Vec<Span<'static>> {
        let Some(dv) = DateVal::from_stored(raw) else { return vec![Span::raw(raw.to_string())] };
        let d = dv.date();
        let t = dv.time().map(|t| format!(" {}", t.format("%H:%M"))).unwrap_or_default();
        let overdue = due && n.is_open() && d < app.today;
        let rel = dates::relative(d, app.today);
        let rel = if (d - app.today).num_days() >= 7 { format!("in {}d", (d - app.today).num_days()) } else { rel };
        vec![
            Span::styled(format!("{}{t}", d.format("%a %b %-d")), if overdue { th.s(Token::Overdue) } else { th.s(Token::Text) }),
            Span::styled(format!(" {} {rel}", g.sep), th.s(Token::Muted)),
        ]
    };
    if let Some(st) = &n.status {
        let tok = match st.as_str() {
            "doing" => Token::Doing,
            "waiting" => Token::Waiting,
            "done" => Token::Done,
            _ => Token::Text,
        };
        clicks.push((lines.len(), 11, 11 + w(st) as u16, Click::Field("status")));
        lines.push(kv(&th, "status", vec![Span::styled(st.clone(), th.s(tok))]));
    }
    let value_w = |l: &Line| (width(&l.spans) as u16).max(11);
    if let Some(d) = &n.scheduled {
        let l = kv(&th, "scheduled", fmt_date(d, false));
        clicks.push((lines.len(), 11, value_w(&l), Click::Field("sched")));
        lines.push(l);
    }
    if let Some(d) = &n.due {
        let l = kv(&th, "due", fmt_date(d, true));
        clicks.push((lines.len(), 11, value_w(&l), Click::Field("due")));
        lines.push(l);
    }
    if let Some(p) = &n.priority {
        clicks.push((lines.len(), 11, 12 + w(p) as u16, Click::Field("priority")));
        lines.push(kv(&th, "priority", vec![Span::styled(format!("!{p}"), if p == "high" { th.strong() } else { th.s(Token::Muted) })]));
    }
    if let Some(r) = n.repeat.as_ref().and_then(|r| r.get("text")).and_then(|t| t.as_str()) {
        lines.push(kv(&th, "repeat", vec![Span::styled(format!("{} {r}", g.repeat), th.s(Token::Text))]));
    }
    for b in detail.blockers.clone() {
        clicks.push((lines.len(), 11, wd as u16, Click::Node(b.id.clone())));
        let label = truncate_str(&s.render_text(&b.label()), wd.saturating_sub(20), g.ellipsis);
        lines.push(kv(&th, "blocked by", vec![Span::styled(format!("{} ", s.short(&b.id)), th.s(Token::Muted)), Span::styled(label, th.s(Token::Text))]));
    }
    let tags = detail.tags.clone();
    if !tags.is_empty() {
        clicks.push((lines.len(), 11, wd as u16, Click::Field("tags")));
        lines.push(kv(&th, "tags", vec![Span::styled(tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" "), th.s(Token::Tag))]));
    }
    for (k, v) in detail.props.clone() {
        lines.push(kv(&th, &k, vec![Span::styled(v.as_str().map(str::to_string).unwrap_or(v.to_string()), th.s(Token::Text))]));
    }
    for a in detail.alerts.clone() {
        lines.push(kv(&th, "alert", vec![Span::styled(format!("{} {} · {}", g.alert, a.fire_at.unwrap_or_default().replace('T', " "), a.state), th.s(Token::Text))]));
    }
    let created = app.derived.data.clock.format(n.created_ms, "%b %-d %H:%M");
    let agent = n.created_by.starts_with("agent");
    lines.push(kv(&th, "created", vec![
        Span::styled(format!("{created} {} ", g.sep), th.s(Token::Text)),
        Span::styled(
            actor_name(g, &n.created_by),
            if agent { th.s(Token::Agent) } else { th.s(Token::Text) },
        ),
    ]));
    // `why here   scheduled today · alert 09:00`, under created, on Today and the agenda.
    if app.view == View::Today {
        // The reasons of the occurrence that's shown: a row under Next 7 days (or an agenda
        // day) is there for its upcoming date, not for today.
        let upcoming = [n.scheduled.as_deref(), n.due.as_deref()]
            .into_iter()
            .flatten()
            .filter_map(|d| DateVal::from_stored(d).map(|v| v.date()))
            .filter(|d| *d > app.today && (*d - app.today).num_days() < 7)
            .min();
        let section = section_above(app, &n.id);
        let ahead = app.agenda_mode || section.as_deref().is_some_and(|t| t.starts_with("Next"));
        let day = match (ahead, upcoming) {
            (true, Some(d)) => d,
            _ => app.today,
        };
        let rs = detail.reasons.get(&day).cloned().unwrap_or_default();
        if !rs.is_empty() {
            lines.push(kv(&th, "why here", vec![Span::styled(thc_core::why::describe(&rs, day, app.today), th.s(Token::Text))]));
        }
    }
    let kids = detail.children.clone();
    if !kids.is_empty() {
        let tasks = kids.iter().filter(|k| k.status.is_some()).count();
        let done = kids.iter().filter(|k| k.status.as_deref() == Some("done")).count();
        lines.push(Line::raw(""));
        let head = if tasks > 0 { format!("Subtasks  {done}/{tasks}") } else { format!("Children  {}", kids.len()) };
        lines.push(Line::styled(head, th.strong()));
        for k in kids.iter().take(6) {
            clicks.push((lines.len(), 0, wd as u16, Click::Node(k.id.clone())));
            let label = if k.title.is_some() { k.label() } else { s.render_text(&k.text).lines().next().unwrap_or("").to_string() };
            let meta = k.due.as_deref().and_then(|d| date_label(&th, app.today, d, true, k.is_open(), false, None, false)).unwrap_or_default();
            let mut row = vec![Span::raw(" "), status_span(&th, k), Span::raw(" ")];
            let room = wd.saturating_sub(6 + width(&meta) + 2);
            row.extend(node_row::truncate_spans(&text_spans(&th, &label, th.s(Token::Text), ""), room, g.ellipsis));
            let pad = wd.saturating_sub(width(&row) + width(&meta));
            row.push(Span::raw(" ".repeat(pad)));
            row.extend(meta);
            lines.push(Line::from(row));
        }
    }
    let back = detail.backlinks.clone();
    if !back.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(format!("{} Linked from", g.backlink), th.strong()));
        for b in back.iter().take(4) {
            clicks.push((lines.len(), 0, wd as u16, Click::Node(b.id.clone())));
            lines.push(Line::styled(format!(" {}", truncate_str(&s.render_text(&b.label()), wd.saturating_sub(2), g.ellipsis)), th.s(Token::Muted)));
        }
    }
    let hist = detail.history.clone();
    let more_history = hist.len() > 1;
    let hist: Vec<_> = hist.into_iter().take(4).collect();
    if !hist.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled("History", th.strong()));
        for e in hist {
            clicks.push((lines.len(), 0, wd as u16, Click::HistoryTx(e.tx.clone())));
            let when = app.derived.data.clock.format(e.ms, "%H:%M");
            let ag = e.actor.starts_with("agent");
            let actor = actor_name(g, &e.actor);
            let what = history_words(app, &e);
            lines.push(Line::from(vec![
                Span::styled(format!("{when}  "), th.s(Token::Muted)),
                Span::styled(format!("{:<8}", truncate_str(&actor, 8, g.ellipsis)), if ag { th.s(Token::Agent) } else { th.s(Token::Text) }),
                Span::styled(truncate_str(&what, wd.saturating_sub(16), g.ellipsis), th.s(Token::Muted)),
            ]));
        }
        if more_history {
            let pairs = [("L", "full history"), ("R", "rewind…")];
            hint_clicks(&mut clicks, lines.len(), &pairs);
            lines.push(Line::from(hints(&th, &pairs)));
        }
    }
    lines.push(Line::raw(""));
    let pairs = [("e", "edit"), ("x", "done"), ("d", "date"), ("p", "priority"), ("m", "move")];
    hint_clicks(&mut clicks, lines.len(), &pairs);
    lines.push(Line::from(hints(&th, &pairs)));
    f.render_widget(Paragraph::new(lines), area);
    for (i, x0, x1, what) in clicks {
        if (i as u16) < area.height {
            target(render, area.x + x0, (area.x + x1).min(area.right()), area.y + i as u16, what);
        }
    }
}

/// The targets of a `hints()` line drawn at the left of its row: each `key label` runs the key.
fn hint_clicks(clicks: &mut Vec<(usize, u16, u16, Click)>, line: usize, pairs: &[(&str, &str)]) {
    let mut x = 0u16;
    for (k, l) in pairs {
        let pw = (w(k) + 1 + w(l)) as u16;
        if let Some((code, mods)) = key_of(k) {
            clicks.push((line, x, x + pw, Click::Key(code, mods)));
        }
        x += pw + 2;
    }
}

fn draw_triage(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect) {
    let th = app.theme;
    let g = th.glyphs();
    let wd = area.width.saturating_sub(1) as usize;
    let Some(n) = app.selected_node().cloned() else { return draw_detail(render, f, app, area) };
    let Some(detail) = app.derived.data.detail.node.as_ref().filter(|d| d.id == n.id) else { return };
    let s = &detail.data;
    let when = app.derived.data.clock.format(n.created_ms, "%b %-d %H:%M");
    let agent = n.created_by.starts_with("agent");
    let mut lines = vec![
        Line::from(vec![
            Span::styled(format!("{} {} inbox {} {when} {} ", s.short(&n.id), g.sep, g.sep, g.sep), th.s(Token::Muted)),
            Span::styled(
                actor_name(g, &n.created_by),
                if agent { th.s(Token::Agent) } else { th.s(Token::Muted) },
            ),
        ]),
        Line::raw(""),
    ];
    for l in s.render_text(&n.text).lines() {
        lines.push(Line::from(text_spans(&th, &truncate_str(l, wd, g.ellipsis), th.s(Token::Text), "")));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled("Triage", th.strong()));
    let recents: Vec<String> = app
        .recent_moves
        .iter()
        .map(|t| match t {
            crate::app::MoveTarget::Inbox => "inbox".to_string(),
            crate::app::MoveTarget::Journal(d) => format!("{} {}", g.journal, d.format("%b %-d")),
            crate::app::MoveTarget::Under(p) | crate::app::MoveTarget::NewPage(p) => node_label(app, p),
        })
        .collect();
    let recent_note = if recents.is_empty() { "none yet".to_string() } else { recents.join(", ") };
    let row = |k: &str, action: &str, note: String| -> Line<'static> {
        Line::from(vec![
            Span::styled(format!("{k:<5}"), th.s(Token::Text)),
            Span::styled(format!("{action:<18}"), th.s(Token::Text)),
            Span::styled(truncate_str(&note, wd.saturating_sub(23), g.ellipsis), th.s(Token::Muted)),
        ])
    };
    lines.push(row("m", "move to…", "page or day".into()));
    lines.push(row("m 1", &format!("{} recent", g.arrow), recent_note));
    lines.push(row("t", "make task", String::new()));
    lines.push(row("d", "set a date", "makes it a task".into()));
    lines.push(row("x", "done", String::new()));
    lines.push(row("D", "delete", "u undo".into()));
    let tags = detail.tags.clone();
    if !tags.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled("tags  ", th.s(Token::Muted)),
            Span::styled(tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" "), th.s(Token::Tag)),
        ]));
    }
    f.render_widget(Paragraph::new(lines), area);
}

fn history_words(app: &App, e: &thc_core::model::HistoryEntry) -> String {
    let g = app.theme.glyphs();
    match e.op.as_str() {
        "node.create" => "created".into(),
        "node.text" => "text edited".into(),
        "node.complete" => match e.body.get("next").and_then(|n| n.get("scheduled").or(n.get("due"))).and_then(|v| v.as_str()) {
            Some(n) => format!("done {} next {n}", g.repeat),
            None => "done".into(),
        },
        "node.move" => "moved".into(),
        "node.delete" => "deleted".into(),
        "node.restore" => "restored".into(),
        _ => app.derived.data.detail.node.as_ref().and_then(|d| d.history_words.get(&e.eid)).cloned().unwrap_or_default(),
    }
}

fn draw_page_preview(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect) {
    let th = app.theme;
    let g = th.glyphs();
    let Some(n) = app.selected_node().cloned().filter(|n| n.parent.is_none() && n.title.is_some()) else {
        return draw_detail(render, f, app, area);
    };
    let wd = area.width.saturating_sub(1) as usize;
    let data = row_data(app, Some(app.cursor), &n.id);
    let open = data.map_or(0, |d| d.open);
    let created = app.derived.data.clock.format(n.created_ms, "%b %-d");
    let who = actor_name(app.theme.glyphs(), &n.created_by);
    let mut bits = vec![data.map(|d| d.short.clone()).unwrap_or_default()];
    if open > 0 {
        bits.push(format!("{open} open"));
    }
    bits.push(format!("created {created} by {who}"));
    let left = vec![Span::styled(format!("{} {}", g.page, n.label()), th.strong())];
    let right = vec![Span::styled(bits.join(&format!(" {} ", g.sep)), th.s(Token::Muted))];
    let pad = wd.saturating_sub(width(&left) + width(&right));
    let mut head = left;
    head.push(Span::raw(" ".repeat(pad)));
    head.extend(right);
    let mut lines = vec![Line::from(head), Line::raw("")];
    // This outline is prepared once for this session/vault revision, outside drawing.
    let Some(preview) = app.derived.preview.as_ref().filter(|p| p.page == n.id && p.vault == app.vault.paths.vault) else { return };
    let rows = &preview.rows;
    for r in rows.iter().take(area.height.saturating_sub(2) as usize) {
        match r {
            Row::Empty { l1, l2 } => {
                lines.push(Line::styled(l1.clone(), th.s(Token::Text)));
                lines.push(Line::from(key_line(&th, l2)));
            }
            Row::Section { .. } => {
                // The pane already has a 1-cell gutter; headings start at its column 0.
                let mut l = row_line(render, app, r, false, false, wd + 1);
                if !l.spans.is_empty() {
                    l.spans.remove(0);
                }
                lines.push(l);
            }
            r => lines.push(row_line(render, app, r, false, false, wd)),
        }
    }
    f.render_widget(Paragraph::new(lines), area);
}

fn draw_calendar(f: &mut Frame, app: &App, area: Rect) {
    let th = app.theme;
    let g = th.glyphs();
    let d = app.journal_date;
    let first = NaiveDate::from_ymd_opt(d.year(), d.month(), 1).unwrap();
    let mut lines = vec![Line::styled(first.format("%B %Y").to_string(), th.strong()), Line::raw(""), Line::styled("Mo Tu We Th Fr Sa Su", th.s(Token::Muted))];
    let calendar = &app.derived.data.detail.calendar;
    // Each week is a 21-cell grid: day (2, right-aligned) + entry dot. Today is drawn as
    // `[3•]`, its brackets taking the neighbouring separator cells.
    let offset = first.weekday().num_days_from_monday() as usize;
    let mut week: Vec<(String, Style)> = vec![(" ".into(), Style::default()); 21];
    let mut day = first;
    let flush_week = |week: &mut Vec<(String, Style)>, lines: &mut Vec<Line<'static>>| {
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (c, st) in week.iter() {
            spans.push(Span::styled(c.clone(), *st));
        }
        lines.push(Line::from(spans));
        *week = vec![(" ".into(), Style::default()); 21];
    };
    let mut col_day = offset;
    while day.month() == d.month() {
        let has = calendar.days.contains(&day);
        let col = col_day * 3;
        let num_style = if day == app.today {
            th.s(Token::Today).add_modifier(Modifier::BOLD)
        } else if day == d {
            th.strong().add_modifier(Modifier::UNDERLINED)
        } else if has {
            th.s(Token::Text)
        } else {
            th.s(Token::Muted)
        };
        let digits: Vec<char> = format!("{:>2}", day.day()).chars().collect();
        for (i, ch) in digits.iter().enumerate() {
            if *ch != ' ' {
                week[col + i] = (ch.to_string(), num_style);
            }
        }
        if has {
            week[col + 2] = (g.live.to_string(), if day == d { th.s(Token::Accent) } else { th.s(Token::Muted) });
        }
        if day == app.today {
            let today_st = th.s(Token::Today);
            let open_at = if day.day() < 10 { col } else { col.saturating_sub(1) };
            week[open_at] = ("[".into(), today_st);
            if col + 3 < 21 {
                week[col + 3] = ("]".into(), today_st);
            }
        }
        if day.weekday().num_days_from_monday() == 6 {
            flush_week(&mut week, &mut lines);
            col_day = 0;
        } else {
            col_day += 1;
        }
        day += Duration::days(1);
    }
    if col_day > 0 {
        flush_week(&mut week, &mut lines);
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled(format!("{} has entries   ", g.live), th.s(Token::Muted)),
        Span::styled("[ ]", th.s(Token::Today)),
        Span::styled(" today", th.s(Token::Muted)),
    ]));
    lines.push(Line::raw(""));
    let tasks = calendar.tasks;
    let done = calendar.done;
    lines.push(Line::styled(d.format("%a %b %-d").to_string(), th.strong()));
    lines.push(Line::styled(format!("{} entries {} {tasks} tasks {} {done} done", calendar.entries, g.sep, g.sep), th.s(Token::Muted)));
    lines.push(Line::raw(""));
    lines.push(Line::from(hints(&th, &[("gd", "go to date…"), ("{ }", "week")])));
    f.render_widget(Paragraph::new(lines), area);
}

/// ` · 22:46 · studio-mini`, with the via only when it isn't the usual cli or tui.
fn header_tail(app: &App, it: &thc_core::review::Item, today: chrono::NaiveDate, sep: &str) -> String {
    let via = if matches!(it.via.as_str(), "cli" | "tui") { String::new() } else { format!(" {sep} via {}", it.via) };
    format!(" {sep} {} {sep} {}{via}", app.derived.data.clock.when_words(it.ms, today), it.dev)
}

/// Lines for one review change: marker row, then `key  old → new` (old muted).
fn review_change_lines(app: &App, c: &thc_core::review::Change, wd: usize) -> Vec<Line<'static>> {
    let th = app.theme;
    let g = th.glyphs();
    let s = &app.derived.data.detail.review_data;
    let title = s.node(&c.node).map(|n| {
        let t = if n.title.is_some() { n.label() } else { s.render_text(&n.text) };
        let t = t.lines().next().unwrap_or("").to_string();
        match n.status.as_deref() {
            Some("done") => format!("[x] {t}"),
            Some(_) => format!("[ ] {t}"),
            None => t,
        }
    }).unwrap_or_else(|| c.text.clone());
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("{} ", crate::app::review_marker(c.change, g)), th.s(Token::Muted)),
        Span::styled(format!("{:<6} ", c.short), th.s(Token::Muted)),
        Span::styled(truncate_str(&title, wd.saturating_sub(9), g.ellipsis), th.s(Token::Text)),
    ])];
    if matches!(c.change, "complete" | "delete" | "restore") {
        lines.push(Line::styled(format!("  {}", app.derived.data.detail.change_words.get(&c.node).cloned().unwrap_or_default()), th.s(Token::Text)));
        return lines;
    }
    for (k, old, new) in crate::app::review_field_lines(c, g, app.today) {
        if (k == "text" || k == "status") && c.change == "create" {
            continue;
        }
        let new = if k == "text" { s.render_text(&new) } else { new };
        let mut spans = vec![Span::raw("  "), Span::styled(format!("{k:<9}"), th.s(Token::Muted))];
        if let Some(o) = old {
            let o = if k == "text" { s.render_text(&o) } else { o };
            let strike = if !th.is_ansi() { Modifier::CROSSED_OUT } else { Modifier::empty() };
            spans.push(Span::styled(o, th.s(Token::Muted).add_modifier(strike)));
            spans.push(Span::styled(format!(" {} ", g.arrow), th.s(Token::Muted)));
        }
        spans.push(Span::styled(new, th.s(Token::Text)));
        lines.push(Line::from(node_row::truncate_spans(&spans, wd, g.ellipsis)));
    }
    if c.change == "delete" {
        if let Some(k) = c.children.filter(|k| *k > 0) {
            lines.push(Line::styled(format!("  deleted with {k} children"), th.s(Token::Muted)));
        }
    }
    lines
}

/// Detail pane in the review lane: the transaction's full diff (agents.md §1.6).
fn draw_review_detail(f: &mut Frame, app: &App, area: Rect) {
    let th = app.theme;
    let g = th.glyphs();
    let wd = area.width.saturating_sub(1) as usize;
    let Some(Row::Tx { tx, .. }) = app.rows.get(app.cursor).cloned() else { return };
    let Some(it) = app.derived.data.detail.review.as_ref().filter(|it| it.tx == tx) else { return };
    let agent = it.actor.starts_with("agent");
    let who = actor_name(g, &it.actor);
    let mut lines = vec![
        Line::from(vec![
            Span::styled(format!("tx {}  ", it.short), th.strong()),
            Span::styled(who, if agent { th.s(Token::Agent) } else { th.s(Token::Text) }),
            Span::styled(header_tail(app, &it, app.today, g.sep), th.s(Token::Muted)),
        ]),
        Line::raw(""),
    ];
    for c in &it.changes {
        lines.extend(review_change_lines(app, c, wd));
        for l in it.later.iter().filter(|l| l.node == c.node && l.actor.starts_with("human") && !(it.conflict && l.field == "text")) {
            let field = if l.field == "parent" { "its place" } else { l.field.as_str() };
            lines.push(Line::styled(format!("  you changed {field} afterwards ({})", app.derived.data.clock.when_words(l.ms, app.today)), th.s(Token::Today)));
        }
        lines.push(Line::raw(""));
    }
    if it.conflict {
        lines.push(Line::styled(format!("{} in conflict with your edit {} c compare", g.conflict, g.sep), th.s(Token::Conflict)));
        lines.push(Line::raw(""));
    }
    lines.push(Line::from(hints(&th, &[("a", "accept"), ("u", "revert"), ("A", "accept all"), ("Enter", "open")])));
    f.render_widget(Paragraph::new(lines), area);
}

/// Node log detail: this transaction's changes to the node (`thc diff --tx`, agents.md §6.3).
fn draw_node_tx_diff(f: &mut Frame, app: &App, area: Rect, tx: &str, node: &str) {
    let th = app.theme;
    let g = th.glyphs();
    let wd = area.width.saturating_sub(1) as usize;
    let Some(it) = app.derived.data.detail.review.as_ref().filter(|it| it.tx == tx) else { return };
    let agent = it.actor.starts_with("agent");
    let who = actor_name(g, &it.actor);
    let mut lines = vec![
        Line::from(vec![
            Span::styled(format!("tx {}  ", it.short), th.strong()),
            Span::styled(who, if agent { th.s(Token::Agent) } else { th.s(Token::Text) }),
            Span::styled(header_tail(app, &it, app.today, g.sep), th.s(Token::Muted)),
        ]),
        Line::raw(""),
    ];
    match it.changes.iter().find(|c| c.node == node) {
        Some(c) => lines.extend(review_change_lines(app, c, wd)),
        None => lines.push(Line::styled("no change to this node", th.s(Token::Muted))),
    }
    if let Some(v) = &it.verdict {
        lines.push(Line::raw(""));
        lines.push(Line::styled(v.clone(), th.s(Token::Muted)));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(hints(&th, &[("R", "rewind to here"), ("u", "undo this tx")])));
    f.render_widget(Paragraph::new(lines), area);
}

fn draw_tx_detail(f: &mut Frame, app: &App, area: Rect) {
    let th = app.theme;
    let g = th.glyphs();
    let wd = area.width.saturating_sub(1) as usize;
    let Some(Row::Tx { tx, entries }) = app.rows.get(app.cursor).cloned() else { return };
    if let Some(node) = &app.log_node {
        return draw_node_tx_diff(f, app, area, &tx, node);
    }
    let e = &entries[0];
    let short = tx[tx.len().saturating_sub(6)..].to_string();
    let at = app.derived.data.clock.format(e.ms, "%a %b %-d %H:%M:%S");
    let agent = e.actor.starts_with("agent");
    let mut lines = vec![
        Line::from(vec![Span::styled(format!("tx {short}"), th.strong()), Span::styled(format!("  {tx}"), th.s(Token::Muted))]),
        Line::raw(""),
        Line::from(vec![
            Span::styled(format!("{:<9}", "actor"), th.s(Token::Muted)),
            Span::styled(
                if agent { format!("{} (agent)", actor_name(g, &e.actor)) } else { actor_name(g, &e.actor) },
                if agent { th.s(Token::Agent) } else { th.s(Token::Text) },
            ),
        ]),
        Line::from(vec![Span::styled(format!("{:<9}", "via"), th.s(Token::Muted)), Span::styled(e.via.clone(), th.s(Token::Text))]),
        Line::from(vec![Span::styled(format!("{:<9}", "device"), th.s(Token::Muted)), Span::styled(e.dev.clone(), th.s(Token::Text))]),
        Line::from(vec![Span::styled(format!("{:<9}", "at"), th.s(Token::Muted)), Span::styled(at, th.s(Token::Text))]),
        Line::raw(""),
        Line::styled(format!("ops  {}", entries.len()), th.strong()),
    ];
    let budget = area.height.saturating_sub(13) as usize;
    let mut op_lines: Vec<Line> = Vec::new();
    for en in entries.iter().rev() {
        let short_e = if en.entity.len() == 12 { app.derived.data.detail.review_data.short(&en.entity) } else { en.entity.clone() };
        if en.op == "node.create" {
            op_lines.push(Line::styled(format!(" {:<14} {short_e}", en.op), th.s(Token::Text)));
            let b = &en.body;
            if let Some(t) = b.get("title").and_then(|v| v.as_str()) {
                op_lines.push(Line::styled(truncate_str(&format!("   title    {t}"), wd, g.ellipsis), th.s(Token::Muted)));
            }
            if let Some(t) = b.get("text").and_then(|v| v.as_str()).filter(|t| !t.is_empty()) {
                op_lines.push(Line::styled(truncate_str(&format!("   text     {}", app.derived.data.detail.review_data.render_text(t)), wd, g.ellipsis), th.s(Token::Muted)));
            }
            if let Some(props) = b.get("props").and_then(|v| v.as_object()) {
                for (k, v) in props {
                    let raw = v.as_str().map(str::to_string).unwrap_or_else(|| v.get("text").and_then(|t| t.as_str()).map(str::to_string).unwrap_or(v.to_string()));
                    let v = match DateVal::from_stored(&raw) {
                        Some(d) => {
                            let t = d.time().map(|t| format!(" {}", t.format("%H:%M"))).unwrap_or_default();
                            format!("{}{t} {} {}", d.date().format("%a %b %-d"), g.sep, dates::relative(d.date(), app.today))
                        }
                        None => raw,
                    };
                    op_lines.push(Line::styled(truncate_str(&format!("   {k:<8} {v}"), wd, g.ellipsis), th.s(Token::Muted)));
                }
            }
            if let Some(p) = b.get("parent").and_then(|v| v.as_str()) {
                let label = app.derived.data.detail.review_data.node(p)
                    .map(|n| match n.journal.as_deref().and_then(DateVal::from_stored) {
                        Some(d) if d.date() == app.today => format!("{} today", g.journal),
                        Some(d) => format!("{} {}", g.journal, d.date().format("%b %-d")),
                        None if n.parent.is_none() && n.title.is_some() => format!("{} {}", g.page, n.label()),
                        None => n.label(),
                    })
                    .unwrap_or_default();
                op_lines.push(Line::styled(truncate_str(&format!("   parent   {label}"), wd, g.ellipsis), th.s(Token::Muted)));
            }
        } else {
            let what = app.derived.data.detail.tx_words.get(&en.eid).cloned().unwrap_or_default();
            op_lines.push(Line::styled(truncate_str(&format!(" {:<14} {short_e:<6} {what}", en.op), wd, g.ellipsis), th.s(Token::Muted)));
        }
    }
    lines.extend(op_lines.into_iter().take(budget));
    let inv = app.derived.data.detail.inverse_count;
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![Span::styled("u", th.s(Token::Text)), Span::styled(format!("  undo this tx {} adds {inv} inverse events", g.sep), th.s(Token::Muted))]));
    lines.push(Line::styled(format!("thc undo --tx {short}"), th.s(Token::Muted)));
    f.render_widget(Paragraph::new(lines), area);
}

// ---- capture drawer and overlays --------------------------------------------------------------

fn draw_drawer(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect) {
    let th = app.theme;
    let g = th.glyphs();
    let Some(Overlay::Capture { input, targets, which }) = &app.overlay else { return };
    let wd = area.width as usize;
    let mut target = targets.get(*which).map(|t| t.label(app.today, g)).unwrap_or_default();
    // `─ capture → § today · #work (@work) ───`: the context's defaults on the target line.
    let tags = app.context_tags();
    if let (false, Some(n)) = (tags.is_empty(), app.context_name()) {
        target.push_str(&format!(" {} {} (@{n})", g.sep, tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" ")));
    }
    let mut head = vec![
        Span::styled(format!("{} ", g.rule), th.s(Token::Line)),
        Span::styled("capture", th.strong()),
        Span::styled(format!(" {} ", g.arrow), th.s(Token::Muted)),
    ];
    // More than one vault in scope: the target says which (`→ personal · § today`, vaults.md §3.5).
    if !app.others.is_empty() {
        head.push(Span::styled(app.vault_name.clone(), th.s(Token::Accent)));
        head.push(Span::styled(format!(" {} ", g.sep), th.s(Token::Muted)));
    }
    head.push(Span::styled(target, th.s(Token::Text)));
    head.push(Span::raw(" "));
    let used = width(&head);
    head.push(Span::styled(g.rule.repeat(wd.saturating_sub(used)), th.s(Token::Line)));
    let bad: Vec<String> = app.derived.data.input.lenient(&input.buf).map(|(_, b)| b).unwrap_or_default();
    let mut in_spans = vec![Span::raw(" "), Span::styled(format!("{} ", g.prompt), th.s(Token::Accent).add_modifier(Modifier::BOLD))];
    in_spans.extend(token_spans(app, &input.buf, &bad));
    in_spans.push(Span::raw(" ".repeat(wd)));
    let input_line = Line::from(in_spans).patch_style(th.fill(Token::AccentTint));
    let preview = if input.buf.trim().is_empty() {
        Line::styled("   e.g.  Call dentist due:fri #health !high   ·   Dentist at:\"tue 2pm\" [[Health]]", th.s(Token::Muted))
    } else {
        let strict_err = match app.derived.data.input.lenient(&input.buf) {
            Ok((_, bad)) if !bad.is_empty() => app.derived.data.input.strict(&input.buf).err(),
            _ => None,
        };
        match strict_err.map(Err).unwrap_or_else(|| app.derived.data.input.lenient(&input.buf).map(|(c, _)| c)) {
            Ok(c) => {
                let mut n = Node {
                    id: "xxxxxxxxxxxx".into(),
                    parent: None,
                    ord: String::new(),
                    title: None,
                    text: c.text.clone(),
                    status: c.status.clone(),
                    scheduled: c.scheduled.map(|d| d.fmt()),
                    due: c.due.map(|d| d.fmt()),
                    priority: c.priority.clone(),
                    repeat: c.repeat.as_ref().and_then(|r| serde_json::to_value(r).ok()),
                    done_at: None,
                    journal: None,
                    is_tag: false,
                    created_ms: 0,
                    created_by: "human".into(),
                    updated_ms: 0,
                    deleted: false,
                };
                n.parent = Some("preview".into());
                let mut spans = vec![Span::raw("   "), status_span(&th, &n), Span::raw(" ")];
                spans.extend(text_spans(&th, &c.text, th.s(Token::Text), ""));
                let meta = node_meta(render, app, &n, false, None).variants.remove(0);
                if !meta.is_empty() {
                    spans.push(Span::raw("   "));
                    spans.extend(meta);
                }
                Line::from(node_row::truncate_spans(&spans, wd.saturating_sub(1), g.ellipsis))
            }
            Err(e) => Line::styled(format!("   {}", e), th.s(Token::Overdue)),
        }
    };
    let rows = [Line::from(head), input_line, preview];
    // A menu like the rest (mouse.md): the head's target cycles where it goes (Tab), a click in
    // the field places the caret, the preview is text, and a click above it closes it.
    set_overlay(render, area);
    self::target(render, area.x, area.right(), area.y, Click::Key(ratatui::crossterm::event::KeyCode::Tab, ratatui::crossterm::event::KeyModifiers::NONE));
    self::target(render, area.x, area.right(), area.y + 1, Click::Caret { x0: area.x + 3 });
    inert(render, area, area.y + 2);
    f.render_widget(Paragraph::new(rows.to_vec()), area);
    let cx = area.x + 3 + w_chars(&input.buf, input.cur) as u16;
    f.set_cursor_position((cx.min(area.right().saturating_sub(1)), area.y + 1));
}

const ASCII_BORDER: ratatui::symbols::border::Set = ratatui::symbols::border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

/// Rounded box: `╭─ title ───…` and `… hint ─╯` (§7.2); `+ - |` in ASCII mode.
/// Help doubles as a menu (mouse.md §5): a row whose key is a single key runs it.
fn help_targets(render: &mut RenderOutput, rect: Rect, lines: &[Line]) {
    for (i, l) in lines.iter().enumerate().take(rect.height as usize) {
        let y = rect.y + i as u16;
        let Some(first) = l.spans.first() else { continue };
        // A group's title, or a row with no one key to run (`click drag`): text.
        match (l.spans.len() >= 2).then(|| first.content.trim().split(' ').next().and_then(|key| key_of(key.trim_end_matches('•')))).flatten() {
            Some((code, mods)) => target(render, rect.x, rect.right(), y, Click::Key(code, mods)),
            None => inert(render, rect, y),
        }
    }
}

/// An overlay's bottom hint (`1 keep yours · b both · Esc later`) as buttons: each `key label`
/// runs its key (mouse.md §5). The hint is right-aligned on the bottom border, before one rule.
fn hint_targets(render: &mut RenderOutput, r: Rect, hint: &str) {
    let y = r.bottom().saturating_sub(1);
    let label = format!(" {hint} ");
    let mut x = r.right().saturating_sub(2 + w(&label) as u16) + 1;
    for seg in hint.split(" · ") {
        let sw = w(seg) as u16;
        if let Some((code, mods)) = seg.split(' ').next().and_then(key_of) {
            target(render, x, x + sw, y, Click::Key(code, mods));
        }
        x += sw + 3;
    }
}

fn overlay_block<'a>(th: &Theme, title: &'a str, hint: &'a str) -> Block<'a> {
    let g = th.glyphs();
    let mut b = Block::bordered()
        .border_style(th.s(Token::Line))
        .title(Line::from(vec![Span::styled(format!("{} ", g.rule), th.s(Token::Line)), Span::styled(title.to_string(), th.strong()), Span::raw(" ")]))
        .title_bottom(Line::from(vec![Span::styled(format!(" {hint} "), th.s(Token::Muted)), Span::styled(g.rule, th.s(Token::Line))]).right_aligned())
        .padding(ratatui::widgets::Padding::horizontal(1));
    b = if th.ascii { b.border_set(ASCII_BORDER) } else { b.border_type(BorderType::Rounded) };
    if !th.is_ansi() {
        b = b.style(th.s(Token::Raised));
    }
    b
}

fn at(area: Rect, width: u16, height: u16, y: u16) -> Rect {
    let wdt = width.min(area.width.saturating_sub(2));
    let h = height.min(area.height.saturating_sub(y + 1));
    Rect { x: area.x + (area.width - wdt) / 2, y: area.y + y, width: wdt, height: h }
}

fn draw_palette(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect, buf: &str, cur: usize, sel: usize) {
    let th = app.theme;
    let g = th.glyphs();
    let r = at(area, (area.width.saturating_sub(6)).min(74), if app.recent_cmds.is_empty() { 9 } else { 11 }, 4);
    f.render_widget(Clear, r);
    set_overlay(render, r);
    let block = overlay_block(&th, "command", "Enter run · Tab complete · Esc");
    hint_targets(render, r, "Enter run · Tab complete · Esc");
    let inner = block.inner(r);
    f.render_widget(block, r);
    let matches = &app.derived.data.overlay.palette_matches;
    let sel = sel.min(matches.len().saturating_sub(1));
    let has_recent = !app.recent_cmds.is_empty() && buf.is_empty();
    let visible = inner.height.saturating_sub(if has_recent { 4 } else { 2 }) as usize;
    let start = sel.saturating_sub(visible.saturating_sub(1));
    let mut lines = vec![Line::from(vec![Span::styled(": ".to_string(), th.s(Token::Accent).add_modifier(Modifier::BOLD)), Span::styled(buf.to_string(), th.s(Token::Text))]), Line::raw("")];
    target(render, inner.x, inner.right(), inner.y, Click::Caret { x0: inner.x + 2 });
    let iw = inner.width as usize;
    for (i, p) in matches.iter().enumerate().skip(start).take(visible) {
        target(render, inner.x, inner.right(), inner.y + lines.len() as u16, Click::Menu(i));
        let pos = fuzzy_positions(buf, &p.label);
        let mut label: Vec<Span> = p
            .label
            .chars()
            .enumerate()
            .map(|(ci, c)| Span::styled(c.to_string(), if pos.contains(&ci) { th.s(Token::Text).patch(th.s(Token::AccentTint)).add_modifier(Modifier::BOLD) } else { th.s(Token::Text) }))
            .collect();
        let lw = width(&label);
        label.push(Span::raw(" ".repeat(30usize.saturating_sub(lw))));
        label.push(Span::styled(format!("{:<36}", truncate_str(&p.cmd, 35, g.ellipsis)), th.s(Token::Muted)));
        // The key it's on now, from the keymap (keymap.md §7); internal names never show.
        label.push(Span::styled(p.shown.clone(), th.s(Token::Text)));
        let line = Line::from(node_row::truncate_spans(&label, iw, g.ellipsis));
        lines.push(if i == sel { node_row::finish(&th, line.spans, true, iw) } else { line });
    }
    if has_recent {
        while (lines.len() as u16) < inner.height.saturating_sub(1) {
            lines.push(Line::raw(""));
        }
        let entries = &app.derived.data.overlay.palette_entries;
        let names: Vec<String> = app.recent_cmds.iter().filter_map(|k| entries.iter().find(|p| &p.keys == k).map(|p| p.label.clone())).collect();
        inert(render, inner, inner.y + lines.len() as u16);
        lines.push(Line::styled(truncate_str(&format!("recent  {}", names.join(" · ")), iw, g.ellipsis), th.s(Token::Muted)));
    }
    f.render_widget(Paragraph::new(lines), inner);
    f.set_cursor_position((inner.x + 2 + w_chars(buf, cur) as u16, inner.y));
}

/// ⌃O: a page or a day by name (writing.md §3).
fn draw_finder(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect, buf: &str, cur: usize, sel: usize) {
    let th = app.theme;
    let g = th.glyphs();
    let items = &app.derived.data.overlay.finder;
    let h = ((items.len() + 4) as u16).clamp(6, area.height.saturating_sub(6).max(6));
    let r = at(area, (area.width.saturating_sub(6)).min(60), h, 4);
    f.render_widget(Clear, r);
    set_overlay(render, r);
    let hint = "↑↓ choose · Enter go · Esc close";
    let block = overlay_block(&th, "open", hint);
    hint_targets(render, r, hint);
    let inner = block.inner(r);
    f.render_widget(block, r);
    let iw = inner.width as usize;
    let sel = sel.min(items.len().saturating_sub(1));
    let mut lines = vec![Line::from(vec![Span::styled(format!("{} ", g.prompt), th.s(Token::Accent).add_modifier(Modifier::BOLD)), Span::styled(buf.to_string(), th.s(Token::Text))]), Line::raw("")];
    target(render, inner.x, inner.right(), inner.y, Click::Caret { x0: inner.x + 2 });
    let visible = inner.height.saturating_sub(2) as usize;
    let start = sel.saturating_sub(visible.saturating_sub(1));
    if items.is_empty() {
        inert(render, inner, inner.y + lines.len() as u16);
        lines.push(Line::styled("type a page or a day: fri, oct 2, yesterday", th.s(Token::Muted)));
    }
    for (i, (label, _)) in items.iter().enumerate().skip(start).take(visible) {
        target(render, inner.x, inner.right(), inner.y + lines.len() as u16, Click::Menu(i));
        let line = Line::styled(truncate_str(label, iw, g.ellipsis), if label.starts_with('+') { th.s(Token::Muted) } else { th.s(Token::Text) });
        lines.push(if i == sel { node_row::finish(&th, line.spans, true, iw) } else { line });
    }
    f.render_widget(Paragraph::new(lines), inner);
    f.set_cursor_position((inner.x + 2 + w_chars(buf, cur) as u16, inner.y));
}

/// The vault picker (vaults.md §8): `NAME  OPEN  INBOX  SYNC`, `*` on the current one, home last.
fn draw_vaults(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect, rows: &[crate::app::VaultRow], sel: usize, naming: Option<&crate::input::LineInput>) {
    let th = app.theme;
    let g = th.glyphs();
    let h = ((rows.len() + 5 + naming.is_some() as usize) as u16).clamp(6, area.height.saturating_sub(6).max(6));
    let r = at(area, (area.width.saturating_sub(6)).min(64), h, 4);
    f.render_widget(Clear, r);
    set_overlay(render, r);
    let hint = if naming.is_some() { "Enter create · Esc back" } else { "↑↓ choose · Enter switch · n new · Esc close" };
    let block = overlay_block(&th, "vaults", hint);
    hint_targets(render, r, hint);
    let inner = block.inner(r);
    f.render_widget(block, r);
    let iw = inner.width as usize;
    let nw = rows.iter().map(|v| w(&v.name)).max().unwrap_or(4).clamp(4, 24);
    let mut lines = vec![Line::styled(format!("  {:<nw$}  OPEN  INBOX  SYNC", "NAME"), th.s(Token::Muted))];
    inert(render, inner, inner.y);
    let pad2 = |n: Option<usize>| n.map_or("--".to_string(), |n| format!("{n:02}"));
    for (i, v) in rows.iter().enumerate() {
        let mark = if v.current { "*" } else { " " };
        let sync = if v.home { format!("{} {} home", v.sync, g.sep) } else { v.sync.to_string() };
        // Each name in its own colour: the picker previews the vault you switch to.
        let name = middle_truncate(&v.name, nw, g.ellipsis);
        let mut vt = th;
        vt.accent = crate::theme::Accent::parse(&v.accent);
        let rest = format!("{}  {:>4}  {:>5}  {sync}", " ".repeat(nw.saturating_sub(w(&name))), pad2(v.open), pad2(v.inbox));
        target(render, inner.x, inner.right(), inner.y + lines.len() as u16, Click::Menu(i));
        let muted = th.s(if v.current { Token::Text } else { Token::Muted });
        let name_style = if th.is_ansi() { th.s(Token::Text).add_modifier(Modifier::BOLD) } else { vt.s(Token::Accent) };
        let room = iw.saturating_sub(2 + w(&name));
        let line = Line::from(vec![Span::styled(format!("{mark} "), muted), Span::styled(name, name_style), Span::styled(truncate_str(&rest, room, g.ellipsis), muted)]);
        lines.push(if i == sel && naming.is_none() { node_row::finish(&th, line.spans, true, iw) } else { line });
    }
    if naming.is_none() {
        // `n` as a row, so the mouse can make a vault too (mouse.md §5).
        target(render, inner.x, inner.right(), inner.y + lines.len() as u16, Click::Key(ratatui::crossterm::event::KeyCode::Char('n'), ratatui::crossterm::event::KeyModifiers::NONE));
        lines.push(Line::styled("+ new vault", th.s(Token::Muted)));
    }
    if let Some(input) = naming {
        lines.push(Line::raw(""));
        let label = format!("new vault {} ", g.prompt);
        target(render, inner.x, inner.right(), inner.y + lines.len() as u16, Click::Caret { x0: inner.x + w(&label) as u16 });
        lines.push(Line::from(vec![Span::styled(label, th.s(Token::Accent).add_modifier(Modifier::BOLD)), Span::styled(input.buf.clone(), th.s(Token::Text))]));
        f.set_cursor_position((inner.x + w(&format!("new vault {} ", g.prompt)) as u16 + w_chars(&input.buf, input.cur) as u16, inner.y + lines.len() as u16 - 1));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// `?` on a view (view-explain.md §3): a line per section (name, count now, the query, its plain
/// reading), the scope, the rules, and the keys to change it.
fn draw_recipe(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect, name: &str) {
    let th = app.theme;
    let g = th.glyphs();
    let Some(r) = app.derived.data.overlay.recipe.as_ref() else { return };
    let v = &r.view;
    // Queries and readings wrap onto rows under their section, never cut: the derivation is
    // the point (view-explain.md §3). Widths first, then the height they need.
    let full_w = area.width.saturating_sub(4).min(110) as usize;
    let iw0 = full_w.saturating_sub(4);
    let nw = v.sections.iter().map(|s| w(&s.title)).max().unwrap_or(4).min(16);
    let qw = (iw0.saturating_sub(nw + 6)) / 2;
    let rw = iw0.saturating_sub(nw + 6 + qw + 1).max(8);
    let wrapped: Vec<(Vec<String>, Vec<String>)> = v
        .sections
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let cut = |t: &str, width: usize| -> Vec<String> { crate::text::wrap(t, width.max(4)).into_iter().map(|(a, b)| t[a..b].trim_end().to_string()).collect() };
            (cut(&s.query, qw), cut(r.readings.get(i).map(String::as_str).unwrap_or(""), rw))
        })
        .collect();
    let body: usize = wrapped.iter().map(|(q, rd)| q.len().max(rd.len()).max(1)).sum();
    let h = (body + 6) as u16;
    let r_ = at(area, area.width.saturating_sub(4).min(110), h.min(area.height.saturating_sub(4)), 3);
    f.render_widget(Clear, r_);
    set_overlay(render, r_);
    let kind = if v.builtin && v.edited { "built-in · edited" } else if v.builtin { "built-in" } else { "saved" };
    let title = format!("@{name} {} {kind}", g.sep);
    let hint = match (name == "today", v.builtin && v.edited) {
        (true, true) => "e edit · c copy · r reset · * scope · ? keys · Esc",
        (true, false) => "e edit · c copy · * scope · ? keys · Esc",
        (false, true) => "e edit · c copy · r reset · ? keys · Esc",
        (false, false) => "e edit · c copy · ? keys · Esc",
    };
    let block = overlay_block(&th, &title, hint);
    hint_targets(render, r_, hint);
    let inner = block.inner(r_);
    f.render_widget(block, r_);
    let iw = inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    let star = ratatui::crossterm::event::KeyCode::Char('*');
    let none = ratatui::crossterm::event::KeyModifiers::NONE;
    // The scope: a click opens the picker.
    target(render, inner.x, inner.right(), inner.y, if name == "today" { Click::Key(star, none) } else { Click::Text });
    let over = if r.overridden { format!("override on this device {} default {}", g.sep, app.derived.data.presentation.scope_default_text.clone()) } else { String::new() };
    let left = format!("scope  {}: {}", r.scope_text, r.names.join(", "));
    lines.push(Line::from(vec![Span::styled(truncate_str(&left, iw.saturating_sub(w(&over) + 2), g.ellipsis), th.s(Token::Text)), Span::raw(" ".repeat(iw.saturating_sub(w(&left).min(iw) + w(&over)))), Span::styled(over, th.s(Token::Muted))]));
    inert(render, inner, inner.y + 1);
    lines.push(Line::styled(g.rule.repeat(iw), th.s(Token::Line)));
    let _ = iw;
    for (i, s) in v.sections.iter().enumerate() {
        let count = r.counts.get(i).copied().unwrap_or(0);
        let (qs, rs) = &wrapped[i];
        for k in 0..qs.len().max(rs.len()).max(1) {
            inert(render, inner, inner.y + lines.len() as u16);
            let (title, cnt) = if k == 0 { (truncate_str(&s.title, nw, g.ellipsis), format!(" {count:>3}  ")) } else { (String::new(), " ".repeat(6)) };
            let q = qs.get(k).cloned().unwrap_or_default();
            let rd = rs.get(k).cloned().unwrap_or_default();
            lines.push(Line::from(vec![
                Span::styled(format!("{title:<nw$}"), th.strong()),
                Span::styled(cnt, th.s(Token::Text)),
                Span::styled(format!("{q:<qw$} "), th.s(Token::Text)),
                Span::styled(rd, th.s(Token::Muted)),
            ]));
        }
    }
    inert(render, inner, inner.y + lines.len() as u16);
    lines.push(Line::styled(g.rule.repeat(iw), th.s(Token::Line)));
    inert(render, inner, inner.y + lines.len() as u16);
    lines.push(Line::styled(truncate_str("A note shows once, in its first matching section · each section keeps its own sort", iw, g.ellipsis), th.s(Token::Muted)));
    f.render_widget(Paragraph::new(lines), inner);
}

/// `*` on Today (view-explain.md §2): all, this vault, then a checkbox per vault in its accent.
fn draw_scope(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect, sel: usize, picked: &[String]) {
    let th = app.theme;
    let choices = &app.derived.data.presentation.scope_choices;
    let h = (choices.len() + 5) as u16;
    let r = at(area, (area.width.saturating_sub(6)).min(52), h, 4);
    f.render_widget(Clear, r);
    set_overlay(render, r);
    let hint = "Space toggle · Enter done · s save as default";
    let block = overlay_block(&th, "Today · scope", hint);
    hint_targets(render, r, hint);
    let inner = block.inner(r);
    f.render_widget(block, r);
    let iw = inner.width as usize;
    let all = choices.iter().all(|c| picked.contains(c));
    let only_here = picked.len() == 1 && picked[0] == app.vault_name;
    let mut lines: Vec<Line> = Vec::new();
    let row = |render: &mut RenderOutput, lines: &mut Vec<Line>, k: usize, spans: Vec<Span<'static>>, key: &str| {
        target(render, inner.x, inner.right(), inner.y + lines.len() as u16, Click::Menu(k));
        let mut sp = spans;
        let used = width(&sp);
        sp.push(Span::raw(" ".repeat(iw.saturating_sub(used + w(key)))));
        sp.push(Span::styled(key.to_string(), th.s(Token::Muted)));
        let line = Line::from(sp);
        lines.push(if k == sel { node_row::finish(&th, line.spans, true, iw) } else { line });
    };
    let dot = |on: bool| if on { "● " } else { "○ " };
    row(render, &mut lines, 0, vec![Span::styled(format!("{}all vaults", dot(all)), th.s(Token::Text))], "a");
    row(render, &mut lines, 1, vec![Span::styled(format!("{}this vault ({})", dot(only_here && !all), app.vault_name), th.s(Token::Text))], ".");
    inert(render, inner, inner.y + lines.len() as u16);
    lines.push(Line::styled(th.glyphs().rule.repeat(iw), th.s(Token::Line)));
    for (i, name) in choices.iter().enumerate() {
        let (short, accent) = app.derived.data.presentation.vault_styles.get(name).cloned().unwrap_or((None, th.accent));
        let mut vt = th;
        vt.accent = accent;
        let mut spans = vec![Span::styled(if picked.contains(name) { "[x] " } else { "[ ] " }, th.s(Token::Text)), Span::styled(short.clone().unwrap_or_else(|| name.clone()), vt.s(Token::Accent))];
        if short.is_some() {
            spans.push(Span::styled(format!("  ({name})"), th.s(Token::Muted)));
        }
        row(render, &mut lines, i + 2, spans, "");
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// `:history` (navigation.md §7.5): the places, newest first, `▸` on where you are; another
/// vault's places name it in its colour. A menu like every overlay.
fn draw_history(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect, sel: usize) {
    let th = app.theme;
    let g = th.glyphs();
    let entries = &app.history.entries;
    let n = entries.len();
    let h = ((n + 3) as u16).clamp(5, area.height.saturating_sub(6).max(5));
    let r = at(area, (area.width.saturating_sub(6)).min(64), h, 4);
    f.render_widget(Clear, r);
    set_overlay(render, r);
    let hint = "↑↓ choose · Enter go · Esc close";
    let block = overlay_block(&th, "history", hint);
    hint_targets(render, r, hint);
    let inner = block.inner(r);
    f.render_widget(block, r);
    let iw = inner.width as usize;
    let here = app.vault.origin.as_ref().map_or(app.vault.paths.vault.clone(), |o| o.vault.clone());
    let visible = inner.height as usize;
    let sel = sel.min(n.saturating_sub(1));
    let start = sel.saturating_sub(visible.saturating_sub(1));
    let mut lines = vec![];
    if n == 0 {
        inert(render, inner, inner.y);
        lines.push(Line::styled("nowhere yet · places you go are kept here", th.s(Token::Muted)));
    }
    for (k, p) in entries.iter().rev().enumerate().skip(start).take(visible) {
        let i = n - 1 - k;
        target(render, inner.x, inner.right(), inner.y + lines.len() as u16, Click::Menu(k));
        let mark = if i == app.history.pos { "▸ " } else { "  " };
        let when = app.derived.data.clock.format(p.ms, "%H:%M");
        let mut spans = vec![Span::styled(mark.to_string(), th.s(Token::Accent)), Span::styled(p.label.clone(), th.s(Token::Text))];
        if p.vault != here {
            let (name, accent) = app.derived.data.presentation.history_styles.get(&p.vault).cloned().unwrap_or_else(|| (p.vault.display().to_string(), th.accent));
            let mut vt = th;
            vt.accent = accent;
            spans.push(Span::styled(format!(" {} ", g.sep), th.s(Token::Muted)));
            spans.push(Span::styled(name.clone(), vt.s(Token::Accent)));
        }
        let used = width(&spans);
        spans.push(Span::raw(" ".repeat(iw.saturating_sub(used + w(&when)))));
        spans.push(Span::styled(when, th.s(Token::Muted)));
        let line = Line::from(node_row::truncate_spans(&spans, iw, g.ellipsis));
        lines.push(if k == sel { node_row::finish(&th, line.spans, true, iw) } else { line });
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_move(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect, node: &str, buf: &str, cur: usize, sel: usize) {
    let th = app.theme;
    let g = th.glyphs();
    let items = &app.derived.data.overlay.moves;
    let wdt = if area.width < SPLIT_AT { 50 } else { (area.width - 6).min(60) };
    let h = ((items.len() + 5) as u16).min(area.height.saturating_sub(6)).max(6);
    let r = at(area, wdt, h, 6);
    f.render_widget(Clear, r);
    set_overlay(render, r);
    let title = format!("move {} to", app.derived.data.rows.short(&app.vault.paths.vault, node));
    let block = overlay_block(&th, &title, "Enter move · Esc");
    hint_targets(render, r, "Enter move · Esc");
    let inner = block.inner(r);
    f.render_widget(block, r);
    let iw = inner.width as usize;
    let mut lines = vec![Line::from(vec![Span::styled(format!("{} ", g.prompt), th.s(Token::Accent).add_modifier(Modifier::BOLD)), Span::styled(buf.to_string(), th.s(Token::Text))])];
    target(render, inner.x, inner.right(), inner.y, Click::Caret { x0: inner.x + 2 });
    let mut recent_line = None;
    if !app.recent_moves.is_empty() && buf.is_empty() {
        let labels: Vec<String> = app
            .recent_moves
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let l = match t {
                    crate::app::MoveTarget::Inbox => "inbox".to_string(),
                    crate::app::MoveTarget::Journal(d) => format!("{} {}", g.journal, d.format("%b %-d")),
                    crate::app::MoveTarget::Under(p) => format!("{} {}", g.page, node_label(app, p)),
                    crate::app::MoveTarget::NewPage(t) => format!("{} {t}", g.page),
                };
                format!("{} {l}", i + 1)
            })
            .collect();
        recent_line = Some(Line::styled(truncate_str(&format!("recent  {}", labels.join("   ")), iw, g.ellipsis), th.s(Token::Muted)));
    }
    let selectable: Vec<usize> = items.iter().enumerate().filter(|(_, i)| i.is_some()).map(|(i, _)| i).collect();
    let sel_row = selectable.get(sel.min(selectable.len().saturating_sub(1))).copied();
    let visible = inner.height.saturating_sub(lines.len() as u16 + 1) as usize;
    let start = sel_row.unwrap_or(0).saturating_sub(visible.saturating_sub(1));
    for (i, it) in items.iter().enumerate().skip(start).take(visible) {
        let Some(it) = it else {
            inert(render, inner, inner.y + lines.len() as u16);
            lines.push(Line::styled(g.rule.repeat(iw), th.s(Token::Line)));
            continue;
        };
        if let Some(k) = selectable.iter().position(|s| *s == i) {
            target(render, inner.x, inner.right(), inner.y + lines.len() as u16, Click::Menu(k));
        }
        let pos = fuzzy_positions(buf, &it.label);
        let spans: Vec<Span> = it
            .label
            .chars()
            .enumerate()
            .map(|(ci, c)| Span::styled(c.to_string(), if pos.contains(&ci) && !buf.is_empty() { th.s(Token::Text).patch(th.s(Token::AccentTint)).add_modifier(Modifier::BOLD) } else { th.s(Token::Text) }))
            .collect();
        let mut spans = node_row::truncate_spans(&spans, 44.min(iw.saturating_sub(w(&it.note) + 1)), g.ellipsis);
        let lw = width(&spans);
        spans.push(Span::raw(" ".repeat(iw.saturating_sub(lw + w(&it.note)))));
        spans.push(Span::styled(it.note.clone(), th.s(Token::Muted)));
        lines.push(if Some(i) == sel_row { node_row::finish(&th, spans, true, iw) } else { Line::from(spans) });
    }
    if items.is_empty() {
        inert(render, inner, inner.y + lines.len() as u16);
        lines.push(Line::styled("no match · try a page title, today, inbox or a date", th.s(Token::Muted)));
    }
    if let Some(r) = recent_line {
        while (lines.len() as u16) < inner.height.saturating_sub(1) {
            lines.push(Line::raw(""));
        }
        inert(render, inner, inner.y + lines.len() as u16);
        lines.push(r);
    }
    f.render_widget(Paragraph::new(lines), inner);
    f.set_cursor_position((inner.x + 2 + w_chars(buf, cur) as u16, inner.y));
}

/// A compare column's head in a document (§9): `yours · this device · 12:43` on the left,
/// `◆ claude · 12:42` on the right; the device goes first when it doesn't fit, never the time.
/// An actor without the glyph (bars, filters): the agent's name, or `you`.
pub(crate) fn actor_word(actor: &str) -> String {
    actor.strip_prefix("agent:").map_or_else(|| "you".into(), str::to_string)
}

/// Who made a change, as a person reads it: `◆ claude` for an agent, `you` for a person
/// (Other people by name once vaults are shared.) `human` never shows.
pub(crate) fn actor_name(g: &crate::theme::Glyphs, actor: &str) -> String {
    match actor.strip_prefix("agent:") {
        Some(a) => format!("{}{}{a}", g.agent, g.agent_sep),
        None => "you".into(),
    }
}

/// `human` never shows: it's `you`.
fn doc_version_head(app: &App, v: &thc_core::model::ConflictVersion, yours: bool, col_w: usize) -> String {
    let g = app.theme.glyphs();
    let when = app.derived.data.clock.format(v.ms, "%H:%M");
    let who = match v.actor.strip_prefix("agent:") {
        Some(a) => format!("{}{}{a}", g.agent, g.agent_sep),
        None if yours => "yours".into(),
        None => "you".into(),
    };
    let dev = if v.dev == app.vault.device { "this device".to_string() } else { v.dev.clone() };
    let full = format!("{who} {} {dev} {} {when}", g.sep, g.sep);
    if w(&full) < col_w { full } else { format!("{who} {} {when}", g.sep) }
}

fn version_head(app: &App, v: &thc_core::model::ConflictVersion) -> String {
    let g = app.theme.glyphs();
    let when = app.derived.data.clock.format(v.ms, "%H:%M");
    let actor = actor_name(g, &v.actor);
    let dev = if v.dev == app.vault.device { "this device".to_string() } else { v.dev.clone() };
    format!("{dev} {} {actor} {} {when}", g.sep, g.sep)
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.lines() {
        let mut cur = String::new();
        for word in para.split(' ') {
            if !cur.is_empty() && w(&cur) + 1 + w(word) > width {
                lines.push(std::mem::take(&mut cur));
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(word);
        }
        lines.push(cur);
    }
    lines
}

fn draw_compare(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect, detail: &thc_core::model::ConflictDetail) {
    let th = app.theme;
    let g = th.glyphs();
    let s = &app.derived.data.detail.compare_data;
    let short = s.short(&detail.node);
    if detail.kind == "rehomed" {
        // daemon.md §4.0a: why it's here, where it lives now, and the person decides.
        let r = at(area, area.width.saturating_sub(8).min(72), 6, area.height.saturating_sub(11));
        f.render_widget(Clear, r);
        set_overlay(render, r);
        let title = format!("{} moved here {} {short}", g.conflict, g.sep);
        let keys = "1 keep here · 2 delete · Esc later";
        let block = overlay_block(&th, &title, keys);
        hint_targets(render, r, keys);
        let inner = block.inner(r);
        f.render_widget(block, r);
        let label = |id: &str| s.node(id).map(|n| s.render_text(&n.label())).unwrap_or_else(|| id.to_string());
        let parent = detail.other.as_ref().map(|o| label(&o.text)).unwrap_or_default();
        let who = detail.other.as_ref().map(|o| if o.dev == app.vault.device { "this device".to_string() } else { o.dev.clone() }).unwrap_or_default();
        let at_t = detail.other.as_ref().map(|o| ms_hhmm(app, o.ms)).unwrap_or_default();
        let added = app.derived.data.detail.compare_added.clone();
        let added_s = added.map(|e| format!(" while this line was added on {} at {}", if e.dev == app.vault.device { "this device".to_string() } else { e.dev.clone() }, ms_hhmm(app, e.ms))).unwrap_or_default();
        let under = detail.current.as_ref().map(|c| c.text.clone()).filter(|t| !t.is_empty());
        let now = match under.as_deref().and_then(|u| s.node(u)) {
            Some(n) if n.journal.is_some() => format!("{} {}", g.journal, n.journal.clone().unwrap_or_default()),
            Some(n) if n.parent.is_none() && n.title.is_some() => format!("{} {}", g.page, s.render_text(&n.label())),
            Some(n) => format!("\"{}\"", s.render_text(&n.label())),
            None => "the inbox".into(),
        };
        let text = format!("Its parent \"{parent}\" was deleted on {who} at {at_t}{added_s}. It now lives under {now}.");
        let lines: Vec<Line> = wrap(&text, inner.width as usize).into_iter().take(inner.height as usize).map(|l| Line::styled(l, th.s(Token::Text))).collect();
        for y in inner.y..inner.y + lines.len() as u16 {
            inert(render, inner, y);
        }
        f.render_widget(Paragraph::new(lines), inner);
        return;
    }
    if detail.kind == "move" {
        // 5-row explanation box (daemon.md §4.3).
        let r = at(area, area.width.saturating_sub(8).min(72), 5, area.height.saturating_sub(10));
        f.render_widget(Clear, r);
        set_overlay(render, r);
        let title = format!("conflict {} {short} {} move", g.sep, g.sep);
        let block = overlay_block(&th, &title, "Enter ok · Esc later");
    hint_targets(render, r, "Enter ok · Esc later");
        let inner = block.inner(r);
        f.render_widget(block, r);
        let title = |id: &str| s.node(id).map(|n| format!("\"{}\"", s.render_text(&n.label()))).unwrap_or_else(|| id.to_string());
        let target = detail.other.as_ref().map(|o| title(&o.text)).unwrap_or_default();
        let parent = s.node(&detail.node).and_then(|n| n.parent.clone()).map(|p| title(&p)).unwrap_or_else(|| "the inbox".into());
        let who = detail.other.as_ref().map(|o| if o.dev == app.vault.device { "this device".to_string() } else { o.dev.clone() }).unwrap_or_default();
        let text = format!("{who} moved it under {target}, which would loop. The move was skipped, so it stays under {parent}.");
        let lines: Vec<Line> = wrap(&text, inner.width as usize).into_iter().take(inner.height as usize).map(|l| Line::styled(l, th.s(Token::Text))).collect();
        for y in inner.y..inner.y + lines.len() as u16 {
            inert(render, inner, y);
        }
        f.render_widget(Paragraph::new(lines), inner);
        return;
    }
    let wdt = area.width.saturating_sub(8).min(72);
    let col_w = (wdt.saturating_sub(5) / 2) as usize;
    // In a document: yours on the left, whichever the engine stored as `current`
    // (tui-editor.md §9). Lists and the Log keep current / other.
    let doc_mode = app.doc.is_some();
    let swap = doc_mode && !app.derived.data.detail.yours_is_current;
    let (cur, oth) = if swap { (detail.other.clone(), detail.current.clone()) } else { (detail.current.clone(), detail.other.clone()) };
    let who = app.derived.data.detail.theirs_name.clone();
    let (left_word, k1, k2) = if doc_mode { ("yours", "1 keep yours".to_string(), format!("2 keep {who}'s")) } else { ("current", "1 keep current".to_string(), "2 keep other".to_string()) };
    let cur_lines = cur.as_ref().map(|c| wrap(&s.render_text(&c.text), col_w)).unwrap_or_default();
    let oth_lines = oth.as_ref().map(|c| wrap(&s.render_text(&c.text), col_w)).unwrap_or_default();
    let body = cur_lines.len().max(oth_lines.len()).min(8);
    let base_rows = if detail.base.is_some() { 2 } else { 0 };
    let h = (2 + 2 + body + base_rows) as u16;
    let r = at(area, wdt, h, area.height.saturating_sub(h + 3));
    f.render_widget(Clear, r);
    set_overlay(render, r);
    let title = format!("{} compare {short}", g.conflict);
    let keys = format!("{k1} · {k2} · b both · e edit · Esc later");
    let block = overlay_block(&th, &title, &keys);
    hint_targets(render, r, &keys);
    let inner = block.inner(r);
    f.render_widget(block, r);
    // Each column is its version's button: a click keeps it (as 1 / 2), head to last line.
    let mid = inner.x + col_w as u16 + 1;
    for y in inner.y..inner.y + 2 + body as u16 {
        target(render, inner.x, mid, y, Click::Key(ratatui::crossterm::event::KeyCode::Char('1'), ratatui::crossterm::event::KeyModifiers::NONE));
        target(render, mid + 2, inner.right(), y, Click::Key(ratatui::crossterm::event::KeyCode::Char('2'), ratatui::crossterm::event::KeyModifiers::NONE));
    }
    if detail.base.is_some() {
        inert(render, inner, inner.y + 3 + body as u16);
    }
    let pad = |sp: Vec<Span<'static>>| -> Vec<Span<'static>> {
        let used = width(&sp);
        let mut v = sp;
        v.push(Span::raw(" ".repeat(col_w.saturating_sub(used))));
        v
    };
    let mut lines: Vec<Line> = Vec::new();
    let head_l = cur.as_ref().map(|c| if doc_mode { doc_version_head(app, c, true, col_w) } else { version_head(app, c) }).unwrap_or_default();
    let head_r = oth.as_ref().map(|c| if doc_mode { doc_version_head(app, c, false, col_w) } else { version_head(app, c) }).unwrap_or_default();
    // `device · actor · time  current`; the label goes first if the column is too narrow.
    let mut l = if doc_mode {
        pad(vec![Span::styled(head_l.clone(), th.s(Token::Muted))])
    } else if w(&head_l) + 9 <= col_w {
        pad(vec![Span::styled(head_l.clone(), th.s(Token::Muted)), Span::styled(format!(" {left_word}"), th.s(Token::Muted))])
    } else {
        pad(vec![Span::styled(truncate_str(&head_l, col_w, g.ellipsis), th.s(Token::Muted))])
    };
    l.push(Span::styled(format!(" {} ", g.vsep), th.s(Token::Line)));
    l.push(Span::styled(truncate_str(&head_r, col_w, g.ellipsis), th.s(Token::Muted)));
    lines.push(Line::from(l));
    let mut l = vec![Span::styled(k1.clone(), th.s(Token::Text))];
    l = pad(l);
    l.push(Span::styled(format!(" {} ", g.vsep), th.s(Token::Line)));
    l.push(Span::styled(k2.clone(), th.s(Token::Text)));
    lines.push(Line::from(l));
    for i in 0..body {
        let mut l = pad(vec![Span::styled(cur_lines.get(i).cloned().unwrap_or_default(), th.s(Token::Text))]);
        l.push(Span::styled(format!(" {} ", g.vsep), th.s(Token::Line)));
        l.push(Span::styled(oth_lines.get(i).cloned().unwrap_or_default(), th.s(Token::Conflict).remove_modifier(Modifier::BOLD)));
        lines.push(Line::from(l));
    }
    if let Some(b) = &detail.base {
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![Span::styled("base  ", th.s(Token::Muted)), Span::styled(truncate_str(&s.render_text(b), (inner.width as usize).saturating_sub(7), g.ellipsis), th.s(Token::Muted))]));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_help(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect, all: bool, scroll: u16) {
    let th = app.theme;
    let in_doc = app.doc.is_some();
    // The keys come from the keymap (keymap.md §7): the current contexts by group, so a remap
    // shows here. A few rows aren't keys (markers, the mouse, syntax); they follow.
    let mut groups: Vec<(String, Vec<(String, String)>)> = app.derived.data.bindings.help.clone()
        .into_iter()
        .filter(|(g, _)| all || *g != "Leader")
        .map(|(g, rows)| (if g == "Write" { "Writing".to_string() } else { g.to_string() }, rows))
        .collect();
    let rows = |v: &[(&str, &str)]| v.iter().map(|(k, w)| (k.to_string(), w.to_string())).collect::<Vec<_>>();
    if in_doc {
        groups.insert(1, ("Typing".into(), rows(&[("[[", "link a page"), ("- [ ] 1. #", "start a line with a marker"), ("⇧ arrows", "select")])));
        groups.push(("Mouse".into(), rows(&[("click drag", "place the cursor · select"), ("click a link", "go there · ⌥-click edits it"), ("⇧-drag", "your terminal's own selection")])));
        if all {
            groups.push(("Terminal".into(), rows(&[("⌥ keys", "need Option as Meta"), ("kitty", "⇧Enter ⌃Enter ⌘ keys")])));
        }
    }
    if all {
        groups.push(("Syntax".into(), rows(&[("dates", "fri · +3d · nov 1 9am"), ("capture", "due: sched: at: every: !high #tag"), ("query", "status:open due<=+3d #tag sort:due")])));
    }
    // Two columns, balanced by lines (each group is its rows, its title and a gap); one column
    // under 100 wide, where two would cut the words. It scrolls either way.
    let one = area.width < 100;
    let total: usize = groups.iter().map(|(_, r)| r.len() + 2).sum();
    let mut cols: [Vec<(String, Vec<(String, String)>)>; 2] = [vec![], vec![]];
    let mut used = 0;
    for g in groups {
        let n = g.1.len() + 2;
        let ci = if one || used + n / 2 <= total / 2 { 0 } else { 1 };
        if ci == 0 {
            used += n;
        }
        cols[ci].push(g);
    }
    let tall = (cols.iter().map(|c| c.iter().map(|(_, r)| r.len() + 2).sum::<usize>()).max().unwrap_or(0) + 3) as u16;
    let r = at(area, (area.width.saturating_sub(6)).min(84), (area.height.saturating_sub(4)).min(tall.max(8)), 2);
    f.render_widget(Clear, r);
    set_overlay(render, r);
    // Keys that don't fit scroll, and the border says so: nothing is clipped silently.
    let visible = r.height.saturating_sub(3);
    let max_scroll = tall.saturating_sub(3).saturating_sub(visible);
    render.help_max_scroll = max_scroll;
    let scroll = scroll.min(max_scroll);
    let more = scroll < max_scroll;
    let hint = match (all, more) {
        (true, true) => "↓ more · Esc",
        (true, false) => "Esc",
        (false, true) => "↓ more · ? every key · Esc",
        (false, false) => "? every key · Esc",
    };
    let block = overlay_block(&th, if all { "every key" } else { "keys" }, hint);
    hint_targets(render, r, hint);
    let inner = block.inner(r);
    f.render_widget(block, r);
    let half = if one { inner.width } else { inner.width / 2 };
    let col_w = half as usize;
    for (ci, col) in cols.iter().enumerate().filter(|(i, _)| !one || *i == 0) {
        let mut lines: Vec<Line<'static>> = Vec::new();
        for (gi, (title, keys)) in col.iter().enumerate() {
            if gi > 0 {
                lines.push(Line::raw(""));
            }
            lines.push(Line::styled(title.clone(), th.strong()));
            for (k, v) in keys {
                // A label never runs into the next column: cut with an ellipsis.
                let kw = w(k).max(10) + 1;
                let room = col_w.saturating_sub(kw + 1);
                let v = truncate_str(v, room, th.glyphs().ellipsis);
                lines.push(Line::from(vec![Span::styled(format!("{k:<width$}", width = kw), th.s(Token::Text)), Span::styled(v, th.s(Token::Muted))]));
            }
        }
        let rect = Rect { x: inner.x + half * ci as u16, y: inner.y, width: half, height: inner.height.saturating_sub(1) };
        let lines: Vec<Line<'static>> = lines.into_iter().skip(scroll as usize).collect();
        help_targets(render, rect, &lines);
        f.render_widget(Paragraph::new(lines), rect);
    }
    if inner.height > 0 {
        let footer = Rect { y: inner.bottom() - 1, height: 1, ..inner };
        let text = if inner.width >= 39 { "remap any key: :remap · thc keys --edit" } else { ":remap · thc keys --edit" };
        target(render, footer.x, footer.right(), footer.y, Click::Action("keys.remap"));
        f.render_widget(Paragraph::new(truncate_str(text, footer.width as usize, th.glyphs().ellipsis)).style(th.s(Token::Muted)), footer);
    }
}



/// Whether the page's own caret (a document's, an inline input's) may show: not under an overlay
/// or the which-key panel. Overlays that take text place their own caret.
pub fn caret_allowed(app: &App) -> bool {
    app.overlay.is_none() && !crate::keymap::popup_due_at(app, app.derived.now)
}



/// `:focus` (tui-editor.md §8.4): every element with its letter, bottom-right; letters
/// toggle live, 1 2 3 pick a preset.
fn draw_focus(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect) {
    use thc_core::tui_config::ELEMENTS;
    let th = app.theme;
    let (w, h) = (30u16.min(area.width), (ELEMENTS.len() as u16 + 3).min(area.height));
    let r = Rect { x: area.right().saturating_sub(w + 1), y: area.bottom().saturating_sub(h + 1), width: w, height: h };
    f.render_widget(Clear, r);
    set_overlay(render, r);
    let title = format!("focus · {}", app.focus_cfg.label());
    let block = overlay_block(&th, &title, "Enter save · Esc");
    hint_targets(render, r, "Enter save · Esc");
    let inner = block.inner(r);
    f.render_widget(block, r);
    let mut lines: Vec<Line<'static>> = ELEMENTS
        .iter()
        .map(|(e, _, letter, label)| {
            let on = app.focus_cfg.has(*e);
            Line::from(vec![
                Span::styled(format!("{letter}  "), th.s(Token::Text)),
                Span::styled(if on { "[x] " } else { "[ ] " }, th.s(if on { Token::Accent } else { Token::Muted })),
                Span::styled(label.to_string(), th.s(if on { Token::Text } else { Token::Muted })),
            ])
        })
        .collect();
    // Each row toggles its element; the presets are buttons too.
    for (i, (_, _, letter, _)) in ELEMENTS.iter().enumerate() {
        target(render, inner.x, inner.right(), inner.y + i as u16, Click::Key(ratatui::crossterm::event::KeyCode::Char(*letter), ratatui::crossterm::event::KeyModifiers::NONE));
    }
    let py = inner.y + ELEMENTS.len() as u16;
    for (k, x0, x1) in [('1', 0u16, 6u16), ('2', 8, 16), ('3', 18, 24)] {
        target(render, inner.x + x0, inner.x + x1, py, Click::Key(ratatui::crossterm::event::KeyCode::Char(k), ratatui::crossterm::event::KeyModifiers::NONE));
    }
    lines.push(Line::from(vec![
        Span::styled("1", th.s(Token::Text)),
        Span::styled(" bare  ", th.s(Token::Muted)),
        Span::styled("2", th.s(Token::Text)),
        Span::styled(" writer  ", th.s(Token::Muted)),
        Span::styled("3", th.s(Token::Text)),
        Span::styled(" plan", th.s(Token::Muted)),
    ]));
    f.render_widget(Paragraph::new(lines), inner);
}

/// Immutable height inputs for the viewport update. This never assigns scroll.
fn list_viewport(app: &App, height: usize, wd: usize) -> crate::update::Viewport {
    let previous_section = matches!(app.rows.get(app.cursor.saturating_sub(1)), Some(Row::Section { .. }));
    let start = crate::update::list_start(app.scroll, app.cursor, height, previous_section);
    let heights = if app.cursor < app.rows.len() {
        (start..=app.cursor).map(|i| {
            let row = &app.rows[i];
            let editing = app.edit.as_ref().is_some_and(|e| match row {
                Row::Editing => e.node.is_none(), Row::Node { node, .. } => e.node.as_deref() == Some(node.id.as_str()), _ => false,
            });
            row_height(app, row, wd, editing)
        }).collect()
    } else { vec![] };
    crate::update::Viewport::List { cursor: app.cursor, scroll: app.scroll, height, previous_section, heights }
}

/// Derived list header and geometry, shared by scroll following and the immutable view.
pub(crate) struct PreparedList {
    area: Rect,
    revision: u64,
    top: Vec<Line<'static>>,
    cursor: Option<(u16, u16)>,
    list: Rect,
    pinned: Option<usize>,
    height: usize,
    viewport: crate::update::Viewport,
}

fn list_revision(app: &App) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    app.vault.paths.vault.hash(&mut h);
    std::mem::discriminant(&app.view).hash(&mut h);
    app.today.hash(&mut h);
    app.journal_date.hash(&mut h);
    app.cursor.hash(&mut h);
    app.tasks_filter.hash(&mut h);
    app.search_terms.hash(&mut h);
    app.pages_filter.hash(&mut h);
    app.page_open.hash(&mut h);
    format!("{:?}", app.edit).hash(&mut h);
    app.rows.len().hash(&mut h);
    for r in &app.rows {
        std::mem::discriminant(r).hash(&mut h);
        if let Row::Node { node, depth, collapsed, child_count, .. } = r {
            node.id.hash(&mut h);
            node.text.hash(&mut h);
            node.title.hash(&mut h);
            node.status.hash(&mut h);
            node.updated_ms.hash(&mut h);
            depth.hash(&mut h);
            collapsed.hash(&mut h);
            child_count.hash(&mut h);
        } else {
            format!("{r:?}").hash(&mut h);
        }
    }
    h.finish()
}

fn focus_areas(app: &App, area: Rect) -> [Rect; 3] {
    use thc_core::tui_config::El;
    let tabs = app.focus_cfg.has(El::Tabs) as u16;
    let bar = (app.focus_cfg.has(El::Footer) || app.prompt.is_some()) as u16;
    Layout::vertical([Constraint::Length(tabs), Constraint::Min(0), Constraint::Length(bar)]).areas(area)
}

fn normal_areas(app: &App, area: Rect) -> [Rect; 6] {
    let banner = if app.conflicts.is_empty() { 0 } else { 1 };
    let drawer = if matches!(app.overlay, Some(Overlay::Capture { .. })) { 3 } else { 0 };
    Layout::vertical([
        Constraint::Length(1), Constraint::Length(1), Constraint::Length(banner),
        Constraint::Min(0), Constraint::Length(drawer), Constraint::Length(1),
    ]).areas(area)
}

/// Runtime boundary: prepare inputs, update viewport, draw immutably, then accept output.
/// Snapshot and interactive callers use exactly the same boundary.
pub(crate) fn draw_app(f: &mut Frame, app: &mut App) {
    update_frame(app, f.area());
    app.render = draw(f, app);
}

fn update_frame(app: &mut App, area: Rect) {
    prepare_frame(app, area);
    let viewport = app.derived.doc.as_ref().and_then(|d| d.viewport(app))
        .or_else(|| app.derived.list.as_ref().map(|l| l.viewport.clone()));
    if let Some(viewport) = viewport { crate::runtime_effects::dispatch(app, crate::update::Msg::ViewportPrepared(viewport)); }
}

fn prepare_frame(app: &mut App, area: Rect) {
    app.screen_width = area.width;
    crate::derived::prepare(app);
    app.derived.doc = None;
    app.derived.list = None;
    if area.width < 60 || area.height < 24 {
        return;
    }
    let content = if app.focus_mode && app.doc.is_some() {
        focus_areas(app, area)[1]
    } else {
        let content = normal_areas(app, area)[3];
        match split_width(app, area.width) {
            Some(width) => Rect { width, ..content },
            None => content,
        }
    };
    if app.doc.is_some() {
        if app.rail_shows() && app.rail.is_empty() {
            app.build_rail();
        }
        let rail = crate::doc_ui::RAIL_W as u16;
        let content = if app.rail_shows() && content.width >= rail + 60 {
            Rect { x: content.x + rail, width: content.width - rail, ..content }
        } else { content };
        crate::doc_ui::prepare(app, content);
    } else {
        let (top, cursor) = top_rows(app, content.width as usize);
        let table_h = u16::from(app.view == View::Tasks && area.width >= SPLIT_AT);
        let top_h = (top.len() as u16).saturating_add(table_h);
        let list = Rect { y: content.y + top_h, height: content.height.saturating_sub(top_h), ..content };
        let h = list.height as usize;
        let pinned = (app.view == View::Tasks && matches!(app.rows.last(), Some(Row::Note { .. })) && app.rows.len() > h).then(|| app.rows.len() - 1);
        let height = h.saturating_sub(usize::from(pinned.is_some()));
        let viewport = list_viewport(app, height, content.width as usize);
        app.derived.list = Some(PreparedList { area: content, revision: list_revision(app), top, cursor, list, pinned, height, viewport });
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn state(app: &App) -> String {
        format!("{:?}", (
            app.view, app.cursor, app.scroll, &app.selected, app.screen_width,
            app.doc.as_ref().map(|d| (d.blocks().to_vec(), d.caret(), d.anchor(), d.goal(), d.scroll)),
            &app.collapsed, &app.scope_override, app.focus_mode, app.show_detail,
        ))
    }

    fn render(app: &App, size: (u16, u16)) -> RenderOutput {
        let mut term = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
        let mut output = None;
        term.draw(|f| output = Some(draw(f, app))).unwrap();
        output.unwrap()
    }

    /// On both editor engines.
    #[test]
    fn immutable_draw_returns_identical_cells_and_geometry_without_changing_presentation() {
        immutable_draw_returns_identical_cells_and_geometry_without_changing_presentation_on();
    }

    fn immutable_draw_returns_identical_cells_and_geometry_without_changing_presentation_on() {
        let (_scratch, vault) = crate::fuzz::scratch("immutable-render");
        crate::SNAPSHOT.with(|s| s.set(true));
        let mut app = App::new(vault).unwrap();
        app.daemon_live = false;
        app.toast = None;
        app.flashes.clear();
        for size in [(60, 24), (100, 32), (120, 40), (160, 48)] {
            for document in [false, true] {
                app.set_view(if document { View::Journal } else { View::Inbox });
                if document {
                    let doc = app.doc.as_mut().unwrap();
                    doc.set_blocks(vec![
                        crate::editor::Line::new(0, thc_core::outline::Kind::Para, "A wide 字 and a wrapped paragraph. ".repeat(8).as_str()),
                        crate::editor::Line::new(0, thc_core::outline::Kind::Task, "A task"),
                    ]);
                    doc.select_range(Some(crate::editor::BlockPos { line: 0, byte: 0 }), crate::editor::BlockPos { line: 1, byte: 3 });
                }
                for focus in [false, true] {
                    app.focus_mode = focus;
                    for help in [false, true] {
                        app.overlay = help.then_some(Overlay::Help { all: true, scroll: 0 });
                        update_frame(&mut app, Rect::new(0, 0, size.0, size.1));
                        let before = state(&app);
                        let a = render(&app, size);
                        let b = render(&app, size);
                        assert_eq!(a, b, "size={size:?}, document={document}, focus={focus}, help={help}");
                        assert_eq!(before, state(&app));
                        // Drawing returns geometry; the runtime has not accepted any of it.
                        assert!(app.render.click_targets.is_empty());
                    }
                }
            }
        }
    }

    #[test]
    fn interleaved_frames_and_small_frames_do_not_reuse_hit_regions() {
        let (_scratch, vault) = crate::fuzz::scratch("independent-render");
        crate::SNAPSHOT.with(|s| s.set(true));
        let mut app = App::new(vault).unwrap();
        app.toast = None;
        app.overlay = Some(Overlay::Help { all: true, scroll: 0 });
        let size = (120, 40);
        update_frame(&mut app, Rect::new(0, 0, size.0, size.1));
        let first = render(&app, size);
        assert!(first.overlay_rect.is_some());
        assert!(!first.click_targets.is_empty());
        app.overlay = None;
        update_frame(&mut app, Rect::new(0, 0, 30, 10));
        let small = render(&app, (30, 10));
        assert!(small.click_targets.is_empty());
        assert!(small.doc_hits.is_empty());
        assert!(small.overlay_rect.is_none());
        assert!(small.doc_scrollbar.is_none());
        assert!(small.list_scrollbar.is_none());
        assert_eq!(small.help_max_scroll, 0);
        app.overlay = Some(Overlay::Help { all: true, scroll: 0 });
        update_frame(&mut app, Rect::new(0, 0, size.0, size.1));
        assert_eq!(first, render(&app, size));
    }

    /// On both editor engines.
    #[test]
    fn preparation_never_follows_scroll_until_the_viewport_message_is_updated() {
        preparation_never_follows_scroll_until_the_viewport_message_is_updated_on();
    }

    fn preparation_never_follows_scroll_until_the_viewport_message_is_updated_on() {
        let (_scratch, vault) = crate::fuzz::scratch("viewport-update");
        crate::SNAPSHOT.with(|s| s.set(true));
        let mut app = App::new(vault).unwrap();
        app.set_view(View::Journal);
        app.doc.as_mut().unwrap().scroll = 999;
        prepare_frame(&mut app, Rect::new(0, 0, 120, 40));
        assert_eq!(app.doc.as_ref().unwrap().scroll, 999);
        let viewport = app.derived.doc.as_ref().unwrap().viewport(&app).unwrap();
        crate::runtime_effects::dispatch(&mut app, crate::update::Msg::ViewportPrepared(viewport));
        assert!(app.doc.as_ref().unwrap().scroll < 999);
        let followed = render(&app, (120, 40));
        assert!(!followed.doc_hits.is_empty());
        assert_eq!(followed, render(&app, (120, 40)));
    }

    /// On both editor engines.
    #[test]
    fn changed_text_and_resized_areas_cannot_use_stale_document_byte_ranges() {
        changed_text_and_resized_areas_cannot_use_stale_document_byte_ranges_on();
    }

    fn changed_text_and_resized_areas_cannot_use_stale_document_byte_ranges_on() {
        let (_scratch, vault) = crate::fuzz::scratch("stale-document-layout");
        crate::SNAPSHOT.with(|s| s.set(true));
        let mut app = App::new(vault).unwrap();
        app.toast = None;
        app.set_view(View::Journal);
        let doc = app.doc.as_mut().unwrap();
        doc.set_blocks(vec![crate::editor::Line::new(0, thc_core::outline::Kind::Para, "A long paragraph 字".repeat(20).as_str())]);
        doc.set_caret(crate::editor::BlockPos::default());
        update_frame(&mut app, Rect::new(0, 0, 120, 40));
        assert!(!render(&app, (120, 40)).doc_hits.is_empty());
        // A shorter replacement would panic if the old byte ranges were used.
        app.doc.as_mut().unwrap().set_text_unannounced(0, "字");
        assert!(render(&app, (120, 40)).doc_hits.is_empty());
        update_frame(&mut app, Rect::new(0, 0, 120, 40));
        assert!(!render(&app, (120, 40)).doc_hits.is_empty());
        assert!(render(&app, (100, 32)).doc_hits.is_empty());
        update_frame(&mut app, Rect::new(0, 0, 100, 32));
        assert!(!render(&app, (100, 32)).doc_hits.is_empty());
        // Same length and caret, different fold membership, also needs preparation.
        { let d = app.doc.as_mut().unwrap(); let id = d.blocks()[0].id.clone(); d.fold(&id); }
        assert!(render(&app, (100, 32)).doc_hits.is_empty());
    }

    #[test]
    fn changed_list_inputs_do_not_reuse_prepared_headers_or_hit_regions() {
        let (_scratch, vault) = crate::fuzz::scratch("stale-list-layout");
        crate::SNAPSHOT.with(|s| s.set(true));
        let mut app = App::new(vault).unwrap();
        app.set_view(View::Tasks);
        update_frame(&mut app, Rect::new(0, 0, 120, 40));
        assert!(render(&app, (120, 40)).list_height > 0);
        app.tasks_filter = "status:done".into();
        assert_eq!(render(&app, (120, 40)).list_height, 0);
        update_frame(&mut app, Rect::new(0, 0, 120, 40));
        assert!(render(&app, (120, 40)).list_height > 0);
        assert_eq!(render(&app, (100, 32)).list_height, 0);
    }

    /// On both editor engines.
    #[test]
    fn attachment_inputs_are_vault_qualified_and_draw_does_not_reopen_files() {
        attachment_inputs_are_vault_qualified_and_draw_does_not_reopen_files_on();
    }

    fn attachment_inputs_are_vault_qualified_and_draw_does_not_reopen_files_on() {
        let (_a, va) = crate::fuzz::scratch("attachment-session-a");
        let (_b, vb) = crate::fuzz::scratch("attachment-session-b");
        crate::SNAPSHOT.with(|s| s.set(true));
        let mut a = App::new(va).unwrap();
        let mut b = App::new(vb).unwrap();
        for (app, bytes) in [(&mut a, 2), (&mut b, 4096)] {
            app.toast = None;
            app.set_view(View::Journal);
            let path = app.vault.paths.vault.join("files/item.txt");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, vec![b'a'; bytes]).unwrap();
            let doc = app.doc.as_mut().unwrap();
            doc.set_blocks(vec![crate::editor::Line::new(0, thc_core::outline::Kind::Para, "![item](files/item.txt)")]);
            doc.set_caret(crate::editor::BlockPos::default());
            app.doc_write = false;
            update_frame(app, Rect::new(0, 0, 120, 40));
        }
        assert_eq!(a.derived.attachment(&a.vault.paths.vault, "files/item.txt").unwrap().label, " · 2 B");
        assert_eq!(b.derived.attachment(&b.vault.paths.vault, "files/item.txt").unwrap().label, " · 4 KB");
        let before = render(&a, (120, 40));
        assert_ne!(before, render(&b, (120, 40)));
        // Removing the actual file after preparation cannot affect immutable drawing.
        std::fs::remove_file(a.vault.paths.vault.join("files/item.txt")).unwrap();
        assert_eq!(before, render(&a, (120, 40)));
    }

    #[test]
    fn prepared_lists_and_choice_overlays_render_without_a_live_database() {
        let (_scratch, mut vault) = crate::fuzz::scratch("snapshot-only-lists");
        let today = thc_core::dates::today();
        vault.actor = thc_core::event::Actor { kind: "agent".into(), name: Some("codex-engineer-3".into()) };
        vault.transact(|store| {
            let mut b = thc_core::builder::TxBuilder::new(store, today);
            let page = b.create_page("Project", &[])?;
            for i in 0..12 {
                let text = format!("[ ] task {i} due:today #work");
                let cap = thc_core::capture::parse(&text, today)?;
                b.create_from_capture((i % 2 == 0).then(|| page.clone()), &cap, None)?;
            }
            Ok((b.finish(), ()))
        }).unwrap();
        crate::SNAPSHOT.with(|s| s.set(true));
        let mut app = App::new(vault).unwrap();
        app.toast = None;
        app.show_detail = false;
        app.daemon_live = false;
        for detail in [false, true] {
            app.show_detail = detail;
            let size = if detail { (160, 40) } else { (120, 40) };
            for view in [View::Today, View::Inbox, View::Tasks, View::Pages, View::Search, View::Log, View::Journal] {
                app.set_view(view);
                if view == View::Log { app.review_lane = true; app.reload().unwrap(); }
                if view == View::Search { app.search_terms = "task".into(); app.reload().unwrap(); }
                if let Some(i) = app.rows.iter().position(|r| r.node().is_some() || matches!(r, Row::Tx { .. })) { app.cursor = i; }
                let overlays = [
                    None,
                    Some(Overlay::Help { all: true, scroll: 0 }),
                    Some(Overlay::Palette { input: Default::default(), sel: 0 }),
                    Some(Overlay::Finder { input: Default::default(), sel: 0 }),
                    Some(Overlay::Scope { sel: 0, picked: vec![] }),
                    Some(Overlay::History { sel: 0 }),
                    Some(Overlay::Focus),
                    Some(Overlay::Vaults { rows: vec![], sel: 0, naming: None }),
                    Some(Overlay::Move { node: thc_core::id::from_key("page:project"), input: Default::default(), sel: 0 }),
                    Some(Overlay::About(Box::new(crate::about::About { scroll: 0, new: vec![0], since: None, version: app.derived.version.clone(), facts: vec![], search: String::new(), typing: false, hit: 0 }))),
                    Some(Overlay::Recipe { name: "today".into() }),
                    Some(Overlay::Capture { input: crate::input::LineInput::with("call due:fryday !high !low"), targets: vec![crate::app::CaptureTarget::Inbox], which: 0 }),
                    Some(Overlay::Capture { input: crate::input::LineInput::with("call due:+2h #work"), targets: vec![crate::app::CaptureTarget::Inbox], which: 0 }),
                ];
                for overlay in overlays {
                    app.overlay = overlay;
                    update_frame(&mut app, Rect::new(0, 0, size.0, size.1));
                    let frozen = render(&app, size);
                    let database = std::mem::replace(&mut app.vault.store.conn, rusqlite::Connection::open_in_memory().unwrap());
                    assert_eq!(frozen, render(&app, size), "view={view:?}, detail={detail}, overlay={:?}", app.overlay);
                    app.vault.store.conn = database;
                }
            }
        }
    }

    #[test]
    fn identical_node_ids_keep_their_vault_metadata() {
        let (_one, mut one) = crate::fuzz::scratch("snapshot-row-one");
        let (_two, mut two) = crate::fuzz::scratch("snapshot-row-two");
        let today = thc_core::dates::today();
        let id = thc_core::id::from_key("same row in two vaults");
        for (vault, name) in [(&mut one, "Alpha"), (&mut two, "Beta")] {
            vault.transact(|store| {
                let mut b = thc_core::builder::TxBuilder::new(store, today);
                let page = b.create_page(name, &[])?;
                let cap = thc_core::capture::Capture { text: format!("task in {name}"), status: Some("todo".into()), ..Default::default() };
                b.create_from_capture(Some(page), &cap, Some(id.clone()))?;
                Ok((b.finish(), ()))
            }).unwrap();
        }
        let first = one.store.node(&id).unwrap().unwrap();
        let second = two.store.node(&id).unwrap().unwrap();
        crate::SNAPSHOT.with(|s| s.set(true));
        let mut app = App::new(one).unwrap();
        app.set_view(View::Today);
        app.doc = None;
        app.show_detail = true;
        app.others.push(crate::app::OtherVault { name: "second".into(), path: two.paths.vault.clone(), accent: app.theme.accent, vault: two });
        let row = |node| Row::Node { node, depth: 0, outline: false, has_children: false, collapsed: false, child_count: 0, under_day: None };
        app.rows = vec![row(first), row(second)];
        app.row_vault.insert(1, 0);
        app.cursor = 1;
        update_frame(&mut app, Rect::new(0, 0, 160, 40));
        let frozen = render(&app, (160, 40));
        let cells: String = frozen.cells.as_ref().unwrap().content.iter().map(|c| c.symbol()).collect();
        assert!(cells.contains("Alpha") && cells.contains("Beta"));
        assert_eq!(app.derived.data.detail.node.as_ref().unwrap().ancestors[0].title.as_deref(), Some("Beta"));
        let one = std::mem::replace(&mut app.vault.store.conn, rusqlite::Connection::open_in_memory().unwrap());
        let two = std::mem::replace(&mut app.others[0].vault.store.conn, rusqlite::Connection::open_in_memory().unwrap());
        assert_eq!(frozen, render(&app, (160, 40)));
        app.vault.store.conn = one;
        app.others[0].vault.store.conn = two;
    }

    #[test]
    fn compare_and_relative_input_draw_from_frozen_values() {
        // Changing a clock environment input must not affect parallel test cases.
        // The subprocess executes only this test, with the same scratch environment.
        if std::env::var_os("THC_FROZEN_INPUT_TEST_CHILD").is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "ui::render_tests::compare_and_relative_input_draw_from_frozen_values", "--nocapture"])
                .env("THC_FROZEN_INPUT_TEST_CHILD", "1").output().unwrap();
            assert!(output.status.success(), "{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            return;
        }
        let (_scratch, mut vault) = crate::fuzz::scratch("snapshot-only-compare");
        let today = thc_core::dates::today();
        let (_, id) = vault.transact(|store| {
            let mut b = thc_core::builder::TxBuilder::new(store, today);
            let page = b.create_page("Destination", &[])?;
            let id = b.create_from_capture(Some(page.clone()), &thc_core::capture::parse("original", today)?, None)?;
            Ok((b.finish(), id))
        }).unwrap();
        crate::SNAPSHOT.with(|s| s.set(true));
        let mut app = App::new(vault).unwrap();
        app.set_view(View::Journal);
        let line_id = app.doc.as_ref().unwrap().blocks()[0].id.clone();
        app.doc.as_mut().unwrap().replace_content(&line_id, "call due:+2h");
        app.doc_write = true;
        let device = app.vault.device.clone();
        let version = |text: &str| thc_core::model::ConflictVersion {
            text: text.into(), actor: "agent:codex-engineer-3".into(), dev: device.clone(), ms: 1_759_756_800_000, eid: None,
        };
        app.overlay = Some(Overlay::Compare { detail: thc_core::model::ConflictDetail {
            id: 1, node: id.clone(), kind: "text".into(), current: Some(version("original")), other: Some(version("remote")), base: Some("base".into()),
        } });
        update_frame(&mut app, Rect::new(0, 0, 160, 40));
        let compare = render(&app, (160, 40));
        app.overlay = None;
        update_frame(&mut app, Rect::new(0, 0, 160, 40));
        let document = render(&app, (160, 40));
        let database = std::mem::replace(&mut app.vault.store.conn, rusqlite::Connection::open_in_memory().unwrap());
        let saved_now = std::env::var_os("THC_NOW");
        unsafe { std::env::set_var("THC_NOW", "2030-01-01T01:00"); }
        assert_eq!(document, render(&app, (160, 40)));
        unsafe { match saved_now { Some(now) => std::env::set_var("THC_NOW", now), None => std::env::remove_var("THC_NOW") } }
        app.vault.store.conn = database;
        app.overlay = Some(Overlay::Compare { detail: thc_core::model::ConflictDetail {
            id: 1, node: id, kind: "text".into(), current: Some(version("original")), other: Some(version("remote")), base: Some("base".into()),
        } });
        update_frame(&mut app, Rect::new(0, 0, 160, 40));
        let database = std::mem::replace(&mut app.vault.store.conn, rusqlite::Connection::open_in_memory().unwrap());
        assert_eq!(compare, render(&app, (160, 40)));
        app.vault.store.conn = database;
    }

    #[test]
    fn toast_visibility_uses_only_the_prepared_clock() {
        let (_scratch, vault) = crate::fuzz::scratch("render-clock");
        crate::SNAPSHOT.with(|s| s.set(true));
        let mut app = App::new(vault).unwrap();
        app.set_view(View::Inbox);
        update_frame(&mut app, Rect::new(0, 0, 120, 40));
        app.derived.pinned_warning = None;
        // The wall clock is beyond the toast's lifetime; the supplied clock is not.
        let at = app.derived.now - std::time::Duration::from_secs(30);
        app.toast = Some(crate::app::Toast { kind: ToastKind::Agent, parts: vec![("sampled clock toast".into(), Token::Agent)], at });
        app.derived.now = at + std::time::Duration::from_secs(1);
        let visible = render(&app, (120, 40));
        let cells = |output: &RenderOutput| output.cells.as_ref().unwrap().content.iter().map(|c| c.symbol()).collect::<String>();
        assert!(cells(&visible).contains("sampled clock toast"));
        assert_eq!(visible, render(&app, (120, 40)));
        app.derived.now = at + std::time::Duration::from_secs(5);
        assert!(!cells(&render(&app, (120, 40))).contains("sampled clock toast"));
    }
}
