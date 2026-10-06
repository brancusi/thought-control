---
title: History
blurb: An append-only log. Undo anything, rewind a note, ask how it was.
icon: history
order: 7
---

Nothing in a vault is overwritten. Every change is an event appended to a log, and what you see is that log replayed. So the past is always there to ask.

- **`thc log`:** recent changes grouped by transaction, with who and how. `--by claude --since 1d` narrows it.

```sh
$ thc log --by claude --since 1d
2026-10-05 09:14  agent:claude  via cli  tx ME2184
    node.set       7gthv  props={"status":"doing","owner":"claude"}
```

- **`thc history <id>`:** every event that touched one note.
- **`thc diff <id> --since 1d`:** field-level changes since a time or a transaction, with who made each.

```sh
$ thc diff 7gthv --since 1d
7gthv  [x] Export drops nested tags · created 09:02 by you

  status     todo → done                  (2 edits)  ◆ claude · 09:31 · 7KSQ9X
  owner      — → claude                   ◆ claude · 09:14 · ME2184
  children   +2
```

- **`thc undo`** undoes the last transaction, as a new change: nothing is erased, and `thc undo` again redoes it. `thc undo --tx <id>` picks one; `--by claude --since 2h` reverts an agent's run after a preview.
- **`thc rewind <id> --to <time|tx>`** sets one note back to how it was. `<tx>^` means just before that transaction.
- **`--as-of=-2d`** on a query reads the vault as it was: what was open last Tuesday?
- **`thc rm` is reversible** with `thc restore`.
- **In the app,** the Log (<kbd>7</kbd>) lists every change; <kbd>u</kbd> undoes the selected one, <kbd>@</kbd> cycles who, <kbd>R</kbd> rewinds. <kbd>L</kbd> on any row shows its history.
- **`thc doctor`** checks the vault and the local store. `--fix` previews repairs (a day or tag made twice on two devices); `--fix --yes` applies them as one undoable transaction.
- **`thc rebuild`** replays the whole log into a fresh local store.
- **Plain formats out:** `thc export` writes a Markdown export Obsidian can read, and `thc export --json` prints a snapshot. The log itself is JSONL.
