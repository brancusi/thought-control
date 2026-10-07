//! Collaborating with a person: a client pushes text (`text.set`) and messages through its own
//! view while the person types through view 0.
//!
//! - Text put in at the person's caret goes after it, whichever way it comes.
//! - A client's `msgs`, `keys` and `text.set` go through its own view when the server gives
//!   clients one (a live editor); `"view": 0` acts as the person.
//! - Under a stream of pushes, the person's typing comes out contiguous, their caret never
//!   leaves it, their undo takes back only their typing, and the trace replays exactly.

use caretline::helix::Selection;
use caretline::trace::replay_trace_views;
use caretline::{Msg, Session, State, Viewport};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::{json, Value};

const START: &str = "Hey there, \nnext\n";
const NOTE: &str = "\n- Agent note: safely.";

/// A person with their caret at the end of the first line.
fn person() -> Session {
    let mut st = State::new(START, Some("notes.md".into()), Viewport { width: 60, height: 10 });
    st.view.selection = Selection::point(11);
    Session::new(st)
}

/// A request from a client of a live editor, which writes through its own view `own`.
fn client(s: &mut Session, own: &mut Option<u32>, req: Value) -> Value {
    let h = s.handle_client(&req.to_string(), None, None, Some(own));
    serde_json::from_str(&h.response).unwrap()
}

fn text(s: &Session) -> String {
    s.state().doc.text.to_string()
}

fn caret(s: &Session) -> usize {
    s.state().view.caret()
}

#[test]
fn text_set_changes_only_what_differs_and_leaves_the_persons_caret() {
    let mut s = person();
    let mut own = None;
    let new = format!("Hey there, {NOTE}\nnext\n");
    let rev = s.rev();
    let r = client(&mut s, &mut own, json!({"op": "text.set", "text": new, "if_rev": rev}));
    assert_eq!(r["result"]["changed"], true, "{r}");
    assert_eq!(r["result"]["msgs"][0]["changes"], json!([{"change": "replace", "from": 12, "to": 12, "text": "- Agent note: safely.\n"}]), "only the inserted line");
    let v = own.expect("the client's own view");
    assert_eq!(r["result"]["view"], v);
    assert_ne!(v, 0);
    assert_eq!(caret(&s), 11, "the person's caret stays before the inserted text");
    s.apply(Msg::InsertText { text: "w".into() });
    assert_eq!(text(&s), format!("Hey there, w{NOTE}\nnext\n"));

    // The same text again changes nothing, not even the rev.
    let rev = s.rev();
    let same = text(&s);
    let r = client(&mut s, &mut own, json!({"op": "text.set", "text": same}));
    assert_eq!((r["result"]["changed"].as_bool(), r["result"]["rev"].as_u64()), (Some(false), Some(rev)));
    // A stale if_rev writes nothing.
    let r = client(&mut s, &mut own, json!({"op": "text.set", "text": "gone", "if_rev": rev - 1}));
    assert_eq!(r["error"]["kind"], "stale", "{r}");
    assert_eq!(own, Some(v), "one view per client");

    // The person's undo takes back their typing only.
    s.apply(Msg::Undo);
    assert_eq!(text(&s), format!("Hey there, {NOTE}\nnext\n"));
}

#[test]
fn a_clients_messages_go_through_its_own_view() {
    let mut s = person();
    let mut own = None;
    let r = client(&mut s, &mut own, json!({"op": "msgs", "msgs": [{"msg": "insert_text", "text": "AGENT"}]}));
    let v = own.unwrap();
    assert_eq!(r["result"]["view"], v, "{r}");
    assert_eq!(caret(&s), 11, "the person's caret didn't move");
    assert_eq!(s.view(v).unwrap().caret(), 16, "the client's own caret moved past its text");
    s.apply(Msg::InsertText { text: "w".into() });
    client(&mut s, &mut own, json!({"op": "keys", "keys": "!"}));
    assert_eq!(text(&s), "Hey there, wAGENT!\nnext\n");
    assert_eq!(caret(&s), 12);
    // "view": 0 acts as the person.
    client(&mut s, &mut own, json!({"op": "msgs", "view": 0, "msgs": [{"msg": "insert_text", "text": "P"}]}));
    assert_eq!((text(&s).as_str(), caret(&s)), ("Hey there, wPAGENT!\nnext\n", 13));
    // Without client views (a headless server), messages go through view 0 as before.
    let mut h = person();
    let r: Value = serde_json::from_str(&h.handle(&json!({"op": "msgs", "msgs": [{"msg": "insert_text", "text": "x"}]}).to_string(), None).response).unwrap();
    assert_eq!(r["result"]["view"], 0);
    assert_eq!(caret(&h), 12);
    assert!(h.views().is_empty());
}

#[test]
fn the_session_api_sets_text_the_same_way() {
    let mut s = person();
    let copy = s.state().view.clone();
    let v = s.open_view(copy);
    let msg = s.set_text_on(v, &format!("Hey there, {NOTE}\nnext\n")).expect("a change");
    assert!(matches!(msg, Msg::External { .. }));
    assert_eq!(caret(&s), 11);
    let same = text(&s);
    assert!(s.set_text(&same).is_none(), "nothing to change");
    // Line breaks become the document's.
    let mut crlf = Session::new(State::new("a\r\nb\r\n", None, Viewport { width: 20, height: 4 }));
    crlf.set_text("a\nX\nb\n");
    assert_eq!(text(&crlf), "a\r\nX\r\nb\r\n");
}

/// Somewhere to put text around the person's caret: at it, at the start of the next line, at
/// the start of the document, or at its end.
fn around(rng: &mut StdRng, t: &str, at: usize) -> usize {
    let chars: Vec<char> = t.chars().collect();
    match rng.random_range(0..4) {
        0 => at,
        1 => chars[at..].iter().position(|&c| c == '\n').map_or(chars.len(), |i| at + i + 1),
        2 => 0,
        _ => chars.len(),
    }
}

const PIECES: &[&str] = &["\n- 1 note", "\n", "## 2\n", "33", "\n- 4 more\n"];

#[test]
fn a_person_keeps_typing_through_a_stream_of_pushes() {
    let seeds: u64 = std::env::var("CARETLINE_COLLAB_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(12);
    for seed in 0..seeds {
        let mut rng = StdRng::seed_from_u64(0xc011_ab00 + seed);
        let mut s = person();
        let mut own = None;
        let mut typed = String::new();
        let mut now = 1_000u64;
        let mut pushes = 0;
        for step in 0..400 {
            let ctx = format!("seed {seed} step {step}");
            now += rng.random_range(10..3000);
            if rng.random_bool(0.4) {
                let c = (b'a' + (typed.len() % 26) as u8) as char;
                s.apply(Msg::Tick { now_ms: now });
                s.apply(Msg::InsertText { text: c.to_string() });
                typed.push(c);
            } else {
                // The client reads, then writes against what it read.
                let t = text(&s);
                let rev = s.rev();
                let at = around(&mut rng, &t, caret(&s));
                let piece = PIECES[rng.random_range(0..PIECES.len())];
                let r = if rng.random_bool(0.5) {
                    let mut new: Vec<char> = t.chars().collect();
                    new.splice(at..at, piece.chars());
                    client(&mut s, &mut own, json!({"op": "text.set", "text": new.into_iter().collect::<String>(), "if_rev": rev}))
                } else {
                    let change = json!({"change": "replace", "from": at, "to": at, "text": piece});
                    client(&mut s, &mut own, json!({"op": "msgs", "msgs": [{"msg": "external", "changes": [change]}], "if_rev": rev}))
                };
                assert!(r.get("result").is_some(), "{ctx}: {r}");
                pushes += 1;
            }
            // The person's typing is one run, and their caret is at its end.
            let t = text(&s);
            let mine = format!("Hey there, {typed}");
            let start = t.find(&mine).unwrap_or_else(|| panic!("{ctx}: {mine:?} not contiguous in {t:?}"));
            let end = t[..start + mine.len()].chars().count();
            assert_eq!(caret(&s), end, "{ctx}: the caret left the person's text in {t:?}");
            assert_eq!(s.state().view.selection.len(), 1, "{ctx}");
        }
        assert!(pushes > 100, "seed {seed}: {pushes} pushes");

        // The trace replays to the same document and views.
        let (state, views, _) = replay_trace_views(&s.trace_jsonl()).unwrap();
        assert_eq!(serde_json::to_value(&state).unwrap(), serde_json::to_value(s.state()).unwrap(), "seed {seed}: replay");
        assert_eq!(views.len(), s.views().len());
        for ((a, va), (b, vb)) in views.iter().zip(s.views()) {
            assert_eq!((a, &va.selection), (b, &vb.selection), "seed {seed}: replayed view");
        }

        // The person's undo takes back their typing and nothing else.
        let want = text(&s).replacen(&format!("Hey there, {typed}"), "Hey there, ", 1);
        let mut guard = 0;
        while s.state().doc.history.current_revision() != 0 {
            s.apply(Msg::Undo);
            guard += 1;
            assert!(guard < 10_000);
        }
        assert_eq!(text(&s), want, "seed {seed}: undo took back more than the person's typing");
    }
}
