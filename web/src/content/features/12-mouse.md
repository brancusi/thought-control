---
title: The mouse
blurb: Everything you can see, you can click. The keys still do the same.
icon: mouse
order: 12
---

The mouse does what the keys do, never something else, so mix them freely.

**While writing**

| Do | Gets you |
|---|---|
| **Click** | The caret exactly there, even on a wrapped line or mid-emoji |
| **Drag** | A selection across lines and notes. <kbd>⌃C</kbd> copies it as Markdown |
| **Double-click** | A word |
| **Triple-click** | The whole note |
| **⇧-click** | Extend the selection |
| **Click a `[ ]`** | Done, and click again to reopen |
| **Click a date** in the margin | Edit it as you'd type it (`fri`, `oct 9`) |
| **Click** a `[[link]]` | Go there, saving first. <kbd>Esc</kbd> comes back. Hovering underlines it |
| **Click its `[[` / `]]`**, or **⌥-click** it | Place the cursor in the link, to edit it |
| **Click a day** in the strip or the Focus month | Go there |
| **Click a `≠`** | Compare the two versions |

**Scrolling**

- **The wheel scrolls without moving your caret.** Start typing and the view comes back.
- **A thin scrollbar** on long pages: drag the thumb, or click the track to move a page. Lists have them too.

**Everywhere else**

- **Tabs** switch views. **Rows** select on a click and open on a double-click; a row's `[ ]` completes it.
- **Footer keys are buttons:** clicking `F1 keys` is pressing <kbd>F1</kbd>.
- **Help is a menu:** click any row to run it.
- **Every pop-up** (the vault picker, the palette, the go-to finder, move, `[[` suggestions, help, `:focus`, compare, the capture bar): hover highlights a row, one click picks it, the wheel scrolls, and a click outside closes without doing anything else.
- **Hover** lights up what's clickable, in a local terminal. Over SSH it's off unless you set `hover = "on"`.

**Your terminal's own selection** is one modifier away: <kbd>⇧</kbd>-drag in WezTerm, kitty and Ghostty, <kbd>⌥</kbd>-drag in iTerm2 and Terminal.app. thc says so the first time you drag.

**Turning it off:** `:mouse off` for this session, `mouse = false` under `[tui]` for good, `hover = "off"` to keep clicks without the highlight.
