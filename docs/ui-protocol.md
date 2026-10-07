# The UI protocol

thc's TUI is state-driven, in the Elm style. Everything the TUI shows is decided by one
serializable value, the **UI state**. It changes only through **messages**, and the screen is
drawn from that state plus data read from the vault. Because of that, other programs can
drive the TUI through the state:

- **read** what a running TUI shows (`thc ui state`) and **push** a new state into it
  (`thc ui set`, `thc ui patch`): an agent can open the person's view on a task, a filter or a
  page,
- **send it keys** and **render** its frame (`thc ui send keys …`, `thc ui render`),
- **watch** it change, keystroke by keystroke (`thc ui send subscribe`),
- **render a state with no TUI running** (`thc ui render --state FILE`), and
- **record** a session and **replay** it to the same frames (`thc tui --trace`, `thc ui replay`).

The wire format follows caretline's [state protocol](caretline/protocol.md): JSON lines,
`rev` and `if_rev`, `subscribe`, traces and replay. The editor inside the TUI is caretline, so
the two read and feel the same.

## Quick start

In one terminal, run the TUI:

```sh
thc
```

In another, on the same vault:

```console
$ thc ui ls
85621    acme           /dev/ttys003   /var/folders/…/T/thc-ui-85621.sock
$ thc ui state --raw | jq '{view, tasks_filter, selected}'
{"view": "today", "tasks_filter": "status:open sort:due", "selected": "23deakf0w6s6"}
$ THC_ACTOR=claude thc ui patch '{"view": "tasks", "tasks_filter": "status:open #health"}'
{"rev":1,"changed":["view","cursor","selected","tasks_filter","toast","history"]}
```

The person's TUI switches to Tasks, filtered to `#health`, and the bar says who did it. ⌘[
(⌃⌥← where ⌘ doesn't reach the terminal) goes back, as for any step. See the frame:

```console
$ thc ui render 100x24
 [•] acme   Today   Inbox 2   Tasks   Pages   Journal   Search   Log 1            Wed Oct 7 · 15:45
────────────────────────────────━━━━━━━─────────────────────────────────────────────────────────────
 q› status:open #health                                                                      1 task

▌ ny82x  [ ] Call dentist to reschedule #health                                     due fri · !high
…
 ◆ claude changed your view · ⌃⌥← back                                             L review  u undo
```

## The UI state

`thc ui state --default` prints the state a new TUI on this vault starts with, and every field
is there. The main groups are:

| Group | Fields |
|---|---|
| The screen | `view` (`today` `inbox` `tasks` `pages` `journal` `search` `log`), `cursor`, `scroll`, `selected` (a row's stable key: a node id, `tag:x`, `view:x`, `tx:x`), `show_detail`, `focus` (`list`/`detail`), `focus_mode`, `focus_cfg`, `collapsed`, `page_ids` |
| Filters and per-view choices | `tasks_filter` (a query, as `thc q` reads it), `search_terms`, `pages_filter`, `log_actor`, `log_node`, `review_lane`, `agenda_mode`, `show_all_done`, `context_on`, `scope_override`, `today_by_vault` |
| Documents | `page_open`, `journal_date`, `document` (caret and scroll: see below), `doc_write`, `doc_parked`, `doc_back`, `doc_origin`, `rail_frozen`, `last_page`, `recent_docs`, the `[[` popup (`link_open`, `link_sel`) |
| Input | `edit` (a row being edited in place), `prompt`, `awaiting` (a confirmation), `overlay` (palette, finder, move, capture, help, recipe, history, vaults, scope, about, compare, focus), `pending_keys` (a key sequence in progress, in keymap notation), `input_untouched`, `recent_cmds`, `recent_moves` |
| The pointer | `hover`, `scroll_drag`, `drag_from`, `click_link`, `last_click` |
| Notices | `toast`, `flashes`, `alert_toast_node`, `update_state`, and the once-only hints |
| Time | `now_ms`, `utc_offset_min`, `today` (see [Time](#time)) |
| History | `history` (the ⌘[ / ⌘] stack: `entries`, `pos`, `tab_run`) |
| Identity | `ui_state_version` (1), `vault_name` (read-only) |

What's **not** in the state:

- **Data:** rows, counts, labels and previews. They're derived from the vault each time the
  state changes, so a state names a node by its id and never carries its text.
- **The document's text and undo:** these live in the editor's own model. `document` carries
  only presentation: `caret_id` and `caret_byte` (an anchor that survives edits elsewhere),
  `scroll`, and, reported but ignored on writes, `target`, `dirty` and `revision`. A state file
  can't carry unsaved typing.
- **Runtime handles:** the vault connection, channels, terminal capabilities, the last frame's
  hit regions and the image protocol. They live beside the state, not in it.

Within `ui_state_version` 1 the state only gains fields. Panels that don't exist yet (the
sidebar's stack of pages) arrive as new fields, and a client that doesn't know them can ignore
them.

### Writing a state

`state.set` (`thc ui set FILE`) replaces the state. **Fields you leave out take their
defaults**, except the session's own (`now_ms`, `utc_offset_min`, `today`, `vault_name`,
`journal_date`). `history` and the recent lists are kept too unless you send them. So a
minimal state is just what you care about:

```json
{"view": "pages", "page_open": "k7q2m9x4b1c8"}
```

`patch` (`thc ui patch JSON`) is an [RFC 7396](https://www.rfc-editor.org/rfc/rfc7396) JSON
merge patch: objects merge, arrays and values replace, `null` clears a field. Use it for small
changes; use `state.set` for whole states, such as the output of `thc ui state`, which `set`
accepts as is.

Both are validated whole before anything changes:

- An unknown field is refused, with the nearest known one (`unknown field 'veiw' · did you
  mean 'view'?`), so an older thc never quietly ignores a field a newer one wrote.
- Wrong types, unknown enum values and an unsupported `ui_state_version` are refused.
- `vault_name` can't change: a state never switches vaults.
- History holds at most 100 entries and filters at most 4,096 bytes.

A refused write changes nothing, not even `rev`.

A pushed state lands the way a step you took would:

- Where you were becomes a history step, so ⌘[ goes back.
- A document it opens is opened *parked*, never mid-sentence in Write.
- The caret goes to `document.caret_id` / `caret_byte` when given.
- An agent's change says so in a toast.

Unsaved typing in the open document is never discarded: leaving it saves it, exactly as
leaving it from the keyboard does.

## Messages

Every change to the state is one message. A **trace** is the list of them. The terminal's own
keys are messages too, so a trace records what the person did and what clients did, in the
order they happened.

| Message | Fields | What it is |
|---|---|---|
| `tick` | `now_ms`, `utc_offset_min` | The clock moved (see [Time](#time)) |
| `key` | `key` | One key as a [key-script token](#key-scripts): `j`, `<cr>`, `<c-o>` |
| `mouse` | `kind` (`down` `up` `drag` `moved` `scroll_up` `scroll_down` `middle_down` `middle_up`), `x`, `y`, `mods` (`c` `m` `s`), `clicks` | A pointer event. Without `clicks`, a press within 400 ms on the same cell counts up (double, triple) |
| `paste` | `text` | A bracketed paste (into the open document) |
| `resize` | `w`, `h` | The terminal's size |
| `focus` | `gained` | The terminal gained or lost focus (losing it saves) |
| `set_state` | `state`, `actor` | `state.set` |
| `patch` | `patch`, `actor` | `patch` |
| `external` | `patch` | A change that came from outside any message, as a merge patch: the daemon's push (an agent's write flashing, an alert toast), a poll that found another device's change, an idle save, an update check. The runtime records it so a trace and subscribers see every change; replay applies it |

```json
{"msg":"key","key":"<c-o>"}
{"msg":"mouse","kind":"down","x":12,"y":5}
{"msg":"patch","patch":{"view":"log"},"actor":"claude"}
```

After each message the layout follows the caret and the list cursor at the session's size, so
where a view scrolls depends only on the messages, never on when a frame happened to be drawn.

### Effects

Applying a message can ask the runtime to do something outside the state. These come back as
**effects**:

| Effect | Asks for |
|---|---|
| `quit` | Leave the TUI |
| `reexec` | Hand over to a newer thc binary |
| `spawn_editor` | `$EDITOR` on a note (`target`), a saved view, the keys or the config |
| `set_mouse` | Mouse capture on or off |
| `switch_vault` | Reopen on another vault (`path`) |

A running TUI performs the effects of its own keys. A client's `msgs` and `keys` only
**report** them unless the request sets `apply_effects: true`, so a script that sends `q`
doesn't close the person's TUI by accident.

### Key scripts

`keys` requests, `THC_TUI_KEYS` and `thc ui send keys` all read the same script:

| Token | Is |
|---|---|
| any character | itself (`j`, `G`, `?`) |
| `<cr>` `<esc>` `<tab>` `<bs>` `<del>` `<space>` `<up>` `<down>` `<left>` `<right>` `<home>` `<end>` `<pgup>` `<pgdn>` `<f1>`…`<f12>` `<lt>` | named keys (`<lt>` is `<`) |
| `<c-x>` `<m-x>` `<s-x>` `<d-x>` | ⌃ ⌥ ⇧ ⌘ with a key, combined in that order: `<c-m-left>`, `<s-tab>` |
| `<click:x,y>` `<dclick:x,y>` `<tclick:x,y>` `<sclick:x,y>` `<cclick:x,y>` `<aclick:x,y>` `<mclick:x,y>` | clicks (double, triple, with ⇧ ⌃ ⌥, middle) |
| `<drag:x1,y1,x2,y2>` `<wheel:up\|down[:n][@x,y]>` `<hover:x,y>` | the rest of the mouse |
| `<paste:TEXT>` | a paste; `\n` is a line break |

Every key a terminal can send has a token, so a trace records any key losslessly.
`THC_TUI_KEYS` also reads the test fixtures `<agent:TEXT>`, `<remote:ID:TEXT>`, `<alert>` and
`<nop>`, and passes over tokens it doesn't know. The protocol refuses both.

## Time

Nothing in the update or the view reads a clock. Time arrives as `tick` messages, and toasts,
flashes, double clicks, the which-key delay and the bar's clock are all measured against the
state's `now_ms`. A running TUI ticks before input, and while something on screen is waiting
on time. With `THC_NOW` pinned the clock never moves, so a pinned run is the same every time.

## Revisions

`rev` goes up by one for every message applied: ticks, resizes and the person's keys too. Use
it two ways:

- **Did something change?** Compare it with the `rev` you last saw. `trace.get` with
  `since_rev` lists what you missed.
- **Write only if nothing changed:** pass `if_rev` on `state.set`, `patch`, `msgs` or `keys`
  (`thc ui patch … --if-rev N`). If the rev has moved, nothing is applied and you get `stale`
  (exit 4). Re-read, then decide.

## Operations

A request is a JSON object with an `op` and an optional `id`, which is echoed back. A success
is `{"id", "result"}` and a failure is `{"id", "error": {"kind", "message"}}`. Every result
carries `rev`.

| Op | Request fields | Result |
|---|---|---|
| `hello` | | `proto` (1), `version`, `rev`, `ops`, `vault`, `size` |
| `state.get` | `history` (default true) | `rev`, `state`. With `history: false`, without `history`, `recent_docs`, `recent_cmds` and `recent_moves` |
| `state.set` | `state`, `if_rev`, `actor` | `rev`, `changed` (the top-level fields that changed) |
| `patch` | `patch`, `if_rev`, `actor` | `rev`, `changed` |
| `msgs` | `msgs` (messages), `if_rev`, `apply_effects`, `actor` | `rev`, `effects`, `msgs` (every message applied, a leading `tick` included), `executed` when effects were performed |
| `keys` | `keys` (a key script), `if_rev`, `apply_effects`, `actor` | Like `msgs` |
| `render` | `w`, `h` (default: the TUI's size), `format` | `rev`, `w`, `h`, `format`, `cursor` (`[x, y]` or null), and `frame` (text, ansi, html) or `rows` (cells) |
| `subscribe` | `frame` (`{w, h, format}`), `with_msgs` (default true), `with_state` | `rev`, `subscribed: true`, then events |
| `unsubscribe` | | `rev`, `subscribed: false` |
| `trace.get` | `since_rev` or `all` | `rev`, `from_rev`, `trace` |
| `trace.checkpoint` | | `rev`. Starts a new trace segment from the current state |

`render` never changes the state: a size other than the TUI's is drawn and then put back.

### Frame formats

| `format` | Result |
|---|---|
| `text` (default) | `frame`: the rows as text, trailing spaces trimmed, as `THC_TUI_SNAPSHOT` prints |
| `ansi` | `frame` with ANSI colours and modifiers |
| `html` | `frame` as a styled HTML page |
| `cells` | `rows`: one `{text, spans}` per row, `spans` listing style runs as `[x, len, fg, bg, modifiers]` |

### Subscribe

After `subscribe`, the connection gets a line for every change from any source: the person at
the keyboard, any client, or the runtime.

```json
{"event":"state","rev":57,"source":"terminal","msgs":[{"msg":"tick","now_ms":1791367201000,"utc_offset_min":120},{"msg":"key","key":"j"}]}
{"event":"state","rev":58,"source":"client","state_set":true,"msgs":[{"msg":"patch","patch":{"view":"log"},"actor":"claude"}]}
```

| Field | Meaning |
|---|---|
| `source` | `terminal` (the person, or the runtime's ticks) or `client` (a request) |
| `state_set` | `true` for `state.set` and `patch` |
| `msgs` | The messages applied (unless `with_msgs: false`) |
| `frame` | A rendered frame, when you subscribed with `frame` |
| `state` | The state without its history, with `with_state: true` |

### Errors

| `kind` | When | `thc ui` exits |
|---|---|---|
| `parse` | The line isn't JSON | 2 |
| `bad_request` | A missing or unknown field, a bad message, a fixture | 2 |
| `unknown_op` | Not one of `hello`'s `ops` | 2 |
| `bad_keys` | The key script doesn't parse | 6 |
| `invalid` | The state or patch doesn't validate | 6 |
| `stale` | `if_rev` didn't match | 4 |
| `trimmed` | `since_rev` is older than the kept trace | 3 |
| `unsupported` | `apply_effects` where effects can't be performed | 2 |

Within protocol version 1, results only gain fields. Ignore the ones you don't know.

## Traces

A trace is JSON lines: a `state` line, then messages.

```json
{"state":{"ui_state_version":1,"view":"today",…},"size":[100,30],"rev":0}
{"msg":"key","key":"3","_rev":1}
{"msg":"patch","patch":{"tasks_filter":"#work"},"actor":"claude","_rev":2}
```

`_rev` is the rev after the message. A `state` line restores that state exactly, with no
history step and no toast. A running TUI keeps its trace in **segments**: each starts with a
`state` line, at the start and at every `trace.checkpoint`, and replays on its own. It keeps
at most 100,000 lines and drops older segments first.

```sh
thc tui --trace session.jsonl          # record a whole session
thc ui send trace.get all --raw > t.jsonl    # or take a running TUI's
thc ui replay t.jsonl                  # the last frame
thc ui replay t.jsonl --every          # a frame per line, separated by form feeds
thc ui replay t.jsonl --size 120x40 --format ansi
```

**Replay is deterministic.** With `THC_NOW` pinned, the same trace on the same vault gives the
same bytes every run. Replay runs on a scratch copy of the vault, so the keys in a trace can
write without touching the real one. A trace records the UI, not the data. What changed in the
*state* because of something from elsewhere (a toast, a flash, a moved caret) is in the trace
as `external` messages and replays. The other device's notes themselves are not, so replaying
on a vault in a different state can draw different rows.

## Headless render

`thc ui render --state FILE` draws a state on a scratch copy of the vault, with no TUI and no
terminal. `-` reads stdin.

```sh
echo '{"view":"tasks","tasks_filter":"status:open due<=+7d"}' | thc ui render 100x30 --state -
thc ui state --default > s.json   # a starting point: edit it, render it
THC_NOW=2026-10-07T10:00 thc ui render 120x32 --state s.json --format html > review.html
```

It reads the vault and draws exactly what `thc` would show for that state. With `THC_NOW`
pinned the output is identical on every run.

## Sessions and transport

Every running TUI listens on a Unix socket, `$TMPDIR/thc-ui-<pid>.sock` (or
`/tmp/thc-ui-<uid>/<pid>.sock` when `$TMPDIR` is too long for a socket path). The socket is
readable only by its user, and the TUI checks the peer's uid. Each TUI also writes
`$TMPDIR/thc-ui/<pid>.json` with its pid, socket, vault name and path, tty, protocol and start
time. Both files go when the TUI exits. `THC_TUI_NO_LISTEN=1` turns the socket off.

`thc ui` finds TUIs through those files, **restricted to the current vault**. With one TUI on
the vault, it's used. With none, the command exits 3. With several, it exits 5 and lists the
candidates; pass `--session PID`. `thc ui ls --all` lists every vault's.

Requests go through the same queue as the person's keys, so a session has one order of events.
Between frames the TUI checks for requests every 10 ms, so a request is answered within a few
milliseconds without drawing extra frames.

**Who may steer:** reading (`state`, `render`, `subscribe`, `trace.get`) is open to any actor
on the machine. Changing the view (`set`, `patch`, `keys`, `msgs`) is refused for read-tier
agents (exit 6; an actor's tier is set in `[actors.<name>]` of the device config, see [features](guide/features.md)). `THC_ACTOR` names the actor, and an agent's
change shows in the TUI's toast. UI control never writes to the vault's log by itself: only
keys that edit (typing into a document, `x` on a task) write, exactly as they would from the
keyboard.

## `thc ui`

| Command | Does |
|---|---|
| `thc ui ls [--all]` | The running TUIs on this vault (every vault with `--all`). `--json` for the discovery records |
| `thc ui state [--no-history] [--raw] [--default]` | The state: `{rev, state}`, or just the state with `--raw`. `--default`: what a new TUI starts with (no TUI needed) |
| `thc ui set FILE\|- [--if-rev N]` | `state.set` |
| `thc ui patch JSON\|@FILE [--if-rev N]` | `patch` |
| `thc ui send …` | `keys SCRIPT`, `msgs FILE\|-\|JSON`, `render [WxH] [FORMAT]`, `state.get [no-history]`, `subscribe [WxH [FORMAT]] [state]`, `trace.get [all\|since REV]`, `trace.checkpoint`, `hello`, or a raw JSON request. With no words, JSON requests from stdin, one per line. `--raw` prints the payload; `--apply-effects` lets the TUI perform effects |
| `thc ui render [WxH] [--format F]` | A running TUI's frame |
| `thc ui render [WxH] --state FILE` | A state's frame, headless |
| `thc ui replay FILE [--size WxH] [--format F] [--every]` | Replay a trace headlessly |
| `thc tui --trace FILE` | Run the TUI, recording its trace |

All take `--session PID` where a running TUI is meant.

### A client in a few lines

```python
import json, socket, subprocess
sessions = json.loads(subprocess.check_output(["thc", "--json", "ui", "ls"]))["sessions"]
s = socket.socket(socket.AF_UNIX); s.connect(sessions[0]["socket"])
f = s.makefile("rw")
def ask(req):
    f.write(json.dumps(req) + "\n"); f.flush()
    return json.loads(f.readline())
rev = ask({"op": "state.get", "history": False})["result"]["rev"]
print(ask({"op": "patch", "patch": {"view": "inbox"}, "if_rev": rev, "actor": "my-script"}))
print(ask({"op": "render", "w": 80, "h": 24})["result"]["frame"])
```

## How it's built

The TUI (`crates/thc-tui`) splits into the parts the Elm architecture names:

- **State:** `ui_state.rs`'s `UiState`, serde with strict parsing and ordered maps (the same
  state always serializes to the same bytes). `App` holds it beside the runtime's handles and
  derefs to it.
- **Messages and the session:** `session.rs`. `Session::apply` is the one door for every
  input: it applies a message, lets layout follow, records the trace and reports effects. The
  terminal loop, `THC_TUI_SNAPSHOT`, `thc ui render` and `thc ui replay` all drive a `Session`.
- **Update:** `update.rs`. `update::action` is the pure update for presentation actions:
  views, cursor motion, prompts, overlays, panes, Focus, Today's toggles, help, quit. It takes
  `&mut UiState` and derived rows and returns effects. `update::update` handles viewport follow,
  clipboard, key remaps and the IDs toggle the same way.
- **Effects:** `runtime_effects.rs` performs effects (reload, save, clipboard, cache writes,
  the editor, mouse mode) and feeds results back as messages.
- **Derived data:** `derived.rs` and the `*_snapshot.rs` files, filled from the vault before
  each frame.
- **View:** `ui.rs`, `doc_ui.rs` and `node_row.rs`. They draw from the state and derived data
  only. A test fails if they reach the store, a clock, the environment, a file or a process.
- **Protocol:** `ui_proto.rs` (requests) and `ui_server.rs` (socket, discovery, subscribers).

### What isn't pure yet

The direction is a pure `update(&mut UiState, Msg) -> Vec<Effect>` for everything. These parts
still run the older way, inside `Session::apply`, with the vault in reach:

- **Write actions:** done, status, priority, dates, tags, text, move, delete, undo, review,
  capture, the palette's commands, in-place row editing (`App` methods in `app.rs`, reached
  from `keymap.rs` and `input.rs`). They read and write the store directly and set state
  around it. They're still single messages in the trace and replay deterministically on the
  same vault, but they aren't `update` functions yet.
- **The document's key path:** `doc_keys.rs` and `doc_app.rs` drive the editor model and save
  through the writer thread on `App`. Its clock is `UiState::now_ms`, so typing runs and idle
  saves replay, but new lines' ids are minted by the runtime as they're needed and aren't in
  the trace.
- **Background work in the terminal loop:** the daemon's pushes and the log poll
  (`drain_live`, `poll_external`), the update check, the registry check, idle saves
  (`doc_tick`) and the in-place update's progress (`drain_update`) still run as runtime code
  on `App`. What they change in the state is caught between frames and recorded as an
  `external` message, so traces and subscribers see it, but they aren't messages with an
  update of their own yet.
- **Frame preparation:** `derived::prepare` and `ui::prepare_frame` read the store, settings
  and attachment metadata every frame. This is the explicit derivation stage, run by the
  runtime before the view; it isn't the view, but it isn't memoized on revisions everywhere
  yet.
- **Process-wide flags:** the `SNAPSHOT` thread-local turns real IO (clipboard, opening
  files) off for headless sessions, and a few caches (image encodings, attachment probes) are
  runtime state.
- **Per-device memory:** caret memory per document and scope overrides are cache files
  written by `App`. Scope overrides are part of the state; caret memory isn't.
