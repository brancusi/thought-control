---
title: The mouse
description: Everything you can see, you can click, and the mouse never does something the keys don't.
order: 7
group: Guide
---

Everything you can see, you can click. The mouse does what the keys do, never something else,
so you can mix them freely.

## While writing

| Do | Gets you |
|---|---|
| **Click** in text | The cursor goes exactly there, even on a wrapped line or in the middle of an emoji or a CJK character. Past the end of a line: the end of that line |
| **Drag** | Select across lines and notes. <kbd>⌘C</kbd> (or <kbd>⌃C</kbd>) copies the selection as Markdown |
| **Double-click** | Select a word |
| **Triple-click** | Select the whole note (a paragraph or a list item) |
| **⇧-click** | Extend the selection to here |
| **Click a `[ ]`** | Mark it done. Click again to reopen it. A click never turns a task back into text; <kbd>⌃T</kbd> does that |
| **Click a date** on the right (`due fri`) | Change it. Type a new date, then <kbd>Enter</kbd> |
| **Click a link** | Goes there, saving first. <kbd>Esc</kbd> comes back to where you were. Hovering a link underlines it |
| **Click a link's `[[` or `]]`**, or **⌥-click** it | Puts the cursor in the link, to edit it. Arrowing into it works too, and then <kbd>⌃O</kbd> goes there. A drag that starts on a link still selects |
| **Click an attachment** | Puts the cursor on it. A **double-click** opens it in your viewer |
| **Click a day** in the strip under the date, or in the Focus month | Go to that day |
| **Click the `≠` mark** | Compare the two versions of a note |

## Scrolling

The wheel and trackpad scroll the page **without moving your cursor**. Start typing and you're
back where you were. Long pages show a thin scrollbar on the right: drag it, or click its track
to jump a page.

## Everywhere else

- **Tabs** along the top switch views.
- **Rows** in Today, Inbox, Tasks and the rest: click to select, double-click to open. Click a
  row's `[ ]` to mark it done.
- **The keys in the footer** are buttons: clicking `F1 keys` is the same as pressing
  <kbd>F1</kbd>.
- **Help** (<kbd>F1</kbd>) is a menu too: click any row to run it.
- **The detail pane** and the chips in lists are clickable.
- **Every pop-up** (the vault picker, the palette, the go-to finder, move, the `[[` suggestions,
  help, `:focus`, the compare, the capture bar): hover highlights a row, **one click picks it**,
  the wheel scrolls, and a click outside closes. That outside click doesn't do anything else.
  The vault picker has a `+ new vault` row.
- **Hover** highlights what's clickable, in terminals that report mouse movement.

## Your terminal's own selection

While `thc` has the mouse, a plain drag selects **in thc**. To use your terminal's own
selection (to copy exactly what's on screen, across panes), hold a key while you drag:

| Terminal | Hold |
|---|---|
| WezTerm, kitty, Ghostty | <kbd>⇧</kbd> |
| iTerm2, Terminal.app | <kbd>⌥</kbd> |

`thc` reminds you of this once, the first time you drag.

## Turning it off

- `:mouse off` for this session (`:mouse on` brings it back).
- `mouse = false` under `[tui]` in `~/.config/thought/config.toml` turns it off for good.
- `hover = "off"` keeps clicks but stops the hover highlight. Over SSH, hover is off unless you
  set `hover = "on"`.
- `wheel_rows` sets how far one wheel step moves a list.
