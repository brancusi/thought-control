# Philosophy

thought-central (`thc`) is a place to put things down: a thought, a todo, a date, a reminder.
It's built for one person, working with a few agents, from a terminal.

These are the ideas behind it. When a design question comes up, start here.

---

## 1. The terminal is the interface

There's no GUI editor, and there won't be one. The CLI is the real interface. The TUI is a
comfortable way to drive it, and the menu bar app is a window onto it. Anything you can do
anywhere, you can do with `thc` and a few words.

GUIs tend to take over. They grow features only the GUI can reach, and the data bends to fit
the UI. We avoid that by making the CLI the only place behavior lives.

## 2. Agents are first-class users

Humans and agents use the same commands. Agents get what they need to be reliable:

- **Stable, short, random IDs.** Sequential numbers collide when work happens in parallel,
  and names drift. A random ID never collides and never needs coordinating.
- **Typed, validated writes.** Agents never hand-edit a file format. They call a command,
  and the command rejects anything malformed (exit code 6), so bad data never gets in.
- **`--json` everywhere, stable exit codes, `--dry-run`, and an `--id` option for idempotent
  creates.**
- **Provenance.** Every change records who made it (`THC_ACTOR=claude`). `thc log --actor claude`
  shows what an agent did, and `thc undo` takes it back.

## 3. Nothing is ever overwritten

The source of truth is an **append-only event log**: one JSON line per change. Nothing
mutates in place, so:

- history, undo and "what did this look like last Tuesday?" come for free
- a mistake, human or agent, is always recoverable
- the local database is disposable. Delete it and `thc rebuild` replays the log.

## 4. Local-first, sync-agnostic, and no conflicted copies

No server and no account. The vault is a folder you can sync with Dropbox, iCloud or Syncthing.
Each device writes **only its own log files**, so a sync tool never sees two writers on one
file. Devices merge deterministically: the newest change wins per field, and if two devices
edit the same text, both versions are kept and flagged rather than one being lost silently.

## 5. One primitive

Everything is a **node**. A page, a journal day, a todo, a reminder and a tag are all nodes
used in a particular way. "Task" isn't a type, just a `status` that any node can have.
"Reminder" isn't a type, just an alert attached to a node. Fewer concepts means fewer edge
cases, for humans and agents alike.

The fields are the ones every serious tool converged on:

- `scheduled`, when you plan to do something, separate from `due`, when it must be done
- alerts as separate objects
- recurrence that advances the same item and keeps a log of each occurrence

## 6. Text at the edges, structure in the core

Humans type `Call dentist due:fri #health !high`. That inline syntax is parsed **once**, at
capture time, into typed fields. Stored text is never re-parsed to find out what something
is. You still get plain text where it helps:

- quick capture
- `thc edit` (a round-trip through `$EDITOR`)
- a `drop/` folder for your phone
- a read-only Markdown export that Obsidian can open

## 7. No lock-in

The log is plain JSONL you can `grep`, `jq`, or rebuild from in fifty lines of any language.
The Markdown export is regenerated continuously. If `thc` vanished tomorrow, your notes
would still be readable and complete.

## 8. Fast enough to be invisible

A query should take about as long as the OS needs to start a process (a few milliseconds),
so agents can call it in loops and humans never wait. That's why it's written in Rust, uses
SQLite, and has no runtime to start on the read path. The budgets are written down in
[docs/SPEC.md §8](docs/SPEC.md) and measured.

## 9. Small and boring

Prefer the boring, proven tool: SQLite over the newer database, a JSONL log over a CRDT
framework, a single binary over a fleet of services. Add complexity only when it removes
more than it adds.
