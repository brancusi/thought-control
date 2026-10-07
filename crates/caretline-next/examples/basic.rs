//! A tour of the caretline-next API: build a state, drive it with messages and keys,
//! read the text and selection, handle effects, render a frame, and save and reload the
//! state.
//!
//! Run it with `cargo run -p caretline-next --example basic`.

use caretline_next::trace::{replay_trace, TraceLine};
use caretline_next::update::selection_text;
use caretline_next::{script_to_msgs, update, view, By, Dir, Effect, Msg, State, Viewport};

fn main() {
    // 1. A state: the text, an optional file path (where `save` writes) and a viewport.
    let start = State::new(
        "hello world\n",
        Some("notes.md".into()),
        Viewport {
            width: 30,
            height: 4,
        },
    );
    let mut state = start.clone();
    let mut trace = vec![TraceLine::State(Box::new(start))];

    // 2. Messages are plain values. The clock arrives as a message too.
    let mut msgs = vec![
        Msg::Tick { now_ms: 1_000 },
        Msg::Move {
            dir: Dir::Forward,
            by: By::Word,
            extend: false,
        },
        Msg::Move {
            dir: Dir::Forward,
            by: By::Word,
            extend: true,
        },
    ];
    // 3. Or keys, through the pure keymap (the same notation as `caretline --keys`).
    msgs.extend(script_to_msgs("<c-c>", 1_000).expect("valid key script"));

    // 4. Apply them one by one. `update` never performs I/O; it returns effects.
    for msg in msgs {
        trace.push(TraceLine::Msg(msg.clone()));
        for effect in update(&mut state, msg) {
            match effect {
                Effect::ClipboardSet { text } => println!("effect: copy {text:?} to the clipboard"),
                Effect::WriteFile { path, .. } => println!("effect: write {path}"),
                Effect::Quit => println!("effect: quit"),
            }
        }
    }

    // 5. Read the result.
    let primary = state.selection.primary();
    println!("text:      {:?}", state.text.to_string());
    println!("selection: anchor {} head {}", primary.anchor, primary.head);
    println!("selected:  {:?}", selection_text(&state));

    // 6. Type over the selection, then save. The runtime performs the write and answers
    //    with `Saved` (or `SaveFailed`), which clears the dirty flag.
    for msg in [
        Msg::InsertText {
            text: " there".into(),
        },
        Msg::Save,
    ] {
        trace.push(TraceLine::Msg(msg.clone()));
        for effect in update(&mut state, msg) {
            if let Effect::WriteFile { path, text } = effect {
                println!("would write {} bytes to {path}", text.len());
                trace.push(TraceLine::Msg(Msg::Saved));
                update(&mut state, Msg::Saved);
            }
        }
    }
    println!("dirty:     {}", state.dirty);

    // 7. Render. `view` is pure: a grid of cells plus the caret's cell.
    let frame = view(&state);
    print!("{}", frame.to_text());
    println!("cursor at  {:?}", frame.cursor);

    // 8. The whole state round-trips through JSON, history included.
    let json = state.to_json();
    let mut back = State::from_json(&json).expect("state parses");
    assert_eq!(back, state);
    update(&mut back, Msg::Undo);
    println!("after undo: {:?}", back.text.to_string());

    // 9. A trace (initial state + every message) replays to the same state.
    let jsonl: String = trace.iter().map(|l| l.to_line() + "\n").collect();
    let (replayed, count) = replay_trace(&jsonl).expect("trace replays");
    assert_eq!(replayed, state);
    println!("replayed {count} messages: same state");
}
