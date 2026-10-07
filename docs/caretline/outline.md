# Outline documents

An outline document is a text buffer read as **blocks**: paragraphs, bullets, numbered items
and tasks, nested by indentation. It has its own editing rules (Enter continues a list, Tab
nests, `Ctrl-T` cycles a task, `Alt-↑`/`Alt-↓` move an item with its children) and gives
every block a stable identity, so a host can keep its own data per block (a database row, a
due date) while the text is edited, undone and redone.

The outline layer lives in [`src/outline`](../../crates/caretline-next/src/outline) and is
on when `state.doc.outline` is set. Without it the document is plain text and nothing here
applies.

```console
$ caretline --outline crates/caretline-app/fixtures/trip.md
```

## The buffer

The text is still one rope, and **one text line is one row**:

```
text                               marks          blank row   block
───────────────────────────────    ───────────    ─────────   ───────────────────────────
Booked the flat in Lisbon.         0 @ line 0                 0  paragraph, depth 0
It faces the river.                                              (continuation)
- [ ] Pay the deposit              1 @ line 2     yes          1  task ' ', depth 0
  - ask Ana about her desk         2 @ line 3                  2  bullet, depth 1
![boiler label](files/x.png)       3 @ line 4     yes          3  paragraph, atomic
```

- **A block's first line** is `indent marker content`. `indent` is `OutlineConfig::indent`
  spaces (2) per depth. The marker is `- ` (also `* `, `+ `) for a bullet, `- [c] ` for a
  task (`c` from the task vocabulary), `12. ` or `12) ` for a numbered item, or nothing for a
  paragraph. A paragraph's first line may start with a heading (`# `, `## `, `### `), a
  quote (`> `) or a code fence (three backticks).
- **Every other line continues the block above it** (a soft break). A continuation line
  holds plain content, without indentation.
- **Block starts** are the first line, every line with a [mark](architecture.md#block-marks),
  and every line outside a fence that starts with a marker (after optional indentation). A
  start without a mark gets one in the same update, so typing `- ` at the start of a
  continuation line makes that line a new item. From an opening fence line to its closing
  one, no marker starts a block.
- **Identity** is the mark on a block's first line (`MarkId`). Marks follow their lines
  through every edit, undo and redo, and a cut and paste keeps them.
- **The blank row before a block** is an attribute of its mark (`BlockAttrs::gap`), never a
  text line. The view draws it as a virtual row: the caret never stops on it, `↑`/`↓` step
  over it, and a click on it lands at the block's content start.

Everything about a block is derived from the text, the marks and the config, by
`outline::derive`, and remembered until they change (`state.blocks()`):

| `BlockInfo` field | Meaning |
|---|---|
| `id` | The block's mark |
| `start`, `end` | The first line's start, and the content's end (the last line's end, before its break) |
| `first_line`, `line_count` | Its lines |
| `depth`, `indent` | Leading spaces on the first line, as levels and as spaces |
| `kind` | `Para`, `Bullet` or `Task` |
| `status` | A task's box character |
| `prefix_len` | Characters of indentation and marker. Never a caret stop |
| `hang` | What a host draws before the content: `None`, `Bullet`, `Number(n)`, `Task(c)`, `Heading(n)`, `Quote`, `Fence` |
| `fence` | A code fence: its lines are all continuations |
| `atomic` | One line whose whole content is an image, `![caption](path)` |
| `gap`, `attrs` | Whether a blank row comes before it, and its attributes as set |

### Blank rows

Unset, a block's blank row follows its kind and the block before it: a paragraph has one
before and after it (a `##` or `###` heading only before), list items are tight, and the first
block has none. `gap: Some(true)` or `Some(false)` overrides that.

A **kind or depth change never moves another block**. When the task cycle, Tab or Shift-Tab
changes a block, every other block keeps the blank row it had. When typing, Backspace or
Delete changes the kind or depth of the caret's block, that block and the one after it keep
theirs. Where the default would now differ, the old value is written to the block's
attributes, in the same undo step. Other edits (Enter, joins, moves, paste) let blocks take
their defaults.

## The caret

**No selection end is ever inside a marker**, and no caret is ever inside an atomic block.
After every message the update loop moves an end that landed there:

- Backward motion by character or word goes to the end of the line before (so `←` from a
  content start goes to the previous block's end in one press).
- Everything else goes to the content start (`Home`, a click on a marker, `↑`/`↓`).
- A caret on an atomic block (an image) selects the whole block: arriving takes one press from
  any side, and so does leaving. A selection made with Shift takes the block whole.

## The rules

Each rule is one Transaction and one undo step. The mark column says what happens to ids.

| Msg | Where | Does | Marks |
|---|---|---|---|
| `insert_newline` (Enter) | In a list item | Splits it: the rest goes to a new item with the same marker and indentation (a task starts open, a number goes up by one) | The new item gets a new id |
| | At an item's content start | A new empty item above | The item keeps its id |
| | On an empty item | It becomes a paragraph (the list ends) | Kept |
| | In a paragraph | A soft break | — |
| | At a paragraph's very start | A new empty paragraph above | The paragraph keeps its id |
| | At the start of a paragraph's later line | That line starts a new paragraph (an empty line there is dropped when more follows). So Enter twice at a paragraph's end starts a new paragraph | New id for the new paragraph |
| | At the end of a paragraph's line, with more lines below | The rest becomes a new paragraph, with the caret on a new empty one between | Two new ids |
| | In a heading or quote | A new paragraph after it (at its content start: a new empty paragraph above) | New id |
| | In a fence | A line break | — |
| | On a selected atomic block | A new empty paragraph after it | New id |
| `soft_break` (`Shift-Enter`, `Ctrl-J`) | In an item, heading or quote | A line break inside the block | — |
| | Elsewhere | As Enter | |
| `delete_backward` (Backspace) at a content start | A task | Removes `[c] `: it becomes a bullet | Kept |
| | A bullet, numbered item, heading or quote | Removes the marker and indentation: it becomes a paragraph | Kept |
| | A paragraph after a paragraph | Joins them, keeping the line break | The lower id goes |
| | A paragraph after anything else | Joins it onto the block above's last line | The lower id goes |
| | After an atomic block | Selects that block (a second Backspace removes it) | — |
| `delete_forward` (Delete) at a block's end | | Joins the next block in (its marker goes); before an atomic block, selects it | The next id goes |
| `delete_word_*`, `delete_to_line_*`, `kill_line` | At a block's edge | As Backspace or Delete | |
| | Inside a block | As in plain text, but never past the content start or into the next line | |
| Backspace or Delete | On a selected atomic block | Removes the block; the status says what | Its id goes (undo brings it back, selected) |
| `insert_text` | On a selected atomic block | A new paragraph after it with the text | New id |
| `indent` / `outdent` (Tab / Shift-Tab) | The caret's block, or every block the selection touches | One level deeper (at most one below the last non-empty block above) or shallower, keeping the selection. Paragraphs don't nest: the status says so | Kept |
| `task_cycle` (`Ctrl-T`) | The caret's block, or every block the selection touches | The first block's next state applies to all: text → `[cycle[0]]` → `[cycle[1]]` → text. A bullet's marker becomes a task's | Kept |
| | Inside a multi-line paragraph | Each selected line becomes its own task; the lines before and after stay paragraphs, tight against them | The first piece keeps the id; the others get new ones with no blank row |
| | A task back to text, next to a paragraph with no blank row between | Joins it (the reverse of the split) | The joined ids go |
| `set_status { id, ch }` | A task's box | Sets the box (a click). Never back to text | Kept |
| `move_block { dir }` (`Alt-↑` / `Alt-↓`) | The caret's block with its children | Swaps with the previous or next sibling and its children. At the end of a list the status says `first in its list` or `last in its list` | Ids move with their blocks |
| `move { by: block }` (`Ctrl-↑` / `Ctrl-↓`) | | To the next block's content start, or back to this block's (then the previous one's) | |
| `select_block { id }` | | Selects the block's content (a triple-click) | |
| `select_word_at { pos }` | | Selects the word at `pos` (a double-click); a `click` with `extend` then extends by words | |
| `insert_blocks { after, blocks }` | | Host blocks after a block (or at the start), as one step | Each block's `mark` if free, else a new id |
| `paste` | Markdown with line breaks | Read into blocks: the first joins the text before the caret (taking its shape when there is none), the rest follow, and the text after the caret ends the last. Images are left out and counted | New ids |
| | Whole blocks from the register, on an empty item | The blocks take the item's place, with their own kinds and statuses, re-indented to its depth | The cut ids come back |
| | Whole blocks from the register, at a block's end | They follow the block's subtree as siblings, at its depth | The cut ids come back |
| | Whole blocks from the register, inside a block's text | As pasted Markdown | New ids |
| | The register (or the same text from the system clipboard) | Pasted as it was cut | The cut ids come back |
| `paste_plain` (`Alt-V`) | | Paragraphs with their line breaks kept; nothing becomes a list | New ids |
| `copy` / `cut` | Inside one block | Plain text | A cut keeps the removed ids in the register |
| | Across blocks | Markdown: the first block's text from the selection's start (its marker only from its content start), then each block with its marker and indentation, a blank line around paragraphs | |
| | Whole blocks (from a block's content start to another block's end, or to the start of the block after them, as Shift-↓ selects) | The register takes their lines with markers and indentation; a cut takes the lines out, leaving no empty item | A cut keeps the ids in the register |

### Effects

| Effect | When |
|---|---|
| `completed { id }` | A task reached done (`task_cycle` or `set_status`): a host may save at once |
| `restored` | Undo or redo changed the document: a host re-reads what it keeps per block |
| `block_left { from, to }` | The primary caret moved to another block: a commit point for a host |
| `notice { text }` | A status message, when the status bar is off |

## Markdown in and out

`outline::markdown` reads and writes Markdown:

- `parse_markdown(text, plain)` reads pasted Markdown into `NewBlock`s (paragraph lines
  joined, list items nested by their first indent, `- [ ]` and bare `[ ]` tasks, headings,
  quotes and rules as one-line paragraphs, fences, tables and front matter as one paragraph
  with its line breaks).
- `to_markdown(state, outline, from, to)` writes a selection for the clipboard.
- `load(md, path, viewport, config)` opens a Markdown file as an outline document, and
  `to_file(state)` writes one back (`save` uses it). In a file, a blank line between blocks
  is a blank row, a continuation line is indented under its item, and a line break inside a
  paragraph is a soft break.

A file round-trips exactly when it is written the way `to_file` writes it. Three things
don't survive a file: an empty paragraph (a fresh line to type on) is left out, an empty
continuation line reads back as a blank row (splitting the block), and two paragraphs with no
blank row between them read back as one.

## In Rust

```rust
use caretline_next::outline::markdown;
use caretline_next::{update, Effect, MarkId, Msg, OutlineConfig, Viewport};

let mut s = markdown::load("- [ ] Pay rent\n- Buy milk\n", None, Viewport { width: 40, height: 6 }, OutlineConfig::default());
let fx = update(&mut s, Msg::TaskCycle);                  // the caret is on "Pay rent"
assert!(fx.contains(&Effect::Completed { id: MarkId(0) }));
assert_eq!(markdown::to_file(&s), "- [x] Pay rent\n- Buy milk\n");

let blocks = s.blocks().unwrap();                          // derived, cached
assert_eq!(blocks.blocks[1].depth, 0);
```

| Item | Does |
|---|---|
| `State::enable_outline(config)` | Makes a state an outline document (marks every block, outside the undo history) |
| `State::blocks() -> Option<Arc<Outline>>` | The derived blocks; `Outline::block_at`, `get`, `index_of`, `subtree_end` look them up |
| `State::outline_changed()` | Call after changing `text` or `marks` directly, not through `update` |
| `Document::blocks()` | The same, on a document shared by several views |
| `outline::content(doc, id)` | A block's content: its lines after the marker, joined with `\n` |
| `OutlineConfig` | `indent`, `task_markers` (the vocabulary), `cycle`, `atomic_images`, `numbered` |
| `NewBlock` | A block to insert: `depth`, `kind`, `status`, `text`, `gap`, `mark` |
| `outline_keymap`, `keymap_for(outline, key)`, `script_to_msgs_for` | The outline's keys |

## Keys

The outline keymap is the plain one (see [messages.md](messages.md#the-keymap)) with these on
top:

| Key | Msg |
|---|---|
| `Tab` / `Shift-Tab` | `indent` / `outdent` |
| `Ctrl-T` | `task_cycle` |
| `Shift-Enter`, `Ctrl-J` | `soft_break` |
| `Alt-↑` / `Alt-↓` | `move_block` |
| `Ctrl-↑` / `Ctrl-↓` | `move` by `block` (Shift extends) |
| `Alt-V` | `paste_plain` |

## Try it

`caretline --outline FILE` edits a Markdown file as an outline. The fixtures include
[`trip.md`](../../crates/caretline-app/fixtures/trip.md) and three saved outline states:

```console
$ caretline --outline crates/caretline-app/fixtures/trip.md --keys '<down><down><down><c-t>' --snapshot 50x18
$ caretline --outline crates/caretline-app/fixtures/trip.md --keys '<down><down><down><down><end><cr>Call Ana<tab>' --snapshot 50x18
$ caretline --state crates/caretline-app/fixtures/outline-split.state.json --snapshot 40x10
$ caretline --outline crates/caretline-app/fixtures/trip.md --keys '<d-down>!<c-s>' --effects
```

Without a layout the view draws each block's text as it is, markers included, with blank rows
as virtual rows. `--layout` adds the outline layout (below), with plain hang glyphs:

```console
$ caretline --layout crates/caretline-app/fixtures/trip.md --snapshot 50x16 --no-status-bar
  #   Lisbon trip

      Booked the flat in Lisbon.
      It faces the river.

  [ ] Pay the deposit
      •   ask Ana about her desk
  [ ] Book flights
  [x] Renew passport

      ![boiler label](files/boiler.png)

  1.  Pack
  2.  Leave
```

## The outline layout

A view's `layout` (an `OutlineLayout`) lays an outline document out line by line, with the
column geometry as data:

| Field | Default | Meaning |
|---|---|---|
| `marks` | 2 | Columns before everything, for marks a host draws (a conflict sign, a flash) |
| `indent` | 4 | Columns per depth |
| `hang` | 4 | Columns of the hang: the bullet, number, task box or heading sign |
| `column` | 72 | The wrap width of depth-0 content |
| `min_column` | 20 | The narrowest a nested block's content wraps at |
| `extra_rows` | `{}` | Rows a host draws after a block, by mark id (its fields on their own row, an inline image) |
| `hang_glyphs` | false | Draw a plain glyph in each hang (`•`, `1.`, `[ ]`, `#`, `│`), for a host that draws none |

For each line:

- **A block's first line skips its prefix** (indentation and marker): the marker moves out of
  the text into the hang. Its content starts at `x = marks + depth·indent + hang` and wraps at
  `max(min_column, column − depth·indent)`, never past the view's right edge. In a view too
  narrow for that, deep blocks stop indenting where their content would get fewer than
  `min_column` columns.
- **Continuation lines** use the same column and width.
- **A fence doesn't wrap.** A long line in it scrolls sideways on its own while the caret is on
  it; the other lines stay put.
- **A gapped block** has a blank row before it; **`extra_rows`** add rows after it. Neither is
  ever a caret stop: `↑`/`↓` step over them keeping the goal column, and a click on them lands at
  the block's text.
- **A folded block's children** take no rows (see [Folds](#folds)).

The goal column of `↑`/`↓` is a screen column, so moving between depths goes straight down.
Motion, paging, scrolling, clicks and drawing use the same rows, through one `Layout`.

The engine draws only text: the marks column and the indentation are blank, and the hang's
cells have the role `Hang` (blank unless `hang_glyphs`). A host draws its own hang, marks and
fields from the frame's row info:

| Item | Does |
|---|---|
| `Frame::rows` | One `RowInfo` per frame row: `Text { block, line, row, first, last, chars, x }`, `Gap { before }`, `Extra { block, index }`, `Past`, `Status` |
| `Cell::char_idx` | The document char a cell shows, for styling spans (links, tags) |
| `view::hit(doc, view, col, row)` | What a cell is: `Text { pos }` (where a click lands), `Hang { block }`, `Marks { block }`, `Gap { block }`, `Extra { block, index }` or `Past` |
| `view::render(doc, view)` | The frame of any view of a document |

```rust
use caretline_next::outline::markdown;
use caretline_next::view::{hit, Hit, RowInfo};
use caretline_next::{view, OutlineConfig, OutlineLayout, Viewport};

let mut s = markdown::load("- [ ] Pay rent\n- Buy milk\n", None, Viewport { width: 40, height: 4 }, OutlineConfig::default());
s.view.layout = Some(OutlineLayout::default());
let f = view(&s);
assert!(matches!(f.rows[0], RowInfo::Text { first: true, x: 6, .. }));
assert_eq!(hit(&s.doc, &s.view, 3, 0), Hit::Hang { block: s.doc.marks.as_slice()[0].id });
```

## Folds

`fold`, `unfold` and `toggle_fold` (by block id) hide or show a block's children in **one
view**: folds are the view's (`view.folds`), so another view of the same document still shows
them. They work with or without a layout. Only a block with children folds.

- Hidden lines take no rows. `↓` from the folded block goes to the next visible one, and `→`
  at its end goes past the children.
- A caret inside the children when they fold, or one that an edit through another view leaves
  there, moves to the folded block's end.
- A fold drops when its block goes. Folds are part of the view's JSON (`"folds":[3]`).

## Cost

Each edit re-derives the blocks once (linear in lines; a few hundred microseconds for 5,000
blocks in a release build) and maps the marks after the first change. The layout lays out only
the rows it needs, so the outline layout costs no more than plain drawing, and a second view
of the document is only rebased (its selection mapped), never laid out. The scale tests type in
a 5,000-block outline, with and without the layout and with a second view open, and check that
a key, rendered, stays under 4 ms in a release build (it is about 1 ms). Deriving only the lines
an edit touched is the next step when that matters.
