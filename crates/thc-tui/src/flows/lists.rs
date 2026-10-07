//! Flows: lists. Today and Tasks: moving, `x` done, `o` open beside, filters, opening a row.
use super::*;

#[test]
fn today_navigation() {
    flow("j and k on Today")
        .expect_view(View::Today)
        .keys("j")
        .expect("the cursor moved", |s| s.app.ui.cursor > 0)
        .keys("k")
        .keys("G")
        .keys("gg")
        .done();
}

#[test]
fn today_x_completes_a_task() {
    flow("x on Today completes the row")
        .click(text("write the plan"))
        .keys("x")
        .expect_saved_status("write the plan", "done")
        .keys("u")
        .expect_saved_status("write the plan", "todo")
        .done();
}

#[test]
#[ignore = "cjn86"]
fn tasks_navigation_and_done() {
    flow("Tasks: j, x, X")
        .keys("3")
        .expect_view(View::Tasks)
        .keys("jj")
        .keys("x")
        .keys("X")
        .done();
}

#[test]
fn tasks_filter() {
    flow("Tasks: f filters").known("cjn86", Known::Restore)
        .keys("3f")
        .type_text(" #work")
        .keys("<cr>")
        .expect_screen("write the plan")
        .expect_no_screen("book flights")
        .done();
}

#[test]
fn tasks_open_a_row() {
    flow("Tasks: Enter opens a task where it lives")
        .keys("3")
        .click(text("Draft the budget"))
        .keys("<cr>")
        .expect_page("Q4 Plan")
        .expect_caret_after("")
        .done();
}

#[test]
fn tasks_open_beside() {
    flow("Tasks: o opens a row beside").known("cjn86", Known::Restore)
        .keys("3")
        .click(text("Draft the budget"))
        .timed("sidebar_open", |f| {
            f.keys("o");
        })
        .expect_panels(1)
        .expect_view(View::Tasks)
        .expect_at(text("Goals for the quarter").in_side())
        .done();
}

#[test]
fn today_open_beside_with_alt_o() {
    flow("Today: ⌥O opens a row beside")
        .click(text("Draft the budget"))
        .keys("<m-o>")
        .expect_panels(1)
        .expect_view(View::Today)
        .done();
}

#[test]
fn search_and_open() {
    flow("Search, type, Enter opens the hit")
        .keys("6")
        .type_text("tomatoes")
        .keys("<cr>")
        .keys("<down><cr>")
        .expect_page("Garden")
        .done();
}

#[test]
fn inbox_then_back() {
    flow("Inbox, then q back").keys("2").expect_view(View::Inbox).expect_screen("book flights").keys("q").done();
}

#[test]
fn help_and_palette_open_and_close() {
    flow("F1 and : open and close")
        .keys("?")
        .expect("help", |s| s.app.ui.overlay.is_some())
        .keys("<esc>:")
        .expect("the palette", |s| s.app.ui.overlay.is_some())
        .keys("<esc>")
        .expect("nothing open", |s| s.app.ui.overlay.is_none())
        .done();
}
