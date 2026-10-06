---
title: Issues
blurb: A task that opens as a document. A board in sections. Claims agents can't race.
icon: board
order: 14
---

No new kind of thing: an **issue is a task**, and the notes under it are its body. That's enough for a small tracker inside a project vault.

- **It opens as a document.** <kbd>Enter</kbd> on a task with notes under it, or <kbd>⌃O</kbd> on its line, opens it with the task line as the header (its box, owner, priority, when it was opened) and its notes below. Write there to add findings, steps, screenshots. Change its status from the lists (<kbd>x</kbd>, or <kbd>⌃T</kbd> on its line while you write). <kbd>Esc</kbd> goes back where you came from.
- **Captures land on the board.** In the vault's `settings.toml`:

```toml
[capture]
target = "¶ Issues"
```

  Now `thc todo "Export drops nested tags" -p high` lands on `¶ Issues`, not the journal.
- **An owner is a field.** `thc set 7gthv owner=claude`; it shows as `◆ claude` in the meta, and `owner=claude` works in queries.
- **The board is a sectioned view:**

```sh
$ thc view set issues \
    --section Todo        'under:"¶ Issues" status:todo sort:priority' \
    --section Doing       'under:"¶ Issues" status:doing' \
    --section Waiting     'under:"¶ Issues" status:waiting' \
    --section "To review" 'under:"¶ Issues" is:to-review'
$ thc q @issues
```

- **To review is derived, not a status.** `is:to-review` finds what an agent marked done that you haven't accepted. Accept it in the review lane and it's simply done; if the agent reopens and finishes it again, it comes back.

**How an agent works an issue**

```sh
$ thc q 'under:"¶ Issues" is:ready sort:priority' --json        # find work
$ thc set 7gthv status=doing owner=claude --expect status=todo   # claim it
updated 7gthv   [/] Export drops nested tags  !high · in Issues
$ thc add --plain --under 7gthv "Reproduced: tags under a heading are dropped."
$ thc done 7gthv
```

- **Claims are atomic.** If a second agent tries the same claim, `--expect` fails and nothing is written:

```sh
$ thc set 7gthv status=doing owner=codex --expect status=todo --json
{"error":{"kind":"stale","message":"expected status=todo, found doing · nothing written", …}}
$ echo $?
4
```

- **Findings are ordinary notes,** signed `◆ claude`. `--plain` keeps quoted syntax and `#words` as written.
- **You close the loop** in the review lane (<kbd>7</kbd>, <kbd>r</kbd>): <kbd>a</kbd> accepts, <kbd>u</kbd> undoes.
