# Markdown documents

caretline's block grammar is Markdown's: list markers, numbered items, headings, quotes and
fences start blocks, and Enter, Backspace and paste follow Markdown list editing. This page is
that grammar and its rules. The blocks themselves (identity, the caret, block operations, the
layout, decorations, folds) are on [structure.md](structure.md).

It lives in [`src/outline`](../../crates/caretline/src/outline) (`markdown.rs` and the rules in
`rules.rs`) and is on when `state.doc.outline` is set. Nothing in it gives a line a meaning
beyond its Markdown shape: a host that gives `[x]` a meaning does that itself, with a
[host command](embedding.md#host-commands).

```console
$ caretline --outline crates/caretline-app/fixtures/trip.md
```

## Markers

A block's first line is `indent marker content`:

| Marker | Block | `kind`, `hang` |
|---|---|---|
| `- `, `* `, `+ ` | A bullet | `Bullet`, `Bullet` |
| `- [c] ` with `c` in `OutlineConfig::tags` | A tagged bullet (see [Tags](#tags)) | `Bullet`, `Bullet`, `tag: Some(c)` |
| `12. ` or `12) ` (with `numbered`) | A numbered item | `Bullet`, `Number(12)` |
| `# `, `## `, `### ` | A heading paragraph | `Para`, `Heading(n)` |
| `> ` | A quote paragraph | `Para`, `Quote` |
| Three backticks | A code fence: every line to the closing fence is its content, and no marker inside starts a block | `Para`, `Fence` |
| none | A paragraph | `Para`, `None` |

Paragraphs nest like items: a nested paragraph's first line is its indentation and its content
(`  first point` is a paragraph at depth 1). A block that is exactly one image
(`![caption](path)`, with `atomic_images`) is atomic: one unit for the caret.

## Tags

A bullet may carry a one-character **tag** in brackets, `- [c] `, the bracket syntax of
GitHub-flavoured Markdown lists among others. Tags are off by default: `- [x] milk` is a bullet
whose text is `[x] milk`. With `OutlineConfig::tags` set, a tag whose character is in it is part
of the marker:

- it is never a caret stop, and the layout draws it in the hang (`[c]` with `hang_glyphs`);
- Backspace at the content's start removes the tag first, then the bullet;
- Enter after a tagged item starts the next one with `new_tag` (none: a plain bullet);
- Markdown paste reads `- [c] ` (and a bare `[c] ` line) as a tagged item, `[X]` as `[x]` when only
  `x` is a tag;
- changing a tag is a text edit (replace one character).

caretline gives a tag no meaning. A host that does (a status, a priority, a label) adds the
rest: commands that change tags, decorations that draw them, and its own reading of
`BlockInfo::tag`. [embedding.md](embedding.md#case-study-tasks-in-thc) builds tasks this way.

## Blank rows

Unset, a block's blank row follows its kind and the block before it: a paragraph has one before
and after it (a `##` or `###` heading only before), list items are tight, and the first block
has none. `gap: Some(true)` or `Some(false)` on its mark overrides that.

## The rules

Each rule is one transaction and one undo step. The marks column says what happens to ids.

| Msg | Where | Does | Marks |
|---|---|---|---|
| `insert_newline` (Enter) | In a list item | Splits it: the rest goes to a new item with the same marker and indentation (a number goes up by one, a tag becomes `new_tag`) | The new item gets a new id |
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
| `delete_backward` (Backspace) at a content start | A tagged bullet | Removes the tag: a plain bullet | Kept |
| | A bullet, numbered item, heading or quote | Removes the marker and indentation: a paragraph | Kept |
| | A paragraph after a paragraph | Joins them, keeping the line break | The lower id goes |
| | A paragraph after anything else | Joins it onto the block above's last line | The lower id goes |
| | After an atomic block | Selects that block (a second Backspace removes it) | — |
| `delete_forward` (Delete) at a block's end | | Joins the next block in (its marker goes); before an atomic block, selects it | The next id goes |
| `delete_word_*`, `delete_to_line_*`, `kill_line` | At a block's edge | As Backspace or Delete | |
| | Inside a block | As in plain text, but never past the content start or into the next line | |
| Backspace or Delete | On a selected atomic block | Removes the block; the status says what | Its id goes (undo brings it back, selected) |
| `insert_text` | On a selected atomic block | A new paragraph after it with the text | New id |
| `indent` | On a later line of a paragraph (a caret, no selection) | That line becomes a paragraph of its own, one level under the paragraph | New id for the line |
| `paste` | Markdown with line breaks | Read into blocks: the first joins the text before the caret (taking its shape when there is none), the rest follow, and the text after the caret ends the last. Images are left out and counted | New ids |
| | Whole blocks from the register, on an empty item | They take the item's place, with their own kinds and tags, re-indented to its depth | The cut ids come back |
| | Whole blocks from the register, at a block's end | They follow the block's subtree as siblings, at its depth | The cut ids come back |
| | Whole blocks from the register, inside a block's text | As pasted Markdown | New ids |
| | The register (or the same text from the system clipboard) | Pasted as it was cut | The cut ids come back |
| | Whole blocks from the register, over a selection of whole blocks | The selected blocks go and the register's take their place, in one step | The cut ids come back |
| `paste_plain` (`Alt-V`) | | Paragraphs with their line breaks kept; nothing becomes a list | New ids |
| `copy` / `cut` | Inside one block | Plain text | A cut keeps the removed ids in the register |
| | Across blocks | Markdown: the first block's text from the selection's start (its marker only from its content start), then each block with its marker and indentation, a blank line around paragraphs | |
| | Whole blocks | The register takes their lines with markers and indentation; a cut takes the lines out, leaving no empty item | A cut keeps the ids in the register |

Tab, Shift-Tab, moving blocks and folds are [block operations](structure.md#block-operations).

## Markdown in and out

`outline::markdown` reads and writes Markdown:

- `parse_markdown(text, plain, config)` reads pasted Markdown into `NewBlock`s (paragraph lines
  joined, list items and paragraphs nested by their first indent, tags the config has, headings,
  quotes and rules as one-line paragraphs, fences, tables and front matter as one paragraph with
  its line breaks).
- `to_markdown(state, outline, from, to)` writes a selection for the clipboard;
  `to_markdown_with(…, suffix)` adds a host's text after each block's first line.
- `load(md, path, viewport, config)` opens a Markdown file as a block document, and
  `to_file(state)` writes one back (`save` uses it). In a file, a blank line between blocks is a
  blank row, a continuation line is indented under its item, and a line break inside a
  paragraph is a soft break.

A file round-trips exactly when it is written the way `to_file` writes it. Three things don't
survive a file: an empty paragraph is left out, an empty continuation line reads back as a blank
row (splitting the block), and two paragraphs with no blank row between them read back as one.

```rust
use caretline::outline::markdown;
use caretline::{update, By, Dir, Msg, OutlineConfig, Viewport};

let mut s = markdown::load("- Pay rent\n", None, Viewport { width: 40, height: 6 }, OutlineConfig::default());
update(&mut s, Msg::Move { dir: Dir::Forward, by: By::LineEnd, extend: false });
update(&mut s, Msg::InsertNewline);
update(&mut s, Msg::InsertText { text: "Call Ana".into() });
update(&mut s, Msg::Indent);
assert_eq!(markdown::to_file(&s), "- Pay rent\n  - Call Ana\n");
```

## `OutlineConfig`

| Field | Default | Meaning |
|---|---|---|
| `indent` | 2 | Spaces per depth on a block's first line |
| `tags` | `""` | The characters a bullet's `[c]` tag may be; empty: brackets are text |
| `new_tag` | none | The tag Enter gives the item after a tagged one |
| `atomic_images` | true | A block that is exactly one image is one caret unit |
| `numbered` | true | `12. ` and `12) ` start numbered items |

## Keys

An outline document's keymap is the plain one with these on top (see [keys.md](keys.md)):

| Key | Command | Msg |
|---|---|---|
| `Tab` / `Shift-Tab` | `structure.indent` / `structure.outdent` | `indent` / `outdent` |
| `Shift-Enter`, `Ctrl-J` | `edit.soft_break` | `soft_break` |
| `Alt-↑` / `Alt-↓` | `structure.move_up` / `structure.move_down` | `move_block` |
| `Ctrl-↑` / `Ctrl-↓` | `move.block_up` / `move.block_down` (Shift: `select.*`) | `move` by `block` |
| `Alt-V` | `clip.paste_plain` | `paste_plain` |

## Try it

```console
$ caretline --outline crates/caretline-app/fixtures/trip.md --keys '<down><down><down><down><end><cr><tab>Call Ana' --snapshot 50x18
$ caretline --state crates/caretline-app/fixtures/outline-edited.state.json --snapshot 50x18
$ caretline --outline crates/caretline-app/fixtures/trip.md --keys '<d-down>!<c-s>' --effects
```

`--layout` adds the outline layout, with plain hang glyphs:

```console
$ caretline --layout crates/caretline-app/fixtures/trip.md --snapshot 50x16 --no-status-bar
  #   Lisbon trip

      Booked the flat in Lisbon.
      It faces the river.

  •   Pay the deposit
      •   ask Ana about her desk
  •   Book flights
  •   Renew passport

      ![boiler label](files/boiler.png)

  1.  Pack
  2.  Leave
```
