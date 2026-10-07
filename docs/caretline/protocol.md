# The state protocol

The state protocol lets another program drive a caretline engine over **JSON lines**: one
JSON request per line in, one JSON response per line out. Use it to:

- run a **headless engine** as a child process (`caretline serve`, on stdin and stdout),
- share one engine with **several clients** on a Unix socket (`caretline serve --socket`),
- **attach to a live editor** a person is using (`caretline FILE --listen`), and
- send one-off requests from a shell (`caretline send`).

The same operations are available in process through
[`Session`](api.md#session).

## Quick start

```console
$ printf 'hello world\nsecond line\n' > notes.md
$ printf '%s\n' '{"id":1,"op":"hello"}' '{"id":2,"op":"keys","keys":"<a-right><s-a-right>"}' '{"id":3,"op":"render","w":30,"h":4}' \
    | caretline serve notes.md --size 30x4 --no-clock
{"id":1,"result":{"proto":1,"version":"0.1.0","rev":0,"ops":["hello","state.get","state.set","text.set","history.get","msgs","keys","render","subscribe","unsubscribe","trace.get","trace.checkpoint","view.open","view.close","view.list"]}}
{"id":2,"result":{"rev":2,"effects":[],"msgs":[{"msg":"move","dir":"forward","by":"word","extend":false},{"msg":"move","dir":"forward","by":"word","extend":true}]}}
{"id":3,"result":{"rev":2,"w":30,"h":4,"format":"text","cursor":[11,0],"frame":"hello world\nsecond line\n\n notes.md         6 sel  1:12\n"}}
```

## Requests and responses

A request is a JSON object with an `op`. An optional `id` (any JSON value) is echoed back so
you can match responses. A connection's responses come back in request order.

```json
{"id": 7, "op": "render", "w": 80, "h": 24}
```

A success is `{"id", "result"}`. A failure is `{"id", "error": {"kind", "message"}}`. Every
result carries the current `rev`.

### Operations

| Op | Request fields | Result |
|---|---|---|
| `hello` | | `proto` (1), `version`, `rev`, `ops` |
| `state.get` | optional `history` (default true) | `rev`, `state` (the full [State](architecture.md#what-state-holds) JSON). With `history: false`, the state [without its undo history](#the-state-without-its-history) |
| `history.get` | | `rev` and what `state.get` with `history: false` leaves out: `history`, `saved_revision`, `saving`, `run`, and `mark_log` and `undo_floor` when set |
| `state.set` | `state` (only `text` needed, see [Minimal state](#minimal-state)), optional `if_rev` | `rev`. Replaces the state (repaired as on load) and starts a new trace segment |
| `text.set` | `text`, optional `if_rev`, `view` | `rev`, `changed`, `view`, `msgs` (the `external` message applied, if any). Puts `text` in as the whole text, changing only what differs, outside the undo history. Safe while someone types: see [Collaborating with a person](#collaborating-with-a-person) |
| `frame` | `text`, optional `highlights` (`[[start, end], …]` char ranges), `caret`, `status`, `if_rev` | `rev`. Shows `text` as the whole document with no undo history: see [Frames](#frames) |
| `msgs` | `msgs` (array of [messages](messages.md)), optional `if_rev`, `apply_effects`, `now_ms`, `view` | `rev`, `effects`, `msgs` (every message applied: a leading `tick`, the request's messages, and fed-back results such as `saved`), `view` (the view they went through), and `executed: true` when effects were performed |
| `keys` | `keys` (a [key script](messages.md#key-scripts)), optional `if_rev`, `apply_effects`, `now_ms` | Like `msgs`; `msgs` shows what the script became |
| `render` | optional `w`, `h` (default: the state's viewport), `format` | `rev`, `w`, `h`, `format`, `cursor` (`[x, y]` or null), and `frame` or `rows` |
| `subscribe` | optional `frame` (`{w, h, format}`), `with_msgs` (default true), `with_state` (default false) | `rev`, `subscribed: true`. Then events, below |
| `unsubscribe` | | `rev`, `subscribed: false` |
| `trace.get` | optional `since_rev` or `all` | `rev`, `from_rev`, `trace`: by default the current segment, as [trace lines](architecture.md#traces-make-sessions-reproducible). See [Traces](#traces) |
| `trace.checkpoint` | | `rev`. Starts a new trace segment with the current state. Changes neither the state nor the rev |
| `view.open` | optional `open` (a [View](architecture.md#documents-and-views): every field optional; a copy of view 0 when absent), `w`, `h` | `rev`, `view`: the new view's id. See [Views](#views) |
| `view.close` | `view` | `rev`, `closed` |
| `view.list` | | `rev`, `views`: `{view, w, h, caret, read_only}` for each, view 0 first |

`render` and `state.get` take an optional `view` (default 0, the state's own). `msgs`, `keys`
and `text.set` take one too; without it they go through view 0 in `caretline serve`, and
through the client's own view in a live editor (see
[Collaborating with a person](#collaborating-with-a-person)).

`render` never changes the state. A size other than the viewport is rendered on a copy, so
the frame is byte for byte what `caretline --state S --snapshot WxH` prints.

### Minimal state

`state.set` (and a `--state` file) needs only what a client knows. Every field but `text` is
optional and takes `State::new`'s default: a caret at 0, an 80x24 viewport, a fresh undo
history, the default config with the line ending detected from the text, no open edit run,
and a document that counts as saved (clean). `dirty` is always recomputed. A missing
`saved_revision` means saved at the current history revision; `"saved_revision": null` means
never saved. Inside `selection`, `old_visual_position` and `primary_index` are optional.

```console
$ caretline send '{"op":"state.set","state":{"text":"hello\nworld\n","selection":{"ranges":[{"anchor":0,"head":5}]},"viewport":{"width":40,"height":10},"config":{"soft_wrap":false}}}'
{"result":{"rev":4}}
```

A live editor resizes a pushed state to its terminal. See
[architecture.md](architecture.md#rehydration) for every default.

### The state without its history

The undo history is most of a state's JSON once there has been some editing: after 3,000
separate edits it is 1.4 MB, against 71 KB for a 1,000-line document's text and view. Most
clients want the text, the selection and the view, so ask for those alone:

```console
$ caretline send '{"op":"state.get","history":false}'
{"result":{"rev":9,"state":{"text":"hello world\n","selection":{…},…,"saved_revision":null,"dirty":true,…}}}
$ caretline send history.get
{"result":{"rev":9,"history":{…},"saved_revision":0,"saving":null,"run":null}}
```

`history: false` leaves out `history`, `saving`, `run`, `mark_log` and `undo_floor`, and
writes `saved_revision` for a fresh history: `0` when the document is clean, `null` when it is
dirty. So the state rehydrates (with `state.set`, `--state` or `State::from_json`) to the same
text, selection, view and dirty flag, with a fresh history: the editor works, and undo starts
from there. `history.get` returns the fields left out with their real values; set them over
the history-less state and you have exactly what `state.get` returns. `caretline send` has
`state.get no-history` and `history.get`. In Rust, `State::without_history()` and
`State::history_part()` serialize the two halves.

### Frames

`frame` pushes one frame of an animation, a demo or a mirror: a whole text, some ranges to
highlight and no history.

```json
{"op":"frame","text":"  @@  \n @@@@ \n  @@  ","highlights":[[2,4],[8,12]],"status":"donut · 120 fps"}
```

- The text replaces the document, with a fresh, clean undo history and the document's path
  and config kept.
- The view keeps its size, scroll, config and frame clock, so a live editor never resizes for
  a frame. `status` replaces the status bar's message; without it the last one stays.
- `highlights` are char ranges drawn in the selection's colour (they are the selection: the
  primary caret sits at the end of the first). `caret` puts the primary caret there instead.
- Like `state.set`, it goes up one rev, starts a trace segment (so the trace replays), and
  subscribers get an event with `state_set: true`.

It is much cheaper than `state.set` with the same text: no state to parse, and nothing to
repair. A live editor answers a 10 KB frame in 30–100 µs. See
[performance.md](performance.md#animation) and the demo client,
`caretline demo scenes` ([`src/demo/scenes.rs`](../../crates/caretline-app/src/demo/scenes.rs)).

### Traces

The session keeps its trace as **segments**. Each starts with a `state` line and holds every
message applied after it, so each replays on its own with `caretline --replay`. A segment
starts when the session starts, at every `state.set`, and at every `trace.checkpoint`.

| Request | Returns |
|---|---|
| `{"op":"trace.get"}` | The current segment: the latest `state` line and every message since. `from_rev` is the rev it starts at |
| `{"op":"trace.get","since_rev":N}` | The lines after rev `N`: what changed since you saw `N`. It starts with a `state` line only if a segment began after `N`, so it replays onto the state you had at `N`. `from_rev` is `N` |
| `{"op":"trace.get","all":true}` | Every line kept, from the session start unless the limit dropped older segments. `from_rev` is the rev it starts at |

The default is the current segment because it is always replayable and stays small after a
`state.set`, while `all` grows for the life of the editor. Use `trace.checkpoint` to mark a
point (the start of a test, say) and then read just what happened after it.

```console
$ caretline send '{"op":"trace.checkpoint"}'
{"result":{"rev":41}}
$ caretline send keys 'hi'
$ caretline send trace.get --raw     # the checkpoint's state, a tick, two insert_text
$ caretline send trace.get since 41 --raw
$ caretline send trace.get all --raw
```

**The size is bounded.** A session keeps at most 100,000 lines (`--trace-limit LINES` on
`serve` and the editor). Past that, it drops the segments before the current one; a segment
that alone outgrows the limit is cut by an automatic checkpoint, a `state` line at the
current rev. `since_rev` older than what's kept is an error of kind `trimmed`, which names
the oldest rev still available. A `--trace` file is separate: it receives every line and is
never trimmed. Every trace, segment and file still replays with `caretline --replay`, since a
later `state` line just restarts the replay from that state.

### Time

`update` reads no clock: time arrives as `tick` messages, and typing within 1.5 s of the
last edit joins the same undo step. So that a client's typing groups by real time:

- `msgs` and `keys` take an optional `now_ms`. The engine applies `{"msg":"tick","now_ms":…}`
  first (when it differs from the state's clock), and `<wait:MS>` in a key script counts from
  it.
- Without `now_ms`, `caretline serve` and a listening editor tick to the **real time** before
  each request's messages (only forward, and only when the clock has moved). The tick is an
  ordinary message: it is in the result's `msgs`, in events and in the trace, so replay stays
  exact.
- `caretline serve --no-clock` turns that off, for fully deterministic sessions. `Session`
  in process never ticks on its own (`Session::handle_at` takes a clock).

```console
$ echo '{"op":"keys","now_ms":1000,"keys":"ab"}' | caretline serve notes.md
{"result":{"rev":3,"effects":[],"msgs":[{"msg":"tick","now_ms":1000},{"msg":"insert_text","text":"a"},{"msg":"insert_text","text":"b"}]}}
```

### Views

One engine can show its document through several views: a second window, a side panel, an
agent's own caret. Each has its own selection, scroll, size, folds and layout; they share the
text, marks and undo history. View 0 is the state's own, the one `state.get` returns and the
live editor draws. `view.open` adds another and returns its id; `msgs`, `keys`, `render` and
`state.get` act through any view with `"view": id`. An edit through one view maps every other
view's selection, so each stays on its text.

```console
$ printf 'hello world\nsecond line\n' > notes.md
$ printf '%s\n' '{"id":1,"op":"view.open","w":24,"h":4}' '{"id":2,"op":"keys","view":1,"keys":"<down><end>"}' \
    '{"id":3,"op":"keys","keys":">> "}' '{"id":4,"op":"render","view":1}' | caretline serve notes.md --size 30x4 --no-clock
{"id":1,"result":{"rev":1,"view":1}}
{"id":2,"result":{"rev":3,"effects":[],"msgs":[…]}}
{"id":3,"result":{"rev":6,"effects":[],"msgs":[…]}}
{"id":4,"result":{"rev":6,"w":24,"h":4,"format":"text","cursor":[11,1],"frame":">> hello world\nsecond line\n\n notes.md [+]      2:12\n"}}
```

Opening and closing a view is a change (`rev` goes up). Traces record views: a message through
view `n` is `{"on":{"view":n,"msg":…}}`, an opened view `{"view_open":{"id":n,"view":…}}`, a
closed one `{"view_close":n}`, and a segment's `state` line is followed by a `view_open` line
for each view open, so every segment still replays on its own. A `state.set` keeps the other
views, fitted to the new document.

### Frame formats

| `format` | Result |
|---|---|
| `text` (default) | `frame`: the rows as text, trailing spaces trimmed, as `--snapshot` prints |
| `ansi` | `frame`: the rows with ANSI styling, as `--snapshot --format ansi` prints |
| `cells` | `rows`: one `{text, spans, info}` per row. `spans` lists runs of non-text roles as `[x, len, role]`; `info` says what the row shows (below) |

```json
{"id":9,"result":{"rev":3,"w":30,"h":4,"format":"cells","cursor":[11,0],"rows":[
  {"text":"hello there                   "},
  {"text":"second line                   "},
  {"text":"                              "},
  {"text":" notes.md [+]            1:12 ","spans":[[0,9,"status"],[9,4,"status_accent"],[13,17,"status"]]}]}}
```

Roles are `text`, `selection`, `status`, `status_accent` and `hang` (an outline block's hang,
with the [outline layout](outline.md#the-outline-layout)).

A row's `info` is `{"kind":"text","block":3,"line":4,"row":0,"first":true,"last":true,"chars":{"start":40,"end":52},"x":6}`
(a row of text: its block, line, visual row, whether it is the block's first or last row, the
chars it shows and the column its text starts), `{"kind":"gap","before":3}` (a block's blank
row), `{"kind":"extra","block":3,"index":0}` (a host's row after a block), `{"kind":"past"}`
or `{"kind":"status"}`.

## Revisions

`rev` starts at 0 when the engine loads. It goes up by **one for every message applied** and
one for every `state.set`. That includes clock ticks, resizes and the `saved` message a
performed save feeds back. In a live editor, one key press is usually two revs: a `tick` and
the key's message. A client request is usually one more than its messages: the leading
`tick` (see [Time](#time)).

Use `rev` two ways.

- **Did something change?** Compare the `rev` you last saw. The difference is the number of
  changes you missed, and `trace.get` with `since_rev` has them.
- **Write only if nothing changed:** pass `if_rev` on `state.set`, `text.set`, `msgs` or `keys`. If the
  rev differs, nothing is applied and you get a `stale` error. Re-read, then decide.

```console
$ caretline send '{"op":"msgs","msgs":[{"msg":"undo"}],"if_rev":0}'
{"error":{"kind":"stale","message":"rev is 12, the request expected 0"}}
caretline: the request failed
```

Reads (`hello`, `state.get`, `render`, `trace.get`) ignore `if_rev`.

## Collaborating with a person

In a live editor (`caretline FILE --listen`) view 0 is the person at the keyboard. A client
never moves their caret unless it asks to:

- **Each client writes through its own view.** A `msgs`, `keys` or `text.set` request without
  a `view` goes through the connection's own view: the first one opens it as a copy of view
  0 (one more `rev`; `if_rev` is checked first), and it closes when the connection does. The
  response's `view` names it, for `render` or `state.get` with `"view": n`. `"view": 0` acts
  as the person (demos, tests, the status bar: `caretline send status` uses it). `caretline
  serve` has no person, so there everything goes through view 0 as before.
- **Text put in at a caret goes after it.** Whatever view a change goes through, a caret
  exactly where text is inserted stays before it, so the person's next key continues their
  own line. A selection keeps what it covered.

| Safe while someone types | Replaces the world |
|---|---|
| `text.set`: your version of the whole text; only what differs changes, outside the undo history, every caret, scroll and fold stays on its text | `state.set`: the whole state, the person's selection, scroll and undo history included. For time travel and hand-off |
| `msgs` and `keys` through your own view; an `external` message for changes the person's undo must not take back | `frame`: shows a text with no history (animations, mirrors). It replaces the screen on purpose |

Guard each write with the `rev` you read (`if_rev`): a `stale` error means the person typed in
between, so read again and redo it. `msgs` that edit through any view are steps in the shared
undo history, so the person's undo may take one back; `text.set` and `external` changes never
are.

```console
$ caretline send --latest state.get no-history --raw > s.json      # read
$ caretline send --latest set-text notes-v2.md                      # push a new version
{"result":{"rev":41,"changed":true,"view":3,"msgs":[{"msg":"external","changes":[…]}]}}
```

## Effects

By default the engine **returns** effects and doesn't perform them. Performing them is the
client's job:

```json
{"id":5,"op":"msgs","msgs":[{"msg":"save"}]}
{"id":5,"result":{"rev":4,"effects":[{"effect":"write_file","path":"notes.md","text":"…"}]}}
```

Write the file, then report back with `{"op":"msgs","msgs":[{"msg":"saved"}]}` (or
`save_failed`), so the editor knows the document is clean.

To have a **live editor** perform them instead (write the file, set the clipboard, quit), set
`apply_effects: true`. The result then lists the effects with `executed: true`. The result
messages the editor applied (such as `saved`) count toward `rev` and are listed in the
result's `msgs` and in subscribe events. `caretline serve` has no one to perform
effects for, so it refuses `apply_effects` with `unsupported`.

## Subscribe to changes

After `subscribe`, the connection receives a line for every change, from any source:

```json
{"event":"state","rev":11,"source":"client","msgs":[{"msg":"move","dir":"forward","by":"doc_end","extend":false},{"msg":"insert_text","text":"t"}]}
{"event":"state","rev":12,"source":"client","msgs":[{"msg":"show_status","text":"agent was here"}],"frame":{"w":30,"h":4,"format":"text","cursor":[10,2],"frame":"hello world\nsecond line\nthird line\n notes.md [+]  agent was 3:11\n"}}
```

| Field | Meaning |
|---|---|
| `event` | Always `"state"` in this version |
| `rev` | The rev after the change |
| `source` | `client` (a protocol request), `terminal` (the person at the keyboard) or `runtime` (startup ticks, resizes) |
| `state_set` | `true` when the state was replaced. Omitted otherwise |
| `view` | The view the messages went through, or the view opened or closed. Omitted for view 0 |
| `msgs` | The messages applied, effect results included. Omitted with `with_msgs: false` |
| `frame` | A rendered frame, when you subscribed with `frame` |
| `state` | The state [without its history](#the-state-without-its-history), when you subscribed with `with_state: true` |

A subscriber also gets events for its own requests, after the response.

## Errors

| `kind` | When |
|---|---|
| `parse` | The line isn't JSON. No `id` comes back |
| `bad_request` | Not an object, no `op`, a missing field (`state.set needs a state`), or a zero size |
| `unknown_op` | The op isn't one of `hello`'s `ops` |
| `bad_keys` | The key script doesn't parse (`unknown key <oops>`) |
| `stale` | `if_rev` didn't match the current rev |
| `trimmed` | `trace.get` asked for a `since_rev` older than the kept trace |
| `unsupported` | `apply_effects` on a server that can't perform effects |
| `no_view` | A `view` that isn't open |

```json
{"id":"b","error":{"kind":"bad_keys","message":"unknown key <oops>"}}
{"error":{"kind":"parse","message":"not JSON: expected ident at line 1 column 2"}}
```

Within protocol version 1, results only gain fields. Ignore fields you don't know.

## Transports

| Command | Transport | Clients | Ends when |
|---|---|---|---|
| `caretline serve [FILE] [--state S] [--size WxH]` | stdin and stdout | One | stdin closes (after the last response is written) |
| `caretline serve … --socket PATH` | Unix socket | Many | The process is stopped (SIGTERM, SIGINT or SIGHUP) |
| `caretline FILE --listen [PATH]` | Unix socket, beside the live terminal editor | Many | The editor quits |

- `serve --trace T.jsonl` appends the initial state and every change to a trace, which
  `caretline --replay` reads. `serve --no-clock` stops the [real-time ticks](#time).
  `--trace-limit LINES` bounds the in-memory trace (see [Traces](#traces)).
- `--no-status-bar` (on `serve`, the editor and `--new-state`) sets `config.status_bar` to
  false: every row shows text, for embedders and panels with their own chrome.
- `--listen` with no path uses `$TMPDIR/caretline-<pid>.sock`, or
  `/tmp/caretline-<uid>/<pid>.sock` when `$TMPDIR` is too long for a socket path. Put FILE
  before `--listen`, or FILE is read as the socket path.
- The socket transports are Unix sockets, so `--listen` and `serve --socket` don't run on
  Windows yet (`serve` on stdin and stdout does). A named-pipe transport is planned, not
  started.
- A socket path must fit the OS limit (103 bytes on macOS, 107 on Linux). A longer one is
  refused with an error that says so. A stale socket file that nobody answers on is replaced.
- The socket (and an editor's discovery file) is removed on exit, and also when the process
  is stopped with SIGTERM, SIGINT or SIGHUP.
- All inputs (terminal events and every client's requests) go through one queue, so the
  editor applies them in one order, and the trace records that order. See
  [architecture.md](architecture.md#the-runtime-merges-inputs-into-one-queue).

`caretline bench` measures the protocol's throughput and latency, in process and over a
socket. Build with `--release` for meaningful numbers. On an Apple-silicon Mac:

| Operation | In process | Socket round trip |
|---|---|---|
| `hello` | | ~11 µs |
| `msgs`, one edit | ~1–2 µs | ~12–14 µs |
| `keys "<down>"` (the same as the `msgs` it becomes) | | ~21–25 µs |
| `render` 100x40 (`text` or `cells`) | ~85–90 µs | ~100 µs |
| `state.get`, 1,000 lines, fresh history | ~23 µs | ~80 µs |
| `state.get`, 1,000 lines, after 3,000 edits (1.4 MB) | ~0.9 ms | ~1.9 ms |
| `state.get` `history: false`, 1,000 lines, after 3,000 edits (71 KB) | ~27 µs | ~85 µs |
| `history.get`, after 3,000 edits (1.4 MB) | ~0.8 ms | ~1.8 ms |
| `state.get`, 100,000 lines (6.6 MB) | ~2.5 ms | ~7 ms |

Update and render cost follows the visible rows, not the document length, and stays flat
along a long line: a typing run of one `insert_text` request per character costs about
10 µs per character at 1,000 and at 64,000 characters, on one line or as prose. `state.get` grows
with the text and the undo history: leave the history out unless you need it, and for a
large document prefer `render` and events.

### Discovery

A listening editor writes `$TMPDIR/caretline/<pid>.json` and removes it on exit:

```json
{"pid":6103,"socket":"/var/folders/…/T/caretline-6103.sock","file":"notes.md","proto":1,"started_ms":1791353249114}
```

`caretline send` uses it to find editors by `--pid`, or by default the most recently started
one that answers (`--latest`). It removes discovery files whose editor is gone.

## `caretline send`

A small client for any server.

| Request words | Sends |
|---|---|
| `hello`, `history.get`, `unsubscribe`, `trace.checkpoint` | That op |
| `state.get [no-history]` | `state.get`, without the undo history with `no-history` |
| `'{"op":"frame",…}'` | Use a raw request for `frame` |
| `trace.get [all \| since REV]` | `trace.get`: the current segment, everything kept, or the lines after `REV` |
| `render [WxH] [text\|ansi\|cells]` | `render` |
| `keys SCRIPT` | `keys` |
| `msgs FILE\|-\|JSON` | `msgs` from a file, stdin or inline JSON |
| `set-text FILE\|-` | `text.set`: only what differs changes; carets stay put |
| `set-state FILE\|-` | `state.set`: replaces everything, carets and undo too |
| `status TEXT` | A `show_status` message: text in the editor's status bar |
| `subscribe [WxH [format]] [state]` | `subscribe`, then prints events until interrupted; `state` adds the state, without its history, to each |
| `'{"op":…}'` | A raw JSON request |
| (nothing) | Reads JSON requests from stdin, one per line |

| Flag | Does |
|---|---|
| `--socket PATH` | Connect to this socket |
| `--pid N` | Connect to the editor with this process id |
| `--latest` | Connect to the newest live editor (the default) |
| `--raw` | Print the payload: the frame for `render`, the state for `state.get`, JSON Lines for `trace.get` |
| `--apply-effects` | Ask the editor to perform the effects of `msgs` or `keys` |
| `--view N` | Act through view `N` (`msgs`, `keys`, `set-text`, `render`, `state.get`). In a live editor, writes otherwise go through the connection's own view, closed when `send` exits; `--view 0` acts as the person |

`send` exits 1 if any response is an error.

## Drive a live editor from another shell

In one terminal, open a file and listen:

```sh
caretline notes.md --listen
```

The status bar says `listening on …/caretline-<pid>.sock`. In another shell:

```console
$ caretline send hello
{"result":{"proto":1,"version":"0.1.0","rev":3,"ops":["hello","state.get","state.set","text.set","history.get","msgs","keys","render","subscribe","unsubscribe","trace.get","trace.checkpoint","view.open","view.close","view.list"]}}
$ caretline send keys '<d-down>from another shell'
{"result":{"rev":23,"effects":[],"msgs":[{"msg":"tick","now_ms":1791353251020},{"msg":"move","dir":"forward","by":"doc_end","extend":false},{"msg":"insert_text","text":"f"}, …]}}
$ caretline send render 40x5 --raw
hello world
second line
from another shell

 notes.md [+]                      3:19
$ caretline send --apply-effects keys '<c-s>'
{"result":{"rev":26,"effects":[{"effect":"write_file","path":"notes.md","text":"hello world\nsecond line\nfrom another shell"}],"executed":true,"msgs":[{"msg":"tick","now_ms":1791353254310},{"msg":"save"},{"msg":"saved"}]}}
$ caretline send --apply-effects keys '<c-q>'
{"result":{"rev":28,"effects":[{"effect":"quit"}],"executed":true,"msgs":[{"msg":"tick","now_ms":1791353256002},{"msg":"quit"}]}}
```

The save took three revs (23 to 26): the clock's `tick`, the `save` message and the `saved`
the editor fed back.

Each `send` writes through its own view, a copy of the person's taken at its first write, so
the person's caret stays where it was (text put in at it goes after it). The person at the
keyboard sees each change as it lands. Without `--apply-effects`, `<c-s>`
would only return the `write_file` effect, and `<c-q>` would leave the editor running.

To watch everything that happens, including the person's typing:

```sh
caretline send subscribe 80x24
```

To save a copy of the live session and replay it later:

```sh
caretline send trace.get all --raw > live.jsonl     # or just trace.get: since the last state.set
caretline --replay live.jsonl --snapshot 80x24
```

## A client in a few lines

See [embedding.md](embedding.md#as-a-process) for a complete Python client that spawns
`caretline serve`, edits, saves and renders.
