---
title: Views
blurb: Seven views, one key each. Today is a query you can rewrite.
icon: views
order: 3
---

`thc` with no arguments opens the app. Seven views sit along the top: press <kbd>1</kbd>–<kbd>7</kbd>, <kbd>Tab</kbd> through them, or click a tab.

| Key | View | What it shows |
|---|---|---|
| <kbd>1</kbd> | **Today** | Overdue, due or scheduled today, reminders firing, doing, next 7 days, done today, from every page, day and vault. Each line names its vault; <kbd>*</kbd> picks which vaults. <kbd>w</kbd> flips to the agenda, a week by day |
| <kbd>2</kbd> | **Inbox** | What has no home yet, oldest first. Triage with <kbd>m</kbd> move, <kbd>t</kbd> task, <kbd>d</kbd> date, <kbd>x</kbd> done, <kbd>D</kbd> delete; the cursor moves on |
| <kbd>3</kbd> | **Tasks** | Any query (<kbd>f</kbd> to filter), saved filters on <kbd>1</kbd>–<kbd>9</kbd>, <kbd>,</kbd> to sort. A table with due, scheduled, priority, where and who from 120 columns |
| <kbd>4</kbd> | **Pages** | Every page with its open count. Type to find, <kbd>Enter</kbd> to open, `+ new page` to make one |
| <kbd>5</kbd> | **Journal** | One page per day, written in. A strip of days under the date |
| <kbd>6</kbd> | **Search** | Full text across pages, tags and notes, as you type |
| <kbd>7</kbd> | **Log** | Every change, newest first, with who made it. <kbd>r</kbd> opens the review lane |

- **The same lists on the command line:** `thc today`, `thc agenda --days 7`, `thc inbox`, `thc pages`, `thc journal fri`, `thc search "words"`, `thc log`.
- **List keys everywhere:** <kbd>x</kbd> done, <kbd>d</kbd> due, <kbd>s</kbd> scheduled, <kbd>p</kbd> <kbd>h</kbd>/<kbd>m</kbd>/<kbd>l</kbd> priority, <kbd>#</kbd> tags, <kbd>m</kbd> move, <kbd>S</kbd> status, <kbd>L</kbd> history, <kbd>y</kbd> copy the id, <kbd>Enter</kbd> open it where it lives.
- **A rail** of pages or days sits beside a document at 120 columns; narrower, a clickable crumb (`¶ Pages › Health`).
- **The detail pane** (<kbd>\\</kbd>) shows everything about the selected item, with its history and links.
- **`?` explains any view:** each section, its count, its query and what it means with dates filled in. <kbd>e</kbd> edits it, <kbd>c</kbd> copies it, <kbd>r</kbd> resets a built-in.
- **Lists keep their order.** Opening and coming back never reshuffle; a finished task stays put, dimmed, until you return or press <kbd>⌃L</kbd>.
- **Contexts:** `thc context work` (or <kbd>C</kbd>) applies a saved view to everything you see, and the header always says so. Local to this device, never synced.

**Built-in views are sections you can edit.** Today, Agenda, Inbox and Tasks are made of named queries. Rewrite them, copy them, or put them back:

```sh
$ thc view set today --section Due 'due<=today status:open' --section Waiting 'status:waiting'
$ thc view copy today acme-today
$ thc view set acme-today --scope vault:acme
$ thc view reset today
```

**Saved views** are named queries that sync like notes and undo like any write:

```sh
$ thc view add work 'status:open #work sort:due'
$ thc q @work due<=+3d
$ thc view ls
```

In the app they're on <kbd>space</kbd> <kbd>v</kbd>, and <kbd>space</kbd> <kbd>v</kbd> <kbd>s</kbd> saves the filter you're looking at.
