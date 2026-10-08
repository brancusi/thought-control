# Writing

The journal and every page open as a plain-text document. You type, and `thc` keeps track of
the structure for you.

## How the text becomes notes

| You write | It's |
|---|---|
| A paragraph (lines with no blank line between) | One note. Enter inside it is just a line break |
| A blank line | The end of a note. The next text starts a new one |
| `- milk` | A list item (one note). Enter continues the list |
| `1. first` | A numbered item. Enter makes `2. ` |
| `[ ] call` / `[x] call` | A task, open or done |
| `# Title`, `## Section` | A heading |
| Indented items | Children of the item above |

Each note has its own identity, dates and history, and you never see that machinery. Delete the
`[ ]` and it's plain text, because what you see is the truth.

## Dates, tags and priority, inline

Type them anywhere on a line:

| Token | Means |
|---|---|
| `due:fri`, `due:2026-10-10`, `due:"nov 1 9am"` | A hard deadline |
| `sched:mon` | When you plan to start (it shows in Today from then) |
| `at:"tue 2pm"` | An event at a time |
| `every:2w`, `every:weekday`, `every!:1mo` | Repeats (`!` counts from completion) |
| `!high` `!med` `!low` | Priority |
| `#tag` | A tag. It stays in the text |
| `[[Page]]` | A link. A new name creates the page |

While you type, tokens are underlined, and the right column previews what they'll set. When you
leave the line, the dates move into that quiet column. Type a new token to change one, or click
it.

## Where you start typing

Open a page or a day (`⌃O`, a link, a row) and just type: the cursor waits on a fresh line after
its notes, so what you write is a note of its own. Come back later and the cursor is where you
left it. `⌃Home` goes to the top.

## The keys you need

| Key | Does |
|---|---|
| Just type | Write |
| `Enter` | A new line. Twice: a new note. In a list: the next item |
| `Tab` / `⇧Tab` | Indent / outdent a note: any note nests under the one above, paragraphs too. Tab on a paragraph's later line makes that line its child |
| `⌃T` | Text → task → done → text |
| `⇧` + arrows | Select |
| `⌃C` / `⌃X` | Copy / cut (paste with your terminal, usually ⌘V) |
| `⌃Z` / `⌃Y` | Undo / redo |
| `[[` | Link a page |
| `⌃O` | Open the link under the cursor, or go to any page or day |
| `⌃P` / `⌃N` | Previous / next day |
| `Esc` | Save and go back: to the Pages list or the view you opened it from, else Today |
| `⌃Q` | Quit (everything is already saved) |
| `F1` | All keys |

## Pages beside the page

`⇧`-click a `[[link]]` (or put the cursor in it and press `⌥O`) and the page opens in the
**sidebar**, on the right, instead of replacing what you're writing. Open more and they stack,
newest on top; opening one that's already there moves it back to the top. A plain click still
follows the link in the main view, from a panel too.

A list beside (Today, a saved view, a `#tag`) works like the list itself: `j` `k` move, `x`
completes, `d` `p` `#` `m` act on its row, `Enter` opens the row in the main view and `o` (or
`⇧Enter`) opens its page or day as another panel. From a list in the main view, `o`, `⇧Enter` or
`⌥O` open the selected row's page beside.

A panel is an editor, not a preview: the same page open in the main view and in a panel is one
page, so typing in either shows in both at once, and `⌃Z` undoes it from either. Changes from an
agent or another device show in panels as they happen.

| Key | Does |
|---|---|
| `⌥O` | Open the link under the cursor (or the selected row's page or day) beside |
| `⌥S` | Move the keyboard to the sidebar and back |
| `Esc` | In the sidebar: back to the main view (the panel stays) |
| `⌥J` / `⌥K` | Next / previous panel |
| `⌥C` | Fold or unfold the panel |
| `⌥W` | Close the panel (`⌥⇧T` reopens it, cursor and all) |
| `⌥M` | Open the panel's page in the main view, where its cursor is |
| `⌥P` | Pin the panel: pinned panels stay on top and aren't closed to make room |
| `⌥⇧K` / `⌥⇧J` | Move the panel up / down |
| `⌥\` | Hide or show the sidebar |
| `⌥=` / `⌥-` / `⌥0` | Wider / narrower / automatic width |
| `⌃P` / `⌃N` | In a day panel: the day before / after, in the same panel |
| `Space w` | The same, from the leader: `w w` focus, `w o` aside, `w x` close, `w X` close all, … |
| `:aside …` | Open a page by title, a day (`today`, `fri`, `2026-10-06`), a list (`@today`, `@inbox`, `@tasks`, `@log`, a saved `@view`), a `#tag` or any query beside. `:aside today` follows the date at midnight |

The sidebar is a column from 120 columns wide (a third of the screen, 40 to 64 columns; drag
the divider to change it, double-click it for automatic); the detail pane and the Journal's
calendar give way to it while it shows. Narrower, it's a drawer over the right of the screen
(90 to 119 columns) or takes the whole screen (under 90), while the keyboard is in it: `Esc`
closes it and keeps the panels. Drag a panel's header to reorder; dropped among the pinned
panels, it's pinned too. It holds up to 8 panels.
Each vault keeps its own stack, and it's there again when you come back. `⌥` keys need
"Option as Meta" in Terminal.app and iTerm2.

## Saving, undo and other devices

- **Saving is automatic:** when you leave a note, after a short pause, and when you leave or
  quit. If a save is late or fails, the footer says so.
- **Undo** (`⌃Z`) works across saves.
- **If an agent or another device changes the note you're in,** `thc` tells you at once and
  applies it when you move off. If you both changed it, both versions are kept, and you choose
  (`1 keep yours`, `2 keep theirs`, `b both`).

## Focus

`⌥Z` hides everything but your words and the date. `:focus` lets you choose what stays: the key
footer, the day strip, a month calendar, the dates column, a word count, a clock. Three
starting points are `bare`, `writer` and `planner`.
