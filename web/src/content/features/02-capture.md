---
title: Capture
blurb: One line from any shell. Dates, tags and priority parsed on the way in.
icon: capture
order: 2
---

Everything you can type in the editor, you can type on the command line.

```sh
$ thc add "Standup: auth refactor is close #work"
added 5ghwy   Standup: auth refactor is close #work  in today's journal
$ thc todo "Fix flaky login test" --due fri -p high -t work
added 750tz   [ ] Fix flaky login test  due fri · !high · in today's journal
$ thc remind "Ship release notes" --at "fri 4pm"
reminder pd0s6   [ ] Ship release notes  fri 16:00 · in today's journal
```

- **`thc add`** writes to today's journal. **`thc todo`** makes a task (`--due`, `--sched`, `-p`, `-t`, `--repeat`). **`thc remind`** makes a task with a time and an alert at that time.
- **Pick the place:** `--inbox`, `--under <id>`, `--journal <date>`. Otherwise it lands in today's journal, or wherever the vault's `[capture] target = "¶ Issues"` in `settings.toml` says.

| Token | Means |
|---|---|
| `due:fri` `due:"nov 1 9am"` | A hard deadline |
| `sched:mon` | When you plan to start |
| `at:"tue 2pm"` | An event at a time |
| `every:2w` `every:weekday` `every!:1mo` | A repeat (`!` counts from completion) |
| `!high` `!med` `!low` | Priority |
| `#tag` | A tag, kept in the text |
| `[[Page]]` | A link; a new name makes the page |
| `[ ]` `[x]` `todo:` | A status |

- **Dates read like you'd say them:** `today`, `tomorrow`, `fri`, `next fri`, `+3d`, `-1w`, `in 2 weeks`, `eom`, `nov 1`, `2026-10-10T14:00`, `+2h`, `+30min`, with times like `9am`, `14:30`, `noon`. A bare `m` is refused as ambiguous: write `+30min` or `+2mo`.
- **Quotes keep text literal.** `thc add 'Write up "due:" rules'` saves the words. `--plain` keeps `#words` and `[[…]]` as text too.
- **A date that doesn't parse** (`due:fryday`) is refused with a hint: `try fri, +3d or 2026-10-09`.
- **Retry-safe:** `--key <your-key>` maps to the same note on every device; the second run prints `exists` and writes nothing.
- **`thc parse "…"`** previews what a capture would save, and writes nothing:

```sh
$ thc parse 'Call the venue due:"fri 3pm" !high #offsite every:2w'
→ [ ] Call the venue #offsite · due 2026-10-09T15:00 · !high · ↻ every 2 weeks
```

- **Outlines:** `thc import - --page "Offsite"` reads indented `- item` lines (capture syntax works) as one transaction.
- **The drop folder:** files in `vault/drop/` become inbox items with `thc ingest`.
- **In the TUI,** <kbd>a</kbd> captures where you are and <kbd>A</kbd> to the inbox; <kbd>Tab</kbd> cycles the target.
