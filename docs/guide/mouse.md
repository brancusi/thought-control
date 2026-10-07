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
| **⇧-click** | Extend the selection to here. On a link's title: open that page beside, in the sidebar (see [Links beside](#links-beside)) |
| **⌘-click**, **⌃-click** or **middle-click** a link | Open that page beside, in the sidebar. Your page and its cursor stay where they are |
| **Click a `[ ]`** | Mark it done. Click again to reopen it. (A click never turns a task back into text; `⌃T` does that) |
| **Click a date** on the right (`due fri`) | Change it. Type a new date, then Enter |
| **Click a link** | Goes there, saving first. **Esc** comes back (to the list, if you opened the page from one), and **⌘[** steps back through where you've been. Drag from a link to select instead. To edit the link, click its `[[` or `]]`, **⌥-click** the title, or arrow into it (then `⌃O` goes there). Hovering a link underlines it |
| **Click a day** in the strip under the date, or in the Focus month | Go to that day |
| **Click the `≠` mark** | Compare the two versions of a note |

## Links beside

Open a link in the sidebar without leaving your page, as in Logseq. Which click does it depends
on what your terminal passes on:

| Terminal | Opens beside |
|---|---|
| WezTerm with `thc setup wezterm` | **⌘-click**, ⌃-click, middle-click. (WezTerm keeps ⇧-click for its own selection) |
| WezTerm without it | ⌃-click, middle-click |
| Ghostty, xterm | **⇧-click**, ⌃-click, middle-click. thc asks for ⇧ only while the pointer is on a link, so ⇧-drag elsewhere is still the terminal's selection |
| kitty, iTerm2 | ⌃-click, middle-click (⇧-click where the terminal passes ⇧ on) |

Terminals don't report ⌘ with a click (the mouse protocol has no bit for it), so ⌘-click needs
the terminal's help: `thc setup wezterm` adds a binding that tells thc the click had ⌘ held.
Everywhere, `⌥O` opens the link under the cursor beside.

Opening a panel never moves your page: the text keeps its row and, where the page can scroll to
hold it, so does the line your cursor is on.

## The sidebar

| Do | Gets you |
|---|---|
| **Click** in a panel's text | The keyboard goes to that panel, the cursor where you clicked |
| **Click a link** in a panel | Follows it in the main view (the panel stays) |
| **Click** a panel's `▾` or title | Fold or unfold it. **Double-click** the title: open it in the main view |
| **Click `↗`** | Open it in the main view, where its cursor is |
| **Click `×`**, or middle-click the header | Close it (`⌥⇧T` reopens it) |
| **Click `pinned`** | Unpin it |
| **Click `↓ 9 more`** | Focus that panel (it gets the room) |
| **Wheel** over a panel | Scroll it, without moving its cursor |
| **Drag** a panel's header | Move it; dropped among the pinned panels it's pinned |
| **Drag the divider** | Resize the sidebar. Double-click it: automatic width |
| **⇧-click** a day (the strip, the rail) or a page in the rail | Open it beside |
| **Click** the main view | The keyboard goes back there |

## Scrolling

The wheel and trackpad scroll the page **without moving your cursor**. Start typing and you're
back where you were. A click after scrolling puts the cursor exactly where you clicked; the
page doesn't jump back. Long pages show a thin scrollbar on the right: drag it, or click its track
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
