# Working with agents

`thc` is built for one person **and their agents**. Claude Code, Codex or your own scripts use
the same `thc` commands you do, and you stay in charge.

## Set up an agent

```console
$ thc setup claude --user      # or: thc setup codex --user
```

This teaches the agent `thc`: what the commands are, how to refer to notes, and the house rules.
In a session, the agent starts with `thc prime`, a short briefing of what's due and the rules.

## What agents can and can't do

- **They can:** read your notes (`--json`), add notes and tasks, set dates, move things, and set
  reminders when you ask.
- **Every change is signed** with the agent's name: `◆ claude` in lists and the Log.
- **They can't accept their own work.** Only you review.
- **They never resolve conflicts.** If two versions of a note exist, they tell you.
- **Policy** (`~/.config/thought/policy.toml`) can make an agent read-only, or require your
  confirmation for big changes.

## Review what they did

Open the Log (`7`) and press `r`. The review lane lists every agent change you haven't seen yet:

| Key | Does |
|---|---|
| `a` | Accept (it's marked reviewed) |
| `u` | Undo it (a new change: nothing is erased) |
| `A` | Accept everything shown |

[render: the review lane]

From the command line: `thc review`, `thc log --by claude --since 1d`, and
`thc undo --tx <id>`.

## Working side by side

**Showing you something.** Ask an agent to put a page, a day or a list beside you and it runs
`thc ui aside "Reading List"`: the page opens at the top of your sidebar, marked `◆ claude`, and
the bar says so. Your keyboard stays where it is. `⌘[` takes it back. Agents can't move your
focus, and they can't close or unpin a panel you pinned.

If an agent changes a note you're writing in, `thc` tells you at once
(`◆ claude changed this line`). It never rewrites the text under your cursor. When you move off,
the change applies. If you both changed it, both versions are kept for you to choose.
