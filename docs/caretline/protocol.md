# The state protocol (landing next)

> **Status: landing next.** The protocol is being built on the `editor/state-protocol`
> branch and is not on `main` yet. This page describes that branch as it is now: every
> example below was run against it. Names and fields may still change before it merges.

The state protocol lets another program drive a caretline engine over **JSON lines**: one
JSON request per line in, one JSON response per line out. Use it to:

- run a **headless engine** as a child process (`caretline serve`, on stdin and stdout),
- share one engine with **several clients** on a Unix socket (`caretline serve --socket`),
- **attach to a live editor** a person is using (`caretline FILE --listen`), and
- send one-off requests from a shell (`caretline send`).

The same operations are available in process through
[`Session`](api.md#session-landing-next).

## Quick start

```console
$ printf 'hello world\nsecond line\n' > notes.md
$ printf '%s\n' '{"id":1,"op":"hello"}' '{"id":2,"op":"keys","keys":"<a-right><s-a-right>"}' '{"id":3,"op":"render","w":30,"h":4}' \
    | caretline serve notes.md --size 30x4
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
| `msgs` | `msgs` (array of [messages](messages.md)), optional `if_rev`, `apply_effects` | `rev`, `effects`, and `executed: true` when effects were performed |
| `keys` | `keys` (a [key script](messages.md#key-scripts)), optional `if_rev`, `apply_effects` | Like `msgs`, plus `msgs`: what the script became |
| `render` | optional `w`, `h` (default: the state's viewport), `format` | `rev`, `w`, `h`, `format`, `cursor` (`[x, y]` or null), and `frame` or `rows` |
| `subscribe` | optional `frame` (`{w, h, format}`), `with_msgs` (default true) | `rev`, `subscribed: true`. Then events, below |
| `unsubscribe` | | `rev`, `subscribed: false` |
| `trace.get` | | `rev`, `trace`: the initial state and every message and replacement since, as [trace lines](architecture.md#traces-make-sessions-reproducible) |

`render` never changes the state. A size other than the viewport is rendered on a copy.

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
the key's message.

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
messages the editor applied (such as `saved`) count toward `rev` and show up in subscribe
events. `caretline serve` has no one to perform
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
| `caretline serve … --socket PATH` | Unix socket | Many | The process is stopped |
| `caretline FILE --listen [PATH]` | Unix socket, beside the live terminal editor | Many | The editor quits |

- `serve --trace T.jsonl` appends the initial state and every change to a trace, which
  `caretline --replay` reads.
- `--listen` with no path uses `$TMPDIR/caretline-<pid>.sock`. Put FILE before `--listen`,
  or FILE is read as the socket path.
- A socket path must fit the OS limit (about 100 bytes on macOS). A stale socket file that
  nobody answers on is replaced.
- All inputs (terminal events and every client's requests) go through one queue, so the
  editor applies them in one order, and the trace records that order. See
  [architecture.md](architecture.md#the-runtime-merges-inputs-into-one-queue-landing-next).

`caretline bench` measures the protocol's throughput and latency, in process and over a
socket. Build with `--release` for meaningful numbers.

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
{"result":{"rev":22,"effects":[],"msgs":[{"msg":"move","dir":"forward","by":"doc_end","extend":false},{"msg":"insert_text","text":"f"}, …]}}
$ caretline send render 40x5 --raw
hello world
second line
from another shell

 notes.md [+]                      3:19
$ caretline send --apply-effects keys '<c-s>'
{"result":{"rev":24,"effects":[{"effect":"write_file","path":"notes.md","text":"hello world\nsecond line\nfrom another shell"}],"executed":true,"msgs":[{"msg":"save"}]}}
$ caretline send --apply-effects keys '<c-q>'
{"result":{"rev":25,"effects":[{"effect":"quit"}],"executed":true,"msgs":[{"msg":"quit"}]}}
```

The save took two revs (22 to 24): the `save` message and the `saved` the editor fed back.

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
