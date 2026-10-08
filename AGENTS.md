# AGENTS.md

Guidance for AI agents. It has two parts: **using `thc`** to manage notes and todos, and
**working on this repo**. Part 1 is generated from the binary (`thc instructions all`) by
`thc setup claude`; edit `crates/thc/src/instructions.rs`, not the block.

---

<!-- thc:begin v0.10.1 -->
## Part 1: Using `thc` as an agent

`thc` is a CLI for a personal stash of notes, todos, dates and reminders. Treat it the way
you'd treat `gh` or `git`: the CLI is the API. Start a session with `thc prime`.

### Rules

1. **Identify yourself:** set `THC_ACTOR` to your actual agent name (e.g. `codex` or
   `claude`), never another model's name. Use a stable role suffix if useful (e.g.
   `codex-reviewer`). Record an available session/thread ID separately as a `session_id`
   property on notes/tasks, with `agent_role` when useful; never invent an ID or store
   credentials. Every write is attributed, so the human can review it (`thc review`) and
   revert it. Only a person can accept your changes; never run `thc review accept`.
2. **Pass `--json`** (or `--fields a,b,c`) when you will parse output. Never scrape human output.
3. **Never edit files under the vault.** The log is append-only and machine-written; go
   through the CLI, which validates input.
4. **Refer to nodes by ID**, not title. Any unique prefix of 4+ characters works; JSON always
   has the full `id` and a display `short`.
5. **Check exit codes** (`thc instructions exit-codes`).
6. **Make retries safe:** `--key <your-own-key>` (or `--id <12 chars>`) on `add`/`todo`/
   `remind`. The same key maps to the same node on every device, so a repeat is a no-op.
7. **Preview risky changes** with `--dry-run` (prints the ops, writes nothing).
8. **Keep output small:** `--fields id,short,due`, `--limit`, `thc show <id> --depth 1`,
   targeted `thc q` queries. `thc schema <cmd>` gives any command's JSON Schema.
9. **Reminders you set are labelled** "set by <your name>". Set them only when asked.
10. **Conflicts are the human's call** (`thc instructions conflicts`).
11. **Guard writes on what you read:** every node's JSON has `rev`; pass `--if-match <rev>`
    (or `--expect status=todo`) and the write only happens if nothing changed since. Exit `4`
    with `kind: stale` means re-read, then decide. **Claim every task before working on it**,
    even a quick fix, with one guarded write: `thc set <id> status=doing owner=<you>
    session_id=<real id> --expect status=todo`; start only on exit `0`. Exclusive on this
    device's vault only, not across synced devices.

### Core commands

```bash
thc prime [--role R]                    # session or team-role briefing
thc board --json                        # project work board and its source
thc role show engineer                  # built-in role card
thc team up pm designer engineer --agent claude   # herdr, WezTerm, or shell commands
thc team ls --json    thc team down --yes         # project roster; close its panes only
thc team add engineer --agent codex --task ID     # human or opted-in lead
thc team rm codex-engineer-2    thc team agents
thc next [--mine] [--role R] [--under ID] --json   # next ready task in outline order
thc status [--since today|3d|7d|all] [--by ACTOR] [--collect] --json   # where the board stands
thc today --json | thc agenda --days 7 --json
thc j [date]   thc p <page>             # people: write in a day / a page (the TUI editor)
thc add "text with due:fri #tag !high"  # → today's journal
thc todo "Draft plan" --due fri -t work -p high [--repeat "every 2w"] [--key k]
thc remind "Pay rent" --at "nov 1 9am" [--repeat "every month on the 1st"]
thc done <id>…    thc skip <id>    thc set <id> due=+3d priority=low client=acme
thc text <id> "…"   thc tag <id> work -someday   thc mv <id> --under <id>|--journal today
thc link <a> <b> --rel blocks   thc unlink <a> <b>   thc rm <id>   thc restore <id>
thc attach <id> <file> [--caption "…"]   # a screenshot or log under a note; show --json lists "abs"
thc apply plan.jsonl [--dry-run]   # one JSON op per line → one transaction (thc instructions apply)
thc view ls   thc view add work 'status:open #work sort:due'   thc view rm work
thc show <id> --json   thc journal [date]   thc search "words"   thc q '<query>' --json
thc alert add <id> --before 1d | --at "fri 9am"    thc alert ls
thc history <id>   thc log --by claude --since 1d   thc undo [--tx <suffix>]
thc diff <id> --since 1d   thc rewind <id> --to <time|tx> --yes   thc review --json
thc daemon status --json    thc schema <cmd>    thc instructions <topic>
thc vault [ls|info] --json   --vault <name> on any command   # vaults: one per project
```

**Vaults:** you work in the current vault (`thc vault` says which and why, usually a
project's `.thc.toml`). Write to another only when asked (`--vault <name>`). JSON names the
vault on every node (`vault`) and listing (`vault.name`).

### Capture syntax (`add`, `todo`, `remind` text)

| Token | Meaning |
|---|---|
| `[ ]` `[x]` `todo:` prefix | status |
| `due:fri` `due:2026-10-10` `due:"nov 1 9am"` | hard deadline |
| `sched:mon` | planned/start date (shows in Today from then) |
| `at:"tue 2pm"` | a scheduled time with no task status (an event) |
| `every:2w` `every:weekday` `every!:1mo` | repeat (`!` = from completion) |
| `!high` `!med` `!low` | priority |
| `#tag` | tag (kept in the text) |
| `[[Page Title]]` | link; creates a stub page if missing |

**Quoted text is never parsed:** inside "double quotes" or `backticks` nothing is a token
(`!high`, `#tag`, `due:`, `[[…]]`), so write about the syntax in quotes. A quoted value
(`due:"nov 1 9am"`) is still a token, so to mention a token put the whole token in
backticks. For findings that quote syntax, `thc add --plain` keeps the text exactly as written.

`thc add` writes to today's journal; `--inbox`, `--under <id>` or `--journal <date>` override.
On the CLI a token whose value doesn't parse (`due:fryday`) exits `6` with a hint; the TUI,
ThoughtBar and the daemon's `capture` keep it as plain text. `thc parse "…" --json` previews.

### Query syntax (`thc q '<query>'`)

```
status:open | todo | doing | waiting | done | cancelled | closed | any | none
due<=+3d   sched=today   done>=-7d   due:none   created>=-1w   journal=today
#tag   -#tag   under:<id>   parent:<id>   by:claude   to:<role|actor>   text:word   bare words
is:task | page | journal | inbox | event | repeating | overdue | conflict | alert | blocked | blocking | ready | unread
blocks:<id>   blocked-by:<id>   rel:<name>   <custom-prop>=<value>   !high   @view
ancestor:( … )   has:child( … )   has:child   happens<=+7d (scheduled, due or an alert)
( … or … )   -( … )   sort:due | sort:priority | sort:updated- | sort:title | sort:date | sort:order
group:parent | group:tag | group:status | group:due | group:actor | group:vault   (layout only; JSON → groups[])
vault:acme   vault:(acme or personal)   vault:*   -vault:side   (top level; rows carry `vault`)
```

`is:ready` = open (`todo`/`doing`), scheduled reached or unset, no open blocker: the next
thing to pick up. Terms AND by default; `or` and parentheses group; a leading `-` negates (a term or a group).
`sort:order` follows the outline: parents before children, siblings in their displayed order.
`sort:order-` reverses it. Priority remains a separate urgency signal.
`@name` runs a saved view (`thc view ls`) and composes: `@work due<=+3d`. Unknown values
(`status:opn`) exit 6 with a suggestion. `thc q '…' --explain --json` says what a query means (relative dates resolved) without running a listing. `--as-of=-2d` evaluates against past state.

### Dates

`today` `tomorrow` `fri` `next fri` `+3d` `-1w` `in 2 weeks` `eom` `nov 1` `2026-10-10`
`2026-10-10T14:00` `+2h` `+30min`. Times: `9am`, `14:30`, `noon`. A bare `m` is ambiguous and exits 6:
write `+30min` for minutes and `+2mo` for months.

### Batch writes (`thc apply`)

One JSON object per line, all applied as **one transaction** or none at all (exit 6 lists
every bad line). `--dry-run` previews. One undo (`thc undo --tx`) and one review item.

```jsonl
{"cmd":"add","text":"Offsite","inbox":true,"as":"off"}
{"cmd":"todo","text":"Book the venue","due":"fri","under":"$off","key":"offsite-venue"}
{"cmd":"set","id":"pab27","props":{"priority":"med"}}
{"cmd":"done","id":"65p83"}
```

- **cmds:** `add` `todo` `remind` (`text`, `due`, `sched`, `at`, `priority`, `tags[]`,
  `repeat`, `under`, `journal`, `inbox`, `key`), `set` (`id`, `props{}`), `text`, `tag`
  (`add[]`, `remove[]`), `done` `reopen` `skip` `rm` `restore`, `mv` (`under` / `journal` /
  `root` / `after` / `before`), `link` `unlink` (`a`, `b`, `rel`), `alert` (`before` or `at`).
- **`text` takes capture syntax**, as on the CLI: `"text":"Book venue due:fri #offsite !high"`.
- **`set`** takes `props{}` or a CLI-style string: `"set":"priority=med due=+3d"`.
- **Input** is JSONL or one JSON array of the same objects.
- **References:** `"as":"name"` on a create, then `"$name"` in any id field later.
- **Outlines:** `thc import - [--page T | --under id | --journal d]` reads `- item` lines
  (indent to nest; capture syntax works) into one transaction.

### Conflicts

When two devices edit the same node's text before syncing, both versions are kept and the
node gets an open conflict. Conflicts are the human's to resolve.

- **Find them:** `thc conflict ls --json` or `thc q 'is:conflict' --json`; nodes carry a
  `conflicts[]` list with the `current` and `other` versions (actor, device, time).
- **Mention them, don't resolve them.** Only run `thc conflict resolve <id> --keep
  current|other|both` when the human asks.
- **Don't write over them.** `thc text` on a node with an open text conflict exits `4` and
  changes nothing; `--force` only when the human said which text to keep. Other writes work.
- **Move conflicts** (a move that would have looped) were already skipped. No action needed.

### Exit codes

`0` ok · `1` error · `2` usage · `3` not found (also `thc daemon status` when offline) ·
`4` conflict or stale (an open sync conflict, or `--if-match`/`--expect` failed: re-read,
then decide) · `5` ambiguous id (the JSON error lists
`candidates`) · `6` validation (fix the input). Errors go to stderr as
`{"error":{"kind","message","candidates"?}}` with `--json`; `stale` errors add
`node`, `rev` (current) and `changed[]`.

### JSON contract

- **Additive only:** within a major version, fields in `--json` output and in the daemon
  protocol are only added, never removed, renamed or retyped. Ignore unknown fields; absent
  optional fields (nulls are omitted) mean "not set".
- **Contexts:** a person may turn on a context (`thc context work`) that filters *their*
  listings. Your `--json` reads and your writes ignore it; listings report it as
  `"context":{"name","applied","hidden"?,"device"}` (`hidden` = how many it removed). Pass `--context <name>` only when asked.
- **Schemas:** `thc schema <cmd>`, `thc schema --all`, `thc schema --proto` (daemon socket).
- **Node:** `{id, short, rev, kind, parent, title, text, status, scheduled, due, priority, repeat,
  done_at, journal, tags[], props{}, created_by, created, updated}`. Rows in `today`/`agenda` add `reasons[]` (`overdue`, `due-today`, `scheduled`, `alert`, `alert-fired`, `repeating`, `doing`, `done-today`). **Writes:** `{ok, tx,
  events, nodes[]}`. `--fields` trims node objects to the fields you name.

### Coordinating work

When the human asks you to coordinate through thc, thc is the task board. Keep no private task
list.

- **The board:** the vault (and its capture page) the human names, or this project's vault
  and its `[capture] target` page. Confirm it once: `thc vault --json`.
- **Pick work:** `thc next --json` and take the top item. The queue order is the outline order
  plus `blocks` links. Don't skip ahead; add a note saying why instead.
- **Claim it, always, before any work** (a quick fix too): `thc set <id> status=doing
  owner=<you> session_id=<your session id> --expect status=todo`. Start only on exit `0`; exit
  `4` means it's taken, so pick again. Give a real session id, never an invented one (leave it
  out if you have none). Work done without a claim has no start time: it shows as untracked.
- **Notes are progress:** `thc add --plain --under <id> "…"`, one per finding. A note starting
  `to: <name> (from <you>)` is a message to another agent; read the notes on a task before
  acting on it.
- **Finish:** `thc done <id>` plus a closing note. It waits for human review; never accept your
  own work.
- **The order is the human's,** or the coordinator's they name. Add tasks with a priority and
  `thc link <a> <b> --rel blocks`; never reorder others' tasks.

### Team roles

`thc board --json` resolves `--board`, `THC_BOARD`, the nearest `.thc.toml` board key,
then the current vault's `[capture] target`. A named board without a page uses that vault's
capture target or root. A board does not change where ordinary captures land.

`thc instructions role <name>` (also `thc role show <name>`) prints a built-in card: lead (or
pm), engineer, merger, designer, reviewer. Roles are lowercase names; lead and pm share a
card. `role=engineer` routes a task; `thc next --role engineer --json` includes engineer
tasks and unrouted tasks.
`thc prime --role engineer --json` adds the board's ¶ Roles rules to the built-in card
and lists the next work. ¶ Roles has one note per role (role name on its first line),
with rules as children. Project rules append to the built-in card.

New nodes created by team members use `THC_ACTOR` for the actual agent's name and
`THC_ROLE` for `agent_role`.
Set `THC_SESSION_ID` only to a real session ID when one is available; never invent an ID.

### Team messages

`thc msg engineer "The copy is ready" --on <task> --key <key>` creates an addressed
plain note under the task. `--about` is an alias for `--on`. Without a task it goes on the
board's ¶ Messages page. Its text starts `to: engineer (from <actor>):` and its properties
are `to=engineer` and `from=<actor>`; capture tokens in the message stay literal.

`thc msgs [--for <role|actor>] [--unread] --json` lists messages without marking them read.
`to:engineer` queries match role messages, direct messages to engineers and `to:all`.
`to:codex-engineer-2` matches that actor, its role and `to:all`.
`is:unread` selects messages for your actor/role with no receipt from your actor. Your role
comes from THC_ROLE, otherwise from the actor's `<agent>-<role>[-number]` name.

`thc msg read <id>…` or `thc msg read --all` acknowledges for your current actor only.
Receipts are ordinary boolean properties `read_<actor>=true`: one engineer reading a role
broadcast never marks it read for another engineer. `prime --role` acknowledges only the
messages it shows; `--readonly` leaves receipts untouched. `--if-match`, `--expect`, `--dry-run`
and the normal write policy apply to acknowledgements too.

`thc watch --for <role|actor> [--board <board>] [--json] [--once]` waits for unread messages
and explicitly routed, ready, unowned tasks. It prints one item per line (JSON objects with
`event`, `node`, `board`, `source` with --json), flushing immediately. Existing unread/ready
items are reconciled on startup and daemon reconnect; --once exits after the first item.
Messages emit once per watch session; tasks emit again if they leave and re-enter readiness.
Watching never acknowledges messages or claims work. Role watches include direct messages
to actors in the role, with your current actor's receipts, like msgs --for. Actor watches use
that actor's receipts and inferred role. Board resolution is the same as msgs and next.
Messages span the board vault (including ¶ Messages); tasks stay under its capture page.
The daemon pushes changes; when offline, watch announces polling every second on stderr and
reconnects automatically. SIGINT and SIGTERM exit cleanly. The stream ignores listing limits.

### Starting a team

`thc team up pm designer engineer --agent claude --agent engineer=codex` starts one agent
per role in this project: herdr first, then a new WezTerm team tab, otherwise shell commands.
Each gets THC_ACTOR, THC_ROLE and the resolved THC_BOARD, independent of the capture vault.
It uses normal agent permissions. `--model` fills the launcher model arguments, including
native arguments after -- in herdr. Custom launchers use [team.agents.NAME] argument arrays in config.

Bare `thc team up` reads [[team.roster]] entries in the project's .thc.toml: role, agent,
optional model, and count (default 1). Entries keep their order and repeated roles get unique
actor suffixes. Listed roles replace the configured roster; --agent and --model override it.
Profiles open a new team tab with balanced columns and the first pm/lead at bottom-right.
[team.layout] sets columns (default 3), tab (default "team"), and pm="bottom-right".
Explicit-role commands keep their previous layout unless [team.layout] is set. The user's
[team] max_agents cap still applies; set it to 7 for a seven-person roster. A Claude entry's
permission_mode="auto"|"manual" is human-only; agents must omit it. Native first-run trust
dialogs are left for the human. --dry-run shows the roster, commands and layout without launching.

`thc team ls --json` joins the cached project roster with live host state, board claims,
unread messages and last activity. `thc team down --yes` closes only the saved panes;
without --yes it asks in a terminal. It keeps the board and claims. Lifecycle commands follow
actor policy (`team`); --readonly refuses them and --dry-run previews without launching.

`thc team add engineer --agent codex --model M --task ID` adds one teammate to the
running project team. `thc team agents` lists built-in, configured and herdr launchers.
Names use the lowest free suffix (codex-engineer, codex-engineer-2). The new agent reads
and claims its first task itself with --expect status=todo; a taken task falls back to next.
Herdr receives model options after -- and the briefing separately through agent prompt.
For Claude, a human may choose --permission-mode auto|manual; omission keeps the user's
default. Agent callers, including an opted-in lead, must omit --permission-mode.
Launchers and native options that skip permission prompts are refused.

Only the human or the lead may add/remove members. A lead needs [team] lead_can_add = true
(default false); max_agents defaults to 6. This permission still respects actor deny/confirm,
read tiers and readonly. `thc team rm ACTOR` (or an unambiguous role) removes membership and
closes only its saved pane. A doing claim requires confirmation or --yes; ownership/status
are kept. Adds and removals write attributed notes under ### Members on ¶ About and on the
associated task. --dry-run previews without cache, board or host writes.

For a new project, a person confirms its proposed board vault in a terminal, or passes
`--new-board <name>` explicitly. --yes alone never approves creating a board. This seeds
Issues, About from README, Roles, @issues and project .thc.toml, preserving an existing vault.
A new board needs pm or lead. The lead writes ordered tasks, completes the keyed First plan
task, and asks the human to accept that completion in review. Other roles' next and prime
report waiting=first-plan until then. Undoing acceptance or reopening waits again; removing
First plan removes the gate. Agents never accept work on the human's behalf.

### Beside the person (`thc ui aside`)

When the human is in the TUI and asks you to show them something, open it in their sidebar:
`thc ui aside "Reading List"` (a page title or id), `thc ui aside today` (a day: `fri`,
`2026-10-06`), `@today` / `@inbox` / `@tasks` / `@log` / `@<saved view>`, `"#tag"` or any query.
The panel opens on top, marked `◆ <you>`; their keyboard stays where it is, the bar tells them, and
`⌘[` takes it back. `--pin` and `--fold` open it pinned or folded; `--close <id>` closes a panel
you opened (never a pinned one); `--ls --json` lists the stack with titles and counts. Exit `3`:
no such page or day; `5`: ambiguous (candidates listed); `6`: a bad query or a refusal. Only on
request: the sidebar is the human's. `thc ui patch` can change it too, but can't move their
focus or close or unpin a pinned panel (exit 6).
<!-- thc:end -->

---

## Part 2: Working on this repo

### How we write code here

These rules come first; every review checks them.

- **One source of truth.** State is normalized: a document, node or query lives in one place,
  and everything else holds a reference (an id) to it. Never copy state between owners, never
  swap it in and out, never keep a parallel version "for this case".
- **Elm architecture everywhere.** Serializable state, messages, a pure `update` that returns
  effects, and a pure `view`. Runtime handles (files, sockets, threads) stay outside the state.
  If it isn't in the state, it can't be replayed, patched or tested, so treat that as a bug.
- **Components are instantiated, not forked.** The same component (an editor pane, a list) is
  used wherever that thing appears, with its own small view state pointing at a shared leaf.
  Differences between places are data (a role, a policy, a size), never a second code path.
- **Special cases are a smell.** A guard like `if in_panel`, a deferred action, a field swap, a
  one-off branch or "edge-case gymnastics" means the model is wrong. Flag it, and fix the model
  so the case disappears, instead of adding another guard.
- **Get things for free.** Before writing code, ask what correct modelling would give us for
  free (replay, agent control, undo, sync, tests). Prefer the change that deletes code.

- **Layout:**
  - `crates/thc-core` holds the model, event log, merge/replay, store, query, capture,
    dates, recurrence, edit round-trip and export.
  - `crates/thc-tui` is the ratatui TUI, built to the TUI design spec (in the private design repo).
  - `crates/thc` is the CLI binary. **A new command is its own module:** a clap `Args` type,
    a `pub const SPEC: Spec = spec!("name", "help line", Args, run, board: …, settings: …)`,
    and one line in `crates/thc/src/registry.rs`. Don't add it to `cli.rs` or `main.rs`.
    The older commands in the `Cmd` enum move over in turn.
- **Reviewing the TUI without a terminal:** `THC_TUI_SNAPSHOT=120x32 THC_TUI_KEYS="3jd" thc tui`
  renders one frame as text after replaying keys (`<cr>`, `<esc>`, `<tab>`, `<s-tab>`, `<bs>`,
  `<up>`/`<down>`/`<left>`/`<right>`, `<space>`, `<c-x>`, `<m-s>` (⌥), `<sclick:x,y>` (⇧-click)
  supported).
  Keys that write land in a scratch copy of the vault, so a snapshot never changes the real one;
  `THC_TUI_SNAPSHOT_WRITE=1` writes for real.
  `THC_THEME=ansi|ember-dark|ember-light` and `THC_GLYPHS=ascii` select themes.
  `THC_TUI_SNAPSHOT_FORMAT=ansi|html` emits styled output (colours and modifiers) for design review.
  The `<agent:TEXT>` key token makes a real agent write mid-session (toast, live flash), and
  `<alert>` raises the alert toast for the first pending alert. Snapshots show `● live` when a
  daemon runs for the vault, otherwise the offline bar.
- **The TUI is state-driven (Elm style), and the state is an API:** see `docs/ui-protocol.md`.
  Every presentation choice lives in one serializable `UiState` (`crates/thc-tui/src/ui_state.rs`);
  every input is a `Msg` applied by `Session::apply` (`session.rs`); presentation actions are a
  pure `update::action` returning effects; the view (`ui.rs`, `doc_ui.rs`, `node_row.rs`) reads
  only state and derived data (a test fails if it reaches the store, a clock, env or files).
  New TUI state goes in `UiState`, never as a hidden field, cell or thread-local; time comes
  from `UiState::now_ms` (ticks), never `Instant::now()`.
  - Drive a running TUI: `thc ui ls`, `thc ui state`, `thc ui patch '{"view":"tasks"}'`,
    `thc ui send keys 'gg<cr>'`, `thc ui render 120x32`, `thc ui send subscribe`.
  - Without a terminal: `thc ui render 120x32 --state s.json` (start from `thc ui state
    --default`). `THC_TUI_SNAPSHOT` still works and reads the same key scripts.
  - Record and replay: `thc tui --trace t.jsonl`, then `thc ui replay t.jsonl [--every]`.
    With `THC_NOW` pinned, renders and replays are byte-identical across runs.
- **The sidebar** (docs/design/sidebar.md in the internal repo): `sidebar.rs` is the stack as
  UiState with its pure rules (and `policy`, the owner's open choices in one place),
  `update::sidebar` the pure update, `sidebar_app.rs` the runtime (a doc panel is a caretline
  view on a shared `Doc`; `App::with_panel` runs a key through it), `sidebar_ui.rs` the drawing.
  Its keys are the `sidebar` context of the keymap. Acceptance checks and goldens:
  `crates/thc/tests/sidebar.rs` (`THC_UPDATE_GOLDENS=1` regenerates).
- **Fixtures:** `scripts/seed-sample.sh [--conflict] <vault>` seeds sample data. `--conflict` also
  runs `scripts/fixture-conflict.sh`, which builds the daemon spec's conflict states: a text
  conflict, a rejected move cycle and an agent reminder. Never point either script at a real vault.
  `THC_FIXTURE_IDS=1` (with `THC_NOW` and `THC_DEVICE`) makes the writer stamp the pinned clock and
  derive tx/event ids from it, and the seed keys its creates, so `scripts/guide-renders.sh` output is
  identical across runs: a render diff means the UI changed.
- **Build/test:**
  - `cargo build`, `cargo test` (the Rust toolchain is pinned via `mise.toml`, so run
    `mise exec -- cargo …` if `cargo` isn't on PATH).
  - `crates/thc-core/tests/convergence.rs` simulates several devices syncing and must
    always pass.
- **The contract is `docs/FORMAT.md`:**
  - Replay must stay a pure function of the events. Anything clock-, random- or
    timezone-dependent belongs in `TxBuilder` (the writer), never in `Store::apply`.
  - New op types need a format version bump.
- **Before every commit and push (public repo):** a stray build folder once reached public main
  and took a history rewrite to remove, so:
  - Stage explicit paths (`git add <files>`), never `git add -A` / `git add .` in a tree that has
    build output in it. Build outside the checkout (`CARGO_TARGET_DIR` in your scratch area) or in
    the gitignored `/target`.
  - Run `scripts/preflight.sh` before pushing and before `gh pr merge`; `--staged` checks the index.
    It fails on build output, files over 512 KB, binaries outside fixtures/goldens/web/public, home
    or scratch paths, the local user name, private repo URLs and credential-shaped strings.
  - `scripts/install-hooks.sh` (once per clone) makes every push run it; CI's Preflight workflow
    runs it on each PR. Never bypass it with `--no-verify`. If it flags something, unstage it or
    fix the cause; a real exception goes in `.preflight-allow` with a comment saying why.
- **caretline (the editor engine):** caretline lives at brancusi/caretline; engine bugs found in
  thc are fixed there first, released or pinned, then pulled into thc. Never patch the engine
  inside thought-control.
  - thc depends on `caretline = "0.3"` (crates.io) in `crates/thc-tui/Cargo.toml`.
  - A fix not yet released is pinned in the root `Cargo.toml`:
    `[patch.crates-io] caretline = { git = "https://github.com/brancusi/caretline", rev = "<sha>" }`.
    Bump the rev when thc needs a newer engine commit (merged on brancusi/caretline main), and
    remove the patch once a caretline release contains it.
  - Working on both at once: point the patch at your caretline checkout
    (`caretline = { path = "../caretline/crates/caretline" }`), never commit that, then land
    the engine PR in brancusi/caretline and pin its merged rev here.
- **Keys:** one table (`crates/thc-tui/src/keymap.rs`, keymap.md) drives dispatch, the footer, help,
  the palette and `thc keys`. The write context's editing keys and their words come from caretline's
  command catalog and default keymap (`crates/thc-tui/src/editing_keys.rs`); thc's own differences are
  explicit rows that win. After changing it, run `scripts/keys-guide.sh` (a test fails while
  docs/guide/keys.md is stale). Users remap with `thc keys --edit` (crates/thc-tui/src/keys_edit.rs:
  a generated, commented `[keys.*]` block in config.toml, checked after the editor closes).
- **Docs:** `docs/SPEC.md` (spec), `docs/FORMAT.md` (the on-disk contract), `PHILOSOPHY.md`
  (principles), `docs/guide/` (the user guide). Design specs are private (see Maintainers).
- **Vaults here:** the repo's `.thc.toml` (gitignored, each maintainer's own) points a bare `thc` at
  the maintainers' real board, not a sandbox. For sandbox work use `THC_VAULT=./vault` (or a scratch
  path), which beats `.thc.toml`; `scripts/seed-sample.sh` fills one with sample entries.
- **Performance budgets** are in `docs/SPEC.md §8`. Check with a release build and
  `hyperfine` before and after changes on the read path.

### Maintainers

Design specs, research notes and the maintainers' coordination rules live in a private
companion repository. If `../thought-control-internal/` exists next to this checkout, read its
`AGENTS-coordination.md` before starting work, and treat its `docs/design/` as the spec.
Contributors without it can work from the code, `docs/SPEC.md`, `docs/FORMAT.md` and the guide.
