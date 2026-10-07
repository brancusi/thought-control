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
{"id":1,"result":{"proto":1,"version":"0.1.0","rev":0,"ops":["hello","state.get","state.set","msgs","keys","render","subscribe","unsubscribe","trace.get"]}}
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
| `state.get` | | `rev`, `state` (the full [State](architecture.md#what-state-holds) JSON) |
| `state.set` | `state`, optional `if_rev` | `rev`. Replaces the state (repaired as on load) |
| `msgs` | `msgs` (array of [messages](messages.md)), optional `if_rev`, `apply_effects`, `now_ms` | `rev`, `effects`, `msgs` (every message applied: a leading `tick`, the request's messages, and fed-back results such as `saved`), and `executed: true` when effects were performed |
| `keys` | `keys` (a [key script](messages.md#key-scripts)), optional `if_rev`, `apply_effects`, `now_ms` | Like `msgs`; `msgs` shows what the script became |
| `render` | optional `w`, `h` (default: the state's viewport), `format` | `rev`, `w`, `h`, `format`, `cursor` (`[x, y]` or null), and `frame` or `rows` |
| `subscribe` | optional `frame` (`{w, h, format}`), `with_msgs` (default true) | `rev`, `subscribed: true`. Then events, below |
| `unsubscribe` | | `rev`, `subscribed: false` |
| `trace.get` | | `rev`, `trace`: the initial state and every message and replacement since, as [trace lines](architecture.md#traces-make-sessions-reproducible) |

`render` never changes the state. A size other than the viewport is rendered on a copy, so
the frame is byte for byte what `caretline --state S --snapshot WxH` prints.

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

### Frame formats

| `format` | Result |
|---|---|
| `text` (default) | `frame`: the rows as text, trailing spaces trimmed, as `--snapshot` prints |
| `ansi` | `frame`: the rows with ANSI styling, as `--snapshot --format ansi` prints |
| `cells` | `rows`: one `{text, spans}` per row. `spans` lists runs of non-text roles as `[x, len, role]` |

```json
{"id":9,"result":{"rev":3,"w":30,"h":4,"format":"cells","cursor":[11,0],"rows":[
  {"text":"hello there                   "},
  {"text":"second line                   "},
  {"text":"                              "},
  {"text":" notes.md [+]            1:12 ","spans":[[0,9,"status"],[9,4,"status_accent"],[13,17,"status"]]}]}}
```

Roles are `text`, `selection`, `status` and `status_accent`.

## Revisions

`rev` starts at 0 when the engine loads. It goes up by **one for every message applied** and
one for every `state.set`. That includes clock ticks, resizes and the `saved` message a
performed save feeds back. In a live editor, one key press is usually two revs: a `tick` and
the key's message. A client request is usually one more than its messages: the leading
`tick` (see [Time](#time)).

Use `rev` two ways.

- **Did something change?** Compare the `rev` you last saw. The difference is the number of
  changes you missed, and `trace.get` has them.
- **Write only if nothing changed:** pass `if_rev` on `state.set`, `msgs` or `keys`. If the
  rev differs, nothing is applied and you get a `stale` error. Re-read, then decide.

```console
$ caretline send '{"op":"msgs","msgs":[{"msg":"undo"}],"if_rev":0}'
{"error":{"kind":"stale","message":"rev is 12, the request expected 0"}}
caretline: the request failed
```

Reads (`hello`, `state.get`, `render`, `trace.get`) ignore `if_rev`.

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
| `msgs` | The messages applied, effect results included. Omitted with `with_msgs: false` |
| `frame` | A rendered frame, when you subscribed with `frame` |

A subscriber also gets events for its own requests, after the response.

## Errors

| `kind` | When |
|---|---|
| `parse` | The line isn't JSON. No `id` comes back |
| `bad_request` | Not an object, no `op`, a missing field (`state.set needs a state`), or a zero size |
| `unknown_op` | The op isn't one of `hello`'s `ops` |
| `bad_keys` | The key script doesn't parse (`unknown key <oops>`) |
| `stale` | `if_rev` didn't match the current rev |
| `unsupported` | `apply_effects` on a server that can't perform effects |

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
- `--no-status-bar` (on `serve`, the editor and `--new-state`) sets `config.status_bar` to
  false: every row shows text, for embedders and panels with their own chrome.
- `--listen` with no path uses `$TMPDIR/caretline-<pid>.sock`, or
  `/tmp/caretline-<uid>/<pid>.sock` when `$TMPDIR` is too long for a socket path. Put FILE
  before `--listen`, or FILE is read as the socket path.
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
| `keys "<down>"` | | ~30–37 µs |
| `render` 100x40 (`text` or `cells`) | ~85–90 µs | ~100 µs |
| `state.get`, 1,000 lines, fresh history | ~23 µs | ~80 µs |
| `state.get`, 100,000 lines (6.6 MB) | ~2.5 ms | ~7 ms |

Update and render cost follows the visible rows, not the document length. `state.get` grows
with the text and the undo history, so for a large document prefer `render` and events.

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
| `hello`, `state.get`, `trace.get`, `unsubscribe` | That op |
| `render [WxH] [text\|ansi\|cells]` | `render` |
| `keys SCRIPT` | `keys` |
| `msgs FILE\|-\|JSON` | `msgs` from a file, stdin or inline JSON |
| `set-state FILE\|-` | `state.set` |
| `status TEXT` | A `show_status` message: text in the editor's status bar |
| `subscribe [WxH [format]]` | `subscribe`, then prints events until interrupted |
| `'{"op":…}'` | A raw JSON request |
| (nothing) | Reads JSON requests from stdin, one per line |

| Flag | Does |
|---|---|
| `--socket PATH` | Connect to this socket |
| `--pid N` | Connect to the editor with this process id |
| `--latest` | Connect to the newest live editor (the default) |
| `--raw` | Print the payload: the frame for `render`, the state for `state.get`, JSON Lines for `trace.get` |
| `--apply-effects` | Ask the editor to perform the effects of `msgs` or `keys` |

`send` exits 1 if any response is an error.

## Drive a live editor from another shell

In one terminal, open a file and listen:

```sh
caretline notes.md --listen
```

The status bar says `listening on …/caretline-<pid>.sock`. In another shell:

```console
$ caretline send hello
{"result":{"proto":1,"version":"0.1.0","rev":3,"ops":["hello","state.get","state.set","msgs","keys","render","subscribe","unsubscribe","trace.get"]}}
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

The person at the keyboard sees each change as it lands. Without `--apply-effects`, `<c-s>`
would only return the `write_file` effect, and `<c-q>` would leave the editor running.

To watch everything that happens, including the person's typing:

```sh
caretline send subscribe 80x24
```

To save a copy of the live session and replay it later:

```sh
caretline send trace.get --raw > live.jsonl
caretline --replay live.jsonl --snapshot 80x24
```

## A client in a few lines

See [embedding.md](embedding.md#as-a-process) for a complete Python client that spawns
`caretline serve`, edits, saves and renders.
