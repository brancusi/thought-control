//! Flows: the sidebar (Logseq's right panel). ⇧-click or ⌘-click a link to open it beside,
//! ⌥O, editing in a panel and seeing the main view follow, moving focus (⌥S, Esc), closing,
//! pinning and folding, narrow screens (drawer, replace), and typing beside an open panel.
use super::typing::{WORDS, q4};
use super::*;

/// The sidebar's text, row by row.
fn side_text(f: &Flow) -> Vec<String> {
    let Some(x) = f.shot.side_x else { return vec![] };
    (0..f.shot.buf.area.height).map(|y| f.shot.row(y, x, f.shot.buf.area.width)).collect()
}

/// Q4 Plan with Garden beside it.
fn with_garden_beside() -> Flow {
    let mut f = q4();
    f.timed("sidebar_open", |f| {
        f.shift_click(text("Garden").in_doc());
    });
    f.expect_panels(1);
    f
}

#[test]
fn shift_click_a_link_opens_it_beside() {
    with_garden_beside()
        .named("⇧click a link opens it beside")
        .expect_page("Q4 Plan")
        .expect_at(text("Tomatoes need staking").in_side())
        .expect_focus(Focus::List)
        .done();
}

#[test]
fn cmd_click_a_link_opens_it_beside() {
    q4().named("⌘click a link opens it beside (WezTerm, Ghostty)")
        .cmd_click(text("Garden").in_doc())
        .expect_page("Q4 Plan")
        .expect_panels(1)
        .expect_at(text("Tomatoes need staking").in_side())
        .done();
}

#[test]
fn shift_click_a_link_in_the_journal() {
    flow("⇧click a link in today's journal")
        .keys("T")
        .shift_click(text("Q4 Plan").in_doc())
        .expect_day(0)
        .expect_panels(1)
        .expect_at(text("Goals for the quarter").in_side())
        .done();
}

#[test]
fn alt_o_opens_the_link_under_the_caret_beside() {
    q4().named("⌥O on a link opens it beside")
        .alt_click(doc_at("Ship the [[Garden]]", 12))
        .keys("<m-o>")
        .expect_panels(1)
        .expect_page("Q4 Plan")
        .expect_at(text("Tomatoes need staking").in_side())
        .done();
}

#[test]
fn shift_click_a_second_link_stacks_panels() {
    with_garden_beside()
        .named("two links beside: two panels")
        .shift_click(text("Lisbon flat").in_doc())
        .expect_panels(2)
        .expect_at(text("Two bedrooms").in_side())
        .expect_at(text("Tomatoes").in_side())
        .done();
}

#[test]
fn shift_click_the_same_link_twice_keeps_one_panel() {
    with_garden_beside().named("the same link twice: one panel").shift_click(text("Garden").in_doc()).expect_panels(1).done();
}

#[test]
fn alt_s_moves_focus_to_the_panel_and_back() {
    with_garden_beside()
        .named("⌥S to the panel and back")
        .keys("<m-s>")
        .expect_focus(Focus::Sidebar)
        .keys("<m-s>")
        .expect_focus(Focus::List)
        .keys("<m-s>")
        .expect_focus(Focus::Sidebar)
        .keys("<esc>")
        .expect_focus(Focus::List)
        .expect_page("Q4 Plan")
        .done();
}

#[test]
fn type_in_a_panel() {
    with_garden_beside()
        .named("type in a panel")
        .keys("<m-s>")
        .type_text("Note: ")
        .expect_at(text("Note: Tomatoes").in_side())
        .expect_saved("Note: Tomatoes need staking")
        .done();
}

#[test]
fn a_click_in_a_panel_places_its_caret_and_focuses_it() {
    with_garden_beside()
        .named("a click in a panel")
        .click(text("fence").in_side())
        .expect_focus(Focus::Sidebar)
        .type_text("!")
        .expect_at(text("Compost by the !fence").in_side())
        .done();
}

#[test]
fn editing_in_a_panel_shows_in_the_main_view() {
    // The same page in both: the main view follows the panel's typing, keystroke by keystroke.
    let mut f = with_garden_beside();
    f.named("the panel and the main view show one document");
    f.keys("<c-o>Garden<cr>").expect_page("Garden").expect_panels(1);
    f.keys("<m-s>").type_text("Both ").expect_at(text("Both Tomatoes").in_side()).expect_at(text("Both Tomatoes").in_main());
    f.done();
}

#[test]
fn back_from_a_panel_on_the_same_page_shows_the_cursor() {
    let mut f = with_garden_beside();
    f.named("⌥S back from a panel on the same page");
    f.keys("<c-o>Garden<cr>").keys("<m-s>").type_text("Both ").keys("<m-s>").expect_focus(Focus::List);
    // And back in the main view, typing there shows in the panel.
    f.click(text("Compost").in_main()).keys("<c-home>").type_text("Main ").expect_at(text("Main Both Tomatoes").in_side()).done();
}

#[test]
fn close_a_panel() {
    with_garden_beside().named("⌥W closes a panel").keys("<m-s><m-w>").expect_panels(0).expect_focus(Focus::List).expect_page("Q4 Plan").done();
}

#[test]
fn close_a_panel_with_its_x() {
    with_garden_beside().named("a click on × closes a panel").click(text("×").in_side()).expect_panels(0).done();
}

#[test]
fn reopen_a_closed_panel() {
    with_garden_beside().named("⌥⇧T reopens").keys("<m-s><m-w>").expect_panels(0).keys("<m-T>").expect_panels(1).done();
}

#[test]
fn pin_a_panel() {
    with_garden_beside()
        .named("⌥P pins")
        .keys("<m-s><m-p>")
        .expect("pinned", |s| s.app.ui.sidebar.open.first().is_some_and(|p| p.pinned))
        .keys("<m-p>")
        .expect("unpinned", |s| s.app.ui.sidebar.open.first().is_some_and(|p| !p.pinned))
        .done();
}

#[test]
fn fold_a_panel() {
    with_garden_beside()
        .named("⌥C folds")
        .keys("<m-s><m-c>")
        .expect("folded", |s| s.app.ui.sidebar.open.first().is_some_and(|p| p.folded))
        .expect_no_screen("Tomatoes need staking")
        .keys("<m-c>")
        .expect_at(text("Tomatoes need staking").in_side())
        .done();
}

#[test]
fn panel_to_main() {
    with_garden_beside().named("⌥M sends a panel to the main view").known("q93zh", Known::Rail).keys("<m-s><m-m>").expect_page("Garden").expect_focus(Focus::List).done();
}

#[test]
fn hide_and_show_the_sidebar() {
    with_garden_beside()
        .named("⌥\\ hides and shows")
        .keys("<m-\\>")
        .expect_no_screen("Tomatoes need staking")
        .keys("<m-\\>")
        .expect_at(text("Tomatoes need staking").in_side())
        .done();
}

#[test]
fn a_narrow_screen_opens_a_drawer() {
    flow("⇧click on a 100-column screen: the drawer")
        .resize(100, 30)
        .keys("<c-o>Q4 Plan<cr>")
        .shift_click(text("Garden").in_doc())
        .expect_panels(1)
        .expect_screen("Tomatoes need staking")
        .keys("<esc>")
        .done();
}

#[test]
fn a_very_narrow_screen_replaces_the_view() {
    flow("⇧click on a 70-column screen: replace")
        .resize(70, 30)
        .keys("<c-o>Q4 Plan<cr>")
        .shift_click(text("Garden").in_doc())
        .expect_panels(1)
        .expect_screen("Tomatoes need staking")
        .keys("<esc>")
        .expect_screen("Goals for the quarter")
        .done();
}

#[test]
fn resize_with_a_panel_open() {
    with_garden_beside().named("resizing with a panel open").resize(100, 30).resize(70, 24).resize(160, 40).expect_at(text("Tomatoes need staking").in_side()).done();
}

#[test]
fn typing_beside_a_panel_never_touches_it() {
    let mut f = with_garden_beside();
    f.named("typing in the main view leaves the panel alone");
    f.click_caret(doc_at("Last line of the plan", 21));
    let side = side_text(&f);
    f.type_text(" ").type_text(WORDS);
    if side_text(&f) != side {
        panic!("the panel changed while typing beside it:\nbefore {side:#?}\nafter {:#?}", side_text(&f));
    }
    f.done();
}

#[test]
fn an_agent_opens_a_panel_while_you_type() {
    let mut f = q4();
    f.named("an agent puts a page beside you mid-sentence").click_caret(doc_at("Last line of the plan", 21)).type_text(" half");
    let id = f.s.app.vault.store.nodes_where("title = 'Garden'", &[]).unwrap()[0].id.clone();
    f.msg("agent aside", Motion::Any, Msg::Aside { target: id, pin: false, fold: false, close: false, actor: Some("claude".into()) })
        .expect_panels(1)
        .expect_focus(Focus::List)
        .type_text(" done")
        .expect_caret_line("Last line of the plan half done")
        .done();
}

#[test]
fn shift_click_a_link_inside_a_panel() {
    with_garden_beside()
        .named("⇧click a link inside a panel stacks another")
        .shift_click(text("Q4 Plan").in_side())
        .expect_panels(2)
        .expect_page("Q4 Plan")
        .done();
}

#[test]
fn a_click_on_a_link_in_a_panel_follows_it_in_the_panel_or_main() {
    with_garden_beside()
        .named("a click on a link in a panel")
        .click(text("Q4 Plan").in_side())
        .expect("a page shows", |s| s.app.doc.is_some())
        .done();
}

#[test]
fn type_in_a_panel_then_back_to_main_keeps_both_carets() {
    with_garden_beside()
        .named("both carets kept across ⌥S")
        .click_caret(doc_at("Grow the newsletter", 5))
        .click(text("fence").in_side())
        .type_text("X")
        .keys("<m-s>")
        .expect_focus(Focus::List)
        .expect_caret_before("the newsletter")
        .type_text("Y")
        .expect_caret_line("Grow Ythe newsletter")
        .keys("<m-s>")
        .type_text("Z")
        .expect_at(text("Compost by the XZfence").in_side())
        .done();
}
