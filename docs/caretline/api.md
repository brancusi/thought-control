# Rust API

The `caretline-next` crate, by task. Every snippet here compiles against the crate on `main`.
The complete program at the end is also in the repo as
[`examples/basic.rs`](../../crates/caretline-next/examples/basic.rs):

```sh
cargo run -p caretline-next --example basic
```

## Add the dependency

caretline-next isn't on crates.io yet. Use a git or path dependency:

```toml
[dependencies]
caretline-next = { git = "https://github.com/brancusi/thought-control" }
# or, in a checkout:
# caretline-next = { path = "../thought-control/crates/caretline-next" }
```

Its dependencies are ropey, smallvec, smartstring, the unicode crates, serde, serde_json and
log. There's no terminal crate and no ratatui.

## The public surface

| Item | Where | Use it to |
|---|---|---|
| `State`, `Config`, `Viewport`, `Scroll` | `caretline_next` | Hold and configure the editor |
| `Msg`, `Dir`, `By`, `Effect` | `caretline_next` | Say what happened; get work back |
| `update`, `replay` | `caretline_next` | Apply one message; fold many |
| `update::selection_text` | `caretline_next::update` | Get the selected text, as a copy would |
| `view`, `Frame` | `caretline_next` | Render to cells |
| `view::{Cell, Role, display_width}` | `caretline_next::view` | Read cells; style them by meaning |
| `keymap`, `Key`, `KeyCode`, `Mods` | `caretline_next` | Map keys to messages |
| `parse_keys`, `script_to_msgs`, `keymap::ScriptItem` | `caretline_next` | Use the `--keys` notation |
| `trace::{TraceLine, parse_msgs, replay_trace}` | `caretline_next::trace` | Record and replay sessions |
| `layout::{Layout, RowPos, text_format, ensure_caret_visible}` | `caretline_next::layout` | Lower-level layout queries |
| `helix::*` | `caretline_next::helix` | Helix's `Selection`, `Range`, `Transaction`, `History`, `Rope`, … |
| `Session`, `protocol::*` | `caretline_next` | A state with a rev and a trace, and the [protocol](protocol.md) in process: see [Session](#session) |
| `Marks`, `Mark`, `MarkId`, `BlockAttrs` | `caretline_next` | Block identity that survives edits: see [Block marks](#block-marks) |
| `marks::{Clipboard, ClipMark, MarkDelta, Fixup, is_line_start}`, `update::mark_only_edit` | `caretline_next::marks`, `::update` | The register with carried marks, the per-revision deltas, a host's undoable mark edit |

## Create a state

From text, with an optional path that `save` writes to, and a viewport in cells (the last row
is the status bar):

```rust
use caretline_next::{State, Viewport};

let state = State::new("# Notes\n", Some("notes.md".into()), Viewport { width: 80, height: 24 });
assert_eq!(state.caret(), 0);
assert!(!state.dirty);
```

From a file. A file that doesn't exist yet is an empty, clean document. The line ending
(LF or CRLF) is detected from the text:

```rust
use caretline_next::{State, Viewport};

fn open(path: &str) -> std::io::Result<State> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    Ok(State::new(&text, Some(path.to_string()), Viewport { width: 80, height: 24 }))
}
```

Change the config directly. It's plain data:

```rust
use caretline_next::{State, Viewport};

let mut state = State::new("a\tb", None, Viewport { width: 80, height: 24 });
state.config.tab_width = 2;
state.config.soft_wrap = false;
state.config.status_bar = false; // every row shows text; no status bar
```

## Apply messages

`update` applies one message and returns the effects:

```rust
use caretline_next::{update, By, Dir, Msg, State, Viewport};

let mut state = State::new("hello world", None, Viewport { width: 40, height: 5 });
update(&mut state, Msg::Move { dir: Dir::Forward, by: By::Word, extend: false });
update(&mut state, Msg::InsertText { text: ",".into() });
assert_eq!(state.text.to_string(), "hello, world");
```

`replay` folds a list and drops the effects:

```rust
use caretline_next::{replay, Msg, State, Viewport};

let mut state = State::new("", None, Viewport { width: 40, height: 5 });
replay(&mut state, [Msg::InsertText { text: "ab".into() }, Msg::DeleteBackward]);
assert_eq!(state.text.to_string(), "a");
```

Keys go through the pure keymap. `script_to_msgs` takes the `--keys` notation
([messages.md](messages.md#key-scripts)) and counts `<wait:MS>` from the time you give it:

```rust
use caretline_next::{keymap, replay, script_to_msgs, Key, KeyCode, Mods, State, Viewport};

let mut state = State::new("hello", None, Viewport { width: 40, height: 5 });
// One key:
let ctrl_e = Key { code: KeyCode::Char('e'), mods: Mods { ctrl: true, ..Mods::default() } };
let msg = keymap(&ctrl_e).expect("Ctrl-E is bound");
// A script:
let mut msgs = vec![msg];
msgs.extend(script_to_msgs(" there<s-a-left>", state.now_ms).unwrap());
replay(&mut state, msgs);
assert_eq!(state.text.to_string(), "hello there");
```

Send `Msg::Tick { now_ms }` with real time before user input if you want typing grouped into
undo steps the way a person expects ([architecture.md](architecture.md#undo-grouping-worked-through)).

## Read the text and the selection

Positions are **char indices** into the rope (Unicode scalar values, not bytes). Every
position `update` produces is on a grapheme boundary.

```rust
use caretline_next::update::selection_text;
use caretline_next::{replay, script_to_msgs, State, Viewport};

let mut state = State::new("one\ntwo three", None, Viewport { width: 40, height: 5 });
replay(&mut state, script_to_msgs("<down><s-a-right>", 0).unwrap());

let text = state.text.slice(..);
let range = state.selection.primary();       // anchor and head
assert_eq!((range.anchor, range.head), (4, 7));
assert_eq!((range.from(), range.to()), (4, 7)); // ordered ends
assert_eq!(range.slice(text).to_string(), "two");
assert_eq!(selection_text(&state).as_deref(), Some("two"));

// Line and column of the caret (0-based):
let line = text.char_to_line(state.caret());
let col = state.caret() - text.line_to_char(line);
assert_eq!((line, col), (1, 3));

// Every range, for multi-range selections:
for r in state.selection.iter() {
    let _ = (r.anchor, r.head);
}
```

### Set the selection yourself

Replace `state.selection` with a Helix `Selection`, then call `sanitize` to snap it to
grapheme boundaries and the text's length:

```rust
use caretline_next::helix::{Range, Selection, SmallVec};
use caretline_next::{update, Msg, State, Viewport};

let mut state = State::new("a-b-c", None, Viewport { width: 40, height: 5 });
// Two carets, after "a" and after "b". The second is primary.
let ranges: SmallVec<[Range; 1]> = [Range::point(1), Range::point(3)].into_iter().collect();
state.selection = Selection::new(ranges, 1);
state.sanitize();
update(&mut state, Msg::InsertText { text: "!".into() });
assert_eq!(state.text.to_string(), "a!-b!-c");
assert_eq!(state.selection.len(), 2);
```

## Render

`view` returns a `Frame`: `height` rows of `width` cells, and the caret's cell when it's on
screen. Each cell has a grapheme `symbol` and a `role`. A wide grapheme takes two cells; the
second has an empty symbol.

```rust
use caretline_next::view::Role;
use caretline_next::{view, State, Viewport};

let state = State::new("hi 漢字", None, Viewport { width: 12, height: 2 });
let frame = view(&state);
assert_eq!(frame.cell(3, 0).symbol, "漢");
assert_eq!(frame.cell(4, 0).symbol, "");          // the second half of 漢
assert_eq!(frame.cell(0, 1).role, Role::Status);  // the last row is the status bar
assert_eq!(frame.cursor, Some((0, 0)));
print!("{}", frame.to_text());                     // or frame.to_ansi()
```

| `Role` | Meaning |
|---|---|
| `Text` | Document text |
| `Selection` | Selected text |
| `Status` | The status bar |
| `StatusAccent` | The dirty marker `[+]` in the status bar |

The engine never picks colours. Your renderer maps roles to styles.

## Handle effects

| Effect | Do this | Then send |
|---|---|---|
| `WriteFile { path, text }` | Write `text` to `path` | `Msg::Saved`, or `Msg::SaveFailed { err }` |
| `ClipboardSet { text }` | Put `text` on the system clipboard | nothing |
| `Quit` | Close the editor | nothing |

```rust
use caretline_next::{update, Effect, Msg, State};

fn dispatch(state: &mut State, msg: Msg) -> bool {
    let mut queue = vec![msg];
    while let Some(msg) = queue.pop() {
        for effect in update(state, msg) {
            match effect {
                Effect::WriteFile { path, text } => queue.push(match std::fs::write(&path, text) {
                    Ok(()) => Msg::Saved,
                    Err(e) => Msg::SaveFailed { err: e.to_string() },
                }),
                Effect::ClipboardSet { text } => { let _ = text; /* your clipboard */ }
                Effect::Quit => return false,
            }
        }
    }
    true
}
```

The pure keymap maps a paste key to `Msg::Paste { text: None }`, which pastes the internal
register. To paste from the system clipboard, read it in your runtime and send
`Msg::Paste { text: Some(..) }`. That way the message records exactly what was pasted.

## Serialize and replay

```rust
use caretline_next::trace::{parse_msgs, replay_trace, TraceLine};
use caretline_next::{update, State, Viewport};

let start = State::new("", None, Viewport { width: 40, height: 5 });

// A state round-trips through JSON, undo history included.
let back = State::from_json(&start.to_json()).unwrap();
assert_eq!(back, start);

// Messages parse from JSON Lines (or a JSON array).
let msgs = parse_msgs(r#"{"msg":"insert_text","text":"hi"}
{"msg":"move","dir":"backward","by":"word","extend":true}"#).unwrap();

// A trace is the initial state, then every message.
let mut live = start.clone();
let mut trace = TraceLine::State(Box::new(start)).to_line() + "\n";
for msg in msgs {
    trace += &(TraceLine::Msg(msg.clone()).to_line() + "\n");
    update(&mut live, msg);
}
let (replayed, count) = replay_trace(&trace).unwrap();
assert_eq!((replayed, count), (live, 2));
```

`State::from_json` repairs hand-edited states (see
[architecture.md](architecture.md#rehydration)). `to_json` is pretty-printed; use
`serde_json::to_string(&state)` for one line.

Deserializing goes through `state::StateInput`, where every field is optional, so a minimal
state works:

```rust
use caretline_next::State;

let s = State::from_json(r#"{"text":"hello\n","viewport":{"width":40,"height":10}}"#).unwrap();
assert!(!s.dirty && s.history.len() == 1); // clean, with a fresh history
```

Undo grouping constants live in `caretline_next::state`: `RUN_GAP_MS` (1500), `RUN_MAX_CHARS`
(256) and `RUN_WORD_BREAK_CHARS` (128). See
[architecture.md](architecture.md#undo-grouping-worked-through).

## Block marks

`state.marks` holds ids at line starts that follow their lines through every edit, undo and
redo (see [architecture.md](architecture.md#block-marks)). Add marks directly, outside the
undo history, when you load a document; use `update::mark_only_edit` for a change of marks
that should be one undo step.

```rust
use caretline_next::{update, BlockAttrs, Msg, State, Viewport};

let mut s = State::new("Groceries\nmilk\n", None, Viewport { width: 40, height: 5 });
let list = s.marks.mint(0);                        // MarkId(0) on line 0
let milk = s.marks.mint(s.text.line_to_char(1));   // MarkId(1) on line 1
s.marks.set_attrs(milk, BlockAttrs { gap: Some(false) });

update(&mut s, Msg::InsertText { text: "Weekly ".into() });  // at the start of line 0
assert_eq!(s.marks.pos(list), Some(0));                      // still line 0
update(&mut s, Msg::Move { dir: caretline_next::Dir::Forward, by: caretline_next::By::DocEnd, extend: false });
update(&mut s, Msg::Undo);                                   // the marks come back exactly
assert_eq!(s.marks.pos(milk), Some(s.text.line_to_char(1)));
```

| `Marks` method | Does |
|---|---|
| `mint(pos)`, `mint_with(pos, attrs)` | A new id at a line start (or the line's existing mark) |
| `insert(Mark)` | Put a known id back; refuses a taken line or a live id |
| `remove(id)`, `remove_at(pos)`, `remove_range(from, to)` | Take marks out, returning them |
| `at(pos)`, `mark_at(pos)`, `pos(id)`, `get(id)`, `in_range(from, to)`, `at_or_before(pos)` | Look marks up |
| `attrs(id)`, `set_attrs(id, attrs)` | A block's attributes |
| `iter()`, `len()`, `next_id()` | Walk them in document order |

## Session

A `Session` wraps a `State` with a revision counter and an in-memory trace. It is the library
face of the [state protocol](protocol.md).

| Method | Does |
|---|---|
| `Session::new(state)` | Starts at rev 0, with the state as the trace's first line |
| `state()`, `rev()`, `trace()`, `trace_jsonl()` | Read the state, the rev and every trace line kept |
| `segment_trace()`, `segment_rev()` | The current segment (from the latest `state` line) and the rev it starts at |
| `trace_since(rev)`, `trace_start_rev()` | The lines after `rev` (`None` once trimmed), and the oldest rev it answers for |
| `checkpoint()` | Starts a new segment with the current state; the rev doesn't change |
| `set_trace_limit(lines)` | Bounds the kept trace (default `session::DEFAULT_TRACE_LIMIT`, 100,000): older segments go first, and a segment that alone outgrows it is cut by an automatic checkpoint |
| `trace_lines_total()`, `trace_lines_from(n)` | Lines recorded so far (dropped ones included) and the lines from absolute position `n`, for copying the trace to a file |
| `apply(msg) -> Vec<Effect>` | Applies one message (rev + 1), returns effects unperformed |
| `apply_all(msgs)` | Applies several |
| `apply_with(msg, exec)` | Applies, performs effects with `exec`, and applies the messages `exec` returns (such as `Saved`) |
| `keys(script)` | Runs a key script; returns the messages and effects |
| `set_state(state)` | Replaces the state (sanitized) and starts a new trace segment; rev + 1 |
| `frame()` | `view` of the current state |
| `render(w, h)` | The frame at another size, without changing the session |
| `handle(line, exec)` | Answers one protocol request line (`protocol::Handled`): the response line, the `Change` it made and any `subscribe` control |
| `handle_at(line, exec, clock_ms)` | `handle` for a runtime with a clock: ticks to `clock_ms` before a request's messages (see [Time](protocol.md#time)) |

`protocol::event_line(&session, &change, &subscription, source)` builds the event a
subscriber receives for a change.

```rust
use caretline_next::{Session, State, Viewport};

let mut s = Session::new(State::new("", None, Viewport { width: 30, height: 4 }));
let (msgs, effects) = s.keys("hi<c-s>").unwrap();
assert_eq!((msgs.len(), s.rev()), (3, 3));
assert!(effects.is_empty()); // no path, so save only sets a status message

let reply = s.handle(r#"{"id":1,"op":"render","w":30,"h":4}"#, None);
println!("{}", reply.response); // {"id":1,"result":{"rev":3,"w":30,…}}
```

## A complete program

This is [`examples/basic.rs`](../../crates/caretline-next/examples/basic.rs). It builds a
state, drives it with messages and keys, handles effects, renders, round-trips the state
through JSON and replays the trace.

```rust
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
    for msg in [Msg::InsertText { text: " there".into() }, Msg::Save] {
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
```

Its output:

```text
effect: copy " world" to the clipboard
text:      "hello world\n"
selection: anchor 5 head 11
selected:  Some(" world")
would write 12 bytes to notes.md
dirty:     false
hello there


 notes.md  saved notes.m 1:12
cursor at  Some((11, 0))
after undo: "hello world\n"
replayed 7 messages: same state
```

The status bar clips its message (`saved notes.md`) so the `line:col` on the right stays
visible.
