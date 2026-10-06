---
title: Query language
description: Every term thc q understands, from status and dates to ancestors, relations, sorting, grouping and vaults.
order: 20
group: Reference
---

One small language finds things everywhere: `thc q` on the command line, the Tasks filter
(<kbd>f</kbd>), saved views, contexts, and the sections of Today. It compiles to a single SQL
query over the local store, so it answers in well under a millisecond.

```sh
$ thc q 'status:open due<=+7d sort:due'
edx71   [ ] Summarize unread newsletters into [[Reading List]]  due tomorrow · ◆ claude · in today's journal
ykwkc   [ ] Collect last quarter's metrics  due thu · in Draft Q4 OKRs
ny82x   [ ] Call dentist to reschedule #health  due fri · !high · in today's journal
pa3e6   [ ] Review draft with the team  due sun · in Draft Q4 OKRs
fk5x2   [ ] Draft Q4 OKRs  today · due Oct 12 (in 7d) · !high · in Q4 Planning
```

Quote the query, so your shell leaves `!`, `#`, `<` and parentheses alone. The examples on
this page come from a sample vault.

## Combining terms

| Write | Means |
|---|---|
| `a b` | Both. Terms are joined by AND unless you say otherwise. `and` is allowed but optional |
| `a or b` | Either |
| `( … )` | A group |
| `-a`, `-( … )` | Not: a leading `-` negates a term or a group |

AND binds tighter than OR, so `#work !high or #urgent` means `(#work !high) or #urgent`.

```sh
$ thc q 'status:open (due<=+3d or !high) -#someday sort:due' --explain
query    status:open (due<=+3d or !high) -#someday sort:due
means    open tasks
         and (due on or before Thu Oct 8, or priority high)
         and not tagged #someday
         sorted by due date, undated last
matches  4 · 0.1 ms
```

A term the language doesn't know is an error, never a silent empty list. It exits `6` with a
suggestion:

```sh
$ thc q 'status:opn'
thc: invalid: unknown status "opn" · did you mean open?
```

## Status

| Term | Matches |
|---|---|
| `status:open` | `todo`, `doing` or `waiting` |
| `status:closed` | `done` or `cancelled` |
| `status:todo` `status:doing` `status:waiting` `status:done` `status:cancelled` | Exactly that status |
| `status:any` | Any task (anything with a status) |
| `status:none` | Not a task |
| `status!=done` | Any status but that one |

A task isn't a kind of note: it's any note with a status. Remove the status and it's plain text
again.

## Dates

Compare a date field with `=` (or `:`), `<`, `<=`, `>`, `>=` or `!=`, against any
[date form](../capture-syntax/#dates): `today`, `fri`, `+3d`, `-1w`, `eom`, `nov 1`,
`2026-10-10`. Comparisons are by day, so `due=fri` matches anything due on Friday, at any time.

| Field | Is |
|---|---|
| `due` | The hard deadline |
| `sched`, `scheduled` | The planned or start date |
| `done`, `done_at` | When it was completed |
| `created` | When the note was made |
| `updated` | When it last changed |
| `journal` | The journal day a note sits in: `journal=today`, `journal>=-7d` |
| `happens` | Any of: scheduled, due, or an alert's time |

| Term | Matches |
|---|---|
| `due<=+3d` | Due in the next three days, or overdue |
| `due=today` | Due today |
| `sched<=today` | Scheduled for today or earlier: ready to start |
| `done>=-7d` | Finished in the last week |
| `created>=-1w` | Made in the last week |
| `happens<=+7d` | Anything with a date in the next week (scheduled, due or an alert) |
| `due:none`, `sched:none`, `happens:none` | No such date |
| `due:any`, `sched:any`, `happens:any` | Has one |

```sh
$ thc q 'happens<=+7d -is:task'
qz8ze   Dentist appointment [[Health]]  tomorrow 14:00 · in today's journal
```

## Tags, priority and text

| Term | Matches |
|---|---|
| `#work` | Tagged `work` |
| `-#someday` | Not tagged `someday` |
| `!high` `!med` `!low` | That priority. The long form is `priority:high` (or `prio:high`); `priority!=low` works too |
| `lisbon` | A bare word: in the text or the title |
| `text:lisbon` | The same, said explicitly |
| `"lisbon flat"` | A phrase |
| `title:"Q4 Planning"` | A title, exactly (case doesn't matter) |

## Kinds: `is:`

| Term | Matches |
|---|---|
| `is:task` | Anything with a status |
| `is:page` | Pages |
| `is:journal` | Journal days |
| `is:inbox` | Inbox items: top-level, no title, not a day |
| `is:event` | A note with a time and no task status (`at:"tue 2pm"`) |
| `is:repeating` | Has a repeat |
| `is:overdue` | Open, and due before today |
| `is:alert` | Has an alert (a reminder) |
| `is:conflict` | Has an open conflict: two versions waiting for you |
| `is:blocked` | Something open blocks it |
| `is:blocking` | It blocks something open |
| `is:ready` | The next thing to pick up: `todo` or `doing`, scheduled for today or earlier (or not scheduled), and nothing open blocking it |
| `is:to-review` | An agent marked it done and you haven't accepted that yet |
| `is:image` / `is:file` | An attachment: a picture, or any other file |
| `is:attachment` | Either |

## Where it lives

Each of these takes an id (any unique prefix) or a page title. Quote a title with spaces.

| Term | Matches |
|---|---|
| `under:<page>`, `in:<page>` | Anywhere under it, at any depth |
| `parent:<page>` | Directly under it |
| `ancestor:( … )` | Under something that matches the inner query |
| `has:child` | Has children |
| `has:child( … )` | Has a child that matches the inner query |

```sh
$ thc q 'under:"Q4 Planning"'
fk5x2   [ ] Draft Q4 OKRs  today · due Oct 12 (in 7d) · !high · in Q4 Planning
ykwkc   [ ] Collect last quarter's metrics  due thu · in Draft Q4 OKRs
pa3e6   [ ] Review draft with the team  due sun · in Draft Q4 OKRs
584x8   Theme for the quarter: fewer, deeper bets  in Q4 Planning

$ thc q 'ancestor:(#work) status:open sort:due'
ykwkc   [ ] Collect last quarter's metrics  due thu · in Draft Q4 OKRs
pa3e6   [ ] Review draft with the team  due sun · in Draft Q4 OKRs
fk5x2   [ ] Draft Q4 OKRs  today · due Oct 12 (in 7d) · !high · in Q4 Planning
```

`ancestor:(#work)` finds every open task anywhere inside a page tagged `#work`, however deep.
`has:child(status:open)` finds the projects that still have something open.

## Relations and people

| Term | Matches |
|---|---|
| `blocks:<id>` | Blocks that note (`thc link a b --rel blocks`: a blocks b) |
| `blocked-by:<id>` | Blocked by that note |
| `embeds:<id>` | The notes that show that attachment |
| `rel:<name>` | Has a link of that kind: `rel:relates`, or one of your own |
| `by:claude` | Made by that agent. `by:human` (or `by:me`) for you |

```sh
$ thc q 'by:claude'
edx71   [ ] Summarize unread newsletters into [[Reading List]]  due tomorrow · ◆ claude · in today's journal
```

## Your own fields

`thc set <id> client=acme` adds a field; its type is fixed on first use (text, number, date or
true/false). Query it by name:

| Term | Matches |
|---|---|
| `client=acme` | Text, case doesn't matter |
| `hours>=3` | Numbers compare as numbers |
| `renewal<=eom` | Dates compare as dates |
| `billable=true` | True or false |

## Saved views: `@name`

A saved view is a query with a name. Use it as a term, and add more:

```sh
$ thc view add work 'status:open #work sort:due'
$ thc q '@work !high' --explain
query    @work !high
@work    (status:open #work sort:due)
means    open tasks
         and tagged #work
         and priority high
         sorted by due date, undated last
```

A view can use other views; one that refers to itself is refused. A view with sections (like
Today) runs on its own: `thc q @today`. See [Every view](../views/#views-with-sections).

## Sorting: `sort:`

| Term | Order |
|---|---|
| `sort:due` | Due date, undated last (ties by scheduled date) |
| `sort:scheduled` | Scheduled date |
| `sort:date` | Scheduled or due, whichever is set |
| `sort:priority` | High first |
| `sort:status` | By status |
| `sort:title` | A to Z |
| `sort:created` | Oldest first |
| `sort:updated` | Least recently changed first |
| `sort:done_at` | Completion date |

Add `-` for the reverse: `sort:updated-` is most recently changed first. Several `sort:` terms
sort by the first, then the next. With none, results come by date (scheduled or due), then most
recently changed.

## Grouping: `group:`

`group:` changes how results are laid out, never which ones match.

| Term | Sections |
|---|---|
| `group:parent` | By the page or day each one lives in |
| `group:tag` | By tag (a note with two tags shows under both) |
| `group:status` | By status |
| `group:due` | Overdue, today, tomorrow, this week, later, no due date |
| `group:actor` | By who made it |
| `group:vault` | By vault (with `vault:`) |

```sh
$ thc q 'is:task group:due'
Tomorrow  1
  edx71   [ ] Summarize unread newsletters into [[Reading List]]  due tomorrow · ◆ claude · in today's journal

This week  3
  ykwkc   [ ] Collect last quarter's metrics  due thu · in Draft Q4 OKRs
  ny82x   [ ] Call dentist to reschedule #health  due fri · !high · in today's journal
  pa3e6   [ ] Review draft with the team  due sun · in Draft Q4 OKRs
…
```

With `--json`, a grouped query returns `groups[]`.

## Vaults: `vault:`

By default a query reads the current [vault](../vaults/). `vault:` at the top level widens it:

| Term | Reads |
|---|---|
| `vault:acme` | That vault |
| `vault:(acme or personal)` | Those vaults |
| `vault:*` | Every registered vault |
| `vault:* -vault:side` | All but one |

Rows from more than one vault say which they came from, and in JSON each carries `vault`.

## Explain, the past, and output

| Flag | Does |
|---|---|
| `--explain` | Says what the query means, with relative dates resolved, and how many match. Runs no listing |
| `--explain=sql` | Adds the SQL it compiles to |
| `--as-of <when>` | Runs against the vault as it was then: `--as-of=-2d`, `--as-of 2026-09-30` |
| `--json` | Machine-readable: `{count, items[], …}` |
| `--fields id,short,due` | Only those fields (implies `--json`) |
| `--limit <n>` | At most n rows (default 50) |

```sh
$ thc q 'sort:due' --limit 2 --fields id,short,due,status
{"count":2,"items":[{"id":"edx71eyt9xsq","short":"edx71","status":"todo","due":"2026-10-06"},{"id":"ykwkcpyfwf8p","short":"ykwkc","status":"todo","due":"2026-10-08"}],…}
```

`--as-of` is possible because nothing is ever overwritten: thc replays the log up to that
moment and asks the question there. "What was open last Monday?" is one flag.

## Quick reference

```
status:open | todo | doing | waiting | done | cancelled | closed | any | none
due<=+3d   sched=today   done>=-7d   due:none   created>=-1w   updated>=-1d   journal=today
#tag   -#tag   !high   priority:med   text:word   title:"…"   bare words   "a phrase"
is:task | page | journal | inbox | event | repeating | overdue | alert | conflict
   | blocked | blocking | ready | to-review | image | file | attachment
under:<id|"Title">   parent:<id|"Title">   ancestor:( … )   has:child   has:child( … )
blocks:<id>   blocked-by:<id>   embeds:<id>   rel:<name>   by:claude   by:human   <field>=<value>
happens<=+7d   @view   a b (and)   a or b   ( … )   -term   -( … )
sort:due | scheduled | date | priority | status | title | created | updated | done_at   (add - to reverse)
group:parent | tag | status | due | actor | vault
vault:acme   vault:(a or b)   vault:*   -vault:side
```
