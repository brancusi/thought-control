//! Drawing layers (layers.rs) in thc's design system: placement is caretline-layers' `plan`,
//! every cell is thc's.
//!
//! Each frame, after everything else is drawn:
//!
//! 1. **Anchors.** An `AnchorMap` of what was drawn where: list rows (`row:<id>`) in every
//!    view and sidebar list panel, the document's lines (`row:<id>` too), tabs, footer hints,
//!    the scope chip, the detail pane, the calendar, sidebar panels and their headers (`ui:…`,
//!    `panel:<n>`), and keymap actions with a button on screen (`action:<id>`). Most come from
//!    the frame's click targets, recorded while drawing; the rest are put while drawing.
//!    Text, block and caret anchors resolve through the engine's frame of the main document
//!    and of each sidebar document panel (`editors`, the one place that scopes them).
//! 2. **Grid.** The whole screen: every drawn cell is text, wide or blank; layers (and a
//!    spotlight's dimming) cover everything above the bar; the caret (while writing) is kept
//!    clear of agents' boxes.
//! 3. **Plan**, with thc's renderers measuring `hint` and `thc.tour` boxes.
//! 4. **Draw**: dim every cell a spotlight leaves out, tint the rings, draw arrows from the
//!    route with box-drawing (ASCII with `THC_GLYPHS=ascii`), edge chips for anchors off
//!    screen, then each box: its border with the owner's attribution (`◆ claude`) on the top
//!    edge, its title and text, and a walkthrough's step dots and buttons.
//!
//! Agents' layers are drawn in the agent hue, thc's and a walkthrough's own in the accent.

use crate::app::App;
use crate::layers::TOUR;
use crate::theme::{Theme, Token};
use crate::ui::{Click, RenderOutput};
use caretline_layers as cl;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};
use serde_json::Value;

/// The narrowest box (so `◆ name` fits on its top edge).
const MIN_W: u16 = 18;

/// Words wrapped to `w` columns (by display width), hard-breaking words longer than a row.
pub fn wrap(s: &str, w: usize) -> Vec<String> {
    let w = w.max(1);
    let mut out = Vec::new();
    for para in s.split('\n') {
        let mut cur = String::new();
        for word in para.split_whitespace() {
            let ww = crate::text::width(word);
            let cw = crate::text::width(&cur);
            if !cur.is_empty() && cw + 1 + ww > w {
                out.push(std::mem::take(&mut cur));
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            if crate::text::width(&cur) + ww > w {
                for ch in word.chars() {
                    if crate::text::width(&cur) + crate::text::width(&ch.to_string()) > w {
                        out.push(std::mem::take(&mut cur));
                    }
                    cur.push(ch);
                }
            } else {
                cur.push_str(word);
            }
        }
        out.push(cur);
    }
    out
}

/// A `hint`'s or a walkthrough step's lines inside the box, `inner` columns wide: the title
/// (bold), the text, and for a step a blank row and its controls row.
struct Lines {
    title: Option<String>,
    body: Vec<String>,
    controls: Option<Controls>,
}

/// A step's controls: dots, then `‹ back`, `next ›` (or `done`) and `×`.
struct Controls {
    step: usize,
    of: usize,
}

impl Controls {
    fn labels(&self, ascii: bool) -> (String, &'static str, &'static str, &'static str) {
        let (on, off) = if ascii { ("*", "o") } else { ("●", "○") };
        let dots: String = (1..=self.of.min(12))
            .map(|i| if i <= self.step { on } else { off })
            .collect();
        let back = if ascii { "< back" } else { "‹ back" };
        let next = if self.step >= self.of {
            "done"
        } else if ascii {
            "next >"
        } else {
            "next ›"
        };
        let stop = if ascii { "x" } else { "×" };
        (dots, back, next, stop)
    }

    fn width(&self) -> usize {
        // dots, 2 spaces, back, 2, next, 2, stop: the same widths in both glyph sets.
        self.of.min(12) + 2 + 6 + 2 + 6 + 2 + 1
    }
}

fn lines_of(kind: &str, data: &Value, inner: usize) -> Lines {
    let title = data
        .get("title")
        .and_then(Value::as_str)
        .filter(|t| !t.trim().is_empty())
        .map(|t| crate::ui::truncate_str(t, inner, "…"));
    let text = data.get("text").and_then(Value::as_str).unwrap_or("");
    let body = if text.is_empty() {
        vec![]
    } else {
        wrap(text, inner)
    };
    let controls = (kind == TOUR).then(|| Controls {
        step: data.get("at").and_then(Value::as_u64).unwrap_or(1) as usize,
        of: data.get("of").and_then(Value::as_u64).unwrap_or(1) as usize,
    });
    Lines {
        title,
        body,
        controls,
    }
}

fn natural_inner(kind: &str, data: &Value) -> usize {
    let title = data
        .get("title")
        .and_then(Value::as_str)
        .map_or(0, crate::text::width);
    let text = data.get("text").and_then(Value::as_str).unwrap_or("");
    let longest = text.split('\n').map(crate::text::width).max().unwrap_or(0);
    let controls = if kind == TOUR {
        lines_of(kind, data, 80).controls.map_or(0, |c| c.width())
    } else {
        0
    };
    title.max(longest).max(controls)
}

/// thc's box renderer, for `hint` and `thc.tour`: a bordered box with one column of padding.
/// Pure: the size depends only on the data and the room.
struct BoxRenderer(&'static str);

impl BoxRenderer {
    fn size(kind: &str, data: &Value, avail: cl::Size, owner: &cl::Owner) -> cl::Size {
        let room = avail.w.saturating_sub(4) as usize;
        // An agent's box is wide enough for ` ◆ name ` on its top edge.
        let label = owner.actor().map_or(0, |a| crate::text::width(a) + 4);
        let want = natural_inner(kind, data).max(MIN_W as usize - 4).max(label);
        let inner = want.min(room).max(1);
        let l = lines_of(kind, data, inner);
        let rows = l.title.is_some() as usize + l.body.len() + l.controls.as_ref().map_or(0, |_| 2);
        let w = (inner as u16 + 4).min(avail.w);
        let h = (rows.max(1) as u16 + 2).min(avail.h);
        if w < 8 || h < 3 {
            cl::Size::new(0, 0)
        } else {
            cl::Size::new(w, h)
        }
    }
}

impl cl::Renderer for BoxRenderer {
    fn measure(&self, cx: &cl::MeasureCtx) -> cl::Size {
        BoxRenderer::size(self.0, cx.data, cx.avail, cx.owner)
    }

    fn chip(&self, data: &Value, _anchor: &cl::Anchor, _off: cl::Off) -> cl::Size {
        let label = chip_label(data);
        cl::Size::new((crate::text::width(&label) as u16 + 4).clamp(6, 24), 1)
    }
}

/// The edge chip's words: the title, or the text's first words.
fn chip_label(data: &Value) -> String {
    let t = data
        .get("title")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .or_else(|| data.get("text").and_then(Value::as_str))
        .unwrap_or("here");
    crate::ui::truncate_str(t, 18, "…")
}

pub fn renderers() -> cl::Renderers {
    cl::Renderers::new()
        .register(cl::HINT, BoxRenderer(cl::HINT))
        .register(TOUR, BoxRenderer(TOUR))
}

// ---- anchors ------------------------------------------------------------------------------------

/// The cells of `[x0, x1)` on row `y` that hold something, trimmed of blanks at both ends.
fn trimmed(buf: &Buffer, x0: u16, x1: u16, y: u16) -> Option<cl::Rect> {
    if y >= buf.area.height {
        return None;
    }
    let x1 = x1.min(buf.area.width);
    let filled = |x: u16| !buf[(x, y)].symbol().trim().is_empty();
    let a = (x0..x1).find(|&x| filled(x))?;
    let b = (x0..x1).rev().find(|&x| filled(x))?;
    Some(cl::Rect::new(a, y, b + 1 - a, 1))
}

fn put(m: &mut cl::AnchorMap, kind: &str, key: &str, r: cl::Rect) {
    if r.w > 0 && r.h > 0 {
        m.put(cl::AnchorKey::host(kind, key), r);
    }
}

pub fn rect(r: ratatui::layout::Rect) -> cl::Rect {
    cl::Rect::new(r.x, r.y, r.width, r.height)
}

/// What the frame drew where, as anchors: from its click targets and its document rows, plus
/// what was put while drawing (`render.anchors`: the detail pane, the calendar, panels).
fn collect(render: &RenderOutput, buf: &Buffer, app: &App) -> cl::AnchorMap {
    let mut m = render.anchors.clone();
    // The row keys layers name (only those are looked for off screen).
    let wanted: std::collections::HashSet<&str> = app
        .ui
        .layers
        .stack
        .layers
        .iter()
        .flat_map(|l| l.anchor.iter())
        .filter_map(|a| match a {
            cl::Anchor::Host { kind, key } if kind == "row" => Some(key.as_str()),
            _ => None,
        })
        .collect();
    let last = buf.area.height.saturating_sub(1);
    for t in &render.click_targets {
        let full = cl::Rect::new(t.x0, t.y, t.x1.saturating_sub(t.x0), 1);
        match &t.what {
            Click::Row(i) => {
                if let (Some(k), Some(r)) = (
                    app.rows.get(*i).and_then(|r| r.key()),
                    trimmed(buf, t.x0, t.x1, t.y),
                ) {
                    put(&mut m, "row", &k, r);
                }
            }
            Click::PanelRow(p, i) => {
                let key = app
                    .ui
                    .sidebar
                    .open
                    .get(*p)
                    .map(|p| p.key())
                    .and_then(|pk| app.lists.get(&pk))
                    .and_then(|rt| rt.rows.get(*i))
                    .and_then(|r| r.key());
                if let (Some(k), Some(r)) = (key, trimmed(buf, t.x0, t.x1, t.y)) {
                    put(&mut m, "row", &k, r);
                }
            }
            Click::View(v) => put(
                &mut m,
                "ui",
                &format!("tab:{}", v.name().to_lowercase()),
                full,
            ),
            Click::Action(a) => {
                put(&mut m, "action", a, full);
                if t.y == last {
                    put(&mut m, "ui", &format!("footer:{a}"), full);
                }
            }
            Click::Key(ratatui::crossterm::event::KeyCode::Char('*'), _) => {
                put(&mut m, "ui", "scope", full)
            }
            Click::Day(d) => put(&mut m, "ui", &format!("day:{d}"), full),
            Click::Field(f) => put(&mut m, "ui", &format!("detail:{f}"), full),
            Click::Node(id) => put(&mut m, "ui", &format!("detail:node:{id}"), full),
            Click::Panel(p, crate::sidebar_ui::Part::Title) => {
                put(&mut m, "ui", &format!("panel:{p}:header"), full)
            }
            _ => {}
        }
    }
    // The open document's lines: the text each row draws.
    if let Some(d) = app.doc.as_ref() {
        for h in &render.doc_hits {
            let Some(l) = d.blocks().get(h.line) else {
                continue;
            };
            let text = l.text.get(h.start..h.end).unwrap_or("");
            let w = crate::text::width(text) as u16;
            let x0 = if h.first { h.hang_x } else { h.text_x };
            let r = cl::Rect::new(x0, h.y, (h.text_x + w.max(1)).saturating_sub(x0), 1);
            put(&mut m, "row", &l.id, r);
        }
        // Lines scrolled out of sight that a layer names: which way they lie (an edge chip
        // points there).
        let shown: Vec<usize> = render.doc_hits.iter().map(|h| h.line).collect();
        if let (Some(&lo), Some(&hi)) = (shown.iter().min(), shown.iter().max()) {
            let x = render.doc_hits.first().map(|h| h.text_x);
            for (i, l) in d.blocks().iter().enumerate().filter(|(_, l)| wanted.contains(l.id.as_str())) {
                if i < lo {
                    off(&mut m, &l.id, cl::Off::Above { x });
                } else if i > hi {
                    off(&mut m, &l.id, cl::Off::Below { x });
                }
            }
        }
    }
    // List rows scrolled out of sight, the same way.
    let drawn: Vec<usize> = render.click_targets.iter().filter_map(|t| if let Click::Row(i) = t.what { Some(i) } else { None }).collect();
    if let (Some(&lo), Some(&hi)) = (drawn.iter().min(), drawn.iter().max()) {
        for (i, r) in app.rows.iter().enumerate() {
            let Some(k) = r.key().filter(|k| wanted.contains(k.as_str())) else { continue };
            if i < lo {
                off(&mut m, &k, cl::Off::Above { x: None });
            } else if i > hi {
                off(&mut m, &k, cl::Off::Below { x: None });
            }
        }
    }
    m
}

/// A row key off screen, unless it's drawn somewhere else (a panel).
fn off(m: &mut cl::AnchorMap, key: &str, o: cl::Off) {
    let k = cl::AnchorKey::host("row", key);
    if m.get(&k).is_none() {
        m.put_off(k, o);
    }
}

/// The editors on screen that resolve text, block and caret anchors: the main document's view
/// (`main`) and each sidebar document panel's (`panel:<n>`), each with its clip and the
/// focused one marked. caretline-layers scopes anchors with them (`{"text": …, "in":
/// "panel:2"}`; unscoped ones go to the focused view first). Edits map anchors per document
/// the same way (session.rs `observe_text`, `Edited::Views`).
fn editors<'a>(app: &'a App, render: &RenderOutput) -> Vec<(String, cl::FrameResolver<'a>)> {
    let mut out = Vec::new();
    let sidebar = app.ui.focus == crate::app::Focus::Sidebar;
    if let (Some(p), Some(d), Some((vx, vy, vw, vh))) = (app.derived.doc.as_ref(), app.doc.as_ref(), render.doc_view) {
        if p.current(app) {
            let r = cl::FrameResolver::new(&p.frame).id("main").at(p.frame_at.0, p.frame_at.1).clip(cl::Rect::new(vx, vy, vw, vh)).with_doc(d.cn_doc());
            out.push(("main".to_string(), if sidebar { r } else { r.focused() }));
        }
    }
    if let Some(sb) = app.derived.sidebar.as_ref() {
        for (i, pp) in sb.panels.iter().enumerate() {
            let (Some(f), Some(at), Some(v)) = (pp.frame.as_ref(), pp.frame_at, pp.view) else {
                continue;
            };
            let id = format!("panel:{i}");
            let mut r = cl::FrameResolver::new(f).id(&id).at(at.0, at.1).clip(rect(v));
            if let Some(d) = app.panel_doc(&pp.key) {
                r = r.with_doc(d.cn_doc());
            }
            if sidebar && app.ui.sidebar.focused.as_ref() == Some(&pp.key) {
                r = r.focused();
            }
            out.push((id, r));
        }
    }
    out
}

/// The grid placement sees: the whole screen and its drawn cells; layers go above the bar.
fn grid(buf: &Buffer, caret: Option<(u16, u16)>) -> cl::Grid {
    let (w, h) = (buf.area.width, buf.area.height);
    let mut g = cl::Grid::new(w, h)
        // Layers go anywhere but the bar, whose row stays readable (and undimmed).
        .with_area(cl::Rect::new(0, 0, w, h.saturating_sub(1)))
        .with_caret(caret);
    for y in 0..h {
        let mut x = 0;
        while x < w {
            let s = buf[(x, y)].symbol();
            let cw = crate::text::width(s);
            if cw >= 2 {
                g.set_kind(x, y, cl::CellKind::Wide);
                if x + 1 < w {
                    g.set_kind(x + 1, y, cl::CellKind::WideTail);
                }
                x += 2;
                continue;
            }
            if !s.trim().is_empty() {
                g.set_kind(x, y, cl::CellKind::Text);
            }
            x += 1;
        }
    }
    g
}

/// The caret on screen, when the person is writing in the open document.
fn caret(app: &App) -> Option<(u16, u16)> {
    let p = app.derived.doc.as_ref()?;
    let (cx, cy) = p.frame.cursor?;
    Some((p.frame_at.0 + cx, p.frame_at.1 + cy))
}

/// Plans the layers for the frame in `buf` (nothing when none show).
pub fn plan(render: &mut RenderOutput, buf: &Buffer, app: &App) -> Option<cl::Plan> {
    if !app.ui.layers.showing() || (app.ui.teaching_demo && (app.overlay.is_some() || app.prompt.is_some() || app.edit.is_some())) {
        return None;
    }
    let anchors = collect(render, buf, app);
    let eds = editors(app, render);
    let mut chain: Vec<&dyn cl::Resolve> = vec![&anchors];
    for (_, r) in &eds {
        chain.push(r);
    }
    let g = grid(buf, if app.main.write { caret(app) } else { None });
    let p = cl::plan(&app.ui.layers.stack, &cl::Chain(chain), &g, &renderers());
    render.anchors = anchors;
    Some(p)
}

// ---- drawing ------------------------------------------------------------------------------------

/// The layer's colours: the agent hue for an agent's, the accent otherwise.
struct Look {
    border: Style,
    fill: Style,
    text: Style,
    muted: Style,
    ring: Style,
}

fn look(th: &Theme, agent: bool) -> Look {
    let hue = if agent { Token::Agent } else { Token::Accent };
    let fill = th.fill(Token::Raised);
    if th.is_ansi() {
        return Look {
            border: th.s(hue).add_modifier(Modifier::BOLD),
            fill: Style::default(),
            text: th.s(Token::Text),
            muted: th.s(Token::Muted),
            ring: Style::default().add_modifier(Modifier::UNDERLINED | Modifier::BOLD),
        };
    }
    Look {
        border: th.s(hue).patch(fill),
        fill,
        text: th.s(Token::Text).patch(fill),
        muted: th.s(Token::Muted).patch(fill),
        ring: th
            .fill(if agent {
                Token::AgentTint
            } else {
                Token::AccentTint
            })
            .add_modifier(Modifier::BOLD),
    }
}

fn set(buf: &mut Buffer, x: u16, y: u16, s: &str, st: Style) {
    if x < buf.area.width && y < buf.area.height {
        buf[(x, y)].set_symbol(s).set_style(st);
    }
}

fn put_str(buf: &mut Buffer, x: u16, y: u16, s: &str, max: usize, st: Style) -> u16 {
    if y >= buf.area.height || x >= buf.area.width {
        return x;
    }
    let (nx, _) = buf.set_stringn(x, y, s, max, st);
    nx
}

fn rgb(c: Option<Color>) -> Option<(u8, u8, u8)> {
    match c {
        Some(Color::Rgb(r, g, b)) => Some((r, g, b)),
        _ => None,
    }
}

/// A cell left out by a spotlight: its fg blended 60% toward its bg in truecolor, faint
/// otherwise. Its symbol stays (copying, accessibility).
fn dim_cell(buf: &mut Buffer, x: u16, y: u16, th: &Theme) {
    let c = &mut buf[(x, y)];
    let bg = rgb(c.style().bg).or_else(|| rgb(th.s(Token::Bg).bg));
    match (rgb(c.style().fg), bg) {
        (Some((r, g, b)), Some((br, bgc, bb))) => {
            let mix = |a: u8, z: u8| ((a as u16 * 2 + z as u16 * 3) / 5) as u8;
            c.set_fg(Color::Rgb(mix(r, br), mix(g, bgc), mix(b, bb)));
        }
        _ => {
            let st = c.style().add_modifier(Modifier::DIM);
            c.set_style(st);
        }
    }
}

fn arrow_glyph(th: &Theme, s: &cl::Step, head: bool) -> &'static str {
    use cl::Dir::*;
    let ascii = th.ascii;
    if head {
        return match (s.leave, ascii) {
            (Up, false) => "▲",
            (Down, false) => "▼",
            (Left, false) => "◀",
            (Right, false) => "▶",
            (Up, true) => "^",
            (Down, true) => "v",
            (Left, true) => "<",
            (Right, true) => ">",
        };
    }
    if s.enter == s.leave {
        return match (matches!(s.enter, Up | Down), ascii) {
            (true, false) => "│",
            (false, false) => "─",
            (true, true) => "|",
            (false, true) => "-",
        };
    }
    if ascii {
        return "+";
    }
    match (s.enter, s.leave) {
        (Right, Down) | (Up, Left) => "╮",
        (Left, Down) | (Up, Right) => "╭",
        (Right, Up) | (Down, Left) => "╯",
        _ => "╰",
    }
}

struct Border {
    h: &'static str,
    v: &'static str,
    tl: &'static str,
    tr: &'static str,
    bl: &'static str,
    br: &'static str,
}

fn border(th: &Theme) -> Border {
    if th.ascii {
        Border {
            h: "-",
            v: "|",
            tl: "+",
            tr: "+",
            bl: "+",
            br: "+",
        }
    } else {
        Border {
            h: "─",
            v: "│",
            tl: "╭",
            tr: "╮",
            bl: "╰",
            br: "╯",
        }
    }
}

/// Who made a layer, as the top edge says it: `◆ claude` for an agent's, nothing for thc's.
fn attribution(th: &Theme, owner: &cl::Owner, tour_actor: Option<&str>) -> Option<String> {
    // A walkthrough's steps are `guide` layers: an agent's walkthrough carries its name.
    let who = owner.actor().or(if *owner == cl::Owner::Guide { tour_actor } else { None })?;
    Some(format!("{} {who}", th.glyphs().agent))
}

/// Draws the plan over the frame.
pub fn draw(buf: &mut Buffer, p: &mut cl::Plan, app: &App) {
    let th = app.theme;
    // Dim what the spotlights leave out.
    if !p.spots.is_empty() {
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                if p.dimmed(x, y) {
                    dim_cell(buf, x, y, &th);
                }
            }
        }
    }
    let layers = p.layers.clone();
    for l in &layers {
        // An agent's layer, or a step of an agent's walkthrough: the agent hue.
        let agent = l.agent || (l.owner == cl::Owner::Guide && app.ui.layers.tour_actor.is_some());
        let lk = look(&th, agent);
        // The ring: the anchor's cells tinted.
        for r in &l.ring {
            for y in r.y..r.bottom().min(buf.area.height) {
                for x in r.x..r.right().min(buf.area.width) {
                    let st = buf[(x, y)].style().patch(lk.ring);
                    buf[(x, y)].set_style(st);
                }
            }
        }
        // The arrow.
        if let Some(rt) = &l.route {
            let n = rt.steps.len();
            let st = if th.is_ansi() {
                lk.border
            } else {
                th.s(if agent { Token::Agent } else { Token::Accent })
            };
            for (k, s) in rt.steps.iter().enumerate() {
                let keep_bg = buf
                    .area
                    .contains((s.x, s.y).into())
                    .then(|| buf[(s.x, s.y)].style().bg)
                    .flatten();
                let mut st = st;
                if let Some(bg) = keep_bg {
                    st = st.bg(bg);
                }
                set(buf, s.x, s.y, arrow_glyph(&th, s, k + 1 == n), st);
            }
        }
        let content = app
            .ui
            .layers
            .stack
            .get(&l.id)
            .and_then(|x| x.content.clone());
        // The edge chip of an anchor off screen.
        if let Some(c) = l.chip {
            let dir = match l.anchor.as_ref().and_then(|a| a.off) {
                Some(cl::Off::Above { .. }) => {
                    if th.ascii {
                        "^"
                    } else {
                        "↑"
                    }
                }
                Some(cl::Off::Below { .. }) => {
                    if th.ascii {
                        "v"
                    } else {
                        "↓"
                    }
                }
                Some(cl::Off::Left { .. }) => {
                    if th.ascii {
                        "<"
                    } else {
                        "←"
                    }
                }
                _ => {
                    if th.ascii {
                        ">"
                    } else {
                        "→"
                    }
                }
            };
            let label = content
                .as_ref()
                .map_or_else(|| "here".to_string(), |c| chip_label(&c.data));
            let st = if th.is_ansi() {
                lk.border.add_modifier(Modifier::REVERSED)
            } else {
                lk.border.patch(th.fill(if agent {
                    Token::AgentTint
                } else {
                    Token::AccentTint
                }))
            };
            for x in c.x..c.right() {
                set(buf, x, c.y, " ", st);
            }
            put_str(
                buf,
                c.x + 1,
                c.y,
                &format!("{dir} {label}"),
                c.w.saturating_sub(2) as usize,
                st,
            );
        }
        let (Some(r), Some(content)) = (l.rect, content) else {
            continue;
        };
        draw_box(buf, p, l, r, &content, &th, &lk, app.ui.layers.tour_actor.as_deref(), &app.ui.layers.tour);
    }
}

fn draw_box(
    buf: &mut Buffer,
    p: &mut cl::Plan,
    l: &cl::Planned,
    r: cl::Rect,
    content: &cl::Content,
    th: &Theme,
    lk: &Look,
    tour_actor: Option<&str>,
    tour: &caretline_tour::TourState,
) {
    let who = attribution(th, &l.owner, tour_actor);
    let b = border(th);
    // A strip: one row across the area.
    if l.mode == Some(cl::Mode::Strip) {
        for x in r.x..r.right() {
            set(buf, x, r.y, " ", lk.text);
        }
        let ls = lines_of(&content.kind, &content.data, 400);
        let mut s = String::new();
        if let Some(w) = &who {
            s.push_str(w);
            s.push_str(" · ");
        }
        if let Some(c) = &ls.controls {
            s.push_str(&format!("{}/{} ", c.step, c.of));
        }
        if let Some(t) = &ls.title {
            s.push_str(t);
            s.push_str(": ");
        }
        s.push_str(&ls.body.join(" "));
        put_str(
            buf,
            r.x + 1,
            r.y,
            &s,
            r.w.saturating_sub(2) as usize,
            lk.border.patch(lk.fill),
        );
        return;
    }
    if r.w < 4 || r.h < 3 {
        return;
    }
    let (x1, y1) = (r.right() - 1, r.bottom() - 1);
    for y in r.y..r.bottom() {
        for x in r.x..r.right() {
            set(buf, x, y, " ", lk.fill);
        }
    }
    for x in r.x + 1..x1 {
        set(buf, x, r.y, b.h, lk.border);
        set(buf, x, y1, b.h, lk.border);
    }
    for y in r.y + 1..y1 {
        set(buf, r.x, y, b.v, lk.border);
        set(buf, x1, y, b.v, lk.border);
    }
    set(buf, r.x, r.y, b.tl, lk.border);
    set(buf, x1, r.y, b.tr, lk.border);
    set(buf, r.x, y1, b.bl, lk.border);
    set(buf, x1, y1, b.br, lk.border);
    // Where the arrow leaves the box.
    let attach = l.route.as_ref().map(|rt| (rt.junction, rt.attach.edge));
    if let Some(((jx, jy), edge)) = attach {
        let g = match (edge, th.ascii) {
            (_, true) => "+",
            (cl::Edge::Top, _) => "┴",
            (cl::Edge::Bottom, _) => "┬",
            (cl::Edge::Left, _) => "┤",
            (cl::Edge::Right, _) => "├",
        };
        set(buf, jx, jy, g, lk.border);
    }
    // The attribution on the top edge, clear of the arrow.
    if let Some(w) = who {
        let label = format!(" {w} ");
        let lw = crate::text::width(&label) as u16;
        let room = r.w.saturating_sub(4);
        if room >= 4 {
            let lw = lw.min(room);
            let mut x = r.x + 2;
            if let Some(((jx, jy), _)) = attach {
                if jy == r.y && jx >= x && jx < x + lw {
                    x = x1.saturating_sub(1 + lw).max(r.x + 1);
                }
            }
            put_str(
                buf,
                x,
                r.y,
                &label,
                lw as usize,
                lk.border.add_modifier(Modifier::BOLD),
            );
        }
    }
    let inner = r.w.saturating_sub(4) as usize;
    let ls = lines_of(&content.kind, &content.data, inner);
    let mut y = r.y + 1;
    let tx = r.x + 2;
    if let Some(t) = &ls.title {
        if y < y1 {
            put_str(buf, tx, y, t, inner, lk.border.add_modifier(Modifier::BOLD));
            y += 1;
        }
    }
    for line in &ls.body {
        if y >= y1 {
            break;
        }
        put_str(buf, tx, y, line, inner, lk.text);
        y += 1;
    }
    // A walkthrough step's controls on its last row inside: dots, back, next, stop.
    if let Some(c) = &ls.controls {
        let cy = y1 - 1;
        if cy > r.y {
            let (dots, back, next, stop) = c.labels(th.ascii);
            let mut x = put_str(buf, tx, cy, &dots, inner, lk.border);
            if let Some(t) = &tour.tour {
                for (i, step) in t.steps.iter().take(c.of.min(12)).enumerate() {
                    if tx + (i as u16) < x {
                        p.regions.push(cl::Region { rect: cl::Rect::new(tx + i as u16, cy, 1, 1), id: format!("tour-step:{}", step.id) });
                    }
                }
            }
            x += 2;
            let back_st = if c.step > 1 { lk.text } else { lk.muted };
            let bx = x;
            x = put_str(buf, x, cy, back, 6, back_st);
            p.regions.push(cl::Region {
                rect: cl::Rect::new(bx, cy, x - bx, 1),
                id: format!("{}/back", l.id),
            });
            x += 2;
            let nx = x;
            x = put_str(buf, x, cy, next, 6, lk.border.add_modifier(Modifier::BOLD));
            p.regions.push(cl::Region {
                rect: cl::Rect::new(nx, cy, x - nx, 1),
                id: format!("{}/next", l.id),
            });
            let sx = x1 - 2;
            put_str(buf, sx, cy, stop, 1, lk.muted);
            p.regions.push(cl::Region {
                rect: cl::Rect::new(sx, cy, 1, 1),
                id: format!("{}/stop", l.id),
            });
        }
    }
}

/// A click on a layer (the frame's plan): a walkthrough's buttons, an edge chip (reveal), a
/// hint's box (dismiss it). True: the click was the layer's.
pub fn click(app: &mut App, x: u16, y: u16) -> bool {
    let Some(region) = app
        .render
        .layer_plan
        .as_ref()
        .and_then(|p| p.hit(x, y))
        .map(|r| r.id.clone())
    else {
        return false;
    };
    let limits = app.layer_limits.limits();
    let now = app.ui.now_ms;
    if let Some(to) = region.strip_prefix("tour-step:") {
        let _ = crate::layers::request(&mut app.ui, &serde_json::json!({"op":"tour.step", "to":to}), None, &limits);
        return true;
    }
    // Ids: `<layer>`, `<layer>/reveal`, a step's buttons `<layer>/next` (a step layer's own id
    // has a `/`: `s1/0`).
    let (id, part) = match region.rsplit_once('/') {
        Some((id, p @ ("next" | "back" | "stop" | "reveal"))) => (id.to_string(), Some(p)),
        _ => (region.clone(), None),
    };
    match part.map(|p| (id.as_str(), p)) {
        Some((_, "next")) => {
            let _ = crate::layers::step(&mut app.ui.layers, "tour.next", now, &limits);
        }
        Some((_, "back")) => {
            let _ = crate::layers::step(&mut app.ui.layers, "tour.back", now, &limits);
        }
        Some((_, "stop")) => {
            let _ = crate::layers::step(&mut app.ui.layers, "tour.stop", now, &limits);
        }
        Some((id, "reveal")) => {
            if let Some(cl::Anchor::Host { kind, key }) = app
                .ui
                .layers
                .stack
                .get(id)
                .and_then(|l| l.anchor.first().cloned())
            {
                if kind == "row" && app.doc.is_none() {
                    app.ui.selected = Some(key);
                }
            }
        }
        Some(_) => {}
        None => {
            // The person dismisses a hint by clicking it. A walkthrough step (a guide layer, or
            // any other kind) stays: only its buttons act.
            let l = app.ui.layers.stack.get(&region);
            let hint = l.and_then(|l| l.content.as_ref()).is_some_and(|c| c.kind == cl::HINT) && l.is_some_and(|l| l.owner != cl::Owner::Guide);
            if hint {
                app.ui.layers.stack.layers.retain(|l| l.id != region);
            }
        }
    }
    true
}

/// Text-only fallback when a normal anchored box cannot fit. Keeps the controls available
/// without changing preferences, vault data or the person's focus.
pub fn compact(buf: &mut Buffer, plan: &mut cl::Plan, app: &App) -> bool {
    if !app.ui.layers.touring() || app.ui.layers.stack.hidden {
        return false;
    }
    let guide = app.ui.layers.stack.layers.iter().find(|l| l.owner == cl::Owner::Guide);
    let Some(layer) = guide else { return false };
    if buf.area.width >= 70 && buf.area.height >= 20 && plan.layers.iter().any(|l| l.id == layer.id && l.mode == Some(cl::Mode::Box) && l.rect.is_some_and(|r| r.h >= 6)) {
        return false;
    }
    let Some(content) = &layer.content else { return false };
    plan.layers.clear();
    plan.spots.clear();
    plan.regions.clear();
    let w = buf.area.width;
    let bottom = buf.area.height.saturating_sub(1);
    let height = bottom.min(9);
    let top = bottom.saturating_sub(height);
    let style = app.theme.s(Token::Text).patch(app.theme.fill(Token::AccentTint));
    for y in top..bottom {
        for x in 0..w { buf[(x, y)].set_symbol(" ").set_style(style); }
    }
    if height == 0 { return true; }
    let title = content.data.get("title").and_then(Value::as_str).unwrap_or("Guide");
    put_str(buf, 0, top, &format!("compact · {title}"), w as usize, style.add_modifier(Modifier::BOLD));
    let text = content.data.get("text").and_then(Value::as_str).unwrap_or("");
    for (i, line) in wrap(text, w as usize).iter().take(height.saturating_sub(2) as usize).enumerate() {
        put_str(buf, 0, top + 1 + i as u16, line, w as usize, style);
    }
    if height > 1 {
        put_str(buf, 0, bottom - 1, "F2 next  S-F2 back  F3 stop", w as usize, style.add_modifier(Modifier::BOLD));
        for (x, n, action) in [(0, 7, "next"), (9, 9, "back"), (20, 7, "stop")] {
            if x < w { plan.regions.push(cl::Region { rect: cl::Rect::new(x, bottom - 1, n.min(w - x), 1), id: format!("{}/{action}", layer.id) }); }
        }
    }
    true
}

/// The person's keys for layers, before anything else: the `layers` context of the keymap
/// (keymap.rs: F2 / ⇧F2 / F3 for a walkthrough, Esc and ⌘[ for agents' layers, as remapped).
/// True: the key was the layers'.
pub fn key(app: &mut App, k: &ratatui::crossterm::event::KeyEvent) -> bool {
    use crate::keymap::{Ctx, Key};
    if !crate::keymap::layer_keys(app) || !app.pending_keys.is_empty() {
        return false;
    }
    // ⇧F2 arrives as F14 from some terminals.
    let k = if k.code == ratatui::crossterm::event::KeyCode::F(14) { ratatui::crossterm::event::KeyEvent::new(ratatui::crossterm::event::KeyCode::F(2), ratatui::crossterm::event::KeyModifiers::SHIFT) } else { *k };
    match crate::keymap::lookup(app, &[Ctx::Layers], &[Key::of(&k)]) {
        Some(Some(b)) => {
            let action = b.action;
            crate::keymap::run(app, action)
        }
        _ => false,
    }
}
