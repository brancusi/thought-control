# caretline: the specification

The rules caretline keeps, written for any app that embeds it (the **host**). Every rule here
has examples in the tests, written in caret notation (§10). When this file and a golden
disagree, that's a bug in one of them.

**Words used throughout:**
- **Block:** a paragraph, list item or task.
- **Document:** a list of blocks, held in a `Doc`.
- **View:** one place a document is shown.
- **Position:** a block and a byte offset in its text, always on a grapheme boundary.
- **Host:** your app.

## 1. The model

| Text | Block |
|---|---|
| Lines with no blank line between them | **One paragraph.** A single newline inside it is a **soft break** (kept as typed) |
| A blank line | A boundary between blocks. **It's never a block itself** |
| `- `, `* ` or `+ ` at a line's start | A list item |
| `[ ] `, `[x] `, `- [ ] `, `- [x] ` | A task (open or done). The host may register more states, such as `[/]` |
| `1. `, `## `, `> ` | A paragraph whose text starts with that marker. The host draws the marker in the hang |
| Indentation (2 spaces or a tab per level) | Depth: a child of the block above. Paragraphs never nest |

- **The marker is the truth.** There's no hidden state. Delete the `[ ] ` and the block is plain
  text.
- **Text is text.** Markers become structure only as you type or paste them at a line's start.
  Nothing already written is ever re-read as a marker.
- **One deviation from CommonMark:** a single newline inside a paragraph is a hard break, not a
  space. The Markdown writer emits it as a trailing `\` so the round trip is lossless.
- **Gaps:** each block records whether a blank line comes before it (`Block::gap`: `None`
  follows its kind's default, `Some(b)` is pinned). A paragraph after a paragraph always has
  one, because otherwise they would be one block.

## 2. Identity

Every block has a host-supplied ID (`BlockLine::Id`). IDs never appear in the text. They follow
the text through edits:

| Edit | ID |
|---|---|
| Typing inside a block | Kept |
| A split (Enter on an empty line, a typed marker, `TaskCycle` on one line of a paragraph) | The piece holding the original first line keeps it. Others are new (`BlockLine::fresh`) |
| A join (`Backspace` at a block's start, a delete across blocks) | The upper block keeps it. The lower one is removed (`Doc::take_deleted` reports it) |
| Un-tasking a line next to a paragraph with no gap | It joins that paragraph with a soft break (the reverse of the split). The upper ID is kept |
| Cut, then paste in the same session | The pasted blocks keep their IDs when the originals are gone (a move). A copy gets new IDs |

## 3. Editing

- **`Newline`:**
  - In a paragraph, a soft break.
  - On an empty line, a new block.
  - In a list item or task, the next item with the same marker and depth.
  - On an empty item, the end of the list (it becomes an empty paragraph line).
- **`SoftBreak`:** a line break inside any block.
- **`Backspace` / `Delete`:** one grapheme. At a block's edge, a join (§2). With a selection, the
  selection.
- **`DeleteWordBack`, `KillToEnd`, `KillToStart`:** a word back (UAX #29), to the end of the
  logical line, or to the start of the visual row. With a selection, only the selection. At a
  block's edge, they join, as `Backspace` and `Delete` do.
- **`Indent` / `Outdent`:** nest or un-nest list items and tasks, with their children. Every
  selected line. Paragraphs don't nest (`Outcome::Nothing`).
- **`TaskCycle`:** text → `[ ]` → `[x]` → text.
  - It acts on the line the caret is on. Inside a multi-line paragraph it splits the paragraph
    there (§2), so only that line changes.
  - With a selection, each selected line.
  - A list item or task with soft breaks changes whole.
  - The host can intercept it, for example to advance a repeating task instead of closing it.
- **`MoveLine(±1)`:** the block and its children, up or down.
- **A kind or depth change never moves another line.** `TaskCycle`, a marker typed or deleted,
  `Indent` and `Outdent` all pin every block's gap. No row above the changed blocks moves, and
  rows below move only by the changed blocks' own change in height.
- **Emptied blocks:** a block emptied to nothing is removed when the caret leaves it, and its
  children move up first.

## 4. Layout

- **`Doc::layout(view)` gives rows:** `(block, start, end, x)` for the view's rect width.
  - Wrapping is by words. A word longer than a row breaks between graphemes, never inside one.
  - Width is measured per grapheme cluster: CJK and most emoji take 2 cells.
  - `x` is where the text starts, after indentation and the hang.
- **Every position belongs to exactly one row:** the last row whose start is at or before it.
  At a soft wrap, the boundary byte is the start of the next row, so a row's end is the position
  just before that. The host draws the caret by the same rule, so drawing and motion can't
  disagree.
- **Stops:** text rows are caret stops. Rows a host adds beside the text (a meta line, a footer,
  a blank gap) aren't. Folded children aren't laid out.
- **The host scrolls** with `scroll_to_caret`, which uses the least scroll that shows the caret.

## 5. Motion

Every motion is a pure function of the layout, the caret and the goal column. It never edits.

| Motion | Rule |
|---|---|
| `Up` / `Down` | **The previous or next stop row, by the goal column:** a screen x that's kept across vertical moves and reset by anything else. Land on the greatest position with x ≤ goal, never past it. On the first row `Up` goes to the document start, and on the last `Down` goes to its end, with the goal kept |
| `Left` / `Right` | One grapheme. At a block's edge, into the next or previous block in one press, with no stop on a gap |
| `Home` / `End` | The visual row's start or end. Pressed again at that edge, the block's |
| `WordLeft` / `WordRight` | The previous word's start or the next word's end (UAX #29), skipping spaces and punctuation, crossing blocks |
| `DocStart` / `DocEnd`, `Page(n)` | The document's ends. `n` stop rows by the goal column |
| `NoteUp` / `NoteDown` | The previous or next block's start |

**Invariants** (property-tested over random documents and widths):
1. The caret is always on a grapheme boundary, in exactly one stop row.
2. `Down` from any stop row but the last lands on the next one, never the same one. `Up` mirrors
   it.
3. `Down` then `Up` returns to the same position when the goal is reachable on both rows.
4. Repeating `Down` (once per stop row) reaches the document's end.
5. `Right` from the start visits every position once, and `Left` reverses it.
6. A resize keeps the caret's block and byte.
7. Motion never changes text or undo.

## 6. Selection and the clipboard

caretline follows a macOS text field.

- **With a selection, a motion without `select` collapses it:** `Left` to the start, `Right` to
  the end, `Up` from the start, `Down` from the end. Word, row and document motions start from
  the matching end. **A motion without `select` always leaves no selection.**
- **With `select`, the anchor stays,** and a selection that shrinks to nothing is gone.
- **Typing or pasting replaces the selection.** Every delete removes only the selection. `Indent`,
  `Outdent`, `TaskCycle` and `MoveLine` act on every selected line.
- **`copy` / `cut`:** inside one block, the plain text. Across blocks, Markdown: the first block's
  marker only if the selection includes its start, then each block with its marker and
  indentation. No selection: nothing (an empty string). `BlockLine::fields` lets a host
  re-attach its own data to each copied line.
- **A copy that starts at the end of a block** begins with the block boundary (`\n\n…`), so pasting it back over the same selection changes nothing (EI8). Pasted into another app, it starts with a blank line.
- **`paste`:** one line types in. Several lines are read as Markdown into blocks (a list stays a
  list), in one undo step. `plain` keeps it as paragraphs.
- **Cut, then paste at the same place, is the identity:** text, kinds and IDs.

## 7. Undo

- **Steps:** typing within a short pause, in one block, is one step. Every other command is one
  step. A typed-over selection starts a typing run.
- **Undo restores the text, the caret and the selection.** Redo re-applies.
- **One stack per `Doc`,** shared by its views. `Undo` through any view undoes the doc's last
  step, wherever it was made.
- **A step never removes a block** that arrived from outside after it (`external_edit`).

## 8. Docs, views and outside edits

- **`Doc::apply(view, command, width_of)`** runs a command at that view's caret and returns an
  `Edit`.
- **`Doc::rebase(other_view, &edit)`** moves another view's caret and selection by block
  identity: it stays on its grapheme where it can, and goes to the block's end when that text is
  gone. `apply_and_rebase` and `insert_and_rebase` do both, so no view is forgotten.
- **`View::read_only`:** editing commands are refused with `Refused::ReadOnly`. Motion,
  selection and copy still work.
- **`Doc::external_edit(f)`** applies a change from outside (sync, another program) and returns
  an `Edit`. The host rebases every view with it.
- **`Doc::rev()`** increases on every edit. **`take_deleted()`** lists blocks removed since the
  last call, for the host's save.

## 9. What the host provides

| Through | The host gives |
|---|---|
| `BlockLine` | Its line type: a `Block` plus its own state; new IDs (`fresh`); whether a line was ever saved (`is_new`); how undo keeps or revives its state (`keep_host_state`, `revive`); what a task's state becomes as text (`text_only`); and extra copy text (`fields`) |
| `width_of` | The text width for a line, for commands that need layout |
| `Rect` | Where a view draws, and its size |
| Its own loop | Keys mapped to commands, drawing the rows, saving, and choosing when to save: when the caret leaves a block, after an idle pause, or on a command |

**Pure for a host that replays:** the engine reads the time only through `Doc::with_clock`
(typing runs that undo as one step, `changed_at`) and makes new blocks only through
`Doc::with_lines(depth, kind, text)` (Enter, a split, a paste). They default to `Instant::now`
and `BlockLine::fresh`; a host whose update loop must be a pure function of its messages passes
each message's time and mints ids from it.

**Also provided:** `Anchor` (`doc.anchor(pos)` and `doc.resolve(&anchor)`: a block ID plus a grapheme-aligned byte) for saving a caret across sessions, and the `caretline-ratatui` adapter (`render`, `caret_cell`, `hit`).

Planned, and not in this version:
- spans (a highlighter for the host's own tokens);
- inline triggers (a `[[`-style picker that inserts its result as one step);
- decorations (right-aligned metadata and non-stop rows, measured by the engine);
- a `Measure` trait for non-terminal hosts.

## 10. Conformance

**Caret notation** writes a document and a caret on one line:

| Mark | Means |
|---|---|
| `▮` | The caret |
| `⟦…▮⟧` | A selection, with the caret at the marked end |
| ` ‖ ` | A block boundary with a blank line |
| ` ¦ ` | A block boundary with no blank line |
| `⏎` | A soft break |

A golden is the document before, the keys, and the document after. The engine's tests hold:
- **G1–G22:** motion, including wrapped rows, goal columns, wide graphemes and soft breaks.
- **E1–E81:** selection, edits, word and line deletes, the clipboard, atomic blocks, and kind
  changes that never move other lines.
- **Property tests:** §5's invariants, plus:
  - a motion without `select` leaves no selection;
  - cut then paste is the identity;
  - undo after any edit restores text, caret and selection;
  - a kind change never moves another line;
  - two views on one doc stay consistent under random commands.

A host that embeds caretline can run the same goldens against its own drawing to check its
rendering agrees with the engine.
