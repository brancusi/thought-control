# Welcome to caretline

A terminal editor where the whole editor is one value you can save, send and replay. Work down this page: try each step, then tick its box with ⌃T. The status bar always says what's next.

## 1 · Type

The caret is already at the end of this paragraph, so just start typing, and keep going past the edge of the window.

- [ ] Done (⌃T ticks this box)

## 2 · Tasks and nesting

- Press ⌃T on this line to make it a task, and again to tick it
- Tab nests this item under the one above, ⇧Tab brings it back
- ⌥↑ and ⌥↓ move an item, children and all
- [ ] Done

## 3 · Select, undo, redo

Hold ⇧ with any arrow to select, ⌥ to go a word at a time. Type over the selection, then ⌃Z to undo and ⌃Y to redo: the text, the caret and the selection come back exactly.

- [ ] Done

## 4 · Folds

- Put the caret on this line and press ⌃O to fold its children away
  - a passport
  - a charger
  - a good book
- [ ] Done

## 5 · One state

The text, every caret, the undo history, the folds, the scroll and the clock are one serializable value. Press ⌃D to write it to state.json and see how big it is.

- [ ] Done

## 6 · Replay

A session is that state plus every message since: each key, each tick of the clock. Press ⌃P to watch yours replay from the start, and land on the identical state.

- [ ] Done

That's the tour. ⌃Q quits (twice to leave without saving). Next: caretline demo scenes, and caretline demo agent.
