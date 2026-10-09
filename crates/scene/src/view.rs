//! The view: `State` in, a frame out. It reads nothing else (no store, clock, env or files).

use crate::model::{Kind, Node, Size, Style as NodeStyle, fill, lookup, scalar};
use crate::state::{Slot, State, bound, items, selected};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, List, ListItem, ListState, Paragraph, Row, Sparkline, Table, TableState, Wrap};

pub fn draw(st: &State, f: &mut Frame) {
    let [body, status] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(f.area());
    node(st, &st.ui.root, f, body);
    let line = match &st.status {
        Some(s) => Line::styled(s.clone(), Style::default().fg(Color::Red)),
        None => Line::styled(
            format!(" ui v{} · tab focus · j/k move · r refresh · q quit", st.version),
            Style::default().add_modifier(Modifier::DIM),
        ),
    };
    let s = &st.stats;
    let meter = format!("{:>5.1} fps · draw {:.2}ms (max {:.1}) · {:>6.0} msg/s ", s.fps, s.draw_ms, s.max_ms, s.msgs);
    let [left, right] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(meter.chars().count() as u16)]).areas(status);
    f.render_widget(Paragraph::new(line), left);
    f.render_widget(Paragraph::new(Line::styled(meter, Style::default().fg(Color::Yellow))), right);
}

fn node(st: &State, n: &Node, f: &mut Frame, area: Rect) {
    let focused = n.id.is_some() && n.id == st.focus;
    let inner = match frame(n, focused) {
        Some(b) => {
            let inner = b.inner(area);
            f.render_widget(b, area);
            inner
        }
        None => area,
    };
    let style = style(n.style.as_ref());
    // A bound node whose source hasn't arrived (or failed) says so instead of drawing.
    if let Some(bind) = &n.bind {
        let name = crate::state::split_bind(bind).0;
        match st.data.get(name) {
            Some(Slot::Error { message }) => {
                let p = Paragraph::new(format!("{name}: {message}")).style(Style::default().fg(Color::Red));
                return f.render_widget(p.wrap(Wrap { trim: true }), inner);
            }
            Some(Slot::Ready { .. }) => {}
            _ if st.ui.data.contains_key(name) => {
                return f.render_widget(Paragraph::new("…").style(Style::default().add_modifier(Modifier::DIM)), inner);
            }
            _ => {}
        }
    }
    match &n.kind {
        Kind::Col { children } | Kind::Row { children } => {
            let cs: Vec<Constraint> = children.iter().map(|c| constraint(c.size.as_ref())).collect();
            let areas = match n.kind {
                Kind::Col { .. } => Layout::vertical(cs).split(inner),
                _ => Layout::horizontal(cs).split(inner),
            };
            for (c, a) in children.iter().zip(areas.iter()) {
                node(st, c, f, *a);
            }
        }
        Kind::Text { text } => {
            let lines: Vec<Line> = fill(text, bound(st, n).unwrap_or(&serde_json::Value::Null)).lines().map(|l| Line::from(l.to_string())).collect();
            f.render_widget(Paragraph::new(lines).style(style).wrap(Wrap { trim: false }), inner);
        }
        Kind::List { item, empty, .. } => {
            let rows = items(st, n);
            if rows.is_empty() {
                let e = empty.clone().unwrap_or_else(|| "(nothing)".into());
                return f.render_widget(Paragraph::new(e).style(Style::default().add_modifier(Modifier::DIM)), inner);
            }
            let (from, sel) = window(n.id.as_ref().map(|id| selected(st, id)), inner.height as usize);
            let list: Vec<ListItem> = rows
                .iter()
                .skip(from)
                .take(inner.height as usize)
                .map(|r| ListItem::new(match item {
                    Some(t) => fill(t, r),
                    None => scalar(Some(r)),
                }))
                .collect();
            let mut ls = ListState::default().with_selected(sel);
            let w = List::new(list).style(style).highlight_style(highlight(focused)).highlight_symbol("▸ ");
            f.render_stateful_widget(w, inner, &mut ls);
        }
        Kind::Table { columns, .. } => {
            let rows = items(st, n);
            let header = Row::new(columns.iter().map(|c| c.title.clone())).style(Style::default().add_modifier(Modifier::BOLD));
            let visible = (inner.height as usize).saturating_sub(1);
            let (from, sel) = window(n.id.as_ref().map(|id| selected(st, id)), visible);
            let body = rows.iter().skip(from).take(visible).map(|r| Row::new(columns.iter().map(|c| fill(&c.value, r))));
            let widths: Vec<Constraint> = columns.iter().map(|c| constraint(c.size.as_ref())).collect();
            let mut ts = TableState::default().with_selected(sel);
            let t = Table::new(body, widths).header(header).style(style).row_highlight_style(highlight(focused)).highlight_symbol("▸ ");
            f.render_stateful_widget(t, inner, &mut ts);
        }
        Kind::Sparkline { values, field } => {
            let nums: Vec<f64> = if !values.is_empty() {
                values.clone()
            } else {
                match bound(st, n) {
                    Some(serde_json::Value::Array(a)) => a
                        .iter()
                        .filter_map(|v| match field {
                            Some(p) => lookup(v, p).and_then(|x| x.as_f64()),
                            None => v.as_f64(),
                        })
                        .collect(),
                    _ => vec![],
                }
            };
            // Sparklines take whole numbers: scale the range onto 0..=100.
            let (lo, hi) = nums.iter().fold((f64::MAX, f64::MIN), |(a, b), &x| (a.min(x), b.max(x)));
            let span = if hi > lo { hi - lo } else { 1.0 };
            let data: Vec<u64> = nums.iter().map(|x| (((x - lo) / span) * 100.0).round() as u64 + 1).collect();
            f.render_widget(Sparkline::default().data(&data).style(style), inner);
        }
    }
}

/// Only the rows that fit are built: the first one shown, and the selection within them. The
/// selection sits on the last visible row once it passes the bottom (what ratatui does itself).
fn window(selected: Option<usize>, height: usize) -> (usize, Option<usize>) {
    let sel = selected.unwrap_or(0);
    let from = (sel + 1).saturating_sub(height.max(1));
    (from, selected.map(|s| s - from))
}

fn frame(n: &Node, focused: bool) -> Option<Block<'static>> {
    if n.title.is_none() && !n.border {
        return None;
    }
    let mut b = Block::bordered().border_type(BorderType::Rounded);
    if let Some(t) = &n.title {
        b = b.title(format!(" {t} "));
    }
    let edge = if focused { Style::default().fg(Color::Cyan) } else { Style::default().add_modifier(Modifier::DIM) };
    Some(b.border_style(edge))
}

fn highlight(focused: bool) -> Style {
    if focused {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    }
}

pub fn constraint(s: Option<&Size>) -> Constraint {
    match s {
        None => Constraint::Fill(1),
        Some(Size::Cells(n)) => Constraint::Length(*n),
        Some(Size::Spec(s)) => {
            let s = s.trim();
            if let Some(p) = s.strip_suffix('%') {
                p.parse().map(Constraint::Percentage).unwrap_or(Constraint::Fill(1))
            } else if let Some(w) = s.strip_suffix('*') {
                Constraint::Fill(if w.is_empty() { 1 } else { w.parse().unwrap_or(1) })
            } else {
                s.parse().map(Constraint::Length).unwrap_or(Constraint::Fill(1))
            }
        }
    }
}

fn style(s: Option<&NodeStyle>) -> Style {
    let Some(s) = s else { return Style::default() };
    let mut out = Style::default();
    if let Some(c) = s.fg.as_deref().and_then(color) {
        out = out.fg(c);
    }
    if let Some(c) = s.bg.as_deref().and_then(color) {
        out = out.bg(c);
    }
    for (on, m) in [(s.bold, Modifier::BOLD), (s.dim, Modifier::DIM), (s.italic, Modifier::ITALIC)] {
        if on {
            out = out.add_modifier(m);
        }
    }
    out
}

fn color(name: &str) -> Option<Color> {
    name.parse().ok()
}
