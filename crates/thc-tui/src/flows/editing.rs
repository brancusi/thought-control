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
fn cmd_a_selects_the_whole_page_from_a_note() {
    for key in ["<d-a>", "<m-a>"] {
        let mut f = q4();
        f.named("select all from the middle of a page");
        f.click_caret(doc_at("Grow the newsletter", 5))
            .keys(key)
            .expect("exact page selection", |s| {
                let d = s.app.doc.as_ref().unwrap();
                let end = d.blocks().len() - 1;
                d.selection() == Some((crate::editor::BlockPos { line: 0, byte: 0 }, crate::editor::BlockPos { line: end, byte: d.blocks()[end].text.len() }))
            });
        f.done();
    }
}

#[test]
fn cmd_a_is_scoped_to_the_focused_page_panel() {
    let mut f = panes::flow_in("Cmd+A in a page panel", panes::Where::Panel);
    f.click_caret(doc_at("Grow the newsletter", 0).in_pane())
        .moves("<s-right><s-right><s-right><s-right>")
        .expect_selection("Grow")
        .keys("<d-a>");
    let d = f.pane_doc().unwrap();
    let end = d.blocks().len() - 1;
    assert_eq!(d.selection(), Some((crate::editor::BlockPos { line: 0, byte: 0 }, crate::editor::BlockPos { line: end, byte: d.blocks()[end].text.len() })));
    assert_eq!(f.s.app.doc.as_ref().unwrap().selection(), None);
    f.keys("<c-c>");
    let clip = crate::runtime_effects::SNAPSHOT_CLIPBOARD.with(|c| c.borrow().clone()).unwrap();
    assert!(clip.contains("Goals for the quarter") && clip.contains("Last line of the plan"));
    assert!(!clip.contains("Morning notes"));
    f.done();
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

#[test]
fn ctrl_t_from_done_to_text_clears_the_done_time() {
    // ⌃T cycles [ ] → [x] → text: back to text, the row says nothing of a done time, while
    // the caret is still on it and after the save, live and restored.
    let mut f = q4();
    f.named("⌃T from done back to text");
    f.click_caret(doc_at("Draft the budget", 5)).keys("<c-t>").expect_saved_status("Draft the budget", "done");
    f.keys("<down><up>").expect_screen("done ");
    f.keys("<c-t>").expect("no done time on the row", |s| {
        let (w, h) = s.size;
        let frame = s.render(w, h, "text").unwrap().frame.unwrap();
        frame.lines().filter(|l| l.contains("Draft the budget")).all(|l| !l.contains("done "))
    });
    f.keys("<down>").expect("still none after the save", |s| {
        let (w, h) = s.size;
        let frame = s.render(w, h, "text").unwrap().frame.unwrap();
        frame.lines().filter(|l| l.contains("Draft the budget")).all(|l| !l.contains("done "))
    });
    f.done();
}
