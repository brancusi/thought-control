//! The host's extension points: commands, input rules, mark payloads, decorations and tags.
//! The test host is made up (it shouts and draws diamonds): nothing here means anything to the
//! engine.

use caretline::helix::Selection;
use caretline::outline::markdown;
use caretline::protocol::{Format, RenderedFrame};
use caretline::trace::{replay_trace_with, TraceLine};
use caretline::view::{hit, Hit};
use caretline::{update, view, Deco, Decoration, Edit, Effect, ExtChange, Host, MarkAttrs, MarkId, MarkOp, Msg, OutlineConfig, OutlineLayout, Session, State, Viewport};
use serde_json::json;

fn vp() -> Viewport {
    Viewport { width: 40, height: 8 }
}

/// Uppercases the caret's line; says how many chars it changed; emits `shouted`.
fn shout(ctx: &caretline::Ctx, args: &serde_json::Value) -> Result<Edit, String> {
    let text = ctx.text();
    let line = text.char_to_line(ctx.caret());
    let start = text.line_to_char(line);
    let end = start + text.line(line).chars().take_while(|c| *c != '\n').count();
    let old: String = text.slice(start..end).to_string();
    if old.is_empty() {
        return Err("nothing to shout".into());
    }
    let new = old.to_uppercase();
    let times = args.get("times").and_then(|t| t.as_u64()).unwrap_or(1);
    Ok(Edit {
        changes: vec![(start, end, new.clone())],
        status: Some(format!("shouted {}", new.chars().count())),
        effects: vec![("shouted".into(), json!({ "line": line, "times": times }))],
        ..Edit::default()
    })
}

fn host() -> Host {
    Host::new()
        .command("test.shout", shout)
        .command("test.pin", |ctx, args| {
            // Gives the caret's line a mark carrying `args` as its payload.
            let text = ctx.text();
            let pos = text.line_to_char(text.char_to_line(ctx.caret()));
            let op = match ctx.doc.marks.at(pos) {
                Some(id) => MarkOp::SetData { id, data: Some(args.clone()) },
                None => MarkOp::Mint { pos, attrs: MarkAttrs { gap: None, data: Some(args.clone()) } },
            };
            Ok(Edit { marks: vec![op], ..Edit::default() })
        })
        // `->` typed becomes an arrow.
        .input_rule("test.arrow", |ctx, msg| {
            let Msg::InsertText { text } = msg else { return None };
            let p = ctx.caret();
            (text == ">" && ctx.selection().primary().is_empty() && p > 0 && ctx.text().char(p - 1) == '-')
                .then(|| Edit { changes: vec![(p - 1, p, "→".into())], selection: Some(Selection::point(p)), ..Edit::default() })
        })
        .decorator(|_, b| Decoration {
            hang: Some(Deco { text: "◆".into(), role: "test.diamond".into(), id: Some("diamond".into()) }),
            gutter: (b.depth > 0).then(|| Deco { text: "›".into(), role: "test.deep".into(), id: None }),
        })
}

fn state(text: &str) -> State {
    let mut s = State::new(text, None, vp());
    s.doc.set_host(host());
    s
}

#[test]
fn a_command_is_one_undo_step_with_its_status_and_effects() {
    let mut s = state("hello\nworld");
    s.view.selection = Selection::point(8);
    let fx = update(&mut s, Msg::Command { name: "test.shout".into(), args: json!({ "times": 2 }) });
    assert_eq!(s.doc.text.to_string(), "hello\nWORLD");
    assert_eq!(s.view.status.as_deref(), Some("shouted 5"));
    assert_eq!(fx, vec![Effect::Host { name: "shouted".into(), data: json!({ "line": 1, "times": 2 }) }]);
    assert_eq!(s.caret(), 11, "the selection maps through the change (past a replaced range)");
    update(&mut s, Msg::Undo);
    assert_eq!(s.doc.text.to_string(), "hello\nworld");
    update(&mut s, Msg::Redo);
    assert_eq!(s.doc.text.to_string(), "hello\nWORLD");
}

#[test]
fn an_unknown_command_or_a_refusal_changes_nothing_and_says_so() {
    let mut s = state("\nx");
    let rev = s.doc.rev;
    update(&mut s, Msg::Command { name: "test.nope".into(), args: json!(null) });
    assert_eq!(s.view.status.as_deref(), Some("no command 'test.nope'"));
    update(&mut s, Msg::Command { name: "test.shout".into(), args: json!(null) });
    assert_eq!(s.view.status.as_deref(), Some("nothing to shout"));
    assert_eq!(s.doc.rev, rev);
    // Without a host, every command is unknown.
    let mut bare = State::new("abc", None, vp());
    update(&mut bare, Msg::Command { name: "test.shout".into(), args: json!(null) });
    assert_eq!(bare.doc.text.to_string(), "abc");
}

#[test]
fn a_read_only_view_refuses_a_command() {
    let mut s = state("abc");
    s.view.read_only = true;
    assert_eq!(update(&mut s, Msg::Command { name: "test.shout".into(), args: json!(null) }), vec![Effect::Refused]);
    assert_eq!(s.doc.text.to_string(), "abc");
}

#[test]
fn commands_record_in_traces_and_replay_with_the_same_host() {
    let mut session = Session::new(state("one\ntwo"));
    session.apply(Msg::Move { dir: caretline::Dir::Forward, by: caretline::By::Line, extend: false });
    session.apply(Msg::Command { name: "test.shout".into(), args: json!(null) });
    session.apply(Msg::InsertText { text: "!".into() });
    let trace = session.trace_jsonl();
    assert!(trace.lines().any(|l| l.contains("\"command\"")), "{trace}");
    let (replayed, _, n) = replay_trace_with(&trace, &host()).unwrap();
    assert_eq!(n, 3);
    assert_eq!(replayed.doc.text.to_string(), session.state().doc.text.to_string());
    assert_eq!(replayed, *session.state());
    // Without the host the command is unknown: the replay differs.
    let (bare, _, _) = caretline::trace::replay_trace_views(&trace).unwrap();
    assert_ne!(bare.doc.text.to_string(), session.state().doc.text.to_string());
}

#[test]
fn hello_names_the_commands_and_state_set_keeps_the_host() {
    let mut session = Session::new(state("abc"));
    let r = session.handle(r#"{"id":1,"op":"hello"}"#, None).response;
    assert!(r.contains(r#""commands":["test.pin","test.shout"]"#), "{r}");
    let r = session.handle(r#"{"id":2,"op":"state.set","state":{"text":"quiet"}}"#, None).response;
    assert!(!r.contains("error"), "{r}");
    session.apply(Msg::Command { name: "test.shout".into(), args: json!(null) });
    assert_eq!(session.state().doc.text.to_string(), "QUIET");
}

#[test]
fn an_input_rule_takes_the_key_before_the_engine() {
    let mut s = state("");
    for c in ["a", "-", ">", "b"] {
        update(&mut s, Msg::InsertText { text: c.into() });
    }
    assert_eq!(s.doc.text.to_string(), "a→b");
    update(&mut s, Msg::Undo);
    update(&mut s, Msg::Undo);
    assert!(!s.doc.text.to_string().contains('→'), "{:?}", s.doc.text.to_string());
}

#[test]
fn a_payload_rides_with_its_mark_through_cut_paste_undo_and_json() {
    let mut s = state("alpha\nbeta\ngamma");
    s.view.selection = Selection::point(7);
    update(&mut s, Msg::Command { name: "test.pin".into(), args: json!({ "row": 42 }) });
    let id = s.doc.marks.at(6).expect("a mark on beta");
    assert_eq!(s.doc.marks.get(id).unwrap().attrs.data, Some(json!({ "row": 42 })));
    // Cut the line and paste it at the end: the mark and its payload come back.
    s.view.selection = Selection::single(6, 11);
    update(&mut s, Msg::Cut);
    assert!(!s.doc.marks.contains(id));
    update(&mut s, Msg::Move { dir: caretline::Dir::Forward, by: caretline::By::DocEnd, extend: false });
    update(&mut s, Msg::InsertNewline);
    update(&mut s, Msg::Paste { text: None });
    let m = s.doc.marks.get(id).expect("pasted back with its id");
    assert_eq!(m.attrs.data, Some(json!({ "row": 42 })));
    // Undo everything: still there, payload and all.
    for _ in 0..3 {
        update(&mut s, Msg::Undo);
    }
    assert_eq!(s.doc.marks.get(id).map(|m| m.attrs.data.clone()), Some(Some(json!({ "row": 42 }))));
    // JSON keeps it.
    let back = State::from_json(&s.to_json()).unwrap();
    assert_eq!(back.doc.marks, s.doc.marks);
    // A change from elsewhere replaces it, outside the history.
    update(&mut s, Msg::External { changes: vec![ExtChange::SetData { id, data: Some(json!("theirs")) }] });
    assert_eq!(s.doc.marks.get(id).unwrap().attrs.data, Some(json!("theirs")));
    update(&mut s, Msg::External { changes: vec![ExtChange::SetData { id: MarkId(999), data: None }] });
}

#[test]
fn a_payload_change_by_command_is_undone() {
    let mut s = state("alpha\nbeta");
    s.view.selection = Selection::point(7);
    update(&mut s, Msg::Command { name: "test.pin".into(), args: json!(1) });
    update(&mut s, Msg::Command { name: "test.pin".into(), args: json!(2) });
    let id = s.doc.marks.at(6).unwrap();
    assert_eq!(s.doc.marks.get(id).unwrap().attrs.data, Some(json!(2)));
    update(&mut s, Msg::Undo);
    assert_eq!(s.doc.marks.get(id).unwrap().attrs.data, Some(json!(1)));
}

fn laid_out(md: &str) -> State {
    let mut s = markdown::load(md, None, Viewport { width: 40, height: 6 }, OutlineConfig::default());
    s.view.config.status_bar = false;
    s.view.layout = Some(OutlineLayout::default());
    s.doc.set_host(host());
    s
}

#[test]
fn decorations_draw_in_the_hang_and_gutter_by_role_and_hit_reports_them() {
    let s = laid_out("- one\n  - two\n");
    let f = view(&s);
    let text = f.to_text();
    assert!(text.contains("◆"), "{text}");
    assert!(text.lines().nth(1).unwrap().starts_with('›'), "{text}");
    let (x, y) = (0..f.width).flat_map(|x| (0..f.height).map(move |y| (x, y))).find(|&(x, y)| &*f.cell(x, y).symbol == "◆").unwrap();
    assert_eq!(f.role_name(f.cell(x, y).role), "test.diamond");
    let cells = serde_json::to_string(&RenderedFrame::new(&f, Format::Cells)).unwrap();
    assert!(cells.contains("test.diamond") && cells.contains("test.deep"), "{cells}");
    let ids: Vec<MarkId> = s.blocks().unwrap().blocks.iter().map(|b| b.id).collect();
    assert_eq!(hit(&s.doc, &s.view, x, y), Hit::Hang { block: ids[0], deco: Some("diamond".into()) });
    assert_eq!(hit(&s.doc, &s.view, 0, 1), Hit::Gutter { block: ids[1], deco: None });
}

#[test]
fn without_a_decorator_plain_glyphs_are_the_layouts_choice() {
    let mut s = laid_out("- one\n");
    s.doc.set_host(Host::new());
    assert!(!view(&s).to_text().contains('•'));
    s.view.layout = Some(OutlineLayout { hang_glyphs: true, ..OutlineLayout::default() });
    assert!(view(&s).to_text().contains('•'));
}

fn tagged() -> OutlineConfig {
    OutlineConfig { tags: "ab".into(), new_tag: Some('a'), ..OutlineConfig::default() }
}

#[test]
fn a_tag_is_part_of_the_marker_only_when_the_config_has_it() {
    let cfg = tagged();
    let s = markdown::load("- [a] one\n- [z] two\n", None, vp(), cfg);
    let o = s.blocks().unwrap();
    assert_eq!((o.blocks[0].tag, o.blocks[0].prefix_len), (Some('a'), 6));
    assert_eq!((o.blocks[1].tag, o.blocks[1].prefix_len), (None, 2), "z isn't a tag: text");
}

#[test]
fn enter_continues_with_the_new_tag_and_backspace_takes_the_tag_first() {
    let mut s = markdown::load("- [b] one", None, vp(), tagged());
    update(&mut s, Msg::Move { dir: caretline::Dir::Forward, by: caretline::By::DocEnd, extend: false });
    update(&mut s, Msg::InsertNewline);
    assert_eq!(s.doc.text.to_string(), "- [b] one\n- [a] ");
    update(&mut s, Msg::InsertText { text: "two".into() });
    update(&mut s, Msg::Move { dir: caretline::Dir::Backward, by: caretline::By::LineStart, extend: false });
    update(&mut s, Msg::DeleteBackward);
    assert_eq!(s.doc.text.to_string(), "- [b] one\n- two");
    update(&mut s, Msg::DeleteBackward);
    assert_eq!(s.doc.text.to_string(), "- [b] one\ntwo");
}

#[test]
fn pasted_markdown_reads_tags_the_config_has() {
    let mut s = markdown::load("", None, vp(), tagged());
    update(&mut s, Msg::Paste { text: Some("- [a] one\n- [q] two\n[X] three".into()) });
    assert_eq!(s.doc.text.to_string(), "- [a] one\n- [q] two\n[X] three");
    let o = s.blocks().unwrap();
    let tags: Vec<Option<char>> = o.blocks.iter().map(|b| b.tag).collect();
    assert_eq!(tags, [Some('a'), None, None], "{:?}", s.doc.text.to_string());
}
