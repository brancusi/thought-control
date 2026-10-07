# Embedding caretline

There are two ways to put caretline in your project:

| Way | You get | Best when |
|---|---|---|
| [As a library](#as-a-library) | `State`, `update` and `view` in your Rust process | Your app is in Rust and owns its terminal or window |
| [As a process](#as-a-process) | `caretline serve` speaking JSON lines | Your app is in another language, or you want isolation |

Either way, your app adds what its text means through the [extension points](#extending-the-engine).

Either way, caretline owns the editing rules and you own everything else: input, drawing,
files, the clipboard and the clock.

## As a library

### Add the dependency

caretline is on [crates.io](https://crates.io/crates/caretline): `cargo add caretline`, or

```toml
[dependencies]
caretline = "0.3"
```

Its library is `caretline::`; this repository's crate is the same code. For something on
`main` that is newer than the latest release, use a git dependency on this repository.

The crate has no terminal dependency, so it works
under any renderer.

### Your event loop

You own the loop. Each turn:

1. Turn your input into a `Msg`. For keys, convert to `caretline::Key` and call
   `keymap`; for paste, resize and mouse events, build the message directly.
2. Send `Msg::Tick { now_ms }` with the wall clock, so typing groups into undo steps.
3. Call `update` and perform the effects it returns.
4. Call `view` and copy the cells to your screen. Put your cursor at `frame.cursor`.

```mermaid
flowchart LR
    input["Your input<br/>(keys, mouse, paste)"] -->|"to_key → keymap"| msg["Msg"]
    clock["Wall clock"] -->|"Tick"| upd
    msg --> upd["update(&mut state, msg)"]
    upd -->|"Vec&lt;Effect&gt;"| fx["Your effect handler<br/>(files, clipboard, quit)"]
    fx -->|"Saved / SaveFailed"| upd
    upd --> v["view(&state) → Frame"] --> draw["Your renderer"]
```

### A minimal ratatui embed

A complete editor in one file, with ratatui 0.30 and crossterm 0.29. It types, moves,
selects and undoes; `Ctrl-Q` twice quits and prints the text.

```toml
[dependencies]
caretline = "0.3"
ratatui = "0.30"
crossterm = "0.29"
```

```rust
use std::time::{SystemTime, UNIX_EPOCH};

use caretline::view::Role;
use caretline::{keymap, update, view, Effect, Key, KeyCode, Mods, Msg, State, Viewport};
use crossterm::event::{self, Event, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::Frame;

/// Your key type to caretline's. Keys caretline doesn't know return None.
fn to_key(k: &event::KeyEvent) -> Option<Key> {
    use event::KeyCode as C;
    let code = match k.code {
        C::Char(c) => KeyCode::Char(c),
        C::Enter => KeyCode::Enter,
        C::Backspace => KeyCode::Backspace,
        C::Delete => KeyCode::Delete,
        C::Left => KeyCode::Left,
        C::Right => KeyCode::Right,
        C::Up => KeyCode::Up,
        C::Down => KeyCode::Down,
        C::Home => KeyCode::Home,
        C::End => KeyCode::End,
        C::PageUp => KeyCode::PageUp,
        C::PageDown => KeyCode::PageDown,
        C::Tab => KeyCode::Tab,
        C::Esc => KeyCode::Esc,
        _ => return None,
    };
    let m = k.modifiers;
    let mods = Mods {
        shift: m.contains(KeyModifiers::SHIFT),
        ctrl: m.contains(KeyModifiers::CONTROL),
        alt: m.contains(KeyModifiers::ALT),
        cmd: m.contains(KeyModifiers::SUPER),
    };
    Some(Key { code, mods })
}

/// Copies caretline's frame into a ratatui area. The status row is the frame's last row.
fn draw_editor(f: &mut Frame, area: Rect, state: &State) {
    let frame = view(state);
    let buf = f.buffer_mut();
    for y in 0..frame.height.min(area.height) {
        for x in 0..frame.width.min(area.width) {
            let cell = frame.cell(x, y);
            if cell.symbol.is_empty() {
                continue; // the second half of a wide grapheme
            }
            let style = match cell.role {
                Role::Text => Style::default(),
                Role::Selection => Style::default().add_modifier(Modifier::REVERSED),
                Role::Status | Role::StatusAccent => Style::default().add_modifier(Modifier::DIM),
            };
            buf.set_string(area.x + x, area.y + y, &cell.symbol, style);
        }
    }
    if let Some((x, y)) = frame.cursor {
        f.set_cursor_position((area.x + x, area.y + y));
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn main() -> std::io::Result<()> {
    let mut terminal = ratatui::init();
    let size = terminal.size()?;
    let mut state = State::new("Edit me.\n", None, Viewport { width: size.width, height: size.height });

    'outer: loop {
        terminal.draw(|f| draw_editor(f, f.area(), &state))?;
        let msg = match event::read()? {
            Event::Key(k) if k.kind == KeyEventKind::Press => match to_key(&k).and_then(|k| keymap(&k)) {
                Some(msg) => msg,
                None => continue,
            },
            Event::Paste(text) => Msg::Paste { text: Some(text) },
            Event::Resize(width, height) => Msg::Resize { width, height },
            _ => continue,
        };
        update(&mut state, Msg::Tick { now_ms: now_ms() });
        for effect in update(&mut state, msg) {
            match effect {
                Effect::Quit => break 'outer,
                Effect::ClipboardSet { .. } => {} // hand it to your clipboard
                Effect::WriteFile { .. } => {}    // write it, then send Msg::Saved
            }
        }
    }
    ratatui::restore();
    println!("{}", state.doc.text);
    Ok(())
}
```

For a fuller runtime (mouse, the system clipboard, atomic saves, kitty keyboard flags,
traces), read the `caretline` binary's
[`runtime.rs`](../../crates/caretline-app/src/runtime.rs).

For your own renderer:

- The frame's size is the state's viewport. When your area changes size, send
  `Msg::Resize`; `update` keeps the caret in view.
- The last row of every frame is caretline's status bar (file name, `[+]`, messages,
  `line:col`). There's no option to turn it off yet; if you don't want it, give the state one
  extra row and don't copy the last one.
- Mouse cells map directly: send `Msg::Click { col, row, extend }` with coordinates relative
  to your area, and `Msg::Scroll { rows }` for the wheel.

### Panels: several independent editors

Each `State` is a whole editor with its own text, selection, undo history and viewport. For
several panels, keep several states. Route input to the focused one and give each its own
area.

```rust
use caretline::{update, Effect, Msg, State, Viewport};
use ratatui::layout::{Constraint, Layout, Rect};

/// One editor per panel. Each has its own text, selection, undo history and viewport.
struct Panels {
    editors: Vec<State>,
    focus: usize,
    clipboard: String, // shared between panels
}

impl Panels {
    fn new(texts: &[&str]) -> Panels {
        let vp = Viewport { width: 1, height: 1 }; // set by layout() before the first draw
        Panels {
            editors: texts.iter().map(|t| State::new(t, None, vp)).collect(),
            focus: 0,
            clipboard: String::new(),
        }
    }

    /// Splits the screen and tells each editor its size.
    fn layout(&mut self, area: Rect) -> Vec<Rect> {
        let n = self.editors.len() as u32;
        let rects = Layout::horizontal((0..n).map(|_| Constraint::Ratio(1, n))).split(area);
        for (state, r) in self.editors.iter_mut().zip(rects.iter()) {
            if (state.view.viewport.width, state.view.viewport.height) != (r.width, r.height) {
                update(state, Msg::Resize { width: r.width, height: r.height });
            }
        }
        rects.to_vec()
    }

    /// Input goes to the focused editor only. Paste uses the shared clipboard.
    fn send(&mut self, msg: Msg) {
        let msg = match msg {
            Msg::Paste { text: None } => Msg::Paste { text: Some(self.clipboard.clone()) },
            m => m,
        };
        for effect in update(&mut self.editors[self.focus], msg) {
            if let Effect::ClipboardSet { text } = effect {
                self.clipboard = text;
            }
        }
    }
}
```

Draw each panel with `draw_editor(f, rects[i], &panels.editors[i])` from the example above.

- **The clipboard register is per state.** To share copy and paste between panels, keep
  the text from `ClipboardSet` yourself and paste it with `Paste { text: Some(..) }`, as
  above.
- **Saving and undo are per state.** Give each state its own `path`.
- **Two panels on the same document** are one `Document` with a `View` each, driven with
  `update_doc`: an edit in one maps the other's selection, and undo is the document's. See
  [api.md](api.md#several-views-of-one-document).
- **Persist the layout** by saving each state with `to_json`. Reopening restores the text,
  the caret, the scroll and the undo history.

## Extending the engine

caretline edits text and knows its shape (blocks, depth, markers), never its meaning. What a
line *means* in your app (a task, a ticket, a status) you add through four extension points,
registered on a `Host` and set on the document:

| Point | Register | Runs |
|---|---|---|
| [Host commands](#host-commands) | `Host::command(name, f)` | `Msg::Command { name, args }`: one transaction, one undo step |
| [Input rules](#input-rules) | `Host::input_rule(name, f)` | Before the engine handles an editing message; the first to return an edit takes it |
| [Mark payloads](#mark-payloads) | (data, not code) | Carried with each block's mark |
| [Decorations](#decorations) | `Host::decorator(f)` | When a view with an outline layout is drawn or hit-tested |

```rust
use caretline::{Edit, Host, State};

let host = Host::new()
    .command("shout", |ctx, _args| {
        let line = ctx.text().char_to_line(ctx.caret());
        let (a, b) = (ctx.text().line_to_char(line), ctx.text().line_to_char(line + 1) - 1);
        Ok(Edit { changes: vec![(a, b, ctx.text().slice(a..b).to_string().to_uppercase())], ..Edit::default() })
    });
let mut state = State::new("hello\nworld\n", None, caretline::Viewport { width: 40, height: 6 });
state.doc.set_host(host);
caretline::update(&mut state, caretline::Msg::Command { name: "shout".into(), args: serde_json::Value::Null });
assert_eq!(state.doc.text.to_string(), "HELLO\nworld\n");
```

**Every extension is a pure function**: no clock, randomness or I/O, the same answer for the
same document, view and arguments. That is what keeps the engine's promises:

- **State** stays one serializable value. The host is not part of it (it compares equal to any
  other host and is never serialized); a state read from JSON has none until you `set_host`.
- **Replay** stays exact. A command is a message, so traces record it; replay a trace that uses
  commands with `trace::replay_trace_with(input, &host)`, registering the same functions.
- **The protocol** keeps working: `msgs` can send `{"msg":"command","name":…,"args":…}`,
  `hello` and `commands.list` name the registered commands, and `state.set` keeps the session's
  host.

### Host commands

A command gets a `Ctx` (the document and the view it acts through: `text()`, `blocks()`,
`selection()`, `caret()`, `mapped_selection(changes)`) and the message's JSON `args`, and
returns an `Edit` or the reason it can't run (shown in the status bar, nothing changed):

| `Edit` field | Meaning |
|---|---|
| `changes` | `[from, to)` replaced by text, in chars of the current text, sorted and apart |
| `selection` | The selection after, in the new text (none: the old one mapped through the changes) |
| `marks` | `MarkOp`s applied after the text: `Mint { pos, attrs }`, `Remove { id }`, `SetGap { id, gap }`, `SetData { id, data }` |
| `status` | A one-line message for the view |
| `effects` | Your own effects, returned from `update` as `Effect::Host { name, data }` |
| `keep_gaps` | Every block keeps its blank row (a change of shape never moves another block) |

An unknown name changes nothing and says so; a read-only view refuses a command like any edit.
Bind keys to commands in your app's keymap: caretline's [default keymap](keys.md) binds only
its own vocabulary.

### Input rules

An input rule sees each editing message (typing, Enter, Backspace, paste…) before the engine
and may return an `Edit` to apply instead, as CodeMirror's input handlers do. Use one for a
shorthand typed in the text. caretline has no transaction filters: the structure a host needs
enforced (a prefix the caret never enters) is data, [tags](markdown.md#tags).

### Mark payloads

Each block's mark carries `MarkAttrs { gap, data }`; `data` is any JSON value, yours. caretline
never reads it and keeps it with the mark through edits, cut and paste, undo and redo, and
changes from elsewhere. Set it with `MarkOp::SetData` in a command (undoable),
`ExtChange::SetData` from elsewhere, or on `doc.marks` directly. Or keep your data in your own
map keyed by `MarkId`, as thc does.

### Decorations

A decorator returns, for each block, a `Decoration { hang, gutter }` of `Deco { text, role, id }`.
caretline draws the text in the slot; the cells carry your role's name
(`Frame::role_name`); `view::hit` reports the `id` under a click. See
[structure.md](structure.md#decorations).

## Case study: tasks in thc

thc (Thought Control) is a notes app built on caretline, and its pages have tasks: `- [ ] Pay
rent`, ⌃T to cycle text → open → done → text, a click on the box to complete it, a save the
moment a task is done. None of that is in caretline. Here is how thc builds it from the
extension points, and how you would build anything like it.

**1. The syntax, as data.** A task is a bullet with a one-character tag. thc turns tags on for
its statuses, and has Enter after a task open a new one:

```rust
let config = OutlineConfig { tags: " x/w-".into(), new_tag: Some(' '), ..OutlineConfig::default() };
```

caretline now keeps the caret out of `[x] `, draws it in the hang, and has Backspace remove it
before the bullet, without knowing that `x` means done. thc maps tag characters to its own
status names (`' '` todo, `x` done, `/` doing, `w` waiting, `-` cancelled).

**2. The cycle, as a command.** ⌃T is a thc key bound to a thc command. The command reads the
blocks the selection touches and rewrites their prefixes with plain text changes:

```rust
fn task_cycle(ctx: &Ctx, _: &Value) -> Result<Edit, String> {
    let o = ctx.blocks().ok_or("only in outline documents")?;
    let b = o.block_at(ctx.text(), ctx.caret());
    let at = b.start + b.indent;
    let (changes, done) = match b.tag {
        Some('x') => (vec![(b.start, b.content_start(), String::new())], false), // done → text
        Some(_) => (vec![(at + 3, at + 4, "x".to_string())], true),             // open → done
        None if b.kind == Kind::Para => (vec![(at, at, "- [ ] ".to_string())], false),
        None => (vec![(at, b.content_start(), "- [ ] ".to_string())], false),    // bullet → open
    };
    Ok(Edit {
        selection: Some(ctx.mapped_selection(&changes)),
        changes,
        keep_gaps: true,
        effects: if done { vec![("thc.completed".into(), json!({ "id": b.id.0 }))] } else { vec![] },
        ..Edit::default()
    })
}
```

thc's real one also applies to every selected block, splits a multi-line paragraph into tasks
(minting marks with `MarkOp::Mint`) and joins them back. It is one undo step, it replays, and
its `thc.completed` effect tells thc to save at once.

**3. The box, as a decoration.** thc's decorator draws `[ ]` or `[x]` in the hang with a role
per status (`"thc.task.done"`) and the id `"box"`; a click whose `hit` is
`Hang { deco: Some("box"), block }` sends `Msg::Command { name: "thc.set_status", args: {"id": block, "status": "x"} }`.

**4. The shorthand, as an input rule.** Typing `[ ] ` at a paragraph line's start becomes
`- [ ] `: an input rule that matches the typed space and returns that edit.

**5. The rest stays in thc.** Due dates, priorities and the vault: thc keeps them per block in its
own map keyed by `MarkId`, sends changes from the vault as `Msg::External`, and draws its own
surface. caretline never learns the word "task".

**6. What thc keeps beside the engine, and why.** caretline owns the text, every block's shape,
the selection, folds, undo and the editing rules; thc reads all of them from the engine and
keeps no copy of the caret or the history. Beside each block it keeps one `Line`, tied to the
block's mark, for what only thc knows:

- **The vault node id.** A block is a node in thc's vault, and its id must outlive what a mark
  doesn't: a block cut and pasted back, a join undone after the delete was saved (then it's a
  new node under a new id), a page reopened. The `Line` holds it; lines whose marks go wait in
  a graveyard so an undo that brings the mark back brings the node back.
- **The save state.** The revision an edit is based on, the text, kind, parent and place as
  last saved, a save in flight, a conflict. thc's save is a diff of the lines against that
  state, sent to the vault as one transaction of block ops.
- **A read-only copy of the block's shape and text,** re-read after every engine step, so the
  save diff and the drawing compare plain strings without walking the rope.

thc changes lines only for changes from elsewhere (a refresh from the vault, a save's parsed
tokens, recovered text); each goes into the engine at once as `Msg::External` (or, for
recovered text, one undoable step), so the engine is the only place the document lives.
New blocks get node ids from an id pool the runtime fills before input: `update` stays pure
(no clock, no randomness), so a session replays to the same ids.

## As a process

Spawn `caretline serve`, write JSON requests to its stdin and read one JSON response per
line from its stdout. Any language that can run a process works.

```python
import json, subprocess

class Caretline:
    """A caretline engine in a child process, spoken to in JSON lines."""

    def __init__(self, *args):
        self.proc = subprocess.Popen(
            ["caretline", "serve", *args],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
        )
        self.next_id = 0

    def call(self, op, **fields):
        self.next_id += 1
        req = {"id": self.next_id, "op": op, **fields}
        self.proc.stdin.write(json.dumps(req) + "\n")
        self.proc.stdin.flush()
        resp = json.loads(self.proc.stdout.readline())
        if "error" in resp:
            raise RuntimeError(f"{resp['error']['kind']}: {resp['error']['message']}")
        return resp["result"]

    def close(self):
        self.proc.stdin.close()
        self.proc.wait()

ed = Caretline("py.md", "--size", "30x4")
print(ed.call("hello")["proto"])
rev = ed.call("keys", keys="<d-down>written from Python")["rev"]

# Save: the engine returns a write_file effect; performing it is your job.
for effect in ed.call("msgs", msgs=[{"msg": "save"}], if_rev=rev)["effects"]:
    if effect["effect"] == "write_file":
        with open(effect["path"], "w") as f:
            f.write(effect["text"])
        ed.call("msgs", msgs=[{"msg": "saved"}])
print(ed.call("render")["frame"], end="")
ed.close()
```

With `py.md` containing `hello world`, this prints:

```text
1
hello world
written from Python

 py.md  saved py.md      2:20
```

- Don't `subscribe` on a connection you read like this: events arrive between responses.
  Use a second connection on a socket (`serve --socket`) for events.
- To show the editor in your UI, ask for `render` with `format: "cells"` and draw the rows
  and spans yourself.
- One `serve` process is one editor. For several panels, start several.
- `serve --no-status-bar` gives every row to text, for a panel that draws its own chrome.
- `serve` ticks to the real time before each request, so typing groups into undo steps by
  time. Pass `now_ms` on a request, or start `serve --no-clock`, to control time yourself.

## Licensing

| Code | License |
|---|---|
| `caretline`, except `src/helix/` | MIT |
| its `src/helix/` (vendored from Helix) | MPL-2.0, per file |
| `caretline-cli` | MIT |

MPL-2.0 is a **file-level** copyleft. In practice, for an embedder:

- **Your own code can use any license**, open or closed. Linking caretline into your
  program doesn't change your files' license.
- **The vendored Helix files stay MPL-2.0.** If you distribute a program that contains them,
  you must make the source of those files available, including any changes you make to them,
  under MPL-2.0. Unchanged files are already public in this repository and upstream.
- **Keep the notices.** Each vendored file names its upstream source and its changes, and
  `src/helix/LICENSE-MPL-2.0` holds the license text.
- The crate's `license` field is `MIT AND MPL-2.0`, so license scanners see both.

This is a summary, not legal advice. Read the
[MPL-2.0 FAQ](https://www.mozilla.org/en-US/MPL/2.0/FAQ/) if it matters for your project.
