//! Every overlay is clickable (mouse.md "Overlays are menus"). The sweep opens
//! each overlay in turn and checks that every row it draws is either a hit region or declared
//! as text (`Click::Text`): a new popup drawn without targets fails here. Then a click on a
//! row does what the keyboard does for it.

use crate::app::{App, Overlay, View};
use crate::input::LineInput;
use crate::ui::Click;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Terminal;
use thc_core::builder::TxBuilder;

fn app(tag: &str) -> (crate::fuzz::Scratch, App) {
    let (s, mut vault) = crate::fuzz::scratch(tag);
    let today = thc_core::dates::today();
    vault
        .transact(|st| {
            let mut b = TxBuilder::new(st, today);
            b.create_from_capture(None, &thc_core::capture::parse("[ ] Buy milk", today).unwrap(), None)?;
            b.create_page("Lisbon flat", &[])?;
            Ok((b.finish(), ()))
        })
        .unwrap();
    crate::SNAPSHOT.with(|s| s.set(true));
    let mut app = App::new(vault).unwrap();
    app.daemon_live = false;
    app.set_view(View::Inbox);
    (s, app)
}

fn draw(app: &mut App, term: &mut Terminal<TestBackend>) -> ratatui::buffer::Buffer {
    term.draw(|f| crate::ui::draw_app(f, app)).unwrap();
    term.backend().buffer().clone()
}

fn key(app: &mut App, code: KeyCode) {
    crate::input::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
}

fn mouse(app: &mut App, kind: MouseEventKind, x: u16, y: u16) {
    crate::input::handle_mouse(app, MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE }, 1);
}

fn click(app: &mut App, x: u16, y: u16) {
    mouse(app, MouseEventKind::Down(MouseButton::Left), x, y);
    mouse(app, MouseEventKind::Up(MouseButton::Left), x, y);
}

/// Rows inside the overlay's box (borders excluded) that show something and have no target.
fn uncovered(app: &App, buf: &ratatui::buffer::Buffer) -> Vec<String> {
    let Some(r) = app.render.overlay_rect else { return vec!["no overlay rect recorded".into()] };
    // The capture drawer has no border: every row of it counts.
    let b = u16::from(!matches!(app.overlay, Some(Overlay::Capture { .. })));
    let mut out = vec![];
    for y in r.y + b..r.bottom().saturating_sub(b) {
        let text: String = (r.x + b..r.right().saturating_sub(b)).map(|x| buf[(x, y)].symbol().to_string()).collect();
        if text.trim().is_empty() {
            continue;
        }
        let hit = app.render.click_targets.iter().any(|t| t.y == y && t.x0 < r.right() && t.x1 > r.x);
        if !hit {
            out.push(text.trim_end().to_string());
        }
    }
    out
}

/// Every overlay, opened the way a person opens it.
fn openers() -> Vec<(&'static str, Box<dyn Fn(&mut App)>)> {
    let ov = |o: fn() -> Overlay| -> Box<dyn Fn(&mut App)> { Box::new(move |a: &mut App| a.overlay = Some(o())) };
    vec![
        ("help", ov(|| Overlay::Help { all: false, scroll: 0 })),
        ("help all", ov(|| Overlay::Help { all: true, scroll: 0 })),
        ("palette", ov(|| Overlay::Palette { input: LineInput::default(), sel: 0 })),
        ("finder", ov(|| Overlay::Finder { input: LineInput::default(), sel: 0 })),
        ("finder no match", ov(|| Overlay::Finder { input: LineInput::with("zzzzqqq"), sel: 0 })),
        ("focus", ov(|| Overlay::Focus)),
        ("history", ov(|| Overlay::History { sel: 0 })),
        ("recipe", ov(|| Overlay::Recipe { name: "inbox".into() })),
        ("about", Box::new(crate::about::open)),
        ("scope", Box::new(|a: &mut App| a.overlay = Some(Overlay::Scope { sel: 0, picked: a.scope_choices() }))),
        // (The test registry is empty and shared: the picker's rows are made here.)
        ("vaults", Box::new(picker)),
        (
            "new vault",
            Box::new(|a: &mut App| {
                picker(a);
                key(a, KeyCode::Char('n'));
            }),
        ),
        (
            "move",
            Box::new(|a: &mut App| {
                let i = a.rows.iter().position(|r| r.key().is_some()).expect("a row");
                a.select_row(i);
                crate::keymap::run(a, "node.move");
            }),
        ),
        ("capture", Box::new(|a: &mut App| { crate::keymap::run(a, "capture.here"); })),
        (
            "compare",
            Box::new(|a: &mut App| {
                let node = a.rows.iter().find_map(|r| r.key()).unwrap();
                let v = |t: &str, actor: &str| thc_core::model::ConflictVersion { text: t.into(), dev: "other".into(), actor: actor.into(), ms: 0, eid: None };
                a.overlay = Some(Overlay::Compare { detail: thc_core::model::ConflictDetail { id: 1, node, kind: "text".into(), current: Some(v("Buy milk", "human")), other: Some(v("Buy oat milk", "agent:claude")), base: Some("Buy".into()) } });
            }),
        ),
    ]
}

#[test]
fn help_remap_footer_is_a_working_menu_action() {
    let (_s, mut app) = app("help-remap");
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    app.overlay = Some(Overlay::Help { all: false, scroll: 0 });
    draw(&mut app, &mut term);
    let footer = app.render.click_targets.iter().find(|t| matches!(t.what, Click::Action("keys.remap"))).expect("remap footer target").clone();
    click(&mut app, footer.x0, footer.y);
    assert!(app.overlay.is_none());
    assert_eq!(app.editor_request.as_deref(), Some("@keys"));
}

fn picker(a: &mut App) {
    let row = |name: &str, current: bool, home: bool| crate::app::VaultRow { name: name.into(), path: std::env::temp_dir().join(format!("thc-overlays-{name}")), open: Some(1), inbox: Some(0), sync: "local only", current, home, accent: "ember".into() };
    a.overlay = Some(Overlay::Vaults { rows: vec![row("acme", false, false), row("side", false, false), row("personal", true, true)], sel: 2, naming: None });
}

#[test]
fn every_overlay_row_is_a_hit_region_or_declared_text() {
    let (_s, mut app) = app("sweep");
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let mut fails = vec![];
    for (name, open) in openers() {
        app.overlay = None;
        app.set_view(View::Inbox);
        draw(&mut app, &mut term);
        open(&mut app);
        assert!(app.overlay.is_some(), "{name} didn't open");
        let buf = draw(&mut app, &mut term);
        let bad = uncovered(&app, &buf);
        if !bad.is_empty() {
            fails.push(format!("{name}: {bad:#?}"));
        }
    }
    assert!(fails.is_empty(), "rows a click can't reach (give them a target, or Click::Text):\n{}", fails.join("\n"));
}

/// The rows of the overlay as drawn: (index, y, x of the row).
fn menu_rows(app: &App) -> Vec<(usize, u16, u16)> {
    app.render.click_targets.iter().filter_map(|t| if let Click::Menu(i) = t.what { Some((i, t.y, t.x0 + 1)) } else { None }).collect()
}

#[test]
fn a_click_on_a_menu_row_does_what_its_keys_do() {
    // For each list overlay, row k: a click vs ↓×k then Enter, from the same start.
    let lists = ["palette", "finder", "vaults", "move"];
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    for name in lists {
        let (_s1, mut by_click) = app(&format!("click-{}", name.replace(' ', "")));
        let (_s2, mut by_keys) = app(&format!("keys-{}", name.replace(' ', "")));
        let open = &openers().into_iter().find(|(n, _)| *n == name).unwrap().1;
        for a in [&mut by_click, &mut by_keys] {
            draw(a, &mut term);
            open(a);
        }
        draw(&mut by_click, &mut term);
        let rows = menu_rows(&by_click);
        assert!(!rows.is_empty(), "{name}: no menu rows");
        let (k, y, x) = rows[rows.len().min(2) - 1];
        // Hover selects it.
        mouse(&mut by_click, MouseEventKind::Moved, x, y);
        assert_eq!(by_click.overlay.as_mut().and_then(|o| o.sel_mut()).copied(), Some(k), "{name}: hover selects");
        click(&mut by_click, x, y);
        let from = by_keys.overlay.as_mut().and_then(|o| o.sel_mut()).copied().unwrap_or(0);
        for _ in 0..k.abs_diff(from) {
            key(&mut by_keys, if k > from { KeyCode::Down } else { KeyCode::Up });
        }
        key(&mut by_keys, KeyCode::Enter);
        let state = |a: &App| (a.view, a.overlay.as_ref().map(|o| format!("{o:?}").split(' ').next().unwrap_or_default().to_string()), a.page_open.clone(), a.journal_date, a.vault.paths.vault.file_name().map(|s| s.to_owned()), a.prompt.as_ref().map(|p| format!("{:?}", p.0)), a.switch_to.clone());
        assert_eq!(state(&by_click), state(&by_keys), "{name}: row {k}");
    }
}

#[test]
fn the_wheel_moves_a_menu_and_a_click_outside_closes_it() {
    let (_s, mut app) = app("wheel");
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    draw(&mut app, &mut term);
    app.overlay = Some(Overlay::Palette { input: LineInput::default(), sel: 0 });
    draw(&mut app, &mut term);
    mouse(&mut app, MouseEventKind::ScrollDown, 50, 10);
    assert_eq!(app.overlay.as_mut().and_then(|o| o.sel_mut()).copied(), Some(1));
    mouse(&mut app, MouseEventKind::ScrollUp, 50, 10);
    assert_eq!(app.overlay.as_mut().and_then(|o| o.sel_mut()).copied(), Some(0));
    // A click in the field places the caret.
    app.overlay = Some(Overlay::Palette { input: LineInput::with("vault"), sel: 0 });
    draw(&mut app, &mut term);
    let (x0, y) = app.render.click_targets.iter().find_map(|t| if let Click::Caret { x0 } = t.what { Some((x0, t.y)) } else { None }).unwrap();
    click(&mut app, x0 + 2, y);
    assert!(matches!(&app.overlay, Some(Overlay::Palette { input, .. }) if input.cur == 2));
    // Outside: closed, and nothing under it ran.
    let view = app.view;
    click(&mut app, 0, 29);
    assert!(app.overlay.is_none() && app.view == view);
}

#[test]
fn the_link_popup_is_a_menu_too() {
    let (_s, mut app) = app("linkpop");
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    app.journal_date = app.today;
    app.set_view(View::Journal);
    draw(&mut app, &mut term);
    for c in "see [[Lis".chars() {
        key(&mut app, KeyCode::Char(c));
    }
    draw(&mut app, &mut term);
    assert!(app.link_open, "the popup opened");
    let rows: Vec<(usize, u16, u16)> = app.render.click_targets.iter().filter_map(|t| if let Click::LinkRow(i) = t.what { Some((i, t.y, t.x0 + 1)) } else { None }).collect();
    assert!(!rows.is_empty(), "its rows are targets");
    let (i, y, x) = rows[0];
    mouse(&mut app, MouseEventKind::Moved, x, y);
    assert_eq!(app.link_sel, Some(i), "hover selects");
    click(&mut app, x, y);
    let d = app.doc.as_ref().unwrap();
    assert!(d.caret_block().text.contains("[[Lisbon flat]]"), "a click inserts: {:?}", d.caret_block().text);
}
