# caretline

A standalone text-editing engine: Helix's editing model (rope, multi-range selections,
transactions, undo history, grapheme-correct motion, soft wrap) inside a strict Elm
architecture. `State` serializes to JSON and back without loss, `update(&mut State, Msg) ->
Vec<Effect>` is pure and deterministic, and `view(&State) -> Frame` is a pure cell grid. No
terminal I/O and no ratatui.

**Scope.** caretline is a text-editing engine only: text and navigation, carets and
selections, undo and redo, generic structure (marks with payloads, blocks, folds, block
operations), views, changes from elsewhere, rendering the editing surface, and state, messages,
replay and the protocol. What your text *means* (tasks, statuses, anything) belongs in your app,
added through host commands, input rules, mark payloads and decorations
([Extending the engine](../../docs/caretline/embedding.md#extending-the-engine)).

`Session` wraps a state with a revision counter and the trace of everything applied, and
answers the [state protocol](../../docs/caretline/protocol.md) in process (`Session::handle`).

The terminal front end is [`caretline-cli`](../caretline-app/README.md). Try it in one line:

```sh
curl -fsSL https://caretline.app/install.sh | sh && caretline demo
```

## Documentation

The full documentation is in [docs/caretline](../../docs/caretline/README.md):

| Page | Covers |
|---|---|
| [Overview](../../docs/caretline/README.md) | What it is and isn't, what belongs in your app, the layers |
| [Architecture](../../docs/caretline/architecture.md) | State, Msg, update, Effect, view; purity, revisions, replay; Helix inside |
| [Rust API](../../docs/caretline/api.md) | The public API by job, with a complete example ([`examples/basic.rs`](examples/basic.rs)) |
| [Messages](../../docs/caretline/messages.md) | Every message, effect and key binding; the key-script syntax |
| [Keys and commands](../../docs/caretline/keys.md) | The command catalog and the default keymap as data |
| [Structure](../../docs/caretline/structure.md) | Blocks, marks and payloads, folds, the layout, decorations |
| [Markdown](../../docs/caretline/markdown.md) | The block grammar, its editing rules, Markdown in and out |
| [CLI](../../docs/caretline/cli.md) | The `caretline` command, end to end |
| [Protocol](../../docs/caretline/protocol.md) | The JSON-lines state protocol |
| [Embedding](../../docs/caretline/embedding.md) | Use it in your own app, as a library or a process; extend it; licensing |
| [Testing](../../docs/caretline/testing.md) | Goldens, replay, the fuzzer, fixtures, tests from traces |

```sh
cargo run -p caretline --example basic
```

## License

MIT, except `src/helix/`, which is vendored from [Helix](https://github.com/helix-editor/helix)
and stays under the Mozilla Public License 2.0 (file-level; see `src/helix/README.md` and
`src/helix/LICENSE-MPL-2.0`).
