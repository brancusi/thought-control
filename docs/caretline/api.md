# Rust API

The `caretline` crate, by task. Every snippet here compiles against the crate on `main`.
The complete program at the end is also in the repo as
[`examples/basic.rs`](../../crates/caretline-next/examples/basic.rs):

```sh
cargo run -p caretline-next --example basic
```

## Add the dependency

caretline is on [crates.io](https://crates.io/crates/caretline):

```sh
cargo add caretline
```

```toml
[dependencies]
caretline = "0.2"
```

Its library is `caretline::`. Inside this repository the crate is still named
`caretline-next` (and imported as `caretline_next::`) until the older engine is removed; the
published crate is the same code. A few things on `main` are newer than the latest release;
for those, use a git dependency (`caretline-next = { git = "https://github.com/brancusi/thought-control" }`).

Its dependencies are ropey, smallvec, smartstring, the unicode crates, serde, serde_json and
log. There's no terminal crate and no ratatui.

## The public surface

| Item | Where | Use it to |
|---|---|---|
| `State`, `Config`, `Viewport`, `Scroll` | `caretline` | Hold and configure the editor: one document and one view |
| `Document`, `View`, `ViewConfig`, `Follow`, `ExternalUndo` | `caretline` | A document and its views, separately: see [Several views](#several-views-of-one-document) |
| `update_doc` | `caretline` | Apply a message through one of several views |
| `ExtChange` | `caretline` | A change from elsewhere, for `Msg::External`: see [messages.md](messages.md#changes-from-elsewhere) |
| `Msg`, `Dir`, `By`, `Effect` | `caretline` | Say what happened; get work back |
| `update`, `replay` | `caretline` | Apply one message; fold many |
| `update::selection_text` | `caretline::update` | Get the selected text, as a copy would |
| `view`, `Frame` | `caretline` | Render to cells |
| `view::{render, hit, Cell, Role, RowInfo, Hit, display_width}` | `caretline::view` | Render any view; read cells and rows; style them by meaning; hit-test a cell |
| `OutlineLayout`, `views::hidden_lines` | `caretline` | The [outline layout](outline.md#the-outline-layout) and folds |
| `keymap`, `Key`, `KeyCode`, `Mods` | `caretline` | Map keys to messages |
| `parse_keys`, `script_to_msgs`, `keymap::ScriptItem` | `caretline` | Use the `--keys` notation |
| `trace::{TraceLine, parse_msgs, replay_trace, replay_trace_views}` | `caretline::trace` | Record and replay sessions (with their views) |
| `layout::{Layout, LineFormat, RowPos, text_format, ensure_caret_visible}` | `caretline::layout` | Lower-level layout queries |
| `helix::*` | `caretline::helix` | Helix's `Selection`, `Range`, `Transaction`, `History`, `Rope`, … |
| `Session`, `protocol::*` | `caretline` | A state with a rev and a trace, and the [protocol](protocol.md) in process: see [Session](#session) |
| `Marks`, `Mark`, `MarkId`, `BlockAttrs` | `caretline` | Block identity that survives edits: see [Block marks](#block-marks) |
| `marks::{Clipboard, ClipMark, MarkDelta, Fixup, is_line_start}`, `update::mark_only_edit` | `caretline::marks`, `::update` | The register with carried marks, the per-revision deltas, a host's undoable mark edit |
| `OutlineConfig`, `Outline`, `BlockInfo`, `Kind`, `NewBlock`, `outline::{markdown, derive, content, Hang}` | `caretline`, `::outline` | Outline documents: blocks, lists and tasks over the same buffer. See [outline.md](outline.md#in-rust) |
| `outline_keymap`, `keymap_for`, `script_to_msgs_for` | `caretline` | The outline's keys |

## Create a state

From text, with an optional path that `save` writes to, and a viewport in cells (the last row
is the status bar):

```rust
use caretline::{State, Viewport};

let state = State::new("# Notes\n", Some("notes.md".into()), Viewport { width: 80, height: 24 });
assert_eq!(state.caret(), 0);
assert!(!state.doc.dirty);
```

From a file. A file that doesn't exist yet is an empty, clean document. The line ending
(LF or CRLF) is detected from the text:

```rust
use caretline::{State, Viewport};

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
use caretline::{State, Viewport};

let mut state = State::new("a\tb", None, Viewport { width: 80, height: 24 });
state.doc.config.tab_width = 2;
state.doc.config.soft_wrap = false;
state.view.config.status_bar = false; // every row shows text; no status bar
```

## Apply messages

`update` applies one message and returns the effects:

```rust
use caretline::{update, By, Dir, Msg, State, Viewport};

let mut state = State::new("hello world", None, Viewport { width: 40, height: 5 });
update(&mut state, Msg::Move { dir: Dir::Forward, by: By::Word, extend: false });
update(&mut state, Msg::InsertText { text: ",".into() });
assert_eq!(state.doc.text.to_string(), "hello, world");
```

`replay` folds a list and drops the effects:

```rust
use caretline::{replay, Msg, State, Viewport};

let mut state = State::new("", None, Viewport { width: 40, height: 5 });
replay(&mut state, [Msg::InsertText { text: "ab".into() }, Msg::DeleteBackward]);
assert_eq!(state.doc.text.to_string(), "a");
```

Keys go through the pure keymap. `script_to_msgs` takes the `--keys` notation
([messages.md](messages.md#key-scripts)) and counts `<wait:MS>` from the time you give it:

```rust
use caretline::{keymap, replay, script_to_msgs, Key, KeyCode, Mods, State, Viewport};

let mut state = State::new("hello", None, Viewport { width: 40, height: 5 });
// One key:
let ctrl_e = Key { code: KeyCode::Char('e'), mods: Mods { ctrl: true, ..Mods::default() } };
let msg = keymap(&ctrl_e).expect("Ctrl-E is bound");
// A script:
let mut msgs = vec![msg];
msgs.extend(script_to_msgs(" there<s-a-left>", state.doc.now_ms).unwrap());
replay(&mut state, msgs);
assert_eq!(state.doc.text.to_string(), "hello there");
```

Send `Msg::Tick { now_ms }` with real time before user input if you want typing grouped into
undo steps the way a person expects ([architecture.md](architecture.md#undo-grouping-worked-through)).

## Read the text and the selection

Positions are **char indices** into the rope (Unicode scalar values, not bytes). Every
position `update` produces is on a grapheme boundary.

```rust
use caretline::update::selection_text;
use caretline::{replay, script_to_msgs, State, Viewport};

let mut state = State::new("one\ntwo three", None, Viewport { width: 40, height: 5 });
replay(&mut state, script_to_msgs("<down><s-a-right>", 0).unwrap());

let text = state.doc.text.slice(..);
let range = state.view.selection.primary();       // anchor and head
assert_eq!((range.anchor, range.head), (4, 7));
assert_eq!((range.from(), range.to()), (4, 7)); // ordered ends
assert_eq!(range.slice(text).to_string(), "two");
assert_eq!(selection_text(&state).as_deref(), Some("two"));

// Line and column of the caret (0-based):
let line = text.char_to_line(state.caret());
let col = state.caret() - text.line_to_char(line);
assert_eq!((line, col), (1, 3));

// Every range, for multi-range selections:
for r in state.view.selection.iter() {
    let _ = (r.anchor, r.head);
}
```

### Set the selection yourself

Replace `state.view.selection` with a Helix `Selection`, then call `sanitize` to snap it to
grapheme boundaries and the text's length:

```rust
use caretline::helix::{Range, Selection, SmallVec};
use caretline::{update, Msg, State, Viewport};

let mut state = State::new("a-b-c", None, Viewport { width: 40, height: 5 });
// Two carets, after "a" and after "b". The second is primary.
let ranges: SmallVec<[Range; 1]> = [Range::point(1), Range::point(3)].into_iter().collect();
state.view.selection = Selection::new(ranges, 1);
state.sanitize();
update(&mut state, Msg::InsertText { text: "!".into() });
assert_eq!(state.doc.text.to_string(), "a!-b!-c");
assert_eq!(state.view.selection.len(), 2);
```

## Render

`view` returns a `Frame`: `height` rows of `width` cells, and the caret's cell when it's on
screen. Each cell has a grapheme `symbol` and a `role`. A wide grapheme takes two cells; the
second has an empty symbol.

```rust
use caretline::view::Role;
use caretline::{view, State, Viewport};

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
| `Hang` | An outline block's hang, with the [outline layout](outline.md#the-outline-layout) |

The engine never picks colours. Your renderer maps roles to styles. Each cell also has the
`char_idx` of the document char it shows (none for blank cells and the status bar), and the
frame has one `RowInfo` per row: what the row shows (a line's text with its char range, a
block's blank row, a host's row, past the end, the status bar). `view::hit(doc, view, col,
row)` says what a cell means for a click.

## Handle effects

| Effect | Do this | Then send |
|---|---|---|
| `WriteFile { path, text }` | Write `text` to `path` | `Msg::Saved`, or `Msg::SaveFailed { err }` |
| `ClipboardSet { text }` | Put `text` on the system clipboard | nothing |
| `Quit` | Close the editor | nothing |

```rust
use caretline::{update, Effect, Msg, State};

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
                _ => {} // notices, outline effects, refused, and kinds added later
            }
        }
    }
    true
}
```

The pure keymap maps a paste key to `Msg::Paste { text: None }`, which pastes the internal
register. To paste from the system clipboard, read it in your runtime and send
`Msg::Paste { text: Some(..) }`. That way the message records exactly what was pasted.

`Effect` is `#[non_exhaustive]`: keep a wildcard arm.

## Several views of one document

A `State` is one `Document` (text, marks, undo, outline) and one `View` (selection, scroll,
viewport, folds). To show one document in several places (a main editor and a side panel, or a
person's caret and an agent's), keep one `Document` and several `View`s, and apply messages
with `update_doc`. An edit through one view maps every other view's selection; undo is the
document's; a read-only view is refused edits.

```rust
use caretline::view::render;
use caretline::{update_doc, Effect, ExtChange, Msg, State, View, Viewport};

let mut doc = State::new("one\ntwo\n", None, Viewport { width: 40, height: 5 }).doc;
let mut views = [View::new(Viewport { width: 40, height: 5 }), View::new(Viewport { width: 20, height: 3 }).read_only(true)];
views[1].selection = caretline::helix::Selection::point(4);    // on "two"
update_doc(&mut doc, &mut views, 0, Msg::InsertText { text: "zero\n".into() });
assert_eq!(views[1].caret(), 9);                                    // still on "two"
assert_eq!(update_doc(&mut doc, &mut views, 1, Msg::DeleteBackward), vec![Effect::Refused]);

// A change from elsewhere maps every view and stays out of undo.
update_doc(&mut doc, &mut views, 0, Msg::External { changes: vec![ExtChange::Replace { from: 0, to: 0, text: "> ".into() }] });
update_doc(&mut doc, &mut views, 0, Msg::Undo);
assert_eq!(doc.text.to_string(), "> one\ntwo\n");
let _panel = render(&doc, &views[1]);
```

`State::from_parts(doc, view)` and `state.into_parts()` move between the two forms. A view kept
apart from `update_doc` misses the rebases; `View::fit(&doc)` at least clamps it to the
document.

## Serialize and replay

```rust
use caretline::trace::{parse_msgs, replay_trace, TraceLine};
use caretline::{update, State, Viewport};

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
use caretline::State;

let s = State::from_json(r#"{"text":"hello\n","viewport":{"width":40,"height":10}}"#).unwrap();
assert!(!s.doc.dirty && s.doc.history.len() == 1); // clean, with a fresh history
```

Undo grouping constants live in `caretline::state`: `RUN_GAP_MS` (1500), `RUN_MAX_CHARS`
(256) and `RUN_WORD_BREAK_CHARS` (128). See
[architecture.md](architecture.md#undo-grouping-worked-through).

## Block marks

`state.doc.marks` holds ids at line starts that follow their lines through every edit, undo and
redo (see [architecture.md](architecture.md#block-marks)). Add marks directly, outside the
undo history, when you load a document; use `update::mark_only_edit` for a change of marks
that should be one undo step.

```rust
use caretline::{update, BlockAttrs, Msg, State, Viewport};

let mut s = State::new("Groceries\nmilk\n", None, Viewport { width: 40, height: 5 });
let list = s.doc.marks.mint(0);                        // MarkId(0) on line 0
let milk = s.doc.marks.mint(s.doc.text.line_to_char(1));   // MarkId(1) on line 1
s.doc.marks.set_attrs(milk, BlockAttrs { gap: Some(false) });

update(&mut s, Msg::InsertText { text: "Weekly ".into() });  // at the start of line 0
assert_eq!(s.doc.marks.pos(list), Some(0));                      // still line 0
update(&mut s, Msg::Move { dir: caretline::Dir::Forward, by: caretline::By::DocEnd, extend: false });
update(&mut s, Msg::Undo);                                   // the marks come back exactly
assert_eq!(s.doc.marks.pos(milk), Some(s.doc.text.line_to_char(1)));
```

| `Marks` method | Does |
|---|---|
| `mint(pos)`, `mint_with(pos, attrs)` | A new id at a line start (or the line's existing mark) |
| `insert(Mark)` | Put a known id back; refuses a taken line or a live id |
| `remove(id)`, `remove_at(pos)`, `remove_range(from, to)` | Take marks out, returning them |
| `at(pos)`, `mark_at(pos)`, `pos(id)`, `get(id)`, `in_range(from, to)`, `at_or_before(pos)` | Look marks up |
| `attrs(id)`, `set_attrs(id, attrs)` | A block's attributes |
| `iter()`, `len()`, `next_id()` | Walk them in document order |

## Outline documents

`markdown::load` opens Markdown as an [outline document](outline.md); `state.enable_outline`
turns an existing state into one. `state.blocks()` gives the derived blocks, each with its
mark id. `save` writes Markdown back.

```rust
use caretline::outline::markdown;
use caretline::{update, By, Dir, Msg, OutlineConfig, Viewport};

let mut s = markdown::load("- [ ] Pay rent\n", None, Viewport { width: 40, height: 6 }, OutlineConfig::default());
update(&mut s, Msg::Move { dir: Dir::Forward, by: By::LineEnd, extend: false });
update(&mut s, Msg::InsertNewline);                 // a new open task below
update(&mut s, Msg::InsertText { text: "Call Ana".into() });
update(&mut s, Msg::Indent);                        // nested under the first
assert_eq!(markdown::to_file(&s), "- [ ] Pay rent\n  - [ ] Call Ana\n");
```

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
| `open_view(view) -> id`, `close_view(id)`, `views()`, `view(id)`, `state_of(id)` | Other views of the document (view 0 is the state's own); opening and closing is a change and is traced |
| `apply_on(id, msg)`, `apply_with_on`, `keys_on(id, script)` | Apply through view `id`; every other view is rebased |
| `render_view(id, size)` | The frame of view `id` |
| `state_lines()` | The lines a segment starts with: the state and a `view_open` for each other view |
| `frame()` | `view` of the current state |
| `render(w, h)` | The frame at another size, without changing the session |
| `handle(line, exec)` | Answers one protocol request line (`protocol::Handled`): the response line, the `Change` it made and any `subscribe` control |
| `handle_at(line, exec, clock_ms)` | `handle` for a runtime with a clock: ticks to `clock_ms` before a request's messages (see [Time](protocol.md#time)) |

`protocol::event_line(&session, &change, &subscription, source)` builds the event a
subscriber receives for a change.

```rust
use caretline::{Session, State, Viewport};

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
use caretline::trace::{replay_trace, TraceLine};
use caretline::update::selection_text;
use caretline::{script_to_msgs, update, view, By, Dir, Effect, Msg, State, Viewport};

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
                // Outline documents also report notices, completed tasks and block changes.
                other => println!("effect: {other:?}"),
            }
        }
    }

    // 5. Read the result.
    let primary = state.view.selection.primary();
    println!("text:      {:?}", state.doc.text.to_string());
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
    println!("dirty:     {}", state.doc.dirty);

    // 7. Render. `view` is pure: a grid of cells plus the caret's cell.
    let frame = view(&state);
    print!("{}", frame.to_text());
    println!("cursor at  {:?}", frame.cursor);

    // 8. The whole state round-trips through JSON, history included.
    let json = state.to_json();
    let mut back = State::from_json(&json).expect("state parses");
    assert_eq!(back, state);
    update(&mut back, Msg::Undo);
    println!("after undo: {:?}", back.doc.text.to_string());

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
