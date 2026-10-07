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
    assert!(s.state().text.to_string().starts_with("Xhello"));
}

#[test]
fn keys_go_through_the_keymap() {
    let mut s = session();
    let r = ask(&mut s, json!({"op": "keys", "keys": "<down>Hey <c-z>"}));
    let msgs: Vec<Msg> = serde_json::from_value(r["result"]["msgs"].clone()).unwrap();
    assert_eq!(msgs[0], Msg::Move { dir: caretline_next::Dir::Forward, by: caretline_next::By::VisualLine, extend: false });
    assert_eq!(*msgs.last().unwrap(), Msg::Undo);
    assert_eq!(r["result"]["rev"], msgs.len() as u64);
    assert_eq!(s.state().text.to_string(), "hello world\nsecond line\n");
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

    let bare = Subscription { msgs: false, frame: None };
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

#[test]
fn trace_replays_to_the_same_state() {
    let mut s = session();
    ask(&mut s, json!({"op": "keys", "keys": "one<cr>two<wait:2000><s-a-left><c-x>"}));
    ask(&mut s, json!({"op": "state.set", "state": State::new("fresh\n", None, Viewport { width: 30, height: 5 })}));
    ask(&mut s, json!({"op": "msgs", "msgs": [{"msg": "move", "dir": "forward", "by": "word"}, {"msg": "insert_text", "text": "!"}]}));
    let r = ask(&mut s, json!({"op": "trace.get"}));
    let lines: Vec<String> = r["result"]["trace"].as_array().unwrap().iter().map(|l| l.to_string()).collect();
    let (replayed, n) = replay_trace(&lines.join("\n")).unwrap();
    assert_eq!(&replayed, s.state());
    assert_eq!(n, lines.iter().filter(|l| l.starts_with("{\"msg\"")).count());
    assert_eq!(s.trace_jsonl().lines().count(), lines.len());
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
    assert!(!s.state().dirty);

    // Through the protocol, with an executor and apply_effects.
    let mut exec = |e: &Effect| matches!(e, Effect::WriteFile { .. }).then_some(Msg::Saved);
    let h = s.handle(r#"{"op":"msgs","apply_effects":true,"msgs":[{"msg":"insert_text","text":"z"},{"msg":"save"}]}"#, Some(&mut exec));
    let r: Value = serde_json::from_str(&h.response).unwrap();
    assert_eq!(r["result"]["executed"], true);
    assert_eq!(h.change.unwrap().msgs.last(), Some(&Msg::Saved));
    assert!(!s.state().dirty);
    // Without apply_effects the executor is not used.
    let mut never = |_: &Effect| -> Option<Msg> { panic!("performed an effect") };
    s.handle(r#"{"op":"msgs","msgs":[{"msg":"save"}]}"#, Some(&mut never));
}

#[test]
fn show_status_is_passive() {
    let mut s = session();
    s.apply(Msg::ShowStatus { text: "hi there\nignored".into() });
    assert_eq!(s.state().status.as_deref(), Some("hi there"));
    assert!(view(s.state()).to_text().contains("hi there"));
    s.apply(Msg::Tick { now_ms: 9 });
    assert_eq!(s.state().status.as_deref(), Some("hi there"));
    s.apply(Msg::InsertText { text: "a".into() });
    assert_eq!(s.state().status, None);
}
