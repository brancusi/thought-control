---
title: Capture from scripts and cron
who: a tinkerer
blurb: Your scripts write to the same notebook you do, safely, and you see it in the Log.
icon: terminal
order: 5
---

The nightly backup job fails about once a month. Until now it emailed you, and you missed it. Now it adds a task to your inbox instead, keyed by date, so a cron job that retries three times still leaves one task. A post-commit hook drops a line into the day's journal for every commit, so the day log writes itself. On Fridays a small script plans the release as one batch.

You see all of it the way you see an agent's work: signed, in the Log, and undoable.

## The commands

A script that's safe to re-run:

```sh
#!/bin/sh
export THC_ACTOR=backup
day=$(date +%F)
if ! restic backup ~/work >/tmp/backup.log 2>&1; then
  thc todo "Nightly backup failed, see /tmp/backup.log" \
    --inbox -p high -t ops --key "backup-$day"
fi
```

```sh
$ ./backup-check.sh
added jdxp1   [ ] Nightly backup failed, see /tmp/backup.log  !high
$ ./backup-check.sh
exists jdxp1   [ ] Nightly backup failed, see /tmp/backup.log  !high
```

The cron line:

```sh
15 3 * * *  ~/bin/backup-check.sh
```

A git hook, `.git/hooks/post-commit`:

```sh
#!/bin/sh
sha=$(git rev-parse --short HEAD)
THC_ACTOR=git thc add --plain --key "commit-$sha" \
  "$(git log -1 --format=%s) ($sha)" >/dev/null
```

`--plain` keeps a commit message's `#123` as text rather than a tag.

Read with `--json` and check exit codes:

```sh
$ thc q 'status:open #ops' --fields short,text,priority
$ thc set jdxp1 status=doing --expect status=todo || echo "someone got there first ($?)"
$ thc daemon status >/dev/null || echo "offline: alerts paused"     # exit 3 when it isn't running
```

A batch, all or nothing:

```sh
$ cat release.jsonl
{"cmd":"add","text":"Release 0.4","inbox":true,"as":"rel"}
{"cmd":"todo","text":"Tag the build due:thu","under":"$rel","key":"rel-0.4-tag"}
{"cmd":"todo","text":"Write notes !med","under":"$rel","key":"rel-0.4-notes"}
$ thc apply release.jsonl --dry-run
apply · 3 ops → 1 transaction (dry run)
  + add      Release 0.4  → inbox
  + todo     [ ] Tag the build  due Thu  → under Release 0.4
  + todo     [ ] Write notes  → under Release 0.4
3 ok · nothing written
$ thc apply release.jsonl
applied 3 ops · tx M86CBM · thc undo --tx M86CBM reverts all of it
```

When a script misbehaves:

```sh
$ thc log --by backup --since 1d
$ thc undo --by backup --since 1d
```

## Why thc fits

- **Idempotent by design.** `--key` gives the same note on every run and every device, so retries and double-fired crons are harmless.
- **A real contract.** `--json` output only ever grows, exit codes mean one thing each, and `thc schema <cmd>` describes them.
- **Scripts are actors too.** `THC_ACTOR` signs every write, so a noisy script is one `thc undo --by` away from gone.
