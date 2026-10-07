# Messages, effects and keys

Every input to the engine is a `Msg`, and every piece of work it hands back is an `Effect`.
Both are plain values with a JSON form. Use the JSON form in `--msgs` files, traces and the
[protocol](protocol.md).

## The JSON form

Messages are tagged with `"msg"` and effects with `"effect"`. Names are `snake_case`.
Variants without fields are just the tag:

```json
{"msg":"undo"}
{"msg":"insert_text","text":"hi"}
{"msg":"move","dir":"backward","by":"word","extend":true}
{"effect":"clipboard_set","text":"hi"}
```

A message file is one message per line (blank lines and `//` comments are skipped), or a
single JSON array.

## Messages

### Editing

Every edit applies at every range of the selection. An edit with a selection replaces or
deletes exactly the selection.

| Msg | JSON | Does |
|---|---|---|
| `InsertText { text }` | `{"msg":"insert_text","text":"hi"}` | Types `text` at every caret, replacing selections. Line breaks are converted to the document's line ending |
| `InsertNewline` | `{"msg":"insert_newline"}` | Inserts the document's line ending |
| `DeleteBackward` | `{"msg":"delete_backward"}` | Deletes one grapheme back, or the selection |
| `DeleteForward` | `{"msg":"delete_forward"}` | Deletes one grapheme forward, or the selection |
| `DeleteWordBackward` | `{"msg":"delete_word_backward"}` | Deletes back to the previous word start. At a line start, only the line break |
| `DeleteWordForward` | `{"msg":"delete_word_forward"}` | Deletes forward to the next word end. At a line end, only the line break |
| `DeleteToLineStart` | `{"msg":"delete_to_line_start"}` | Deletes back to the start of the visual row. At the row start, one grapheme |
| `DeleteToLineEnd` | `{"msg":"delete_to_line_end"}` | Deletes forward to the end of the visual row. At the row end, one grapheme |
| `KillLine` | `{"msg":"kill_line"}` | Deletes to the end of the document line, or the line break when already there |

### Motion and selection

| Msg | JSON | Does |
|---|---|---|
| `Move { dir, by, extend }` | `{"msg":"move","dir":"forward","by":"word","extend":false}` | Moves every caret. `extend` (default `false`) keeps the anchors, so the selection grows or shrinks |
| `Click { col, row, extend }` | `{"msg":"click","col":4,"row":0}` | Places the caret at a screen cell, or extends to it. Leaves one range |
| `Scroll { rows }` | `{"msg":"scroll","rows":-3}` | Scrolls the view (negative is up). The caret moves only if it would leave the view |
| `SelectAll` | `{"msg":"select_all"}` | Selects the whole document |
| `Collapse` | `{"msg":"collapse"}` | Collapses every range to its caret |

`dir` is `backward` or `forward`. `by` is one of:

| `by` | Moves to |
|---|---|
| `grapheme` | The next or previous grapheme cluster |
| `word` | The end of the next word (forward) or the start of the previous one (backward) |
| `line` | The same column one **document** line up or down, ignoring soft wrap |
| `visual_line` | The goal column one **visual** (wrapped) row up or down |
| `line_start` | The start of the caret's visual row (`dir` is ignored) |
| `line_end` | The end of the caret's visual row (`dir` is ignored) |
| `page` | A screenful of visual rows; the view scrolls by the same amount |
| `doc_start`, `doc_end` | The start or end of the document (`dir` is ignored) |

**Collapse rules.** A motion without `extend` on a non-empty selection collapses it instead
of moving from the caret. Backward goes to the selection's start and forward to its end. Up
and down start from that edge. `↑` on the first row goes to the document start and `↓` on
the last row to the end, as in a macOS text field.

### Clipboard and history

| Msg | JSON | Does |
|---|---|---|
| `Copy` | `{"msg":"copy"}` | Copies the selection to the register and returns `clipboard_set`. Several ranges join with the line ending |
| `Cut` | `{"msg":"cut"}` | Copy, then delete the selection. One undo step |
| `Paste { text }` | `{"msg":"paste","text":"x"}` or `{"msg":"paste"}` | Pastes `text`, or the internal register when `text` is absent. Outside text is converted to the document's line ending; the register is pasted as is |
| `Undo` | `{"msg":"undo"}` | Steps back one revision, restoring text and selection |
| `Redo` | `{"msg":"redo"}` | Steps forward one revision |

### Files, lifecycle and the runtime

| Msg | JSON | Does |
|---|---|---|
| `Save` | `{"msg":"save"}` | Returns `write_file` when the state has a `path`; otherwise sets a status message |
| `Saved` | `{"msg":"saved"}` | The runtime finished a save: marks that revision as saved |
| `SaveFailed { err }` | `{"msg":"save_failed","err":"disk full"}` | The runtime couldn't save: shows the error |
| `Quit` | `{"msg":"quit"}` | Returns `quit`. With unsaved changes, the first quit only warns and a second one quits |
| `Resize { width, height }` | `{"msg":"resize","width":80,"height":24}` | Sets the viewport (clamped to at least 1 × 1) |
| `Tick { now_ms }` | `{"msg":"tick","now_ms":1000}` | Reports the time. Undo grouping uses it |
| `ShowStatus { text }` | `{"msg":"show_status","text":"hi"}` | Shows a one-line message in the status bar (the first line of `text`) |

`tick`, `resize`, `saved`, `save_failed` (and `show_status`) are **passive**. They don't
clear the status message, don't end a typing run and don't disarm a pending quit.

## Effects

| Effect | JSON | The runtime should |
|---|---|---|
| `WriteFile { path, text }` | `{"effect":"write_file","path":"notes.md","text":"…"}` | Write the file, then send `saved` or `save_failed` |
| `ClipboardSet { text }` | `{"effect":"clipboard_set","text":"…"}` | Put the text on the system clipboard |
| `Quit` | `{"effect":"quit"}` | Exit |

## The keymap

`keymap(&Key) -> Option<Msg>` is pure. It follows macOS text-field keys, with Ctrl twins for
terminals that don't forward Cmd (`d-` in key scripts). Adding Shift to any motion extends
the selection.

| Key | Msg |
|---|---|
| `←` `→` | `move` by `grapheme` |
| `⌥←` `⌥→`, `Ctrl-←` `Ctrl-→`, `Alt-B` `Alt-F` | `move` by `word` |
| `↑` `↓` | `move` by `visual_line` |
| `⌘↑` `⌘↓`, `Ctrl-Home` `Ctrl-End`, `⌘Home` `⌘End` | `move` to `doc_start` / `doc_end` |
| `Home` `End`, `⌘←` `⌘→`, `Ctrl-A` `Ctrl-E` | `move` to `line_start` / `line_end` |
| `PgUp` `PgDn` | `move` by `page` |
| `Esc` | `collapse` |
| `⌘A`, `Alt-A` | `select_all` |
| `Backspace`, `Ctrl-H` | `delete_backward` |
| `Delete` | `delete_forward` |
| `⌥Backspace`, `Ctrl-Backspace`, `Ctrl-W` | `delete_word_backward` |
| `⌥Delete`, `Ctrl-Delete`, `Alt-D` | `delete_word_forward` |
| `⌘Backspace`, `Ctrl-U` | `delete_to_line_start` |
| `⌘Delete` | `delete_to_line_end` |
| `Ctrl-K` | `kill_line` |
| `⌘C` `⌘X` `⌘V`, `Ctrl-C` `Ctrl-X` `Ctrl-V` | `copy`, `cut`, `paste` (with no text) |
| `⌘Z`, `Ctrl-Z` | `undo` |
| `⇧⌘Z`, `Ctrl-Shift-Z`, `⌘Y`, `Ctrl-Y`, `⌘R`, `Ctrl-R` | `redo` |
| `⌘S`, `Ctrl-S` | `save` |
| `⌘Q`, `Ctrl-Q` | `quit` |
| `Enter` | `insert_newline` |
| `Tab` | `insert_text` with `"\t"` |
| Any other character | `insert_text` with that character |

Unbound: `Shift-Tab`, `Alt-↑`/`Alt-↓`, `Ctrl-↑`/`Ctrl-↓`, and Cmd or Ctrl with any letter
not listed. Moving by document `line` has no key; send the message.

## Key scripts

`--keys`, `script_to_msgs`, the tests and the protocol's `keys` op share one notation.
Literal characters type themselves. `<…>` is a special key or a modifier chord. Keys go
through the keymap, so a script tests the bindings too.

| Token | Key |
|---|---|
| `<cr>` `<enter>` `<ret>` `<return>` | Enter |
| `<bs>` `<backspace>` | Backspace |
| `<del>` `<delete>` | Delete |
| `<tab>` `<s-tab>` | Tab, Shift-Tab |
| `<esc>` | Escape |
| `<space>` | A space |
| `<left>` `<right>` `<up>` `<down>` | Arrows |
| `<home>` `<end>` `<pgup>` `<pageup>` `<pgdn>` `<pagedown>` | Home, End, Page Up, Page Down |
| `<lt>` `<gt>` | A literal `<` or `>` |
| `<wait:MS>` | Advances the clock by MS milliseconds (a `tick`) |

Modifier prefixes combine in any order:

| Prefix | Modifier |
|---|---|
| `s-` | Shift |
| `c-` | Ctrl |
| `a-` or `m-` | Alt / Option |
| `d-` | Cmd (Super) |

Examples: `<s-left>` (extend left), `<a-left>` (word left), `<c-s-z>` (redo), `<d-a>`
(select all), `<s-a-right>` (extend a word right). A newline or tab character in the script
is Enter or Tab. A `<` that doesn't start a token is a literal `<`. An unknown token such as
`<nope>` or an unknown modifier such as `<q-x>` is an error.

```console
$ caretline --state s.json --keys 'one<wait:2000> two<c-z>' --snapshot 40x6
```

Here `<wait:2000>` puts " two" in its own undo step, so `<c-z>` removes only " two".
