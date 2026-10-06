---
title: Keys
blurb: One table behind every key, hint and help screen. Press Space and read.
icon: key
order: 13
---

One keymap table drives dispatch, the footer, help, the palette and `thc keys`, so they can never disagree.

- **The footer** shows only the keys that work right now, and fits narrow windows.
- **The leader:** <kbd>space</kbd> in any list opens a which-key panel of every command, grouped. <kbd>g</kbd> go, <kbd>f</kbd> find, <kbd>n</kbd> new, <kbd>t</kbd> toggle, <kbd>v</kbd> saved views. <kbd>⌫</kbd> steps back, <kbd>Esc</kbd> closes. While writing, space is a space.
- **The palette:** <kbd>:</kbd> (or <kbd>space</kbd> <kbd>space</kbd>) lists every command by name with its key, your last three first. Type a few letters, <kbd>Enter</kbd>.
- **Help:** <kbd>F1</kbd>, or <kbd>?</kbd> in lists, shows the keys for exactly where you are. <kbd>?</kbd> again shows every key. Click a row to run it.
- **Back and forward like a browser:** <kbd>⌘[</kbd> / <kbd>⌘]</kbd> (or <kbd>⌃⌥←</kbd> / <kbd>⌃⌥→</kbd>), across pages, days, views and vaults. `:history` lists where you've been.
- **Remap anything with `thc keys --edit`:** your config opens at every key, commented out with what it does. Uncomment, change, save, and thc checks it (`keys ok · 1 remapped`).
- **Two layers, no modes to learn:** in a document you just type, and the writing keys are chords (<kbd>⌃T</kbd>, <kbd>⌃O</kbd>, <kbd>⌃P</kbd>/<kbd>⌃N</kbd>, <kbd>⌥Z</kbd>). <kbd>Esc</kbd> saves and goes back to the lists, where single letters act (<kbd>x</kbd>, <kbd>d</kbd>, <kbd>m</kbd>).
- **Readline and Mac keys both work:** <kbd>⌃A</kbd> <kbd>⌃E</kbd> <kbd>⌃W</kbd> <kbd>⌃K</kbd> <kbd>⌃U</kbd>, and <kbd>⌘←</kbd>/<kbd>⌘→</kbd> line ends, <kbd>⌘↑</kbd>/<kbd>⌘↓</kbd> document ends, <kbd>⌥←</kbd>/<kbd>⌥→</kbd> words, <kbd>⌘⌫</kbd> and <kbd>⌥⌫</kbd>. Add <kbd>⇧</kbd> to select.

```sh
$ thc keys                # every key, by context
$ thc keys --json         # the table itself
$ thc keys --conflicts    # what a remap would be refused for
```

**Remap per context** in `~/.config/thought/config.toml` (or a vault's `settings.toml`):

```toml
[keys.list]
"C-x" = "node.done"     # ⌃X completes, as well as x
"q" = "no_op"           # never quit with q

[keys.write]
"A-t" = "doc.task_cycle"
```

thc checks remaps when it starts and says what it refused: an unknown action, a plain letter in `write`, a key that starts a longer one, or taking <kbd>Esc</kbd> away.

**WezTerm in one command.** `thc setup wezterm --yes` adds the Mac editing keys and <kbd>⌘V</kbd> screenshots: thc's own key file plus one marked block in the config WezTerm loads, backed up first and never added twice. The keys act only inside thc; in your shell, an editor or SSH they do what they did. It also turns on the kitty keyboard mode, which is what lets <kbd>⇧Enter</kbd> break a line. `--undo --yes` takes it out. After an update, thc refreshes its own key file the next time it runs (never your wezterm.lua), so new ⌘ keys such as <kbd>⌘[</kbd> just work. In kitty, the <kbd>⌘</kbd> keys work as themselves.

**Terminal notes:** thc asks for the smallest keyboard mode it needs, so capitals, other layouts and accented keys arrive as typed, and it puts the keyboard back if it ever crashes. In a Mac terminal without Option as Meta, or a tmux holding <kbd>Esc</kbd> too long, it tells you how to fix it.
