---
title: Keys you'll use
description: The handful of keys that do most of the work, the one key that finds the rest, and how to remap any of them.
order: 8
group: Guide
---

You don't need to learn many. There are two places you can be, and one key in each that shows
you everything else:

- **Writing** (a journal day or a page): <kbd>F1</kbd>.
- **Everywhere else** (Today, Inbox, Tasks, Search, Log): <kbd>Space</kbd>, which opens a panel
  of every command, grouped. Or <kbd>?</kbd> for help.

The footer always shows the few keys that work right now, and they're your keys: remap one and
the footer, help and the palette all change with it. Every key, by where it works, is in the
[keys reference](../keys-reference/), generated from the same table thc runs on, so it can't
drift.

## While writing

| | |
|---|---|
| <kbd>⌃T</kbd> | Text → task → done → text |
| <kbd>Enter</kbd> | New line. Twice: a new note |
| <kbd>⇧Enter</kbd> or <kbd>⌃J</kbd> | A line break inside the same item |
| <kbd>Tab</kbd> / <kbd>⇧Tab</kbd> | Indent / outdent |
| <kbd>⌥↑</kbd> / <kbd>⌥↓</kbd> | Move a line, with its children |
| <kbd>⇧</kbd> + arrows | Select |
| <kbd>⌘C</kbd> / <kbd>⌘X</kbd> (or <kbd>⌃C</kbd> / <kbd>⌃X</kbd>) | Copy / cut, as Markdown |
| <kbd>⌘A</kbd> (or <kbd>⌥A</kbd>) | Select all |
| <kbd>⌘V</kbd> / <kbd>⌃V</kbd> | Paste, screenshots too. <kbd>⌥V</kbd> makes the next paste plain |
| <kbd>⌘Z</kbd> / <kbd>⇧⌘Z</kbd> (or <kbd>⌃Z</kbd> / <kbd>⌃Y</kbd>) | Undo / redo |
| <kbd>[[</kbd> | Link a page |
| <kbd>⌃O</kbd> | Open the link under the cursor, or go to any page or day |
| <kbd>⌃P</kbd> / <kbd>⌃N</kbd> | The day before / after |
| <kbd>⌥Z</kbd> | Focus |
| <kbd>Esc</kbd> | Save and go **up**: to the list or day where you started, else Today |
| <kbd>⌘[</kbd> / <kbd>⌘]</kbd> (or <kbd>⌃⌥←</kbd> / <kbd>⌃⌥→</kbd>) | Back and forward through the places you've been, like a browser. `:history` (or <kbd>space</kbd> <kbd>g</kbd> <kbd>h</kbd>) lists them |
| <kbd>⌃Q</kbd> | Quit. It's all saved already |

**Moving around** works the way a Mac text field does: <kbd>⌘←</kbd> <kbd>⌘→</kbd> to the line's
start and end, <kbd>⌘↑</kbd> <kbd>⌘↓</kbd> to the top and bottom of the page, <kbd>⌥←</kbd>
<kbd>⌥→</kbd> by word, add <kbd>⇧</kbd> to select. <kbd>⌘⌫</kbd> deletes to the start of the
line, <kbd>⌥⌫</kbd> a word. Readline keys work too (<kbd>⌃A</kbd> <kbd>⌃E</kbd> <kbd>⌃W</kbd>
<kbd>⌃K</kbd> <kbd>⌃U</kbd>), and so do <kbd>Home</kbd>, <kbd>End</kbd>, <kbd>PgUp</kbd> and
<kbd>PgDn</kbd>. The arrows follow what's on screen: through wrapped lines, keeping your column.

Your terminal has to send the <kbd>⌘</kbd> keys. kitty does. In WezTerm, run
`thc setup wezterm --yes` once: the keys then act only while thc is running, and your shell and
editors are untouched. After an update, thc refreshes its own key file the next time it runs (it never
edits your wezterm.lua), so new ⌘ keys work without running setup again. Everywhere else, the <kbd>⌃</kbd> and <kbd>⌥</kbd> twins work. On a Mac,
turn on "Option as Meta" in your terminal for the <kbd>⌥</kbd> keys; thc tells you if it's off.

## In lists

| | |
|---|---|
| <kbd>j</kbd> / <kbd>k</kbd> | Move (or the arrows) |
| <kbd>Enter</kbd> | Open it where it lives |
| <kbd>x</kbd> / <kbd>X</kbd> | Done / reopen |
| <kbd>d</kbd> / <kbd>s</kbd> | Due / scheduled date |
| <kbd>p</kbd> then <kbd>h</kbd> <kbd>m</kbd> <kbd>l</kbd> | Priority |
| <kbd>#</kbd> | Tags |
| <kbd>m</kbd> | Move |
| <kbd>a</kbd> / <kbd>A</kbd> | Add here / to the inbox |
| <kbd>D</kbd> | Delete (<kbd>u</kbd> brings it back) |
| <kbd>u</kbd> | Undo |
| <kbd>/</kbd> | Search |
| <kbd>f</kbd> | Filter (Tasks) |
| <kbd>1</kbd>–<kbd>7</kbd>, <kbd>Tab</kbd> | Views |
| <kbd>:</kbd> | Commands, by name |
| <kbd>⌃O</kbd> | Go to a page or day |
| <kbd>q</kbd> | Back, then quit |

On Pages and Search, typing finds: letters go into the find box, the arrows move, and
<kbd>Enter</kbd> opens. With the box empty, <kbd>1</kbd>–<kbd>7</kbd>, <kbd>?</kbd>, <kbd>:</kbd>,
<kbd>Space</kbd> and <kbd>q</kbd> work as everywhere else. <kbd>Esc</kbd> clears the find, then
goes back.

<kbd>Tab</kbd> always goes round the views. Arriving on a journal day or a page by
<kbd>Tab</kbd>, the document waits: <kbd>Tab</kbd> keeps moving between views, and any other
key, or a click in the text, starts writing. Nothing you type is lost.

## The leader

<kbd>Space</kbd> in any list opens a panel that shows what can follow. Read it instead of
remembering:

```
─ space ─────────────────────────────────────────────────────
 f  +find          t  +toggle        r  review lane    q  quit
 g  +go            v  +views         e  $EDITOR        ?  every key
 n  +new           space  commands   E  $EDITOR page
```

<kbd>space</kbd> <kbd>g</kbd> <kbd>t</kbd> goes to Today, <kbd>space</kbd> <kbd>n</kbd>
<kbd>p</kbd> makes a page, <kbd>space</kbd> <kbd>t</kbd> <kbd>f</kbd> toggles Focus,
<kbd>space</kbd> <kbd>v</kbd> <kbd>1</kbd> runs your first saved view. <kbd>⌫</kbd> steps back
one key, <kbd>Esc</kbd> closes.

## The mouse

Click to place the cursor, drag to select, click a `[ ]` to toggle it, click a date to change
it, and click any tab, row or footer key. Hold <kbd>⇧</kbd> (<kbd>⌥</kbd> in iTerm2 and
Terminal.app) while dragging for your terminal's own selection. More in
[The mouse](../mouse/).

## Remapping

**The easy way:** `thc keys --edit`. It opens your config at a list of every key, commented out
and grouped by where it works, each with what it does:

```toml
# ── Keys (thc keys --edit) ─────────────────────────────────────────
# Remap a key: uncomment a line and its [keys.…] line, and change the key on the left.
# "no_op" unbinds a key. Notation: C- ⌃  A- ⌥  S- ⇧  Cmd- ⌘

# [keys.global]
# "Cmd-[" = "nav.back"               # ⌘[       back
# "?" = "help.context"               # ?        keys
```

Uncomment a line, change the key, save, and thc checks it: `keys ok · 1 remapped`, or the problem
and its line. Run it again any time: the list refreshes, and your changes stay.
`--context write` opens at one table, and `--print` shows the block without opening an editor.

Three common moves:

```toml
[keys.global]
"q" = "no_op"            # unbind a key
"F9" = "focus.toggle"    # add a second key for an action

[keys.write]
"C-g" = "doc.open"       # give an action a different key
```

**By hand,** the same tables work anywhere in your config. Put `[keys.<context>]` tables in `~/.config/thought/config.toml`: a key on the left, an action
on the right. A key replaces what it did in that context; `"no_op"` unbinds it.

```toml
[keys.list]
"C-x" = "node.done"     # ⌃X completes, as well as x
"q" = "no_op"           # never quit with q

[keys.write]
"A-t" = "doc.task_cycle"
```

The action names are in the [keys reference](../keys-reference/). Contexts: `global`, `list`,
`today`, `inbox`, `tasks`, `pages`, `journal`, `search`, `log`, `write`. The pop-ups keep their
own keys for now. A vault's `settings.toml` can carry `[keys.*]` too, so a team shares a layout.

thc checks your remaps when it starts and says in the bar what it refused: an unknown action, a
plain letter in `write` (it would stop you typing it), a key that starts a longer one, or
<kbd>Esc</kbd> and <kbd>⌃C</kbd> taken away.

```sh
$ thc keys              # the keymap in effect, your remaps included
$ thc keys --conflicts  # every problem a remap has
$ thc keys --json       # the table, for scripts
```
