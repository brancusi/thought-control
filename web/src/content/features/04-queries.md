---
title: Queries
blurb: A small query language for everything. Ask it what it means, or how it was.
icon: query
order: 4
---

One language drives Tasks, saved views, Today's sections and `thc q`. Terms AND together; `or` and parentheses group; a leading `-` negates a term or a group.

```sh
$ thc q 'status:open #work due<=+3d sort:due'
$ thc q '(#home or #health) -status:done'
$ thc q 'is:ready sort:priority' --json
```

| Kind | Terms |
|---|---|
| Status | `status:open` `todo` `doing` `waiting` `done` `cancelled` `closed` `any` `none` |
| Dates | `due<=+3d` `sched=today` `done>=-7d` `due:none` `created>=-1w` `journal=today` `happens<=+7d` |
| Tags, words | `#tag` `-#tag` `text:word` bare words `!high` |
| Who | `by:claude` |
| Where | `under:<id>` `parent:<id>` (page titles work: `under:"¶ Issues"`) `ancestor:( … )` `has:child( … )` |
| Kinds | `is:task` `page` `journal` `inbox` `event` `repeating` `overdue` `conflict` `alert` `to-review` |
| Relations | `is:blocked` `is:blocking` `blocks:<id>` `blocked-by:<id>` `rel:<name>` |
| Your fields | `client=acme`, any property you `thc set` |
| Layout | `sort:due` `priority` `updated-` `title` `date` · `group:parent` `tag` `status` `due` `actor` `vault` |
| Views | `@work`, composable: `@work due<=+3d` |
| Vaults | `vault:acme` `vault:(acme or personal)` `vault:*` `-vault:side` |

- **`is:ready`** is the next thing to pick up: open, scheduled date reached or unset, nothing open blocking it.
- **Relations are links:** `thc link a b --rel blocks` makes `a` block `b`, and `is:ready` respects it.
- **`--explain`** says what a query means, with relative dates resolved, without running a listing:

```sh
$ thc q 'is:ready sort:due' --explain
query    is:ready sort:due
means    ready (todo or doing, scheduled by Mon Oct 5 or unscheduled, nothing open blocking)
         sorted by due date, undated last
matches  4 · 0.1 ms
```

- **`--as-of=-2d`** runs the query against the vault as it was two days ago.
- **Typos are caught:** `status:opn` is refused with a suggestion.
- **`--json`** returns rows (and `groups[]` when grouped); `--fields id,short,due` trims them.
