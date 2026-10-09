//! Flows: the Logseq model (writing.md §1, approved 2026-10-08). On pages and journal days a
//! typed line is a bullet note; Enter starts a new note, ⇧Enter (⌃J) breaks the line inside
//! one, Tab nests, ⇧Tab outdents; document mode (`space t D`, the palette) swaps Enter and ⇧Enter.
//! And the owner's demo of 2026-10-08, step by step: what it showed, what it must show now.
use super::typing::q4;
use super::*;

/// The saved note whose text is exactly `text`.
fn saved(f: &Flow, text: &str) -> Option<thc_core::model::Node> {
    let st = &f.s.app.vault.store;
    st.nodes_where("1=1", &[]).unwrap_or_default().into_iter().find(|n| st.render_text(&n.text) == text)
}

/// The text (or a page's title) of the saved note `text`'s parent.
fn parent_of(f: &Flow, text: &str) -> Option<String> {
    let n = saved(f, text)?;
    let p = f.s.app.vault.store.node(n.parent.as_deref()?).ok().flatten()?;
    Some(p.title.clone().unwrap_or(p.text))
}

/// The main document's rows as drawn (the text column, trailing spaces off).
fn doc_rows(f: &Flow) -> Vec<String> {
    let r = f.shot.doc_view.expect("a document on screen");
    (r.y..r.y + r.height).map(|y| f.shot.row(y, r.x, r.x + r.width).trim_end().to_string()).collect()
}

/// The drawn row holding `s`.
fn row_with(f: &Flow, s: &str) -> Option<String> {
    doc_rows(f).into_iter().find(|r| r.contains(s))
}

/// The saved structure is what was typed: separate notes, the right parents, no line breaks.
fn expect_structure(f: &mut Flow, want: &[(&str, &str)]) {
    f.save();
    for (text, parent) in want {
        let got = parent_of(f, text);
        if got.as_deref() != Some(*parent) {
            let all: Vec<String> = f.s.app.vault.store.nodes_where("1=1", &[]).unwrap_or_default().into_iter().map(|n| format!("{:?}", n.text)).collect();
            f.expect_fail(&format!("{text:?} is under {got:?}, want {parent:?}; notes: {all:?}"));
        }
    }
}

#[test]
fn a_typed_line_is_a_bullet_and_enter_starts_the_next() {
    let mut f = q4();
    f.named("typed lines are bullet notes; Enter starts the next").keys("<c-end>").type_text("First typed note");
    let r = row_with(&f, "First typed note").unwrap_or_default();
    assert!(r.contains("· First typed note"), "a bullet: {r:?}");
    f.keys("<cr>").type_text("Second typed note").keys("<cr>").type_text("Third typed note");
    f.expect_line("First typed note").expect_line("Second typed note").expect_line("Third typed note");
    expect_structure(&mut f, &[("First typed note", "Q4 Plan"), ("Second typed note", "Q4 Plan"), ("Third typed note", "Q4 Plan")]);
    f.done();
}

#[test]
fn shift_enter_breaks_the_line_inside_a_note() {
    let mut f = q4();
    f.named("⇧Enter: a line break inside the note, one bullet with a hanging line")
        .keys("<c-end>")
        .type_text("line one")
        .keys("<s-cr>")
        .type_text("line two")
        .keys("<c-j>")
        .type_text("line three")
        .expect_caret_line("line one\nline two\nline three");
    let one = row_with(&f, "line one").unwrap_or_default();
    let two = row_with(&f, "line two").unwrap_or_default();
    assert!(one.contains("· line one"), "the note's bullet: {one:?}");
    assert!(!two.contains('·'), "a hanging line, no bullet of its own: {two:?}");
    let col = |r: &str, s: &str| r[..r.find(s).unwrap()].chars().count();
    assert_eq!(col(&one, "line one"), col(&two, "line two"), "the lines hang under the first's text");
    f.expect_saved("line one\nline two\nline three");
    f.done();
}

#[test]
fn enter_splits_at_the_caret_and_tab_nests_the_new_note() {
    q4().named("Enter splits at the caret; Tab nests; ⇧Tab outdents")
        .click_caret(doc_at("Last line of the plan", 9))
        .keys("<cr>")
        .expect_line("Last line")
        .expect_caret_line("of the plan")
        .keys("<tab>")
        .expect_depth("of the plan", 1)
        .keys("<s-tab>")
        .expect_depth("of the plan", 0)
        .keys("<tab>")
        .expect_saved("of the plan")
        .expect("nested under the split note", |s| {
            let nodes = s.app.vault.store.nodes_where("1=1", &[]).unwrap_or_default();
            let child = nodes.iter().find(|n| n.text == "of the plan");
            let parent = nodes.iter().find(|n| n.text == "Last line");
            matches!((child, parent), (Some(c), Some(p)) if c.parent.as_deref() == Some(p.id.as_str()))
        })
        .done();
}

#[test]
fn enter_on_an_empty_note_comes_out_a_level_then_stays() {
    let mut f = q4();
    f.named("Enter on an empty nested note outdents; at the top it does nothing")
        .keys("<c-end>")
        .type_text("parent note")
        .keys("<cr><tab>")
        .type_text("child note")
        .keys("<cr>")
        .expect("a new empty note beside the child", |s| s.app.doc.as_ref().is_some_and(|d| d.caret_block().text.is_empty() && d.caret_block().depth == 1))
        .keys("<cr>")
        .expect("Enter on it: out a level", |s| s.app.doc.as_ref().is_some_and(|d| d.caret_block().text.is_empty() && d.caret_block().depth == 0));
    let before = doc_rows(&f);
    f.keys("<cr>");
    assert_eq!(doc_rows(&f), before, "Enter on an empty top-level note changes nothing");
    f.type_text("sibling note");
    expect_structure(&mut f, &[("child note", "parent note"), ("sibling note", "Q4 Plan")]);
    f.done();
}

#[test]
fn task_syntax_on_a_typed_line() {
    q4().named("`[ ] ` and ⌃T on a typed line make it a task; ⌃T back makes it a note")
        .keys("<c-end>")
        .type_text("[ ] buy stakes")
        .expect_caret_line("buy stakes")
        .expect_saved_status("buy stakes", "todo")
        .keys("<cr>")
        .type_text("next task")
        .expect_saved_status("next task", "todo")
        .keys("<cr><cr>")
        .type_text("plain again")
        .expect("an empty task's Enter ends the checklist", |s| {
            let d = s.app.doc.as_ref().unwrap();
            d.caret_block().kind() == thc_core::outline::Kind::Bullet
        })
        .keys("<c-t>")
        .expect_saved_status("plain again", "todo")
        .keys("<c-t><c-t>")
        .expect("back to a bullet note", |s| s.app.doc.as_ref().unwrap().caret_block().kind() == thc_core::outline::Kind::Bullet)
        .done();
}

#[test]
fn a_pasted_markdown_outline_keeps_its_shape() {
    let mut f = q4();
    f.named("a pasted Markdown outline: one note per item, nested as written")
        .keys("<c-end>")
        .paste("- Trip\n  - Book flights\n  - [ ] Pack\n- Home\n");
    expect_structure(&mut f, &[("Trip", "Q4 Plan"), ("Book flights", "Trip"), ("Pack", "Trip"), ("Home", "Q4 Plan")]);
    f.expect_saved_status("Pack", "todo").done();
}

#[test]
fn imported_paragraphs_keep_working() {
    // A Markdown paragraph pasted in (as an import makes them): its lines stay one note, Enter
    // splits it into notes, ⇧Enter still breaks its lines, and a line typed after it is a bullet.
    let mut f = q4();
    f.named("paragraphs from Markdown keep working")
        .keys("<c-end>")
        .paste("Prose first line\nprose second line\n\nAnother paragraph\n")
        .expect_line("Prose first line prose second line")
        .expect_line("Another paragraph")
        .click_caret(doc_at("Another paragraph", 7))
        .keys("<cr>")
        .expect_line("Another")
        .expect_caret_line("paragraph")
        .expect("both halves are paragraphs", |s| {
            let d = s.app.doc.as_ref().unwrap();
            d.blocks().iter().filter(|l| l.text == "Another" || l.text == "paragraph").all(|l| l.kind() == thc_core::outline::Kind::Para)
        })
        .keys("<end><cr>")
        .type_text("typed after")
        .expect("a typed line after a paragraph is a bullet", |s| s.app.doc.as_ref().unwrap().caret_block().kind() == thc_core::outline::Kind::Bullet)
        .expect_saved("typed after");
    f.done();
}

#[test]
fn document_mode_swaps_enter_and_shift_enter() {
    let mut f = q4();
    f.named("document mode: Enter breaks the line, ⇧Enter starts a note")
        .keys("<c-end>")
        .keys("<m-:>document mode<cr>")
        .expect("document mode on", |s| s.app.ui.document_mode)
        .expect_screen("document mode")
        .type_text("A long-form paragraph")
        .keys("<cr>")
        .type_text("its second line")
        .expect_caret_line("A long-form paragraph\nits second line")
        .keys("<s-cr>")
        .type_text("a new note")
        .expect_caret_line("a new note")
        .keys("<m-:>document mode<cr>")
        .expect("document mode off", |s| !s.app.ui.document_mode)
        .expect_no_screen("document mode ")
        .keys("<cr>")
        .type_text("outline again");
    expect_structure(&mut f, &[("A long-form paragraph\nits second line", "Q4 Plan"), ("a new note", "Q4 Plan"), ("outline again", "Q4 Plan")]);
    f.done();
}

#[test]
fn document_mode_from_the_leader_and_the_palette() {
    let mut f = q4();
    f.named("document mode from space t D (navigating) and the palette")
        .keys("<esc>")
        .expect("navigating, not writing", |s| !s.app.main.write || s.app.doc.is_none())
        .keys(" tD")
        .expect("on from the leader", |s| s.app.ui.document_mode)
        .keys("  document mode<cr>")
        .expect("off from the palette", |s| !s.app.ui.document_mode);
    f.done();
}

#[test]
fn an_agent_cannot_flip_document_mode() {
    let mut f = q4();
    f.named("an agent's patch can't turn document mode on")
        .keys("<c-end>");
    let r = f.s.apply(Msg::Patch { patch: serde_json::json!({ "document_mode": true }), actor: Some("claude".into()) });
    assert!(r.is_err_and(|e| e.contains("document_mode")), "refused");
    assert!(!f.s.app.ui.document_mode, "unchanged");
    f.keys("<m-:>document mode<cr>");
    assert!(f.s.app.ui.document_mode);
    f.msg("an agent's patch leaves it alone", Motion::Any, Msg::Patch { patch: serde_json::json!({ "show_detail": true }), actor: Some("claude".into()) });
    assert!(f.s.app.ui.document_mode, "kept");
    f.done();
}

#[test]
fn three_lines_typed_with_enter_save_and_reopen_with_nothing_shifting() {
    // The demo: three lines typed with Enter were saved as one note, and after the save blank
    // rows appeared between them and the page shifted. Now: three notes, and the rows typed are
    // the rows saved and the rows reopened.
    let mut f = q4();
    f.named("three lines with Enter: saved, reopened, nothing shifts")
        .keys("<c-end>")
        .type_text("Alpha note")
        .keys("<cr>")
        .type_text("Beta note")
        .keys("<cr>")
        .type_text("Gamma note");
    let typed = doc_rows(&f);
    f.save();
    assert_eq!(doc_rows(&f), typed, "the save moved nothing");
    f.idle();
    assert_eq!(doc_rows(&f), typed, "nor did the idle time after it");
    expect_structure(&mut f, &[("Alpha note", "Q4 Plan"), ("Beta note", "Q4 Plan"), ("Gamma note", "Q4 Plan")]);
    let at = |rows: &[String], s: &str| rows.iter().position(|r| r.contains(s));
    let (a, b, c) = (at(&typed, "Alpha note").unwrap(), at(&typed, "Beta note").unwrap(), at(&typed, "Gamma note").unwrap());
    assert_eq!((b - a, c - b), (1, 1), "no blank rows between the notes: {typed:#?}");
    // Reopen: leave, come back.
    f.keys("<esc>").keys("<c-o>Q4 Plan<cr>");
    let back = doc_rows(&f);
    assert_eq!(at(&back, "Alpha note"), Some(a), "reopened where it was typed: {back:#?}");
    assert_eq!((at(&back, "Beta note"), at(&back, "Gamma note")), (Some(b), Some(c)));
    f.done();
}

#[test]
fn linked_from_shows_a_multi_line_note_with_a_separator() {
    // The demo: "…with TabAnother…": a multi-line note's lines glued in the `linked from` row.
    let mut f = flow("a multi-line note in linked from");
    f.keys("<c-o>Garden<cr><c-end>")
        .type_text("A thought for [[Q4 Pl")
        .keys("<cr>")
        .type_text(" with Tab")
        .keys("<s-cr>")
        .type_text("Another line")
        .save()
        .keys("<esc><c-o>Q4 Plan<cr>")
        .expect_screen("with Tab ⏎ Another line")
        .expect_no_screen("TabAnother");
    f.done();
}

// ---- the owner's demo, 2026-10-08 ---------------------------------------------------------------
// Its script, key for key (scratchpad morning_demo.py): jump, nest, link, aside, panel-edit. Health
// there is Garden here.

/// The demo up to the sidebar: ⌃O to a page and type, Enter, Tab (from the line's start), Enter,
/// ⇧Tab, `[[Gar` Enter (the link popup), every step held to the invariants (no shifting), and
/// the vault's structure checked: one note per line typed, nested as typed.
fn demo_main() -> Flow {
    let mut f = flow("the 2026-10-08 demo");
    f.keys("<c-o>Q4 Plan<cr>")
        .type_text("Kickoff notes: I jumped here with Ctrl-O and started typing straight away.")
        .keys("<cr>")
        .type_text("A thought tucked under it with Tab")
        .keys("<home>")
        .keys("<tab>")
        .keys("<end>")
        .keys("<cr>")
        .type_text("Another nested line, then Shift-Tab back out")
        .keys("<s-tab>")
        .keys("<cr>")
        .type_text("Related: [[Gar")
        .keys("<cr>")
        .type_text(" for the beds")
        .expect_caret_line("Related: [[Garden]] for the beds");
    let kickoff = "Kickoff notes: I jumped here with Ctrl-O and started typing straight away.";
    expect_structure(
        &mut f,
        &[
            (kickoff, "Q4 Plan"),
            ("A thought tucked under it with Tab", kickoff),
            ("Another nested line, then Shift-Tab back out", "Q4 Plan"),
            ("Related: [[Garden]] for the beds", "Q4 Plan"),
        ],
    );
    f
}

/// The demo's `aside`: the caret back into the link, ⌥O.
fn demo_aside() -> Flow {
    let mut f = demo_main();
    for _ in 0.." for the beds".len() + 3 {
        f.moves("<left>");
    }
    f.keys("<m-o>").expect_panels(1).expect_page("Q4 Plan");
    f
}

#[test]
fn the_demo_in_the_main_view() {
    let mut f = demo_aside();
    // ⌃End and type, in the main view: a fresh note at the end, never glued to the last one.
    f.keys("<c-end>").type_text("Typed at the end").expect_caret_line("Typed at the end");
    expect_structure(&mut f, &[("Typed at the end", "Q4 Plan"), ("Related: [[Garden]] for the beds", "Q4 Plan")]);
    f.done();
}

#[test]
fn the_demo_ctrl_end_in_a_panel_goes_to_a_fresh_line() {
    // ⌃End in a panel appended to the last note's text ("Refill prescriptionEdited…"). ⌃End
    // is the shared editor's (doc_keys): the fresh line after the notes, in a panel as in main.
    let mut f = demo_aside();
    f.keys("<m-s>").keys("<c-end>").type_text("Edited from the sidebar panel").save();
    let texts = |s: &Session| {
        let st = &s.app.vault.store;
        st.nodes_where("1=1", &[]).unwrap_or_default().iter().map(|n| st.render_text(&n.text)).collect::<Vec<_>>()
    };
    f.expect("a note of its own", |s| texts(s).iter().any(|t| t == "Edited from the sidebar panel"))
        .expect("the last note's text unchanged", |s| texts(s).iter().any(|t| t == "See [[Q4 Plan]] for the budget"))
        .done();
}

#[test]
#[ignore = "3j9d3"]
fn the_demo_esc_from_a_panel_returns_to_the_main_document() {
    // Superseded by pane unification: Esc from a panel left the main view on the panel's page
    // (Health in the demo) with focus `list`, and the next keys typed into it.
    let mut f = demo_aside();
    f.keys("<m-s>")
        .expect_focus(Focus::Sidebar)
        .keys("<c-end>")
        .type_text("Edited from the sidebar panel")
        .keys("<esc>")
        .expect_page("Q4 Plan")
        .expect("writing in the main document", |s| s.app.main.write && s.app.ui.focus != Focus::Sidebar)
        .type_text("typed after Esc")
        .expect("typed into Q4 Plan, not Garden", |s| s.app.doc.as_ref().unwrap().caret_block().text.contains("typed after Esc"))
        .done();
}

#[test]
#[ignore = "1wj8f"]
fn the_demo_panel_never_scrolls_by_itself() {
    // Superseded by pane unification: a short panel scrolled a row by itself after opening,
    // focusing or editing, hiding its first line.
    let mut f = demo_aside();
    f.expect_at(text("Tomatoes need staking").in_side());
    f.keys("<m-s>").expect_at(text("Tomatoes need staking").in_side());
    f.keys("<c-end>").type_text("one more").expect_at(text("Tomatoes need staking").in_side());
    f.done();
}
