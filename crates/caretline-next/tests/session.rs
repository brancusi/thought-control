//! The in-process Session and the protocol handler: every op, the error cases, rev
//! ordering, subscriptions and trace replay.

use caretline_next::protocol::{event_line, Control, Format, Subscription};
use caretline_next::trace::replay_trace;
use caretline_next::{view, Effect, Msg, Session, State, Viewport};
use serde_json::{json, Value};

fn session() -> Session {
    Session::new(State::new("hello world\nsecond line\n", Some("doc.md".into()), Viewport { width: 30, height: 5 }))
}

fn ask(s: &mut Session, req: Value) -> Value {
    let h = s.handle(&req.to_string(), None);
    serde_json::from_str(&h.response).unwrap()
}

fn error_kind(v: &Value) -> &str {
    v["error"]["kind"].as_str().unwrap_or_else(|| panic!("not an error: {v}"))
}

#[test]
fn hello_reports_the_protocol() {
    let mut s = session();
    let r = ask(&mut s, json!({"id": "a", "op": "hello"}));
    assert_eq!(r["id"], "a");
    assert_eq!(r["result"]["proto"], 1);
    assert!(r["result"]["version"].is_string());
    assert_eq!(r["result"]["rev"], 0);
}

#[test]
fn state_get_returns_the_state_and_set_replaces_it() {
    let mut s = session();
    let r = ask(&mut s, json!({"id": 1, "op": "state.get"}));
    let got: State = serde_json::from_value(r["result"]["state"].clone()).unwrap();
    assert_eq!(&got, s.state());

    let other = State::new("replaced\n", None, Viewport { width: 20, height: 4 });
    let r = ask(&mut s, json!({"id": 2, "op": "state.set", "state": other}));
    assert_eq!(r["result"]["rev"], 1);
    assert_eq!(s.state(), &other);
    let r = ask(&mut s, json!({"op": "state.get"}));
    assert_eq!(r["result"]["state"]["text"], "replaced\n");
    assert!(r.get("id").is_none(), "no id, none echoed");
}

#[test]
fn state_set_repairs_an_out_of_range_selection() {
    let mut s = session();
    let mut st = serde_json::to_value(State::new("abc", None, Viewport { width: 20, height: 4 })).unwrap();
    st["selection"]["ranges"][0]["head"] = json!(99);
    st["selection"]["ranges"][0]["anchor"] = json!(99);
    let r = ask(&mut s, json!({"op": "state.set", "state": st}));
    assert!(r.get("result").is_some(), "{r}");
    assert_eq!(s.state().caret(), 3);
}

#[test]
fn msgs_apply_in_order_and_return_effects() {
    let mut s = session();
    let r = ask(
        &mut s,
        json!({"id": 1, "op": "msgs", "msgs": [
            {"msg": "insert_text", "text": "X"},
            {"msg": "select_all"},
            {"msg": "copy"},
            {"msg": "save"},
        ]}),
    );
    assert_eq!(r["result"]["rev"], 4);
    let effects: Vec<Effect> = serde_json::from_value(r["result"]["effects"].clone()).unwrap();
    assert!(matches!(&effects[0], Effect::ClipboardSet { text } if text.starts_with("Xhello")));
    assert!(matches!(&effects[1], Effect::WriteFile { path, .. } if path == "doc.md"));
    assert!(s.state().doc.text.to_string().starts_with("Xhello"));
}

#[test]
fn keys_go_through_the_keymap() {
    let mut s = session();
    let r = ask(&mut s, json!({"op": "keys", "keys": "<down>Hey <c-z>"}));
    let msgs: Vec<Msg> = serde_json::from_value(r["result"]["msgs"].clone()).unwrap();
    assert_eq!(msgs[0], Msg::Move { dir: caretline_next::Dir::Forward, by: caretline_next::By::VisualLine, extend: false });
    assert_eq!(*msgs.last().unwrap(), Msg::Undo);
    assert_eq!(r["result"]["rev"], msgs.len() as u64);
    assert_eq!(s.state().doc.text.to_string(), "hello world\nsecond line\n");
}

#[test]
fn render_matches_view_in_every_format() {
    let mut s = session();
    ask(&mut s, json!({"op": "keys", "keys": "<s-a-right>"}));
    let frame = view(s.state());
    let r = ask(&mut s, json!({"op": "render"}));
    assert_eq!(r["result"]["frame"], frame.to_text());
    assert_eq!(r["result"]["format"], "text");
    let r = ask(&mut s, json!({"op": "render", "format": "ansi"}));
    assert_eq!(r["result"]["frame"], frame.to_ansi());
    let r = ask(&mut s, json!({"op": "render", "format": "cells"}));
    let rows = r["result"]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0]["text"].as_str().unwrap().trim_end(), "hello world");
    // The selected word, then the status bar.
    assert_eq!(rows[0]["spans"], json!([[0, 5, "selection"]]));
    assert_eq!(rows[4]["spans"][0][2], "status");
    assert_eq!(r["result"]["cursor"], json!([5, 0]));
}

#[test]
fn render_at_another_size_leaves_the_session_alone() {
    let mut s = session();
    let before = s.state().clone();
    let r = ask(&mut s, json!({"op": "render", "w": 12, "h": 3}));
    assert_eq!(r["result"]["w"], 12);
    assert_eq!(r["result"]["frame"].as_str().unwrap().lines().count(), 3);
    assert_eq!(s.state(), &before);
    assert_eq!(s.rev(), 0);
    let r = ask(&mut s, json!({"op": "render", "w": 0, "h": 3}));
    assert_eq!(error_kind(&r), "bad_request");
}

#[test]
fn errors_carry_the_id_and_a_kind() {
    let mut s = session();
    let r = ask(&mut s, json!({"id": 7, "op": "frobnicate"}));
    assert_eq!((r["id"].clone(), error_kind(&r)), (json!(7), "unknown_op"));
    let r = ask(&mut s, json!({"id": 8, "op": "msgs", "msgs": [{"msg": "nope"}]}));
    assert_eq!((r["id"].clone(), error_kind(&r)), (json!(8), "bad_request"));
    let r = ask(&mut s, json!({"id": 9, "op": "msgs"}));
    assert_eq!(error_kind(&r), "bad_request");
    let r = ask(&mut s, json!({"id": 10, "op": "keys", "keys": "<c-nokey>"}));
    assert_eq!(error_kind(&r), "bad_keys");
    let r = ask(&mut s, json!({"id": 11, "op": "state.set"}));
    assert_eq!(error_kind(&r), "bad_request");
    let r = ask(&mut s, json!({"id": 12}));
    assert_eq!((r["id"].clone(), error_kind(&r)), (json!(12), "bad_request"));
    let r = ask(&mut s, json!({"id": 13, "op": "msgs", "msgs": [], "apply_effects": true}));
    assert_eq!(error_kind(&r), "unsupported");
    let h = s.handle("{not json", None);
    let r: Value = serde_json::from_str(&h.response).unwrap();
    assert_eq!(error_kind(&r), "parse");
    let h = s.handle("[1,2]", None);
    let r: Value = serde_json::from_str(&h.response).unwrap();
    assert_eq!(error_kind(&r), "bad_request");
    // Nothing above changed anything.
    assert_eq!(s.rev(), 0);
    assert_eq!(s.trace().len(), 1);
}

#[test]
fn rev_counts_every_change_and_guards_writes() {
    let mut s = session();
    let mut last = 0;
    for req in [
        json!({"op": "msgs", "msgs": [{"msg": "insert_text", "text": "a"}]}),
        json!({"op": "keys", "keys": "bc"}),
        json!({"op": "state.set", "state": State::new("x", None, Viewport { width: 10, height: 3 })}),
        json!({"op": "msgs", "msgs": [{"msg": "tick", "now_ms": 5}, {"msg": "insert_text", "text": "y"}]}),
    ] {
        let r = ask(&mut s, req);
        let rev = r["result"]["rev"].as_u64().unwrap();
        assert!(rev > last, "rev went from {last} to {rev}");
        last = rev;
    }
    assert_eq!(last, 6);
    // Reads don't move it.
    ask(&mut s, json!({"op": "state.get"}));
    ask(&mut s, json!({"op": "render"}));
    assert_eq!(s.rev(), 6);
    // if_rev: a stale writer is refused, a current one goes through.
    let r = ask(&mut s, json!({"op": "msgs", "if_rev": 5, "msgs": [{"msg": "undo"}]}));
    assert_eq!(error_kind(&r), "stale");
    let r = ask(&mut s, json!({"op": "msgs", "if_rev": 6, "msgs": [{"msg": "undo"}]}));
    assert_eq!(r["result"]["rev"], 7);
}

#[test]
fn subscribe_returns_a_control_and_events_describe_changes() {
    let mut s = session();
    let h = s.handle(r#"{"op":"subscribe","frame":{"w":20,"h":3,"format":"cells"}}"#, None);
    let Some(Control::Subscribe(sub)) = h.control else { panic!("no subscribe control") };
    assert!(sub.msgs);
    assert_eq!(sub.frame.unwrap().format, Format::Cells);

    let h = s.handle(r#"{"op":"msgs","msgs":[{"msg":"insert_text","text":"Q"}]}"#, None);
    let change = h.change.expect("a change");
    assert_eq!(change.rev, 1);
    let ev: Value = serde_json::from_str(&event_line(&s, &change, &sub, Some("client"))).unwrap();
    assert_eq!(ev["event"], "state");
    assert_eq!(ev["rev"], 1);
    assert_eq!(ev["msgs"][0]["text"], "Q");
    assert_eq!(ev["frame"]["rows"][0]["text"].as_str().unwrap().trim_end(), "Qhello world");

    let bare = Subscription { msgs: false, frame: None, state: false };
    let ev: Value = serde_json::from_str(&event_line(&s, &change, &bare, None)).unwrap();
    assert!(ev.get("msgs").is_none() && ev.get("frame").is_none());

    let h = s.handle(r#"{"op":"state.set","state":{"text":"t","selection":{"ranges":[{"anchor":0,"head":0}],"primary_index":0},"scroll":{"line":0,"row":0,"col":0},"viewport":{"width":10,"height":3},"clipboard":"","path":null,"history":{"revisions":[{"parent":0,"last_child":null,"transaction":{"changes":{"changes":[],"len":0,"len_after":0},"selection":null},"inversion":{"changes":{"changes":[],"len":0,"len_after":0},"selection":null},"timestamp":0}],"current":0},"saved_revision":0,"saving":null,"dirty":false,"config":{"tab_width":4,"soft_wrap":true,"scrolloff":2,"line_ending":"LF"},"status":null,"now_ms":0,"run":null,"quit_armed":false}}"#, None);
    assert!(h.response.contains("\"result\""), "{}", h.response);
    let change = h.change.unwrap();
    assert!(change.state_set);
    let ev: Value = serde_json::from_str(&event_line(&s, &change, &bare, None)).unwrap();
    assert_eq!(ev["state_set"], true);

    let h = s.handle(r#"{"op":"unsubscribe"}"#, None);
    assert_eq!(h.control, Some(Control::Unsubscribe));
}

fn trace_lines(r: &Value) -> Vec<String> {
    r["result"]["trace"].as_array().unwrap().iter().map(|l| l.to_string()).collect()
}

#[test]
fn trace_replays_to_the_same_state() {
    let mut s = session();
    ask(&mut s, json!({"op": "keys", "keys": "one<cr>two<wait:2000><s-a-left><c-x>"}));
    let before_set = s.rev();
    ask(&mut s, json!({"op": "state.set", "state": State::new("fresh\n", None, Viewport { width: 30, height: 5 })}));
    ask(&mut s, json!({"op": "msgs", "msgs": [{"msg": "move", "dir": "forward", "by": "word"}, {"msg": "insert_text", "text": "!"}]}));

    // The whole trace replays, through the replacement.
    let r = ask(&mut s, json!({"op": "trace.get", "all": true}));
    assert_eq!(r["result"]["from_rev"], 0);
    let lines = trace_lines(&r);
    let (replayed, n) = replay_trace(&lines.join("\n")).unwrap();
    assert_eq!(&replayed, s.state());
    assert_eq!(n, lines.iter().filter(|l| l.starts_with("{\"msg\"")).count());
    assert_eq!(s.trace_jsonl().lines().count(), lines.len());

    // By default: the current segment, from the state.set on, which replays on its own.
    let r = ask(&mut s, json!({"op": "trace.get"}));
    assert_eq!(r["result"]["from_rev"], before_set + 1);
    let segment = trace_lines(&r);
    assert_eq!(segment.len(), 3);
    assert!(segment[0].starts_with("{\"state\""));
    let (replayed, n) = replay_trace(&segment.join("\n")).unwrap();
    assert_eq!((&replayed, n), (s.state(), 2));
}

#[test]
fn trace_since_rev_and_checkpoints() {
    let mut s = session();
    ask(&mut s, json!({"op": "keys", "now_ms": 0, "keys": "abc"}));
    assert_eq!(s.rev(), 3);

    // The lines after a rev: what changed since.
    let r = ask(&mut s, json!({"op": "trace.get", "since_rev": 1}));
    assert_eq!(r["result"]["from_rev"], 1);
    assert_eq!(trace_lines(&r).len(), 2);
    let r = ask(&mut s, json!({"op": "trace.get", "since_rev": 3}));
    assert_eq!(trace_lines(&r).len(), 0);

    // A checkpoint starts a segment without changing the state or the rev.
    let r = ask(&mut s, json!({"op": "trace.checkpoint"}));
    assert_eq!(r["result"]["rev"], 3);
    assert_eq!(s.rev(), 3);
    ask(&mut s, json!({"op": "keys", "keys": "d"}));
    let r = ask(&mut s, json!({"op": "trace.get"}));
    assert_eq!(r["result"]["from_rev"], 3);
    let segment = trace_lines(&r);
    assert_eq!(segment.len(), 2);
    let (replayed, _) = replay_trace(&segment.join("\n")).unwrap();
    assert_eq!(&replayed, s.state());
    // since_rev skips the checkpoint at the rev itself.
    let r = ask(&mut s, json!({"op": "trace.get", "since_rev": 3}));
    assert_eq!(trace_lines(&r), vec![r#"{"msg":{"msg":"insert_text","text":"d"}}"#.to_string()]);
    // The full trace (the checkpoint included) still replays.
    let (replayed, _) = replay_trace(&s.trace_jsonl()).unwrap();
    assert_eq!(&replayed, s.state());

    let r = ask(&mut s, json!({"op": "trace.get", "since_rev": 1, "all": true}));
    assert_eq!(error_kind(&r), "bad_request");
}

#[test]
fn the_trace_limit_drops_old_segments_and_cuts_long_ones() {
    let mut s = session();
    s.set_trace_limit(10);
    for _ in 0..4 {
        s.apply(Msg::InsertText { text: "x".into() });
    }
    s.set_state(State::new("new\n", None, Viewport { width: 30, height: 5 }));
    for _ in 0..7 {
        s.apply(Msg::InsertText { text: "y".into() });
    }
    // 1 + 4 + 1 + 7 lines: the first segment went; the second (8 lines) stays whole.
    assert_eq!(s.trace().len(), 8);
    assert_eq!(s.trace_start_rev(), 5);
    assert_eq!(s.trace_lines_total(), 13);
    let r = ask(&mut s, json!({"op": "trace.get", "since_rev": 2}));
    assert_eq!(error_kind(&r), "trimmed");
    let r = ask(&mut s, json!({"op": "trace.get", "since_rev": 5}));
    assert_eq!(trace_lines(&r).len(), 7);

    // A segment that outgrows the limit on its own is cut by an automatic checkpoint.
    for _ in 0..10 {
        s.apply(Msg::InsertText { text: "z".into() });
    }
    assert!(s.trace().len() <= 10, "{}", s.trace().len());
    let (replayed, _) = replay_trace(&s.trace_jsonl()).unwrap();
    assert_eq!(&replayed, s.state());
    assert!(s.trace_lines_from(0).is_none(), "dropped lines are gone");
    assert_eq!(s.trace_lines_from(s.trace_lines_total()).unwrap().len(), 0);
}

#[test]
fn state_set_takes_a_minimal_state() {
    let mut s = session();
    let r = ask(
        &mut s,
        json!({"op": "state.set", "state": {
            "text": "hello\nworld\n",
            "selection": {"ranges": [{"anchor": 0, "head": 5}]},
            "viewport": {"width": 20, "height": 4},
            "config": {"soft_wrap": false}
        }}),
    );
    assert!(r["result"]["rev"].is_u64(), "{r}");
    let st = s.state();
    assert_eq!(st.doc.text.to_string(), "hello\nworld\n");
    assert_eq!((st.view.selection.primary().anchor, st.view.selection.primary().head), (0, 5));
    assert!(!st.doc.dirty, "a pushed state counts as saved");
    assert!(!st.doc.config.soft_wrap);
    assert_eq!(st.doc.config.tab_width, 4);
    // A working editor: type over the selection, then undo it.
    ask(&mut s, json!({"op": "msgs", "msgs": [{"msg": "insert_text", "text": "bye"}]}));
    assert_eq!(s.state().doc.text.to_string(), "bye\nworld\n");
    assert!(s.state().doc.dirty);
    ask(&mut s, json!({"op": "msgs", "msgs": [{"msg": "undo"}]}));
    assert_eq!(s.state().doc.text.to_string(), "hello\nworld\n");
    assert!(!s.state().doc.dirty);

    // Text alone works too: the rest takes State::new's defaults.
    let r = ask(&mut s, json!({"op": "state.set", "state": {"text": "a\r\nb\r\n"}}));
    assert!(r["result"]["rev"].is_u64(), "{r}");
    let fresh = State::new("a\r\nb\r\n", None, Viewport { width: 80, height: 24 });
    assert_eq!(s.state(), &fresh);
    // An explicit null saved_revision means never saved.
    ask(&mut s, json!({"op": "state.set", "state": {"text": "x", "saved_revision": null}}));
    assert!(s.state().doc.dirty);
}

#[test]
fn apply_with_feeds_effect_results_back() {
    let mut s = session();
    let mut performed = Vec::new();
    let (effects, applied) = s.apply_with(Msg::Save, &mut |e| {
        performed.push(e.clone());
        matches!(e, Effect::WriteFile { .. }).then_some(Msg::Saved)
    });
    assert_eq!(effects.len(), 1);
    assert_eq!(performed, effects);
    assert_eq!(applied, vec![Msg::Save, Msg::Saved]);
    assert_eq!(s.rev(), 2);
    assert!(!s.state().doc.dirty);

    // Through the protocol, with an executor and apply_effects.
    let mut exec = |e: &Effect| matches!(e, Effect::WriteFile { .. }).then_some(Msg::Saved);
    let h = s.handle(r#"{"op":"msgs","apply_effects":true,"msgs":[{"msg":"insert_text","text":"z"},{"msg":"save"}]}"#, Some(&mut exec));
    let r: Value = serde_json::from_str(&h.response).unwrap();
    assert_eq!(r["result"]["executed"], true);
    // The response lists every message applied, the fed-back result included.
    assert_eq!(r["result"]["msgs"].as_array().unwrap().last().unwrap()["msg"], "saved");
    assert_eq!(h.change.unwrap().msgs.last(), Some(&Msg::Saved));
    assert!(!s.state().doc.dirty);
    // Without apply_effects the executor is not used.
    let mut never = |_: &Effect| -> Option<Msg> { panic!("performed an effect") };
    s.handle(r#"{"op":"msgs","msgs":[{"msg":"save"}]}"#, Some(&mut never));
}

#[test]
fn show_status_is_passive() {
    let mut s = session();
    s.apply(Msg::ShowStatus { text: "hi there\nignored".into() });
    assert_eq!(s.state().view.status.as_deref(), Some("hi there"));
    assert!(view(s.state()).to_text().contains("hi there"));
    s.apply(Msg::Tick { now_ms: 9 });
    assert_eq!(s.state().view.status.as_deref(), Some("hi there"));
    s.apply(Msg::InsertText { text: "a".into() });
    assert_eq!(s.state().view.status, None);
}

#[test]
fn requests_tick_to_their_own_time_or_the_runtimes() {
    let mut s = session();
    // A request's now_ms becomes a tick before its messages, and waits count from it.
    let r = ask(&mut s, json!({"op": "keys", "now_ms": 1000, "keys": "a<wait:500>b"}));
    let msgs = &r["result"]["msgs"];
    assert_eq!(msgs[0], json!({"msg": "tick", "now_ms": 1000}));
    assert_eq!(msgs[2], json!({"msg": "tick", "now_ms": 1500}));
    assert_eq!(s.state().doc.now_ms, 1500);

    // A runtime clock ticks forward only, and the request's own time wins.
    let h = s.handle_at(r#"{"op":"msgs","msgs":[{"msg":"insert_text","text":"c"}]}"#, None, Some(9000));
    assert_eq!(h.change.unwrap().msgs[0], Msg::Tick { now_ms: 9000 });
    let h = s.handle_at(r#"{"op":"msgs","msgs":[{"msg":"insert_text","text":"d"}]}"#, None, Some(100));
    assert_eq!(h.change.unwrap().msgs.len(), 1, "no tick backwards");
    let h = s.handle_at(r#"{"op":"msgs","now_ms":20000,"msgs":[]}"#, None, Some(9500));
    assert_eq!(h.change.unwrap().msgs, vec![Msg::Tick { now_ms: 20000 }]);

    // Typing far apart in time is separate undo steps; close together, one.
    let mut s = session();
    ask(&mut s, json!({"op": "keys", "now_ms": 1, "keys": "ab"}));
    ask(&mut s, json!({"op": "keys", "now_ms": 5000, "keys": "cd"}));
    ask(&mut s, json!({"op": "keys", "keys": "<c-z>"}));
    assert!(s.state().doc.text.to_string().starts_with("abhello"));
}

#[test]
fn the_status_bar_can_be_hidden() {
    let mut st = State::new("one\ntwo\nthree\n", None, Viewport { width: 20, height: 3 });
    st.view.config.status_bar = false;
    let text = view(&st).to_text();
    assert_eq!(text, "one\ntwo\nthree\n");
    // A state saved before the setting existed shows it.
    let mut v = serde_json::to_value(&st).unwrap();
    v["config"].as_object_mut().unwrap().remove("status_bar");
    let old: State = serde_json::from_value(v).unwrap();
    assert!(old.view.config.status_bar);
}

#[test]
fn two_views_of_one_document_over_the_protocol() {
    let mut s = session();
    let r = ask(&mut s, json!({"op":"view.open","w":20,"h":4}));
    let v = r["result"]["view"].as_u64().unwrap();
    assert_eq!(v, 1);
    // View 1 goes to the second line; view 0 types at the start.
    ask(&mut s, json!({"op":"keys","view":v,"keys":"<down><end>"}));
    ask(&mut s, json!({"op":"msgs","msgs":[{"msg":"insert_text","text":">> "}]}));
    assert_eq!(s.state().doc.text.to_string(), ">> hello world\nsecond line\n");
    // View 1 kept its place on its own line: typing there lands at its end.
    ask(&mut s, json!({"op":"msgs","view":v,"msgs":[{"msg":"insert_text","text":"!"}]}));
    assert_eq!(s.state().doc.text.to_string(), ">> hello world\nsecond line!\n");
    let r = ask(&mut s, json!({"op":"render","view":v}));
    assert_eq!(r["result"]["w"], 20);
    assert_eq!(r["result"]["cursor"], json!([12, 1]));
    let r = ask(&mut s, json!({"op":"state.get","view":v}));
    assert_eq!(r["result"]["state"]["viewport"]["width"], 20);
    let r = ask(&mut s, json!({"op":"view.list"}));
    assert_eq!(r["result"]["views"].as_array().unwrap().len(), 2);
    // Undo through view 1 takes back its own last step.
    ask(&mut s, json!({"op":"msgs","view":v,"msgs":[{"msg":"undo"}]}));
    assert_eq!(s.state().doc.text.to_string(), ">> hello world\nsecond line\n");
    // The trace replays both views.
    let lines: String = s.trace_jsonl();
    let (state, views, _) = caretline_next::trace::replay_trace_views(&lines).unwrap();
    assert_eq!(state, *s.state());
    assert_eq!(views, s.views().to_vec());
    // Unknown views are errors; view 0 never closes.
    assert_eq!(error_kind(&ask(&mut s, json!({"op":"render","view":9}))), "no_view");
    assert_eq!(error_kind(&ask(&mut s, json!({"op":"view.close","view":0}))), "bad_request");
    assert_eq!(ask(&mut s, json!({"op":"view.close","view":v}))["result"]["closed"], 1);
    assert!(s.views().is_empty());
}

#[test]
fn an_external_change_then_local_undo_over_the_protocol() {
    let mut s = session();
    ask(&mut s, json!({"op":"keys","keys":"<end>!"}));
    ask(&mut s, json!({"op":"msgs","msgs":[{"msg":"external","changes":[{"change":"replace","from":13,"to":19,"text":"2nd"}]}]}));
    assert_eq!(s.state().doc.text.to_string(), "hello world!\n2nd line\n");
    ask(&mut s, json!({"op":"msgs","msgs":[{"msg":"undo"}]}));
    assert_eq!(s.state().doc.text.to_string(), "hello world\n2nd line\n", "undo kept the change from elsewhere");
    let (state, _) = replay_trace(&s.trace_jsonl()).unwrap();
    assert_eq!(state, *s.state());
}

#[test]
fn a_checkpoint_carries_the_open_views() {
    let mut s = session();
    let v = s.open_view(caretline_next::View::new(Viewport { width: 10, height: 3 }));
    s.apply_on(v, Msg::Move { dir: caretline_next::Dir::Forward, by: caretline_next::By::DocEnd, extend: false });
    s.checkpoint();
    let seg: String = s.segment_trace().iter().map(|l| l.to_line() + "\n").collect();
    let (state, views, _) = caretline_next::trace::replay_trace_views(&seg).unwrap();
    assert_eq!(state, *s.state());
    assert_eq!(views, s.views().to_vec());
}

/// A session with three separate undo steps (each edit 10 s after the last), unsaved.
fn edited_session() -> Session {
    let mut s = session();
    for (i, t) in ["a", "b", "c"].iter().enumerate() {
        s.apply(Msg::Tick { now_ms: 10_000 * (i as u64 + 1) });
        s.apply(Msg::InsertText { text: t.to_string() });
    }
    s
}

#[test]
fn state_get_without_history_leaves_out_the_undo_history() {
    let mut s = edited_session();
    let full = ask(&mut s, json!({"op": "state.get"}));
    let light = ask(&mut s, json!({"op": "state.get", "history": false}));
    let st = &light["result"]["state"];
    for field in ["history", "saving", "run", "mark_log", "undo_floor"] {
        assert!(st.get(field).is_none(), "{field} left out: {st}");
    }
    assert!(full["result"]["state"].get("history").is_some());
    assert_eq!(st["text"], full["result"]["state"]["text"]);
    assert_eq!(st["selection"], full["result"]["state"]["selection"]);
    assert_eq!(st["dirty"], true);
    assert_eq!(st["saved_revision"], Value::Null, "a dirty document is never saved in a fresh history");
    assert!(light.to_string().len() < full.to_string().len());
}

#[test]
fn a_state_without_history_rehydrates_to_a_working_editor() {
    let mut s = edited_session();
    let light = ask(&mut s, json!({"op": "state.get", "history": false}))["result"]["state"].clone();
    let mut t = Session::new(State::default());
    let r = ask(&mut t, json!({"op": "state.set", "state": light}));
    assert!(r.get("result").is_some(), "{r}");
    let st = t.state();
    assert_eq!(st.doc.text, s.state().doc.text);
    assert_eq!(st.view.selection, s.state().view.selection);
    assert!(st.doc.dirty, "dirty is kept");
    assert_eq!(st.doc.history.len(), 1, "a fresh history");
    assert_eq!(view(st), view(s.state()), "the same frame");
    // It edits, and undo takes back only what happened after the push.
    let before = st.doc.text.to_string();
    t.apply(Msg::Tick { now_ms: 100_000 });
    t.apply(Msg::InsertText { text: "z".into() });
    assert_ne!(t.state().doc.text.to_string(), before);
    t.apply(Msg::Undo);
    assert_eq!(t.state().doc.text.to_string(), before);
    t.apply(Msg::Undo);
    assert_eq!(t.state().doc.text.to_string(), before, "nothing older to undo");

    // A clean document stays clean.
    let mut c = session();
    let light = ask(&mut c, json!({"op": "state.get", "history": false}))["result"]["state"].clone();
    assert_eq!(light["saved_revision"], 0);
    let back: State = State::from_json(&light.to_string()).unwrap();
    assert!(!back.doc.dirty);
}

#[test]
fn history_get_returns_what_state_get_leaves_out() {
    let mut s = edited_session();
    let mut light = ask(&mut s, json!({"op": "state.get", "history": false}))["result"]["state"].clone();
    let h = ask(&mut s, json!({"id": 4, "op": "history.get"}));
    assert_eq!(h["id"], 4);
    let part = h["result"].as_object().unwrap();
    assert_eq!(part["rev"], s.rev());
    for (k, v) in part.iter().filter(|(k, _)| *k != "rev") {
        light[k] = v.clone();
    }
    let merged: State = State::from_json(&light.to_string()).unwrap();
    assert_eq!(&merged, s.state(), "the two parts make the whole state");
}

#[test]
fn subscribers_can_have_the_state_without_history() {
    let mut s = session();
    let h = s.handle(r#"{"op":"subscribe","with_state":true,"with_msgs":false}"#, None);
    let Some(Control::Subscribe(sub)) = h.control else { panic!("no subscribe control") };
    assert!(sub.state);
    let h = s.handle(r#"{"op":"keys","keys":"hi"}"#, None);
    let ev: Value = serde_json::from_str(&event_line(&s, &h.change.unwrap(), &sub, Some("client"))).unwrap();
    assert_eq!(ev["state"]["text"], "hihello world\nsecond line\n");
    assert!(ev["state"].get("history").is_none());
    assert!(ev.get("msgs").is_none());
}
