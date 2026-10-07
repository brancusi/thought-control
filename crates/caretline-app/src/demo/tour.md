# Welcome to caretline

A terminal editor where the whole editor is one value you can save, send and replay. Work down this page with ↓: the status bar names the keys for the section the caret is in.

## 1 · Type

The caret is already at the end of this paragraph, so just start typing, and keep going past the edge of the window.

## 2 · Move

⌥← and ⌥→ jump a word at a time. ↑ and ↓ move by the rows you see, not by lines, and keep their column: go down through this paragraph and watch the caret hold its place across wrapped rows and short ones.
Short row.
And a long row again, so the column has somewhere to come back to as you keep going down past the short one.

## 3 · Select

Hold ⇧ with any arrow to select, and ⇧⌥ to select by words. As in a macOS text field, ← on a selection goes to its start, → to its end, and Esc collapses it.

## 4 · Multiple carets

Put the caret just before the first "one" below and press ⌃N twice: a caret on each row, in the same column. Now type. Esc goes back to one caret.

- one apple
- one pear
- one plum

## 5 · Undo, exactly

Select a few words and type over them, then ⌃Z: the text, the caret and the selection come back exactly as they were. ⌃Y redoes. Typing within a second and a half is one step.

## 6 · Indent and move

Tab and ⇧Tab indent and outdent a line with everything under it. ⌥↑ and ⌥↓ move it past its neighbours, children and all.

- Pack the bag
- Passport
  - and the charger
- Book

## 7 · Folds

- Put the caret on this line and press ⌃O to fold away the lines under it
  - a passport
  - a charger
  - a good book

## 8 · Marks

Every block carries a mark: an id that follows it through edits, moves, undo, and cut and paste. With the caret here, the status bar shows the block's mark. Move the line below with ⌥↑, cut and paste it, undo: the number stays.

- Hold on to me

## 9 · A second view

⌃G opens a second view of this document below this one: the same text, with its own caret and scroll. Type up here and watch it change there. ⌃G closes it.

## 10 · One state

The text, every caret, the undo history, the marks, the folds, the scroll and the clock are one serializable value. Press ⌃D to write it to state.json and see how big it is.

## 11 · Replay

A session is that state plus every message since: each key, each tick of the clock. Press ⌃P to watch yours replay from the start and land on the identical state.

That's the tour. ⌃Q quits (twice to leave without saving). Next: caretline demo agent, where an agent edits beside you, and caretline demo scenes.
