# thought-central

**`thc`: thought control.** A CLI-first, local-first stash for notes, todos, dates and
reminders, built for one human and their agents.

```console
$ thc add "Call dentist due:fri #health !high"
added n7n8h   [ ] Call dentist #health  due fri · !high · in today's journal

$ thc remind "Pay rent" --at "nov 1 9am" --repeat "every month on the 1st"
reminder zx02k   [ ] Pay rent  Nov 1 09:00 (in 29d) · ↻ every month on the 1st

$ thc today
Today · Saturday, October 3
Today
  kparq   [ ] Water plants  today · ↻ every 3 days · in today's journal
Journal: 6 entries today

$ thc q 'status:open (due<=+7d or priority:high) -#someday sort:due' --json
{"count":2,"items":[{"id":"yt2nrdwwrbpb","short":"yt2nr","kind":"task", …}]}
```

**New here?** Start with [the guide](docs/guide/README.md): your first ten minutes, every view,
how writing works, and every key.

## Why

Notion, Logseq and Obsidian are GUI-first. The app ends up deciding how data is created,
queried and shaped. `thc` turns that around:

- **The terminal is the interface.** It has a full CLI, and agents use exactly the same
  commands. The TUI (`thc` with no arguments) is a thin client over the same core.
- **Nothing is overwritten.** The source of truth is an append-only event log, so you get
  history, undo and as-of queries for free. A local SQLite store makes reads take a few
  milliseconds.
- **Local-first sync with no conflicted copies.** Put the vault in Dropbox, iCloud or
  Syncthing. Each device only appends to its own files.
- **No lock-in.** The log is plain JSONL, and `thc export` writes Obsidian-readable
  Markdown.

Read [PHILOSOPHY.md](PHILOSOPHY.md) for the principles and [docs/SPEC.md](docs/SPEC.md) for
the design.

## Install (from source)

```bash
mise install            # or have a recent stable Rust toolchain
cargo install --path crates/thc
```

## Quick start

```bash
thc init                          # creates ./vault and ./.thc.toml (or set THC_VAULT)
thc add "first thought"           # goes to today's journal
thc todo "ship it" --due fri -p high
thc                               # opens the TUI (press ? for keys)
thc journal                       # today's entries
thc edit                          # edit today's journal in $EDITOR, round-tripped by ID
```

## What's in the box

| Area | Commands |
|---|---|
| Capture | `add` (today's journal by default; `--inbox`, `--under`, `--journal`), `todo`, `remind` |
| Change | `done`, `reopen`, `skip`, `set k=v`, `text`, `tag`, `mv`, `rm`, `restore`, `edit` |
| Views | `today`, `agenda`, `journal`, `inbox`, `pages`, `show`, `search` |
| Query | `q '<query>'` with `--as-of` time travel |
| Reminders | `alert add/ls/ack/snooze/rm` |
| History | `history <id>`, `log --by`, `undo` |
| Sync & safety | `conflict ls/resolve`, `doctor`, `rebuild`, `export`, `ingest` (from `drop/`) |

Everything takes `--json`, `--dry-run` and `--actor`.

## Data model in one paragraph

Everything is a **node**: pages, journal days, tasks, events and tags.
- **Structure:** a node has a parent, a fractional order key, text, and typed properties.
  `status` makes it a task. `scheduled` is when you plan to do it, `due` is the hard
  deadline. `repeat` is an RRULE subset plus an Org-style mode.
- **Links and reminders:** **edges** link nodes (`tag`, `mention`, `blocks`…), and
  **alerts** are reminders attached to nodes.
- **Storage:** every change is an event in `vault/log/<device>/<month>.jsonl`.

## For agents

See [AGENTS.md](AGENTS.md). In short: set `THC_ACTOR`, use `--json`, refer to nodes by ID,
check exit codes, and never touch the vault files directly.

## Status

| Milestone | State |
|---|---|
| M0: event format, core, store, convergence tests | done |
| M1: CLI | done |
| M2: daemon (`thc daemon …`: watch, socket, alerts, export) | done |
| M3: TUI (`thc` / `thc tui`) | done, built to its design spec |

macOS and Linux. MIT licensed.
