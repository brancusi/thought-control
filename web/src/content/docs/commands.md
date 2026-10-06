---
title: Commands
description: Every thc command, grouped, one line each, plus the global flags, the JSON contract and the exit codes.
order: 22
group: Reference
---

One binary does everything. `thc` with no arguments opens the TUI; everything else is a
subcommand. `thc <command> --help` gives the details of any of them, and `thc schema <command>`
gives its JSON Schema.

Ids can be any unique prefix of 4+ characters (`k7q2`), and from another vault, `acme/k7q2m`.

## Write

| Command | Does |
|---|---|
| `thc` / `thc tui` | Open the TUI |
| `thc j [date]` | Write in a journal day: `thc j`, `thc j yesterday`, `thc j fri`. `--no-focus` for the full screen |
| `thc p <page>` | Write in a page, by title or id |
| `thc edit [id]` | Edit a note and its children in `$EDITOR` (default: today's journal) |

## Capture

| Command | Does |
|---|---|
| `thc add "…"` | Capture into today's journal, reading [tokens](../capture-syntax/). `--inbox`, `--under <id>`, `--journal <date>`, `--plain`, `--key <k>` |
| `thc todo "…"` | A task. `--due`, `--scheduled`, `-p` priority, `-t` tag, `--repeat "every 2w"` |
| `thc remind "…" --at "nov 1 9am"` | A task with a time and an alert at that time. `--repeat`, `--no-task` |
| `thc attach <id> <file>…` | Attach screenshots or files to a note, several at once. `--caption "…"`. Records each one's size, and `--json` returns the attachments' own ids |
| `thc shot` | Pick an area of the screen (macOS) and attach it to today's journal, or `--under <id>`. `--caption "…"` |
| `thc import <file>` | An outline of `- item` lines (indent to nest) as one transaction. `-` reads stdin. `--page`, `--under`, `--journal` |
| `thc parse "…"` | Preview what a capture would save. Writes nothing |
| `thc ingest` | Turn files in the vault's `drop/` folder into inbox items |

## Change

| Command | Does |
|---|---|
| `thc done <id>…` | Complete. A repeating task moves to its next date |
| `thc reopen <id>…` | Reopen |
| `thc skip <id>` | Skip this occurrence of a repeating task |
| `thc set <id> due=fri priority=high client=acme` | Set fields. An empty value unsets one |
| `thc text <id> "…"` | Replace the text |
| `thc tag <id> work +urgent -someday` | Add or remove tags |
| `thc mv <id> --under <id>` | Move. Or `--journal <date>`, `--root`, `--after <id>`, `--before <id>` |
| `thc link <a> <b> --rel blocks` | Link: a blocks b. Rels: `blocks`, `relates` (the default), or your own |
| `thc unlink <a> <b>` | Remove a link (`--rel` for one kind) |
| `thc rm <id>…` | Delete, with children. `thc restore` or `thc undo` brings it back |
| `thc restore <id>…` | Restore deleted notes |
| `thc apply <file>` | Many operations, one JSON per line, as one transaction: all or nothing. [More](../agents/#many-changes-one-transaction) |

## Read

| Command | Does |
|---|---|
| `thc today` | Overdue, due or scheduled today, and today's alerts, from every vault. `--why` says why each is there |
| `thc agenda` | The days ahead (`--days 7`) |
| `thc inbox` | Captures with no page or day |
| `thc journal [date]` | A journal day |
| `thc pages` | Your pages (`thc page ls`). `thc page new "Title"` makes one |
| `thc show [id]` | A note with its children, fields, backlinks, alerts and attachments. `--depth N` |
| `thc search "words"` | Full-text search |
| `thc q '<query>'` | A [query](../query-language/). `--explain`, `--as-of <when>` |
| `thc view ls` | Saved views, with how many match now, and the built-ins |
| `thc view add <name> '<query>'` | Save a view. `--tasks <1-9>` puts it on the Tasks row |
| `thc view set <name>` | Change one. `--section <title> '<query>'` (repeat), `--scope vault:*` |
| `thc view copy` / `reset` / `rm` | Duplicate a view, put a built-in back, remove one |
| `thc context [name]` | A view applied to your listings and captures on this device. `none` turns it off |

## Reminders

| Command | Does |
|---|---|
| `thc alert add <id> --before 1d` | Alert a day before it's due (`--anchor scheduled` for the scheduled date) |
| `thc alert add <id> --at "fri 9am"` | Alert at a time |
| `thc alert ls` | Upcoming alerts |
| `thc alert ack` / `snooze` / `rm` | Acknowledge, snooze or remove one |
| `thc alert preview` | What alerts would deliver at a time. Writes nothing |

## History and review

| Command | Does |
|---|---|
| `thc log` | Recent changes, grouped by transaction. `--by claude`, `--since 1d` |
| `thc history <id>` | Every change to one note |
| `thc diff <id> --since 1d` | Field-level changes since a time (or `--tx`, `--as-of`) |
| `thc undo` | Undo the last transaction, or `--tx <id>`. `--by claude --since 2h` reverts all of an agent's changes, after a preview |
| `thc rewind <id> --to <time\|tx>` | Set a note back to how it was then (`<tx>^`: just before it) |
| `thc review` | Agents' changes waiting for you. `show`, `accept` (a person only), `revert` |
| `thc conflict ls` | Notes with two versions waiting for you |
| `thc conflict resolve <id> --keep current\|other\|both` | Choose |

## Vaults and settings

| Command | Does |
|---|---|
| `thc vault` | Which vault you're in, and why |
| `thc vault ls` / `info` | Every vault with counts / one vault's path, sync and counts |
| `thc vault new <name> [path]` | Make a vault (default `~/thought-vaults/<name>`) |
| `thc vault add <folder>` | Join a vault someone shared |
| `thc vault use <name>` | Make it current on this device |
| `thc vault rename` / `rm` | Rename one / unregister one (nothing is deleted) |
| `thc init [path]` | A vault here, with a `.thc.toml` pointing at it. `--global` makes it your default |
| `thc config` | Every setting in effect and where it came from. `--why <key>` |
| `thc keys` | The keymap in effect. `--json`, `--markdown`, `--conflicts` |

## Agents

| Command | Does |
|---|---|
| `thc prime` | A session briefing for an agent: state, then the rules (about 15 lines) |
| `thc instructions [topic\|all]` | The agent guide built into the binary |
| `thc setup claude --user` | Teach Claude Code `thc` (or `codex`). `--hook` runs `thc prime` at session start, `--check` exits 1 if stale |
| `thc schema <command>` | JSON Schema of a command's input and output. `--all` for code generation |
| `thc next` | The next task to pick up: the first ready, unowned task on the board, in the page's order. `--role <role>`, `--mine`, `-n 3` |
| `thc prime --role <role>` | Brief an agent for its role: its card, its next tasks, its teammates and its unread messages |
| `thc msg <role\|actor> "…"` | Leave a message for a teammate. `--on <task>` puts it under a task. `thc msg read <id>…` (or `--all`) marks them read |
| `thc msgs` | Messages for you. `--for <role>`, `--unread` |
| `thc watch --for <role>` | Wait for messages and newly ready tasks, one line each as they arrive. `--once` returns after the first |
| `thc team up <roles>…` | Start one agent per role in new panes (herdr, then WezTerm; elsewhere it prints the commands). `--agent claude` or `--agent engineer=codex`, `--model` |
| `thc team ls` / `down` | Who's on the team and what they're on / close the panes `team up` opened |

## The machine

| Command | Does |
|---|---|
| `thc setup` | Set up this machine: vault, command, login item, agent skills. `--status`, `--undo` |
| `thc setup wezterm --yes` | Mac editing keys and ⌘V screenshots in WezTerm. `--undo --yes` takes them out |
| `thc update` | Update to the latest release, verified. `--check`, `--rollback` |
| `thc daemon status` | Is the background service running? `start`, `stop`, `restart`, `install`, `uninstall`, `run` |
| `thc doctor` | Check the vault and local store. `--fix` previews repairs, `--fix --yes` makes them as one undoable step |
| `thc rebuild` | Rebuild the local store by replaying the log |
| `thc export` | Write the Markdown export (`vault/export`). `--json` prints a snapshot |

## Global flags

These work on every command.

| Flag | Does |
|---|---|
| `--json` | Machine-readable output, and JSON errors on stderr |
| `--fields a,b,c` | Only these fields in JSON (implies `--json`). With no value, lists the fields |
| `--limit <n>` | At most n items in listings (default 50) |
| `--dry-run` | Print the operations a write would make, without writing |
| `--if-match <rev>` | Write only if the note hasn't changed since this `rev`; else exit 4 |
| `--expect field=value` | Write only if the field has this value (repeatable); else exit 4 |
| `--vault <name>` | Use this vault (also `THC_VAULT`) |
| `--actor <name>` | Who is acting: an agent's name (also `THC_ACTOR`) |
| `--context <name>` | Apply this context to this command; `none` turns one off |
| `--readonly` | Refuse every write in this process (also `THC_READONLY=1`) |
| `-y`, `--yes` | Confirm: skip prompts, and run a verb your policy marks `confirm` |

## The JSON contract

- **Additive only.** Within a major version, fields in `--json` output are only added, never
  removed, renamed or retyped. Ignore fields you don't know. Missing optional fields (nulls are
  left out) mean "not set".
- **A note:** `{id, short, rev, kind, parent, title, text, status, scheduled, due, priority,
  repeat, done_at, journal, tags[], props{}, created_by, created, updated, vault}`. Rows in
  `today` and `agenda` add `reasons[]` (`overdue`, `due-today`, `scheduled`, `alert`,
  `alert-fired`, `repeating`, `doing`, `done-today`).
- **A write:** `{ok, tx, events, nodes[]}`.
- **A listing** names its vault (`vault.name`) and any context in effect
  (`context: {name, applied, hidden?}`).
- **Errors** go to stderr: `{"error":{"kind","message","candidates"?}}`. A `stale` error adds
  `node`, `rev` and `changed[]`.
- **Schemas:** `thc schema <command>` for one, `thc schema --all` for every one.

## Exit codes

| Code | Means |
|---|---|
| `0` | OK |
| `1` | Error |
| `2` | Usage |
| `3` | Not found (also `thc daemon status` when the service isn't running) |
| `4` | Conflict or stale: an open sync conflict, or `--if-match` / `--expect` failed. Re-read, then decide |
| `5` | Ambiguous id: the JSON error lists `candidates` |
| `6` | Validation: fix the input. The message suggests how |
