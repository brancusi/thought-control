//! The examples in docs/caretline run: structure.md (decorations), markdown.md (a list edit),
//! embedding.md (a host command, and the case study's cycle built on tags).

use caretline::outline::markdown;
use caretline::outline::Kind;
use caretline::view::{hit, Hit};
use caretline::{update, view, By, Ctx, Deco, Decoration, Dir, Edit, Effect, Host, Msg, OutlineConfig, OutlineLayout, State, Viewport};
use serde_json::{json, Value};

/// structure.md#decorations
#[test]
fn the_decoration_example_runs() {
    let mut s = markdown::load("- Pay rent\n- Buy milk\n", None, Viewport { width: 40, height: 4 }, OutlineConfig::default());
    s.view.layout = Some(OutlineLayout::default());
    s.doc.set_host(Host::new().decorator(|_, _| Decoration {
        hang: Some(Deco { text: "◆".into(), role: "accent".into(), id: Some("diamond".into()) }),
        gutter: None,
    }));
    let f = view(&s);
    assert_eq!(f.to_text().lines().next(), Some("  ◆   Pay rent"));
    assert_eq!(hit(&s.doc, &s.view, 2, 0), Hit::Hang { block: s.doc.marks.as_slice()[0].id, deco: Some("diamond".into()) });
}

/// markdown.md#markdown-in-and-out
#[test]
fn the_markdown_example_runs() {
    let mut s = markdown::load("- Pay rent\n", None, Viewport { width: 40, height: 6 }, OutlineConfig::default());
    update(&mut s, Msg::Move { dir: Dir::Forward, by: By::LineEnd, extend: false });
    update(&mut s, Msg::InsertNewline);
    update(&mut s, Msg::InsertText { text: "Call Ana".into() });
    update(&mut s, Msg::Indent);
    assert_eq!(markdown::to_file(&s), "- Pay rent\n  - Call Ana\n");
}

/// embedding.md#extending-the-engine
#[test]
fn the_host_command_example_runs() {
    let host = Host::new().command("shout", |ctx, _args| {
        let line = ctx.text().char_to_line(ctx.caret());
        let (a, b) = (ctx.text().line_to_char(line), ctx.text().line_to_char(line + 1) - 1);
        Ok(Edit { changes: vec![(a, b, ctx.text().slice(a..b).to_string().to_uppercase())], ..Edit::default() })
    });
    let mut state = State::new("hello\nworld\n", None, Viewport { width: 40, height: 6 });
    state.doc.set_host(host);
    update(&mut state, Msg::Command { name: "shout".into(), args: Value::Null });
    assert_eq!(state.doc.text.to_string(), "HELLO\nworld\n");
}

/// embedding.md#case-study-tasks-in-thc, step 2.
fn task_cycle(ctx: &Ctx, _: &Value) -> Result<Edit, String> {
    let o = ctx.blocks().ok_or("only in outline documents")?;
    let b = o.block_at(ctx.text(), ctx.caret());
    let at = b.start + b.indent;
    let (changes, done) = match b.tag {
        Some('x') => (vec![(b.start, b.content_start(), String::new())], false),
        Some(_) => (vec![(at + 3, at + 4, "x".to_string())], true),
        None if b.kind == Kind::Para => (vec![(at, at, "- [ ] ".to_string())], false),
        None => (vec![(at, b.content_start(), "- [ ] ".to_string())], false),
    };
    Ok(Edit {
        selection: Some(ctx.mapped_selection(&changes)),
        changes,
        keep_gaps: true,
        effects: if done { vec![("completed".into(), json!({ "id": b.id.0 }))] } else { vec![] },
        ..Edit::default()
    })
}

#[test]
fn the_case_study_builds_tasks_on_tags() {
    let config = OutlineConfig { tags: " x/w-".into(), new_tag: Some(' '), ..OutlineConfig::default() };
    let mut s = markdown::load("Pay rent\n", None, Viewport { width: 40, height: 6 }, config);
    s.doc.set_host(Host::new().command("task_cycle", task_cycle));
    let cycle = || Msg::Command { name: "task_cycle".into(), args: Value::Null };
    update(&mut s, cycle());
    assert_eq!(s.doc.text.to_string(), "- [ ] Pay rent");
    let fx = update(&mut s, cycle());
    assert_eq!(s.doc.text.to_string(), "- [x] Pay rent");
    assert_eq!(fx, vec![Effect::Host { name: "completed".into(), data: json!({ "id": 0 }) }]);
    update(&mut s, cycle());
    assert_eq!(s.doc.text.to_string(), "Pay rent");
    update(&mut s, Msg::Undo);
    assert_eq!(s.doc.text.to_string(), "- [x] Pay rent", "one undo step each");
}

/// keys.md#the-command-catalog
#[test]
fn the_keys_example_runs() {
    use caretline::commands::{command, command_for};
    use caretline::{command_msg, commands, default_keymap, Key, KeyCode, Mods};
    assert!(commands().iter().any(|c| c.id == "history.undo"));
    assert_eq!(command("move.word_right").unwrap().name, "Word right");
    assert_eq!(command_msg("history.undo", None), Some(Msg::Undo));
    let ctrl_z = Key { code: KeyCode::Char('z'), mods: Mods { ctrl: true, ..Mods::default() } };
    assert_eq!(command_for(false, &ctrl_z), Some("history.undo"));
    assert!(default_keymap(true).iter().any(|b| b.keys == "<tab>" && b.command == "structure.indent"));
}

/// api.md#block-marks
#[test]
fn the_api_marks_example_runs() {
    use caretline::MarkAttrs;
    let mut s = State::new("Groceries\nmilk\n", None, Viewport { width: 40, height: 5 });
    let list = s.doc.marks.mint(0);
    let milk = s.doc.marks.mint(s.doc.text.line_to_char(1));
    s.doc.marks.set_attrs(milk, MarkAttrs { gap: Some(false), data: Some(json!({ "row": 7 })) });
    update(&mut s, Msg::InsertText { text: "Weekly ".into() });
    assert_eq!(s.doc.marks.pos(list), Some(0));
    update(&mut s, Msg::Move { dir: Dir::Forward, by: By::DocEnd, extend: false });
    update(&mut s, Msg::Undo);
    assert_eq!(s.doc.marks.pos(milk), Some(s.doc.text.line_to_char(1)));
    assert_eq!(s.doc.marks.get(milk).unwrap().attrs.data, Some(json!({ "row": 7 })));
}
