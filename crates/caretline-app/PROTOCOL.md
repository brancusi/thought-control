# The caretline state protocol

A JSON Lines protocol for reading and driving a caretline editor: a headless engine
(`caretline serve`) or a running interactive editor (`caretline FILE --listen`). Every
operation goes through the same pure `update` and `view` as the keyboard, so anything a
client does is recorded in the trace and replays exactly.

- [Transports](#transports)
- [Requests and responses](#requests-and-responses)
- [Operations](#operations)
- [Events](#events)
- [Effects](#effects)
- [Errors](#errors)
- [The client: `caretline send`](#the-client-caretline-send)
- [In process: `Session`](#in-process-session)
- [Performance](#performance)

## Transports

| Command | Speaks the protocol on |
|---|---|
| `caretline serve [FILE \| --state S.json] [--size WxH]` | stdin and stdout; exits when stdin ends |
| `caretline serve … --socket PATH` | a Unix socket, any number of clients |
| `caretline FILE --listen [PATH]` | a Unix socket, from the running interactive editor |

`serve` also takes `--trace T.jsonl`. A file argument to `serve` only names the document;
the server never writes it (see [Effects](#effects)).

`--listen` without a path uses `$TMPDIR/caretline-<pid>.sock` (put `FILE` before
`--listen`, or it is read as the path). The editor shows `listening on PATH` in its status
bar until the first key, prints the path when it exits, and writes a discovery file:

```json
// $TMPDIR/caretline/<pid>.json
{"pid":4242,"socket":"/var/folders/…/T/caretline-4242.sock","file":"notes.md","proto":1,"started_ms":1791000000000}
```

The socket and the discovery file are removed when the editor exits normally.
`caretline send --pid N` and `--latest` read these files.

## Requests and responses

One JSON object per line in each direction. A request has an `op`, optionally an `id`
(any JSON value), and the op's parameters at the top level:

```json
{"id":1,"op":"keys","keys":"Hello<cr>"}
```

Every request gets exactly one response line, in request order per client. A success has
`result`; a failure has `error`. The `id` is echoed when the request had one:

```json
{"id":1,"result":{"rev":6,"effects":[],"msgs":[…]}}
{"id":2,"error":{"kind":"unknown_op","message":"unknown op \"frob\"; known ops: …"}}
```

Lines with an `event` key are [events](#events), not responses.

**Revisions.** The session's `rev` starts at 0 when it loads and goes up by one for every
message applied and every `state.set`, whoever caused it (a client, the local keyboard, the
runtime). It never goes down. Reads (`state.get`, `render`, `trace.get`, `hello`) don't
change it. A write may carry `if_rev: N` to apply only if the rev is still `N` (otherwise the
`stale` error, and nothing changes).

**Order.** All inputs (request lines from every client and, in the interactive editor,
terminal events) go through one queue, so the session sees one total order. That order is
what the trace records.

**Compatibility.** `hello` reports `proto: 1`. Within a protocol version, responses and
events only gain fields; ignore fields you don't know.

## Operations

### `hello`

```json
→ {"id":1,"op":"hello"}
← {"id":1,"result":{"proto":1,"version":"0.1.0","rev":0,"ops":["hello","state.get","state.set","msgs","keys","render","subscribe","unsubscribe","trace.get"]}}
```

### `state.get`

The full `State` (the same JSON as `--dump-state` and `--state`): text, selection, scroll,
viewport, clipboard register, undo history, config, status, clock.

```json
→ {"id":2,"op":"state.get"}
← {"id":2,"result":{"rev":3,"state":{"text":"abc","selection":{"ranges":[{"anchor":3,"head":3,"old_visual_position":null}],"primary_index":0},…}}}
```

### `state.set`

Replaces the state. It is repaired the way a loaded file is (selections clamped to the
text and to grapheme boundaries, a zero viewport raised to 1). Params: `state`, optional
`if_rev`.

```json
→ {"id":3,"op":"state.set","state":{…}}
← {"id":3,"result":{"rev":4}}
```

In a live editor, a state whose viewport differs from the terminal is followed by a
`resize` to the terminal's size (it shows up as an event and in the trace).

### `msgs`

Applies [messages](README.md#states-snapshots-and-replay) in order. Params: `msgs` (an
array), optional `if_rev`, optional `apply_effects` (see [Effects](#effects)). Returns the
rev after the last one and every effect `update` returned, in order.

```json
→ {"id":4,"op":"msgs","msgs":[{"msg":"insert_text","text":"hi"},{"msg":"move","dir":"backward","by":"word","extend":true},{"msg":"copy"}]}
← {"id":4,"result":{"rev":7,"effects":[{"effect":"clipboard_set","text":"hi"}]}}
```

Messages carry no clock of their own: the typing-run rule (typing within 1.5 s is one undo
step) uses the last `tick`. Send `{"msg":"tick","now_ms":…}` when that matters.
`{"msg":"show_status","text":"…"}` puts a one-line note in the status bar until the next
input.

### `keys`

A key script, with the same syntax as `--keys`, through the keymap. Params: `keys`,
optional `if_rev` and `apply_effects`. Also returns the messages the keys became.

```json
→ {"id":5,"op":"keys","keys":"<down><end>!<s-a-left>"}
← {"id":5,"result":{"rev":11,"effects":[],"msgs":[{"msg":"move","dir":"forward","by":"visual_line","extend":false},…]}}
```

`<wait:MS>` becomes a `tick` that many milliseconds after the state's clock.

### `render`

The frame. Params: `w` and `h` (default: the state's viewport) and `format`: `text`
(default), `ansi` or `cells`. Rendering at another size doesn't change the session: it is
exactly the frame `caretline --state S --snapshot WxH` prints for the current state (the
resize is applied to a copy). Every format reports `cursor`, the caret's cell `[x, y]`, or
`null` when it is off screen.

```json
→ {"id":6,"op":"render","w":20,"h":3}
← {"id":6,"result":{"rev":11,"w":20,"h":3,"format":"text","cursor":[1,1],"frame":"hi\nx\n [scratch] [+]  2:2\n"}}
```

`text` and `ansi` are the `--snapshot` output, byte for byte. `cells` is a compact grid:
each row's graphemes joined (the second cell of a wide grapheme adds nothing, so string
offsets and columns differ after one) and its runs of styled cells as `[x, len, role]` in
columns. Roles: `selection`, `status`, `status_accent`; unlisted cells are `text`.

```json
→ {"id":7,"op":"render","w":20,"h":3,"format":"cells"}
← {"id":7,"result":{"rev":11,"w":20,"h":3,"format":"cells","cursor":[1,1],"rows":[
    {"text":"hi                  "},
    {"text":"x                   "},
    {"text":" [scratch] [+]  2:2 ","spans":[[0,10,"status"],[10,4,"status_accent"],[14,6,"status"]]}]}}
```

### `subscribe` and `unsubscribe`

After `subscribe`, the client receives an [event](#events) after every change, from any
source, until it sends `unsubscribe` or disconnects. Params: `with_msgs` (default `true`)
and `frame`, an optional `{w, h, format}` (each optional) to get a rendered frame with
every event.

```json
→ {"id":8,"op":"subscribe","frame":{"format":"text"}}
← {"id":8,"result":{"rev":11,"subscribed":true}}
→ {"id":9,"op":"unsubscribe"}
← {"id":9,"result":{"rev":11,"subscribed":false}}
```

### `trace.get`

The trace since load: the initial state, then every message and every replacement state in
order. Each element is one trace line, so writing them one per line gives a file
`caretline --replay` reads, and replaying it gives the current state exactly.

```json
→ {"id":10,"op":"trace.get"}
← {"id":10,"result":{"rev":11,"trace":[{"state":{…}},{"msg":{"msg":"insert_text","text":"h"}},…]}}
```

## Events

```json
{"event":"state","rev":12,"source":"terminal","msgs":[{"msg":"tick","now_ms":1791000001234},{"msg":"insert_text","text":"k"}]}
{"event":"state","rev":13,"source":"client","state_set":true,"frame":{"w":80,"h":24,"format":"text","cursor":[0,0],"frame":"…"}}
```

- `rev`: the rev after the change. One event can cover several revs (a `msgs` request with
  three messages is one event).
- `source`: `client` (a protocol request, from any client), `terminal` (the local user's
  input) or `runtime` (the editor itself: start-up ticks, a resize after `state.set`).
- `msgs`: the messages applied (unless subscribed with `with_msgs: false`). A save's
  result (`saved`, `save_failed`) appears here after the `save`.
- `state_set`: present and `true` when the state was replaced.
- `frame`: when subscribed with `frame`, rendered after the change.

A client's own request is answered before the event it caused.

## Effects

`update` returns effects instead of performing them: `write_file {path, text}`,
`clipboard_set {text}` and `quit`.

- **`caretline serve`** never performs effects. They come back in the response. A client
  that performs one can report the result as a message (`{"msg":"saved"}` or
  `{"msg":"save_failed","err":"…"}`). `apply_effects: true` is refused (`unsupported`).
- **A live editor** performs effects for the local user's own input, as always. For pushed
  `msgs` and `keys` it returns them unperformed, unless the request sets
  `"apply_effects": true`: then the editor performs them (writes the file, sets the
  clipboard, quits) and applies the results (`saved`), and the response has
  `"executed": true`.

Note that a pushed `save` without `apply_effects` leaves the state waiting for a result
(`saving` is set) until a `saved` or `save_failed` message arrives.

## Errors

`{"id":…,"error":{"kind":…,"message":…}}`. The id is echoed whenever the line parsed as
JSON with one. Nothing changes on an error.

| `kind` | When |
|---|---|
| `parse` | The line isn't JSON. |
| `bad_request` | Not an object, no `op`, a missing or ill-typed parameter, an unknown message, `w`/`h` of 0. |
| `unknown_op` | An `op` this version doesn't know (the message lists the known ones). |
| `bad_keys` | A key script with an unknown key or modifier. |
| `stale` | `if_rev` didn't match the current rev. |
| `unsupported` | `apply_effects` on a server that doesn't perform effects. |

## The client: `caretline send`

```sh
caretline send [--socket PATH | --pid N | --latest] [--raw] [--apply-effects] [REQUEST…]
```

The target defaults to `--latest`: the most recently started editor whose socket answers
(discovery files of editors that are gone are removed). `REQUEST` is a JSON request or a
shorthand:

| Shorthand | Request |
|---|---|
| `hello`, `state.get`, `trace.get`, `unsubscribe` | that op |
| `render [WxH] [text\|ansi\|cells]` | `render` |
| `keys SCRIPT` | `keys` |
| `msgs FILE\|-\|JSON` | `msgs` (JSON Lines or an array, like `--msgs`) |
| `set-state FILE\|-` | `state.set` |
| `status TEXT` | `msgs` with one `show_status` |
| `subscribe [WxH [format]]` | `subscribe`, then prints events until interrupted |

It prints the response line and exits non-zero on an error. `--raw` prints the payload
instead: the frame for `render`, the state (pretty) for `state.get`, the trace (JSON Lines)
for `trace.get`. With no `REQUEST`, it sends each stdin line as a request and prints
everything the server sends back until stdin ends and every response is in.

```sh
caretline notes.md --listen                     # in one pane
caretline send --latest state.get --raw > s.json
caretline send --latest keys '<c-end><cr>Added from outside'
caretline send --latest render 80x24 --raw
caretline send --latest set-state s.json
caretline send --latest status 'build finished'
caretline send --latest subscribe               # watch every change
printf '%s\n' '{"id":1,"op":"hello"}' '{"id":2,"op":"render","format":"cells"}' | caretline send --latest
```

## In process: `Session`

`caretline_next::Session` is the same engine without a transport: a state, its rev and its
trace. Use it from Rust tests or to embed the editor.

```rust
use caretline_next::{Msg, Session, State, Viewport};

let mut s = Session::new(State::new("hello\n", None, Viewport { width: 40, height: 10 }));
let effects = s.apply(Msg::InsertText { text: "> ".into() });     // effects returned, not run
let (msgs, effects) = s.keys("<end> world<s-a-left>")?;
let frame = s.render(40, 10);                                    // a Frame (cells); to_text(), to_ansi()
assert_eq!(s.rev(), 1 + msgs.len() as u64);
let replay = s.trace_jsonl();                                    // `caretline --replay` input

// The protocol itself, one request line at a time:
let handled = s.handle(r#"{"id":1,"op":"state.get"}"#, None);
println!("{}", handled.response);
```

`Session::apply_with(msg, &mut exec)` performs effects with a closure and feeds its result
messages back. `Session::handle` returns the response line plus the `Change` it made (for
events, built with `protocol::event_line`) and any `subscribe`/`unsubscribe` control.

## Performance

`caretline bench` (build with `--release`) measures the session in process and over a
socket, on 1,000- and 100,000-line documents. Indicative numbers on an Apple-silicon Mac:

| Operation | In process | Socket round trip |
|---|---|---|
| `hello` | | ~11 µs |
| `msgs`, one edit (type or backspace) | ~1–2 µs | ~12–14 µs |
| `keys "<down>"` | | ~30–37 µs |
| `render` 100x40 (`text` or `cells`) | ~85 µs | ~95–100 µs |
| `state.get`, 1,000 lines (fresh history) | ~22 µs | ~80 µs |
| `state.get`, 100,000 lines (6.6 MB) | ~2.5 ms | ~7 ms |

Render and update cost depends on the visible rows, not the document's length. `state.get`
is proportional to the text plus the undo history, which grows with every edit; for a large
document, prefer `render` or events to repeated `state.get`.
