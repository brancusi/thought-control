//! Flows: selecting and editing. Word motion, ⇧ selection, select-all, cut / copy / paste,
//! deleting across lines, moving lines with ⌥↑ ⌥↓, and ⌃T's task cycle.
use super::typing::q4;
use super::*;

#[test]
fn word_motion() {
    q4().named("⌥← ⌥→ move by word")
        .click_caret(doc_at("Grow the newsletter", 0))
        .moves("<m-right>")
        .expect_caret_after("Grow")
        .moves("<m-right>")
        .expect_caret_after("Grow the")
        .moves("<m-left>")
        .expect_caret_after("Grow ")
        .moves("<c-right><c-right>")
        .expect_caret_after("newsletter")
        .done();
}

#[test]
fn home_and_end() {
    q4().named("Home and End")
        .click_caret(doc_at("Grow the newsletter", 5))
        .moves("<end>")
        .expect_caret_after("newsletter")
        .moves("<home>")
        .expect_caret_before("Grow")
        .done();
}

#[test]
fn shift_arrows_select() {
    q4().named("⇧→ and ⇧⌥→ select")
        .click_caret(doc_at("Grow the newsletter", 0))
        .moves("<s-right><s-right><s-right><s-right>")
        .expect_selection("Grow")
        .moves("<s-m-right>")
        .expect_selection("Grow the")
        .moves("<s-left><s-left><s-left><s-left>")
        .expect_selection("Grow")
        .done();
}

#[test]
fn shift_down_selects_across_lines() {
    q4().named("⇧↓ selects into the next line")
        .click_caret(doc_at("Grow the newsletter", 5))
        .moves("<s-down>")
        .expect("a selection across two lines", |s| s.app.doc.as_ref().and_then(|d| d.selection()).is_some_and(|(a, b)| a.line != b.line))
        .done();
}

#[test]
fn select_all_then_type_replaces_everything() {
    q4().named("select all, then type")
        .keys("<m-a>")
        .expect("a selection", |s| s.app.doc.as_ref().and_then(|d| d.selection()).is_some())
        .type_text("replaced")
        .expect("one line left", |s| s.app.doc.as_ref().is_some_and(|d| d.blocks().iter().filter(|l| !l.text.is_empty()).count() == 1))
        .keys("<c-z><c-z>")
        .expect_line("Goals for the quarter")
        .done();
}

#[test]
fn cut_and_paste_a_word() {
    q4().named("⌃X a word, ⌃V it elsewhere")
        .click_caret(doc_at("Grow the newsletter", 0))
        .moves("<s-m-right>")
        .expect_selection("Grow")
        .keys("<c-x>")
        .expect_caret_line(" the newsletter")
        .click_caret(doc_at("Last line of the plan", 21))
        .paste_clipboard()
        .expect_caret_after("planGrow")
        .done();
}

#[test]
fn copy_and_paste_a_line() {
    q4().named("⌃C a whole line, ⌃V it below")
        .click_caret(doc_at("Grow the newsletter", 0))
        .moves("<s-end>")
        .keys("<c-c>")
        .expect_caret_line("Grow the newsletter")
        .click_caret(doc_at("Last line of the plan", 21))
        .keys("<cr>")
        .paste_clipboard()
        .expect_caret_line("Grow the newsletter")
        .done();
}

#[test]
fn delete_across_lines() {
    q4().named("select across lines, Backspace joins them")
        .click_caret(doc_at("Grow the newsletter", 5))
        .moves("<s-down><s-down>")
        .keys("<bs>")
        .expect_no_line("Ship the")
        .expect_caret_after("Grow ")
        .keys("<c-z>")
        .expect_line("Grow the newsletter")
        .done();
}

#[test]
fn delete_word_and_kill_line() {
    q4().named("⌥⌫ deletes a word, ⌃K the rest")
        .click_caret(doc_at("Grow the newsletter", 8))
        .keys_as(Motion::Typing, "<m-bs>")
        .expect_caret_line("Grow  newsletter")
        .keys_as(Motion::Typing, "<c-k>")
        .expect_caret_line("Grow ")
        .done();
}

#[test]
fn move_lines_up_and_down() {
    q4().named("⌥↑ ⌥↓ move a line")
        .click_caret(doc_at("Last line of the plan", 3))
        .keys("<m-up>")
        .expect_caret_line("Last line of the plan")
        .expect("the line moved above the paragraph", |s| {
            let d = s.app.doc.as_ref().unwrap();
            let i = d.blocks().iter().position(|l| l.text == "Last line of the plan").unwrap();
            d.blocks()[i + 1].text.starts_with("This paragraph")
        })
        .keys("<m-down>")
        .expect("and back", |s| {
            let d = s.app.doc.as_ref().unwrap();
            let i = d.blocks().iter().position(|l| l.text == "Last line of the plan").unwrap();
            d.blocks()[i - 1].text.starts_with("This paragraph")
        })
        .expect_caret_before("t line")
        .done();
}

#[test]
fn ctrl_t_cycles_a_task() {
    q4().named("⌃T: note → task → done → note")
        .click_caret(doc_at("Last line of the plan", 3))
        .keys("<c-t>")
        .expect("a task", |s| s.app.doc.as_ref().is_some_and(|d| d.caret_block().kind() == thc_core::outline::Kind::Task))
        .keys("<c-t>")
        .expect("done", |s| s.app.doc.as_ref().is_some_and(|d| d.caret_block().status.as_deref() == Some("done")))
        .keys("<c-t>")
        .expect("not a task", |s| s.app.doc.as_ref().is_some_and(|d| d.caret_block().kind() != thc_core::outline::Kind::Task))
        .expect_caret_before("t line")
        .done();
}

#[test]
fn ctrl_t_keeps_the_caret_and_the_rows() {
    // ⌃T adds a box in the hang: nothing else on screen moves.
    q4().named("⌃T doesn't shift the rows around it")
        .click_caret(doc_at("Grow the newsletter", 5))
        .keys_as(Motion::Typing, "<c-t>")
        .keys_as(Motion::Typing, "<c-t>")
        .done();
}

#[test]
fn undo_redo_selection_edits() {
    q4().named("undo and redo a cut and a paste")
        .click_caret(doc_at("Grow the newsletter", 0))
        .moves("<s-end>")
        .keys("<c-x>")
        .expect_caret_line("")
        .keys("<c-z>")
        .expect_line("Grow the newsletter")
        .keys("<c-y>")
        .expect_no_line("Grow the newsletter")
        .done();
}
