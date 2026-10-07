# caretline-next

A standalone text-editing engine: Helix's editing model (rope, multi-range selections,
transactions, undo history, grapheme-correct motion, soft wrap) inside a strict Elm
architecture. `State` serializes to JSON and back without loss, `update(&mut State, Msg) ->
Vec<Effect>` is pure and deterministic, and `view(&State) -> Frame` is a pure cell grid. No
terminal I/O and no ratatui.

The terminal front end is [`caretline-app`](../caretline-app/README.md).

## License

MIT, except `src/helix/`, which is vendored from [Helix](https://github.com/helix-editor/helix)
and stays under the Mozilla Public License 2.0 (file-level; see `src/helix/README.md` and
`src/helix/LICENSE-MPL-2.0`).
