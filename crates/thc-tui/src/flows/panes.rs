//! Panes: the main view and a sidebar panel are the same editor (docs/design/panes.md). A
//! parity flow runs one script on Q4 Plan twice, once in the main view and once in a panel
//! beside today's journal, and compares what the person gets: the saved document, the caret
//! after each step, the `[[` popup's rows as drawn, and what each step did to the main view.
//!
//! The roles differ only where the design says so, and each difference is named here:
//!
//! - **Arrival.** The main view arrives on a fresh line after the notes; a new panel shows its
//!   page from the top (owner's decision). Scripts start by clicking where they edit.
//! - **Leave (Esc).** In the main view Esc leaves the document for where it came from; in a
//!   panel it hands the keyboard back to the main view. Both: the pane loses the keyboard.
//! - **Follow** ([`Except::Follow`]). A link followed from a panel opens in the main view
//!   (`policy::PANEL_CLICK_FOLLOWS_IN_MAIN`).
//! - **History.** Only the main view's moves are history (⌘[ / ⌘]): a panel never changes it.
//!
//! In the panel run, every step must leave the main view's document and history as they were
//! (bar a documented exception): that's where a panel's key running against the main view's
//! state shows.
//!
//! Bug repros for the panes refactor come after the parity flows, ignored on their board task.
use super::*;
use crate::sidebar::PanelKey;

/// Where a parity script runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Where {
    Main,
    Panel,
}

/// Q4 Plan in the main view (⌃O), or in a panel beside today's journal (⇧-click its link,
/// ⌥S), with the keyboard either way.
pub fn flow_in(name: &str, w: Where) -> Flow {
    let mut f = flow(name);
    match w {
        Where::Main => {
            f.keys("<c-o>Q4 Plan<cr>");
            f.expect_page("Q4 Plan");
        }
        Where::Panel => {
            f.keys("T");
            f.shift_click(text("Q4 Plan").in_doc());
            f.expect_panels(1).keys("<m-s>").expect_focus(Focus::Sidebar);
            f.pane = PaneId::Panel(q4_key(&f));
        }
    }
    f
}

fn q4_key(f: &Flow) -> PanelKey {
    f.s.app.ui.sidebar.open.iter().map(|p| p.key()).find(|k| f.s.app.panel_doc(k).is_some_and(|d| matches!(&d.target, crate::editor::Target::Page { title, .. } if title == "Q4 Plan"))).expect("Q4 Plan beside")
}

/// A documented role difference a parity script may show (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Except {
    /// A link followed: from a panel it opens in the main view, so the main view's document
    /// and history change in the panel run too.
    Follow,
}

/// One step as the person sees it, in the pane under test.
#[derive(Clone, Debug, PartialEq)]
pub struct Rec {
    pub desc: String,
    /// The pane under test still shows the document.
    pub shown: bool,
    /// It has the keyboard.
    pub focused: bool,
    /// Its caret (the line's text, the byte), while it has the keyboard.
    pub caret: Option<(String, usize)>,
    /// The `[[` popup's rows as drawn on screen.
    pub popup: Option<Vec<String>>,
    /// The document's notes (depth, status, text), without the fresh line the document
    /// arrived with (the main view's arrival: it differs by role).
    pub lines: Vec<(usize, Option<String>, String)>,
    /// What the last copy or cut put on the clipboard.
    pub clip: Option<String>,
    /// The main view's document and history (entries, position): a panel's step leaves them.
    pub main_doc: Option<String>,
    pub history: (usize, usize),
}

impl Rec {
    pub fn of(f: &mut Flow, desc: &str) -> Rec {
        let pane = f.pane.clone();
        let shown = f.shot.pane(&pane).is_some() || f.pane_doc().is_some();
        let focused = f.shot.focused.as_ref() == Some(&pane);
        let caret = if focused {
            with_pane_doc(&mut f.s.app, &pane, |d| {
                let c = d.caret();
                (d.blocks()[c.line].text.clone(), c.byte)
            })
        } else {
            None
        };
        let popup_rows: Vec<String> = f.s.app.render.click_targets.iter().filter(|t| matches!(t.what, crate::ui::Click::LinkRow(_))).map(|t| f.shot.row(t.y, t.x0, t.x1).trim().to_string()).collect();
        let popup = (!popup_rows.is_empty()).then_some(popup_rows);
        let mut lines: Vec<(usize, Option<String>, String)> = f.pane_doc().map(|d| d.blocks().iter().map(|l| (l.depth, l.status.clone(), l.text.clone())).collect()).unwrap_or_default();
        if f.pane_doc().is_some_and(|d| d.has_fresh_end()) {
            lines.pop();
        }
        let clip = crate::runtime_effects::SNAPSHOT_CLIPBOARD.with(|c| c.borrow().clone());
        let main_doc = f.s.app.doc.as_ref().map(|d| crate::doc_app::caret_key(&d.target));
        let h = &f.s.app.ui.history;
        Rec { desc: desc.to_string(), shown, focused, caret, popup, lines, clip, main_doc, history: (h.entries.len(), h.pos) }
    }
}

/// A parity flow: [`parity`]`(name).run(script)`.
pub struct Parity {
    name: String,
    except: Vec<Except>,
}

pub fn parity(name: &str) -> Parity {
    Parity { name: name.to_string(), except: Vec::new() }
}

impl Parity {
    pub fn except(mut self, e: Except) -> Self {
        self.except.push(e);
        self
    }

    /// Run `script` in the main view and in a panel, and compare.
    pub fn run(self, script: impl Fn(&mut Flow)) {
        crate::runtime_effects::SNAPSHOT_CLIPBOARD.with(|c| *c.borrow_mut() = None);
        let mut m = flow_in(&format!("{} (main)", self.name), Where::Main);
        m.trail = Some(Vec::new());
        script(&mut m);
        let main_saved = saved_q4(&mut m);
        crate::runtime_effects::SNAPSHOT_CLIPBOARD.with(|c| *c.borrow_mut() = None);
        let mut p = flow_in(&format!("{} (panel)", self.name), Where::Panel);
        let start = Rec::of(&mut p, "start");
        p.trail = Some(Vec::new());
        script(&mut p);
        let panel_saved = saved_q4(&mut p);
        let (tm, tp) = (m.trail.take().unwrap(), p.trail.take().unwrap());
        let fail = |msg: String| -> ! { panic!("\nparity `{}`: {msg}\n--- main ---\n{}\n--- panel ---\n{}\n", self.name, m.shot.frame(), p.shot.frame()) };
        if tm.len() != tp.len() {
            fail(format!("the main run took {} steps, the panel run {}", tm.len(), tp.len()));
        }
        let follows = self.except.contains(&Except::Follow);
        for (a, b) in tm.iter().zip(&tp) {
            // The panel's role: the main view's document and history stay as they were.
            if !follows && (b.main_doc != start.main_doc || b.history != start.history) {
                fail(format!("step `{}` in the panel changed the main view: document {:?} → {:?}, history {:?} → {:?}", b.desc, start.main_doc, b.main_doc, start.history, b.history));
            }
            if a.focused != b.focused {
                fail(format!("step `{}`: the pane has the keyboard in main: {}, in the panel: {}", a.desc, a.focused, b.focused));
            }
            if a.caret != b.caret {
                fail(format!("step `{}`: the caret in main {:?}, in the panel {:?}", a.desc, a.caret, b.caret));
            }
            if a.popup != b.popup {
                fail(format!("step `{}`: the [[ popup in main {:?}, in the panel {:?}", a.desc, a.popup, b.popup));
            }
            // While both still show it (main's Esc leaves the document).
            if a.shown && b.shown && a.lines != b.lines {
                fail(format!("step `{}`: the document differs\n  main  {:?}\n  panel {:?}", a.desc, a.lines, b.lines));
            }
            if a.clip != b.clip {
                fail(format!("step `{}`: the clipboard in main {:?}, in the panel {:?}", a.desc, a.clip, b.clip));
            }
        }
        if main_saved != panel_saved {
            fail(format!("the saved Q4 Plan differs\n  main  {main_saved:?}\n  panel {panel_saved:?}"));
        }
        m.done();
        p.done();
    }
}

/// Q4 Plan as saved: each note's depth, status and text.
fn saved_q4(f: &mut Flow) -> Vec<(usize, Option<String>, String)> {
    f.save();
    let st = &f.s.app.vault.store;
    let page = st.nodes_where("1=1", &[]).unwrap_or_default().into_iter().find(|n| n.title.as_deref() == Some("Q4 Plan") && !n.deleted).expect("Q4 Plan");
    thc_core::outline::render_for_editor(st, &page.id).unwrap_or_default().iter().map(|b| (b.depth, b.status.clone(), b.text.clone())).collect()
}

/// A task's box in the pane under test: the cell a click there hits as the box.
fn task_box(f: &mut Flow, line: &str) -> At {
    let r = f.shot.pane(&f.pane).expect("the pane under test is on screen").rect;
    let pane = f.pane.clone();
    with_pane_doc(&mut f.s.app, &pane, |d| {
        let idx = d.blocks().iter().position(|l| l.text.contains(line))?;
        (r.y..r.y + r.height).flat_map(|y| (r.x..r.x + r.width).map(move |x| (x, y))).find(|&(x, y)| pane_hit(d, r, x, y) == Some((idx, 0, true)))
    })
    .flatten()
    .map(|(x, y)| At::Cell(x, y))
    .unwrap_or_else(|| panic!("no box for {line:?} on screen"))
}

/// The middle of the pane under test.
fn pane_middle(f: &Flow) -> At {
    let r = f.shot.pane(&f.pane).expect("the pane under test is on screen").rect;
    At::Cell(r.x + r.width / 2, r.y + r.height / 2)
}

// ---- parity flows ----------------------------------------------------------------------------

#[test]
fn parity_typing_and_enter() {
    parity("typing and Enter").run(|f| {
        f.click_caret(doc_at("Grow the newsletter", 19)).type_text(" weekly\nand a podcast").expect_caret_after("and a podcast");
    });
}

#[test]
fn parity_tab_and_shift_tab() {
    parity("Tab and ⇧Tab").run(|f| {
        f.click_caret(doc_at("Grow the newsletter", 19)).type_text("\nNested").keys("<tab>").type_text(" more").keys("<s-tab>").expect_caret_after("Nested more");
    });
}

#[test]
fn parity_ctrl_t_cycles_a_task() {
    parity("⌃T cycles a task").run(|f| {
        f.click_caret(doc_at("Draft the budget", 6)).keys("<c-t>").keys("<c-t>");
    });
}

#[test]
fn parity_ctrl_end_then_type() {
    // ⌃End goes to the fresh line after the notes in any pane (fresh_end is per view).
    parity("⌃End then type").run(|f| {
        f.click_caret(doc_at("Grow the newsletter", 4)).keys("<c-end>").type_text("At the end");
    });
}

#[test]
fn parity_esc() {
    parity("Esc").run(|f| {
        f.click_caret(doc_at("Grow the newsletter", 19)).type_text("!").keys("<esc>");
    });
}

#[test]
fn parity_link_popup() {
    parity("[[Gar, ↓, Enter").run(|f| {
        f.click_caret(doc_at("Last line of the plan", 21)).type_text(" [[Gar").keys("<down>").keys("<cr>");
    });
}

#[test]
fn parity_paste_lines() {
    parity("paste lines").run(|f| {
        f.click_caret(doc_at("Grow the newsletter", 19)).paste("\nalpha\nbeta");
    });
}

#[test]
fn parity_undo_and_redo() {
    parity("undo and redo").run(|f| {
        f.click_caret(doc_at("Grow the newsletter", 19)).type_text(" fast").idle().keys("<c-z>").keys("<c-y>").expect_caret_after("fast");
    });
}

#[test]
fn parity_click_the_box() {
    parity("click a task's box").run(|f| {
        let at = task_box(f, "Draft the budget");
        f.click(at);
    });
}

#[test]
fn parity_drag_select_and_cut() {
    parity("drag-select and ⌃X").run(|f| {
        f.click_caret(doc_at("Grow the newsletter", 0)).drag(doc_at("Grow the newsletter", 5), doc_at("Grow the newsletter", 9)).expect_selection("the ").keys("<c-x>");
    });
}

#[test]
fn parity_wheel_then_type() {
    parity("wheel, then type").run(|f| {
        f.click_caret(doc_at("Grow the newsletter", 19));
        let mid = pane_middle(f);
        f.wheel(true, 2, Some(mid)).type_text("x").expect_caret_after("newsletterx");
    });
}

#[test]
fn parity_resize() {
    parity("resize").run(|f| {
        f.click_caret(doc_at("Grow the newsletter", 19)).resize(160, 40).type_text("r").expect_caret_after("newsletterr");
    });
}

#[test]
fn parity_another_writer_while_typing() {
    parity("another writer while typing").run(|f| {
        f.click_caret(doc_at("Last line of the plan", 21)).type_text(" ab").remote_edit("Grow the newsletter", "Grow the newsletter fast").type_text("c").expect_caret_after(" abc").expect_line("Grow the newsletter fast");
    });
}

#[test]
fn parity_restore_on_a_new_line() {
    // The flow's restore check (done) brings back an empty, unsaved caret line in either pane.
    parity("restore on a new line").run(|f| {
        f.click_caret(doc_at("Last line of the plan", 21)).type_text("\n");
    });
}

#[test]
fn parity_esc_clears_selection_before_leaving() {
    parity("Esc clears selection before leaving").run(|f| {
        f.click_caret(doc_at("Grow the newsletter", 0)).drag(doc_at("Grow the newsletter", 5), doc_at("Grow the newsletter", 9)).expect_selection("the ");
        f.keys("<esc>").expect_no_selection();
        f.type_text("x").keys("<esc>");
    });
}

#[test]
fn parity_meta_date_click() {
    parity("date chips use the same editor buttons").run(|f| {
        f.click_caret(doc_at("Draft the budget", 6));
        f.click(At::Target(|t| matches!(t, crate::ui::Click::Meta { field: "due", .. }), "due chip"));
        f.keys("<esc>");
    });
}

#[test]
fn parity_link_popup_mouse_insert() {
    parity("[[ popup mouse insert").run(|f| {
        f.click_caret(doc_at("Last line of the plan", 21)).type_text(" [[Gar");
        f.click(At::Target(|t| matches!(t, crate::ui::Click::LinkRow(0)), "link popup row"));
        f.expect_caret_after("[[Garden]]");
    });
}

#[test]
fn panel_popup_stays_on_screen_in_drawer_and_replace() {
    for width in [100, 80] {
        let mut f = flow_in("popup in narrow panel", Where::Panel);
        f.resize(width, 36).keys("<c-end>").type_text("[[Gar");
        let rows: Vec<_> = f.s.app.render.panel_targets.iter().flat_map(|(_, ts)| ts).filter(|t| matches!(t.what, crate::ui::Click::LinkRow(_))).collect();
        assert!(!rows.is_empty());
        assert!(rows.iter().all(|t| t.x1 <= width && t.y < 36));
        f.click(At::Target(|t| matches!(t, crate::ui::Click::LinkRow(0)), "narrow link popup row"));
        f.expect_caret_after("[[Garden]]").done();
    }
}

#[test]
fn a_focused_panels_editor_state_is_not_agent_controlled() {
    let mut f = flow_in("panel editor belongs to the person", Where::Panel);
    f.click_caret(doc_at("Grow the newsletter", 19)).type_text("!");
    let mut state = f.s.app.ui.to_json();
    state["sidebar"]["open"][0]["view"]["editor"]["doc_parked"] = serde_json::json!(true);
    let before = f.s.app.ui.clone();
    assert!(f.s.apply(Msg::SetState { state, actor: Some("test-agent".into()) }).is_err());
    assert_eq!(f.s.app.ui, before);
    f.done();
}

// ---- bug repros ------------------------------------------------------------------------------

/// Q4 Plan in main with Garden beside it.
#[test]
fn overlay_frames_and_line_anchors_come_from_the_shared_panel_body() {
    let mut f = q4_with_garden();
    f.keys("<m-s>");
    let app = &f.s.app;
    for p in &app.derived.sidebar.as_ref().unwrap().panels {
        let Some(editor) = p.editor.as_ref() else { continue };
        assert_eq!(p.frame_at, Some(editor.frame_at));
        assert_eq!(p.frame.as_ref().unwrap().cursor, editor.frame.cursor);
        let hits = &app.render.panel_hits.iter().find(|(k, _)| *k == p.key).unwrap().1;
        let doc = app.panel_doc(&p.key).unwrap();
        assert!(!p.line_rows.is_empty());
        for (y, x, _, id) in &p.line_rows {
            let hit = hits.iter().find(|h| h.y == *y).expect("anchor is on a drawn row");
            assert_eq!(*x, if hit.first { hit.hang_x } else { hit.text_x });
            assert_eq!(*id, doc.blocks()[hit.line].id);
        }
    }
    f.done();
}

fn q4_with_garden() -> Flow {
    let mut f = flow("Q4 Plan, Garden beside");
    f.keys("<c-o>Q4 Plan<cr>").shift_click(text("Garden").in_doc()).expect_panels(1);
    f
}

#[test]
fn esc_from_a_panel_with_a_stale_popup_keeps_the_main_view() {
    // The `[[` popup's flag outlives its query (`]]` closed the link): Esc skipped the
    // sidebar's chords and ran the main view's `doc.done` against main's origin, with the
    // panel's document in its place.
    let mut f = q4_with_garden();
    f.named("Esc from a panel with a stale popup");
    f.keys("<m-s>").click(doc_at_side("Compost by the fence")).type_text(" [[Gar]]").keys("<esc>").expect_page("Q4 Plan").expect_focus(Focus::List).expect_at(text("Tomatoes need staking").in_side()).done();
}

/// Garden in main with Q4 Plan (long) beside it, its caret on its last line.
fn garden_with_q4_low(screen: (u16, u16)) -> Flow {
    let mut f = flow_with("Garden, Q4 Plan beside", Size::Long, screen);
    f.keys("<c-o>Garden<cr>").shift_click(text("Q4 Plan").in_doc()).expect_panels(1);
    f.click(doc_at_side("Last line of the plan"));
    f
}

#[test]
fn an_inactive_panel_keeps_its_scroll_when_another_takes_the_keyboard() {
    // A panel without the keyboard gets fewer rows (here 5, its caret below them): its view
    // stays put as the keyboard moves between panels. The flow's invariant checks it each
    // step: a pane without the keyboard never scrolls by itself (k5wvp: not reproduced here).
    let mut f = garden_with_q4_low((140, 36));
    f.named("an inactive panel keeps its scroll");
    f.msg("Long Page beside", Motion::Any, Msg::Aside { target: "Long Page".into(), pin: false, fold: false, close: false, actor: None }).expect_panels(2);
    f.keys("<m-s>").keys("<m-j>").keys("<m-k>").keys("<m-j>");
    f.done();
}

#[test]
fn a_render_at_another_size_leaves_a_panels_scroll() {
    // `thc ui render 140x10` lays everything out at 10 rows and puts back the main view's
    // scroll; a panel's must come back too (2519m: not reproduced here).
    let mut f = garden_with_q4_low((140, 36));
    f.named("a render at another size leaves a panel's scroll");
    let id = PaneId::Panel(f.s.app.ui.sidebar.open[0].key());
    let before = f.shot.pane(&id).map(|p| (p.scroll, p.top.clone()));
    let _ = f.s.render(140, 10, "text");
    f.idle();
    let after = f.shot.pane(&id).map(|p| (p.scroll, p.top.clone()));
    f.expect(&format!("the panel's scroll as it was: {before:?}, now {after:?}"), |_| before == after).done();
}

#[test]
fn a_panel_saves_the_line_it_left_after_the_frame() {
    // In the main view, leaving a line saves it after the next frame (not at idle); a panel
    // saves the same way.
    let mut f = q4_with_garden();
    f.named("a panel saves the line it left after the frame");
    f.keys("<m-s>").click_caret(doc_at_side("Tomatoes need staking")).type_text("!").moves("<down>");
    let saved = f.s.app.vault.store.nodes_where("1=1", &[]).unwrap_or_default().iter().any(|n| n.text == "Tomatoes need staking!");
    f.expect("the left line saved after the frame", |_| saved).done();
}

/// The end of a line in the sidebar (its text, found on screen).
fn doc_at_side(line: &str) -> At {
    text(line).in_side().dx(line.chars().count() as i32)
}
