# The mouse

Everything you can see, you can click. The mouse does what the keys do, never something else,
so you can mix them freely.

## While writing

| Do | Gets you |
|---|---|
| **Click** in text | The cursor goes exactly there, even on a wrapped line or in the middle of an emoji or a CJK character |
| **Drag** | Select across lines and notes. `⌃C` copies the selection as Markdown |
| **Double-click** | Select a word |
| **Triple-click** | Select the whole note (a paragraph or a list item) |
| **⇧-click** | Extend the selection to here |
| **Click a `[ ]`** | Mark it done. Click again to reopen it. (A click never turns a task back into text; `⌃T` does that) |
| **Click a date** on the right (`due fri`) | Change it. Type a new date, then Enter |
| **Click a link** | Goes there, saving first. **Esc** comes back. To edit the link instead, click its `[[` or `]]`, **⌥-click** the title, or arrow into it (then `⌃O` goes there). Hovering a link underlines it |
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
- **The keys in the footer** are buttons: clicking `F1 keys` is the same as pressing F1.
- **Help** (`F1`) is a menu too: click any row to run it.
- **Pop-ups** (the palette, link suggestions, `:focus`, the compare): click a row or button to
  choose it, and click outside to close. That outside click doesn't do anything else.
- **Hover** highlights what's clickable, in terminals that report mouse movement.

## Your terminal's own selection

While `thc` has the mouse, a plain drag selects **in thc**. To use your terminal's own selection
(to copy exactly what's on screen, across panes), hold a key while you drag:

| Terminal | Hold |
|---|---|
| WezTerm, kitty, Ghostty | `⇧` |
| iTerm2, Terminal.app | `⌥` |

`thc` reminds you of this once, the first time you drag.

## Turning it off

- `:mouse off` for this session (`:mouse on` brings it back).
- `mouse = false` under `[tui]` in `~/.config/thought/config.toml` to turn it off for good.
- `hover = "off"` keeps clicks but stops the hover highlight. Over SSH, hover is off by default.
