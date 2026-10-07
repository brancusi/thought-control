//! Flows: the mouse. Clicks that place the caret (on wrapped rows, on wide characters), double
//! and triple clicks, drag-select, the wheel in the editor and in lists, and clicks on tabs, list
//! rows and footer hints.
use super::typing::q4;
use super::*;

#[test]
fn click_places_the_caret() {
    q4().named("a click puts the caret there")
        .click_caret(doc_at("Grow the newsletter", 5))
        .expect_caret_before("the newsletter")
        .click_caret(doc_at("Last line of the plan", 10))
        .expect_caret_before("of the plan")
        .type_text("X")
        .expect_caret_line("Last line Xof the plan")
        .done();
}

#[test]
fn click_by_the_text_on_screen() {
    // Found by what's drawn, not by the model: the 'n' of "newsletter".
    q4().named("a click on drawn text").click_caret(text("newsletter").in_doc()).expect_caret_before("newsletter").done();
}

#[test]
fn click_on_a_wrapped_row() {
    q4().named("a click on the third row of a wrapped paragraph")
        .click_caret(text("typing inside it").in_doc())
        .expect_caret_before("typing inside it")
        .type_text("Z")
        .expect_caret_after("Z")
        .done();
}

#[test]
fn click_past_the_end_of_a_row() {
    q4().named("a click right of a line's end puts the caret at its end")
        .click_caret(text("Grow the newsletter").in_doc().dx(40))
        .expect_caret_after("newsletter")
        .done();
}

#[test]
fn click_on_wide_characters() {
    flow("a click on a wide character")
        .keys("<c-o>Lisbon flat<cr>")
        .click_caret(text("本").in_doc())
        .expect_caret_before("本語")
        .click_caret(text("本").in_doc().dx(1))
        .expect("the caret on a character boundary", |s| {
            let d = s.app.doc.as_ref().unwrap();
            d.caret_block().text.is_char_boundary(d.caret().byte)
        })
        .click_caret(text("🙂").in_doc())
        .expect_caret_before("🙂")
        .done();
}

#[test]
fn double_click_selects_a_word() {
    q4().named("a double click selects the word").double_click(text("newsletter").in_doc().dx(3)).expect_selection("newsletter").done();
}

#[test]
fn triple_click_selects_the_line() {
    q4().named("a triple click selects the note").triple_click(text("newsletter").in_doc()).expect_selection("Grow the newsletter").done();
}

#[test]
fn drag_selects() {
    q4().named("a drag selects")
        .drag(text("Grow").in_doc(), text("newsletter").in_doc())
        .expect_selection("Grow the ")
        .type_text("Keep ")
        .expect_caret_line("Keep newsletter")
        .done();
}

#[test]
fn drag_selects_across_lines() {
    q4().named("a drag across lines")
        .drag(text("newsletter").in_doc(), text("redesign").in_doc())
        .expect("a selection across lines", |s| s.app.doc.as_ref().and_then(|d| d.selection()).is_some_and(|(a, b)| a.line != b.line))
        .done();
}

#[test]
fn shift_click_extends_the_selection() {
    q4().named("⇧click extends from the caret")
        .click_caret(text("Grow").in_doc())
        .shift_click(text("newsletter").in_doc())
        .expect_selection("Grow the ")
        .done();
}

#[test]
fn wheel_scrolls_the_editor_without_moving_the_caret() {
    let mut f = flow_with("the wheel in the editor", Size::Long, (140, 36));
    f.keys("<c-o>Long Page<cr><c-home>");
    let caret = f.s.app.doc.as_ref().unwrap().caret();
    f.wheel(true, 5, Some(text("Line 2").in_doc()))
        .expect("the view scrolled", |s| s.app.doc.as_ref().unwrap().scroll() > 0)
        .expect("the caret stayed", |s| s.app.doc.as_ref().unwrap().caret() == caret)
        .wheel(false, 5, Some(text("Line").in_doc()))
        .expect("back at the top", |s| s.app.doc.as_ref().unwrap().scroll() == 0)
        .done();
}

#[test]
fn wheel_then_type_brings_the_caret_back() {
    flow_with("the wheel, then a key", Size::Long, (140, 36))
        .keys("<c-o>Long Page<cr><c-home>")
        .wheel(true, 10, None)
        .type_text("x")
        .expect("the caret is on screen again", |s| s.app.doc.as_ref().unwrap().scroll() == 0)
        .done();
}

#[test]
fn wheel_scrolls_a_list() {
    flow_with("the wheel in a list", Size::Long, (140, 24)).known("cjn86", Known::Restore).keys("3").expect_view(View::Tasks).wheel(true, 3, None).wheel(false, 3, None).done();
}

#[test]
fn click_the_view_tabs() {
    let mut f = flow("clicks on the tabs");
    for v in [View::Tasks, View::Pages, View::Journal, View::Inbox, View::Today] {
        f.click(tab(v)).expect_view(v);
    }
    f.done();
}

#[test]
fn click_a_list_row() {
    flow("a click selects a row, a double click opens it")
        .keys("3")
        .click(text("book flights"))
        .expect("the row selected", |s| s.app.selected_node().is_some_and(|n| n.text.contains("book flights")))
        .double_click(text("book flights"))
        .done();
}

#[test]
fn click_a_task_box_in_a_list() {
    flow("a click on a row's box completes it").known("cjn86", Known::Restore)
        .keys("3")
        .click(text("[ ] book flights").dx(1))
        .expect_saved_status("book flights", "done")
        .done();
}

#[test]
fn click_a_task_box_in_the_editor() {
    q4().named("a click on a task's box in the editor").known("64j4y", Known::Rail).click(text("[ ] Draft").in_doc().dx(1)).expect_saved_status("Draft the budget", "done").done();
}

#[test]
fn click_a_footer_hint() {
    q4().named("a click on the footer's F1 keys").click(text("F1 keys").in_footer()).expect("help open", |s| s.app.ui.overlay.is_some()).keys("<esc>").done();
}

#[test]
fn click_a_day_in_the_strip() {
    // Yesterday in the strip, as it's drawn: `WED 07`.
    let y = thc_core::dates::today().pred_opt().unwrap();
    let label = y.format("%a %d").to_string().to_uppercase();
    flow("a click on a day in the journal's strip").keys("T").click(text(&label).in_main()).expect_day(-1).done();
}

#[test]
fn hover_never_moves_anything() {
    let mut f = q4();
    f.named("hover only colours");
    for at in [text("Tasks"), text("Goals").in_doc(), text("F1 keys").in_footer()] {
        let (x, y) = f.locate(&at);
        f.msg("hover", Motion::Caret, Msg::Mouse { mouse: crate::session::Mouse { kind: crate::session::MouseKind::Moved, x, y, mods: String::new(), clicks: None } });
    }
    f.done();
}

#[test]
#[ignore = "emtsr"]
fn the_first_drag_hint_replays() {
    // The hint shows once per device (a flag file in the cache): the replay must draw it too.
    let mut f = q4();
    f.named("the first drag's hint, replayed");
    let _ = std::fs::remove_file(f.s.app.vault.paths.cache.join("mouse-hint-shown"));
    f.drag(text("Grow").in_doc(), text("newsletter").in_doc()).expect_screen("selected in thc").done();
}

#[test]
fn click_below_the_last_line() {
    q4().named("a click below the page's last line")
        .click_caret(text("Last line of the plan").in_doc().dx(5))
        .click(text("linked from").in_doc().dx(-20))
        .expect_page("Q4 Plan")
        .done();
}

#[test]
#[ignore = "zszv1"]
fn wheel_over_a_panel_scrolls_the_panel() {
    let mut f = flow_with("the wheel over a panel", Size::Long, (140, 36));
    f.keys("<c-o>Garden<cr>");
    let id = f.s.app.vault.store.nodes_where("title = 'Long Page'", &[]).unwrap()[0].id.clone();
    f.msg("aside", Motion::Any, Msg::Aside { target: id, pin: false, fold: false, close: false, actor: None }).expect_panels(1);
    let main_scroll = f.s.app.doc.as_ref().unwrap().scroll();
    f.wheel(true, 5, Some(text("Line 2").in_side()))
        .expect("the main view didn't scroll", |s| s.app.doc.as_ref().unwrap().scroll() == main_scroll)
        .expect_no_screen("Line 0:")
        .done();
}

#[test]
fn clicks_on_many_places_in_a_wrapped_paragraph() {
    // Every cell of the paragraph's rows puts the cursor where the caret is.
    let mut f = q4();
    f.named("a click on each word of a wrapped paragraph");
    for w in ["paragraph", "several", "typing", "exactly", "flow", "beyond"] {
        f.click_caret(text(w).in_doc().dx(2)).expect_caret_before(&w[2..]);
    }
    f.done();
}
