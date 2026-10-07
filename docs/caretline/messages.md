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
| `block` | In a [block document](structure.md): the next block's content start, or back to this block's (then the previous one's). Elsewhere, as `line` |

**Collapse rules.** A motion without `extend` on a non-empty selection collapses it instead
of moving from the caret. Backward goes to the selection's start and forward to its end. Up
and down start from that edge. `↑` on the first row goes to the document start and `↓` on
the last row to the end, as in a macOS text field.

### Clipboard and history

| Msg | JSON | Does |
|---|---|---|
| `Copy` | `{"msg":"copy"}` | Copies the selection to the register and returns `clipboard_set`. Several ranges join with the line ending |
| `Cut` | `{"msg":"cut"}` | Copy, then delete the selection. One undo step. With one range, the register keeps the [marks](architecture.md#block-marks) the cut removed |
| `Paste { text }` | `{"msg":"paste","text":"x"}` or `{"msg":"paste"}` | Pastes `text`, or the internal register when `text` is absent. Outside text is converted to the document's line ending; the register is pasted as is. Pasting the register, or text equal to what it put on the system clipboard, brings its marks back where their ids aren't in use |
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
| `Frame { now_ms }` | `{"msg":"frame","now_ms":1008}` | A display frame from the runtime's frame clock. Advances the clock as `tick` does; animation state advances from it |
| `FrameClock { fps }` | `{"msg":"frame_clock","fps":120}` | Asks the runtime for a frame clock: a `frame` message `fps` times a second (`0` turns it off). Stored in the view as `frame_clock` |

### Block documents

These act on documents with [block structure](structure.md) (`state.doc.outline` set).
Elsewhere they only set the status message `only in outline documents`, except `soft_break`
(a line break), `select_word_at` and `paste_plain` (a paste). In a block document, Enter,
Backspace, Delete, the word and line deletes, typing, copy, cut and paste also follow the
[Markdown rules](markdown.md#the-rules).

| Msg | JSON | Does |
|---|---|---|
| `SoftBreak` | `{"msg":"soft_break"}` | A line break inside the block (in a paragraph, as Enter) |
| `Indent`, `Outdent` | `{"msg":"indent"}` | Nests the caret's block, or every block the selection touches, one level deeper or shallower |
| `MoveBlock { dir }` | `{"msg":"move_block","dir":"backward"}` | Swaps the caret's block and its children with the previous or next sibling |
| `SelectBlock { id }` | `{"msg":"select_block","id":3}` | Selects a block's content |
| `SelectWordAt { pos }` | `{"msg":"select_word_at","pos":12}` | Selects the word at a char position; a `click` with `extend` right after extends by words |
| `Edit { changes, join }` | `{"msg":"edit","changes":[[0,5,"Hello"]],"join":false}` | A host's own edit, one undo step: each `[from, to, text]` replaces chars `[from, to)` of the current text (in order, apart). `join`: folded into the last undo step |
| `InsertBlocks { after, blocks }` | `{"msg":"insert_blocks","after":3,"blocks":[{"kind":"bullet","text":"Call Ana"}]}` | Inserts host blocks after a block, or at the start without `after`. One undo step |
| `PastePlain { text }` | `{"msg":"paste_plain","text":"a\nb"}` | Pastes as paragraphs with their line breaks kept |

A block is named by its mark id, a number. A `NewBlock` is `{"depth":0,"kind":"para"|"bullet","tag":"a","text":"…","gap":true,"mark":7}`;
everything but `kind` and `text` is optional (`tag`: a bullet's [tag](markdown.md#tags)).

### Host commands

| Msg | JSON | Does |
|---|---|---|
| `Command { name, args }` | `{"msg":"command","name":"shout","args":{"times":2}}` | Runs the host's command `name` (registered with `Host::command`) as one transaction and one undo step. An unknown name changes nothing and says so. See [embedding.md](embedding.md#host-commands) |

### Views and folds

| Msg | JSON | Does |
|---|---|---|
| `ScrollView { rows }` | `{"msg":"scroll_view","rows":5}` | Scrolls the view without moving the caret (`scroll` moves it when it would leave the view). The view stays put until the next caret motion or edit |
| `Fold { id }` | `{"msg":"fold","id":3}` | Hides a block's children in this view (outline documents). A caret inside them moves to the block's end. A block without children doesn't fold |
| `Unfold { id }`, `ToggleFold { id }` | `{"msg":"toggle_fold","id":3}` | Shows them again, or toggles |

Folds belong to a view: another view of the same document still shows the children. Hidden
lines take no rows, so `↓` steps over them and `→` at the block's end goes past them. A fold
drops when its block goes.

### Changes from elsewhere

`External { changes }` applies changes made outside the editor (another device, a daemon, an
agent) to the document, in order, outside the undo history: every view is mapped through
them, and undo takes back only local edits around them. It is passive, and a read-only view
takes it too.

```json
{"msg":"external","changes":[
  {"change":"replace_content","id":3,"text":"Call Ana\nabout the desk"},
  {"change":"set_shape","id":4,"depth":1,"kind":"bullet","tag":"b"},
  {"change":"insert_block","after":4,"block":{"kind":"bullet","text":"new","mark":12}},
  {"change":"remove_block","id":5},
  {"change":"set_gap","id":6,"gap":true},
  {"change":"set_data","id":6,"data":{"row":42}},
  {"change":"replace","from":0,"to":5,"text":"Hello"}]}
```

| Change | Does |
|---|---|
| `replace_content { id, text }` | A block's content after its marker (`\n` for soft breaks). Only the chars that differ change |
| `set_shape { id, depth, kind, tag }` | Rewrites a block's indentation and list marker. A number, heading or quote marker stays, as content |
| `set_gap { id, gap }` | A block's blank row (`null`: the default) |
| `set_data { id, data }` | A mark's payload (`null`: none). Any document with marks; the text is untouched |
| `insert_block { after, block }` | A [`NewBlock`](#block-documents) after a block's own lines, or first without `after`. `block.mark` gives it that id when free |
| `remove_block { id }` | A block's lines, its continuations included, not its children |
| `replace { from, to, text }` | Chars `[from, to)` of the current text (any document) |

A change that names a missing block is skipped with a `notice` effect. See
[architecture.md](architecture.md#changes-from-elsewhere) for the history transform.

`tick`, `frame`, `frame_clock`, `resize`, `saved`, `save_failed`, `show_status` and `external` are **passive**. They
don't clear the status message, don't end a typing run and don't disarm a pending quit.

## Effects

| Effect | JSON | The runtime should |
|---|---|---|
| `WriteFile { path, text }` | `{"effect":"write_file","path":"draft.md","text":"…"}` | Write the file, then send `saved` or `save_failed` |
| `ClipboardSet { text }` | `{"effect":"clipboard_set","text":"…"}` | Put the text on the system clipboard |
| `Quit` | `{"effect":"quit"}` | Exit |
| `Notice { text }` | `{"effect":"notice","text":"nothing to nest under"}` | Show a message: an outline document's status message when the status bar is off |
| `Host { name, data }` | `{"effect":"host","name":"shouted","data":{"line":3}}` | Whatever the host's own command meant by it (its runtime handles it) |
| `BlockLeft { from, to }` | `{"effect":"block_left","from":2,"to":3}` | Nothing required. The caret moved to another block of an outline |
| `Refused` | `{"effect":"refused"}` | Nothing required. An editing message reached a read-only view and changed nothing |

`Effect` is `#[non_exhaustive]`: new kinds may come, so a `match` needs a wildcard arm (a
runtime can ignore kinds it doesn't know).

## The keymap

`keymap(&Key) -> Option<Msg>` is pure: a lookup in caretline's default keymap, which is data
(`default_keymap`), binding key chords to the [command catalog](keys.md)'s ids. It follows
macOS text-field keys, with Ctrl twins for terminals that don't forward ⌘ (`d-` in key scripts).
Adding Shift to a motion extends the selection. Printable keys without Ctrl, Alt or ⌘ type
themselves.

| Key | Command | Msg |
|---|---|---|
| `←` `→` | `move.left`, `move.right` | `move` by `grapheme` |
| `⌥←` `⌥→`, `Ctrl-←` `Ctrl-→`, `Alt-B` `Alt-F` | `move.word_left`, `move.word_right` | `move` by `word` |
| `↑` `↓` | `move.up`, `move.down` | `move` by `visual_line` |
| `⌘↑` `⌘↓`, `Ctrl-Home` `Ctrl-End`, `⌘Home` `⌘End` | `move.doc_start`, `move.doc_end` | `move` to `doc_start` / `doc_end` |
| `Home` `End`, `⌘←` `⌘→`, `Ctrl-A` `Ctrl-E` | `move.line_start`, `move.line_end` | `move` to `line_start` / `line_end` |
| `PgUp` `PgDn` | `move.page_up`, `move.page_down` | `move` by `page` |
| `Esc` | `select.collapse` | `collapse` |
| `⌘A`, `Alt-A` | `select.all` | `select_all` |
| `Backspace`, `Ctrl-H` | `edit.backspace` | `delete_backward` |
| `Delete` | `edit.delete_forward` | `delete_forward` |
| `⌥Backspace`, `Ctrl-Backspace`, `Ctrl-W` | `edit.delete_word` | `delete_word_backward` |
| `⌥Delete`, `Ctrl-Delete`, `Alt-D` | `edit.delete_word_forward` | `delete_word_forward` |
| `⌘Backspace`, `Ctrl-U` | `edit.delete_to_line_start` | `delete_to_line_start` |
| `⌘Delete` | `edit.delete_to_line_end` | `delete_to_line_end` |
| `Ctrl-K` | `edit.kill_line` | `kill_line` |
| `⌘C` `⌘X` `⌘V`, `Ctrl-C` `Ctrl-X` `Ctrl-V` | `clip.copy`, `clip.cut`, `clip.paste` | `copy`, `cut`, `paste` (with no text) |
| `⌘Z`, `Ctrl-Z` | `history.undo` | `undo` |
| `⇧⌘Z`, `Ctrl-Shift-Z`, `⌘Y`, `Ctrl-Y`, `⌘R`, `Ctrl-R` | `history.redo` | `redo` |
| `⌘S`, `Ctrl-S` | `file.save` | `save` |
| `⌘Q`, `Ctrl-Q` | `file.quit` | `quit` |
| `Enter` | `edit.newline` | `insert_newline` |
| `Tab` | `edit.insert_tab` | `insert_text` with `"\t"` |

Unbound: `Shift-Tab`, `Alt-↑`/`Alt-↓`, `Ctrl-↑`/`Ctrl-↓`, and chords the table doesn't list.
Moving by document `line` has no key; send the message.

A block document uses `outline_keymap`, which adds `Tab`/`Shift-Tab` (`structure.indent` /
`structure.outdent`), `Shift-Enter` and `Ctrl-J` (`edit.soft_break`), `Alt-↑`/`Alt-↓`
(`structure.move_up` / `structure.move_down`), `Ctrl-↑`/`Ctrl-↓` (`move.block_up` /
`move.block_down`) and `Alt-V` (`clip.paste_plain`). `keymap_for(outline, key)` picks the right
one; key scripts, `Session::keys` and the protocol's `keys` op use the state's. See
[keys.md](keys.md) for the catalog, `caretline keys`, and building an app's own table.

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
