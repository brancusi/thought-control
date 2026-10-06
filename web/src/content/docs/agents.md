---
title: Agents
description: Claude Code, Codex and your own scripts use the same CLI you do, every change is signed, and only you accept their work.
order: 6
group: Guide
---

`thc` is built for one person **and their agents**. Claude Code, Codex or a shell script use the
same `thc` commands you do. There is no separate API, no plugin and no server: the CLI is the
API, the way `git` and `gh` are. You stay in charge, because every change an agent makes is
signed, reviewable and one key from undone.

## Set up an agent

```sh
$ thc setup claude --user      # Claude Code, for every project
$ thc setup codex --user       # Codex
$ thc setup claude             # just this repo: a marked block in its AGENTS.md
$ thc setup claude --user --hook   # also run `thc prime` at the start of each session
```

This installs a skill (or an `AGENTS.md` block) generated from the binary itself: the commands,
the capture and query syntax, the id rules, the exit codes and the house rules. It never drifts
from the version you run: `thc update` refreshes it, and `thc setup claude --user --check` exits
`1` if it's stale. The same text is built in: `thc instructions all`, or one topic at a time
(`rules`, `capture`, `query`, `dates`, `conflicts`, `exit-codes`, `json`, `commands`, `apply`).

## A session, start to finish

An agent starts by saying who it is, then asking for a briefing:

```sh
$ export THC_ACTOR=claude
$ thc prime
```

`thc prime` is about fifteen lines: the version, who it's acting as, which vault and why, whether
the background service is live, then heads-up lines (alerts that fired, conflicts to tell you
about, agent changes waiting for your review, a context you have on), what it last did, and the
rules. Then it reads, as JSON, only the fields it needs:

```sh
$ thc today --json --fields id,short,title,due,rev
$ thc q 'status:open #offsite due<=+7d sort:due' --json
$ thc show k7q2m --depth 1 --json
```

It writes with ids, never titles, and with a key of its own so a retry can't make a duplicate:

```sh
$ thc todo "Book the venue due:fri #offsite !high" --key offsite-venue --json
$ thc set k7q2m due=+3d priority=low --json
$ thc done k7q2m --expect status=todo --json
```

And you see it all, signed `◆ claude`, in Today, the Log and the review lane.

## A team of agents

Give a project a few agents with different jobs, and thc is their shared board. Each one picks
work from the same page, in the order you put it there, and nobody keeps a private to-do list.

```sh
$ thc team up pm engineer designer --agent claude
board ready · acme-site · ¶ Issues · ¶ About from README.md
team up · board acme-site · ¶ Issues · pm (claude), engineer (claude), designer (claude)
```

- **One agent per role, in new panes:** in herdr first, then WezTerm. Anywhere else, `team up`
  prints the command to start each one, for you to run. Mix agents with
  `--agent engineer=codex`. Names follow the role: `claude-pm`, `claude-engineer`.
- **A new project gets a board** the first time: a vault named after the folder, a `¶ Issues`
  page, a `¶ About` page started from your README, and a `.thc.toml` so every agent finds it. In
  a terminal it asks first; `--new-board <name>` is the explicit yes.
- **The lead plans first.** It turns the README into ordered tasks, each with a role, and
  finishes a task called *First plan*. Until you accept that in the review lane, everyone else is
  told `waiting for the lead's first plan`.
- **Agents never skip your permission prompts.** thc won't start an agent with a flag that
  bypasses them.
- **`thc team ls`** shows who's there and what each is on; **`thc team down`** closes only the
  panes `team up` opened.

Each agent starts with a briefing for its role: what the role does and never does, what's next
for it, its teammates and its messages.

```sh
$ thc prime --role engineer
thc 0.9.58 · you are claude-engineer (role: engineer) on the board acme-site · ¶ Issues

next for engineer
  waiting for the lead's first plan
team: claude-pm · claude-engineer · claude-designer

messages for engineer (0 unread)
```

The work loop is the one above, scoped to the role:

```sh
$ thc next --role engineer                     # the top ready task routed to engineers
$ thc set k7q2m status=doing owner=claude-engineer --expect status=todo
$ thc msg designer "Which spacing scale for the cards?" --on k7q2m
$ thc msgs --unread                            # what's waiting for you
$ thc watch --for engineer --once              # wait for the next message or task
```

`thc watch` waits without polling while the background service runs, and keeps going (checking
every second) while it restarts. Tasks are routed with a `role` field (`thc set <id>
role=engineer`); a task with no role is anyone's.

## The house rules

`thc setup` teaches every agent these eleven rules. They are also why you can let one loose:

1. **Identify yourself:** set `THC_ACTOR` to your actual agent name (e.g. `codex` or
   `claude`), never another model's name. Use a stable role suffix if useful (e.g.
   `codex-reviewer`). Record an available session/thread ID separately as a `session_id`
   property on notes/tasks, with `agent_role` when useful; never invent an ID or store
   credentials. Every write is attributed, so the human can review it (`thc review`) and
   revert it. Only a person can accept your changes; never run `thc review accept`.
2. **Pass `--json`** (or `--fields a,b,c`) to parse output. Never scrape the human format.
3. **Never edit files under the vault.** The log is append-only and machine-written. Go through
   the CLI, which validates every input.
4. **Refer to notes by id**, not title. Any unique prefix of 4+ characters works; JSON always
   carries the full `id` and a display `short`.
5. **Check exit codes** (below).
6. **Make retries safe** with `--key <your-key>` (or `--id <12 chars>`) on `add`, `todo` and
   `remind`. The same key maps to the same note on every device, so a repeat is a no-op that
   returns the existing note.
7. **Preview risky changes** with `--dry-run`: it prints the ops and writes nothing.
8. **Keep output small:** `--fields`, `--limit`, `thc show <id> --depth 1`, targeted queries.
9. **Reminders an agent sets are labelled** "set by claude". Agents set them only when asked.
10. **Conflicts are the human's call.** Mention them, don't resolve them.
11. **Guard writes on what you read.** Every note's JSON has a `rev`. Pass `--if-match <rev>`
    (or `--expect status=todo`) and the write only happens if nothing changed since. Exit `4`
    with `kind: stale` means: re-read, then decide.

## Guarded writes

Rule 11 is what makes an agent safe to run beside you. You might tick a task off while the agent
is still thinking about it:

```sh
$ rev=$(thc show k7q2m --json | jq -r .rev)
$ # … the agent thinks; meanwhile you tick the task off …
$ thc done k7q2m --if-match "$rev" --json
```

```json
{"error":{"kind":"stale","message":"…","node":"k7q2m…","rev":"…","changed":[…]}}
```

Exit code `4`. Nothing was written. The error names the current `rev` and which fields
changed, so the agent re-reads and decides again, instead of writing over you.

## Many changes, one transaction

`thc apply` takes one JSON op per line and applies them all, or none:

```jsonl
{"cmd":"add","text":"Offsite","inbox":true,"as":"off"}
{"cmd":"todo","text":"Book the venue due:fri #offsite !high","under":"$off","key":"offsite-venue"}
{"cmd":"todo","text":"Collect dietary needs","under":"$off","key":"offsite-diet"}
{"cmd":"set","id":"pab27","props":{"priority":"med"}}
{"cmd":"done","id":"65p83"}
```

```sh
$ thc apply plan.jsonl --dry-run   # what would happen
$ thc apply plan.jsonl --json      # do it
$ thc apply - < plan.jsonl         # or from stdin
```

- If any line is bad, exit `6` lists every bad line and nothing is written.
- `"as":"name"` on a create, then `"$name"` in any id field later.
- The commands: `add` `todo` `remind` `set` `text` `tag` `done` `reopen` `skip` `rm`
  `restore` `mv` `link` `unlink` `alert`. `text` takes capture syntax, as on the CLI.
- It's one transaction: one review item for you, one `thc undo --tx` to take it back.

For an outline, `thc import - --page "Offsite"` reads `- item` lines (indent to nest; capture
syntax works) as one transaction.

## Files and screenshots

An agent can attach logs or screenshots to a note, several at once, signed like any write:

```sh
$ thc attach k7q2m ./repro.png ./server.log --caption "after the fix"
```

`thc show <id> --json` lists a note's attachments with their full path, so an agent can read
the image you pasted.

## Review what they did

Open the Log (<kbd>7</kbd>) and press <kbd>r</kbd>. The review lane lists every agent change you
haven't accepted yet, oldest first:

| Key | Does |
|---|---|
| <kbd>a</kbd> | Accept (it's marked reviewed) |
| <kbd>u</kbd> | Undo it: a new change, nothing is erased |
| <kbd>A</kbd> | Accept everything shown |

When an agent writes while you're in thc, a toast says so: <kbd>L</kbd> reviews it,
<kbd>u</kbd> undoes it.

From the command line:

```sh
$ thc review                          # what's waiting, oldest first
$ thc review --by claude --since 1d
$ thc review show 1                   # one entry with its raw ops
$ thc review accept 1 2               # yours to run, never the agent's
$ thc review revert 3                 # keeps fields you changed afterwards (unless --force)
$ thc log --by claude --since 1d      # everything it did
$ thc undo --tx <id>                  # undo one transaction
$ thc undo --by claude --since 2h     # undo all of it: a preview, then a confirmation
```

**Work an agent finished waits for you.** `is:to-review` finds what an agent marked done that
you haven't accepted yet. Once you accept it, it's simply done. Put it in a view:

```sh
$ thc view set issues --section "To review" 'is:to-review' --section Open 'status:open'
```

Undo is never destructive. It writes new events that put things back, so the history keeps both
the change and its reversal. `thc diff <id> --since 1d` shows a note's field-level changes, and
`thc rewind <id> --to <time|tx>` sets it back to how it was.

## Working side by side

If an agent changes the note you're writing in, `thc` tells you at once
(`◆ claude changed this line`). It never rewrites the text under your cursor. When you move off,
the change applies. If you both changed it, both versions are kept, and you choose:
`keep yours` or `keep claude's`.

Agents never resolve conflicts. They can find them (`thc conflict ls --json`,
`thc q 'is:conflict'`), and they tell you. `thc text` on a note with an open text conflict
exits `4` and changes nothing.

## Limits you set

In `~/.config/thought/config.toml`, give each agent a tier:

```toml
[actors.codex]
tier = "read"          # reads only: every write is refused

[actors.claude]
tier = "write"         # the default for agents
confirm = ["rewind"]   # needs your yes first
```

| Tier | Can |
|---|---|
| `read` | Every read, plus `prime`, `instructions`, `schema` and `--dry-run` of anything |
| `write` | Add, edit, complete, move, link, tag, alerts, `apply`, views, and undo its own work. Not delete |
| `full` | Everything. The default for you |

A refusal says what was refused and why, and writes nothing. `--readonly` (or
`THC_READONLY=1`) refuses every write in one process, whoever is acting.

This is a guard against mistakes, not a security boundary: anything that can run `thc` could
also edit the config. It stops a well-meaning agent from doing what it shouldn't, loudly.

## Exit codes

| Code | Means | What to do |
|---|---|---|
| `0` | OK | |
| `1` | Error | Read the message |
| `2` | Usage | Fix the command line |
| `3` | Not found | The id or page doesn't exist (also `thc daemon status` when it isn't running) |
| `4` | Conflict or stale | An open sync conflict, or `--if-match` / `--expect` failed. Re-read, then decide |
| `5` | Ambiguous id | The prefix matches several notes: the JSON error lists `candidates` |
| `6` | Validation | Bad input, such as `due:fryday` or `status:opn`. The message suggests a fix |

With `--json`, errors go to stderr as `{"error":{"kind","message","candidates"?}}`. A stale
error adds `node`, `rev` and `changed[]`.

## The JSON contract

- **Additive only.** Within a major version, fields in `--json` output are only ever added:
  never removed, renamed or retyped. Ignore what you don't know. A missing optional field means
  "not set".
- **A note** is `{id, short, rev, kind, parent, title, text, status, scheduled, due, priority,
  repeat, done_at, journal, tags[], props{}, created_by, created, updated, vault}`. Rows in
  `today` and `agenda` add `reasons[]`: `overdue`, `due-today`, `scheduled`, `alert`,
  `alert-fired`, `repeating`, `doing`, `done-today`.
- **A write** returns `{ok, tx, events, nodes[]}`.
- **Schemas:** `thc schema <cmd>` for one command's input and output, `thc schema --all` for
  code generation.
- **Contexts don't leak.** A context you turned on filters your listings, never an agent's
  `--json` reads or its writes, unless it passes `--context`.

The whole command list is in [Commands](../commands/), and the query grammar in
[Query language](../query-language/).
