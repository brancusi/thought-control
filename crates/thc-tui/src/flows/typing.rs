//! Flows: typing. Long paragraphs that wrap, Enter and new notes, Tab nesting, typing at a
//! page's start, middle and end, typing fast, wide characters, a pasted outline, and undo and
//! redo across all of it. The invariants (mod.rs) watch every keystroke: nothing above the
//! caret moves, the view doesn't scroll, the cursor sits on the caret.
use super::*;

pub const WORDS: &str = "the quick brown fox jumps over the lazy dog while the words keep flowing ";

/// A flow on the page Q4 Plan.
pub fn q4() -> Flow {
    let mut f = flow("on Q4 Plan");
    f.keys("<c-o>Q4 Plan<cr>");
    f
}

#[test]
fn type_a_long_paragraph_that_wraps() {
    q4().named("type a long paragraph that wraps")
        .click_caret(doc_at("Last line of the plan", 21))
        .type_text(" ")
        .type_text(&WORDS.repeat(3))
        .expect_caret_after("keep flowing ")
        .expect_saved_contains("Last line of the plan the quick brown fox")
        .done();
}

#[test]
fn type_inside_a_wrapped_paragraph() {
    // In the middle of the long paragraph: rows above stay; the words after reflow.
    q4().named("type inside a wrapped paragraph")
        .click_caret(doc_at("This paragraph is long", 120))
        .type_text("INSERTED WORDS HERE ")
        .expect_caret_after("INSERTED WORDS HERE ")
        .type_text(WORDS)
        .done();
}

#[test]
fn backspace_inside_a_wrapped_paragraph() {
    let mut f = q4();
    f.named("backspace inside a wrapped paragraph").click_caret(doc_at("This paragraph is long", 150));
    for _ in 0..25 {
        f.keys_as(Motion::Typing, "<bs>");
    }
    f.done();
}

#[test]
fn enter_makes_a_new_note() {
    q4().named("Enter: a new note")
        .click_caret(doc_at("Last line of the plan", 21))
        .keys("<cr>")
        .type_text("a new note below")
        .expect_caret_line("a new note below")
        .expect_line("Last line of the plan")
        .expect_saved("a new note below")
        .done();
}

#[test]
fn enter_splits_a_line() {
    q4().named("Enter in the middle of a line splits it").known("0d61e", Known::Restore)
        .click_caret(doc_at("Last line of the plan", 9))
        .keys("<cr>")
        .expect_caret_before(" of the plan")
        .expect_line("Last line")
        .done();
}

#[test]
fn tab_and_shift_tab_nest() {
    q4().named("Tab and ⇧Tab nest a line")
        .click_caret(doc_at("Last line of the plan", 21))
        .keys("<cr>")
        .type_text("child line")
        .keys("<tab>")
        .expect_depth("child line", 1)
        .type_text(" deeper")
        .keys("<s-tab>")
        .expect_depth("child line deeper", 0)
        .keys("<tab>")
        .expect_depth("child line deeper", 1)
        .expect_caret_after("deeper")
        .expect_saved("child line deeper")
        .done();
}

#[test]
fn type_at_the_start_of_a_page() {
    q4().named("type at a page's start")
        .keys("<c-home>")
        .type_text("Top: ")
        .expect_caret_after("Top: ")
        .expect_saved_contains("Top: Goals for the quarter")
        .done();
}

#[test]
fn type_at_the_end_of_a_page() {
    // ⌃End is the fresh line a page arrives with (doc_app::arrive): a paragraph, so Enter twice
    // makes the next note.
    q4().named("type at a page's end")
        .keys("<c-end>")
        .type_text("(end)")
        .keys("<cr><cr>")
        .type_text("after the end")
        .expect_saved("(end)")
        .expect_saved("after the end")
        .done();
}

#[test]
fn type_in_the_middle_of_a_page() {
    q4().named("type mid-page")
        .click_caret(doc_at("Grow the newsletter", 4))
        .type_text(" faster")
        .expect_caret_line("Grow faster the newsletter")
        .done();
}

#[test]
fn typing_fast() {
    // No logical time between keys: a burst, as a fast typist or a key repeat.
    q4().named("typing fast").pace(0).keys("<c-end><cr>").type_text(&WORDS.repeat(2)).expect_caret_after("keep flowing ").done();
}

#[test]
fn typing_with_pauses_makes_undo_steps() {
    q4().named("typing, a pause, typing: undo takes back the second burst")
        .keys("<c-end><cr>")
        .type_text("first burst")
        .idle()
        .type_text(" second burst")
        .keys("<c-z>")
        .expect_caret_line("first burst")
        .keys("<c-y>")
        .expect_caret_line("first burst second burst")
        .done();
}

#[test]
fn emoji_and_wide_characters() {
    q4().named("emoji and wide characters")
        .keys("<c-end><cr>")
        .type_text("日本語のテキスト 🙂 café ✓ 한국어")
        .expect_caret_after("한국어")
        .keys("<left><left><bs>")
        .expect_caret_line("日本語のテキスト 🙂 café ✓ 국어")
        .keys("<home><right><right>")
        .expect_caret_after("日本")
        .type_text("X")
        .expect_saved("日本X語のテキスト 🙂 café ✓ 국어")
        .done();
}

#[test]
fn wide_characters_wrap_at_the_edge() {
    q4().named("wide characters across a soft wrap").keys("<c-end><cr>").type_text(&"語".repeat(70)).type_text("末").expect_caret_after("末").done();
}

#[test]
fn paste_a_markdown_outline() {
    q4().named("paste a multi-line Markdown outline")
        .keys("<c-end><cr>")
        .paste("- first pasted\n  - nested pasted\n- [ ] a pasted task\n- last pasted")
        .expect_line("first pasted")
        .expect_depth("nested pasted", 1)
        .expect_line("last pasted")
        .expect_saved("nested pasted")
        .done();
}

#[test]
fn paste_then_undo_and_redo() {
    q4().named("paste, undo, redo")
        .keys("<c-end><cr>")
        .paste("- one\n- two\n- three")
        .expect_line("three")
        .keys("<c-z>")
        .expect_no_line("three")
        .keys("<c-y>")
        .expect_line("three")
        .done();
}

#[test]
fn undo_and_redo_across_typing_enter_and_tab() {
    q4().named("undo and redo across typing, Enter and Tab")
        .keys("<c-end><cr>")
        .type_text("alpha")
        .idle()
        .keys("<cr>")
        .type_text("beta")
        .idle()
        .keys("<tab>")
        .idle()
        .keys("<c-z>")
        .expect_depth("beta", 0)
        .keys("<c-z><c-z>")
        .expect_no_line("beta")
        .keys("<c-y><c-y><c-y>")
        .expect_depth("beta", 1)
        .done();
}

#[test]
fn type_in_todays_journal() {
    flow("type in today's journal").keys("T<c-end><cr>").type_text("meeting notes: ").type_text(WORDS).expect_saved_contains("meeting notes: the quick").done();
}

#[test]
fn type_on_a_long_page_near_the_bottom() {
    flow_with("type near the bottom of a long page", Size::Long, (140, 36))
        .keys("<c-o>Long Page<cr>")
        .keys("<c-end>")
        .type_text("tail")
        .keys("<cr><cr>")
        .type_text("new last line")
        .expect_caret_line("new last line")
        .done();
}

#[test]
fn type_on_a_long_page_after_scrolling_mid_way() {
    flow_with("type mid-way down a long page", Size::Long, (140, 36))
        .keys("<c-o>Long Page<cr>")
        .keys("<pgdn><pgdn><pgdn>")
        .type_text("typed here ")
        .expect_caret_after("typed here ")
        .done();
}

#[test]
fn type_in_a_task_line() {
    q4().named("type in a task")
        .click_caret(doc_at("Draft the budget", 16))
        .type_text(" for Q1")
        .expect_caret_after("for Q1")
        .expect_saved_contains("Draft the budget for Q1")
        .done();
}

#[test]
fn type_a_link_with_the_popup() {
    q4().named("[[ opens the popup, Enter links")
        .keys("<c-end><cr>")
        .type_text("see [[Gar")
        .expect_screen("Garden")
        .keys("<cr>")
        .expect_caret_after("[[Garden]]")
        .done();
}

/// The typing flows that matter most, at the sizes people use: a laptop split, a full screen.
fn type_and_wrap(w: u16, h: u16) {
    let mut f = flow_with(&format!("type and wrap at {w}x{h}"), Size::Small, (w, h));
    // The last typed space ends a row: the caret's own row isn't saved (5jzx9).
    f.known("5jzx9", Known::Restore)
        .keys("<c-o>Q4 Plan<cr>")
        .click_caret(doc_at("This paragraph is long", 90))
        .type_text("WIDE 日本 🙂 ")
        .type_text(WORDS)
        .keys("<c-end><cr>")
        .type_text(&WORDS.repeat(2))
        .expect_caret_after("keep flowing ")
        .done();
}

#[test]
fn type_and_wrap_at_80x24() {
    type_and_wrap(80, 24);
}

#[test]
fn type_and_wrap_at_100x30() {
    type_and_wrap(100, 30);
}

#[test]
fn type_and_wrap_at_120x36() {
    type_and_wrap(120, 36);
}

#[test]
fn type_and_wrap_at_200x50() {
    type_and_wrap(200, 50);
}

#[test]
fn enter_at_the_bottom_scrolls_one_row_at_a_time() {
    // New lines past the view's bottom: the view follows the caret, and only when it must.
    let mut f = flow_with("Enter past the bottom of the view", Size::Long, (100, 30));
    f.keys("<c-o>Long Page<cr>").keys("<c-home><end>");
    for i in 0..30 {
        f.keys("<cr>").type_text(&format!("new {i}"));
    }
    f.expect_caret_line("new 29").done();
}

#[test]
fn backspace_joins_notes() {
    q4().named("Backspace at a line's start joins it to the one above")
        .click_caret(doc_at("Last line of the plan", 0))
        .keys("<bs>")
        .expect_caret_line("Last line of the plan")
        .keys("<bs>")
        .expect_caret_before("Last line of the plan")
        .expect("the two notes joined", |s| s.app.doc.as_ref().unwrap().caret_block().text.starts_with("This paragraph"))
        .keys("<c-z><c-z>")
        .expect_line("Last line of the plan")
        .done();
}

#[test]
fn resize_while_typing_keeps_the_caret_on_screen() {
    q4().named("resize mid-paragraph")
        .click_caret(doc_at("This paragraph is long", 200))
        .type_text("abc")
        .resize(80, 24)
        .type_text("def")
        .resize(200, 50)
        .type_text("ghi")
        .expect_caret_after("abcdefghi")
        .done();
}

#[test]
fn backspace_at_the_end_of_a_scrolled_page() {
    // Blank lines typed at the end of a long page, then taken back: the view stays put.
    let mut f = flow_with("Backspace at the end of a scrolled page", Size::Long, (100, 30));
    f.keys("<c-o>Long Page<cr><c-end>");
    for _ in 0..8 {
        f.keys("<cr>");
    }
    f.moves("<up><up><up><up><up><up>");
    for _ in 0..6 {
        f.keys_as(Motion::Typing, "<bs>");
    }
    f.done();
}

#[test]
fn a_link_hint_on_another_line_holds_still_while_typing() {
    // Edit a link into a near miss ([[Gardena]]): its hint shows on that line. Typing on a line
    // below must not make it flicker away under your eyes.
    q4().named("a near-miss link hint while typing elsewhere")
        .alt_click(doc_at("Ship the [[Garden]] redesign", 16))
        .type_text("a")
        .moves("<down><down><down><down>")
        .type_text("xyz")
        .done();
}

#[test]
fn a_click_near_the_top_or_bottom_never_scrolls() {
    // On a long page, scrolled into the middle: a click on the first or last text row places
    // the caret there and the text stays where it is under the mouse.
    let mut f = flow_with("a click near the view's edges", Size::Long, (120, 30));
    f.keys("<c-o>Long Page<cr>").keys("<pgdn><pgdn>");
    let r = f.shot.doc_view.unwrap();
    let rows: Vec<u16> = f.s.app.render.doc_hits.iter().map(|h| h.y).collect();
    let (top, bottom) = (*rows.iter().min().unwrap(), *rows.iter().filter(|y| **y < r.y + r.height).max().unwrap());
    f.alt_click(At::Cell(r.x + 12, top)).alt_click(At::Cell(r.x + 12, bottom)).done();
}

#[test]
fn a_wide_character_at_the_end_of_a_row_wraps() {
    // Fill a row to its last free column, then type a wide character: it goes to the next row,
    // never into the scrollbar's column.
    for (w, h) in [(80, 24), (120, 36)] {
        let mut f = flow_with(&format!("a wide character at a row's end at {w}x{h}"), Size::Long, (w, h));
        f.keys("<c-o>Long Page<cr><c-end><cr>").type_text("x");
        let bar = f.shot.bar_x.expect("a scrollbar on a long page");
        let mut n = 0;
        for wide in ["🙂", "日", "🙂"] {
            // Narrow characters up to one column before the scrollbar's, then the wide one.
            while f.shot.cursor.is_some_and(|(x, _)| x + 1 < bar) && n < 400 {
                f.type_text("a");
                n += 1;
            }
            f.type_text(wide);
            let (cx, _) = f.shot.cursor.unwrap();
            assert!(cx < bar, "the cursor went into the scrollbar's column after {wide:?}: {cx} ≥ {bar}");
            for y in 0..f.shot.buf.area.height {
                let sym = f.shot.buf[(bar.saturating_sub(1), y)].symbol();
                if unicode_width::UnicodeWidthStr::width(sym) > 1 {
                    f.dump();
                    panic!("{}: a wide character at column {} covers the scrollbar's column {bar} on row {y}", f.name, bar - 1);
                }
            }
        }
        f.done();
    }
}
