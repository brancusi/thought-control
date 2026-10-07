# caretline-next

A standalone text-editing engine: Helix's editing model (rope, multi-range selections,
transactions, undo history, grapheme-correct motion, soft wrap) inside a strict Elm
architecture. `State` serializes to JSON and back without loss, `update(&mut State, Msg) ->
Vec<Effect>` is pure and deterministic, and `view(&State) -> Frame` is a pure cell grid. No
terminal I/O and no ratatui.

The terminal front end is [`caretline-app`](../caretline-app/README.md).

## Documentation

The full documentation is in [docs/caretline](../../docs/caretline/README.md):

| Page | Covers |
|---|---|
| [Overview](../../docs/caretline/README.md) | What it is, what it covers, the layers |
| [Architecture](../../docs/caretline/architecture.md) | State, Msg, update, Effect, view; purity, revisions, replay; Helix inside |
| [Rust API](../../docs/caretline/api.md) | The public API by task, with a complete example ([`examples/basic.rs`](examples/basic.rs)) |
| [Messages](../../docs/caretline/messages.md) | Every message, effect and key binding; the key-script syntax |
| [CLI](../../docs/caretline/cli.md) | The `caretline` command, end to end |
| [Protocol](../../docs/caretline/protocol.md) | The JSON-lines state protocol (landing next) |
| [Embedding](../../docs/caretline/embedding.md) | Use it in your own app, as a library or a process; licensing |
| [Testing](../../docs/caretline/testing.md) | Goldens, replay, the fuzzer, fixtures, tests from traces |

```sh
cargo run -p caretline-next --example basic
```

## License

MIT, except `src/helix/`, which is vendored from [Helix](https://github.com/helix-editor/helix)
and stays under the Mozilla Public License 2.0 (file-level; see `src/helix/README.md` and
`src/helix/LICENSE-MPL-2.0`).
