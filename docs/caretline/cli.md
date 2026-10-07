# The `caretline` command

`caretline` is the interactive editor and a headless tool for states, snapshots and replay.
It comes from the `caretline-app` crate. Every output on this page is from a real run.

## Install

```sh
cargo install --locked --path crates/caretline-app   # installs `caretline`
cargo run -p caretline-app -- notes.md                # or run it from the checkout
```

## Commands at a glance

| Command | Does |
|---|---|
| `caretline [FILE]` | Edit FILE interactively (created on first save) |
| `caretline FILE --trace T.jsonl` | Edit, recording the session to a trace |
| `caretline FILE --no-mouse` | Edit without capturing the mouse |
| `caretline --new-state FILE [--size WxH]` | Print an initial state for FILE as JSON |
| `caretline --state S.json` | Edit a saved state interactively |
| `caretline --state S.json --keys SCRIPT …` | Apply a key script, headless |
| `caretline --state S.json --msgs M.jsonl …` | Apply messages, headless (`-` reads stdin) |
| `caretline --replay T.jsonl …` | Start from a trace with all its messages applied, headless |
| `… --snapshot WxH [--format text\|ansi]` | Print the rendered frame |
| `… --dump-state OUT.json` | Write the final state (`-` for stdout) |
| `… --effects` | Print the effects `update` returned, one JSON per line |
| `… --size WxH` | Resize before applying messages |
| `caretline serve`, `caretline send`, `--listen` | The state protocol, see [protocol.md](protocol.md) |
| `caretline bench` | Protocol throughput and latency (build with `--release`) |
| `caretline --outline FILE` | Edit FILE as an [outline](outline.md): lists, tasks and blocks with their own keys, Markdown in and out (also for `--new-state` and `serve`) |
| `caretline --layout FILE` | As `--outline`, with the [outline layout](outline.md#the-outline-layout): markers in a hang with plain glyphs, a column per depth (also for `serve`) |
| `… --no-status-bar` | Hide the status bar (`config.status_bar = false`): every row shows text |
| `… --trace-limit LINES` | Editor and `serve`: bound the in-memory trace `trace.get` serves (default 100,000 lines; see [protocol.md](protocol.md#traces)) |

A run is **headless** when any of `--snapshot`, `--dump-state`, `--msgs`, `--keys` or
`--replay` is given. Headless runs never perform effects: no file is written and the
clipboard isn't touched. `--effects` shows what would have happened.

## Edit a file

```sh
caretline notes.md
```

The keys are macOS text-field keys with Ctrl twins (see [messages.md](messages.md#the-keymap)).
`Ctrl-S` saves and `Ctrl-Q` quits; with unsaved changes, press `Ctrl-Q` twice. Cmd (⌘) keys
need a terminal that speaks the kitty keyboard protocol (kitty, WezTerm, Ghostty, iTerm2 with
the option on). caretline turns it on when the terminal supports it. Copy also writes the
system clipboard (`pbcopy`, `wl-copy`, `xclip` or `xsel`, else OSC 52), and paste reads it
when it can.

Saving writes a temporary sibling file and renames it over the target, so a failed write
never leaves half a file.

## The state, snapshot and replay workflow

This walks through every headless flag on a small file.

### 1. Capture a state

```console
$ printf '# Plan\n\nShip the editor docs.\n' > plan.md
$ caretline --new-state plan.md --size 40x6 > s0.json
$ caretline --state s0.json --snapshot 40x6
# Plan

Ship the editor docs.


 plan.md                            1:1
```

`--new-state` uses 80x24 unless you pass `--size`. The state is pretty-printed JSON: the
text, the selection, the scroll, the viewport, the undo history and the config. You can edit
it by hand; loading repairs anything out of range.

You can also write a state from scratch. Only `text` is needed; everything else takes
`--new-state`'s defaults (a caret at 0, 80x24, a fresh history, a clean document):

```console
$ cat > min.json <<'EOF'
{"text": "hello\nworld\n", "selection": {"ranges": [{"anchor": 0, "head": 5}]}, "viewport": {"width": 20, "height": 4}}
EOF
$ caretline --state min.json --keys 'bye' --snapshot 20x4
bye
world

 [scratch] [+]  1:4
```

See [architecture.md](architecture.md#rehydration) for every default.

### 2. Drive it with keys, and save the result

```console
$ caretline --state s0.json --keys '<d-down>Then review them.' --snapshot 40x6 --dump-state s1.json
# Plan

Ship the editor docs.
Then review them.

 plan.md [+]                       4:18
```

`<d-down>` is ⌘↓ (document end). The status bar shows the file name, `[+]` for unsaved
changes, and `line:col`.

### 3. Select, undo, inspect

```console
$ caretline --state s1.json --keys '<s-a-left><s-a-left>' --snapshot 40x6
# Plan

Ship the editor docs.
Then review them.

 plan.md [+]                12 sel  4:6
```

```console
$ caretline --state s1.json --keys '<c-z>' --snapshot 40x6
# Plan

Ship the editor docs.


 plan.md                            4:1
```

The undo history is part of the state, so `s1.json` can still undo the typing it recorded.
After the undo, the document matches the saved revision again, so `[+]` is gone.

### 4. See effects instead of performing them

```console
$ caretline --state s1.json --keys '<c-s>' --effects --dump-state /dev/null
{"effect":"write_file","path":"plan.md","text":"# Plan\n\nShip the editor docs.\nThen review them."}
$ cat plan.md
# Plan

Ship the editor docs.
```

The file is unchanged: headless runs report effects and never perform them.

### 5. Apply a message file

```console
$ cat m.jsonl
{"msg":"move","dir":"backward","by":"word","extend":true}
{"msg":"copy"}
{"msg":"move","dir":"forward","by":"doc_end"}
{"msg":"insert_newline"}
{"msg":"paste"}
$ caretline --state s1.json --msgs m.jsonl --effects --snapshot 40x6
{"effect":"clipboard_set","text":"them."}
# Plan

Ship the editor docs.
Then review them.
them.
 plan.md [+]                        5:6
```

`--msgs -` reads the messages from stdin. A JSON array works too.

### 6. Resize

`--size` resizes before the messages; `--snapshot` resizes after them if its size differs.
At 20 columns the long line wraps:

```console
$ caretline --state s1.json --size 20x6 --snapshot 20x6
# Plan

Ship the editor
docs.
Then review them.
 plan.md [+]   4:18
```

### 7. Record and replay a session

```sh
caretline notes.md --trace t.jsonl          # edit, then quit
caretline --replay t.jsonl --snapshot 80x24 # the final frame, exactly
caretline --replay t.jsonl --dump-state -   # the final state
```

`--trace` appends, so restarting into the same file adds a new `state` line and replay
continues from it. The file gets every line, also past `--trace-limit`, which bounds only
the in-memory trace. `--replay` reads any trace: a full file, the output of
`caretline send trace.get --raw` (one segment), or of `trace.get all`; a `state` line in the
middle (a `state.set` or checkpoint) restarts the replay from that state. The fixture `session.trace.jsonl` is a recorded session:

```console
$ caretline --replay crates/caretline-app/fixtures/session.trace.jsonl --snapshot 36x8

Shift and the arrows grow one from
the caret's end,
and a plain arrow collapses it to
the edge.

arrows grow one from the caret's
 select.md [+]                 5:33
```

## Snapshot formats

`--format text` (the default) prints the rows with trailing spaces trimmed. `--format ansi`
adds styling: the selection in reverse colours, the status bar highlighted, and the caret as
an underlined reverse cell (a snapshot has no terminal cursor).

```console
$ caretline --state s1.json --keys '<s-a-left>' --snapshot 40x6 --format ansi | sed -n 4p | cat -v
^[[0mThen review ^[[0m^[[7;4mt^[[0m^[[30;46mhem.^[[0m                       ^[[0m
```

Here `^[[7;4m` marks the caret cell and `^[[30;46m` the rest of the selection.

## Fixtures

[`crates/caretline-app/fixtures`](../../crates/caretline-app/fixtures) holds saved states
with their text and ANSI snapshots. The CLI tests check that each one still renders the
same.

| Fixture | Shows |
|---|---|
| `wrapped-paragraph` | A Markdown note soft-wrapped at 44 columns, caret on a wrapped row |
| `emoji-line` | Emoji (skin tone, ZWJ family, flags), combining accents and wide CJK, with a selection |
| `mid-selection` | A selection across lines, ending inside a wrapped row |
| `after-undo` | An edit undone, with the redo step still in the history |
| `no-wrap-table` | Wrapping off: a long table row scrolled sideways |
| `session.trace.jsonl` | A recorded session; `session.snapshot.txt` is its replay at 36x8 |
| `outline-trip` | `trip.md` opened as an outline: a heading, paragraphs, nested tasks, an image block and a numbered list, with blank rows drawn as virtual rows |
| `outline-edited` | The same after keys: a new nested task added and done, an item moved up |
| `outline-split` | A paragraph whose two selected lines the task cycle turned into tasks (`standup.md`) |

```console
$ caretline --state crates/caretline-app/fixtures/emoji-line.state.json --keys '<s-right><s-right>' --snapshot 40x6
Emoji 👍🏽 and family 👨‍👩‍👧, accents café
and résumé, wide 漢字かな, flags 🇫🇷🇯🇵.
Second line: ½ ⅓ → ✓


 emoji.md                   5 sel  1:19
```

```console
$ caretline --state crates/caretline-app/fixtures/no-wrap-table.state.json --keys '<home>' --snapshot 48x6
| Name | Role | Notes |
|---|---|---|
| Ada | engine | Wrapping is off here, so long t
| Lin | review | Short row. |

 nowrap.md                                  3:1
```

The status bar's `N sel` counts Unicode scalar values, while `line:col` counts graphemes.

## Errors

Errors go to stderr with exit code 1:

```console
$ caretline --state s0.json --snapshot 80
caretline: bad size "80": expected WIDTHxHEIGHT, like 80x24
$ caretline --state s0.json --keys '<nope>'
caretline: unknown key <nope>
$ echo '{"msg":"jump"}' | caretline --state s0.json --msgs -
caretline: line 1: unknown variant `jump`, expected one of `insert_text`, `insert_newline`, … at line 1 column 13
```

## Limits

- One document of plain text or Markdown (`--outline` reads it as blocks). No syntax
  highlighting, search or multiple buffers yet.
- The engine edits multiple selections, but no key creates them yet.
- `End` on a wrapped row stops before the space where the row wraps.
- Display widths follow Helix's table (`unicode-width` 0.1.12). A terminal with different
  emoji widths can misplace the caret on those graphemes.
