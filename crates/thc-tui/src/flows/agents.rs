//! Flows: an agent beside you. `thc add` and `thc ui patch` while you type: your caret stays
//! where it was and nothing on screen shifts under it.
use super::typing::q4;
use super::*;

#[test]
fn an_agent_adds_to_today_while_you_type_there() {
    let mut f = flow("thc add to today while typing in today");
    f.keys("T<c-end><cr>").type_text("my own words");
    f.agent_add("from the agent");
    f.expect_caret_after("my own words").expect_screen("from the agent").type_text(" go on").expect_caret_line("my own words go on");
    f.expect_saved("my own words go on").done();
}

#[test]
fn an_agent_adds_above_the_caret_and_nothing_shifts() {
    // The agent's line lands at the end of the day; typing mid-day, rows above stay.
    let mut f = flow("thc add while typing mid-day");
    f.keys("T").click_caret(doc_at("Morning notes", 13)).type_text(" and coffee");
    f.agent_add("agent line one").agent_add("agent line two");
    f.type_text(" and toast").expect_caret_line("Morning notes and coffee and toast").done();
}

#[test]
fn an_agent_adds_to_today_while_you_type_on_a_page() {
    q4().named("thc add while typing on a page")
        .click_caret(doc_at("Last line of the plan", 21))
        .type_text(" more")
        .agent_add("agent elsewhere")
        .type_text(" text")
        .expect_caret_line("Last line of the plan more text")
        .done();
}

#[test]
fn an_agent_patches_the_view_while_you_type() {
    q4().named("thc ui patch while typing")
        .click_caret(doc_at("Last line of the plan", 21))
        .type_text(" typing")
        .ui_patch(serde_json::json!({"show_detail": true}), "claude")
        .expect_page("Q4 Plan")
        .type_text(" on")
        .expect_caret_line("Last line of the plan typing on")
        .done();
}

#[test]
fn an_agent_moves_you_and_back_goes_home() {
    q4().named("an agent switches the view; ⌘[ comes back")
        .click_caret(doc_at("Grow the newsletter", 5))
        .ui_patch(serde_json::json!({"view": "tasks"}), "claude")
        .expect_view(View::Tasks)
        .expect_screen("claude changed your view")
        .keys("<d-[>")
        .expect_page("Q4 Plan")
        .expect_caret_before("the newsletter")
        .done();
}

#[test]
fn another_device_edits_a_line_above_you() {
    q4().named("a remote edit above the caret")
        .click_caret(doc_at("Last line of the plan", 21))
        .type_text(" here")
        .remote_edit("Grow the newsletter", "Grow the newsletter weekly")
        .expect_screen("weekly")
        .type_text(" still")
        .expect_caret_line("Last line of the plan here still")
        .done();
}

#[test]
fn another_device_edits_the_line_you_are_typing() {
    q4().named("a remote edit of the caret's line")
        .click_caret(doc_at("Last line of the plan", 21))
        .type_text(" mine")
        .remote_edit("Last line of the plan", "Last line of the plan (theirs)")
        .type_text(" more")
        .expect_caret_after("mine more")
        .done();
}
