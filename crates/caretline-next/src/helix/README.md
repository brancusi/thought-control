# Vendored Helix core

These files come from [Helix](https://github.com/helix-editor/helix) at commit
`ba40e547426b0f9896c8bdc699a4ab11f2b37dbc` and are licensed under the Mozilla Public License 2.0 (`LICENSE-MPL-2.0`).
MPL-2.0 applies per file: the rest of `caretline-next` is MIT.

| File here | Upstream |
|---|---|
| `selection.rs` | `helix-core/src/selection.rs` |
| `transaction.rs` | `helix-core/src/transaction.rs` |
| `history.rs` | `helix-core/src/history.rs` |
| `graphemes.rs` | `helix-core/src/graphemes.rs` |
| `movement.rs` | `helix-core/src/movement.rs` |
| `position.rs` | `helix-core/src/position.rs` |
| `doc_formatter.rs`, `doc_formatter/test.rs` | `helix-core/src/doc_formatter.rs` and its test module |
| `line_ending.rs` | `helix-core/src/line_ending.rs` |
| `chars.rs` | `helix-core/src/chars.rs` |
| `text_annotations.rs` | `helix-core/src/text_annotations.rs` (needed by the formatter) |
| `test.rs` | `helix-core/src/test.rs` (test helpers, compiled for tests only) |
| `stdx/rope.rs`, `stdx/range.rs` | `helix-stdx/src/rope.rs`, `helix-stdx/src/range.rs` |

Each file's header lists what changed from upstream. In short:

- Module paths point at `crate::helix` instead of `helix_core` and `helix_stdx`.
- Tree-sitter, textobject and regex code is removed, so no syntax or loader crates are needed.
- `History` takes caller-supplied millisecond timestamps instead of reading
  `std::time::Instant`, so editing stays a pure function of its inputs. It also gains
  `amend_current_revision`, which folds a run of typing into one undo step.
- `DocumentFormatter` gains `resume_at_row` and `indent_level`, so layout can restart inside a
  long soft-wrapped line at a row it already knows.
- The selection, transaction and history types derive serde, so editor state serializes.
  `Range::old_visual_position` and `Selection::primary_index` may be left out when
  deserializing.

`mod.rs` and this README are not from Helix.
