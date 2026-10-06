# Starting a team

Run `thc team up pm designer engineer --agent claude --agent engineer=codex` in a project
with a board. Each teammate starts with its role, actor name and board, then reads
`thc prime --role <role>`. The capture vault can stay separate from that board.

For a team you start repeatedly, put the roster in the project's `.thc.toml`:

```toml
vault = "my-project"
board = "my-project:¶ Issues"

[[team.roster]]
role = "pm"
agent = "claude"
model = "claude-opus-5-5"

[[team.roster]]
role = "designer"
agent = "claude"
model = "claude-opus-5-5"

[[team.roster]]
role = "engineer"
agent = "claude"
model = "claude-opus-5-5"

[[team.roster]]
role = "merger"
agent = "claude"
model = "claude-opus-5-5"

[[team.roster]]
role = "reviewer"
agent = "claude"
model = "claude-haiku-4-5-20251001"
permission_mode = "manual"

[[team.roster]]
role = "engineer"
agent = "codex"
model = "gpt-6.1-sol"
count = 2

[team.layout]
columns = 3
pm = "bottom-right"
tab = "team"
```

The seven-person example also needs this in **your device config**,
`~/.config/thought/config.toml`:

```toml
[team]
max_agents = 7
```

`count` defaults to one and must be positive. Entries keep their listed order in `team ls`.
Two Codex engineers become `codex-engineer` and `codex-engineer-2`. The maximum team size
defaults to six; project roster entries cannot raise your device's cap.

Preview with `thc team up --dry-run --json`, then run `thc team up` to start the roster.
Herdr receives each model through native arguments, and each opening briefing separately.
With WezTerm, the launcher starts in a new tab. Without either host, thc prints shell commands.

Profiles default to three balanced columns in a new tab named `team`, with the first PM or
lead at the bottom right. A smaller roster uses fewer columns. `[team.layout]` can change the
column count or tab name; omit its `pm` field to keep the roster's visual order. Columns have
equal widths and rows divide each column evenly, subject to terminal cell rounding.

`--agent claude` overrides every entry's agent; `--agent engineer=codex` overrides that role.
`--model M` overrides every entry's model. Supplying roles, as in `thc team up pm engineer`,
replaces the configured roster and keeps the earlier lead-left layout unless `[team.layout]`
is present. Configured permission modes must match the chosen agent.

Claude's `permission_mode` is an explicit human choice, like `team add --permission-mode`.
Only a person can start a profile that sets it; agent callers must use a profile without
permission modes. Omission keeps the agent's own defaults. New folders may still show the
agent's repository trust dialog: answer that yourself. thc preserves these native decisions.
If startup stops at a dialog, `thc team ls` keeps the created pane available to inspect, and
`thc team down` can close it before you retry.

`thc team ls --json` shows saved teammates, models, current claims and unread messages.
`thc team down` closes only saved team panes and keeps the board and claims. To change the
roster, stop its existing panes first. `team add` and `team rm` remain available for individual
changes; those operations require a person or an explicitly enabled lead.

## Work times and tokens

`thc status --since today` shows completed work, work in flight, blockers and time spent.
Use `--by codex-engineer` to focus on one owner, or `--json` for structured reports.

Record a real `session_id` when claiming a task. While the daemon runs, it reads usage
numbers from Claude Code and Codex session logs about once a minute. It never calls agents.
Task and owner totals include both completed work and tokens already spent in flight;
coverage says how many tasks have known usage. Missing logs remain unknown.

Minute samples stay in the local derived cache, which status and TUI reports can read.
They also arrive through the daemon's `tokens` RPC and subscription topic, with a collection
time on each snapshot. Usage props are written as `collector` only at hourly checkpoints
and completion, so the append-only log stays small. The collector respects device policy.
`thc done` also attempts final collection when the daemon is offline.

`thc status --collect` is the manual path for collecting or reconciling usage, including
in-flight tasks. Final props can be reviewed and undone like other agent writes. The
collector uses the task's tracked start/completion window and session IDs on its claim notes;
work without a claim cannot be attributed automatically.
