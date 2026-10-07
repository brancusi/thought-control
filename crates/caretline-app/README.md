# caretline

A terminal text editor for plain text and Markdown, built on the
[`caretline-next`](../caretline-next/README.md) engine. Every state can be saved as JSON,
reopened exactly, and every session replayed message by message.

```sh
cargo run -p caretline-app -- notes.md         # or, installed: caretline notes.md
cargo install --locked --path crates/caretline-app
```

Full documentation: [the `caretline` command](../../docs/caretline/cli.md), and
[docs/caretline](../../docs/caretline/README.md) for the engine, its API and embedding.

## How it works

The engine is an Elm architecture around Helix's editing core:

- **`State`** holds everything that decides the screen and the behaviour: the text, the
  selections (anchor and head per range, plus the goal column for up and down), the scroll
  position, the viewport size, the clipboard register, the undo history, the dirty flag, the
  file path and the config (tab width, soft wrap).
- **`Msg`** is every input: key intents (`insert_text`, `move`, `undo`, …), `resize`, `click`,
  `scroll`, `tick` (the clock) and results from the runtime (`saved`, `save_failed`).
- **`update(state, msg)`** is pure. It reads no clock (time arrives in `tick`), uses no
  randomness and does no I/O. It returns **effects** as values: `write_file`,
  `clipboard_set`, `quit`. The binary performs them and feeds results back as messages.
- **`view(state)`** is pure and returns a cell grid; the binary copies it to the terminal.
- **The keymap** is a pure function from a key to an optional message.

So a session is an initial state plus a list of messages, and replaying them gives the same
state and the same frame.

## Keys

macOS text-field keys, with Ctrl twins for terminals that don't forward Cmd. Cmd (⌘) needs a
terminal that speaks the kitty keyboard protocol (kitty, WezTerm, Ghostty, iTerm2 with the
option on); caretline turns it on when the terminal supports it and works without it.

| Action | Keys |
|---|---|
| Move by character / row | `←` `→` `↑` `↓` |
| Move by word | `⌥←` `⌥→`, `Ctrl-←` `Ctrl-→`, `Alt-B` `Alt-F` |
| Row start / end (the visual row when wrapped) | `Home` `End`, `⌘←` `⌘→`, `Ctrl-A` `Ctrl-E` |
| Document start / end | `⌘↑` `⌘↓`, `Ctrl-Home` `Ctrl-End` |
| Page up / down | `PgUp` `PgDn` |
| Extend the selection | any motion with `Shift` |
| Collapse the selection | `Esc` |
| Select all | `⌘A`, `Alt-A` |
| Delete back / forward | `Backspace` `Delete` |
| Delete word back / forward | `⌥Backspace`, `Ctrl-W`, `Ctrl-Backspace` / `⌥Delete`, `Alt-D` |
| Delete to row start / end | `⌘Backspace`, `Ctrl-U` / `⌘Delete` |
| Delete to the line end (then the line break) | `Ctrl-K` |
| Copy / cut / paste | `⌘C` `⌘X` `⌘V`, `Ctrl-C` `Ctrl-X` `Ctrl-V` (and the terminal's own paste) |
| Undo / redo | `⌘Z` `⇧⌘Z`, `Ctrl-Z` `Ctrl-Shift-Z` `Ctrl-Y` `Ctrl-R` |
| Save / quit | `⌘S` `⌘Q`, `Ctrl-S` `Ctrl-Q` (quit twice to discard unsaved changes) |
| Mouse | click places the caret, Shift-click and drag extend, the wheel scrolls (`--no-mouse` turns this off) |

The rules follow a macOS text field: a motion without Shift collapses a selection to its edge
instead of moving from the caret (`←` goes to the start, `→` to the end, `↑` and `↓` start
from the start or end); a word or line delete with a selection deletes only the selection;
`↑` on the first row goes to the document start and `↓` on the last row to the end. Typing
within 1.5 s is one undo step; every other command is its own step, and undo restores the
text, caret and selection exactly.

Copy also writes the system clipboard (`pbcopy`, `wl-copy`, `xclip` or `xsel`, else OSC 52),
and paste reads it when it can, falling back to the internal register.

## States, snapshots and replay

```sh
# An initial state for a file (80x24 unless --size is given).
caretline --new-state notes.md --size 60x20 > s.json

# Open it interactively, exactly as saved.
caretline --state s.json

# Apply a key script headlessly and print the frame (text or ANSI).
caretline --state s.json --keys 'Hello<cr><s-a-left><c-x>' --snapshot 60x20
caretline --state s.json --keys '<down><down><end>' --snapshot 60x20 --format ansi

# Apply messages (JSON Lines or a JSON array) and save the final state.
caretline --state s.json --msgs m.jsonl --snapshot 60x20 --dump-state out.json

# Record a session, then reproduce its final frame.
caretline notes.md --trace t.jsonl
caretline --replay t.jsonl --snapshot 80x24
```

- `--snapshot WxH` renders after the messages, resizing first if the state's viewport differs.
  `--size WxH` resizes before the messages instead.
- Headless runs never perform effects (no file writes); `--effects` prints them as JSON.
- **Key scripts:** literal characters plus `<cr>` `<bs>` `<del>` `<tab>` `<esc>` `<space>`
  `<left>` `<right>` `<up>` `<down>` `<home>` `<end>` `<pgup>` `<pgdn>` `<lt>` (a literal
  `<`) and `<wait:MS>` (advances the clock). Modifiers prefix a key and combine: `s-` Shift,
  `c-` Ctrl, `a-` Alt/Option, `d-` Cmd. For example `<s-a-left>`, `<c-s-z>`, `<d-a>`. Keys go
  through the keymap, so a script tests the bindings too.
- **Messages** are tagged JSON, one per line:

  ```json
  {"msg":"insert_text","text":"hi"}
  {"msg":"move","dir":"backward","by":"word","extend":true}
  {"msg":"paste","text":"two\nlines"}
  {"msg":"tick","now_ms":2000}
  {"msg":"resize","width":60,"height":20}
  {"msg":"show_status","text":"a note in the status bar"}
  ```

  `by` is one of `grapheme`, `word`, `line`, `visual_line`, `line_start`, `line_end`, `page`,
  `doc_start`, `doc_end`.
- **Traces** are JSON Lines: `{"state": …}` first, then `{"msg": …}` per message, including
  the clock ticks and the runtime's result messages, so a replay is exact.

### Example

```console
$ caretline --state crates/caretline-app/fixtures/wrapped-paragraph.state.json \
    --keys '<up><up><s-a-right><s-a-right>' --snapshot 44x14
# Field notes

The editing model is a rope of text with
one or more selections, each an anchor and
a head. Every edit is a transaction that
maps the selections through its changes, so
nothing drifts.

- Soft wrap keeps a goal column when moving
up and down across wrapped rows.
- Undo restores the exact text and
selection.

 wrapped.md                     9 sel  3:21
```

## The state protocol

A running editor can be read and driven from outside, and a headless engine can be kept
running, through a JSON Lines protocol: get or replace the whole state, push messages or
key scripts, render frames (text, ANSI or a cell grid), subscribe to changes and fetch the
trace. Everything goes through the same `update`, in one order, and lands in the trace.

```sh
caretline notes.md --listen                      # the editor, also serving a socket
caretline send --latest keys '<c-end><cr>hello'  # from another shell: it redraws at once
caretline send --latest state.get --raw          # read the state back
caretline serve notes.md                         # a headless engine on stdin/stdout
caretline serve --socket /tmp/cl.sock            # … or on a socket, for many clients
caretline bench                                  # throughput and latency
```

Pushed messages' effects (saves, the clipboard, quit) are returned to the client, not
performed, unless the request asks. The library side is `caretline_next::Session`. See
[docs/caretline/protocol.md](../../docs/caretline/protocol.md) for every operation, with examples.

## Fixtures

`fixtures/` holds saved states with their text and ANSI snapshots (the CLI tests check that
each still renders the same):

| Fixture | Shows |
|---|---|
| `wrapped-paragraph` | A Markdown note soft-wrapped at 44 columns, caret on a wrapped row |
| `emoji-line` | Emoji (skin tone, ZWJ family, flags), combining accents and wide CJK, with a selection |
| `mid-selection` | A selection across lines, ending inside a wrapped row |
| `after-undo` | An edit undone: the redo step is still in the history |
| `no-wrap-table` | Wrapping off: a long table row scrolled sideways |
| `session.trace.jsonl` | A recorded session; `session.snapshot.txt` is its replay at 36x8 |

```sh
caretline --state crates/caretline-app/fixtures/emoji-line.state.json --snapshot 40x6 --format ansi
```

## Limits

- One document, plain text and Markdown. No syntax highlighting, search or multiple buffers yet.
- The engine supports multiple selections, but no key creates them yet.
- `End` on a wrapped row stops before the space where the row wraps (a caret position has no
  "end of the row above" affinity).
- Display widths follow Helix's table (`unicode-width` 0.1.12), with emoji sequences counted
  as two cells; a terminal with different emoji widths can misplace the caret on those.
