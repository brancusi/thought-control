# Event log format (v2)

This is the on-disk contract. The SQLite store, the export and every client are derived
from it. Change it only with a version bump.

**Versions.** v2 (2026-10) is an additive bump over v1: it adds `tx.review` and
`tx.unreview`, and nothing else changes. Every event carries the lowest version that
understands it, so events a v2 writer emits for v1 ops still say `"v":1`.

## Layout

```
<vault>/
  thc-vault.toml            # marker: format, and the vault's identity (below)
  log/<device>/<YYYY-MM>.jsonl
  drop/                     # inbound files → inbox items (consumed by `thc ingest`)
  files/<YYYY>/<MM>/<hash>-<name>.<ext>   # attachments (attachments.md), written once
  export/                   # generated Markdown; never read back
```

- **Attachments** are files, not ops. A note refers to one with a Markdown image in its text,
  `![caption](files/2026/10/k3m9q-shot.png)`. The path is relative to the vault. `<hash>` is
  five base32 characters of the content's FNV-1a hash, and `<name>` is the original file name
  made lowercase with dashes. The same bytes under the same name land on the same path, and a
  file there is never edited. `files/.orphans/` holds files `thc doctor --fix` moved aside, and
  readers ignore it. No format version change: it's text plus files, synced like the log.

- **The marker** holds `format`, and since vaults (vaults.md §1) the vault's synced identity:
  `id` (12 characters, written once and never changed), `name` (lowercase letters, digits and
  `-`; renamed by `thc vault rename`) and `created` (a date). A vault from before vaults gets
  them on first open; its `id` is `from_key("vault:" + the oldest event id)`, so devices that
  share the vault derive the same one. Readers ignore unknown keys. None of this is in the log.
- **Reserved, not yet written:** `vault:<vault-id>/<node-id>` as an `edge.add` `dst`, and
  `[[vault:…]]` in text, for links across vaults. Today's readers keep both as plain
  values and text.
- A device appends **only** to `log/<its-device-id>/`. The month in a file name is the UTC
  month of the event's HLC timestamp.
- Readers ignore any trailing partial line. It is read once the newline arrives.
- File names containing a space (sync-tool "conflicted copy" artefacts) are ignored.
- **Readers ignore unknown ops.** A well-formed line whose `op` the reader doesn't know, or
  whose `v` is above the reader's version, is skipped silently (it's from a newer writer,
  not damage). A reader records the format version it was built with; after an upgrade it
  re-materializes once, so ops it used to skip are applied.

### A live node under a deleted parent

A `node.delete` tombstones the node and the descendants its writer knew. A child another device
added meanwhile stays live under a deleted parent. **Readers show it under its nearest live
ancestor** (or the inbox, when every ancestor is deleted) **and flag it** as a `rehomed`
conflict until a person decides: a `node.move` to where it shows keeps it, a `node.delete`
deletes it, and restoring the parent moves it back. This is derived from the events (no op
records it), so every device shows the same thing. No version bump: older readers simply
don't show such a node.

## Envelope

One JSON object per line:

```json
{"v":1,"eid":"01K6…","hlc":[1759500000123,0],"dev":"mbp-7f3a",
 "actor":{"kind":"agent","name":"claude"},"via":"cli","tx":"01K6…",
 "op":"node.set","id":"k3f9a2mq7x1c","props":{"due":"2026-10-06"}}
```

| Field | Meaning |
|---|---|
| `v` | The lowest format version that understands this event (1, or 2 for `tx.*` ops) |
| `eid` | Event ID (ULID), globally unique |
| `hlc` | Hybrid logical clock `[unix_ms, counter]` |
| `dev` | Device ID (also the log directory name) |
| `actor` | `{kind: "human"}` or `{kind: "agent", name}` |
| `via` | `cli`, `tui`, `bar`, `mcp`, `edit`, `drop`, … |
| `tx` | Transaction ID. All events from one command share it, and undo works per transaction |
| `op` | Operation name. The op's fields are flattened into the envelope |

**Total order:** `(hlc.ms, hlc.counter, dev, eid)`, compared as the string
`format!("{ms:013}.{counter:06}.{dev}.{eid}")`. Every device replays in this order.

## Operations

| op | fields | notes |
|---|---|---|
| `node.create` | `id, parent, order, text, title?, props?` | `order` is a base-62 fractional index |
| `node.text` | `id, text, base?` | `base` = the `eid` of the text this edit started from |
| `node.set` | `id, props{key: value\|null}` | `null` unsets. Built-ins: `title status scheduled due priority repeat done_at journal tag style gap`; attachment nodes add `system kind path mime bytes w h` |
| `node.move` | `id, parent, order` | Rejected (and recorded as a conflict) if it would create a cycle |
| `node.complete` | `id, at, occurrence?, next?` | With `next{scheduled?,due?}` the node repeats: dates advance, status stays `todo` |
| `node.skip` | `id, occurrence, next` | Skip one occurrence of a repeating node |
| `node.delete` / `node.restore` | `id` | Soft delete |
| `edge.add` / `edge.remove` | `src, rel, dst` | `rel`: `tag`, `mention`, `embed` (a note shows an attachment), `blocks`, `relates`, … |
| `alert.add` | `id, node, trigger{at? \| offset+anchor}` | `offset` is a signed duration like `-1440m`, where **`m` means minutes** (this is the stored format, read at replay forever; user input spells minutes `min` and months `mo`, and rejects a bare `m`). `anchor` is `due` or `scheduled` |
| `alert.ack` / `alert.remove` | `id` | |
| `alert.snooze` | `id, until` | |
| `prop.define` | `key, type` | `number`, `bool`, `date` or `text`. The first definition wins |
| `tx.review` (v2) | `txs[], verdict` | A verdict on other transactions: `accepted` or `reverted`. Changes no node |
| `tx.unreview` (v2) | `txs[]` | Withdraws the verdict, putting the transactions back in the review queue |

**IDs** are 60 random bits written as 12 characters of Crockford base32 (`0-9a-z` without
`i l o u`).

**Keyed IDs.** A node every device would create the same way gets the same id everywhere:
FNV-1a 64 over `thc-key:<key>`, folded to 60 bits (`id::from_key`). Then two devices that
create it offline make one node, because replay keeps the first `node.create` of an id. The
keys are:
- a journal day's root: `journal:2026-10-04`
- a tag: `tag:<name, lower-cased>`
- a page made by title (`thc page new`, a `[[link]]`): `page:<title, lower-cased and trimmed>`
- the saved-views page and the built-in views (`views.rs`)
- an agent's `--key`

A writer uses the keyed id only when it's free. If a node with that id already exists, even
deleted, it makes a random id instead. That covers two cases:
- **Re-creating after a rename.** A page keeps its id when it's renamed. A new page with the
  old title finds the keyed id taken and gets a random one. It never reuses the renamed page.
- **Re-creating after a delete.** A deleted day or tag likewise isn't revived by a new create.

Logs from before keyed ids may hold two roots for one day or two nodes for one tag.
`thc doctor --fix` merges them (moves and soft deletes, one transaction).

**`style`** is how a line reads (editor.md §3.1). It's absent for a bullet (the default), and
`para` for a paragraph. The other line forms live in the text itself, so every reader sees
them:

| Form | Stored as |
|---|---|
| Bullet | no `style` |
| Paragraph | `style = "para"` |
| Task | `status` set (`todo`, `doing`, `waiting`, `done`, `cancelled`) |
| Heading | a paragraph whose text starts `# `, `## ` or `### ` |
| Quote | a paragraph whose text starts `> ` |
| Numbered item | a bullet whose text starts `1. ` (any number, then `. ` or `) `) |
| Code block | a paragraph whose text starts with three backticks |
| Rule | a paragraph whose text is `---` or `***` |

**`gap`** says whether a blank line comes before a note (writing.md §1). It's the string `"1"`
(a blank line) or `"0"` (none), and absent means the default for its kind: a blank line next to
a paragraph, none between list items. Paragraph after paragraph always reads as `1`. It's an
ordinary prop, so it merges per field and replays like any other, and readers that don't know it
render the default; no format version change. Markdown writes a blank line where it's 1, and
`thc edit` and `thc import` read one back as 1.

**Attachments**. A file a note shows, `![caption](files/…)` in its text, is
also its own node, so it can be queried, described and found again:

- **The node:** a root node with no parent and empty text. Its id is keyed from the file's path,
  `id::from_key("file:" + path)`, so the same file is the same node on every device, even when
  two make it offline. Its title is the first caption it was given (else the file name). Props:
  `system = "attachment"` (so Pages, the finder and queries skip it, as they skip ¶ Views),
  `kind` (`image` or `file`), `path` (relative to the vault), `mime`, and when known `bytes`,
  `w` and `h`.
- **The edge:** `embed`, from the note to the node, derived from the note's text and kept in
  step with it the way `mention` is. Removing the reference or the note removes it.
- **The text is unchanged.** It stays the portable Markdown it always was. A reader that doesn't
  know attachment nodes reads the text as before and, like every released reader, hides nodes
  with a `system` prop. So there are no new op types and no format version change.
- **Existing vaults:** the first write after the upgrade (and the daemon at start, and
  `thc doctor --fix`) gives the notes that lack them their nodes and edges, in one transaction
  by the agent `thc`, undoable as a whole. It never changes any note's text, and with nothing to
  do it writes nothing. `thc doctor` reports a note whose text and embeds disagree.

**Reserved: cross-vault links.** `vault:<vault-id>/<node-id>` (as an `edge.add` `dst`, and as
`[[vault:…]]` in text) is reserved for links between vaults. No writer
produces it yet, and readers keep it as plain text until a format version defines it.

**Dates** are `YYYY-MM-DD` or floating local `YYYY-MM-DDTHH:MM`.

**`repeat`** is `{rule, mode, text}`:
- `rule` is an RFC 5545 RRULE subset (`FREQ`, `INTERVAL`, `BYDAY`, `BYMONTHDAY`).
- `mode` is `fixed`, `catch_up` or `from_done`.

## Merge rules

- **Per-field last-writer-wins.** For each `(entity, field)`, the event latest in the total
  order wins. Fields are: position (parent and order together), `text`, each property,
  deleted, and each `edge:<rel>:<dst>`.
- **Concurrent text edits.** Two `node.text` events with the same non-null `base` are
  concurrent. The later one wins, and the other is stored as a conflict. An edit made from
  the current text resolves open conflicts on that node.
- **Recurrence is computed by the writer.** Next dates go into the event, so replay never
  depends on the replaying device's clock or timezone.
- **Review verdicts** (v2). Each reviewed transaction has one verdict field, last-writer-wins
  like any other: `tx.review` sets it, `tx.unreview` clears it. `accepted` counts only when
  the actor is a human; an agent's `accepted` is ignored on replay (the writer refuses it
  too). `reverted` is valid from anyone: an undo of a transaction's changes carries a
  `tx.review … reverted` for it, and undoing that undo withdraws it. A transaction is in the
  review queue when its actor is an agent, it has no verdict, and it holds no `tx.*` ops.
- **v1 readers and v2 logs.** A v1 binary reports `tx.*` lines as unreadable (`thc doctor`)
  and otherwise ignores them: node state is identical, it just has no review queue. Upgrade
  every device to v2 to share verdicts.
- **Sibling order ties.** Two devices appending offline can give siblings the same `order`.
  Siblings sort by `(order, id)`, so every reader agrees. A writer placing a node between two
  equal keys gets a key just after them (nothing fits between equal keys), never an error.
- **Late arrivals.** If an event arrives that sorts before already-applied events, the
  reader re-materializes from scratch. That's simple, correct, and fast at personal scale.

## Block ops (writers, not the log)

Block ops are how the Mac editor (and `$EDITOR`'s round trip) write a document: `edit`,
`create`, `move`, `delete`, `kind`, `status` (`thc_core::outline`, mac-editor-arch.md §3). They
are a **writer's API, not a log format**. Each becomes the ordinary operations above, so they
add no op types and no format version. One save is one transaction. What a writer guarantees:

- **Edits carry their base.** An `edit` names the `text_rev` (the `eid` of the text) it started
  from, and that becomes the `node.text` event's `base`. If someone else changed the text in
  between, replay records an ordinary text conflict: both versions are kept, the later write
  shows, and every reader sees the same `≠`. Nothing is held back or lost.
- **Edits to deleted nodes restore them.** A `node.restore` comes first, so the node keeps its
  id and history.
- **Placement.** `create` and `move` take `parent` (default: the document root) and `after`, the
  sibling to follow; without `after` the node goes first among its siblings.
- **Client-made ids.** `create` takes an id made by the client (the alphabet and length above,
  60 random bits). A create whose id already exists is not written and is reported as `exists`,
  so a retried save never makes a second node. Two offline devices that somehow made the same
  id converge like any other concurrent `node.create`.
- **Per-op stale.** `move`, `delete`, `kind` and `status` may carry `rev` (the node's latest
  `eid`). If it's out of date, that op is left out of the transaction and reported `stale` with
  the node's current state. The other ops in the save are still written.
