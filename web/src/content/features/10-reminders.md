---
title: Reminders and repeats
blurb: Alerts at a time or before a deadline. Repeats that count from the date or from done.
icon: bell
order: 10
---

**Reminders** are tasks with a time and an alert at that time.

```sh
$ thc remind "Pay rent" --at "nov 1 9am" --repeat "every month on the 1st"
$ thc alert add 750tz --before 1d
alert jw4mn fires 2026-10-08 09:00
$ thc alert add 750tz --at "fri 9am"
$ thc alert ls
jw4mn   2026-10-08 09:00  pending  Fix flaky login test
0v0wp   2026-10-09 16:00  pending  Ship release notes
```

- **`--before`** is relative to the due date (or `--anchor scheduled`); **`--at`** is a time.
- **When one fires** in the app, a toast offers <kbd>x</kbd> done, <kbd>z</kbd> snooze, <kbd>Z</kbd> acknowledge. Outside the app, it's a system notification.
- **Snooze and acknowledge from anywhere:** <kbd>z</kbd> / <kbd>Z</kbd> on a row, or `thc alert snooze <id> --until +1h` and `thc alert ack <id>`.
- **`thc alert preview --at <when>`** shows what would be delivered then (singles, summaries, missed-while-away), and writes nothing.
- **Reminders in every vault reach you,** named: `acme · Book the venue · due 17:00`.

**Repeats** sit on any task or event.

| Token | Repeats |
|---|---|
| `every:3d` | Every three days from its date |
| `every:2w` | Every two weeks |
| `every:weekday` | Monday to Friday |
| `every!:1mo` | A month after you finish it |
| `--repeat "every month on the 1st"` | In words, on the CLI |

- **`thc done`** on a repeating task advances it to its next date. The row reads `↻ 3d`, `↻ 3mo!` or `↻ monthly`.
- **`thc skip <id>`** (or <kbd>r</kbd> on a row) skips this occurrence without completing it.
- **`thc q is:repeating`** lists them all.
