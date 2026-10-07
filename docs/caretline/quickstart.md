# Quickstart

Try caretline in one line. This installs the `caretline` binary and opens a guided tour:

```sh
curl -fsSL https://caretline.app/install.sh | sh && caretline demo
```

The installer picks the build for your machine (macOS or Linux, Apple silicon, ARM or x86-64),
checks its sha256 and puts `caretline` in `~/.local/bin`. Nothing else is touched, except a
line that adds `~/.local/bin` to your `PATH` in your shell's startup file when it isn't there
yet. If the shell you ran it in doesn't have that directory on its `PATH`, the installer prints
the `export` line to run first.

Prefer to build it yourself? With a Rust toolchain:

```sh
cargo install --git https://github.com/brancusi/thought-control caretline-app
```

## Three demos

Each demo writes its files to a fresh temporary directory, so there is nothing to set up and
nothing to clean. When it quits, it says where the files are.

### `caretline demo`: the tour

A short outline you work down by doing. The status bar always names the next step.

| Step | You try |
|---|---|
| 1. Type | Type past the edge of the window: lines wrap at word boundaries |
| 2. Tasks and nesting | `⌃T` turns an item into a task and ticks it; `Tab`/`⇧Tab` nest; `⌥↑`/`⌥↓` move an item with its children |
| 3. Select, undo, redo | `⇧` with any arrow selects; `⌃Z` and `⌃Y` bring back the text, caret and selection exactly |
| 4. Folds | `⌃O` folds the item under the caret, and opens it again |
| 5. One state | `⌃D` writes the whole editor (text, carets, undo history, folds, scroll, clock) to `state.json` and says how big it is |
| 6. Replay | `⌃P` replays your session, from the first state through every message, and checks it lands on the identical state |

Tick each step's `Done` box with `⌃T`. `⌃Q` quits (twice to leave without saving).

`⌃O`, `⌃D` and `⌃P` are the demo's own keys. In your own program the same things are a
[`toggle_fold`](messages.md#views-and-folds) message, `State::to_json` and
[`replay_trace`](api.md).

### `caretline demo scenes`: the frame path

Six ASCII animations (a warp field around the CARETLINE logo, a shaded donut, a wireframe
cube, an XOR tunnel, plasma and fire) play inside the real editor. A client on the editor's
own socket sends each frame as one [`frame`](protocol.md#frames) request: a whole new
document, its brightest cells sent as selections. `←` and `→` change the scene, `q` quits.

With `--bench`, it plays them into an editor that is already running instead and reports the
frame rates achieved (see [Performance](performance.md)):

```sh
caretline notes.md --listen          # in one terminal
caretline demo scenes --bench        # in another
```

### `caretline demo agent`: co-editing

The editor starts with [`--listen`](protocol.md), and a scripted agent connects to its socket
like any other client. It opens a [view](protocol.md#views) of its own, shown as the pane
under yours, with its own caret, and types into its own section while you type in yours:

- **Every keystroke is a guarded write.** The agent reads the revision, then writes with
  [`if_rev`](protocol.md#revisions). If you typed in between, the editor refuses the write as
  `stale` and the agent reads again. Its status line counts the refusals.
- **Undo is yours.** The agent's text arrives as [changes from
  elsewhere](messages.md#changes-from-elsewhere), outside your undo history, so `⌃Z` takes back
  only what you typed.
- **The status bar says who is editing**, and when the agent is done, `⌃P` replays the whole
  session, both of you, to the identical state.

While it runs, the editor is also reachable from another shell:

```sh
caretline send --latest keys '<d-down>hello from another shell'
```

## Then

```sh
caretline notes.md               # edit a file
caretline --outline todo.md      # lists, tasks and folds
caretline --new-state notes.md --size 60x20 > s.json
caretline --state s.json --keys 'Hello<cr>' --snapshot 60x20   # headless
caretline notes.md --trace t.jsonl                             # record a session…
caretline --replay t.jsonl --snapshot 80x24                    # …and replay it
```

- [The caretline command](cli.md): the editor and the headless tools, end to end.
- [Architecture](architecture.md): the one `State`, `Msg`, `update` and `view`.
- [State protocol](protocol.md) and [MCP server](mcp.md): drive a live editor from a
  script or an agent.
- [Embedding](embedding.md): put the engine in your own program with `cargo add caretline`.

## Releases

Each `caretline-v*` tag builds the binary for `aarch64-apple-darwin`, `x86_64-apple-darwin`,
`x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` (static) and publishes it on the
[releases page](https://github.com/brancusi/thought-control/releases), as
`caretline-<version>-<target>.tar.gz` with a `.sha256` beside it. The installer takes
`CARETLINE_INSTALL_DIR` (default `~/.local/bin`), `CARETLINE_VERSION` (default the newest) and
`CARETLINE_NO_MODIFY_PATH=1`. The macOS builds aren't signed: a binary fetched with `curl`
isn't quarantined, so Gatekeeper doesn't stop it.
