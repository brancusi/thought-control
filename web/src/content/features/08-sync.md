---
title: Sync
blurb: Any synced folder works. Every device writes its own file, so nothing collides.
icon: sync
order: 8
---

thc has no server. A vault is a folder, so sync it with whatever you already use: Dropbox, iCloud Drive, Syncthing, a network share.

```
vault/
  log/<device>/2026-10.jsonl    # each device appends only to its own files
  files/2026/10/…               # attachments, written once
  settings.toml                 # the vault's shared settings
```

- **One writer per file.** Each device appends only to its own log, so a sync tool never sees two machines editing the same file, and never makes a "conflicted copy".
- **Replay is deterministic.** Every device orders the events the same way and arrives at the same notes, whatever order they synced in.
- **The same day, tag or page made on two machines** before they sync becomes one afterwards, with everything from both.
- **Retry-safe ids:** a capture with `--key` gets the same id on every device.

**Conflicts are kept, never lost.** If two devices change the same note's text before syncing, both versions stay and the note is marked `≠`. Today shows a banner (`≠ 2 conflicts`), and the line says who else changed it (`≠ claude`, or the other device).

| Key | In the compare panel |
|---|---|
| <kbd>c</kbd> or click the `≠` | Compare, yours first |
| <kbd>1</kbd> | Keep yours |
| <kbd>2</kbd> | Keep theirs |
| <kbd>b</kbd> | Keep both |
| <kbd>e</kbd> | Merge them in `$EDITOR` |
| <kbd>Esc</kbd> | Later |

- **From the CLI:** `thc conflict ls`, `thc q is:conflict`, and `thc conflict resolve <id> --keep current|other|both`.
- **Moves that would loop** (each device moved a note under the other) are skipped and noted. Nothing to do.
- **Rehomed lines:** a line whose parent was deleted on another device moves to where you can see it, flagged. Keep it here or delete it.
- **Live on every device:** with the background service running, another machine's changes appear as the sync tool delivers them.
