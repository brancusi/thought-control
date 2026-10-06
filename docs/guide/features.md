# Everything it can do

The complete list, grouped. Each item links to where it's explained.

## Writing

- Plain-text pages and journal days. Paragraphs, lists, numbered lists, tasks, headings, quotes,
  code blocks and rules, all as Markdown. ([Writing](writing.md))
- Inline tokens for due and scheduled dates, events, repeats, priority, tags and links. They're
  previewed while you type, then folded into a quiet column.
- `⌃T`: text → task → done → text.
- `[[` links, with suggestions. A new name creates the page, and a near-miss name gets a "did you
  mean".
- Paste Markdown from anywhere, and it becomes notes. Copy gives clean Markdown back.
- Automatic saving and undo across saves.
- Live changes from agents and other devices, without disturbing your cursor. Conflicts are
  kept, never lost.
- Focus mode with choosable elements (`bare`, `writer`, `planner`).
- Emoji, accents and CJK handled as whole characters.
- Mouse: click to place the cursor, select, toggle tasks, jump days, click any key in the footer.
  ([mouse](mouse.md))

## Organising

- Today: overdue, today, doing, next 7 days, done today, from everywhere.
- Inbox triage: move, task, date, done, delete.
- Tasks: a filter language (`status:open #work due<=+7d sort:due`), saved views and a table view.
- Pages and a daily journal. Pages can be nested and linked, and backlinks show "linked from".
- Tags, priorities, scheduled vs due dates, and repeats (fixed or from completion, with skip).
- Reminders and alerts (`--before 1d`, `--at "fri 9am"`), with snooze and acknowledge.
- Contexts: a saved view applied to everything you see, always visible.
- Search across pages, tags and notes.

## History and safety

- An append-only log: nothing is ever overwritten.
- Undo any change, rewind a note to a point in time, and diff a note since yesterday.
- As-of queries: "what did this look like last Tuesday?"
- `thc doctor`: checks the vault, and fixes duplicates in one undoable step.

## Agents

- The same CLI for people and agents, with `--json` everywhere and stable exit codes.
- Every change signed with who made it, and a review lane to accept or undo.
- Retry-safe writes (`--key`), guarded writes (`--if-match`) and batch writes (`thc apply`).
- Policy tiers per agent: read-only, write, or confirm.
- `thc prime` / `thc brief`: a session briefing for agents.

## Local-first

- Your notes are a folder (`~/thought`). Sync it with iCloud, Dropbox or Syncthing, with no
  conflicted copies.
- A background service for reminders and live updates, started at login.
- Markdown export that Obsidian can read. The log is plain JSONL.
- Self-updating (`thc update` / `:update`), signed and verified.

## The terminal

- Works in any terminal: WezTerm, kitty, Ghostty, iTerm2, Terminal.app, tmux, over SSH.
- Ember dark and light themes, plus 256-colour and 16-colour fallbacks, and `NO_COLOR`.
- A footer that shows only the keys that work right now, and `F1` for every key.
