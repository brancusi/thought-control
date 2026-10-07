# thought-central: spec and implementation plan

- **Status:** v0.5 (2026-10-03). M0–M3 are implemented (CLI, TUI, daemon); M4 (ThoughtBar) is next (see §11)
- **Binary:** `thc` ("thought control")
- **Platforms:** macOS and Linux
- **Background:** research notes on data models (not published)

---

## 0. What changed since v0.1

| v0.1 | v0.2 | Why |
|---|---|---|
| Markdown files were the source of truth | **An append-only event log is the source of truth.** SQLite is a local view rebuilt from it, and Markdown is an export | Stable IDs, typed fields, transactions and history without a fragile parse-and-rewrite layer. Logseq made the same move in 2026 (see research §2) |
| Tasks were inline `- [ ]` lines in any file | **One primitive, the node.** "Task" is a trait any node can have | Every modern tool has converged on this (research §6) |
| Cross-platform, including Windows | **macOS and Linux only** | Unix sockets, launchd and systemd only |
| The CLI sent its reads through the daemon | **The CLI reads SQLite directly.** Writes go through the daemon when it's running | Measured: a direct read takes about 2.3 ms in total, and routing through the daemon would save only about 0.5 ms (§8) |

---

## 1. Goals and non-goals

**Goals**

1. A personal stash for notes, todos, dates and reminders, driven by a **CLI and a TUI**. There is no GUI editor.
2. **Agents have the same capabilities as humans**: stable IDs, validated typed writes, `--json` output, and an audit trail of who changed what.
3. **Local-first:** it works offline, needs no server, and syncs through Dropbox (or iCloud or Syncthing) with **no conflicted copies**.
4. **Immutable history:** every change is an event, so undo, per-item history and "what did this look like last Tuesday?" queries come for free.
5. **No lock-in:** the source of truth is plain JSONL, and a continuously updated Markdown export sits next to it.
6. **Fast:** a CLI query should take under 5 ms in total, and the menu bar should update within 250 ms of a change.
7. A **macOS menu bar app** that is a thin client for reviewing what's pending and due, checking items off and quick capture.

**Non-goals (v1)**

- Rich media, styling, whiteboards, or graph visualization.
- Rich text (WYSIWYG). Pages and days are plain text with Markdown markers, written in the TUI's
  document editor (§6.3) or in `$EDITOR`.
- Multi-user collaboration or a hosted backend.
- Calendar sync (CalDAV/ICS). The model is designed so it can be added later (§3.6).
- Windows.

---

## 2. Architecture

```
 Dropbox/thought/                           (synced, append-only, no conflicts by design)
 ├── log/<device>/2026-10.jsonl  ◄── each device appends ONLY to its own files
 ├── drop/                       ◄── any .md/.txt dropped here becomes inbox items (mobile capture)
 └── export/                     ◄── generated Markdown, read-only

            │ watch + replay                         ▲ render
            ▼                                        │
 ┌──────────────────────── thc (one Rust binary) ─────────────────────────┐
 │  thc-core: model · ops · validation · HLC · replay · query compiler    │
 │  thc-store: SQLite materialized view (local cache dir, never synced)   │
 ├───────────────┬───────────────┬──────────────────┬─────────────────────┤
 │ CLI           │ TUI (ratatui) │ daemon           │ mcp (stdio)         │
 │ reads: direct │ reads: direct │ replay, watch,   │ same ops as CLI     │
 │ writes: via   │ live via      │ write serializer,│                     │
 │ daemon if up  │ subscription  │ events, alerts,  │                     │
 │               │               │ export           │                     │
 └───────────────┴───────────────┴────────▲─────────┴─────────────────────┘
                                          │ JSON-RPC over a Unix socket
                               ┌──────────┴──────────┐
                               │ ThoughtBar.app      │  SwiftUI MenuBarExtra
                               │ (subscribe + act)   │  no file or DB access
                               └─────────────────────┘
```

**Principles**

1. **The log is the truth.** SQLite and the export can be deleted at any time and rebuilt by replaying the log.
2. **Single writer per device.** Only this device's process ever appends to `log/<this-device>/`, so Dropbox never sees two writers on one file.
3. **The daemon is optional.** It speeds things up and pushes live updates, but the CLI and TUI work fully without it.
4. **Inline syntax only at human edges.** Typed fields are canonical. Inline syntax (`due:fri`, `#tag`, `[[link]]`) is parsed once at capture or `$EDITOR` time and stored as fields.

---

## 3. Data model

### 3.1 Primitives

There are **four record types**:

```
Node    the single content primitive: a page, a block, a task, a journal day and a tag are all nodes
Edge    a typed relation between nodes (mention, tag, blocks, mirror_of, …)
Alert   a reminder attached to a node, either absolute or relative to its dates
Prop    a typed property definition (registry); values live on nodes
```

**Node**

| Field | Type | Notes |
|---|---|---|
| `id` | 60-bit random, Crockford base32, 12 chars (`k3f9a2mq7x1c`) | Generated with no coordination between devices. **Displayed as the shortest unique prefix, at least 5 chars** (`k3f9a`), like git hashes. JSON always includes the full ID |
| `parent` | node ID or null | null means a root node (a page, journal day, tag or inbox item) |
| `order` | fractional-index string (base-62) | Sorts siblings. Inserting or moving never renumbers other nodes (same approach as Logseq DB `:block/order`) |
| `text` | Markdown, may be multi-line | References are stored as `[[id]]` and rendered with the target's current title, so renames never break links |
| `title` | string or null | Set on page-like nodes. Unique among root nodes with the same tag, case-insensitively |
| `props` | map key → typed value | See §3.2. Built-in properties are stored as indexed columns |
| `created_at`, `created_by` | HLC, actor | Derived from the creating event |
| `deleted` | tombstone | Soft delete. Restorable, and visible in history |

**Edge:** `(src, rel, dst)` plus provenance.
- `mention` and `tag` edges are **derived from the text** whenever the text changes.
- Other relations are explicit: `blocks`, `relates`, `mirror_of`, `parent_of` is *not* an edge because that's the `parent` field.

**Alert**

| Field | Type | Notes |
|---|---|---|
| `id`, `node` | | Many alerts per node are allowed |
| `trigger` | `{at: datetime}` or `{offset: "-1d"\|"-15m", anchor: "scheduled"\|"due"}` | Same idea as VALARM `TRIGGER` and Todoist relative reminders |
| `state` | pending / fired / acked / snoozed (`until`) | Changes when you snooze or acknowledge |

A reminder with no task attached ("alert me Nov 1 at 9am: pay rent") is just a node with one absolute alert.

### 3.2 Built-in properties (traits)

Built-in properties are what turn a node into a task or a dated item. **Any node can be given a status. A separate "task type" doesn't exist.**

| Prop | Type | Meaning |
|---|---|---|
| `status` | enum: `todo` · `doing` · `waiting` · `done` · `cancelled` | Present means the node is a task. Removing it turns the node back into plain content |
| `scheduled` | date or datetime | **The planned or start date.** The node shows in Today from this date on |
| `due` | date or datetime | **The hard deadline.** It counts as overdue after this |
| `priority` | enum: `high` · `med` · `low` | |
| `repeat` | `{rule, mode}` | See §3.4 |
| `done_at` | datetime | Set automatically when the node is completed |
| `journal` | date | Marks the node as a journal day. Unique per date |
| `estimate` | duration | Optional |

Using `scheduled` and `due` as two separate fields follows Org `SCHEDULED`/`DEADLINE`, Things `when`/`deadline` and Capacities. Todoist users will think of them as the planned date and the hard deadline.

**Custom properties:**
- `thc set <id> client=acme` auto-registers `client` as a `text` property on first use.
- After that the type is **fixed** (Logseq DB does the same), and writes are validated against it.
- Types: `text`, `number`, `bool`, `date`, `datetime`, `node`, `enum(…)`, and `[T]` for lists.

**Tags:**
- `#work` creates or links a root node titled `work` with a `tag` edge.
- In v2, a tag node can declare **default properties** for nodes tagged with it, like Tana supertags or Logseq tag classes. For example, `#bug` would imply `status=todo` and a `severity` property.

### 3.3 Conventions built from primitives

These are conventions, not new types:

| Concept | How it's represented |
|---|---|
| Page / note | Root node with a `title`. Its children are its blocks |
| Long-form note | A page whose children are paragraphs. A single node can also hold multi-line text when you don't want blocks |
| Journal day | Root node with `journal=2026-10-03`, created on demand |
| Inbox | Root nodes that have no title and aren't journal days. Triage means moving them under a page or journal day |
| Todo | Any node with `status` |
| Event / appointment | Node with `scheduled` set to a datetime, plus an optional `end` property and alerts |
| Reminder | Node with an absolute alert |
| Subtask | Child node with `status` |
| Mirror / transclusion (v2) | Node with a `mirror_of` edge. Renders the origin's text and children (Workflowy-style origin and replica) |

### 3.4 Recurrence

`repeat = {rule, mode}`

- **`rule`** is a subset of RFC 5545 RRULE: `FREQ=DAILY|WEEKLY|MONTHLY|YEARLY`, `INTERVAL`, `BYDAY`, `BYMONTHDAY`, `UNTIL`, `COUNT`.
- **CLI input is natural language** (`every 2w`, `every weekday`, `every month on the 1st`), and the original string is kept for display (as Todoist does).
- **`mode`** matches Org repeaters and covers Todoist's `every`/`every!` and Things' after-completion mode:

| Mode | Org | Next date after completion |
|---|---|---|
| `fixed` | `+1w` | previous date + interval (can still be overdue) |
| `catch_up` | `++1w` | previous date + interval, repeated until it's in the future |
| `from_done` | `.+1w` | completion date + interval |

**Completing a repeating node does not change its status to done.**
1. A `node.complete {occurrence}` event is emitted.
2. The same node's `scheduled` and `due` dates advance, and relative alerts re-arm.
3. Subtasks are reset to `todo`.

Each occurrence's completion is preserved in the event log, which gives per-occurrence history (`thc history <id>`) without Things' template-and-copies machinery. "Skip this occurrence" is a `node.skip` event.

### 3.5 Identity rules for agents

- IDs are random, not sequential, so parallel agents or devices never collide (sequential IDs collided in beads).
- **Prefix resolution:**
  - Any unique prefix of at least 4 characters is accepted.
  - An ambiguous prefix fails with exit code `5` and lists the candidates. The tool never guesses.
- **Idempotent creates:** `--id <full-id>` or `--key <idempotency-key>`. Repeating the same create is a no-op that returns the existing node (like Todoist's `temp_id`/`uuid`).
- **Titles are never keys.** `[[Some Title]]` is resolved to an ID at write time. An unresolved title creates a stub page and reports it in the output.

### 3.6 Mapping to standards (for later export)

| Here | iCalendar |
|---|---|
| node with `status` | `VTODO` with `UID` = id |
| `scheduled` | `DTSTART` |
| `due` | `DUE` |
| `repeat.rule` | `RRULE` |
| `alert` | `VALARM`, `TRIGGER;RELATED=START\|END` |
| `status` | `STATUS` |

This keeps a future read-only ICS feed or CalDAV bridge easy to build.

---

## 4. Event log and sync

### 4.1 Event envelope

One JSON object per line, in `log/<device-id>/<YYYY-MM>.jsonl`:

```json
{"v":1,"eid":"01JA3…","hlc":[1759500000123,0],"dev":"mbp-7f3a",
 "actor":{"kind":"agent","name":"claude"},"via":"cli","tx":"01JA3…",
 "op":"node.set","id":"k3f9a2mq7x1c","props":{"due":"2026-10-06"},"base":"01JA2…"}
```

| Field | Meaning |
|---|---|
| `eid` | Event ID (ULID) |
| `hlc` | Hybrid logical clock, as `[unix_ms, counter]`. Ties are broken by `dev` then `eid`, which gives every event a total order across devices |
| `actor` | Who made the change: `human`, or `agent` plus a name. Set with `THC_ACTOR`, or the `--actor` flag |
| `via` | `cli` · `tui` · `bar` · `mcp` · `edit` · `drop` |
| `tx` | Groups the events of one command, so they're applied, displayed and undone together |
| `base` | For text edits: the `eid` of the text version this edit started from. Used to detect conflicts |

### 4.2 Operations

```
node.create   {id, parent, order, text, title?, props?}
node.text     {id, text, base}
node.set      {id, props:{k: v|null}}          title is set here too
node.move     {id, parent, order}
node.complete {id, occurrence?}                 handles repeats (§3.4)
node.skip     {id, occurrence}
node.delete   {id}        node.restore {id}
edge.add      {src, rel, dst}    edge.remove {src, rel, dst}
alert.add     {id, node, trigger}   alert.ack {id}   alert.snooze {id, until}   alert.remove {id}
prop.define   {key, type}
```

Undo emits the **inverse events** for a `tx`. History is never rewritten.

### 4.3 Merge semantics (deterministic on every device)

| Case | Rule |
|---|---|
| Same field set on two devices | The higher HLC wins (last write wins, per field) |
| Concurrent `node.text` with the same `base` | The higher HLC wins. **The losing text is kept** in a `conflicts` record and flagged in the TUI, menu bar and `thc doctor`. `thc conflict resolve <id>` picks a version or opens a 3-way merge in `$EDITOR` |
| Concurrent moves that would create a cycle | Replaying in HLC order rejects the move that would close the cycle, and records a conflict |
| Edit to a deleted node | It stays deleted. The edit is still recorded and reappears on restore |
| Concurrent completion of the same occurrence | The second completion is a no-op (idempotent by `(id, occurrence)`) |

**Events that arrive late:** Dropbox can deliver an event whose HLC is older than events already applied. When that happens, the store is rebuilt by replaying every event from scratch (`catch_up_locked` → `rebuild_locked`; there are no snapshots yet). At personal scale, a full replay of about 100k events takes under a second, so this stays simple and correct. Replaying from a local snapshot taken before that HLC is a later optimisation.

### 4.4 Sync over Dropbox

1. **Files:** each device appends only to its own files. A new file each month keeps them small.
2. **Writing:** append the line, then `fsync`, then update SQLite in the same step (§5.3).
3. **Reading other devices:**
   - The daemon watches `log/` and keeps a byte-offset cursor per file in SQLite.
   - It replays only complete, newline-terminated lines. A partial line mid-sync is left until it finishes.
4. **Device ID:** generated on first run and stored locally in `~/.config/thought/device`.
5. **Snapshots:** taken periodically and stored **locally** in the cache directory, never in Dropbox. Every device can rebuild from the log alone.
6. **Dropbox online-only files:** a `log/` file that is online-only (macOS File Provider, `SF_DATALESS`) is reported by `thc doctor` with a fix: mark `thought/` as "Available offline".
7. **Mobile capture via `drop/`:**
   - Any `.md` or `.txt` file saved there (from the iOS Files app, Drafts, Shortcuts or Obsidian mobile) becomes inbox nodes. Each list item or paragraph becomes one node, and `due:fri` / `#tag` are parsed.
   - The file is then moved to `drop/.ingested/`.
   - Only one device ingests a given file. Each claims it by renaming it with its device ID before reading, and the loser of a race skips it.

### 4.5 Markdown export

- **Who writes it:** the daemon, 2 s after changes settle (debounced). Without a daemon, `thc export` does it.
- **Layout:** `export/pages/<slug>.md`, `export/journal/2026-10-03.md`, `export/agenda.md`, `export/inbox.md`.
- **Format:** Obsidian-readable. Tasks render as `- [ ] text 📅 2026-10-06 ^k3f9a`, with block IDs, so the export is also browsable read-only on mobile in Obsidian.
- **Edits are discarded:** every file has a "generated, edits will be overwritten, use thc or drop/" header.
- **Also:** `thc export --json` dumps a full JSON snapshot.

---

## 5. Local store (SQLite)

### 5.1 Location and settings

- **Location:** `~/Library/Caches/thought/<vault-hash>/store.db` on macOS, `$XDG_CACHE_HOME/thought/<vault-hash>/store.db` on Linux.
- **Settings:** `journal_mode=WAL`, `synchronous=NORMAL` (the log provides durability), `foreign_keys=ON`, `busy_timeout=5000`. All tables are `STRICT`.

### 5.2 Schema (materialized view)

```sql
nodes(id TEXT PK, parent TEXT, ord TEXT, title TEXT, text TEXT,
      status TEXT, scheduled TEXT, due TEXT, priority INT, repeat TEXT,
      done_at TEXT, journal TEXT UNIQUE,
      created_hlc TEXT, created_by TEXT, updated_hlc TEXT, deleted INT) STRICT;
props(node TEXT, key TEXT, v_text TEXT, v_num REAL, v_date TEXT, PRIMARY KEY(node,key)) STRICT;
prop_defs(key TEXT PK, type TEXT) STRICT;
edges(src TEXT, rel TEXT, dst TEXT, PRIMARY KEY(src,rel,dst)) STRICT;
alerts(id TEXT PK, node TEXT, at TEXT, offset TEXT, anchor TEXT, state TEXT, snooze_until TEXT,
       fire_at TEXT) STRICT;                         -- fire_at is computed, used by the scheduler
events(eid TEXT PK, hlc TEXT, dev TEXT, actor TEXT, via TEXT, tx TEXT, op TEXT,
       entity TEXT, body TEXT) STRICT;               -- full copy of the log, for history and as-of queries
conflicts(id TEXT PK, node TEXT, field TEXT, loser_eid TEXT, resolved INT) STRICT;
cursors(file TEXT PK, offset INT) STRICT;
nodes_fts USING fts5(title, text, content='nodes', tokenize='unicode61 remove_diacritics 2');

CREATE INDEX open_by_sched ON nodes(scheduled) WHERE status IN ('todo','doing','waiting') AND deleted=0;
CREATE INDEX open_by_due   ON nodes(due)       WHERE status IN ('todo','doing','waiting') AND deleted=0;
CREATE INDEX by_parent     ON nodes(parent, ord);
CREATE INDEX alerts_due    ON alerts(fire_at) WHERE state IN ('pending','snoozed');
CREATE INDEX events_entity ON events(entity, hlc);
```

### 5.3 Time travel

- **`thc history <id>`:** reads the `events` rows for that entity.
- **`thc q '…' --as-of "last tue"`:**
  1. Load the nearest local snapshot taken before that time.
  2. Replay events up to that time into an in-memory SQLite.
  3. Run the query.

  At personal scale this takes well under a second, which avoids maintaining a separate versions table.
- **`thc undo [--tx <id> | --last]`:** emits inverse events.
- **`thc log [--actor claude] [--since 1d]`:** audit view of recent changes.

### 5.4 Write path

1. Validate the op against the prop registry and the merge rules.
2. Append the event line(s) for the `tx` to this device's log and `fsync`.
3. Apply them to SQLite in **one transaction**, and advance this device's cursor.

If the process crashes between steps 2 and 3, the next start replays from the cursor, so the result is identical. When the daemon is running, the CLI, TUI and menu bar send write ops to it (one serialized writer per device, which also pushes the change live). When it isn't running, the CLI writes itself while holding a local `flock` on `store.db.lock`.

---

## 6. Interfaces

### 6.1 CLI

**Agent contract:**
- `--json` on every command, with compact objects and full IDs.
- `--fields a,b,c` and `--limit` to keep output small.
- Never prompts when stdin isn't a TTY.
- Exit codes: `0` ok · `2` usage · `3` not found · `4` conflict · `5` ambiguous ID · `6` validation error.
- `--dry-run` on writes prints the ops it would emit.
- `thc schema [cmd]` prints the JSON Schema for a command's input and output.

```bash
# capture (inline syntax parsed once into fields)
thc add "Call dentist due:fri #health !high"     # → inbox node, status=todo, due, tag, priority
thc add "idea: sync reading list"                # → inbox node, no status
thc add --journal "called bank, waiting on fax"  # → child of today's journal node
thc add --under k3f9a "subtask"                  # → child of a node

# tasks and dates
thc todo "Draft Q4 plan" --scheduled mon --due 2026-10-10 --tag work
thc remind "Pay rent" --at "nov 1 9am" [--repeat "every month on the 1st"]
thc alert add k3f9a --before 1d [--anchor due]
thc done k3f9a q81zz                             # several IDs at once; handles repeats
thc skip k3f9a                                   # skip this occurrence
thc set k3f9a due=+3d priority=low client=acme
thc today                                        # overdue + scheduled/due today + alerts today
thc agenda --days 7
thc inbox

# notes and structure
thc page new "Dentist" [--tag health]
thc show k3f9a [--depth 2] [--json]               # node + children + backlinks + props
thc edit k3f9a                                    # $EDITOR round-trip (§6.2)
thc mv k3f9a --under 7hh2p [--after q81zz]
thc journal [2026-10-02]
thc link k3f9a 7hh2p --rel blocks

# search, query, time
thc search "passport"
thc q 'status:open (due<=+3d or priority:high) #work -#someday sort:due'
thc q 'status:done done_at>=-7d' --as-of "last tue"
thc history k3f9a | thc undo --last | thc log --by claude --since 1d

# system
thc doctor | thc conflict resolve k3f9a | thc export [--json]
thc daemon run|start|stop|restart|status|install|uninstall
thc mcp
```

**Query language (`thc q`):** one grammar shared by the CLI, the TUI filter bar, menu bar presets and MCP. It compiles to a single SQL query:

```
expr  := term (("and"|"or") term)*        ; implicit "and" between terms
term  := "-"? atom | "(" expr ")"
atom  := "status:" (open|todo|doing|waiting|done|cancelled|any)
       | "#"tag | "under:"id | "is:" (task|page|journal|inbox|event|repeating|conflict)
       | field op value | "text:"word | "sort:"field["-"]
field := scheduled|due|done_at|created|updated|priority|<custom prop>
op    := ":" | "=" | "<" | "<=" | ">" | ">="
value := ISO date | relative ("today", "+3d", "-1w", "fri", "eom") | literal
```

### 6.2 `$EDITOR` round-trip (`thc edit`)

`thc edit` renders a node and its subtree into a temporary file in a **canonical outline format** with trailing IDs:

```markdown
# Dentist                                                   ^7hh2p

Dr. Patel, 555-0100, office on 4th St. Parking behind the
building.                                                   ^a1b2c

- [ ] Call to reschedule due:2026-10-06 !high               ^k3f9a
    - [ ] Check insurance first                             ^q81zz
- Notes from last visit:                                    ^m4n5p
  multi-line text continues indented under its bullet
```

**Prose and bullets** (on a page or a journal day, the `#` header form):
- Lines without `- ` are **prose**. Consecutive prose lines form one paragraph, and so **one note**, with
  its lines joined by spaces (Markdown's rule: a single newline doesn't break a paragraph). A blank line
  ends the paragraph, so re-wrapping text in the editor never splits a note.
- A paragraph's `^id` goes at the end of its last line. A line ending in `\` keeps a line break inside
  the note (a Markdown hard break; the TUI's ⌃J).
- Top-level plain notes (no status, dates, priority or repeat, no children) render back as paragraphs,
  separated by blank lines. Everything else renders as `- ` bullets, as above, and bullets stay tight.
- No new node kind and no format change: a paragraph is an ordinary note.

When you save:
1. The file is parsed (it's *our* format, not arbitrary Markdown).
2. It's diffed against what was rendered, using the IDs to match lines.
3. The differences become ops:
   - Changed text → `node.text`.
   - Changed indentation or order → `node.move`.
   - Changed checkbox or `due:` → `node.set`.
   - A line with no ID → `node.create`.
   - A missing ID → `node.delete` (the delete count is reported).
4. If the file contains syntax errors, it is reopened with the error shown, and nothing is applied.

The rendered `base` is stored, so if someone else changed the same nodes in the meantime, the conflict rules (§4.3) apply.

### 6.3 TUI (`thc` with no args, or `thc tui`)

**Views:**
1. Today/Agenda (opens by default)
2. Inbox (triage: `m` move, `t` make a task, `d` set a date, `x` done, `D` delete)
3. Tasks (filter bar using `thc q`)
4. Pages (nucleo fuzzy finder, outline view with collapse and expand)
5. Journal (`[` and `]` move between days)
6. Search
7. History/Log

**Editing:**
- A page or a journal day opens as one plain-text document you type into (docs/guide/writing.md):
  each line is a node, `Tab`/`S-Tab` nest and un-nest it (fractional order means no renumbering),
  and a save turns the document's changes into one transaction of node ops.
- The editing engine is caretline (`crates/caretline`): the text, selection, motion, undo and the
  editing keys (its command catalog) are its own. thc adds tasks on its extension points (a tag
  per status, ⌃T as a host command, the box as a decoration) and keeps each line's node id and
  save state beside the engine (docs/caretline/embedding.md, the thc case study).
- `e` opens `$EDITOR` on the subtree (§6.2).

**Live updates:** subscribes to the daemon, so changes from agents or other devices appear immediately. Without the daemon, it runs its own log watcher in-process.

### 6.4 Daemon (`thc daemon`)

**Responsibilities:**
1. Watch `log/` (FSEvents or inotify via `notify`, debounced ~100 ms) and replay new lines.
2. Run a safety-net rescan every 5 minutes and on wake from sleep.
3. Act as the single writer for local ops.
4. Push events: `changed {ids}`, `agenda {snapshot}`, `alert.fire {alert,node}`, `conflict {id}`.
5. **Alert scheduler:** a timer on the next `alerts.fire_at` that emits `alert.fire`. Clients show the notification: the Mac app uses `UNUserNotificationCenter`, Linux uses `notify-rust`. If no client acknowledges, it falls back to the OS notification itself.
6. Regenerate the export (debounced) and ingest `drop/`.

**IPC:**
- JSON-RPC 2.0, newline-delimited, over `~/Library/Application Support/thought/thc.sock` (macOS) or `$XDG_RUNTIME_DIR/thought/thc.sock` (Linux), mode `0600`. No TCP listener.
- Methods mirror the CLI one-to-one: `query`, `agenda`, `search`, `show`, `apply {ops}`, `undo`, `subscribe`, `status`.

**Install:** a launchd LaunchAgent (`thc daemon install`, or registered by the Mac app through `SMAppService`), or a `systemd --user` unit on Linux.

**Reinstalls and updates:** an install renames a new binary over the old one, which keeps the path (and the version, for a rebuild), so a running daemon can't be judged by those alone. The daemon records which file it started from (`exe_id`, `dev:inode`, in `status`); a daemon whose file has since been replaced, or that runs another binary than its login item names, is *stale*. `thc setup` (install.sh's last step), `thc daemon install`, `thc daemon start` and `thc update` restart a stale daemon through whatever runs it (launchd `kickstart -k`, `systemctl --user restart`, or a plain stop and spawn), wait for the new process to answer and report it. `thc daemon install` also points a login item that names another binary at this one. `thc daemon status --json` carries `stale` (why) and `thc doctor` lists it as an issue with the fix.

**As built (M2):**
- **Writes:** the CLI and TUI write directly under the device write lock (`flock` on `<cache>/write.lock`, plus `BEGIN IMMEDIATE`). They never route writes through the socket, so they never depend on the daemon. The daemon sees every append to the log, this device's included, and pushes `changed` within ~120 ms. The socket's write methods (`capture`, `complete`, `set`, `snooze`, `ack`, `undo`) exist for clients without file access, such as ThoughtBar, and take the same lock.
- **Socket:** `/tmp/thc-<uid>/<hash of cache dir>.sock`, mode 0600, because macOS caps socket paths at 104 bytes. Clients discover it through `<cache>/daemon.json`.

### 6.5 ThoughtBar.app (macOS menu bar)

- **Form:** a SwiftUI `MenuBarExtra(.window)`. The badge counts overdue items, items due or scheduled today, and fired unacknowledged alerts, each node once (the daemon computes it as `TodayPanel.badge`).
- **Panel sections:** Overdue · Today · Upcoming 7d · Inbox (count + latest 5). A checkbox completes an item (`apply node.complete`), plus snooze and acknowledge for alerts.
- **Quick-capture field:** sends the same inline syntax as `thc add`.
- **Clicking a row:** opens the user's terminal on `thc tui --focus <id>` (Ghostty, iTerm or Terminal). **The app has no editing UI.**
- **Data:**
  - It connects to the socket with `Network.framework` (`NWEndpoint.unix`), calls `subscribe(["agenda","alerts"])`, and renders the pushed snapshots.
  - It never touches files or the database, and never polls.
  - Swift `Codable` types are generated from `thc-proto`'s JSON Schema.
- **Packaging:**
  - The app bundles the `thc` binary and registers the daemon with `SMAppService.agent`. Launch at login uses `SMAppService.mainApp`.
  - It runs unsandboxed.
  - When the socket is missing it shows "daemon offline · Start".

### 6.6 Agents

- **CLI first.** Ship `AGENTS.md` and a Claude Code skill describing the commands, the query grammar, the ID rules and the exit codes.
- **`thc mcp`:** a stdio MCP server built with `rmcp` on the same `thc-core`. Tools: `query`, `today`, `show`, `add`, `todo`, `set`, `complete`, `move`, `search`, `history`. Each tool is annotated as read-only, destructive or idempotent.
- **Provenance:** every agent write records its `actor`. `thc log --by <name>` and `thc undo --tx` let you review and roll back what an agent did.
- **Token hygiene:**
  - `show --depth N` instead of whole-graph dumps.
  - `--fields` to trim output, and `--limit` defaults to 50.
  - Large text is truncated with `"truncated":true`, and `--full` disables that.

---

## 7. Code layout

```
crates/
  thc-core/       model, ops, validation, HLC, merge/replay, store (SQLite, FTS), NL date+recurrence
                  parsing, query compiler, outline (document ↔ block ops)
  thc-daemon/     watcher, socket server, subscriptions, alert scheduler, exporter, drop ingest
  thc-tui/        ratatui app; its document editor runs on caretline
  thc/            single binary: clap dispatch
  caretline/      the text-editing engine (no thc crate, ever)
  caretline-app/  caretline's own terminal editor (`caretline-cli`)
  caretline-mcp/  an MCP server for live caretline sessions
docs/  SPEC.md · FORMAT.md (event log contract, versioned) · caretline/ · guide/
```

**Crates:**
- `rusqlite` (`bundled`)
- `clap` (derive)
- `serde`, `serde_json`, `schemars`
- `jiff` (dates and time zones)
- `ulid`
- `fractional_index`
- `notify` + `notify-debouncer-full`
- `tokio` (daemon only)
- `ratatui` + `crossterm`
- `nucleo`
- `rmcp`
- `notify-rust` (Linux notifications)

---

## 8. Performance

> **Budgets are targets, not gates**. Feature
> work doesn't run benchmarks; only CI's guards hold a merge, and they're set at about ten times
> these targets to catch order-of-magnitude regressions (typing 40 ms p50, a day opening 50 ms,
> reads 60 ms mean on the runner). A dedicated performance pass brings numbers back to target.

**Measured on an Apple-silicon dev machine, average wall-clock time per run over 300 runs:**

| Process | ms |
|---|---|
| `/usr/bin/true` (floor for starting a process) | 1.23 |
| C hello world | 1.39 |
| **C: open SQLite + indexed query over 20k rows + print 50** | **2.32** |
| Swift hello world | 1.73 |
| Python hello world | 13.4 |
| Node hello world / Node + `node:sqlite` same query | 19.1 / 20.1 |

Rust has no runtime to start, so its startup matches C's. The expected target for a `thc` read is **about 2–3 ms**, most of which is the OS starting the process.

**Targets (mean wall-clock per process with `hyperfine`, on vaults from `scripts/gen-vault.sh`; 50k nodes ≈ 200k events):**

| Operation | Budget |
|---|---|
| `thc q …` returning up to ~100 results (direct read), at 50k nodes | < 5 ms |
| `thc today --json` / `thc agenda --json` | < 8 ms at 50k nodes, < 5 ms at 10k. Cost scales with the rows shown, not the vault: every lookup is indexed |
| Broad `thc q …` returning thousands of matches | no fixed budget: scales with matches (`q '#work'`, 5k hits at 50k: ~11 ms) |
| `thc add …` with daemon running (socket round trip + append + apply) | < 5 ms |
| `thc add …` without daemon (flock + append + fsync + apply) | < 10 ms |
| Remote event → applied → menu bar updated | < 250 ms after Dropbox delivers it |
| Full rebuild from log (200k events) | < 2 s |
| `--as-of` query | < 500 ms |
| Daemon idle | ~0% CPU, < 30 MB RSS |
| ThoughtBar idle (panel closed, daemon live) | < 1% CPU, ~60 MB. Measured at M4.3 (release, Apple-silicon dev machine): 0.0% CPU over 60 s, 15 MB physical footprint (`footprint`; RSS 77 MB counts shared system frameworks) |
| ThoughtBar: external write → panel updated | < 500 ms (checked by `swift run ThoughtBarChecks`) |

**How it's measured (revised 2026-10-04, after the read-path pass in 18714e8):**
- `scripts/gen-vault.sh <dir> 50000` builds a deterministic vault that reads like two lived-in years: each chunk is written at its own `THC_NOW`; old work is almost all done and open tasks cluster in the last weeks. At 50k: 50,557 nodes, 201,278 events, 301 open tasks, 85 overdue, 19 in Today.
- `hyperfine -N --warmup 20 --runs 300` on release builds, `THC_NOW=2026-10-03T10:00`. To compare builds, `scripts/bench-reads.sh <dir> 300 a=bin@cache b=bin@cache` interleaves runs so drift hits both.
- Measured on an Apple-silicon dev machine, mean / p95 ms:

| Command | 10k nodes | 50k nodes |
|---|---|---|
| `/usr/bin/true` | 0.72 / 0.77 | 0.78 / 0.93 |
| `thc --version` | — | 2.39 / 2.76 |
| `thc q title=zzzz` (fixed cost of a read) | 4.28 / 4.55 | 3.20 / 3.79 |
| `thc q 'status:open #work sort:due'` | 4.44 / 4.75 | 4.86 / 5.59 |
| `thc q 'status:open due<=+3d'` | — | 4.89 / 5.17 |
| `thc q is:ready` | 4.96 / 5.30 | 5.11 / 5.53 |
| `thc today --json` | 4.79 / 5.14 | 6.69 / 7.28 |
| `thc agenda --json` | 4.89 / 5.26 | 7.03 / 7.48 |

**Across vaults** (vaults-architecture.md §3; 0.9.27, release, Apple-silicon dev machine, `gen-vault.sh` 10k nodes
per vault, mean): budgets 1 vault < 5 ms, 3 vaults < 8 ms, 10 vaults < 15 ms.

| Command | ms |
|---|---|
| `thc q 'status:open sort:due' --limit 100` (one vault) | 3.6 |
| `thc q 'vault:(v0 or v1 or v2) status:open sort:due' --limit 100` | 5.7 |
| `thc q 'vault:* status:open sort:due' --limit 100` (10 vaults) | 12.2 |
| the same with `--json` | 13.4 |

Each vault opens its own store on its own thread (no tokio), and the rows merge by the
query's own ORDER BY in an in-memory table.

**Today across vaults** (0.9.28, same machine and vaults): `thc today --json` reads every
registered vault for a person: 14.3 ms at 10 vaults (an agent, one vault: 5.1 ms). The TUI's
first frame on a cross-vault Today: 70 ms at 10 vaults of 10k nodes with ~70 overdue each (about
700 rows drawn); a journal day: 23 ms.

- Where a read's time goes: process start ~0.8 ms; loading the binary ~1.6 ms beyond that (0.6 of it CoreFoundation/CoreServices, ~0.4 clap); SQLite's first touch of the store ~0.6 ms; then the query and output (~25 µs per JSON row).

**Parked optimisations** (measured, not done; pick up if the budgets tighten):
- **(a) Split the daemon into `thcd`** (~0.6 ms off every command and agent call). `thc` links CoreFoundation/CoreServices only through chrono's `iana-time-zone` fallback (used when neither `TZ` nor `/etc/localtime` gives a zone) and `notify`'s FSEvents in the daemon. Fix: a local stand-in for `iana-time-zone` that reads the `/etc/localtime` link (a workspace `[patch]`), and the daemon run loop in its own binary that `thc daemon run` execs. Touches the login item, the ThoughtBar bundle (a second helper) and the notifier's `thc tui` command. About half a day.
- **(c) Batch per-row lookups in listings** (~1.5 ms off `today` at 50k). `node_json` looks up `rev`, tags and props per node; load them for the whole row set in one query each. That plus (a) should bring `today` at 50k to ~4.6 ms.

**Rules for the hot path:**
- Reads never start tokio, never call the daemon, and never load config they don't need.
- Prepare one statement per command.
- Compare dates as ISO ranges (`due >= ?1 AND due < ?2`), never `substr(due,1,10)`, so indexes apply.
- Keep planner statistics fresh (`ANALYZE` after a rebuild, `PRAGMA optimize` after big syncs); without them SQLite can pick the wrong partial index.
- Write output through a single locked `BufWriter`.
- `exit` right after flushing.
- Before reading, the CLI checks the log file sizes against the stored cursors (a few `stat` calls, microseconds). It replays itself only when the daemon is down and new events arrived.

**Release profile:** `lto="fat"`, `codegen-units=1`, `panic="abort"`, `strip=true`. Sign the binary (ad-hoc signing is enough locally) so Gatekeeper doesn't add delay on first run.

---

### 8.1 The editor (tui-editor.md §10)

The keystroke rows below were measured before the editor moved onto caretline (0.10.0); on
caretline typing and paging on the 5,000-line page take about 1 ms a key (RELEASE_NOTES 0.10.0).
Re-measure with `scripts/bench-editor.sh` in the next performance pass.

Measured 2026-10-05 with `scripts/bench-editor.sh` (release build, Apple-silicon dev machine, 120×40): a scratch vault
with a 5,000-line page (nested items, a task every ninth line) and a 300-line journal day.
Times are app time from `THC_TUI_TRACE=1` (key read to frame written), not terminal paint.

| Measure | Budget | Measured |
|---|---|---|
| Keystroke, 5,000-line page (typing at the top and the middle) | p50 ≤ 4 ms, p99 ≤ 12 ms | p50 3.9–4.0 ms, p99 4.1–4.3 ms |
| Holding PgDn, 5,000-line page | 60 frames/s (≤ 16 ms) | p50 4.0 ms, p99 4.3 ms |
| Open a 5,000-line page, first frame | ≤ 100 ms | 32 ms on a compacted store; 70 ms right after a bulk import (see below) |
| Open a 300-line day / Today, first frame | — | 4 ms / 3 ms |
| Keystroke, 300-line day | p50 ≤ 4 ms | p50 0.5 ms, p99 0.6 ms |
| Paste 1,000 lines into the 5,000-line page, saved | never blocks typing | 1.0 s to save, done by the daemon's writer thread when it runs (inline without a daemon) |

What the pass changed (0.9.18):
- **Counts and the review badge** read the index, not every note: the TUI's start went from 64 ms
  to 3 ms at 600 open tasks (`count(*)`; `events_actor` and `open_by_status` indexes).
- **Placing a note** reads its two neighbours by index (`Store::neighbour_ords`) instead of every
  sibling: the 1,000-line paste went from 4.3 s to 1.0 s.
- **`thc j` / `thc p`** skip building Today first: the day opens in 4 ms, not 45.
- **Store compaction:** a store that grew row by row interleaves its tables' pages, and a cold
  start reads many of them (43 ms instead of 4 for the same query). The daemon runs `VACUUM`
  once a day on a quiet rescan (`Store::vacuum_if_due`), so a lived-in store stays laid out.
- **The open itself (0.9.20):** the subtree is computed once into a temp table the other lookups
  join (it was recomputed six times, 5 ms each), and the editor's render skips the per-block
  latest-event lookup it never uses (`render_for_editor`; daemon clients still get `rev`). The
  5,000-line open went from 74 to 32 ms compacted, and from 107 to 70 ms on a fresh import.
  Both are inside the budget, so the full render before the first frame stays: it's simpler
  than a viewport-first layout, which tui-editor.md §10 suggests if pages grow past this.

**Read regression guard (2026-10-06):** `scripts/bench-read-guard.py` uses
`gen-vault.sh` to create a disposable 50k-node/~200k-event vault, then measures
empty, tagged, due, ready and Today JSON reads. It checks that results are present,
interleaves commands after warmup, and reports mean/p50/p95/stddev and process floor.
CI runs it on macos-15 after the editor's release build (LTO off, 16 codegen units),
with a coarse **30 ms mean per command** ceiling. This catches large regressions;
it does not enforce the developer-machine budgets in §8.

The investigation on an Apple-silicon dev machine, fat-LTO release, used identical cache snapshots of the
existing scratch fixture (50,565 nodes/201,286 events, including eight benchmark-view
nodes; 301 open tasks). Interleaved 150-run comparisons found no measurable
0.9.54→0.9.56 regression. Stale statistics still described 1,349 nodes and one open
task, making the ready query scan all open blockers per candidate. Indexed
edge-first probes reduced ready process time **32.3→8.9 ms** (p95 33.5→9.9);
empty/tagged/due/Today were broadly unchanged in that run. SQL work fell from
~25 ms to <0.5 ms. A 60-run guard check with up to 100 query rows measured
empty 6.1, tagged 7.9, due 9.3, ready 9.8 and Today 11.3 ms means.
The process floor was 1.9 ms, versus the earlier SPEC sample's 0.78 ms; background
noise and startup prevent claiming the <5 ms absolute budget is met here.
Large local writes and syncs now refresh statistics with bounded analysis, and
row metadata reuses prepared statements; statement caching does not cache values.

## 9. Milestones (v1 = M0–M4)

| # | Deliverable | Done when |
|---|---|---|
| **M0** | `FORMAT.md` (event contract) + `thc-core` (ops, HLC, merge, replay) + `thc-store` | Property tests: replaying random event interleavings from N simulated devices converges to an identical SQLite state |
| **M1** | CLI: capture, todo, remind, done/skip, set, show, today/agenda, q, search, history/undo/log, `--json`, `$EDITOR` round-trip, export | Usable daily by you and your agents on one machine |
| **M2** | Daemon: watch/replay, socket, writer, subscriptions, alert scheduler, drop ingest, launchd/systemd install | Two machines syncing through Dropbox with no conflicts. Alerts fire |
| **M3** | TUI: all views, outline editing, live updates | You can live in it |
| **M4** | ThoughtBar.app: panel, complete, snooze, capture, notifications, bundled daemon | Daily-driver menu bar |
| M5 | MCP server, tag defaults (supertags), mirrors, ICS feed, conflict 3-way merge UI | v1.x |

---

## 10. Decisions (resolved 2026-10-03)

1. **Name:** `thc`, "thought control". The repo is thought-control.
2. **Default capture target:** `thc add`, `todo` and `remind` write to **today's journal**. `--inbox`, `--under <id>` and `--journal <date>` override it. The inbox is root nodes with no title (what `drop/` and `--inbox` produce).
3. **Vault location:** for now the vault lives in the repo as `./vault` (gitignored), found through `./.thc.toml`. Moving to Dropbox later means changing `vault = …` in `.thc.toml` or `~/.config/thought/config.toml`, or setting `THC_VAULT`. The local cache lives in `./.thc-cache` while developing.

## 11. Implementation status

| Spec item | State |
|---|---|
| Event log, HLC, total order, per-device files (§4.1–4.4) | done (`thc-core/src/{event,hlc,log}.rs`) |
| Merge rules: per-field LWW, text conflicts, cycle rejection, late-arrival rebuild (§4.3) | done. Covered by `tests/convergence.rs` (3 devices, random edits, random sync) |
| SQLite view, FTS5, history, as-of, undo via stored inverses (§5) | done (`store.rs`, `model.rs`, `vault.rs`) |
| Data model: nodes, edges, alerts, prop registry, recurrence modes (§3) | done |
| CLI (§6.1), query language, `$EDITOR` round-trip (§6.2), export (§4.5), `drop/` ingest | done |
| CLI: `thc schema [cmd]` (inputs from clap, outputs with shared `$defs`), `--fields a,b,c` (bare `--fields` lists them), `thc link/unlink … --rel` with `is:blocked`, `is:blocking`, `blocks:<id>`, `blocked-by:<id>`, `rel:<name>`, and `--key <idempotency-key>` (id derived from the key, idempotent on every device) | done (closed the M1 gap after M2) |
| TUI (§6.3) | **done (M3)** |
| Daemon, socket RPC, alert notifications, live export (§6.4) | **done (M2)**: watch/replay/push, protocol v1, alerts with per-device fired state, delivery handshake and OS fallback (terminal-notifier / osascript / notify-send), conflicts end to end, `thc daemon …`, live TUI. Verified with a simulated two-device sync shim; a real two-machine Dropbox run is still to do |
| Menu bar app (§6.5) | paused; not part of this repository |
| MCP server (§6.6) | M5 |

**Left after M3 (deliberately deferred):**
- Move picker "Tab under a node". Goes to M5 with the node-level picker.
- Tasks `c` columns, Log `o` raw ops, Search `n`/`N` in the preview, and more than 4 saved filters.

**M2 performance (release, Apple-silicon dev machine, ~5.6k events):**
- **Before the daemon:** `today --json` 4.60 ms, `q` 4.83 ms, `add` 8.63 ms.
- **After step 4, daemon down:** `today --json` 4.78 ms, `q` 4.76 ms, `add` 8.74 ms.
- **After step 4, daemon up:** `today --json` 4.26 ms, `q` 4.09 ms, `add` 8.31 ms.
- **Push latency** (CLI write to subscriber event): ~120 ms. Budget 250 ms.

**Alert fired state (M2 decision):** whether an alert has fired is **per-device local state** (table `alert_local` in the device's SQLite cache), never an event in the log. Firing is a side effect of a device's clock, so each device fires an alert at most once per `fire_at` value, and re-arms when `fire_at` changes (snooze, or a rescheduled anchor date). Ack, snooze and done are replicated ops, so acting on one device stops the others from firing once it syncs; if the other device already fired, it withdraws that notification silently (daemon.md §2.4a). `alert_local` survives `thc rebuild` and is not touched by `Store::apply`.

**Departures from the spec in the implementation:**
- The store lives in `thc-core` instead of a separate `thc-store` crate.
- Recurrence `next` dates are computed by the writer and stored in `node.complete` events, which keeps replay independent of clock and timezone.
- `--as-of` replays the local `events` table into an in-memory store. There are no snapshots yet.
- The measured release-build latencies on an Apple-silicon dev machine are:
  - `today --json`: 4.3 ms
  - `q`: 4.5 ms
  - `search`: 4–6 ms
  - `add` with fsync: about 8.7 ms
