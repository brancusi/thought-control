//! The view: `State` in, a frame out. It reads nothing else (no store, clock, env or files).
//! Besides the frame it returns the hit regions (what is under each cell, for the mouse), and
//! it places the UI's layers with caretline-layers over what it drew.

use crate::model::{Kind, LayerSpec, Look, Node, Seg, Size, Style as NodeStyle, fill, lookup, number, scalar, segments};
use crate::state::{Slot, State, bound, items, selected, split_bind, tab_of, value_of};
use caretline_layers as cl;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Map, MapResolution, Points};
use ratatui::widgets::{Axis, Block, BorderType, Chart, Dataset, Gauge, GraphType, LegendPosition, Paragraph, Sparkline, Wrap};
use serde_json::{Value, json};

/// What a cell belongs to: a node, and maybe one of its rows (or bars, or points) or tabs.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub rect: Rect,
    pub id: String,
    pub row: Option<usize>,
    pub tab: Option<usize>,
}

/// The screen's colours: the UI's `theme`, over these defaults.
struct Theme {
    bg: Option<Color>,
    fg: Option<Color>,
    border: Color,
    accent: Color,
    dim: Color,
    pos: Color,
    neg: Color,
    hover: Color,
    panel: Color,
    ring: Color,
}

struct Cx<'a> {
    st: &'a State,
    th: Theme,
    hits: Vec<Hit>,
    anchors: cl::AnchorMap,
}

impl Cx<'_> {
    /// A colour by theme name, CSS-ish name or `#rrggbb`.
    fn color(&self, name: &str) -> Option<Color> {
        let name = self.st.ui.theme.get(name).map(String::as_str).unwrap_or(name);
        name.parse().ok()
    }

    fn style(&self, s: Option<&NodeStyle>) -> Style {
        let mut out = Style::default();
        let Some(s) = s else { return out };
        if let Some(c) = s.fg.as_deref().and_then(|c| self.color(c)) {
            out = out.fg(c);
        }
        if let Some(c) = s.bg.as_deref().and_then(|c| self.color(c)) {
            out = out.bg(c);
        }
        for (on, m) in [(s.bold, Modifier::BOLD), (s.dim, Modifier::DIM), (s.italic, Modifier::ITALIC)] {
            if on {
                out = out.add_modifier(m);
            }
        }
        out
    }

    fn mark(&mut self, id: &str, row: Option<usize>, tab: Option<usize>, rect: Rect) {
        self.hits.push(Hit { rect, id: id.to_string(), row, tab });
        let key = match row {
            Some(r) => cl::AnchorKey::host("row", &format!("{id}#{r}")),
            None if tab.is_some() => return,
            None => cl::AnchorKey::host("node", id),
        };
        self.anchors.put(key, cl::Rect::new(rect.x, rect.y, rect.width, rect.height));
    }

    fn hovered(&self, id: &str, row: usize) -> bool {
        self.st.hover.as_ref().is_some_and(|h| h.id == id && h.row == Some(row))
    }

    /// Spans from a template: `{±x}` coloured by sign.
    /// Spans from a template (tabs and fills dropped): `{±x}` coloured by sign, `<spec>` styled.
    fn spans(&self, template: &str, v: &Value, base: Style) -> Vec<Span<'static>> {
        self.parts(template, v, base)
            .into_iter()
            .filter_map(|p| match p {
                Part::Text(s) => Some(s),
                _ => None,
            })
            .collect()
    }

    fn parts(&self, template: &str, v: &Value, base: Style) -> Vec<Part> {
        segments(template, v).into_iter().map(|g| self.part(g, base)).collect()
    }

    fn part(&self, g: Seg, base: Style) -> Part {
        let mut st = match &g.style {
            Some(spec) => self.spec(spec, base),
            None => base,
        };
        match g.sign {
            Some(true) => st = st.fg(self.th.pos),
            Some(false) => st = st.fg(self.th.neg),
            None => {}
        }
        if g.tab {
            Part::Tab
        } else if let Some(c) = g.fill {
            Part::Fill(c, st)
        } else {
            Part::Text(Span::styled(g.text, st))
        }
    }

    /// A style spec over `base`: `accent+b`, `muted`, `text+on-selection`, `#ff8800+u`.
    fn spec(&self, spec: &str, base: Style) -> Style {
        let mut st = base;
        for t in spec.split('+') {
            st = match t {
                "b" => st.add_modifier(Modifier::BOLD),
                "d" => st.add_modifier(Modifier::DIM),
                "i" => st.add_modifier(Modifier::ITALIC),
                "u" => st.add_modifier(Modifier::UNDERLINED),
                "r" => st.add_modifier(Modifier::REVERSED),
                "s" => st.add_modifier(Modifier::CROSSED_OUT),
                t => match t.strip_prefix("on-") {
                    Some(c) => self.color(c).map_or(st, |c| st.bg(c)),
                    None => self.color(t).map_or(st, |c| st.fg(c)),
                },
            };
        }
        st
    }
}

/// A run of a drawn line: text, the point where the right-aligned rest starts, or a fill.
enum Part {
    Text(Span<'static>),
    Tab,
    Fill(char, Style),
}

/// One line of parts into `r`: the left side from `r.x`, anything after a tab right-aligned to
/// the end (the left side is cut to leave a cell between them), and a fill taking what's left.
fn draw_parts(buf: &mut Buffer, r: Rect, parts: Vec<Part>) {
    let (mut left, mut right, mut seen_tab) = (Vec::new(), Vec::new(), false);
    for p in parts {
        match p {
            Part::Tab if !seen_tab => seen_tab = true,
            Part::Tab => {}
            p if seen_tab => right.push(p),
            p => left.push(p),
        }
    }
    let width = |ps: &[Part]| ps.iter().map(|p| if let Part::Text(s) = p { s.width() } else { 0 }).sum::<usize>();
    let rw = width(&right).min(r.width as usize);
    let room = (r.width as usize).saturating_sub(if rw > 0 { rw + 1 } else { 0 });
    let fixed = width(&left);
    let mut x = r.x;
    let end = r.x + room as u16;
    for p in left {
        if x >= end {
            break;
        }
        match p {
            Part::Text(s) => {
                let (nx, _) = buf.set_span(x, r.y, &s, end - x);
                x = nx;
            }
            Part::Fill(c, st) => {
                let n = room.saturating_sub(fixed) as u16;
                let n = n.min(end - x);
                buf.set_string(x, r.y, c.to_string().repeat(n as usize), st);
                x += n;
            }
            Part::Tab => {}
        }
    }
    let mut x = r.x + r.width - rw as u16;
    for p in right {
        if let Part::Text(s) = p {
            let (nx, _) = buf.set_span(x, r.y, &s, r.right().saturating_sub(x));
            x = nx;
        }
    }
}

fn theme(st: &State) -> Theme {
    let get = |k: &str, d: Color| st.ui.theme.get(k).and_then(|c| c.parse().ok()).unwrap_or(d);
    Theme {
        bg: st.ui.theme.get("bg").and_then(|c| c.parse().ok()),
        fg: st.ui.theme.get("fg").and_then(|c| c.parse().ok()),
        border: get("border", Color::Rgb(0x4a, 0x50, 0x60)),
        accent: get("accent", Color::Rgb(0x5f, 0xb3, 0xff)),
        dim: get("dim", Color::Rgb(0x55, 0x58, 0x60)),
        pos: get("pos", Color::Rgb(0x4c, 0xd1, 0x7f)),
        neg: get("neg", Color::Rgb(0xf2, 0x5f, 0x5c)),
        hover: get("hover", Color::Rgb(0x26, 0x2c, 0x3a)),
        panel: get("panel", Color::Rgb(0x1f, 0x24, 0x30)),
        ring: get("ring", Color::Rgb(0x26, 0x3a, 0x55)),
    }
}

/// Draw the whole screen; returns what's under each cell.
pub fn draw(st: &State, f: &mut Frame) -> Vec<Hit> {
    let mut cx = Cx { st, th: theme(st), hits: Vec::new(), anchors: cl::AnchorMap::new() };
    let mut base = Style::default();
    if let Some(bg) = cx.th.bg {
        base = base.bg(bg);
    }
    if let Some(fg) = cx.th.fg {
        base = base.fg(fg);
    }
    let all = f.area();
    f.buffer_mut().set_style(all, base);
    let bar = st.ui.status_bar.unwrap_or(true);
    let [body, status] = Layout::vertical([Constraint::Fill(1), Constraint::Length(bar as u16)]).areas(f.area());
    node(&mut cx, &st.ui.root, f, body);
    layers(&cx, f.buffer_mut(), body);
    if !bar {
        return cx.hits;
    }
    let line = match &st.status {
        Some(s) => Line::styled(s.clone(), Style::default().fg(cx.th.neg)),
        None => Line::styled(
            format!(" ui v{} · tab focus · j/k move · ←/→ tabs · click, wheel, hover · q quit", st.version),
            Style::default().fg(cx.th.dim),
        ),
    };
    let s = &st.stats;
    let meter = format!("{:>5.1} fps · draw {:.2}ms (max {:.1}) · {:>6.0} msg/s ", s.fps, s.draw_ms, s.max_ms, s.msgs);
    let [left, right] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(meter.chars().count() as u16)]).areas(status);
    f.render_widget(Paragraph::new(line), left);
    f.render_widget(Paragraph::new(Line::styled(meter, Style::default().fg(Color::Yellow))), right);
    cx.hits
}

fn border_type(edge: Option<&str>) -> BorderType {
    match edge {
        Some("double") => BorderType::Double,
        Some("thick") => BorderType::Thick,
        Some("plain") => BorderType::Plain,
        _ => BorderType::Rounded,
    }
}

fn node(cx: &mut Cx, n: &Node, f: &mut Frame, area: Rect) {
    let st = cx.st;
    let focused = n.id.is_some() && n.id == st.focus;
    if let Some(id) = &n.id {
        cx.mark(id, None, None, area);
    }
    let style = cx.style(n.style.as_ref());
    let inner = if n.title.is_some() || n.border || n.edge.is_some() {
        let edge = if focused { cx.th.accent } else { cx.th.border };
        let mut b = Block::bordered().border_type(border_type(n.edge.as_deref())).border_style(Style::default().fg(edge));
        if let Some(bg) = n.style.as_ref().and_then(|s| s.bg.as_deref()).and_then(|c| cx.color(c)) {
            b = b.style(Style::default().bg(bg));
        }
        if let Some(t) = &n.title {
            let ts = if focused { Style::default().fg(cx.th.accent).add_modifier(Modifier::BOLD) } else { Style::default().fg(cx.th.fg.unwrap_or(Color::Reset)) };
            b = b.title(Span::styled(format!(" {t} "), ts));
        }
        let inner = b.inner(area);
        f.render_widget(b, area);
        inner
    } else {
        if let Some(bg) = style.bg {
            f.buffer_mut().set_style(area, Style::default().bg(bg));
        }
        area
    };
    // A bound node whose source hasn't arrived (or failed) says so instead of drawing.
    if let Some(bind) = n.bind.as_ref().filter(|b| !b.starts_with('@')) {
        let name = split_bind(bind).0;
        match st.data.get(name) {
            Some(Slot::Error { message }) => {
                let p = Paragraph::new(format!("{name}: {message}")).style(Style::default().fg(cx.th.neg));
                return f.render_widget(p.wrap(Wrap { trim: true }), inner);
            }
            Some(Slot::Ready { .. }) => {}
            _ if st.ui.data.contains_key(name) => {
                return f.render_widget(Paragraph::new("…").style(Style::default().fg(cx.th.dim)), inner);
            }
            _ => {}
        }
    }
    let align = match n.align.as_deref() {
        Some("center") => Alignment::Center,
        Some("right") => Alignment::Right,
        _ => Alignment::Left,
    };
    let id = n.id.clone().unwrap_or_default();
    match &n.kind {
        Kind::Col { children } | Kind::Row { children } => {
            let cs: Vec<Constraint> = children.iter().map(|c| constraint(c.size.as_ref())).collect();
            let areas = match n.kind {
                Kind::Col { .. } => Layout::vertical(cs).split(inner),
                _ => Layout::horizontal(cs).split(inner),
            };
            for (c, a) in children.iter().zip(areas.iter()) {
                node(cx, c, f, *a);
            }
        }
        Kind::Text { text } => {
            let v = bound(st, n).unwrap_or(&Value::Null);
            // Lines with a tab or a fill are laid out by hand (no wrapping); the rest wrap.
            // Lines whose placeholders all came out empty are left out (optional details).
            let shown: Vec<&str> = text.split('\n').filter(|l| crate::model::shows(l, v)).collect();
            if text.contains('\t') || text.contains("{*") {
                for (k, l) in shown.iter().enumerate().take(inner.height as usize) {
                    let r = Rect::new(inner.x, inner.y + k as u16, inner.width, 1);
                    draw_parts(f.buffer_mut(), r, cx.parts(l, v, style));
                }
            } else {
                let lines: Vec<Line> = shown.iter().map(|l| Line::from(cx.spans(l, v, style))).collect();
                f.render_widget(Paragraph::new(lines).style(style).alignment(align).wrap(Wrap { trim: false }), inner);
            }
        }
        Kind::Rule { glyph } => {
            let g = glyph.clone().unwrap_or_else(|| if inner.height > inner.width { "│".into() } else { "─".into() });
            let ls = if style.fg.is_some() { style } else { style.fg(cx.th.border) };
            for y in inner.y..inner.bottom() {
                f.buffer_mut().set_string(inner.x, y, g.repeat(inner.width as usize), ls);
            }
        }
        Kind::List { item, empty, variant, variants, mark, lead, selected: sel_spec, .. } => {
            let rows = items(st, n);
            if rows.is_empty() {
                let e = empty.clone().unwrap_or_else(|| "(nothing)".into());
                return f.render_widget(Paragraph::new(e).style(Style::default().fg(cx.th.dim)), inner);
            }
            let sel = n.id.as_ref().map(|id| selected(st, id));
            let (from, _) = window(sel, inner.height as usize);
            for (k, r) in rows.iter().enumerate().skip(from).take(inner.height as usize) {
                let y = inner.y + (k - from) as u16;
                let line = Rect::new(inner.x, y, inner.width, 1);
                let mut rs = style;
                if cx.hovered(&id, k) {
                    rs = rs.bg(cx.th.hover);
                }
                let picked = variant.as_ref().and_then(|p| variants.get(&scalar(lookup(r, p))));
                let is_sel = sel == Some(k) && !n.skips(r);
                if is_sel {
                    rs = match sel_spec {
                        Some(spec) => cx.spec(spec, rs),
                        None if focused => rs.add_modifier(Modifier::REVERSED),
                        None => rs.add_modifier(Modifier::BOLD),
                    };
                }
                f.buffer_mut().set_style(line, rs);
                let mut parts = Vec::new();
                if picked.is_none() || !n.skips(r) {
                    let lead_t = if is_sel { mark.as_deref().unwrap_or("▸ ") } else { lead.as_deref().unwrap_or("  ") };
                    parts.extend(cx.parts(lead_t, r, rs));
                }
                match picked.or(item.as_ref()) {
                    Some(t) => parts.extend(cx.parts(t, r, rs)),
                    None => parts.push(Part::Text(Span::styled(scalar(Some(r)), rs))),
                }
                draw_parts(f.buffer_mut(), line, parts);
                if !id.is_empty() {
                    cx.mark(&id, Some(k), None, line);
                }
            }
        }
        Kind::Table { columns, .. } => {
            let rows = items(st, n);
            let widths: Vec<Constraint> = columns.iter().map(|c| constraint(c.size.as_ref())).collect();
            if inner.height == 0 {
                return;
            }
            // Column positions once; each row reuses them at its own y.
            let spans = Layout::horizontal(widths).spacing(1).split(Rect::new(inner.x + 2, inner.y, inner.width.saturating_sub(2), 1));
            let cols = |y: u16| spans.iter().map(move |a| Rect { y, ..*a });
            let head = Style::default().fg(cx.th.accent).add_modifier(Modifier::BOLD);
            for (c, a) in columns.iter().zip(cols(inner.y)) {
                let al = if c.align.as_deref() == Some("right") { Alignment::Right } else { Alignment::Left };
                f.render_widget(Paragraph::new(c.title.clone()).style(head).alignment(al), a);
            }
            let visible = (inner.height as usize).saturating_sub(1);
            let sel = n.id.as_ref().map(|id| selected(st, id));
            let (from, _) = window(sel, visible);
            for (k, r) in rows.iter().enumerate().skip(from).take(visible) {
                let y = inner.y + 1 + (k - from) as u16;
                let line = Rect::new(inner.x, y, inner.width, 1);
                let mut rs = style;
                if cx.hovered(&id, k) {
                    rs = rs.bg(cx.th.hover);
                }
                if sel == Some(k) {
                    rs = if focused { rs.bg(cx.th.accent).fg(Color::Black) } else { rs.add_modifier(Modifier::BOLD) };
                }
                f.buffer_mut().set_style(line, rs);
                if sel == Some(k) {
                    f.buffer_mut().set_string(inner.x, y, "▸", rs);
                }
                for (c, a) in columns.iter().zip(cols(y)) {
                    let mut cs = rs;
                    if let Some(h) = &c.heat
                        && let Some(v) = number(&fill(&h.value, r))
                    {
                        cs = cs.bg(heat(v, h.min, h.max, cx.th.neg, cx.th.pos, cx.th.panel)).fg(Color::White);
                        f.buffer_mut().set_style(a, cs);
                    }
                    let al = if c.align.as_deref() == Some("right") { Alignment::Right } else { Alignment::Left };
                    let line = Line::from(cx.spans(&c.value, r, cs)).alignment(al);
                    f.buffer_mut().set_line(line_x(&line, a), y, &line, a.width);
                }
                if !id.is_empty() {
                    cx.mark(&id, Some(k), None, line);
                }
            }
        }
        Kind::Sparkline { values, field } => {
            let nums = numbers(values, bound(st, n), field.as_deref());
            let (lo, hi) = nums.iter().fold((f64::MAX, f64::MIN), |(a, b), &x| (a.min(x), b.max(x)));
            let span = if hi > lo { hi - lo } else { 1.0 };
            let data: Vec<u64> = nums.iter().map(|x| (((x - lo) / span) * 100.0).round() as u64 + 1).collect();
            let skip = data.len().saturating_sub(inner.width as usize);
            f.render_widget(Sparkline::default().data(&data[skip..]).style(style), inner);
        }
        Kind::Chart { series, labels } => {
            let data: Vec<Vec<(f64, f64)>> = series
                .iter()
                .map(|s| {
                    numbers(&[], value_of(st, &s.bind), s.field.as_deref()).into_iter().enumerate().map(|(i, y)| (i as f64, y)).collect()
                })
                .collect();
            let all = data.iter().flatten();
            let (lo, hi) = all.clone().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
            let xmax = data.iter().map(|d| d.len()).max().unwrap_or(1).saturating_sub(1).max(1) as f64;
            if lo > hi {
                return;
            }
            let pad = ((hi - lo) * 0.08).max(1e-9);
            let (lo, hi) = (lo - pad, hi + pad);
            let sets: Vec<Dataset> = series
                .iter()
                .zip(data.iter())
                .map(|(s, d)| {
                    let c = s.color.as_deref().and_then(|c| cx.color(c)).unwrap_or(cx.th.accent);
                    Dataset::default().name(s.name.clone()).marker(Marker::Braille).graph_type(GraphType::Line).style(Style::default().fg(c)).data(d)
                })
                .collect();
            let axis = Style::default().fg(cx.th.dim);
            let ylabels = vec![compact(lo), compact((lo + hi) / 2.0), compact(hi)];
            let chart = Chart::new(sets)
                .style(style)
                .legend_position(Some(LegendPosition::TopLeft))
                .x_axis(Axis::default().style(axis).bounds([0.0, xmax]).labels(labels.clone()))
                .y_axis(Axis::default().style(axis).bounds([lo, hi]).labels(ylabels));
            f.render_widget(chart, inner);
        }
        Kind::Bars { label, value, width, .. } => bars(cx, n, f, inner, label, value, *width, &id),
        Kind::Gauge { value, max, label } => {
            let v = bound(st, n).unwrap_or(&Value::Null);
            let x = number(&fill(value, v)).unwrap_or(0.0);
            let ratio = (x / max.unwrap_or(1.0)).clamp(0.0, 1.0);
            let text = label.as_ref().map(|l| fill(l, v)).unwrap_or_else(|| format!("{:.0}%", ratio * 100.0));
            let fg = style.fg.unwrap_or(cx.th.accent);
            let g = Gauge::default()
                .gauge_style(Style::default().fg(fg).bg(cx.th.panel))
                .ratio(ratio)
                .use_unicode(true)
                .label(Span::styled(text, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)));
            f.render_widget(g, inner);
        }
        Kind::Big { text } => {
            let v = bound(st, n).unwrap_or(&Value::Null);
            let segs = segments(text, v);
            let sign = segs.iter().find_map(|g| g.sign);
            let s: String = segs.into_iter().map(|g| g.text).collect();
            let mut bs = style;
            if bs.fg.is_none() {
                bs = bs.fg(match sign {
                    Some(true) => cx.th.pos,
                    Some(false) => cx.th.neg,
                    None => cx.th.fg.unwrap_or(Color::White),
                });
            }
            let lines: Vec<Line> = big(&s).into_iter().map(|l| Line::styled(l, bs)).collect();
            let top = inner.y + inner.height.saturating_sub(3) / 2;
            let at = Rect::new(inner.x, top, inner.width, 3.min(inner.height));
            f.render_widget(Paragraph::new(lines).alignment(align), at);
        }
        Kind::Tabs { tabs, children } => {
            let shown = tab_of(st, n).min(children.len().saturating_sub(1));
            let mut x = inner.x;
            for (k, t) in tabs.iter().enumerate() {
                let label = format!(" {t} ");
                let w = label.chars().count() as u16;
                if x + w > inner.right() {
                    break;
                }
                let r = Rect::new(x, inner.y, w, 1);
                let ts = if k == shown {
                    Style::default().fg(Color::Black).bg(cx.th.accent).add_modifier(Modifier::BOLD)
                } else if cx.st.hover.as_ref().is_some_and(|h| h.id == id && h.row == Some(1000 + k)) {
                    Style::default().fg(cx.th.accent).bg(cx.th.hover)
                } else {
                    Style::default().fg(cx.th.dim)
                };
                f.buffer_mut().set_string(r.x, r.y, &label, ts);
                if !id.is_empty() {
                    cx.hits.push(Hit { rect: r, id: id.clone(), row: Some(1000 + k), tab: Some(k) });
                }
                x += w + 1;
            }
            let rule = Rect::new(inner.x, inner.y + 1, inner.width, 1);
            f.buffer_mut().set_string(rule.x, rule.y, "─".repeat(inner.width as usize), Style::default().fg(cx.th.border));
            if let Some(c) = children.get(shown) {
                let body = Rect::new(inner.x, inner.y + 2, inner.width, inner.height.saturating_sub(2));
                node(cx, c, f, body);
            }
        }
        Kind::Map { lat, lon, label } => {
            let rows = match bound(st, n) {
                Some(Value::Array(a)) => a.as_slice(),
                _ => &[],
            };
            let pts: Vec<(f64, f64, String)> = rows
                .iter()
                .map(|r| {
                    let la = lookup(r, lat).and_then(Value::as_f64).unwrap_or(0.0);
                    let lo = lookup(r, lon).and_then(Value::as_f64).unwrap_or(0.0);
                    (lo, la, label.as_ref().map(|l| fill(l, r)).unwrap_or_default())
                })
                .collect();
            let coords: Vec<(f64, f64)> = pts.iter().map(|p| (p.0, p.1)).collect();
            let (land, dot) = (cx.th.border, style.fg.unwrap_or(cx.th.accent));
            let hover_row = cx.st.hover.as_ref().filter(|h| h.id == id).and_then(|h| h.row);
            let canvas = Canvas::default()
                .marker(Marker::Braille)
                .x_bounds([-180.0, 180.0])
                .y_bounds([-60.0, 85.0])
                .paint(|ctx| {
                    ctx.draw(&Map { resolution: MapResolution::High, color: land });
                    ctx.layer();
                    ctx.draw(&Points { coords: &coords, color: dot });
                    for (k, (x, y, l)) in pts.iter().enumerate() {
                        let s = if hover_row == Some(k) { Style::default().fg(Color::Black).bg(dot) } else { Style::default().fg(dot) };
                        ctx.print(*x, *y, Line::styled(format!("●{l}"), s));
                    }
                });
            f.render_widget(canvas, inner);
            for (k, (x, y, _)) in pts.iter().enumerate() {
                let cx_ = inner.x as f64 + (x + 180.0) / 360.0 * (inner.width.saturating_sub(1)) as f64;
                let cy = inner.y as f64 + (85.0 - y) / 145.0 * (inner.height.saturating_sub(1)) as f64;
                if !id.is_empty() {
                    cx.mark(&id, Some(k), None, Rect::new(cx_.round() as u16, cy.round() as u16, 1, 1));
                }
            }
        }
    }
}

/// Where a line starts in its cell, by its alignment.
fn line_x(line: &Line, a: Rect) -> u16 {
    let w = (line.width() as u16).min(a.width);
    match line.alignment {
        Some(Alignment::Right) => a.x + a.width - w,
        Some(Alignment::Center) => a.x + (a.width - w) / 2,
        _ => a.x,
    }
}

/// Bars over zero: positive up, negative down, eighth blocks at the tips, labels underneath.
#[allow(clippy::too_many_arguments)]
fn bars(cx: &mut Cx, n: &Node, f: &mut Frame, inner: Rect, label: &str, value: &str, width: Option<u16>, id: &str) {
    let rows = items(cx.st, n);
    if rows.is_empty() || inner.height < 3 {
        return;
    }
    let vals: Vec<f64> = rows.iter().map(|r| lookup(r, value).and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(number))).unwrap_or(0.0)).collect();
    let (lo, hi) = vals.iter().fold((0.0f64, 0.0f64), |(a, b), &x| (a.min(x), b.max(x)));
    let span = (hi - lo).max(1e-9);
    let h = (inner.height - 2) as f64; // bars, then the zero line and the labels below
    let zero = inner.y + ((hi / span) * h).round() as u16;
    let n_ = rows.len() as u16;
    let w = width.unwrap_or(((inner.width / n_.max(1)).saturating_sub(1)).clamp(1, 9));
    let total = n_ * (w + 1);
    let x0 = inner.x + inner.width.saturating_sub(total) / 2;
    let base = cx.style(n.style.as_ref());
    let eighths = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    for (k, (r, v)) in rows.iter().zip(vals.iter()).enumerate() {
        let x = x0 + k as u16 * (w + 1);
        if x + w > inner.right() {
            break;
        }
        let mut color = if *v >= 0.0 { base.fg.unwrap_or(cx.th.pos) } else { cx.th.neg };
        let hovered = cx.hovered(id, k);
        if hovered {
            color = cx.th.accent;
        }
        let cells = (v.abs() / span) * h;
        let full = cells.floor() as u16;
        let part = ((cells - cells.floor()) * 8.0).round() as usize;
        let bar = Style::default().fg(color);
        // Bars stay between the panel's top and the label row.
        let room = |y: u16| y >= inner.y && y + 1 < inner.bottom();
        for i in 0..full {
            let y = if *v >= 0.0 { zero.wrapping_sub(1 + i) } else { zero + 1 + i };
            if room(y) {
                f.buffer_mut().set_string(x, y, "█".repeat(w as usize), bar);
            }
        }
        if part > 0 {
            if *v >= 0.0 && zero > inner.y + full {
                f.buffer_mut().set_string(x, zero - 1 - full, eighths[part].repeat(w as usize), bar);
            } else if *v < 0.0 && part >= 4 && room(zero + 1 + full) {
                f.buffer_mut().set_string(x, zero + 1 + full, "▀".repeat(w as usize), bar);
            }
        }
        let l: String = fill(label, r).chars().take(w as usize).collect();
        let ls = if hovered { Style::default().fg(cx.th.accent).add_modifier(Modifier::BOLD) } else { Style::default().fg(cx.th.dim) };
        f.buffer_mut().set_string(x, inner.bottom() - 1, &l, ls);
        if !id.is_empty() {
            let top = if *v >= 0.0 { zero.saturating_sub(full + 1) } else { zero + 1 };
            cx.mark(id, Some(k), None, Rect::new(x, top.max(inner.y), w, (full + 1).max(1)));
        }
    }
    f.buffer_mut().set_string(inner.x, zero, "┈".repeat(inner.width as usize), Style::default().fg(cx.th.border));
}

/// Numbers from literal `values`, or an array (each item's `field`).
fn numbers(values: &[f64], v: Option<&Value>, field: Option<&str>) -> Vec<f64> {
    if !values.is_empty() {
        return values.to_vec();
    }
    match v {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| match field {
                Some(p) => lookup(x, p).and_then(Value::as_f64),
                None => x.as_f64(),
            })
            .collect(),
        _ => vec![],
    }
}

/// `1234567` → `1.23M`.
fn compact(x: f64) -> String {
    let a = x.abs();
    match a {
        a if a >= 1e9 => format!("{:.2}B", x / 1e9),
        a if a >= 1e6 => format!("{:.2}M", x / 1e6),
        a if a >= 1e4 => format!("{:.1}k", x / 1e3),
        a if a >= 100.0 => format!("{x:.0}"),
        _ => format!("{x:.2}"),
    }
}

/// A colour between `neg` (at `min`), `mid` (at zero) and `pos` (at `max`).
fn heat(v: f64, min: f64, max: f64, neg: Color, pos: Color, mid: Color) -> Color {
    if v >= 0.0 {
        mix(mid, pos, (v / max.max(1e-9)).clamp(0.0, 1.0))
    } else {
        mix(mid, neg, (v / min.min(-1e-9)).clamp(0.0, 1.0))
    }
}

fn rgb(c: Color) -> Option<(u8, u8, u8)> {
    match c {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => None,
    }
}

fn mix(a: Color, b: Color, t: f64) -> Color {
    match (rgb(a), rgb(b)) {
        (Some(a), Some(b)) => {
            let l = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
            Color::Rgb(l(a.0, b.0), l(a.1, b.1), l(a.2, b.2))
        }
        _ if t < 0.5 => a,
        _ => b,
    }
}

/// Only the rows that fit are built: the first one shown, and the selection within them. The
/// selection sits on the last visible row once it passes the bottom.
fn window(selected: Option<usize>, height: usize) -> (usize, Option<usize>) {
    let sel = selected.unwrap_or(0);
    let from = (sel + 1).saturating_sub(height.max(1));
    (from, selected.map(|s| s - from))
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

// ---- big digits ---------------------------------------------------------------------------

/// A glyph: five rows of pixels, drawn as three rows of half blocks.
fn glyph(c: char) -> &'static [&'static str; 5] {
    match c.to_ascii_uppercase() {
        '0' => &["###", "#.#", "#.#", "#.#", "###"],
        '1' => &[".#.", "##.", ".#.", ".#.", "###"],
        '2' => &["###", "..#", "###", "#..", "###"],
        '3' => &["###", "..#", ".##", "..#", "###"],
        '4' => &["#.#", "#.#", "###", "..#", "..#"],
        '5' => &["###", "#..", "###", "..#", "###"],
        '6' => &["###", "#..", "###", "#.#", "###"],
        '7' => &["###", "..#", "..#", ".#.", ".#."],
        '8' => &["###", "#.#", "###", "#.#", "###"],
        '9' => &["###", "#.#", "###", "..#", "###"],
        '$' => &[".##", "##.", ".#.", ".##", "##."],
        '.' => &[".", ".", ".", ".", "#"],
        ',' => &[".", ".", ".", "#", "#"],
        '-' => &["...", "...", "###", "...", "..."],
        '+' => &["...", ".#.", "###", ".#.", "..."],
        '%' => &["#.#", "..#", ".#.", "#..", "#.#"],
        'K' => &["#.#", "#.#", "##.", "#.#", "#.#"],
        'M' => &["#.#", "###", "###", "#.#", "#.#"],
        'B' => &["##.", "#.#", "##.", "#.#", "##."],
        'X' => &["...", "#.#", ".#.", "#.#", "..."],
        'D' => &["##.", "#.#", "#.#", "#.#", "##."],
        'Y' => &["#.#", "#.#", ".#.", ".#.", ".#."],
        ':' => &[".", "#", ".", "#", "."],
        _ => &["..", "..", "..", "..", ".."],
    }
}

/// Text in the big font: three lines.
pub fn big(s: &str) -> [String; 3] {
    let mut out = [String::new(), String::new(), String::new()];
    for (i, c) in s.chars().enumerate() {
        let g = glyph(c);
        let w = g[0].chars().count();
        if i > 0 {
            for l in out.iter_mut() {
                l.push(' ');
            }
        }
        for (row, l) in out.iter_mut().enumerate() {
            let top: Vec<char> = g[row * 2].chars().collect();
            let bot: Vec<char> = g.get(row * 2 + 1).map(|r| r.chars().collect()).unwrap_or_else(|| vec!['.'; w]);
            for x in 0..w {
                l.push(match (top[x] == '#', bot[x] == '#') {
                    (true, true) => '█',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    _ => ' ',
                });
            }
        }
    }
    out
}

// ---- layers ---------------------------------------------------------------------------------

/// The `callout` renderer: measures a box of a title and wrapped text.
struct Callout;

fn wrap(s: &str, w: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for para in s.split('\n') {
        let mut cur = String::new();
        for word in para.split_whitespace() {
            if !cur.is_empty() && cur.chars().count() + 1 + word.chars().count() > w {
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

fn callout_lines(data: &Value, inner: usize) -> Vec<String> {
    let mut lines = wrap(data["text"].as_str().unwrap_or(""), inner);
    if let Some(t) = data["title"].as_str() {
        lines.insert(0, t.to_string());
    }
    lines
}

impl cl::Renderer for Callout {
    fn measure(&self, cx: &cl::MeasureCtx) -> cl::Size {
        let inner = cx.avail.w.saturating_sub(4).max(8) as usize;
        let lines = callout_lines(cx.data, inner);
        let label = cx.owner.actor().map_or(0, |a| a.chars().count() + 5);
        let w = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0).max(label).min(inner);
        cl::Size::new(w as u16 + 4, lines.len() as u16 + 2)
    }
}

/// The layers this frame shows: the UI's, plus the tip of whatever row the mouse is over.
fn specs(st: &State) -> Vec<LayerSpec> {
    let mut out = st.ui.layers.clone();
    if let Some(h) = &st.hover
        && let Some(row) = h.row.filter(|r| *r < 1000)
        && let Some(n) = st.ui.root.find(&h.id)
        && let Some(tip) = &n.tip
    {
        let rows = items(st, n);
        let rows = if rows.is_empty() {
            match bound(st, n) {
                Some(Value::Array(a)) => a.as_slice(),
                _ => &[],
            }
        } else {
            rows
        };
        if let Some(r) = rows.get(row) {
            out.push(LayerSpec {
                on: h.id.clone(),
                row: Some(row),
                title: tip.title.as_ref().map(|t| fill(t, r)),
                text: Some(fill(&tip.text, r)),
                arrow: true,
                place: vec!["right".into(), "left".into(), "below".into(), "above".into()],
                width: Some(44),
                look: Some(Look { edge: Some("plain".into()), ..Look::default() }),
                ..LayerSpec::default()
            });
        }
    }
    out
}

fn layers(cx: &Cx, buf: &mut Buffer, body: Rect) {
    let st = cx.st;
    let specs = specs(st);
    if specs.is_empty() {
        return;
    }
    let mut ls = cl::Layers::default();
    for (i, s) in specs.iter().enumerate() {
        let (kind, key) = match s.row {
            Some(r) => ("row", format!("{}#{}", s.on, r)),
            None => ("node", s.on.clone()),
        };
        let mut l = cl::Layer::new(cl::Anchor::Host { kind: kind.into(), key });
        l.id = format!("s{i}");
        let v = s.bind.as_deref().and_then(|b| value_of(st, b)).unwrap_or(&Value::Null);
        if s.title.is_some() || s.text.is_some() {
            let data = json!({"title": s.title.as_ref().map(|t| fill(t, v)), "text": s.text.as_ref().map(|t| fill(t, v)).unwrap_or_default()});
            l = l.with_content(cl::Content::new("callout", data));
        }
        l.arrow = s.arrow;
        if s.ring || s.pulse.is_some() {
            l.ring = Some(cl::Ring { pulse: s.pulse.map(|p| cl::Pulse { period_ms: p, cycles: 0 }) });
        }
        if s.spotlight {
            l.spotlight = Some(cl::Spotlight::default());
        }
        l.place = s
            .place
            .iter()
            .filter_map(|p| match p.as_str() {
                "below" => Some(cl::Side::Below),
                "above" => Some(cl::Side::Above),
                "right" => Some(cl::Side::Right),
                "left" => Some(cl::Side::Left),
                _ => None,
            })
            .collect();
        l.max_width = s.width;
        let _ = cl::apply(&mut ls, cl::LayerOp::Push(l), s.by.as_deref(), 0, &cl::Limits::default());
    }
    let grid = cl::Grid::new(buf.area.width, buf.area.height).with_area(cl::Rect::new(body.x, body.y, body.width, body.height));
    let renderers = cl::Renderers::new().register("callout", Callout);
    let plan = cl::plan(&ls, &cx.anchors, &grid, &renderers);

    // Spotlights first: everything outside the holes fades.
    if !plan.spots.is_empty() {
        for y in body.y..body.bottom() {
            for x in body.x..body.right() {
                if plan.dimmed(x, y) {
                    let c = &mut buf[(x, y)];
                    c.set_style(Style::default().fg(cx.th.dim).remove_modifier(Modifier::BOLD | Modifier::REVERSED));
                }
            }
        }
    }
    for p in &plan.layers {
        let i: usize = p.id.trim_start_matches('s').parse().unwrap_or(0);
        let s = &specs[i];
        let look = s.look.clone().unwrap_or_default();
        let c = |n: &Option<String>, d: Color| n.as_deref().and_then(|x| cx.color(x)).unwrap_or(d);
        let accent = c(&look.accent, cx.th.accent);
        let fill_bg = c(&look.fill, cx.th.panel);
        let fg = c(&look.fg, cx.th.fg.unwrap_or(Color::Rgb(0xd8, 0xdc, 0xe6)));
        let mut ring = c(&look.ring, cx.th.ring);
        if let Some(period) = s.pulse.filter(|p| *p > 0) {
            let t = (st.now_ms % period as u64) as f64 / period as f64;
            ring = mix(ring, accent, (1.0 - (t * std::f64::consts::TAU).cos()) / 2.0);
        }
        let g = Glyphs::of(look.edge.as_deref());
        for r in &p.ring {
            for x in r.x..r.x + r.w {
                for y in r.y..r.y + r.h {
                    if x < buf.area.width && y < buf.area.height {
                        buf[(x, y)].set_bg(ring);
                    }
                }
            }
        }
        let line = Style::default().fg(accent);
        if let Some(rt) = &p.route {
            let n = rt.steps.len();
            for (k, st_) in rt.steps.iter().enumerate() {
                let sym = if k + 1 == n {
                    match st_.leave {
                        cl::Dir::Up => "▲",
                        cl::Dir::Down => "▼",
                        cl::Dir::Left => "◀",
                        cl::Dir::Right => "▶",
                    }
                } else if st_.enter == st_.leave {
                    if matches!(st_.enter, cl::Dir::Up | cl::Dir::Down) { g.v } else { g.h }
                } else {
                    match (st_.enter, st_.leave) {
                        (cl::Dir::Right, cl::Dir::Down) | (cl::Dir::Up, cl::Dir::Left) => g.tr,
                        (cl::Dir::Left, cl::Dir::Down) | (cl::Dir::Up, cl::Dir::Right) => g.tl,
                        (cl::Dir::Right, cl::Dir::Up) | (cl::Dir::Down, cl::Dir::Left) => g.br,
                        _ => g.bl,
                    }
                };
                put(buf, st_.x, st_.y, sym, line);
            }
        }
        let (Some(r), Some(content)) = (p.rect, ls.get(&p.id).and_then(|l| l.content.as_ref())) else { continue };
        let fill_s = Style::default().fg(fg).bg(fill_bg);
        for y in r.y..r.y + r.h {
            for x in r.x..r.x + r.w {
                put(buf, x, y, " ", fill_s);
            }
        }
        let lines = callout_lines(&content.data, r.w.saturating_sub(4) as usize);
        if p.mode == Some(cl::Mode::Strip) {
            buf.set_stringn(r.x + 1, r.y, lines.join(" "), r.w as usize - 1, fill_s);
            continue;
        }
        let border = Style::default().fg(accent).bg(fill_bg);
        let (x1, y1) = (r.x + r.w - 1, r.y + r.h - 1);
        for x in r.x + 1..x1 {
            put(buf, x, r.y, g.h, border);
            put(buf, x, y1, g.h, border);
        }
        for y in r.y + 1..y1 {
            put(buf, r.x, y, g.v, border);
            put(buf, x1, y, g.v, border);
        }
        put(buf, r.x, r.y, g.tl, border);
        put(buf, x1, r.y, g.tr, border);
        put(buf, r.x, y1, g.bl, border);
        put(buf, x1, y1, g.br, border);
        if let Some(actor) = p.owner.actor() {
            let label = format!(" ◆ {actor} ");
            let n = label.chars().count() as u16;
            let mut at = 2;
            if let Some(a) = p.route.as_ref().map(|rt| rt.attach)
                && a.edge == cl::Edge::Top
                && (at..at + n).contains(&a.offset)
            {
                at = a.offset + 2;
            }
            if at + n < r.w {
                buf.set_string(r.x + at, r.y, label, border.add_modifier(Modifier::ITALIC));
            }
        }
        if let (Some(rt), Some(side)) = (&p.route, p.side) {
            let j = match side {
                cl::Side::Above => g.jt,
                cl::Side::Below => g.jb,
                cl::Side::Right => g.jr,
                cl::Side::Left => g.jl,
            };
            put(buf, rt.junction.0, rt.junction.1, j, border);
        }
        let has_title = content.data["title"].is_string();
        for (k, l) in lines.iter().enumerate().take(r.h as usize - 2) {
            let s = if k == 0 && has_title { border.add_modifier(Modifier::BOLD) } else { fill_s };
            buf.set_line(r.x + 2, r.y + 1 + k as u16, &Line::from(Span::styled(l.clone(), s)), r.w - 4);
        }
    }
}

fn put(buf: &mut Buffer, x: u16, y: u16, s: &str, style: Style) {
    if x < buf.area.width && y < buf.area.height {
        buf[(x, y)].set_symbol(s).set_style(style);
    }
}

/// Box and arrow glyphs for an edge style.
struct Glyphs {
    h: &'static str,
    v: &'static str,
    tl: &'static str,
    tr: &'static str,
    bl: &'static str,
    br: &'static str,
    /// Junctions for an arrow leaving a box placed above, below, left of or right of its anchor
    /// (`┬`, `┴`, `├`, `┤` in the plain set).
    jt: &'static str,
    jb: &'static str,
    jl: &'static str,
    jr: &'static str,
}

impl Glyphs {
    fn of(edge: Option<&str>) -> Glyphs {
        match edge {
            Some("double") => Glyphs { h: "═", v: "║", tl: "╔", tr: "╗", bl: "╚", br: "╝", jt: "╦", jb: "╩", jl: "╠", jr: "╣" },
            Some("thick") => Glyphs { h: "━", v: "┃", tl: "┏", tr: "┓", bl: "┗", br: "┛", jt: "┳", jb: "┻", jl: "┣", jr: "┫" },
            Some("plain") => Glyphs { h: "─", v: "│", tl: "┌", tr: "┐", bl: "└", br: "┘", jt: "┬", jb: "┴", jl: "├", jr: "┤" },
            _ => Glyphs { h: "─", v: "│", tl: "╭", tr: "╮", bl: "╰", br: "╯", jt: "┬", jb: "┴", jl: "├", jr: "┤" },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn big_digits_are_three_rows() {
        let [a, b, c] = big("1.5");
        assert_eq!((a.as_str(), b.as_str(), c.as_str()), ("▄█    █▀▀", " █    ▀▀█", "▀▀▀ ▀ ▀▀▀"));
    }
}
