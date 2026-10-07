//! Flows: getting around. Jumping to a page (⌃O, `:`), following links (Enter, ⌥Enter, a
//! click), back and forward, today's journal, and the days either side of it.
use super::*;

#[test]
fn jump_to_a_page_with_ctrl_o_and_type() {
    flow("jump to a page with ⌃O and type")
        .timed("page_open", |f| {
            f.keys("<c-o>Q4 Plan<cr>");
        })
        .expect_page("Q4 Plan")
        .expect_screen("Goals for the quarter")
        .keys("<c-end>")
        .type_text(" and more")
        .expect_caret_after("and more")
        .expect_saved_contains("and more")
        .done();
}

#[test]
fn jump_to_a_page_from_inside_a_document() {
    // ⌃O over a line that isn't a link opens the finder; from there to another page.
    flow("⌃O from a journal day to a page")
        .keys("T")
        .expect_day(0)
        .keys("<c-home><c-o>")
        .expect_screen("open")
        .type_text("Garden")
        .keys("<cr>")
        .expect_page("Garden")
        .done();
}

#[test]
fn jump_to_a_page_with_the_palette() {
    flow("jump to a page with :")
        .keys(":")
        .type_text("Q4 Plan")
        .keys("<cr>")
        .expect_page("Q4 Plan")
        .done();
}

#[test]
fn jump_to_a_day_with_the_palette() {
    flow("jump to yesterday with :")
        .keys(":")
        .type_text("yesterday")
        .keys("<cr>")
        .expect_day(-1)
        .done();
}

#[test]
fn finder_arrows_choose_a_page() {
    flow("⌃O, ↓ to a page, Enter")
        .keys("<c-o>")
        .expect_screen("¶ Garden")
        .keys("<down><down><cr>")
        .expect("a page open", |s| s.app.doc.is_some())
        .done();
}

#[test]
fn open_a_link_with_alt_enter() {
    flow("⌥Enter on a link opens it")
        .known("q93zh", Known::Rail)
        .keys("<c-o>Garden<cr>")
        .expect_page("Garden")
        .alt_click(doc_at("See [[Q4 Plan]]", 8))
        .expect_page("Garden")
        .keys("<m-cr>")
        .expect_page("Q4 Plan")
        .done();
}

#[test]
fn open_a_link_with_ctrl_o() {
    flow("⌃O on a link opens it")
        .known("q93zh", Known::Rail)
        .keys("<c-o>Garden<cr>")
        .alt_click(doc_at("See [[Q4 Plan]]", 8))
        .keys("<c-o>")
        .expect_page("Q4 Plan")
        .done();
}

#[test]
fn open_a_link_with_a_click() {
    flow("a click on a link's title follows it")
        .known("q93zh", Known::Rail)
        .keys("<c-o>Garden<cr>")
        .expect_page("Garden")
        .click(text("Q4 Plan").in_doc())
        .expect_page("Q4 Plan")
        .done();
}

#[test]
fn open_a_page_from_the_pages_list_with_enter() {
    flow("Pages, j, Enter opens the page")
        .keys("4")
        .expect_view(View::Pages)
        .keys("<down><down><cr>")
        .expect("a page open", |s| s.app.doc.is_some())
        .done();
}

#[test]
fn back_and_forward_with_cmd_brackets() {
    flow("⌘[ and ⌘] walk the history")
        .known("q93zh", Known::Rail)
        .keys("<c-o>Q4 Plan<cr>")
        .expect_page("Q4 Plan")
        .keys("<c-o>Garden<cr>")
        .expect_page("Garden")
        .keys("<d-[>")
        .expect_page("Q4 Plan")
        .keys("<d-]>")
        .expect_page("Garden")
        .keys("<c-m-left>")
        .expect_page("Q4 Plan")
        .keys("<c-m-right>")
        .expect_page("Garden")
        .done();
}

#[test]
fn back_keeps_the_caret_where_it_was() {
    flow("⌘[ comes back to the same caret")
        .keys("<c-o>Q4 Plan<cr>")
        .click_caret(doc_at("Grow the newsletter", 5))
        .expect_caret_before("the newsletter")
        .keys("<c-o>Garden<cr>")
        .keys("<d-[>")
        .expect_page("Q4 Plan")
        .expect_caret_before("the newsletter")
        .done();
}

#[test]
fn open_todays_journal() {
    flow("T opens today's journal").keys("T").expect_day(0).expect_screen("Morning notes").done();
}

#[test]
fn open_todays_journal_from_a_page() {
    flow("⌥T from a page opens today").keys("<c-o>Garden<cr>").keys("<m-t>").expect_day(0).done();
}

#[test]
fn switch_days_with_ctrl_p_and_ctrl_n() {
    flow("⌃P and ⌃N switch days")
        .keys("T")
        .expect_day(0)
        .keys("<c-p>")
        .expect_day(-1)
        .expect_screen("Yesterday's entry")
        .keys("<c-n>")
        .expect_day(0)
        .keys("<c-n>")
        .expect_day(1)
        .keys("<c-p><c-p>")
        .expect_day(-1)
        .done();
}

#[test]
fn type_on_another_day_and_come_back() {
    flow("type yesterday, come back, it's there")
        .keys("T<c-p><c-end>")
        .type_text("\nlate thought")
        .keys("<c-n><c-p>")
        .expect_day(-1)
        .expect_line("late thought")
        .expect_saved("late thought")
        .done();
}

#[test]
fn escape_from_a_page_goes_back_to_the_list() {
    flow("Esc from a page returns to the list it came from")
        .keys("4<down><cr>")
        .expect("a page open", |s| s.app.doc.is_some())
        .keys("<esc>")
        .expect_view(View::Pages)
        .expect_no_doc()
        .done();
}

#[test]
fn view_tabs_by_number_and_tab_key() {
    let mut f = flow("1–7 and Tab walk the views");
    for (k, v) in [("1", View::Today), ("2", View::Inbox), ("3", View::Tasks), ("4", View::Pages), ("6", View::Search), ("7", View::Log)] {
        f.keys("<esc>").keys(k).expect_view(v);
    }
    f.keys("<esc>1<tab>").expect_view(View::Inbox).keys("<s-tab>").expect_view(View::Today);
    f.done();
}

#[test]
fn a_new_page_from_the_finder() {
    flow("⌃O a name that isn't a page, Enter makes it")
        .keys("<c-o>")
        .type_text("Fresh Page")
        .keys("<cr>")
        .expect_page("Fresh Page")
        .type_text("first words")
        .expect_caret_after("first words")
        .expect_saved("first words")
        .done();
}

#[test]
#[ignore = "rdfar"]
fn jump_to_a_page_and_type_right_away() {
    // The owner's first wish: jump, type, and the words land in a note of their own.
    flow("jump to a page and type at once").keys("<c-o>Q4 Plan<cr>").type_text("hello").expect_line("Goals for the quarter").expect_line("hello").done();
}
