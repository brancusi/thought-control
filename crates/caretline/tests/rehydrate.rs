//! Rehydration and replay: a state survives JSON exactly, and a recorded session replays to
//! exactly the live result.

mod common;

use caretline::trace::{parse_msgs, replay_trace, TraceLine};
use caretline::{update, view, Effect, Msg, State, Viewport};
use common::*;
use rand::rngs::StdRng;
use rand::SeedableRng;

#[track_caller]
fn round_trip(state: &State) {
    let json = state.to_json();
    let back = State::from_json(&json).expect("state parses");
    assert_eq!(&back, state, "state differs after a round trip");
    assert_eq!(view(&back), view(state), "frame differs after a round trip");
    assert_eq!(back.to_json(), json, "serialization is stable");
}

#[test]
fn fresh_and_edited_states_round_trip() {
    let mut s = state_wh("Hello ⟦wor▮⟧ld\nsecond line with 漢字 and 👍🏽", 24, 6);
    round_trip(&s);
    keys(&mut s, "X<wait:2000>yz<cr><c-z><a-left><s-down><c-c><end><c-v>");
    round_trip(&s);
    // The goal column lives in the selection and survives too.
    let mut w = state_wh("aaaa bbbb cc▮cc dddd eeee ffff gggg\nxy\nhhhh iiii jjjj", 20, 6);
    keys(&mut w, "<down><down>");
    round_trip(&w);
    let back = State::from_json(&w.to_json()).unwrap();
    let mut a = w.clone();
    let mut b = back;
    keys(&mut a, "<down>");
    keys(&mut b, "<down>");
    assert_eq!(show(&a), show(&b));
    assert_eq!(show(&a), "aaaa bbbb cccc dddd eeee ffff gggg\nxy\nhhhh iiii jj▮jj");
}

#[test]
fn undo_history_survives_a_round_trip() {
    let mut s = state("▮");
    keys(&mut s, "one<wait:2000> two<wait:2000> three<s-a-left><bs>");
    let mut back = State::from_json(&s.to_json()).unwrap();
    for _ in 0..4 {
        keys(&mut s, "<c-z>");
        keys(&mut back, "<c-z>");
        assert_eq!(s, back);
    }
    assert_eq!(show(&back), "▮");
    keys(&mut back, "<c-y><c-y><c-y>");
    assert_eq!(show(&back), "one two three▮");
}

#[test]
fn a_typing_run_continues_across_a_round_trip() {
    let mut s = state("▮");
    keys(&mut s, "ab");
    let mut back = State::from_json(&s.to_json()).unwrap();
    keys(&mut back, "cd<c-z>");
    assert_eq!(show(&back), "▮", "the run was still open after rehydration");
}

#[test]
fn hand_written_states_are_repaired_on_load() {
    let mut s = state("a👍🏽b▮");
    let mut json: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
    // A selection inside the emoji and past the end.
    json["selection"]["ranges"][0]["anchor"] = 2.into();
    json["selection"]["ranges"][0]["head"] = 99.into();
    json["viewport"]["width"] = 0.into();
    let fixed = State::from_json(&json.to_string()).unwrap();
    assert_eq!(fixed.view.selection.primary().anchor, 1);
    assert_eq!(fixed.view.selection.primary().head, 4);
    assert_eq!(fixed.view.viewport.width, 1);
    s.sanitize();
    round_trip(&s);
}

/// A stand-in for the interactive runtime: records the initial state and every message
/// (including the result messages its effects produce) exactly as the binary does.
struct Recorder {
    state: State,
    trace: String,
}

impl Recorder {
    fn new(state: State) -> Self {
        let trace = TraceLine::State(Box::new(state.clone())).to_line() + "\n";
        Recorder { state, trace }
    }

    fn dispatch(&mut self, msg: Msg) {
        let mut queue = vec![msg];
        while let Some(msg) = queue.pop() {
            self.trace.push_str(&TraceLine::Msg(msg.clone()).to_line());
            self.trace.push('\n');
            for effect in update(&mut self.state, msg) {
                if let Effect::WriteFile { .. } = effect {
                    queue.insert(0, Msg::Saved);
                }
            }
        }
    }
}

#[test]
fn replaying_a_trace_gives_the_live_state() {
    let start = state_wh("The quick brown fox\njumps over ▮the lazy dog", 30, 8);
    let mut live = Recorder::new(start);
    let mut now = 1_000;
    for script in ["hello", "<a-left><s-a-right>", "<c-x>", "<down><c-v>", "<c-s>", "<c-z><c-z><c-y>"] {
        now += 700;
        live.dispatch(Msg::Tick { now_ms: now });
        for msg in caretline::script_to_msgs(script, now).unwrap() {
            live.dispatch(msg);
        }
    }
    live.dispatch(Msg::Resize { width: 18, height: 5 });
    live.dispatch(Msg::Paste { text: Some("pasted\ntext".into()) });

    let (replayed, count) = replay_trace(&live.trace).unwrap();
    assert!(count > 10);
    assert_eq!(replayed, live.state);
    assert_eq!(view(&replayed), view(&live.state));
    assert!(!replayed.doc.dirty || replayed.doc.saved_revision.is_some());
}

#[test]
fn random_sessions_replay_exactly() {
    let mut rng = StdRng::seed_from_u64(99);
    for seed in 0..8 {
        let text = common::gen::text(&mut rng);
        let start = State::new(&text, Some(format!("doc{seed}.md")), Viewport { width: 40, height: 12 });
        let mut live = Recorder::new(start);
        for _ in 0..300 {
            let msg = common::gen::msg(&mut rng, &live.state);
            live.dispatch(msg);
        }
        let (replayed, _) = replay_trace(&live.trace).unwrap();
        assert_eq!(replayed, live.state, "seed {seed}");
        assert_eq!(view(&replayed).to_text(), view(&live.state).to_text());
    }
}

#[test]
fn message_files_parse_as_lines_or_an_array() {
    let lines = "{\"msg\":\"insert_text\",\"text\":\"hi\"}\n\n// a comment\n{\"msg\":\"move\",\"dir\":\"backward\",\"by\":\"word\"}\n{\"msg\":\"tick\",\"now_ms\":5}\n";
    let msgs = parse_msgs(lines).unwrap();
    assert_eq!(msgs.len(), 3);
    let array = serde_json::to_string(&msgs).unwrap();
    assert_eq!(parse_msgs(&array).unwrap(), msgs);
    let mut s = state("▮");
    send(&mut s, msgs);
    assert_eq!(show(&s), "▮hi");
}
