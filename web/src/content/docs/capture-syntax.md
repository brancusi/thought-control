---
title: Capture syntax
description: The inline tokens that turn a line of text into a task with dates, a repeat, a priority, tags and links, and every date thc can read.
order: 21
group: Reference
---

Write a line the way you'd say it. `thc` reads the tokens in it once, when you write it, and
turns them into fields. The same syntax works in the editor, in `thc add`, `thc todo` and
`thc remind`, and in `thc apply`'s `text`.

```sh
$ thc parse 'Book the venue due:fri sched:mon #offsite !high every:2w [[Offsite]]'
→ [ ] Book the venue #offsite [[Offsite]] · sched 2026-10-05 · due 2026-10-09 · !high · ↻ every 2 weeks
```

`thc parse` previews a capture and writes nothing. With `--json` it shows the fields:

```json
{"text":"Book the venue #offsite","kind":"task","status":"todo","scheduled":"2026-10-05",
 "due":"2026-10-09","priority":"high","tags":["offsite"],"links":[],"bad":[],"bad_why":[]}
```

## Tokens

| Token | Sets |
|---|---|
| `[ ]` at the start, or `todo:` | An open task |
| `[x]` at the start | A done task |
| `due:fri` `due:2026-10-10` `due:"nov 1 9am"` | The hard deadline. Past it, the task is overdue |
| `sched:mon` | The planned or start date. The task shows in Today from then |
| `at:"tue 2pm"` | A time, with no task box: an event |
| `every:2w` `every:weekday` `every!:1mo` | A repeat. With `!` the next date counts from when you finish |
| `!high` `!med` `!low` | Priority |
| `#tag` | A tag. It stays in the text where you wrote it |
| `[[Page Title]]` | A link. If the page doesn't exist, a page is made |

The tokens are taken out of the text (except tags and links, which stay), so the line reads
cleanly: `Book the venue #offsite [[Offsite]]`. A date or a repeat makes the line a task;
`at:` on its own makes an event.

Values with spaces go in quotes: `due:"nov 1 9am"`, `at:"tue 2pm"`. A `## Heading` is a heading,
not a tag.

## Quoted text is never parsed

Inside "double quotes" or `backticks`, nothing is a token: not `!high`, not `#tag`, not `due:`,
not `[[…]]`. That's how you write *about* the syntax:

```sh
$ thc parse 'Write "due:fri" in quotes, `#tag` too'
→ Write "due:fri" in quotes, `#tag` too
```

A quoted *value* (`due:"nov 1 9am"`) is still a token. To mention a token with a quoted value,
put the whole token in backticks. Code blocks are never parsed either.

**`--plain`** keeps a capture exactly as written, with no tokens and no tags. Use it for notes
that quote a lot of syntax:

```sh
$ thc add --plain 'The filter was #work !high due<=+3d and it matched nothing'
```

With `--plain`, `[[…]]` stays text too.

## Two of the same token

One line can only have one priority, one due date, and so on. If it has two
(`!high … !low`, `due:fri … due:mon`):

- **On the command line,** thc refuses and asks you to keep one, or quote them:

  ```sh
  $ thc add 'Ship it !high and !low'
  thc: invalid: two priorities (!high, !low) · keep one, or quote them to keep them as text
  ```

- **While you write,** the last one counts. The earlier one stays as plain words, and the hint
  says `2 priorities · using !low`.

## When a value doesn't parse

On the command line, a token whose value can't be read (`due:fryday`) is refused with a hint,
and nothing is written:

```
thc: can't read "fryday" as a date · try fri, +3d or 2026-10-09
```

In the editor and the capture box, it stays in the line as plain text, and the right-hand
column says what it couldn't read. You never lose what you typed.

## Dates

| Write | Means |
|---|---|
| `today` `tomorrow` `yesterday` | Those days (`tod`, `tom`, `tmrw` work too) |
| `fri`, `friday` | The coming Friday (today, if it's Friday) |
| `next fri` | Friday of next week |
| `next week`, `next month` | The Monday of next week, the first of next month |
| `+3d` `-1w` `+2mo` | Days, weeks or months from today |
| `in 2 weeks`, `in 3 days`, `in 1 month` | The same, in words |
| `eow` `eom` `eoy` | End of the week, month or year |
| `nov 1`, `1 nov`, `november 1st` | The next November 1st |
| `nov 1 2027`, `2026-10-10`, `10/10` | A specific date |
| `2026-10-10T14:00` | A date and time |
| `+2h` `+30min` | A time from now |

**Times** go after the date: `fri 9am`, `nov 1 14:30`, `tomorrow at 2:30pm`, `fri noon`. A time
alone (`9am`) means today.

**Minutes or months?** A bare `m` is ambiguous, so thc won't guess: write `+30min` for minutes
and `+2mo` for months. On the command line, `+30m` is refused with that hint.

## Repeats

| Write | Repeats |
|---|---|
| `every:day` | Every day |
| `every:2w` | Every two weeks |
| `every:weekday` | Monday to Friday |
| `every:mon,thu` | On those days |
| `every:month` | Monthly |
| `every:"month on the 1st"` | On the 1st of each month |
| `every!:3mo` | Three months **after you finish**, not after the last date |

Completing a repeating task doesn't close it: its dates move to the next occurrence, its
subtasks reopen, and the log keeps a record of each one you finished. `thc skip <id>` skips one
occurrence. On the command line, `--repeat "every 2w"` does the same as the token.

## Where a capture goes

| Command | Lands |
|---|---|
| `thc add "…"` | Today's journal |
| `thc add --inbox "…"` | The inbox, to triage later |
| `thc add --journal fri "…"` | That day's journal |
| `thc add --under <id> "…"` | Under that note or page |

A vault can send captures to a page instead of the journal (`[capture] target = "¶ Issues"` in
its `settings.toml`). Naming a place still wins. See [Vaults](../vaults/).

`thc todo` and `thc remind` take the same tokens, and flags for the same fields:

```sh
$ thc todo "Draft plan" --due fri --scheduled mon -t work -p high --repeat "every 2w"
$ thc remind "Pay rent" --at "nov 1 9am" --repeat "every month on the 1st"
$ thc alert add k7q2m --before 1d          # a reminder a day before it's due
```

A reminder is a task with a time and an alert at that time; `--no-task` makes it a plain dated
note instead.
