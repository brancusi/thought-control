//! About (docs/design/about.md): what changed, and what this thc is. A full-pane
//! page in three sections: What's new (every release since the version you last looked at),
//! This thc (version, vault, daemon, terminal, config), and the changelog, bundled into the
//! binary so it works offline. Opening it is the update step: the facts are gathered then and
//! the last-seen version moves; drawing only reads them.

use crate::app::App;
use crate::theme::{Theme, Token};
use crate::ui::{Click, RenderOutput};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use std::sync::OnceLock;
use unicode_width::UnicodeWidthStr;

const NOTES: &str = include_str!("../../../RELEASE_NOTES.md");

/// One release: `**x.y.z**` or `**x.y.z: title**`, then its notes.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub title: String,
    pub body: String,
    /// The title was the first note's bold lead: that note starts after it, not again.
    pub derived: bool,
}

/// Every release in RELEASE_NOTES.md, newest first (the file's order).
pub fn releases() -> &'static [Release] {
    static R: OnceLock<Vec<Release>> = OnceLock::new();
    R.get_or_init(|| parse(NOTES))
}

/// The same split as the website's changelog: a line that is only `**x.y.z**` or
/// `**x.y.z: title**` starts a release. With no title, the first note's bold lead names it.
pub fn parse(src: &str) -> Vec<Release> {
    let mut out: Vec<Release> = Vec::new();
    for line in src.lines() {
        let t = line.trim();
        let head = t.strip_prefix("**").and_then(|r| r.strip_suffix("**")).filter(|h| !h.contains("**"));
        let parsed = head.and_then(|h| {
            let (v, title) = h.split_once(':').map_or((h, ""), |(v, t)| (v, t.trim()));
            let ok = v.split('.').count() == 3 && v.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
            ok.then(|| (v.to_string(), title.to_string()))
        });
        match parsed {
            Some((version, title)) => out.push(Release { version, title, body: String::new(), derived: false }),
            None => {
                if let Some(r) = out.last_mut() {
                    r.body.push_str(line);
                    r.body.push('\n');
                }
            }
        }
    }
    for r in &mut out {
        r.body = r.body.trim().to_string();
        if r.title.is_empty() {
            r.title = r.body.lines().next().and_then(|l| l.split("**").nth(1)).map(|t| t.trim().trim_end_matches('.').to_string()).unwrap_or_default();
            r.derived = !r.title.is_empty();
            // No bold lead: the first note's first sentence, without its marks.
            if r.title.is_empty() {
                let first = r.body.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().trim_start_matches("- ").replace(['`', '*'], "");
                let sentence = first.split(". ").next().unwrap_or("").trim_end_matches('.');
                let mut t: String = sentence.chars().take(60).collect();
                if sentence.chars().count() > 60 {
                    t = format!("{}…", t.trim_end());
                }
                r.title = t;
            }
        }
    }
    out
}

// ---- what you've seen (per device) -------------------------------------------------------------

/// `about.json` beside the update check's file (the device's cache, not a vault's):
/// `seen` is the version About was last opened on, `launched` the last version started.
fn state_file() -> Option<std::path::PathBuf> {
    Some(thc_core::release::check_file()?.with_file_name("about.json"))
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Seen {
    #[serde(default)]
    seen: Option<String>,
    #[serde(default)]
    launched: Option<String>,
}

fn load() -> Option<Seen> {
    serde_json::from_slice(&std::fs::read(state_file()?).ok()?).ok()
}

fn store(s: &Seen) {
    // A snapshot looks but never marks anything seen (it may run on your own machine, and tests
    // share a cache); THC_TUI_SNAPSHOT_ABOUT=1 keeps what it marks (the About tests).
    if crate::SNAPSHOT.with(|x| x.get()) && std::env::var_os("THC_TUI_SNAPSHOT_ABOUT").is_none() {
        return;
    }
    if let Some(p) = state_file() {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, serde_json::to_vec(s).unwrap_or_default());
    }
}

/// Start-up: the first launch of a new version says so once (a toast), and the footer reads
/// `thc x.y.z · new` until About is opened on it. A first install has nothing new.
pub fn on_start(app: &mut App) {
    let current = app.derived.version.clone();
    let mut s = match load() {
        Some(s) => s,
        None => {
            store(&Seen { seen: Some(current.clone()), launched: Some(current) });
            return;
        }
    };
    let updated = s.launched.as_deref().is_some_and(|l| thc_core::release::newer(&current, l));
    if s.launched.as_deref() != Some(current.as_str()) {
        s.launched = Some(current.clone());
        store(&s);
    }
    app.about_new = s.seen.as_deref().is_none_or(|v| thc_core::release::newer(&current, v));
    if updated {
        app.toast_parts(crate::app::ToastKind::Info, vec![(format!("updated to {current}"), Token::Text), (" · what's new: :about".into(), Token::Muted)]);
    }
}

// ---- the page --------------------------------------------------------------------------------

/// About as it was opened: what's new, and the facts about this thc, gathered then.
#[derive(Clone, Debug, PartialEq)]
pub struct About {
    pub scroll: u16,
    /// The releases since the version last looked at (indices into `releases()`), newest first.
    /// Empty: up to date.
    pub new: Vec<usize>,
    /// The version you'd last looked at, when there's something new since.
    pub since: Option<String>,
    pub version: String,
    pub facts: Vec<(&'static str, String, Option<&'static str>)>,
    /// `/`: the search, and whether it's being typed.
    pub search: String,
    pub typing: bool,
    /// Which match `n` / `N` is on.
    pub hit: usize,
}

/// Where each section starts and which lines match, from the last frame (for 1 2 3, n N).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Out {
    pub max_scroll: u16,
    pub sections: [u16; 3],
    pub hits: Vec<u16>,
}

/// Open About (`:about`, the footer version, `space a`); `:changes` opens it at What's new, which
/// is its top. Opening it is looking: the last-seen version moves to this one.
pub fn open(app: &mut App) {
    let current = app.derived.version.clone();
    let mut s = load().unwrap_or_default();
    let all = releases();
    let since = s.seen.clone().filter(|v| thc_core::release::newer(&current, v));
    let new: Vec<usize> = match &since {
        Some(v) => (0..all.len()).filter(|&i| thc_core::release::newer(&all[i].version, v) && !thc_core::release::newer(&all[i].version, &current)).collect(),
        None => vec![],
    };
    s.seen = Some(current.clone());
    s.launched.get_or_insert(current.clone());
    store(&s);
    app.about_new = false;
    let facts = facts(app, &current);
    app.overlay = Some(crate::app::Overlay::About(Box::new(About { scroll: 0, new, since, version: current, facts, search: String::new(), typing: false, hit: 0 })));
}

/// This thc (§2.2): the same facts `thc vault`, `thc daemon status` and the capability probe
/// give, paths with `~`. (label, value, the key that acts on it)
fn facts(app: &App, version: &str) -> Vec<(&'static str, String, Option<&'static str>)> {
    let tilde = |p: &std::path::Path| thc_core::vault::tilde(p);
    let update = thc_core::release::mode();
    let vault_path = app.vault.origin.as_ref().map_or(&app.vault.paths.vault, |o| &o.vault);
    let why = app.vault.source_note.clone().unwrap_or_else(|| if app.vault_home { "your home vault".into() } else { "chosen on this device".into() });
    let daemon = if app.daemon_live { "● live · watching".to_string() } else { "○ offline · :daemon start".to_string() };
    let term = std::env::var("TERM_PROGRAM").ok().filter(|t| !t.is_empty()).map(|t| match std::env::var("TERM_PROGRAM_VERSION").ok().filter(|v| !v.is_empty()) {
        Some(v) => format!("{t} {v}"),
        None => t,
    });
    let yes = |b: bool| if b { "✓" } else { "✗" };
    let images = match crate::images::proto() {
        Some(p) => format!("{p:?}").to_lowercase(),
        None => "chips".to_string(),
    };
    let terminal = format!("{} · kitty keys {} · ⌘ keys {} · images: {images}", term.unwrap_or_else(|| "terminal".into()), yes(app.kitty), yes(app.cmd_seen));
    vec![
        ("version", format!("{version} · updates: {update}"), None),
        ("vault", format!("{} · {} · {why}", app.vault_name, tilde(vault_path)), None),
        ("daemon", daemon, None),
        ("terminal", terminal, None),
        ("doctor", "thc doctor checks the vault, the daemon and the install".to_string(), None),
        ("config", thc_core::policy::config_path_display(), Some("e edit")),
    ]
}

/// A key while About is open. False: it closed.
pub fn key(app: &mut App, a: &mut About, k: ratatui::crossterm::event::KeyEvent) -> bool {
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};
    let out = app.render.about.clone();
    if a.typing {
        match k.code {
            KeyCode::Esc => {
                a.typing = false;
                a.search.clear();
            }
            KeyCode::Enter => {
                a.typing = false;
                a.hit = 0;
                if let Some(&y) = out.hits.first() {
                    a.scroll = y.saturating_sub(2).min(out.max_scroll);
                }
            }
            KeyCode::Backspace => {
                a.search.pop();
            }
            KeyCode::Char(c) if !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                a.search.push(c);
                a.hit = 0;
            }
            _ => {}
        }
        return true;
    }
    let page = app.render.size.height.saturating_sub(6).max(1) as i32;
    let step = match k.code {
        KeyCode::Down | KeyCode::Char('j') => Some(1),
        KeyCode::Up | KeyCode::Char('k') => Some(-1),
        KeyCode::PageDown | KeyCode::Char(' ') => Some(page),
        KeyCode::PageUp => Some(-page),
        KeyCode::Char('g') | KeyCode::Home => Some(-(u16::MAX as i32)),
        KeyCode::Char('G') | KeyCode::End => Some(u16::MAX as i32),
        _ => None,
    };
    if let Some(d) = step {
        a.scroll = (a.scroll as i32 + d).clamp(0, out.max_scroll as i32) as u16;
        return true;
    }
    match k.code {
        KeyCode::Esc | KeyCode::Char('q') => return false,
        KeyCode::Char(c @ '1'..='3') => a.scroll = out.sections[(c as u8 - b'1') as usize].min(out.max_scroll),
        KeyCode::Char('/') => {
            a.typing = true;
            a.search.clear();
            a.hit = 0;
        }
        KeyCode::Char('n') | KeyCode::Char('N') if !out.hits.is_empty() => {
            let n = out.hits.len();
            a.hit = if k.code == KeyCode::Char('n') { (a.hit + 1) % n } else { (a.hit + n - 1) % n };
            a.scroll = out.hits[a.hit].saturating_sub(2).min(out.max_scroll);
        }
        KeyCode::Char('e') => {
            app.editor_request = Some("@config".into());
            return false;
        }
        _ => {}
    }
    true
}

/// The wheel over About.
pub fn wheel(app: &App, a: &mut About, down: bool) {
    let d: i32 = if down { 3 } else { -3 };
    a.scroll = (a.scroll as i32 + d).clamp(0, app.render.about.max_scroll as i32) as u16;
}

// ---- drawing ---------------------------------------------------------------------------------

/// Text with `**bold**` and `` `code` `` as styled pieces.
fn inline(th: &Theme, text: &str, base: Style) -> Vec<(String, Style)> {
    let mut out = Vec::new();
    let (mut bold, mut code) = (false, false);
    let mut cur = String::new();
    let style = |bold: bool, code: bool| {
        if code {
            th.s(Token::Tag)
        } else if bold {
            base.add_modifier(Modifier::BOLD)
        } else {
            base
        }
    };
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '`' {
            out.push((std::mem::take(&mut cur), style(bold, code)));
            code = !code;
        } else if c == '*' && !code && chars.peek() == Some(&'*') {
            chars.next();
            out.push((std::mem::take(&mut cur), style(bold, code)));
            bold = !bold;
        } else {
            cur.push(c);
        }
    }
    out.push((cur, style(bold, code)));
    out.into_iter().filter(|(t, _)| !t.is_empty()).collect()
}

/// Pieces wrapped to `width`, every line after the first indented by `hang`.
fn wrap(pieces: Vec<(String, Style)>, first: &str, hang: usize, width: usize) -> Vec<Vec<(String, Style)>> {
    let mut lines: Vec<Vec<(String, Style)>> = vec![vec![(first.to_string(), Style::default())]];
    let mut used = first.width();
    for (text, st) in pieces {
        for (i, word) in text.split(' ').enumerate() {
            let w = word.width() + usize::from(i > 0);
            if used + w > width && used > hang && !word.is_empty() {
                lines.push(vec![(" ".repeat(hang), Style::default())]);
                used = hang;
                lines.last_mut().unwrap().push((word.to_string(), st));
                used += word.width();
                continue;
            }
            let piece = if i > 0 { format!(" {word}") } else { word.to_string() };
            used += piece.width();
            lines.last_mut().unwrap().push((piece, st));
        }
    }
    lines
}

/// A release's notes: each `- ` bullet a `·` with a hang, other text as it is.
fn release_lines(th: &Theme, r: &Release, width: usize) -> Vec<Vec<(String, Style)>> {
    let mut out = vec![vec![(format!("{:<8}", r.version), th.s(Token::Accent)), (r.title.clone(), th.strong())]];
    let mut para = String::new();
    let mut first = r.derived;
    let mut flush = |para: &mut String, out: &mut Vec<Vec<(String, Style)>>| {
        let mut t = para.trim().to_string();
        para.clear();
        if t.is_empty() {
            return;
        }
        // The title came from this note's bold lead: the note goes on after it.
        if std::mem::take(&mut first) {
            if let Some(rest) = t.strip_prefix("- **").and_then(|r| r.split_once("**")).map(|(_, rest)| rest.trim_start().to_string()) {
                t = format!("- {rest}");
            }
        }
        let (lead, text, hang) = match t.strip_prefix("- ") {
            Some(rest) => ("   · ", rest.to_string(), 5),
            None => ("   ", t, 3),
        };
        out.extend(wrap(inline(th, &text, th.s(Token::Text)), lead, hang, width));
    };
    for line in r.body.lines() {
        let l = line.trim_end();
        if l.trim().is_empty() || l.trim_start().starts_with("- ") {
            flush(&mut para, &mut out);
        }
        para.push(' ');
        para.push_str(l.trim());
    }
    flush(&mut para, &mut out);
    out
}

fn to_line(pieces: Vec<(String, Style)>, search: &str, th: &Theme) -> (Line<'static>, bool) {
    let mut hit = false;
    let spans: Vec<Span<'static>> = if search.is_empty() {
        pieces.into_iter().map(|(t, s)| Span::styled(t, s)).collect()
    } else {
        let needle = search.to_lowercase();
        let mut v = Vec::new();
        for (t, s) in pieces {
            let low = t.to_lowercase();
            // Lowercasing can change byte lengths (rare letters): such a piece isn't marked.
            if low.len() != t.len() {
                v.push(Span::styled(t, s));
                continue;
            }
            let mut at = 0;
            while let Some(i) = low[at..].find(&needle) {
                let (a, b) = (at + i, at + i + needle.len());
                if a > at {
                    v.push(Span::styled(t[at..a].to_string(), s));
                }
                v.push(Span::styled(t[a..b].to_string(), th.s(Token::Accent).add_modifier(Modifier::REVERSED)));
                hit = true;
                at = b;
            }
            if at < t.len() {
                v.push(Span::styled(t[at..].to_string(), s));
            }
        }
        v
    };
    (Line::from(spans), hit)
}

/// The page, over the whole screen but the footer.
pub fn draw(render: &mut RenderOutput, f: &mut Frame, app: &App, area: Rect, a: &About) {
    let th = app.theme;
    let r = Rect { height: area.height.saturating_sub(1), ..area };
    f.render_widget(Clear, r);
    f.render_widget(ratatui::widgets::Block::default().style(th.s(Token::Text).bg(th.s(Token::Bg).bg.unwrap_or_default())), r);
    crate::ui::set_overlay_rect(render, r);
    if r.height < 4 || r.width < 20 {
        return;
    }
    // The view is two narrower than the screen, and every body line starts with a space.
    let width = r.width.saturating_sub(3) as usize;
    // The header: the sections, clickable, and Esc.
    let mut head: Vec<Span<'static>> = vec![Span::styled(" About thc", th.strong())];
    let mut x = r.x + 10;
    for (i, name) in ["What's new", "This thc", "Changelog"].iter().enumerate() {
        head.push(Span::styled(" · ", th.s(Token::Muted)));
        x += 3;
        head.push(Span::styled(name.to_string(), th.s(Token::Text)));
        crate::ui::target(render, x, x + name.width() as u16, r.y, Click::Key(ratatui::crossterm::event::KeyCode::Char((b'1' + i as u8) as char), ratatui::crossterm::event::KeyModifiers::NONE));
        x += name.width() as u16;
    }
    let right = if a.typing || !a.search.is_empty() { format!("/{}{}   Esc ", a.search, if a.typing { "▏" } else { "" }) } else { "Esc close ".to_string() };
    let pad = (r.width as usize).saturating_sub(head.iter().map(|s| s.content.width()).sum::<usize>() + right.width());
    head.push(Span::raw(" ".repeat(pad)));
    head.push(Span::styled(right.clone(), th.s(Token::Muted)));
    crate::ui::target(render, r.right().saturating_sub(right.width() as u16), r.right(), r.y, Click::Key(ratatui::crossterm::event::KeyCode::Esc, ratatui::crossterm::event::KeyModifiers::NONE));
    f.render_widget(Paragraph::new(Line::from(head)), Rect { height: 1, ..r });
    f.render_widget(Paragraph::new(Line::styled("─".repeat(r.width as usize), th.s(Token::Line))), Rect { y: r.y + 1, height: 1, ..r });

    // The body, every line, then the visible part.
    let all = &app.derived.data.overlay.releases;
    let mut body: Vec<Vec<(String, Style)>> = Vec::new();
    let mut sections = [0u16; 3];
    let right_align = |left: Vec<(String, Style)>, right: (String, Style)| {
        let used: usize = left.iter().map(|(t, _)| t.width()).sum();
        let mut v = left;
        v.push((" ".repeat(width.saturating_sub(used + right.0.width())), Style::default()));
        v.push(right);
        v
    };
    sections[0] = body.len() as u16;
    if a.new.is_empty() {
        body.push(right_align(vec![(" What's new".into(), th.strong())], (format!("You're up to date · {}", a.version), th.s(Token::Muted))));
        body.push(vec![]);
        if let Some(r0) = all.iter().find(|r| r.version == a.version).or(all.first()) {
            body.extend(release_lines(&th, r0, width).into_iter().map(|mut l| {
                l.insert(0, (" ".into(), Style::default()));
                l
            }));
        }
    } else {
        let n = a.new.len();
        let title = format!(" What's new since {} ({n} release{})", a.since.clone().unwrap_or_default(), if n == 1 { "" } else { "s" });
        body.push(right_align(vec![(title, th.strong())], ("new since you last looked".into(), th.s(Token::Accent))));
        for &i in &a.new {
            body.push(vec![]);
            body.extend(release_lines(&th, &all[i], width).into_iter().map(|mut l| {
                l.insert(0, (" ".into(), Style::default()));
                l
            }));
        }
    }
    body.push(vec![]);
    sections[1] = body.len() as u16;
    body.push(vec![(" This thc".into(), th.strong())]);
    for (label, value, act) in &a.facts {
        let room = width.saturating_sub(13 + act.map_or(0, |k| k.width() + 2));
        let value = if value.width() > room { format!("{}…", value.chars().scan(0, |w, c| { *w += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0); (*w < room).then_some(c) }).collect::<String>()) } else { value.clone() };
        let left = vec![(format!("   {label:<10}"), th.s(Token::Muted)), (value, th.s(Token::Text))];
        body.push(match act {
            Some(k) => right_align(left, (k.to_string(), th.s(Token::Muted))),
            None => left,
        });
    }
    body.push(vec![]);
    sections[2] = body.len() as u16;
    body.push(right_align(vec![(format!(" Changelog · {} releases", all.len()), th.strong())], ("/ search".into(), th.s(Token::Muted))));
    let changelog_from = body.len();
    for r0 in all {
        body.push(vec![]);
        body.extend(release_lines(&th, r0, width).into_iter().map(|mut l| {
            l.insert(0, (" ".into(), Style::default()));
            l
        }));
    }
    let search = if a.search.is_empty() { "" } else { a.search.as_str() };
    let mut hits = Vec::new();
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(body.len());
    for (i, pieces) in body.into_iter().enumerate() {
        let (line, hit) = to_line(pieces, if i >= changelog_from { search } else { "" }, &th);
        if hit {
            hits.push(i as u16);
        }
        lines.push(line);
    }
    let view = Rect { x: r.x + 1, y: r.y + 2, width: r.width.saturating_sub(2), height: r.height.saturating_sub(2) };
    let max_scroll = (lines.len() as u16).saturating_sub(view.height);
    let scroll = a.scroll.min(max_scroll);
    // A page of text (the sweep: declared, not forgotten); the config line acts: `e`, or a click.
    for y in r.y + 1..r.bottom() {
        crate::ui::inert(render, Rect { y, height: 1, ..r }, y);
    }
    if let Some(i) = a.facts.iter().position(|(l, _, _)| *l == "config") {
        let y = sections[1] as i32 + 1 + i as i32 - scroll as i32;
        if y >= 0 && (y as u16) < view.height {
            crate::ui::target(render, view.x, view.right(), view.y + y as u16, Click::Key(ratatui::crossterm::event::KeyCode::Char('e'), ratatui::crossterm::event::KeyModifiers::NONE));
        }
    }
    let shown: Vec<Line<'static>> = lines.into_iter().skip(scroll as usize).take(view.height as usize).collect();
    f.render_widget(Paragraph::new(shown), view);
    render.about = Out { max_scroll, sections, hits };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releases_parse_like_the_website() {
        let r = parse("**0.9.2: Big one**\n\n- **First.** text\n- two\n\n**0.9.1**\n\n- **Fixed a thing.** more\n\n**not a version**\n");
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].version.as_str(), r[0].title.as_str()), ("0.9.2", "Big one"));
        assert_eq!((r[1].version.as_str(), r[1].title.as_str()), ("0.9.1", "Fixed a thing"));
        assert!(r[1].body.contains("**not a version**"));
        // The bundled notes: every release, newest first, each with a title.
        let all = releases();
        assert!(all.len() > 50, "{}", all.len());
        assert!(all.iter().all(|r| !r.title.is_empty()), "{:?}", all.iter().find(|r| r.title.is_empty()));
        assert!(thc_core::release::newer(&all[0].version, &all[1].version));
    }

    #[test]
    fn notes_render_as_text() {
        let th = crate::theme::Theme::detect();
        let r = Release { version: "1.2.3".into(), title: "T".into(), derived: false, body: "- **Bold lead.** then `code` and more words that wrap around the narrow width here".into() };
        let lines = release_lines(&th, &r, 30);
        let text: Vec<String> = lines.iter().map(|l| l.iter().map(|(t, _)| t.as_str()).collect()).collect();
        assert_eq!(text[0].trim_end(), "1.2.3   T");
        assert!(text[1].starts_with("   · Bold lead. then code"), "{text:?}");
        assert!(text.iter().all(|t| !t.contains("**") && !t.contains('`')), "{text:?}");
        assert!(text[2..].iter().all(|t| t.starts_with("     ")), "the hang: {text:?}");
        assert!(text.iter().all(|t| t.width() <= 30), "{text:?}");
        let code = lines[1].iter().find(|(t, _)| t.contains("code")).unwrap();
        assert_eq!(code.1, th.s(Token::Tag));
    }
}

#[cfg(test)]
mod footer_tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// The footer version is About's door: a click target, and `· new` until About is opened.
    #[test]
    fn the_footer_version_opens_about() {
        let (_s, vault) = crate::fuzz::scratch("about-footer");
        crate::SNAPSHOT.with(|s| s.set(true));
        let mut app = crate::app::App::new(vault).unwrap();
        app.daemon_live = false;
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        app.about_new = true;
        term.draw(|f| crate::ui::draw_app(f, &mut app)).unwrap();
        let last: String = (0..120).map(|x| term.backend().buffer()[(x, 29)].symbol().to_string()).collect();
        let t = app.render.click_targets.iter().find(|t| t.what == crate::ui::Click::Action("about")).unwrap_or_else(|| panic!("no target on the version: {last:?}"));
        let row: String = (t.x0..t.x1).map(|x| term.backend().buffer()[(x, t.y)].symbol().to_string()).collect();
        assert!(row.starts_with("thc ") && row.ends_with("· new"), "{row:?}");
        crate::keymap::run(&mut app, "about");
        assert!(matches!(app.overlay, Some(crate::app::Overlay::About(_))));
        assert!(!app.about_new);
    }
}
