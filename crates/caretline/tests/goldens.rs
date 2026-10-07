//! Behaviour goldens: a starting text and selection, keys or messages, and the exact
//! expected text, selection and (where it matters) frame.
//!
//! The baseline is a macOS text field. Cases are grouped by area; the plain-text cases of
//! the editing behaviour contract are numbered `e01`… in the order of that contract.

mod common;

use caretline::{Effect, Msg};
use common::*;

// ---------------------------------------------------------------------------------------
// Selection collapse: a motion without Shift collapses a selection instead of moving from
// the caret; with Shift the anchor stays and the caret moves.

#[test]
fn e01_left_collapses_to_start() {
    golden("Hello ⟦wor▮⟧ld", "<left>", "Hello ▮world");
}

#[test]
fn e02_right_collapses_to_end() {
    golden("Hello ⟦wor▮⟧ld", "<right>", "Hello wor▮ld");
}

#[test]
fn e03_collapse_ignores_which_end_the_caret_is_at() {
    golden("Hello ⟦▮wor⟧ld", "<right>", "Hello wor▮ld");
    golden("Hello ⟦▮wor⟧ld", "<left>", "Hello ▮world");
}

#[test]
fn e04_shift_left_shrinks_from_the_caret() {
    golden("Hello ⟦wor▮⟧ld", "<s-left>", "Hello ⟦wo▮⟧rld");
}

#[test]
fn e05_shift_right_grows_from_the_caret() {
    golden("Hello ⟦wor▮⟧ld", "<s-right>", "Hello ⟦worl▮⟧d");
}

#[test]
fn e06_caret_back_on_anchor_means_no_selection() {
    golden("ab ⟦▮c⟧ d", "<s-right>", "ab c▮ d");
}

#[test]
fn e07_collapse_never_jumps_rows() {
    golden("First line\nSec⟦ond li▮⟧ne", "<left>", "First line\nSec▮ond line");
}

#[test]
fn e08_up_with_selection_starts_from_its_start() {
    golden("Line one\nLine ⟦two and▮⟧ more", "<up>", "Line ▮one\nLine two and more");
}

#[test]
fn e09_down_with_selection_starts_from_its_end() {
    golden("Line ⟦one▮⟧\nLine two", "<down>", "Line one\nLine two▮");
}

#[test]
fn e10_word_left_with_selection_starts_from_its_start() {
    golden("one two ⟦thr▮⟧ee", "<a-left>", "one ▮two three");
}

#[test]
fn e11_word_right_with_selection_starts_from_its_end() {
    golden("one ⟦tw▮⟧o three", "<a-right>", "one two▮ three");
}

#[test]
fn e12_escape_collapses_to_the_caret() {
    golden("ab ⟦cd▮⟧ ef", "<esc>", "ab cd▮ ef");
    golden("ab ⟦▮cd⟧ ef", "<esc>", "ab ▮cd ef");
}

#[test]
fn e13_shift_click_keeps_the_anchor() {
    let mut s = state("Hello ⟦wor▮⟧ld");
    send(&mut s, [Msg::Click { col: 8, row: 0, extend: true }]);
    assert_eq!(show(&s), "Hello ⟦wo▮⟧rld");
}

#[test]
fn e14_click_places_the_caret() {
    let mut s = state("⟦Hello▮⟧ world");
    send(&mut s, [Msg::Click { col: 9, row: 0, extend: false }]);
    assert_eq!(show(&s), "Hello wor▮ld");
}

#[test]
fn collapse_rules_for_line_and_document_keys() {
    golden("ab ⟦cd▮⟧ ef\ngh", "<home>", "▮ab cd ef\ngh");
    golden("ab ⟦▮cd⟧ ef\ngh", "<end>", "ab cd ef▮\ngh");
    golden("ab ⟦cd▮⟧ ef\ngh", "<d-down>", "ab cd ef\ngh▮");
    golden("ab ⟦cd▮⟧ ef\ngh", "<d-up>", "▮ab cd ef\ngh");
}

// ---------------------------------------------------------------------------------------
// Edits with a selection

#[test]
fn e15_typing_replaces_the_selection() {
    golden("Hello ⟦wor▮⟧ld", "X", "Hello X▮ld");
}

#[test]
fn e16_backspace_deletes_only_the_selection() {
    golden("Hello ⟦wor▮⟧ld", "<bs>", "Hello ▮ld");
}

#[test]
fn e17_delete_deletes_only_the_selection() {
    golden("Hello ⟦wor▮⟧ld", "<del>", "Hello ▮ld");
}

#[test]
fn e18_word_and_line_deletes_never_extend_a_selection() {
    golden("one ⟦two thr▮⟧ee", "<a-bs>", "one ▮ee");
    golden("one ⟦two thr▮⟧ee", "<a-del>", "one ▮ee");
    golden("one ⟦two thr▮⟧ee", "<d-bs>", "one ▮ee");
    golden("one ⟦two thr▮⟧ee", "<d-del>", "one ▮ee");
    golden("one ⟦two thr▮⟧ee", "<c-k>", "one ▮ee");
    golden("one ⟦two thr▮⟧ee", "<c-w>", "one ▮ee");
    golden("one ⟦two thr▮⟧ee", "<c-u>", "one ▮ee");
}

#[test]
fn e19_typing_over_a_multi_line_selection() {
    golden("One ⟦two\nthree fo▮⟧ur", "X", "One X▮ur");
}

#[test]
fn e20_backspace_over_whole_lines() {
    golden("A⟦a\nBb\nC▮⟧c", "<bs>", "A▮c");
}

#[test]
fn e21_enter_replaces_the_selection_with_a_line_break() {
    golden("ab⟦cd▮⟧ef", "<cr>", "ab\n▮ef");
}

#[test]
fn e24_one_undo_restores_text_and_selection_after_typing_over_it() {
    golden("Hello ⟦wor▮⟧ld", "XY<c-z>", "Hello ⟦wor▮⟧ld");
    golden("Hello ⟦▮wor⟧ld", "XY<d-z>", "Hello ⟦▮wor⟧ld");
}

// ---------------------------------------------------------------------------------------
// Word and line deletes without a selection

#[test]
fn e25_word_delete_back_inside_a_word() {
    golden("one two thr▮ee", "<a-bs>", "one two ▮ee");
}

#[test]
fn e26_word_delete_back_skips_spaces_first() {
    golden("one two ▮three", "<a-bs>", "one ▮three");
    golden("one two ▮three", "<c-w>", "one ▮three");
}

#[test]
fn e27_word_delete_back_at_a_line_start_joins_like_backspace() {
    golden("First\n▮Second", "<a-bs>", "First▮Second");
    golden("First▮\nSecond", "<a-del>", "First▮Second");
}

#[test]
fn e28_word_delete_forward() {
    golden("one ▮two three", "<a-del>", "one ▮ three");
    golden("one ▮two three", "<a-d>", "one ▮ three");
}

#[test]
fn e29_delete_to_row_start() {
    golden("one two thr▮ee", "<d-bs>", "▮ee");
    golden("one two thr▮ee", "<c-u>", "▮ee");
    // At a row start it deletes the previous character (the line break).
    golden("one\n▮two", "<d-bs>", "one▮two");
}

#[test]
fn e30_kill_to_the_line_end() {
    golden("one ▮two\nthree", "<c-k>", "one ▮\nthree");
}

#[test]
fn e31_kill_at_the_line_end_removes_the_break() {
    golden("one▮\nthree", "<c-k>", "one▮three");
}

#[test]
fn e32_delete_to_row_end() {
    golden("one ▮two", "<d-del>", "one ▮");
}

// ---------------------------------------------------------------------------------------
// The clipboard

#[test]
fn e33_copy_changes_nothing_but_the_clipboard() {
    let mut s = state("Hello ⟦wor▮⟧ld");
    let before = s.clone();
    let fx = keys(&mut s, "<d-c>");
    assert_eq!(fx, vec![Effect::ClipboardSet { text: "wor".into() }]);
    assert_eq!(s.doc.clipboard, "wor");
    assert_eq!(show(&s), "Hello ⟦wor▮⟧ld");
    assert_eq!(s.doc.history, before.doc.history);
    assert_eq!(s.doc.dirty, before.doc.dirty);
    // The Ctrl twin does the same.
    let mut t = before.clone();
    assert_eq!(keys(&mut t, "<c-c>"), fx);
}

#[test]
fn e36_copy_with_nothing_selected_copies_nothing() {
    let mut s = state("Hello wor▮ld");
    s.doc.clipboard = "kept".into();
    let fx = keys(&mut s, "<c-c>");
    assert!(fx.is_empty());
    assert_eq!(s.doc.clipboard, "kept");
    assert_eq!(s.view.status.as_deref(), Some("nothing selected"));
    assert_eq!(show(&s), "Hello wor▮ld");
}

#[test]
fn e37_cut_is_one_undo_step() {
    let mut s = state("Hello ⟦wor▮⟧ld");
    let fx = keys(&mut s, "<c-x>");
    assert_eq!(fx, vec![Effect::ClipboardSet { text: "wor".into() }]);
    assert_eq!(show(&s), "Hello ▮ld");
    keys(&mut s, "<c-z>");
    assert_eq!(show(&s), "Hello ⟦wor▮⟧ld");
}

#[test]
fn e38_paste_replaces_the_selection() {
    let mut s = state("Hello ⟦wor▮⟧ld");
    s.doc.clipboard = "X".into();
    keys(&mut s, "<c-v>");
    assert_eq!(show(&s), "Hello X▮ld");
}

#[test]
fn e39_multi_line_paste_is_one_step() {
    let mut s = state("▮");
    send(&mut s, [Msg::Paste { text: Some("- a\n- b".into()) }]);
    assert_eq!(show(&s), "- a\n- b▮");
    keys(&mut s, "<c-z>");
    assert_eq!(show(&s), "▮");
}

#[test]
fn e40_cut_then_paste_in_place_is_the_identity() {
    let mut s = state("One ⟦two\nth▮⟧ree");
    keys(&mut s, "<c-x><c-v>");
    assert_eq!(show(&s), "One two\nth▮ree");
}

#[test]
fn e41_select_all_then_left() {
    let mut s = state("ab\ncd▮");
    keys(&mut s, "<d-a>");
    assert_eq!(show(&s), "⟦ab\ncd▮⟧");
    keys(&mut s, "<d-a>");
    assert_eq!(show(&s), "⟦ab\ncd▮⟧", "a second select-all changes nothing");
    keys(&mut s, "<left>");
    assert_eq!(show(&s), "▮ab\ncd");
    // Alt-A is the twin for terminals that don't forward Cmd.
    keys(&mut s, "<a-a>");
    assert_eq!(show(&s), "⟦ab\ncd▮⟧");
}

#[test]
fn e42_select_all_delete_then_undo() {
    let mut s = state("ab\ncd▮");
    keys(&mut s, "<d-a><bs>");
    assert_eq!(show(&s), "▮");
    keys(&mut s, "<c-z>");
    assert_eq!(show(&s), "⟦ab\ncd▮⟧");
}

#[test]
fn e43_redo_with_nothing_to_redo() {
    let mut s = state("Hello ⟦wor▮⟧ld");
    keys(&mut s, "X");
    keys(&mut s, "<c-s-z>");
    assert_eq!(show(&s), "Hello X▮ld");
    assert_eq!(s.view.status.as_deref(), Some("nothing to redo"));
    keys(&mut s, "<c-z>");
    assert_eq!(s.view.status, None);
    keys(&mut s, "<c-y>");
    assert_eq!(show(&s), "Hello X▮ld");
}

// ---------------------------------------------------------------------------------------
// Undo grouping

#[test]
fn typing_within_the_gap_is_one_step_and_a_pause_starts_another() {
    let mut s = state("▮");
    keys(&mut s, "ab<wait:1000>cd<wait:1600>ef");
    assert_eq!(show(&s), "abcdef▮");
    keys(&mut s, "<c-z>");
    assert_eq!(show(&s), "abcd▮");
    keys(&mut s, "<c-z>");
    assert_eq!(show(&s), "▮");
    keys(&mut s, "<c-s-z>");
    assert_eq!(show(&s), "abcd▮");
    keys(&mut s, "<c-s-z>");
    assert_eq!(show(&s), "abcdef▮");
}

#[test]
fn a_motion_ends_a_typing_run() {
    let mut s = state("▮");
    keys(&mut s, "ab<left>c");
    assert_eq!(show(&s), "ac▮b");
    keys(&mut s, "<c-z>");
    assert_eq!(show(&s), "a▮b");
    keys(&mut s, "<c-z>");
    assert_eq!(show(&s), "▮");
}

#[test]
fn every_command_is_its_own_step() {
    let mut s = state("one two three▮");
    keys(&mut s, "<a-bs><a-bs>");
    assert_eq!(show(&s), "one ▮");
    keys(&mut s, "<c-z>");
    assert_eq!(show(&s), "one two ▮");
}

#[test]
fn held_backspace_is_one_step() {
    let mut s = state("abcdef▮");
    keys(&mut s, "<bs><bs><bs>");
    assert_eq!(show(&s), "abc▮");
    keys(&mut s, "<c-z>");
    assert_eq!(show(&s), "abcdef▮");
}

#[test]
fn undo_and_redo_restore_exact_text_and_selection() {
    let mut s = state("alpha ⟦▮beta⟧ gamma");
    keys(&mut s, "<a-del>");
    assert_eq!(show(&s), "alpha ▮ gamma");
    keys(&mut s, "<right><wait:2000>X");
    assert_eq!(show(&s), "alpha  X▮gamma");
    keys(&mut s, "<c-z>");
    assert_eq!(show(&s), "alpha  ▮gamma");
    keys(&mut s, "<c-z>");
    assert_eq!(show(&s), "alpha ⟦▮beta⟧ gamma");
    keys(&mut s, "<c-z>");
    assert_eq!(s.view.status.as_deref(), Some("nothing to undo"));
    keys(&mut s, "<c-y>");
    assert_eq!(show(&s), "alpha ▮ gamma");
    keys(&mut s, "<c-y>");
    assert_eq!(show(&s), "alpha  X▮gamma");
}

#[test]
fn saving_marks_clean_and_editing_marks_dirty() {
    let mut s = state("ab▮");
    assert!(!s.doc.dirty);
    keys(&mut s, "c");
    assert!(s.doc.dirty);
    let fx = keys(&mut s, "<c-s>");
    assert_eq!(
        fx,
        vec![Effect::WriteFile { path: "test.md".into(), text: "abc".into() }]
    );
    send(&mut s, [Msg::Saved]);
    assert!(!s.doc.dirty);
    // Typing straight after a save starts a new step, so the save point stays exact.
    keys(&mut s, "d");
    assert!(s.doc.dirty);
    keys(&mut s, "<c-z>");
    assert!(!s.doc.dirty);
    assert_eq!(show(&s), "abc▮");
}

#[test]
fn quitting_with_unsaved_changes_asks_twice() {
    let mut s = state("ab▮");
    assert_eq!(keys(&mut s, "<c-q>"), vec![Effect::Quit]);
    keys(&mut s, "c");
    assert!(keys(&mut s, "<c-q>").is_empty());
    assert!(s.view.status.is_some());
    assert_eq!(keys(&mut s, "<c-q>"), vec![Effect::Quit]);
}

// ---------------------------------------------------------------------------------------
// Basic editing

#[test]
fn enter_in_mid_line_splits_it() {
    golden("hel▮lo", "<cr>", "hel\n▮lo");
}

#[test]
fn backspace_and_delete_join_lines() {
    golden("hel\n▮lo", "<bs>", "hel▮lo");
    golden("hel▮\nlo", "<del>", "hel▮lo");
}

#[test]
fn edges_of_the_document_are_safe() {
    golden("▮abc", "<bs><left><up><home><a-left>", "▮abc");
    golden("abc▮", "<del><right><down><end><a-right>", "abc▮");
}

#[test]
fn up_on_the_first_row_goes_to_the_start_and_down_on_the_last_to_the_end() {
    golden("ab▮c\ndef", "<up>", "▮abc\ndef");
    golden("abc\nd▮ef", "<down>", "abc\ndef▮");
}

#[test]
fn crlf_documents_stay_crlf() {
    let mut s = state("a▮\r\nb");
    keys(&mut s, "<cr>");
    assert_eq!(s.doc.text.to_string(), "a\r\n\r\nb");
    // A CRLF is one grapheme: one backspace removes both characters.
    keys(&mut s, "<bs>");
    assert_eq!(s.doc.text.to_string(), "a\r\nb");
    send(&mut s, [Msg::Paste { text: Some("x\ny".into()) }]);
    assert_eq!(s.doc.text.to_string(), "ax\r\ny\r\nb");
}

#[test]
fn shift_motions_build_selections() {
    golden("one ▮two three", "<s-a-right>", "one ⟦two▮⟧ three");
    golden("one two▮ three", "<s-a-left><s-a-left>", "⟦▮one two⟧ three");
    golden("one ▮two\nthree", "<s-down>", "one ⟦two\nthre▮⟧e");
    golden("one ▮two", "<s-end>", "one ⟦two▮⟧");
    golden("one ▮two", "<s-home>", "⟦▮one ⟧two");
}

#[test]
fn tab_inserts_a_tab_and_renders_to_the_stop() {
    let mut s = state_wh("a▮b", 20, 3);
    keys(&mut s, "<tab>");
    assert_eq!(show(&s), "a\t▮b");
    assert_eq!(frame(&s).lines().next().unwrap(), "a   b");
    assert_eq!(cursor(&s), Some((4, 0)));
}

// ---------------------------------------------------------------------------------------
// Word motion

#[test]
fn word_motion_moves_by_words_skipping_spaces_and_punctuation() {
    golden("▮Hello, wide world!", "<a-right>", "Hello▮, wide world!");
    golden("▮Hello, wide world!", "<a-right><a-right>", "Hello, wide▮ world!");
    golden("▮Hello, wide world!", "<a-right><a-right><a-right><a-right>", "Hello, wide world!▮");
    golden("Hello, wide world!▮", "<a-left>", "Hello, wide ▮world!");
    golden("Hello, wide world!▮", "<a-left><a-left><a-left>", "▮Hello, wide world!");
    golden("snake_case wo▮rd", "<a-left><a-left>", "▮snake_case word");
    // Emacs twins.
    golden("one ▮two", "<a-f>", "one two▮");
    golden("one two▮", "<a-b>", "one ▮two");
    // Ctrl-arrows move by word too.
    golden("one ▮two", "<c-right>", "one two▮");
}

#[test]
fn word_motion_crosses_lines() {
    golden("one▮\ntwo", "<a-right>", "one\ntwo▮");
    golden("one\n▮two", "<a-left>", "▮one\ntwo");
}

// ---------------------------------------------------------------------------------------
// Unicode: emoji, combining marks, wide characters

#[test]
fn emoji_with_a_modifier_is_one_step() {
    golden("a▮👍🏽b", "<right>", "a👍🏽▮b");
    golden("a👍🏽▮b", "<left>", "a▮👍🏽b");
    golden("a👍🏽▮b", "<bs>", "a▮b");
    golden("a▮👍🏽b", "<del>", "a▮b");
    golden("a▮👍🏽b", "<s-right>", "a⟦👍🏽▮⟧b");
}

#[test]
fn zwj_family_emoji_is_one_grapheme() {
    golden("▮👨‍👩‍👧x", "<right>", "👨‍👩‍👧▮x");
    golden("👨‍👩‍👧▮x", "<bs>", "▮x");
}

#[test]
fn combining_accents_move_with_their_base() {
    golden("cafe\u{301}▮ ok", "<left>", "caf▮e\u{301} ok");
    golden("caf▮e\u{301} ok", "<right>", "cafe\u{301}▮ ok");
    golden("cafe\u{301}▮ ok", "<bs>", "caf▮ ok");
    // The accented letter is part of the word.
    golden("▮cafe\u{301} ok", "<a-right>", "cafe\u{301}▮ ok");
}

#[test]
fn wide_characters_take_two_columns() {
    let s = state_wh("漢字ab▮c", 20, 3);
    assert_eq!(frame(&s).lines().next().unwrap(), "漢字abc");
    assert_eq!(cursor(&s), Some((6, 0)));
    // Vertical motion keeps the visual column across wide characters.
    golden("漢字▮abc\nabcdef", "<down>", "漢字abc\nabcd▮ef");
    golden("漢字abc\nab▮cdef", "<up>", "漢▮字abc\nabcdef");
    golden("漢字abc\nabc▮def", "<up>", "漢▮字abc\nabcdef");
}

#[test]
fn emoji_and_cjk_render_with_the_caret_on_the_right_cell() {
    let mut s = state_wh("▮", 20, 3);
    send(&mut s, [Msg::InsertText { text: "a👍🏽漢e\u{301}".into() }]);
    assert_eq!(frame(&s).lines().next().unwrap(), "a👍🏽漢e\u{301}");
    // a(1) + emoji(2) + wide(2) + accented e(1).
    assert_eq!(cursor(&s), Some((6, 0)));
}

#[test]
fn a_keycap_emoji_takes_two_cells() {
    // `1️⃣` starts with an ASCII digit but is an emoji (2 cells), as terminals draw it.
    let mut s = state_wh("▮", 20, 3);
    send(&mut s, [Msg::InsertText { text: "a1\u{fe0f}\u{20e3}b".into() }]);
    assert_eq!(cursor(&s), Some((4, 0)));
    golden("a▮1\u{fe0f}\u{20e3}b", "<right>", "a1\u{fe0f}\u{20e3}▮b");
}

// ---------------------------------------------------------------------------------------
// Soft wrap: visual rows, the goal column, Home and End

/// Three lines; at width 20 the first and last wrap:
/// row 0 `aaaa bbbb cccc dddd ` · row 1 `eeee ffff gggg` · row 2 `xy` ·
/// row 3 `hhhh iiii jjjj kkkk ` · row 4 `llll`.
const WRAPPED: &str = "aaaa bbbb cccc dddd eeee ffff gggg\nxy\nhhhh iiii jjjj kkkk llll";

fn wrapped(at: &str) -> caretline::State {
    // `at` is WRAPPED with a caret mark in it.
    state_wh(at, 20, 7)
}

#[test]
fn wrapped_paragraph_frame() {
    let s = wrapped(&format!("▮{WRAPPED}"));
    assert_eq!(
        frame(&s),
        "aaaa bbbb cccc dddd\neeee ffff gggg\nxy\nhhhh iiii jjjj kkkk\nllll\n\n test.md        1:1\n"
    );
    assert_eq!(cursor(&s), Some((0, 0)));
}

#[test]
fn arrows_across_wrapped_rows_keep_the_goal_column() {
    let mut s = wrapped("aaaa bbbb cc▮cc dddd eeee ffff gggg\nxy\nhhhh iiii jjjj kkkk llll");
    keys(&mut s, "<down>");
    assert_eq!(show(&s), "aaaa bbbb cccc dddd eeee ffff gg▮gg\nxy\nhhhh iiii jjjj kkkk llll");
    assert_eq!(cursor(&s), Some((12, 1)));
    keys(&mut s, "<down>");
    assert_eq!(show(&s), "aaaa bbbb cccc dddd eeee ffff gggg\nxy▮\nhhhh iiii jjjj kkkk llll");
    keys(&mut s, "<down>");
    assert_eq!(show(&s), "aaaa bbbb cccc dddd eeee ffff gggg\nxy\nhhhh iiii jj▮jj kkkk llll");
    keys(&mut s, "<down>");
    assert_eq!(show(&s), "aaaa bbbb cccc dddd eeee ffff gggg\nxy\nhhhh iiii jjjj kkkk llll▮");
    keys(&mut s, "<up><up><up>");
    assert_eq!(show(&s), "aaaa bbbb cccc dddd eeee ffff gg▮gg\nxy\nhhhh iiii jjjj kkkk llll");
    // A horizontal move resets the goal column.
    keys(&mut s, "<left><down><down>");
    assert_eq!(show(&s), "aaaa bbbb cccc dddd eeee ffff gggg\nxy\nhhhh iiii j▮jjj kkkk llll");
}

#[test]
fn logical_line_motion_ignores_wrapping() {
    let mut s = wrapped("aaaa bbbb cc▮cc dddd eeee ffff gggg\nxy\nhhhh iiii jjjj kkkk llll");
    send(
        &mut s,
        [Msg::Move { dir: caretline::Dir::Forward, by: caretline::By::Line, extend: false }],
    );
    assert_eq!(show(&s), "aaaa bbbb cccc dddd eeee ffff gggg\nxy▮\nhhhh iiii jjjj kkkk llll");
}

#[test]
fn home_and_end_work_on_visual_rows() {
    // On the second row of the wrapped line.
    let mut s = wrapped("aaaa bbbb cccc dddd eeee ff▮ff gggg\nxy\nhhhh iiii jjjj kkkk llll");
    keys(&mut s, "<home>");
    assert_eq!(show(&s), "aaaa bbbb cccc dddd ▮eeee ffff gggg\nxy\nhhhh iiii jjjj kkkk llll");
    assert_eq!(cursor(&s), Some((0, 1)));
    keys(&mut s, "<end>");
    assert_eq!(show(&s), "aaaa bbbb cccc dddd eeee ffff gggg▮\nxy\nhhhh iiii jjjj kkkk llll");
    // On the first row, End stops where the row wraps (before the wrapping space), so the
    // caret stays on that row.
    let mut s = wrapped("aaaa bb▮bb cccc dddd eeee ffff gggg\nxy\nhhhh iiii jjjj kkkk llll");
    keys(&mut s, "<end>");
    assert_eq!(show(&s), "aaaa bbbb cccc dddd▮ eeee ffff gggg\nxy\nhhhh iiii jjjj kkkk llll");
    assert_eq!(cursor(&s), Some((19, 0)));
    keys(&mut s, "<home>");
    assert_eq!(show(&s), "▮aaaa bbbb cccc dddd eeee ffff gggg\nxy\nhhhh iiii jjjj kkkk llll");
    // The Cmd and Ctrl twins.
    keys(&mut s, "<d-right>");
    assert_eq!(cursor(&s), Some((19, 0)));
    keys(&mut s, "<c-a>");
    assert_eq!(cursor(&s), Some((0, 0)));
    keys(&mut s, "<c-e>");
    assert_eq!(cursor(&s), Some((19, 0)));
}

#[test]
fn delete_to_row_start_on_a_wrapped_row() {
    let mut s = wrapped("aaaa bbbb cccc dddd eeee ff▮ff gggg\nxy");
    keys(&mut s, "<d-bs>");
    assert_eq!(show(&s), "aaaa bbbb cccc dddd ▮ff gggg\nxy");
}

#[test]
fn wrapped_continuation_rows_keep_the_indent() {
    let s = state_wh("▮  - item one two three four five six", 20, 4);
    assert_eq!(frame(&s), "  - item one two\n  three four five\n  six\n test.md        1:1\n");
}

#[test]
fn narrow_viewports_turn_wrapping_off_and_scroll_sideways() {
    let mut s = state_wh("▮abcdefghijklmnop", 8, 2);
    assert_eq!(frame(&s).lines().next().unwrap(), "abcdefgh");
    keys(&mut s, "<end>");
    assert_eq!(frame(&s).lines().next().unwrap(), "jklmnop");
    assert_eq!(cursor(&s), Some((7, 0)));
}

// ---------------------------------------------------------------------------------------
// Paste

#[test]
fn paste_of_multi_line_text_at_the_caret() {
    let mut s = state("x▮y");
    send(&mut s, [Msg::Paste { text: Some("one\ntwo\nthree".into()) }]);
    assert_eq!(show(&s), "xone\ntwo\nthree▮y");
    keys(&mut s, "<c-z>");
    assert_eq!(show(&s), "x▮y");
    keys(&mut s, "<c-y>");
    assert_eq!(show(&s), "xone\ntwo\nthree▮y");
}

#[test]
fn paste_normalizes_line_endings() {
    let mut s = state("▮");
    send(&mut s, [Msg::Paste { text: Some("a\r\nb\rc".into()) }]);
    assert_eq!(show(&s), "a\nb\nc▮");
}

// ---------------------------------------------------------------------------------------
// Scrolling and pages

fn numbered(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n")
}

#[test]
fn page_down_moves_a_screenful_and_keeps_the_column() {
    let mut s = state_wh(&format!("line▮ 1\n{}", &numbered(60)["line 1\n".len()..]), 20, 11);
    keys(&mut s, "<pgdn>");
    assert_eq!(s.doc.text.char_to_line(s.caret()), 10);
    assert_eq!(cursor(&s).map(|c| c.0), Some(4));
    keys(&mut s, "<pgdn><pgup>");
    assert_eq!(s.doc.text.char_to_line(s.caret()), 10);
    keys(&mut s, "<pgup><pgup>");
    assert_eq!(s.caret(), 0);
}

#[test]
fn the_view_follows_the_caret_with_a_margin() {
    let mut s = state_wh(&format!("▮{}", numbered(40)), 20, 11);
    for _ in 0..9 {
        keys(&mut s, "<down>");
    }
    // Ten text rows, a two-row margin: line 10 sits on row 7, so the view scrolled by 2.
    assert_eq!(s.view.scroll.line, 2);
    assert_eq!(cursor(&s), Some((0, 7)));
    keys(&mut s, "<d-down>");
    assert_eq!(frame(&s).lines().nth(9).unwrap(), "line 40");
    keys(&mut s, "<d-up>");
    assert_eq!(s.view.scroll.line, 0);
}

#[test]
fn wheel_scrolling_drags_the_caret_along() {
    let mut s = state_wh(&format!("▮{}", numbered(40)), 20, 11);
    send(&mut s, [Msg::Scroll { rows: 5 }]);
    assert_eq!(s.view.scroll.line, 5);
    // The caret moved down to stay two rows inside the view.
    assert_eq!(s.doc.text.char_to_line(s.caret()), 7);
    send(&mut s, [Msg::Scroll { rows: 100 }]);
    assert_eq!(s.view.scroll.line, 30, "stops with the last line at the bottom");
}

#[test]
fn clicking_below_the_text_goes_to_the_end() {
    let mut s = state_wh("ab\n▮cd", 20, 10);
    send(&mut s, [Msg::Click { col: 1, row: 8, extend: false }]);
    assert_eq!(show(&s), "ab\ncd▮");
    send(&mut s, [Msg::Click { col: 15, row: 0, extend: false }]);
    assert_eq!(show(&s), "ab▮\ncd");
}

// ---------------------------------------------------------------------------------------
// Status bar

#[test]
fn status_bar_shows_name_dirty_marker_and_position() {
    let mut s = state_wh("ab\nc▮d", 30, 3);
    assert_eq!(frame(&s).lines().nth(2).unwrap(), " test.md                  2:2");
    keys(&mut s, "x");
    assert_eq!(frame(&s).lines().nth(2).unwrap(), " test.md [+]              2:3");
    keys(&mut s, "<s-left><s-left>");
    assert_eq!(frame(&s).lines().nth(2).unwrap(), " test.md [+]       2 sel  2:1");
    keys(&mut s, "<c-c>");
    assert_eq!(frame(&s).lines().nth(2).unwrap(), " test.md [+]  copi 2 sel  2:1");
}
