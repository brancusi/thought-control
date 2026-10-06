---
title: Agents
blurb: The CLI is the API. Every write signed, every change reviewable.
icon: agent
order: 6
---

Claude Code, Codex and your own scripts use the same `thc` you do. You stay in charge.

```sh
$ thc setup claude --user      # or: thc setup codex --user
$ export THC_ACTOR=claude
$ thc prime                    # a short briefing: what's due, then the rules
```

- **Teach an agent once.** `thc setup` installs a skill (or an `AGENTS.md` block) generated from the binary, so it always matches the version you run. `thc update` refreshes it. `thc instructions [topic]` prints the same guide.
- **Every write is signed.** `THC_ACTOR=claude` puts `◆ claude` on everything it touches, in lists, the Log and history.
- **`--json` everywhere,** and `--fields id,short,due` to keep it small. Fields are only ever added within a major version.
- **`thc schema <cmd>`** gives any command's JSON Schema; `--all` for code generation.

| Exit | Means |
|---|---|
| `0` | ok |
| `1` | error |
| `2` | usage |
| `3` | not found |
| `4` | conflict, or a guarded write found the node changed |
| `5` | ambiguous id (the error lists `candidates`) |
| `6` | validation |

- **Retry-safe:** `--key <your-key>` on `add`, `todo` and `remind` maps to the same note on every device. Run it twice, get one note.
- **Guarded writes:** every node carries a `rev`. `--if-match <rev>` or `--expect status=todo` writes only if nothing changed; otherwise exit `4`, `kind: stale`, with what changed.
- **`--dry-run`** prints the ops a write would make, and writes nothing.
- **`thc apply plan.jsonl`:** one JSON op per line, all applied as one transaction or none. `"as":"name"` on a create, then `"$name"` later. One undo, one review item.
- **Refer by id:** any unique prefix of 4+ characters. An ambiguous one exits `5` and lists the candidates.
- **Reminders an agent sets are labelled** "set by claude".
- **Policy:** in `~/.config/thought/config.toml`, `[actors.<name>] tier = "read"` makes an agent read-only, and `confirm = [...]` needs your yes for the verbs you list.
- **Agents never resolve conflicts,** and `thc text` refuses to write over one.

**A team, on one board.** `thc team up pm engineer designer --agent claude` starts one agent per role in new panes (herdr, then WezTerm; anywhere else it prints the commands). Each is briefed with `thc prime --role`, picks its work with `thc next --role`, leaves messages with `thc msg designer "…" --on <task>`, and waits with `thc watch --for engineer`. The lead's first plan waits for your OK before anyone else starts. `thc team ls` shows who's on what; `thc team down` closes only the panes it opened. [How it works](../docs/agents/#a-team-of-agents).

**The review lane.** Open the Log (<kbd>7</kbd>) and press <kbd>r</kbd>: every agent change you haven't seen, oldest first. <kbd>a</kbd> accept, <kbd>u</kbd> undo, <kbd>A</kbd> accept all. Only a person can accept.

```sh
$ thc review
Review · 3 changes by claude · oldest first

1  ME2184  ◆ claude · 09:14 · this device
   ~ 7gthv  status    todo → doing
            owner     — → claude
...
$ thc undo --by claude --since 2h     # revert an agent's afternoon: a preview, then a yes
```

When an agent changes a line you're writing in, a toast says so; <kbd>L</kbd> reviews it, <kbd>u</kbd> undoes it.
