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

## The keys you need

| Key | Does |
|---|---|
| Just type | Write |
| `Enter` | A new line. Twice: a new note. In a list: the next item |
| `Tab` / `⇧Tab` | Indent / outdent a list item |
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
