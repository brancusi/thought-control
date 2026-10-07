//! Drawing the sidebar (sidebar.md §4, §6, §14): a header row per panel, its body (a doc panel
//! is its caretline view, laid out at the sidebar's width), a blank row between panels, the
//! height shared between them.
//!
//! Like the main document (doc_ui.rs), everything is laid out before the frame (`prepare`,
//! which may move a view's scroll to keep its caret in sight); `draw` only copies the prepared
//! rows to the screen and records where clicks go.

use crate::app::{App, Focus};
use crate::editor::{DocRow, ViewGeometry};
use crate::sidebar::{Layout, PanelKey, PanelKind, policy};
use crate::text::width;
use crate::theme::Token;
use crate::ui::{Click, RenderOutput};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use thc_core::outline::Kind;

const MARKS: usize = crate::editor::MARKS as usize;
const HANG: usize = crate::editor::HANG as usize;

/// A header's parts a click acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// `▾`/`▸` or the title: fold, and make it active (a double-click: open in main).
    Title,
    Close,
    OpenMain,
    Pinned,
    /// `↓ 9 more`: focus the panel.
    More,
}

/// One panel, laid out.
pub struct PreparedPanel {
    pub key: PanelKey,
    pub header_y: u16,
    pub header: Line<'static>,
    /// Header click spans: (x0, x1, part).
    pub header_targets: Vec<(u16, u16, Part)>,
    pub body_y: u16,
    pub body: Vec<Line<'static>>,
    /// The doc view's rect on screen (for the engine's hit-testing), when it shows one.
    pub view: Option<Rect>,
    /// The caret's cell on screen, when it's in the view.
    pub caret: Option<(u16, u16)>,
    /// A `↓ N more` row's y.
    pub more_y: Option<u16>,
}

/// The sidebar, laid out for one frame.
pub struct PreparedSidebar {
    pub area: Rect,
    pub panels: Vec<PreparedPanel>,
    /// Panels folded for lack of room that didn't fit at all (`↓ 3 more panels`).
    pub hidden: usize,
}

/// Where the sidebar is this frame at screen width `w` (§6.1, §6.4): a column beside the main
/// area, the drawer over its right side or the whole body (narrower). The drawer and replace
/// show while the keyboard is in the sidebar; none at all with no panels, hidden, or in Focus.
pub fn placement(app: &App, w: u16) -> Option<Layout> {
    let sb = &app.ui.sidebar;
    if !sb.has_panels() || !sb.shown || (app.focus_mode && app.doc.is_some()) {
        return None;
    }
    match crate::sidebar::layout_at(sb, w) {
        l @ Layout::Column { .. } => Some(l),
        l if app.ui.focus == Focus::Sidebar => Some(l),
        _ => None,
    }
}

/// The column's width at screen width `w`, when the sidebar is a column now (§6.1).
pub fn column(app: &App, w: u16) -> Option<u16> {
    match placement(app, w) {
        Some(Layout::Column { width }) => Some(width),
        _ => None,
    }
}

/// What a panel needs to lay out its body: its natural height and how to draw it.
struct Measure {
    natural: u16,
    linked: usize,
}

fn backlinks(app: &App, key: &PanelKey) -> usize {
    let (PanelKind::Page, Some(id)) = (key.kind, key.id.as_deref()) else { return 0 };
    let here: std::collections::HashSet<String> = app.panel_doc(key).map(|d| d.blocks().iter().map(|l| l.id.clone()).collect()).unwrap_or_default();
    app.vault
        .store
        .nodes_where("n.deleted=0 AND n.id IN (SELECT src FROM edges WHERE rel='mention' AND dst=?1) ORDER BY n.updated_ms DESC LIMIT 50", &[&id])
        .unwrap_or_default()
        .iter()
        .filter(|n| !here.contains(&n.id))
        .count()
}

/// Lay a doc panel's view out at `w` columns and `h` rows (metas on their own row where they
/// don't fit beside the text). The rows it lays out to.
fn lay_out(app: &mut App, key: &PanelKey, w: u16, h: u16) -> Option<usize> {
    let column = w.saturating_sub((MARKS + HANG) as u16 + 1).max(10);
    let (d, _) = app.panel_doc_mut(key)?;
    let mut g = ViewGeometry { width: w.max(1), height: h.max(1), column, extra_rows: Vec::new(), typewriter: false };
    d.set_view(&g);
    let with_meta: Vec<(usize, String)> = d.blocks().iter().enumerate().filter(|(_, l)| !l.meta.is_empty()).map(|(i, l)| (i, l.meta.clone())).collect();
    let ends = d.first_row_ends(&with_meta.iter().map(|(i, _)| *i).collect::<Vec<_>>());
    for ((i, meta), end) in with_meta.iter().zip(ends) {
        let l = &d.blocks()[*i];
        let m = crate::doc_ui::marker_len(l).min(end.min(l.text.len()));
        let used = MARKS + HANG + l.depth * 4 + width(&l.text[m..end.min(l.text.len())]);
        if used + 2 + width(meta) > w as usize {
            g.extra_rows.push((*i, 1));
        }
    }
    if !g.extra_rows.is_empty() {
        d.set_view(&g);
    }
    let total = d.scroll_rows().0;
    app.main_view_current();
    Some(total)
}

fn measure(app: &mut App, key: &PanelKey, w: u16) -> Measure {
    let linked = backlinks(app, key);
    if !key.kind.is_doc() {
        return Measure { natural: 1, linked: 0 };
    }
    if app.panels.get(key).is_some_and(|rt| rt.problem.is_some()) {
        return Measure { natural: 1, linked: 0 };
    }
    let rows = lay_out(app, key, w, 200).unwrap_or(1);
    Measure { natural: (rows + (linked > 0) as usize).min(u16::MAX as usize) as u16, linked }
}

/// Share `avail` rows between the panels (§6.3). Returns each body's rows (None: folded for
/// lack of room) and how many didn't fit at all.
fn share(natural: &[(u16, bool, bool)], avail: u16) -> (Vec<Option<u16>>, usize) {
    // (natural rows, folded, active)
    let n = natural.len();
    let need = |bodies: &[Option<u16>]| -> u32 {
        let shown = bodies.iter().filter(|b| b.is_some()).count().max(1) as u32;
        bodies.iter().map(|b| 1 + b.unwrap_or(0) as u32).sum::<u32>() + shown.saturating_sub(1)
    };
    let mut bodies: Vec<Option<u16>> = natural.iter().map(|&(r, folded, _)| Some(if folded { 0 } else { r })).collect();
    if need(&bodies) <= avail as u32 {
        return (bodies, 0);
    }
    for (i, &(r, folded, active)) in natural.iter().enumerate() {
        if !active && !folded {
            bodies[i] = Some(r.min(policy::UNFOCUSED_ROWS));
        }
    }
    if need(&bodies) > avail as u32 {
        if let Some(a) = natural.iter().position(|x| x.2) {
            let others: u32 = need(&bodies) - bodies[a].unwrap_or(0) as u32;
            let room = (avail as u32).saturating_sub(others) as u16;
            bodies[a] = Some(room.max(policy::MIN_ACTIVE_ROWS).min(natural[a].0.max(policy::MIN_ACTIVE_ROWS)));
        }
    }
    // Still too tall: fold panels from the bottom up (never the active one), then drop them.
    let mut i = n;
    while need(&bodies) > avail as u32 && i > 0 {
        i -= 1;
        if !natural[i].2 && bodies[i].is_some_and(|b| b > 0) {
            bodies[i] = Some(0);
        }
    }
    let mut hidden = 0;
    let mut i = n;
    while need(&bodies) + (hidden > 0) as u32 > avail as u32 && i > 0 {
        i -= 1;
        if !natural[i].2 && bodies[i].is_some() {
            bodies[i] = None;
            hidden += 1;
        }
    }
    (bodies, hidden)
}

/// Lay the sidebar out in `area` (the column).
pub(crate) fn prepare(app: &mut App, area: Rect) {
    app.derived.sidebar = None;
    let keys = app.panel_keys();
    if keys.is_empty() || area.width < 10 || area.height < 3 {
        return;
    }
    let s_w = area.width;
    let view_w = s_w.saturating_sub(2);
    let active = app.ui.sidebar.active_key();
    let sidebar_focus = app.ui.focus == Focus::Sidebar;
    let measures: Vec<Measure> = keys.iter().map(|k| measure(app, k, view_w)).collect();
    let natural: Vec<(u16, bool, bool)> = keys
        .iter()
        .zip(&measures)
        .map(|(k, m)| (m.natural, app.ui.sidebar.get(k).is_some_and(|p| p.folded), Some(k) == active.as_ref()))
        .collect();
    let (bodies, hidden) = share(&natural, area.height);
    let th = app.theme;
    let g = th.glyphs();
    let newest = app.ui.sidebar.newest();
    let mut y = area.y;
    let mut out = Vec::new();
    for (i, key) in keys.iter().enumerate() {
        let Some(body_h) = bodies[i] else { continue };
        if y >= area.bottom() {
            break;
        }
        let panel = app.ui.sidebar.get(key).cloned().unwrap();
        let focused = sidebar_focus && Some(key) == active.as_ref();
        let folded = panel.folded || body_h == 0;
        let hovered = app.hover.is_some_and(|(hx, hy)| hy == y && hx >= area.x && hx < area.right());
        // ---- the header (§4.1)
        let fill = if focused { th.fill(Token::Selection) } else { Style::default() };
        let mut left: Vec<Span<'static>> = Vec::new();
        left.push(if focused { Span::styled(g.cursor, th.s(Token::Accent).add_modifier(Modifier::BOLD).patch(fill)) } else { Span::styled(" ", fill) });
        left.push(Span::styled(if folded { g.fold_closed } else { g.fold_open }, th.s(Token::Muted).patch(fill)));
        left.push(Span::styled(" ", fill));
        let tag = key.kind == PanelKind::Query && key.query.as_deref().is_some_and(|q| q.starts_with('#'));
        let sym = match key.kind {
            PanelKind::Page => g.page,
            PanelKind::Day => g.journal,
            _ if tag => "#",
            _ => g.query,
        };
        left.push(Span::styled(sym, th.s(if tag { Token::Tag } else { Token::Muted }).patch(fill)));
        left.push(Span::styled(" ", fill));
        let mut title = app.panel_title(key);
        if tag {
            title = title.trim_start_matches('#').to_string();
        }
        let today_suffix = key.kind == PanelKind::Day && key.day_date(app.today) == Some(app.today);
        let mut rights: Vec<(String, Style, Option<Part>)> = Vec::new();
        if panel.vault != app.ui.vault_name {
            rights.push((panel.vault.clone(), th.s(Token::Muted), None));
        }
        if let Some(by) = &panel.opened_by {
            rights.push((format!("{} {}", g.agent, crate::ui::actor_word(by)), th.s(Token::Agent), None));
        }
        if folded {
            let count = app.panel_doc(key).map(|d| {
                let open = d.blocks().iter().filter(|l| l.kind() == Kind::Task && matches!(l.status.as_deref(), Some("todo" | "doing" | "waiting"))).count();
                if open > 0 { format!("{open} open") } else { format!("{} notes", d.blocks().iter().filter(|l| !l.text.trim().is_empty()).count()) }
            });
            if let Some(c) = count {
                rights.push((c, th.s(Token::Muted), None));
            }
        }
        if panel.pinned {
            rights.push(("pinned".into(), th.s(Token::Muted), Some(Part::Pinned)));
        }
        if (focused || hovered) && key.kind.is_doc() {
            rights.push((g.open_main.into(), th.s(if hovered { Token::Text } else { Token::Muted }), Some(Part::OpenMain)));
        }
        rights.push((g.close.into(), th.s(if hovered { Token::Text } else { Token::Muted }), Some(Part::Close)));
        let right_w: usize = rights.iter().map(|(t, _, _)| width(t)).sum::<usize>() + 2 * rights.len().saturating_sub(1);
        let left_w: usize = left.iter().map(|s| width(&s.content)).sum();
        let suffix = if today_suffix { " · today" } else { "" };
        let room = (s_w as usize).saturating_sub(left_w + right_w + 2 + 1);
        let shown_title = crate::ui::truncate_str(&title, room.saturating_sub(width(suffix)), g.ellipsis);
        let bold = focused || newest.as_ref() == Some(key);
        let mut tst = th.s(Token::Text).patch(fill);
        if bold {
            tst = tst.add_modifier(Modifier::BOLD);
        }
        if focused && th.is_ansi() {
            tst = tst.add_modifier(Modifier::UNDERLINED);
        }
        let mut targets = vec![(area.x + 1, area.x + (left_w + width(&shown_title)) as u16, Part::Title)];
        left.push(Span::styled(shown_title.clone(), tst));
        if !suffix.is_empty() {
            left.push(Span::styled(suffix, th.s(Token::Today).patch(fill)));
        }
        let used: usize = left.iter().map(|s| width(&s.content)).sum();
        let pad = (s_w as usize).saturating_sub(used + right_w + 1);
        left.push(Span::styled(" ".repeat(pad), fill));
        let mut x = area.x + (used + pad) as u16;
        let n = rights.len();
        for (k, (t, st, part)) in rights.into_iter().enumerate() {
            let tw = width(&t) as u16;
            if let Some(p) = part {
                targets.push((x, x + tw, p));
            }
            left.push(Span::styled(t, st.patch(fill)));
            x += tw;
            if k + 1 < n {
                left.push(Span::styled("  ", fill));
                x += 2;
            }
        }
        left.push(Span::styled(" ", fill));
        // A pinned panel opened again flashes (§2 rule 2).
        let flashing = app.ui.sidebar.flash.as_ref().is_some_and(|(k, at)| k == key && app.ui.now_ms.saturating_sub(*at) < 600);
        let header = if flashing && !th.is_ansi() {
            Line::from(left.into_iter().map(|s| Span::styled(s.content, s.style.bg(th.s(Token::Accent).fg.unwrap_or_default()))).collect::<Vec<_>>())
        } else {
            Line::from(left)
        };
        let header_y = y;
        y += 1;
        // ---- the body (§4.2)
        let body_y = y;
        let body_h = body_h.min(area.bottom().saturating_sub(y));
        let mut body: Vec<Line<'static>> = Vec::new();
        let mut view = None;
        let mut caret = None;
        let mut more_y = None;
        if !folded && body_h > 0 {
            let m = &measures[i];
            if let Some(why) = app.panels.get(key).and_then(|rt| rt.problem.clone()) {
                body.push(Line::from(vec![Span::raw("   "), Span::styled(format!("{why} · ⌥W close"), th.s(Token::Muted))]));
            } else if key.kind.is_doc() {
                let capped = body_h < m.natural && !focused && Some(key) != active.as_ref();
                let linked_row = m.linked > 0 && body_h >= 2 && !capped;
                let doc_h = body_h - linked_row as u16 - capped as u16;
                let (lines, cur) = doc_rows(app, key, view_w, doc_h.max(1));
                let r = Rect { x: area.x + 1, y: body_y, width: view_w, height: doc_h.max(1) };
                view = Some(r);
                caret = cur.map(|(cx, cy)| (r.x + cx, r.y + cy));
                body.extend(lines.into_iter().take(doc_h as usize));
                while body.len() < doc_h as usize {
                    body.push(Line::raw(""));
                }
                if capped {
                    let shown_rows = doc_h as usize;
                    let more = (m.natural as usize).saturating_sub(shown_rows);
                    more_y = Some(body_y + body.len() as u16);
                    body.push(Line::from(vec![Span::raw("   "), Span::styled(format!("{} {more} more", g.more), th.s(Token::Muted))]));
                } else if linked_row {
                    body.push(Line::from(vec![Span::raw("   "), Span::styled(format!("{} linked from {}", g.fold_closed, m.linked), th.s(Token::Muted))]));
                }
            } else {
                body.push(Line::from(vec![Span::raw("   "), Span::styled("lists beside you come in the next release", th.s(Token::Muted))]));
            }
        }
        y = body_y + body.len() as u16;
        out.push(PreparedPanel { key: key.clone(), header_y, header, header_targets: targets, body_y, body, view, caret, more_y });
        // A blank row between panels.
        y += 1;
    }
    app.derived.sidebar = Some(PreparedSidebar { area, panels: out, hidden });
}

/// A doc panel's rows at `w` × `h`, styled, and its caret's cell in the view.
fn doc_rows(app: &mut App, key: &PanelKey, w: u16, h: u16) -> (Vec<Line<'static>>, Option<(u16, u16)>) {
    lay_out(app, key, w, h);
    let th = app.theme;
    let journal = key.kind == PanelKind::Day;
    let mut out = Vec::new();
    let mut cursor = None;
    let forms: Vec<crate::doc_ui::Form> = match app.panel_doc(key) {
        Some(d) => d.blocks().iter().map(|l| crate::doc_ui::form(app, l)).collect(),
        None => return (out, None),
    };
    let now = app.ui.now_ms;
    // Rows changed elsewhere in the last 3 s get the live tint (§4.3).
    let live: std::collections::HashSet<String> = if th.is_ansi() { Default::default() } else { app.ui.flashes.iter().filter(|(_, (t, _))| now.saturating_sub(*t) < 3000).map(|(id, _)| id.clone()).collect() };
    let Some((d, _)) = app.panel_doc_mut(key) else { return (out, None) };
    let frame = d.frame();
    let sel = d.selection();
    let blocks = d.blocks();
    let empty = blocks.len() == 1 && blocks[0].text.is_empty() && blocks[0].is_new;
    let extra: std::collections::HashSet<usize> = frame.rows.iter().filter_map(|r| if let DocRow::Extra { line, index: 0 } = r { Some(*line) } else { None }).collect();
    for r in frame.rows.iter().take(h as usize) {
        match *r {
            DocRow::Text { line, start, end, first, .. } => {
                let l = &blocks[line];
                let fm = &forms[line];
                let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
                let mark = if l.conflict {
                    Span::styled("≠ ", th.s(Token::Conflict))
                } else if l.remote_text.is_some() {
                    Span::styled("◆ ", th.s(Token::Agent))
                } else if l.save_error.is_some() {
                    Span::styled("◌ ", th.s(Token::Muted))
                } else {
                    Span::raw("  ")
                };
                spans.push(if first { mark } else { Span::raw("  ") });
                spans.push(Span::raw(" ".repeat(l.depth * 4)));
                spans.push(if first { Span::styled(format!("{:>4}", fm.hang), fm.hang_style) } else { Span::raw("    ") });
                if empty {
                    let msg = if journal { "Nothing here yet · type to start the day" } else { "This page is empty." };
                    spans.push(Span::styled(msg, th.s(Token::Muted)));
                    out.push(Line::from(spans));
                    continue;
                }
                let start = if first { start.max(crate::doc_ui::marker_len(l)).min(end) } else { start };
                let text = &l.text[start..end];
                let (sa, sb) = match sel {
                    Some((s, e)) if line >= s.line && line <= e.line => {
                        let a = if line == s.line { s.byte.clamp(start, end) } else { start };
                        let b = if line == e.line { e.byte.clamp(start, end) } else { end };
                        (a - start, b - start)
                    }
                    _ => (0, 0),
                };
                let base = fm.text_style;
                if sa < sb {
                    spans.extend(crate::ui::text_spans(&th, &text[..sa], base, ""));
                    spans.push(Span::styled(text[sa..sb].to_string(), base.patch(th.fill(Token::Selection))));
                    spans.extend(crate::ui::text_spans(&th, &text[sb..], base, ""));
                } else {
                    spans.extend(crate::ui::text_spans(&th, text, base, ""));
                }
                if first && !l.meta.is_empty() && !extra.contains(&line) {
                    let used: usize = spans.iter().map(|s| width(&s.content)).sum();
                    let pad = (w as usize + 1).saturating_sub(used + width(&l.meta)).max(2);
                    let flashing = l.flash_until.is_some_and(|t| t > now);
                    spans.push(Span::raw(" ".repeat(pad)));
                    spans.push(Span::styled(l.meta.clone(), th.s(if flashing { Token::Accent } else { Token::Muted })));
                }
                if live.contains(&l.id) {
                    out.push(Line::from(spans).patch_style(th.fill(Token::AgentTint)));
                } else {
                    out.push(Line::from(spans));
                }
            }
            DocRow::Extra { line, index: 0 } if !blocks[line].meta.is_empty() => {
                let meta = blocks[line].meta.clone();
                let pad = (w as usize + 1).saturating_sub(width(&meta));
                out.push(Line::from(vec![Span::raw(" ".repeat(pad)), Span::styled(meta, th.s(Token::Muted))]));
            }
            DocRow::Gap { .. } | DocRow::Extra { .. } => out.push(Line::raw("")),
            DocRow::Past => {}
        }
    }
    if let Some((cx, cy)) = frame.cursor {
        cursor = Some((cx, cy));
    }
    app.main_view_current();
    (out, cursor)
}

/// Draw the prepared sidebar in `area`, with the divider left of it.
pub fn draw(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect, divider: Option<Rect>) {
    let th = app.theme;
    let g = th.glyphs();
    if let Some(dv) = divider {
        let lines: Vec<Line> = (0..dv.height).map(|_| Line::styled(g.vsep, th.s(Token::Line))).collect();
        f.render_widget(Paragraph::new(lines), dv);
    }
    let Some(p) = app.derived.sidebar.as_ref() else { return };
    if p.area != area {
        return;
    }
    let sidebar_focus = app.ui.focus == Focus::Sidebar;
    let active = app.ui.sidebar.active_key();
    for (i, pp) in p.panels.iter().enumerate() {
        if pp.header_y >= area.bottom() {
            break;
        }
        f.render_widget(Paragraph::new(pp.header.clone()), Rect { x: area.x, y: pp.header_y, width: area.width, height: 1 });
        for &(x0, x1, part) in &pp.header_targets {
            crate::ui::target(render, x0, x1, pp.header_y, Click::Panel(i, part));
        }
        let h = (pp.body.len() as u16).min(area.bottom().saturating_sub(pp.body_y));
        if h > 0 {
            f.render_widget(Paragraph::new(pp.body.clone()), Rect { x: area.x, y: pp.body_y, width: area.width, height: h });
        }
        if let Some(my) = pp.more_y.filter(|y| *y < area.bottom()) {
            crate::ui::target(render, area.x, area.right(), my, Click::Panel(i, Part::More));
        }
        if let Some(v) = pp.view {
            render.panel_views.push((pp.key.clone(), v));
        }
        // The caret: the live one in the focused panel, a cell of `selection` elsewhere (§5.1).
        if let Some((cx, cy)) = pp.caret.filter(|&(_, y)| y < area.bottom() && pp.view.is_some_and(|v| y < v.bottom())) {
            let here = sidebar_focus && active.as_ref() == Some(&pp.key);
            if here && crate::ui::caret_allowed(app) {
                f.set_cursor_position((cx.min(area.right() - 1), cy));
            } else {
                unfocused_caret(f, &th, cx.min(area.right() - 1), cy);
            }
        }
    }
    // A header being dragged: an accent `━` where it would drop (§3.5).
    if let Some(crate::sidebar_app::Drag::Header { to: Some(at), key, .. }) = &app.sidebar_drag {
        let others: Vec<&PreparedPanel> = p.panels.iter().filter(|pp| pp.key != *key).collect();
        let y = match others.get(*at) {
            Some(pp) => pp.header_y.saturating_sub(1),
            None => others.last().map_or(area.y, |pp| pp.body_y + pp.body.len() as u16),
        };
        if y >= area.y && y < area.bottom() {
            let st = if th.is_ansi() { Style::default().add_modifier(Modifier::BOLD) } else { th.s(Token::Accent) };
            f.render_widget(Paragraph::new(Line::styled("━".repeat(area.width as usize), st)), Rect { x: area.x, y, width: area.width, height: 1 });
        }
    }
    if p.hidden > 0 {
        let y = p.panels.last().map_or(area.y, |pp| pp.body_y + pp.body.len() as u16 + 1).min(area.bottom().saturating_sub(1));
        f.render_widget(Paragraph::new(Line::from(vec![Span::raw("   "), Span::styled(format!("{} {} more panels", g.more, p.hidden), th.s(Token::Muted))])), Rect { x: area.x, y, width: area.width, height: 1 });
    }
}

/// The drawer (90–119 columns, §6.4): the same sidebar over the right of the main view, on the
/// `raised` background, with a `│` left edge (`┬` on the tab rule).
pub fn draw_drawer(render: &mut RenderOutput, f: &mut Frame, app: &App, rect: Rect, rule_y: u16) {
    let th = app.theme;
    let g = th.glyphs();
    f.render_widget(ratatui::widgets::Clear, rect);
    let raised = th.fill(Token::Raised);
    f.render_widget(Paragraph::new("").style(raised), rect);
    let buf = f.buffer_mut();
    if buf.area.contains((rect.x, rule_y).into()) {
        buf[(rect.x, rule_y)].set_symbol(g.tee).set_style(th.s(Token::Line));
    }
    let edge = Rect { width: 1, ..rect };
    let lines: Vec<Line> = (0..edge.height).map(|_| Line::styled(g.vsep, th.s(Token::Line).patch(raised))).collect();
    f.render_widget(Paragraph::new(lines), edge);
    let inner = Rect { x: rect.x + 1, width: rect.width - 1, ..rect };
    draw(render, f, app, inner, None);
    patch_bg(f, inner, raised);
    // Clicks under the drawer belong to it, not to the main view beneath.
    render.click_targets.retain(|t| !(t.y >= rect.y && t.y < rect.bottom() && t.x0 >= rect.x && t.x1 <= rect.right()) || matches!(t.what, Click::Panel(..)));
}

/// Replace (under 90 columns, §6.4): the sidebar takes the body, under its back row
/// `‹ § Wed 07 Oct   beside it: 2 panels`.
pub fn draw_replace(render: &mut RenderOutput, f: &mut Frame, app: &App, rect: Rect) {
    let th = app.theme;
    let g = th.glyphs();
    f.render_widget(ratatui::widgets::Clear, rect);
    let here = main_name(app);
    let n = app.ui.sidebar.open.len();
    let back = Line::from(vec![
        Span::raw(" "),
        Span::styled(g.back, th.s(Token::Accent)),
        Span::raw(" "),
        Span::styled(here.clone(), th.s(Token::Muted).add_modifier(Modifier::UNDERLINED)),
        Span::styled(format!("   beside it: {n} panel{}", if n == 1 { "" } else { "s" }), th.s(Token::Muted)),
    ]);
    f.render_widget(Paragraph::new(back), Rect { height: 1, ..rect });
    crate::ui::target(render, rect.x + 1, rect.x + 3 + width(&here) as u16, rect.y, Click::Action("sidebar.back"));
    draw(render, f, app, Rect { y: rect.y + 1, height: rect.height.saturating_sub(1), ..rect }, None);
}

/// The main view as the back row names it: `§ Wed 07 Oct`, `¶ Health`, `Today`.
fn main_name(app: &App) -> String {
    let g = app.theme.glyphs();
    match app.doc.as_ref().map(|d| &d.target) {
        Some(crate::editor::Target::Journal { date }) => format!("{} {}", g.journal, date.format("%a %d %b")),
        Some(crate::editor::Target::Page { title, .. }) => format!("{} {title}", g.page),
        None => format!("{:?}", app.view),
    }
}

/// Every cell of `r` without a background gets `fill`'s.
fn patch_bg(f: &mut Frame, r: Rect, fill: Style) {
    let Some(bg) = fill.bg else { return };
    let buf = f.buffer_mut();
    for y in r.y..r.bottom() {
        for x in r.x..r.right() {
            if buf.area.contains((x, y).into()) {
                let c = &mut buf[(x, y)];
                if c.bg == ratatui::style::Color::Reset {
                    c.set_bg(bg);
                }
            }
        }
    }
}

/// An unfocused view's caret: one cell of `selection` (ANSI: underlined).
pub(crate) fn unfocused_caret(f: &mut Frame, th: &crate::theme::Theme, x: u16, y: u16) {
    let buf = f.buffer_mut();
    if !buf.area.contains((x, y).into()) {
        return;
    }
    let cell = &mut buf[(x, y)];
    match th.fill(Token::Selection).bg {
        Some(bg) if !th.is_ansi() => {
            cell.set_bg(bg);
        }
        _ => {
            let st = cell.style().add_modifier(Modifier::UNDERLINED);
            cell.set_style(st);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::share;

    #[test]
    fn height_is_shared_as_specified() {
        // Everything fits: natural heights.
        assert_eq!(share(&[(5, false, true), (4, false, false)], 20), (vec![Some(5), Some(4)], 0));
        // Too tall: the others cap at 6, the active fills the rest.
        let (b, _) = share(&[(30, false, false), (30, false, true), (30, false, false)], 37);
        assert_eq!(b, vec![Some(6), Some(20), Some(6)]);
        // Tiny: the active keeps 3, the rest fold, then drop.
        let (b, hidden) = share(&[(30, false, true), (30, false, false), (30, false, false), (30, false, false)], 9);
        assert_eq!(b[0], Some(3));
        assert!(hidden > 0 || b[1..].iter().all(|x| *x == Some(0)));
    }
}
