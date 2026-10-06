//! A click selects the row drawn under the pointer, in every list: the hit
//! regions come from the same loop that draws the rows, so multi-line rows (the Log's
//! transaction and its detail line, wrapped rows), section headings and the scroll offset can't
//! disagree with what's on screen. The sweep scrolls each list, clicks the first, middle and
//! last line of every visible row, and checks the selection and that the view didn't move.

use crate::app::{App, View};
use crate::ui::Click;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Terminal;
use thc_core::builder::TxBuilder;

fn app(tag: &str) -> (crate::fuzz::Scratch, App) {
    let (s, mut vault) = crate::fuzz::scratch(tag);
    let today = thc_core::dates::today();
    // Many separate writes: the Log gets a transaction (and its detail line) for each.
    for i in 0..40 {
        vault
            .transact(|st| {
                let mut b = TxBuilder::new(st, today);
                let text = match i % 4 {
                    0 => format!("[ ] task {i} due:today"),
                    1 => format!("[ ] task {i} with a long text that goes on and on so that on a narrow screen it may wrap onto more than one line #work"),
                    2 => format!("[ ] task {i} due:+{}d !high", i % 5),
                    _ => format!("a note {i} about [[Page {}]]", i % 3),
                };
                b.create_from_capture(None, &thc_core::capture::parse(&text, today).unwrap(), None)?;
                Ok((b.finish(), ()))
            })
            .unwrap();
    }
    crate::SNAPSHOT.with(|s| s.set(true));
    let mut app = App::new(vault).unwrap();
    app.daemon_live = false;
    (s, app)
}

fn draw(app: &mut App, term: &mut Terminal<TestBackend>) {
    term.draw(|f| crate::ui::draw_app(f, app)).unwrap();
}

fn click(app: &mut App, x: u16, y: u16) {
    for kind in [MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left)] {
        crate::input::handle_mouse(app, MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE }, 1);
    }
}

/// The rows on screen: (row index, its lines' ys, an x where a click lands on the row itself).
fn drawn_rows(app: &App) -> Vec<(usize, Vec<u16>, u16)> {
    let mut by: Vec<(usize, Vec<u16>, u16)> = vec![];
    let mut ys: Vec<u16> = app.render.click_targets.iter().filter(|t| matches!(t.what, Click::Row(_))).map(|t| t.y).collect();
    ys.sort();
    ys.dedup();
    for y in ys {
        // The topmost target at a point wins (input.rs): find an x where that's the row, past
        // its box and away from its chips.
        let row_at = |x: u16| app.render.click_targets.iter().rev().find(|t| t.y == y && x >= t.x0 && x < t.x1).map(|t| t.what.clone());
        let Some((x, i)) = (8..90u16).rev().find_map(|x| if let Some(Click::Row(i)) = row_at(x) { Some((x, i)) } else { None }) else { continue };
        match by.iter_mut().find(|(j, _, _)| *j == i) {
            Some((_, v, _)) => v.push(y),
            None => by.push((i, vec![y], x)),
        }
    }
    by
}

#[test]
fn a_click_selects_the_row_under_it_in_every_list() {
    let (_s, mut app) = app("lists");
    let mut fails = vec![];
    for (w, h) in [(100u16, 30u16), (64, 24)] {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        for view in [View::Today, View::Inbox, View::Tasks, View::Pages, View::Log] {
            app.overlay = None;
            app.set_view(view);
            draw(&mut app, &mut term);
            let n = app.rows.len();
            for at in [0, n / 2, n.saturating_sub(1)] {
                // Scroll there the way a person does: move the cursor.
                app.select_row(at);
                draw(&mut app, &mut term);
                let rows = drawn_rows(&app);
                for (i, ys, x) in rows {
                    if !app.rows[i].selectable() {
                        continue;
                    }
                    let picks: Vec<u16> = [ys[0], ys[ys.len() / 2], ys[ys.len() - 1]].into_iter().collect();
                    for y in picks {
                        let scroll = app.scroll;
                        click(&mut app, x, y);
                        if app.cursor != i {
                            fails.push(format!("{w}x{h} {view:?} @{at}: clicked row {i} ({:?}) at y {y}, selected {}", ys, app.cursor));
                            continue;
                        }
                        draw(&mut app, &mut term);
                        if app.scroll != scroll {
                            fails.push(format!("{w}x{h} {view:?} @{at}: clicking row {i} at y {y} scrolled {scroll} → {}", app.scroll));
                        }
                        let under = app.render.click_targets.iter().rev().find(|t| t.y == y && x >= t.x0 && x < t.x1).map(|t| t.what.clone());
                        if !matches!(under, Some(Click::Row(j)) if j == i) {
                            fails.push(format!("{w}x{h} {view:?} @{at}: after the click, y {y} shows {under:?}, not row {i}"));
                        }
                    }
                }
            }
        }
    }
    fails.dedup();
    assert!(fails.is_empty(), "{} misses:\n{}", fails.len(), fails.iter().take(30).cloned().collect::<Vec<_>>().join("\n"));
}
