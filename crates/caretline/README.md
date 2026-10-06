# caretline

**A plain-text block editor engine: Markdown in, blocks out, with exact caret motion.** Bring
your own renderer and storage.

caretline is the editor inside [thought-central](https://thoughtcontrol.app),
a notebook in the terminal, and it's written for any app:
- Writing feels like typing in a text file.
- Structure comes from Markdown markers you can see (`- `, `[ ] `, `## `).
- Where the caret goes after every key is a pure function with a test.

## The model in sixty seconds

- **A document is a list of blocks:** paragraphs, list items and tasks, nested by depth. A
  heading, a numbered item or a quote is a paragraph whose text starts with its marker, which
  your app draws in the margin.
- **Markers are text.** Delete `[ ] ` and the task is plain text, with no hidden state. Typing a
  marker at a line's start makes the block.
- **One newline inside a block is a soft break.** A blank line, or Enter on an empty line,
  starts a new block. That's the one difference from CommonMark.
- **A `Doc` holds the blocks, their identity and undo. A `View` is one place it's shown:** a
  caret, a selection, a goal column, a scroll position, folds, a rect, and whether it may edit.
  Two views on one doc share the text and the undo, and an edit through one rebases the others.
- **Commands are data.** Your app maps its keys to a `Command` (plus `insert`, `paste`, `copy`
  and `cut`). caretline applies it and says what happened.
- **Layout is rows.** `Doc::layout` wraps a view's blocks to its width, by grapheme and cell
  width, and every caret position belongs to exactly one row. What's drawn, where a click lands
  and where ↓ goes can never disagree.

## Five minutes

```rust
use caretline::{Block, BlockLine, Command, Doc, Kind, Motion, Rect, View};

// 1. Your line type: a caretline Block, plus whatever your app keeps beside it.
#[derive(Clone, Debug, PartialEq)]
struct Line {
    block: Block<u64>,
    saved: bool,
}

impl std::ops::Deref for Line {
    type Target = Block<u64>;
    fn deref(&self) -> &Block<u64> { &self.block }
}
impl std::ops::DerefMut for Line {
    fn deref_mut(&mut self) -> &mut Block<u64> { &mut self.block }
}

static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl BlockLine for Line {
    type Id = u64;
    fn fresh(depth: usize, kind: Kind, text: &str) -> Self {
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Line { block: Block::new(id, depth, kind, text), saved: false }
    }
    fn is_new(&self) -> bool { !self.saved }
    fn keep_host_state(&mut self, cur: &Self) { self.saved = cur.saved; }
    fn revive(&mut self) { self.saved = false; }
}

fn main() {
    // 2. A document, and a view onto it 40 cells wide.
    let mut doc = Doc::new(vec![Line::fresh(0, Kind::Para, "")]);
    let mut view = View::new(Rect { x: 0, y: 0, width: 40, height: 10 });
    let width = |_: &Line| 40;

    // 3. Type, as a host would after mapping keys to commands.
    doc.insert(&mut view, "Plan the trip").unwrap();
    doc.apply(&mut view, Command::Newline, &width).unwrap(); // a line break, same block
    doc.apply(&mut view, Command::Newline, &width).unwrap(); // on an empty line: a new block
    doc.insert(&mut view, "Book the flat").unwrap();
    doc.apply(&mut view, Command::TaskCycle, &width).unwrap(); // ⌃T: this line is a task
    assert_eq!(doc.lines().len(), 2);
    assert_eq!(doc.lines()[1].kind, Kind::Task);

    // 4. Move and select: ⇧Home selects back to the start of the row. Copy is Markdown-aware.
    doc.apply(&mut view, Command::Move { motion: Motion::Home, select: true }, &width).unwrap();
    assert_eq!(doc.copy(&view), "Book the flat");

    // 5. Draw: the rows the view shows, each a byte range of one block, starting at cell x.
    for row in doc.layout(&view) {
        let text = &doc.lines()[row.line].text[row.start..row.end];
        println!("{:>width$}{text}", "", width = row.x);
    }
    // Undo is one command, and puts the selection back too.
    doc.apply(&mut view, Command::Undo, &width).unwrap();
    assert_eq!(doc.lines()[1].kind, Kind::Para); // the ⌃T step, undone
}
```

That's the whole loop: blocks in, commands applied, rows out. Your app owns the keys, the
colours, the files and the saves.

To see it in a terminal, a whole editor in one file (keys, mouse, a Markdown file saved with
⌃S) is `caretline-ratatui`'s example: `cargo run -p caretline-ratatui --example editor -- notes.md`.

## What it is not

| Not | Because |
|---|---|
| A rich-text or WYSIWYG editor | The text you see is the text you have. Styling is your app's, through the rows it draws |
| A sync engine or a CRDT | Your app owns storage and merging. caretline reports what changed (`Doc::rev`, `take_deleted`) and accepts edits from outside (`Doc::external_edit`, then `Doc::rebase` for every view) |
| A terminal UI | It lays out in cells and never draws. A ratatui adapter (`caretline-ratatui`) is separate |
| Opinionated about dates, links or files | Those are your app's. It keeps its own state beside each block through `BlockLine` |

## What's inside

- **The block model and Markdown:** `markdown::parse_paste` reads Markdown into blocks, and
  `markdown::to_markdown` writes them back (copy and cut).
- **Editing:**
  - Enter's rules, soft breaks, joins and nesting.
  - The task cycle: `⌃T` goes text → `[ ]` → `[x]` → text, per line, even inside a paragraph.
  - Moving blocks, and word and line deletes.
  - A blank line before a block is kept as layout, so a kind change never moves another line.
- **Motion:** ↑↓ with a goal column, ←→ by grapheme, words (UAX #29), Home/End, pages and the
  document's ends, each with `select`.
- **Selection and the clipboard,** following a macOS text field: arrows collapse a selection,
  typing replaces it, and copy is Markdown-aware.
- **Undo:** typing runs are grouped, every other command is one step, and the selection is
  restored.

## Tests you can trust

caretline's rules are written as examples, and the examples run:
- **Goldens in caret notation** (`▮` is the caret, `⟦…⟧` a selection): the motion scenarios
  G1–G22 and the editing scenarios E1–E81.
- **Property tests over random documents:**
  - every caret position is valid;
  - ↓ never gets stuck;
  - a motion without ⇧ leaves no selection;
  - cut then paste is the identity;
  - undo restores everything;
  - a kind change never moves another line.
- **The boundary:** caretline depends on nothing from its first app, and CI checks that.

`cargo test -p caretline` runs them all. The rules themselves are in [SPEC.md](SPEC.md).

Licence: MIT OR Apache-2.0.
