//! Outline goldens: block documents before, keys, and the exact document, selection and block
//! ids after.
//!
//! Notation: each block is written as its first line's text (indentation and marker
//! included), `⏎` is a soft break (a continuation line), ` ‖ ` separates two blocks with a
//! blank row between them and ` ¦ ` two blocks with none. `▮` is the caret and `⟦…⟧` a
//! selection, as in the plain goldens. Blocks get ids 0, 1, 2… in order.
//!
//! The cases that share a number with a plain-text golden (`e22`, `e70`…) are the outline
//! half of the same editing contract.

mod common;

use caretline_next::helix::Selection;
use caretline_next::outline::markdown;
use caretline_next::{script_to_msgs_for, update, view, BlockAttrs, Effect, Kind, MarkId, Msg, NewBlock, OutlineConfig, State, Viewport};

// ---------------------------------------------------------------------------------------
// The notation

/// A document from notation, 80x24.
fn doc(notation: &str) -> State {
    doc_wh(notation, 80, 24)
}

fn doc_wh(notation: &str, width: u16, height: u16) -> State {
    // Split into blocks, remembering each one's separator.
    let mut blocks: Vec<(String, bool)> = Vec::new();
    let mut rest = notation;
    let mut gap = false;
    loop {
        let a = rest.find(" ‖ ");
        let b = rest.find(" ¦ ");
        let (at, next_gap, len) = match (a, b) {
            (Some(x), Some(y)) if x < y => (x, true, " ‖ ".len()),
            (_, Some(y)) => (y, false, " ¦ ".len()),
            (Some(x), None) => (x, true, " ‖ ".len()),
            (None, None) => break,
        };
        blocks.push((rest[..at].to_string(), gap));
        gap = next_gap;
        rest = &rest[at + len..];
    }
    blocks.push((rest.to_string(), gap));

    let mut text = String::new();
    let mut n = 0usize;
    let mut starts = Vec::new();
    let (mut caret, mut open, mut close) = (None, None, None);
    for (k, (b, _)) in blocks.iter().enumerate() {
        if k > 0 {
            text.push('\n');
            n += 1;
        }
        starts.push(n);
        for c in b.chars() {
            match c {
                '▮' => caret = Some(n),
                '⟦' => open = Some(n),
                '⟧' => close = Some(n),
                '⏎' => {
                    text.push('\n');
                    n += 1;
                }
                c => {
                    text.push(c);
                    n += 1;
                }
            }
        }
    }
    let mut s = State::new(&text, Some("t.md".into()), Viewport { width, height });
    for &p in &starts {
        s.doc.marks.mint(p);
    }
    s.enable_outline(OutlineConfig::default());
    // Blank rows as written: explicit only where the default differs.
    let o = s.blocks().unwrap();
    for (k, (_, want)) in blocks.iter().enumerate().skip(1) {
        let b = o.get(MarkId(k as u64)).unwrap_or_else(|| panic!("no block for segment {k} of {notation:?}"));
        if b.gap != *want {
            s.doc.marks.set_attrs(b.id, BlockAttrs { gap: Some(*want) });
        }
    }
    s.outline_changed();
    let head = caret.expect("notation needs a caret ▮");
    let anchor = match (open, close) {
        (Some(o), Some(c)) => {
            if head == o {
                c
            } else {
                o
            }
        }
        _ => head,
    };
    s.view.selection = Selection::single(anchor, head);
    update(&mut s, Msg::Resize { width, height });
    s
}

/// The document in notation.
fn show(s: &State) -> String {
    let o = s.blocks().expect("an outline");
    let r = s.view.selection.primary();
    let chars: Vec<char> = s.doc.text.chars().collect();
    let mut out = String::new();
    let mark = |out: &mut String, i: usize| {
        if r.anchor == r.head {
            if i == r.head {
                out.push('▮');
            }
            return;
        }
        if i == r.from() {
            out.push('⟦');
            if r.head == i {
                out.push('▮');
            }
        }
        if i == r.to() {
            if r.head == i {
                out.push('▮');
            }
            out.push('⟧');
        }
    };
    for (k, b) in o.blocks.iter().enumerate() {
        if k > 0 {
            out.push_str(if b.gap { " ‖ " } else { " ¦ " });
        }
        for (i, &c) in chars.iter().enumerate().take(b.end).skip(b.start) {
            mark(&mut out, i);
            out.push(if c == '\n' { '⏎' } else { c });
        }
        mark(&mut out, b.end);
    }
    out
}

/// Block ids in document order.
fn ids(s: &State) -> Vec<u64> {
    s.blocks().unwrap().blocks.iter().map(|b| b.id.0).collect()
}

/// Runs keys through the outline keymap; returns the effects.
fn keys(s: &mut State, script: &str) -> Vec<Effect> {
    let msgs = script_to_msgs_for(script, s.doc.now_ms, true).expect("key script");
    msgs.into_iter().flat_map(|m| update(s, m)).collect()
}

fn send(s: &mut State, msgs: impl IntoIterator<Item = Msg>) -> Vec<Effect> {
    msgs.into_iter().flat_map(|m| update(s, m)).collect()
}

/// `before · keys → after`.
#[track_caller]
fn golden(before: &str, script: &str, after: &str) -> State {
    let mut s = doc(before);
    keys(&mut s, script);
    assert_eq!(show(&s), after, "{before:?} · {script:?}");
    s
}

fn clip(fx: &[Effect]) -> Option<String> {
    fx.iter().find_map(|e| match e {
        Effect::ClipboardSet { text } => Some(text.clone()),
        _ => None,
    })
}

#[test]
fn the_notation_round_trips() {
    for n in ["A ‖ B▮", "- a ¦ - b ‖ Para⏎more▮", "▮", "# Head ‖ Text ‖ - [ ] t ¦   - [x] u▮", "x ¦ y▮"] {
        assert_eq!(show(&doc(n)), n);
    }
}

// ---------------------------------------------------------------------------------------
// Selection and edits across blocks

#[test]
fn e07_collapsing_never_jumps_to_another_block() {
    golden("First note ‖ Sec⟦ond no▮⟧te", "<left>", "First note ‖ Sec▮ond note");
}

#[test]
fn e08_e09_up_and_down_step_over_the_blank_row() {
    golden("Line one ‖ Line ⟦two and▮⟧ more", "<up>", "Line ▮one ‖ Line two and more");
    golden("Line ⟦one▮⟧ ‖ Line two", "<down>", "Line one ‖ Line two▮");
}

#[test]
fn e19_typing_over_blocks_keeps_the_first_id() {
    let s = golden("One ⟦two ‖ three fo▮⟧ur", "X", "One X▮ur");
    assert_eq!(ids(&s), [0]);
}

#[test]
fn e20_deleting_over_blocks_removes_the_covered_ones() {
    let mut s = golden("A⟦a ‖ Bb ‖ C▮⟧c", "<bs>", "A▮c");
    assert_eq!(ids(&s), [0]);
    keys(&mut s, "<c-z>");
    assert_eq!(show(&s), "A⟦a ‖ Bb ‖ C▮⟧c");
    assert_eq!(ids(&s), [0, 1, 2]);
}

#[test]
fn e22_tab_nests_every_selected_item_and_keeps_the_selection() {
    golden("- o⟦ne ¦ - tw▮⟧o", "<tab>", "- o⟦ne ¦   - tw▮⟧o");
    golden("- o⟦ne ¦   - tw▮⟧o", "<s-tab>", "- o⟦ne ¦ - tw▮⟧o");
}

#[test]
fn e22_a_paragraph_doesnt_nest() {
    let mut s = doc("Para▮");
    keys(&mut s, "<tab>");
    assert_eq!(show(&s), "Para▮");
    assert_eq!(s.view.status.as_deref(), Some("paragraphs don't nest"));
}

#[test]
fn e23_the_task_cycle_applies_to_every_selected_block() {
    golden("o⟦ne ‖ tw▮⟧o", "<c-t>", "- [ ] o⟦ne ‖ - [ ] tw▮⟧o");
}

#[test]
fn e27_a_word_delete_at_a_paragraph_start_joins_like_backspace() {
    let s = golden("First ‖ ▮Second", "<a-bs>", "First⏎▮Second");
    assert_eq!(ids(&s), [0]);
}

#[test]
fn word_and_row_deletes_stop_at_the_marker() {
    golden("- [ ] ...▮", "<a-bs>", "- [ ] ▮");
    golden("- [ ] one two▮", "<d-bs>", "- [ ] ▮");
    golden("- ab▮ ¦ - cd", "<a-del>", "- ab▮cd");
}

// ---------------------------------------------------------------------------------------
// The clipboard

#[test]
fn e34_e35_copy_across_blocks_writes_markdown() {
    let mut s = doc("- [ ] Buy ⟦milk ¦ - [ ] Call ▮⟧Sam");
    assert_eq!(clip(&keys(&mut s, "<c-c>")).as_deref(), Some("milk\n- [ ] Call "));
    assert_eq!(show(&s), "- [ ] Buy ⟦milk ¦ - [ ] Call ▮⟧Sam", "copy changes nothing");
    let mut s = doc("⟦One ‖ - [x] Two▮⟧");
    assert_eq!(clip(&keys(&mut s, "<c-c>")).as_deref(), Some("One\n\n- [x] Two"));
    let mut s = doc("Plan the trip⟦ ‖ - [ ] Book the flat▮⟧");
    assert_eq!(clip(&keys(&mut s, "<c-c>")).as_deref(), Some("\n\n- [ ] Book the flat"));
    let mut s = doc("Hello ⟦wor▮⟧ld");
    assert_eq!(clip(&keys(&mut s, "<c-c>")).as_deref(), Some("wor"), "inside one block: plain text");
}

#[test]
fn e39_pasting_a_list_into_an_empty_block_makes_items() {
    let mut s = doc("▮");
    send(&mut s, [Msg::Paste { text: Some("- a\n- b".into()) }]);
    assert_eq!(show(&s), "- a ¦ - b▮");
    keys(&mut s, "<c-z>");
    assert_eq!(show(&s), "▮", "one undo step");
}

#[test]
fn pasted_markdown_joins_the_text_around_the_caret() {
    let mut s = doc("Start ▮end");
    send(&mut s, [Msg::Paste { text: Some("one\n\n- two\n- [ ] three".into()) }]);
    assert_eq!(show(&s), "Start one ‖ - two ¦ - [ ] three▮end");
    let mut s = doc("- [ ] ▮");
    send(&mut s, [Msg::Paste { text: Some("a\n\nb".into()) }]);
    assert_eq!(show(&s), "a ‖ b▮", "an empty item takes the first block's shape");
}

#[test]
fn a_plain_paste_keeps_line_breaks_and_reads_no_lists() {
    let mut s = doc("▮");
    send(&mut s, [Msg::PastePlain { text: Some("- a\nb\n\nc".into()) }]);
    assert_eq!(show(&s), "- a⏎b ‖ c▮");
    // A soft break inside the first block: "- a" read as text would be a marker, so the
    // pasted paragraph starts a list item. Plain text never makes a task or nests.
}

#[test]
fn e40_cut_then_paste_is_a_move_with_the_same_ids() {
    let mut s = doc("One ⟦two ‖ th▮⟧ree");
    keys(&mut s, "<c-x>");
    assert_eq!(show(&s), "One ▮ree");
    keys(&mut s, "<c-v>");
    assert_eq!(show(&s), "One two ‖ th▮ree");
    assert_eq!(ids(&s), [0, 1]);
}

#[test]
fn cut_blocks_pasted_elsewhere_keep_their_ids_and_blank_rows() {
    let mut s = doc("⟦- a ¦ - b▮⟧ ¦ - c");
    // Select the whole lines, from the first content start to the next block's start.
    let c = s.blocks().unwrap().blocks[2].start;
    s.view.selection = Selection::single(0, c);
    keys(&mut s, "<c-x>");
    assert_eq!(show(&s), "- ▮c");
    assert_eq!(ids(&s), [2]);
    // At the end: a new item, then Enter again ends the list on an empty paragraph.
    keys(&mut s, "<d-down><cr><cr><c-v>");
    assert_eq!(ids(&s), [2, 0, 1, 3], "{}", show(&s));
}

#[test]
fn e41_e42_select_all_delete_and_undo_with_ids() {
    let mut s = golden("ab ‖ cd▮", "<d-a>", "⟦ab ‖ cd▮⟧");
    keys(&mut s, "<bs>");
    assert_eq!(show(&s), "▮");
    assert_eq!(ids(&s), [0]);
    keys(&mut s, "<c-z>");
    assert_eq!(show(&s), "⟦ab ‖ cd▮⟧");
    assert_eq!(ids(&s), [0, 1]);
}

// ---------------------------------------------------------------------------------------
// Words and blocks by the mouse

#[test]
fn e44_e45_e46_word_and_block_selection() {
    let mut s = doc("The quick fox ‖ ▮Next");
    send(&mut s, [Msg::SelectWordAt { pos: 6 }]);
    assert_eq!(show(&s), "The ⟦quick▮⟧ fox ‖ Next");
    // A shift-click (a drag) on "fox" extends by whole words.
    send(&mut s, [Msg::Click { col: 11, row: 0, extend: true }]);
    assert_eq!(show(&s), "The ⟦quick fox▮⟧ ‖ Next");
    send(&mut s, [Msg::SelectBlock { id: MarkId(0) }]);
    assert_eq!(show(&s), "⟦The quick fox▮⟧ ‖ Next");
    let mut s = doc("- [ ] Pay ▮rent");
    send(&mut s, [Msg::SelectBlock { id: MarkId(0) }]);
    assert_eq!(show(&s), "- [ ] ⟦Pay rent▮⟧", "a block's content, never its marker");
}

// ---------------------------------------------------------------------------------------
// Atomic blocks: an image line is one unit

const IMG: &str = "![shot](files/k3m9q-shot.png)";

fn img(n: &str) -> String {
    n.replace("[img]", IMG)
}

#[track_caller]
fn golden_img(before: &str, script: &str, after: &str) -> State {
    golden(&img(before), script, &img(after))
}

#[test]
fn e47_a_click_on_an_image_selects_it() {
    let mut s = doc(&img("Above ‖ [img] ‖ Below▮"));
    send(&mut s, [Msg::Click { col: 4, row: 2, extend: false }]);
    assert_eq!(show(&s), img("Above ‖ ⟦[img]▮⟧ ‖ Below"));
}

#[test]
fn e49_e52_e53_e54_one_press_onto_an_image_and_one_off() {
    golden_img("Above▮ ‖ [img] ‖ Below", "<right>", "Above ‖ ⟦[img]▮⟧ ‖ Below");
    golden_img("Above ‖ ⟦[img]▮⟧ ‖ Below", "<right>", "Above ‖ [img] ‖ ▮Below");
    golden_img("Above ‖ [img] ‖ ▮Below", "<left>", "Above ‖ ⟦[img]▮⟧ ‖ Below");
    golden_img("Above ‖ ⟦[img]▮⟧ ‖ Below", "<left>", "Above▮ ‖ [img] ‖ Below");
}

#[test]
fn e50_typing_on_an_image_starts_a_block_after_it() {
    let s = golden_img("Above ‖ ⟦[img]▮⟧ ‖ Below", "x", "Above ‖ [img] ‖ x▮ ‖ Below");
    assert_eq!(ids(&s), [0, 1, 3, 2]);
}

#[test]
fn e55_down_onto_an_image_and_off_keeps_the_goal_column() {
    let mut s = golden_img("Ab▮ove ‖ [img] ‖ Below", "<down>", "Above ‖ ⟦[img]▮⟧ ‖ Below");
    keys(&mut s, "<down>");
    assert_eq!(show(&s), img("Above ‖ [img] ‖ Be▮low"));
}

#[test]
fn e56_e57_e58_backspace_and_delete_select_an_image_before_removing_it() {
    let mut s = golden_img("Above ‖ ⟦[img]▮⟧ ‖ Below", "<bs>", "Above▮ ‖ Below");
    assert_eq!(s.view.status.as_deref(), Some("removed k3m9q-shot.png"));
    keys(&mut s, "<c-z>");
    assert_eq!(show(&s), img("Above ‖ ⟦[img]▮⟧ ‖ Below"), "undo brings it back, selected");
    golden_img("Above ‖ [img] ‖ ▮Below", "<bs>", "Above ‖ ⟦[img]▮⟧ ‖ Below");
    golden_img("Above▮ ‖ [img] ‖ Below", "<del>", "Above ‖ ⟦[img]▮⟧ ‖ Below");
}

#[test]
fn e59_a_selection_across_an_image_deletes_it_with_the_rest() {
    let mut s = golden_img("Abo⟦ve ‖ [img] ‖ Bel▮⟧ow", "<bs>", "Abo▮ow");
    keys(&mut s, "<c-z>");
    assert_eq!(show(&s), img("Abo⟦ve ‖ [img] ‖ Bel▮⟧ow"));
}

#[test]
fn e60_shift_arrow_takes_the_whole_image() {
    golden_img("Above▮ ‖ [img] ‖ Below", "<s-right>", "Above⟦ ‖ [img]▮⟧ ‖ Below");
}

#[test]
fn e61_e62_copy_an_image_and_enter_after_it() {
    let mut s = doc(&img("Above ‖ ⟦[img]▮⟧ ‖ Below"));
    assert_eq!(clip(&keys(&mut s, "<c-c>")).as_deref(), Some(IMG));
    golden_img("Above ‖ ⟦[img]▮⟧ ‖ Below", "<cr>", "Above ‖ [img] ‖ ▮ ‖ Below");
}

// ---------------------------------------------------------------------------------------
// Kind changes: per line, and nothing else moves

#[test]
fn e70_the_task_cycle_inside_a_paragraph_splits_out_that_line() {
    let s = golden("First line⏎sec▮ond line⏎third line", "<c-t>", "First line ¦ - [ ] sec▮ond line ¦ third line");
    assert_eq!(ids(&s), [0, 1, 2]);
}

#[test]
fn e71_on_the_first_line_the_task_keeps_the_id() {
    let s = golden("fir▮st⏎second", "<c-t>", "- [ ] fir▮st ¦ second");
    assert_eq!(ids(&s), [0, 1]);
}

#[test]
fn e72_each_selected_line_becomes_its_own_task() {
    golden("a⏎⟦b⏎c▮⟧⏎d", "<c-t>", "a ¦ - [ ] ⟦b ¦ - [ ] c▮⟧ ¦ d");
}

#[test]
fn e73_a_list_item_with_a_soft_break_stays_one_item() {
    golden("- item one⏎more of it▮", "<c-t>", "- [ ] item one⏎more of it▮");
}

#[test]
fn e74_a_task_after_a_task_keeps_its_blank_row() {
    golden("- [ ] Buy milk ‖ Call ▮the bank", "<c-t>", "- [ ] Buy milk ‖ - [ ] Call ▮the bank");
}

#[test]
fn e75_back_to_text_adds_no_blank_row() {
    golden("- [ ] A ¦ - [ ] B▮", "<c-t><c-t>", "- [ ] A ¦ B▮");
}

#[test]
fn e76_back_to_text_never_joins_across_a_blank_row() {
    let s = golden("Para one ‖ - [ ] Ta▮sk", "<c-t><c-t>", "Para one ‖ Ta▮sk");
    assert_eq!(ids(&s), [0, 1]);
}

#[test]
fn e77_tab_keeps_the_blank_row() {
    golden("- [ ] A ‖ - [ ] B▮", "<tab>", "- [ ] A ‖   - [ ] B▮");
}

#[test]
fn e78_undo_puts_the_paragraph_back() {
    let mut s = golden("First line⏎sec▮ond line⏎third line", "<c-t><c-z>", "First line⏎sec▮ond line⏎third line");
    assert_eq!(ids(&s), [0]);
    keys(&mut s, "<c-y>");
    assert_eq!(show(&s), "First line ¦ - [ ] sec▮ond line ¦ third line");
    assert_eq!(ids(&s), [0, 1, 2], "redo brings back the same ids");
}

#[test]
fn e79_deleting_the_marker_adds_no_blank_row() {
    golden("- [ ] ▮A ¦ - [ ] B", "<bs><bs><bs><bs>", "▮A ¦ - [ ] B");
}

#[test]
fn e80_blank_rows_round_trip_through_markdown() {
    let s = doc("- [ ] A ‖ - [ ] B▮");
    let md = markdown::to_file(&s);
    assert_eq!(md, "- [ ] A\n\n- [ ] B\n");
    let back = markdown::load(&md, None, Viewport { width: 80, height: 24 }, OutlineConfig::default());
    let gaps: Vec<bool> = back.blocks().unwrap().blocks.iter().map(|b| b.gap).collect();
    assert_eq!(gaps, [false, true]);
}

#[test]
fn e81_back_to_text_joins_the_lines_it_split_from() {
    let s = golden("First line⏎sec▮ond line⏎third line", "<c-t><c-t><c-t>", "First line⏎sec▮ond line⏎third line");
    assert_eq!(ids(&s), [0]);
}

// ---------------------------------------------------------------------------------------
// Enter

#[test]
fn enter_in_a_list_item_starts_the_next_item() {
    golden("- one▮", "<cr>", "- one ¦ - ▮");
    golden("- o▮ne", "<cr>", "- o ¦ - ▮ne");
    golden("  * deep▮", "<cr>", "  * deep ¦   * ▮");
    golden("- [x] done▮", "<cr>", "- [x] done ¦ - [ ] ▮");
    golden("9. nine▮", "<cr>", "9. nine ¦ 10. ▮");
    golden("3) three▮", "<cr>", "3) three ¦ 4) ▮");
}

#[test]
fn enter_on_an_empty_item_ends_the_list() {
    // The paragraph it becomes takes a paragraph's blank row.
    golden("- one ¦   - ▮", "<cr>", "- one ‖ ▮");
    golden("- one ¦ 2. ▮", "<cr>", "- one ‖ ▮");
}

#[test]
fn enter_at_an_items_content_start_opens_an_item_above_and_keeps_the_id() {
    let s = golden("- a ¦ - ▮b", "<cr>", "- a ¦ -  ¦ - ▮b");
    assert_eq!(ids(&s), [0, 2, 1]);
}

#[test]
fn enter_in_a_paragraph_is_a_soft_break_and_twice_a_new_paragraph() {
    let s = golden("ab▮cd", "<cr>", "ab⏎▮cd");
    assert_eq!(ids(&s), [0]);
    let s = golden("text▮", "<cr><cr>", "text ‖ ▮");
    assert_eq!(ids(&s), [0, 1]);
    golden("text▮", "<cr><cr>more", "text ‖ more▮");
}

#[test]
fn enter_at_a_paragraphs_start_opens_one_above_and_keeps_the_id() {
    let s = golden("A ‖ ▮B", "<cr>", "A ‖  ‖ ▮B");
    assert_eq!(ids(&s), [0, 2, 1]);
}

#[test]
fn enter_at_the_start_of_a_later_line_splits_the_paragraph() {
    let s = golden("one⏎▮two", "<cr>", "one ‖ ▮two");
    assert_eq!(ids(&s), [0, 1]);
    golden("one▮⏎two", "<cr>", "one ‖ ▮ ‖ two");
}

#[test]
fn a_heading_or_quote_is_one_line() {
    golden("# Title▮", "<cr>", "# Title ‖ ▮");
    golden("> said▮", "<cr>x", "> said ‖ x▮");
    golden("# Title▮", "<s-cr>", "# Title⏎▮");
}

#[test]
fn enter_in_a_fence_stays_in_it() {
    golden("```⏎code▮⏎```", "<cr>- x", "```⏎code⏎- x▮⏎```");
}

#[test]
fn a_soft_break_continues_an_item() {
    golden("- one▮", "<s-cr>more", "- one⏎more▮");
    golden("- one▮", "<c-j>", "- one⏎▮");
}

#[test]
fn a_marker_typed_on_a_continuation_line_starts_a_block() {
    // It takes the blank row a list after a paragraph has.
    let s = golden("Para⏎▮", "- x", "Para ‖ - x▮");
    assert_eq!(ids(&s), [0, 1]);
}

#[test]
fn enter_over_a_selection_is_one_step() {
    let mut s = golden("- a⟦b ¦ - c▮⟧d", "<cr>", "- a ¦ - ▮d");
    keys(&mut s, "<c-z>");
    assert_eq!(show(&s), "- a⟦b ¦ - c▮⟧d");
}

// ---------------------------------------------------------------------------------------
// Backspace and Delete at block edges

#[test]
fn backspace_removes_a_marker_a_step_at_a_time() {
    golden("- [ ] ▮task", "<bs>", "- ▮task");
    golden("- ▮item", "<bs>", "▮item");
    golden("12. ▮item", "<bs>", "▮item");
    golden("## ▮Title", "<bs>", "▮Title");
    golden("  - [x] ▮deep", "<bs><bs>", "▮deep");
}

#[test]
fn backspace_at_a_paragraphs_start_joins_it_to_the_block_above() {
    let s = golden("one ‖ ▮two", "<bs>", "one⏎▮two");
    assert_eq!(ids(&s), [0]);
    let s = golden("- item ‖ ▮two", "<bs>", "- item▮two");
    assert_eq!(ids(&s), [0]);
    golden("▮first", "<bs>", "▮first");
}

#[test]
fn delete_at_a_blocks_end_joins_the_next() {
    let s = golden("- a▮ ¦ - b", "<del>", "- a▮b");
    assert_eq!(ids(&s), [0]);
    golden("one▮ ‖ two", "<del>", "one▮⏎two");
    golden("last▮", "<del>", "last▮");
    golden("- a▮ ¦ - b", "<c-k>", "- a▮b");
}

#[test]
fn joins_never_move_the_blocks_after_them() {
    // `c` keeps the tight row it had, whatever now comes before it.
    golden("Para ‖ - ▮b ¦ - c", "<bs><bs>", "Para⏎▮b ¦ - c");
}

// ---------------------------------------------------------------------------------------
// Tab and Shift-Tab

#[test]
fn tab_nests_one_level_below_the_block_above_at_most() {
    golden("- a ¦ - ▮b", "<tab>", "- a ¦   - ▮b");
    golden("- a ¦ - ▮b", "<tab><tab>", "- a ¦   - ▮b");
    golden("- ▮a", "<tab>", "- ▮a");
    golden("- a ¦   - b ¦     - ▮c", "<s-tab><s-tab>", "- a ¦   - b ¦ - ▮c");
}

#[test]
fn shift_tab_at_the_top_level_says_so() {
    let mut s = doc("- ▮a");
    keys(&mut s, "<s-tab>");
    assert_eq!(s.view.status.as_deref(), Some("already at the top level"));
}

// ---------------------------------------------------------------------------------------
// The task cycle and the task box

#[test]
fn the_task_cycle_goes_text_open_done_text() {
    let mut s = doc("Call ▮Sam");
    keys(&mut s, "<c-t>");
    assert_eq!(show(&s), "- [ ] Call ▮Sam");
    let fx = keys(&mut s, "<c-t>");
    assert_eq!(show(&s), "- [x] Call ▮Sam");
    assert!(fx.contains(&Effect::Completed { id: MarkId(0) }));
    keys(&mut s, "<c-t>");
    assert_eq!(show(&s), "Call ▮Sam");
    golden("- bul▮let", "<c-t>", "- [ ] bul▮let");
    golden("  - [/] doing▮", "<c-t>", "  - [x] doing▮");
}

#[test]
fn a_click_on_the_box_sets_the_status_and_never_makes_text() {
    let mut s = doc("- [ ] Pay▮ ¦ - [x] Done");
    let fx = send(&mut s, [Msg::SetStatus { id: MarkId(0), ch: 'x' }]);
    assert_eq!(show(&s), "- [x] Pay▮ ¦ - [x] Done");
    assert_eq!(fx, [Effect::Completed { id: MarkId(0) }]);
    send(&mut s, [Msg::SetStatus { id: MarkId(1), ch: ' ' }]);
    assert_eq!(show(&s), "- [x] Pay▮ ¦ - [ ] Done");
    send(&mut s, [Msg::SetStatus { id: MarkId(1), ch: 'q' }]);
    assert_eq!(s.view.status.as_deref(), Some("'q' isn't a task state"));
}

// ---------------------------------------------------------------------------------------
// Moving blocks

#[test]
fn alt_arrows_move_an_item_with_its_children() {
    let s = golden("- a ¦ - b▮ ¦   - b1 ¦ - c", "<a-up>", "- b▮ ¦   - b1 ¦ - a ¦ - c");
    assert_eq!(ids(&s), [1, 2, 0, 3]);
    let s = golden("- a ¦ - b▮ ¦   - b1 ¦ - c", "<a-down>", "- a ¦ - c ¦ - b▮ ¦   - b1");
    assert_eq!(ids(&s), [0, 3, 1, 2]);
}

#[test]
fn moving_past_the_end_of_a_list_is_refused() {
    let mut s = doc("- ▮a ¦ - b");
    keys(&mut s, "<a-up>");
    assert_eq!(show(&s), "- ▮a ¦ - b");
    assert_eq!(s.view.status.as_deref(), Some("first in its list · Shift-Tab to move out"));
    let mut s = doc("- a ¦   - ▮b");
    keys(&mut s, "<a-down>");
    assert_eq!(s.view.status.as_deref(), Some("last in its list · Shift-Tab to move out"));
}

#[test]
fn a_move_is_one_undo_step_and_keeps_blank_rows() {
    let mut s = golden("Intro ‖ - a ¦ - b▮", "<a-up>", "Intro ‖ - b▮ ¦ - a");
    keys(&mut s, "<c-z>");
    assert_eq!(show(&s), "Intro ‖ - a ¦ - b▮");
    assert_eq!(ids(&s), [0, 1, 2]);
}

// ---------------------------------------------------------------------------------------
// The caret never stops inside a marker

#[test]
fn the_caret_skips_markers() {
    golden("- [ ] one ¦ - ▮two", "<left>", "- [ ] one▮ ¦ - two");
    golden("- [ ] one▮ ¦ - two", "<right>", "- [ ] one ¦ - ▮two");
    golden("- [ ] one tw▮o", "<home>", "- [ ] ▮one two");
    golden("x ‖ - [ ] one▮", "<d-up>", "▮x ‖ - [ ] one");
    golden("▮x ‖ - [ ] one", "<down>", "x ‖ - [ ] ▮one");
}

#[test]
fn a_click_on_a_marker_or_a_blank_row_lands_at_the_content_start() {
    let mut s = doc("Top▮ ‖ - [ ] task");
    send(&mut s, [Msg::Click { col: 2, row: 2, extend: false }]);
    assert_eq!(show(&s), "Top ‖ - [ ] ▮task");
    let mut s = doc("Top▮ ‖ - [ ] task");
    send(&mut s, [Msg::Click { col: 5, row: 1, extend: false }]);
    assert_eq!(show(&s), "Top ‖ - [ ] ▮task", "the blank row belongs to the block below");
}

#[test]
fn ctrl_arrows_step_by_block() {
    golden("- a▮a ¦ - bb ‖ cc", "<c-down>", "- aa ¦ - ▮bb ‖ cc");
    golden("- aa ¦ - bb ‖ c▮c", "<c-up>", "- aa ¦ - bb ‖ ▮cc");
    golden("- aa ¦ - bb ‖ ▮cc", "<c-up>", "- aa ¦ - ▮bb ‖ cc");
}

// ---------------------------------------------------------------------------------------
// Blank rows are drawn, never stored

#[test]
fn a_blank_row_is_a_virtual_row() {
    let mut s = doc_wh("# Trip ‖ Booked.⏎Faces the river. ‖ - [ ] Pay▮ ¦   - ask Ana", 30, 8);
    assert_eq!(s.doc.text.to_string(), "# Trip\nBooked.\nFaces the river.\n- [ ] Pay\n  - ask Ana");
    s.view.config.status_bar = false;
    assert_eq!(
        view(&s).to_text(),
        "# Trip\n\nBooked.\nFaces the river.\n\n- [ ] Pay\n  - ask Ana\n\n"
    );
    assert_eq!(view(&s).cursor, Some((9, 5)));
}

// ---------------------------------------------------------------------------------------
// Host messages and effects

#[test]
fn insert_blocks_adds_host_blocks_as_one_step() {
    let mut s = doc("- a▮ ¦ - b");
    let blocks = vec![
        NewBlock { depth: 1, kind: Kind::Task, status: Some(' '), text: "sub".into(), gap: None, mark: Some(MarkId(40)) },
        NewBlock::para(IMG),
    ];
    send(&mut s, [Msg::InsertBlocks { after: Some(MarkId(0)), blocks }]);
    assert_eq!(show(&s), format!("- a▮ ¦   - [ ] sub ‖ {IMG} ‖ - b"));
    assert_eq!(ids(&s), [0, 40, 41, 1], "the given id where it's free, else a new one");
    keys(&mut s, "<c-z>");
    assert_eq!(show(&s), "- a▮ ¦ - b");
}

#[test]
fn block_left_restored_and_notices() {
    let mut s = doc("one▮ ‖ two");
    let fx = keys(&mut s, "<down>");
    assert_eq!(fx, [Effect::BlockLeft { from: Some(MarkId(0)), to: Some(MarkId(1)) }]);
    keys(&mut s, "x");
    let fx = keys(&mut s, "<c-z>");
    assert!(fx.contains(&Effect::Restored), "{fx:?}");
    s.view.config.status_bar = false;
    let fx = keys(&mut s, "<tab>");
    assert!(fx.contains(&Effect::Notice { text: "paragraphs don't nest".into() }), "{fx:?}");
}

#[test]
fn outline_messages_in_a_plain_document_only_say_so() {
    let mut s = common::state("- a▮");
    update(&mut s, Msg::Indent);
    assert_eq!(common::show(&s), "- a▮");
    assert_eq!(s.view.status.as_deref(), Some("only in outline documents"));
}

#[test]
fn save_writes_markdown_and_load_reads_it_back() {
    let md = "# Trip\n\n- [ ] Pay the deposit\n  - ask Ana\n    on two lines\n- [x] Flights\n\nNotes.\n";
    let mut s = markdown::load(md, Some("trip.md".into()), Viewport { width: 80, height: 24 }, OutlineConfig::default());
    assert!(!s.doc.dirty);
    keys(&mut s, "<d-down>!");
    let fx = keys(&mut s, "<c-s>");
    let Effect::WriteFile { text, .. } = &fx[0] else { panic!("{fx:?}") };
    assert_eq!(text, &md.replace("Notes.", "Notes.!"));
}

#[test]
fn an_outline_state_round_trips_through_json() {
    let mut s = doc("- [ ] a▮ ‖ b");
    keys(&mut s, "<cr>x<tab><c-t>");
    let back = State::from_json(&s.to_json()).unwrap();
    assert_eq!(back, s);
    assert_eq!(show(&back), show(&s));
    let mut back = back;
    keys(&mut back, "<c-z><c-z><c-z><c-z>");
    assert_eq!(show(&back), "- [ ] a▮ ‖ b");
}

/// The example in docs/caretline/outline.md.
#[test]
fn the_docs_example_runs() {
    let mut s = markdown::load("- [ ] Pay rent\n- Buy milk\n", None, Viewport { width: 40, height: 6 }, OutlineConfig::default());
    let fx = update(&mut s, Msg::TaskCycle);
    assert!(fx.contains(&Effect::Completed { id: MarkId(0) }));
    assert_eq!(markdown::to_file(&s), "- [x] Pay rent\n- Buy milk\n");
    let blocks = s.blocks().unwrap();
    assert_eq!(blocks.blocks[1].depth, 0);
}

/// The example in docs/caretline/api.md.
#[test]
fn the_api_example_runs() {
    let mut s = markdown::load("- [ ] Pay rent\n", None, Viewport { width: 40, height: 6 }, OutlineConfig::default());
    update(&mut s, Msg::Move { dir: caretline_next::Dir::Forward, by: caretline_next::By::LineEnd, extend: false });
    update(&mut s, Msg::InsertNewline);
    update(&mut s, Msg::InsertText { text: "Call Ana".into() });
    update(&mut s, Msg::Indent);
    assert_eq!(markdown::to_file(&s), "- [ ] Pay rent\n  - [ ] Call Ana\n");
}
