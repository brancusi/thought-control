# Structure: blocks, marks, folds and the layout

caretline can read a document as **blocks**: runs of lines with an identity that survives every
edit, a depth, and a prefix the caret never enters. Blocks are generic structure. caretline
knows where a block starts, how deep it is and which characters are its marker; it never knows
what a block *means*. A host that keeps data per block (a database row, a node id) keys it by
the block's mark, or lets the mark carry it as a payload.

The structure layer is on when `state.doc.outline` is set (an `OutlineConfig`). Its grammar,
which lines start blocks and what their markers are, is Markdown's: see [markdown.md](markdown.md).
Without it the document is plain text and nothing here applies.

```console
$ caretline --outline crates/caretline-app/fixtures/trip.md
```

## Marks: identity

A **mark** is a numeric id (`MarkId`) pinned to a line start. Marks follow their lines through
every edit, undo and redo, a cut and paste keeps them, and changes from elsewhere map them like
any edit (see [architecture.md](architecture.md#block-marks) for the exact rules).

Each mark carries **attributes** (`MarkAttrs`), beside the text and never in it:

| Field | Meaning |
|---|---|
| `gap` | A blank row before the block: `Some(true)` always, `Some(false)` never, `None` the grammar's default |
| `data` | **The host's payload**: any JSON value, or none. caretline never reads it. It travels with the mark through edits, cut and paste, undo and redo, changes from elsewhere and JSON |

Set a payload with a host command's `MarkOp::SetData` (one undo step), with
`ExtChange::SetData { id, data }` (a change from elsewhere, outside the history), or directly
on `doc.marks` (then call `outline_changed`).

Why a JSON value and not a type parameter: a parameter would reach `Mark`, `Document`, `State`,
`Msg`, traces, the session and the protocol, and a client in another language couldn't name it.
A JSON value keeps the state one serializable type and costs nothing when absent.

## The buffer

The text is still one rope, and **one text line is one row**:

```
text                               marks          blank row   block
───────────────────────────────    ───────────    ─────────   ─────────────────────────────
Booked the flat in Lisbon.         0 @ line 0                 0  paragraph, depth 0
It faces the river.                                              (continuation)
- Pay the deposit                  1 @ line 2     yes          1  bullet, depth 0
  - ask Ana about her desk         2 @ line 3                  2  bullet, depth 1
![boiler label](files/x.png)       3 @ line 4     yes          3  paragraph, atomic
```

- **A block's first line** is `indent marker content`: indentation (`OutlineConfig::indent`
  spaces per depth), then the grammar's marker, then the content.
- **Every other line continues the block above it** (a soft break).
- **Block starts** are the first line, every line with a mark, and every line the grammar says
  starts a block. A start without a mark gets one in the same update.
- **Identity** is the mark on a block's first line.
- **The blank row before a block** is its mark's `gap`, drawn as a virtual row: the caret never
  stops on it, `↑`/`↓` step over it, and a click on it lands at the block's content start.

Everything about a block is derived from the text, the marks and the config by
`outline::derive`, and remembered until they change (`state.blocks()`):

| `BlockInfo` field | Meaning |
|---|---|
| `id` | The block's mark |
| `start`, `end` | The first line's start, and the content's end (the last line's end, before its break) |
| `first_line`, `line_count` | Its lines |
| `depth`, `indent` | Leading spaces on the first line, as levels and as spaces |
| `kind` | `Para` or `Bullet` (a list item: a bullet, tagged or not, or a numbered item) |
| `tag` | A bullet's one-character tag, when the config has tags (see [markdown.md](markdown.md#tags)) |
| `prefix_len` | Characters of indentation and marker. Never a caret stop |
| `hang` | The marker's shape: `None`, `Bullet`, `Number(n)`, `Heading(n)`, `Quote`, `Fence` |
| `fence` | A code fence: its lines are all continuations |
| `atomic` | One line whose whole content is an image: one unit for the caret |
| `gap`, `attrs` | Whether a blank row comes before it, and its mark's attributes |

A **change of kind or depth never moves another block.** When Tab, Shift-Tab or a host command
with `keep_gaps` changes a block, every other block keeps the blank row it had. When typing,
Backspace or Delete changes the caret's block, that block and the one after it keep theirs.
Where the default would now differ, the old value is written to the mark, in the same undo step.

## The caret

**No selection end is ever inside a prefix**, and no caret is ever inside an atomic block. After
every message the update loop moves an end that landed there:

- Backward motion by character or word goes to the end of the line before.
- Everything else goes to the content start (`Home`, a click on a marker, `↑`/`↓`).
- A caret on an atomic block selects the whole block: arriving takes one press from any side, and
  so does leaving. A selection made with Shift takes the block whole.

## Block operations

These are the grammar-independent operations on blocks. Each is one transaction and one undo
step.

| Msg | Key | Does | Marks |
|---|---|---|---|
| `indent` / `outdent` | `Tab` / `Shift-Tab` | The caret's block, or every block the selection touches, one level deeper (at most one below the last non-empty block above) or shallower, keeping the selection. Nothing to nest under, or nothing to outdent: the status says so | Kept |
| `move_block { dir }` | `Alt-↑` / `Alt-↓` | Swaps the caret's block and its children with the previous or next sibling and its children. At the end of a list the status says so | Ids move with their blocks |
| `move { by: block }` | `Ctrl-↑` / `Ctrl-↓` | To the next block's content start, or back to this block's (then the previous one's); Shift extends | |
| `select_block { id }` | | Selects the block's content (a triple-click) | |
| `insert_blocks { after, blocks }` | | A host's blocks after a block (or at the start), as one step | Each `NewBlock`'s `mark` if free, else a new id |
| `fold`, `unfold`, `toggle_fold { id }` | | Hide or show a block's children in this view (see [Folds](#folds)) | |

A host adds its own operations as [host commands](embedding.md#host-commands): pure functions
from the document and view to an edit, run by `Msg::Command`.

## Effects

| Effect | When |
|---|---|
| `block_left { from, to }` | The primary caret moved to another block: a commit point for a host |
| `notice { text }` | A status message, when the status bar is off |
| `host { name, data }` | A host command's own effect |

## The outline layout

A view's `layout` (an `OutlineLayout`) lays the blocks out line by line, with the column
geometry as data:

| Field | Default | Meaning |
|---|---|---|
| `gutter` | 2 | Columns before everything, for a decoration's `gutter` text (read as `marks` in older states) |
| `indent` | 4 | Columns per depth |
| `hang` | 4 | Columns of the hang, before the content: where a block's marker glyph or decoration goes |
| `column` | 72 | The wrap width of depth-0 content |
| `min_column` | 20 | The narrowest a nested block's content wraps at |
| `extra_rows` | `{}` | Rows a host draws after a block, by mark id |
| `hang_glyphs` | false | Without a decorator, draw the marker's plain glyph in the hang (`•`, `1.`, `#`, `│`, and a tag as `[c]`) |

For each line:

- **A block's first line skips its prefix**: the marker moves out of the text into the hang. Its
  content starts at `x = gutter + depth·indent + hang` and wraps at
  `max(min_column, column − depth·indent)`, never past the view's right edge. In a view too
  narrow for that, deep blocks stop indenting where their content would get fewer than
  `min_column` columns.
- **Continuation lines** use the same column and width.
- **Content wraps as prose:** between words at whitespace only, a word moving to the next row
  whole unless it's longer than a row. A row never starts with a space.
- **A fence doesn't wrap.** A long line in it scrolls sideways on its own while the caret is on it.
- **A gapped block** has a blank row before it; **`extra_rows`** add rows after it. Neither is
  ever a caret stop.
- **A folded block's children** take no rows.

The goal column of `↑`/`↓` is a screen column, so moving between depths goes straight down.
Motion, paging, scrolling, clicks and drawing all use the same rows, through one `Layout`.

## Decorations

The engine draws text. What goes beside a block, in its hang and gutter, is a **decoration**: a
host's decorator (`Host::decorator`) is a pure function from a block to

```rust
pub struct Decoration { pub hang: Option<Deco>, pub gutter: Option<Deco> }
pub struct Deco { pub text: String, pub role: String, pub id: Option<String> }
```

caretline lays the slot out and draws `text` there. `role` is a style name the host defines:
the frame's cells carry it as `Role::Named(i)` (`Frame::role_name` gives the name, and the
protocol's `cells` spans carry it), and colours stay with the host. Hit-testing reports the
decoration's `id`:

| Item | Does |
|---|---|
| `Frame::rows` | One `RowInfo` per frame row: `Text { block, line, row, first, last, chars, x }`, `Gap { before }`, `Extra { block, index }`, `Past`, `Status` |
| `Cell::char_idx` | The document char a cell shows, for styling spans |
| `view::hit(doc, view, col, row)` | What a cell is: `Text { pos }` (where a click lands), `Hang { block, deco }`, `Gutter { block, deco }`, `Gap { block }`, `Extra { block, index }` or `Past` |
| `view::render(doc, view)` | The frame of any view of a document |

```rust
use caretline::outline::markdown;
use caretline::view::{hit, Hit};
use caretline::{view, Deco, Decoration, Host, OutlineConfig, OutlineLayout, Viewport};

let mut s = markdown::load("- Pay rent\n- Buy milk\n", None, Viewport { width: 40, height: 4 }, OutlineConfig::default());
s.view.layout = Some(OutlineLayout::default());
s.doc.set_host(Host::new().decorator(|_, b| Decoration {
    hang: Some(Deco { text: "◆".into(), role: "accent".into(), id: Some("diamond".into()) }),
    gutter: None,
}));
let f = view(&s);
assert_eq!(f.to_text().lines().next(), Some("  ◆   Pay rent"));
assert_eq!(hit(&s.doc, &s.view, 2, 0), Hit::Hang { block: s.doc.marks.as_slice()[0].id, deco: Some("diamond".into()) });
```

## Folds

`fold`, `unfold` and `toggle_fold` (by block id) hide or show a block's children in **one
view**: folds are the view's (`view.folds`), so another view of the same document still shows
them. They work with or without a layout. Only a block with children folds.

- Hidden lines take no rows. `↓` from the folded block goes to the next visible one, and `→` at
  its end goes past the children.
- A caret inside the children when they fold, or one an edit through another view leaves there,
  moves to the folded block's end.
- A fold drops when its block goes. Folds are part of the view's JSON (`"folds":[3]`).

## In Rust

| Item | Does |
|---|---|
| `State::enable_outline(config)` | Makes a state a block document (marks every block, outside the undo history) |
| `State::blocks() -> Option<Arc<Outline>>` | The derived blocks; `Outline::block_at`, `get`, `index_of`, `subtree_end` look them up |
| `State::outline_changed()` | Call after changing `text` or `marks` directly, not through `update` |
| `Document::blocks()` | The same, on a document shared by several views |
| `outline::content(doc, id)` | A block's content: its lines after the marker, joined with `\n` |
| `Marks::set_gap`, `set_data`, `set_attrs` | A mark's attributes |
| `NewBlock` | A block to insert: `depth`, `kind`, `tag`, `text`, `gap`, `mark` |

## Cost

Each edit re-derives the blocks once around the change and maps the marks after the first
change. The layout lays out only the rows it needs, so the outline layout costs no more than
plain drawing, and a second view of the document is only rebased, never laid out. The scale
tests type in a 5,000-block outline, with and without the layout and with a second view open,
and check that a key, rendered, stays under 4 ms in a release build (it is about 1 ms).
