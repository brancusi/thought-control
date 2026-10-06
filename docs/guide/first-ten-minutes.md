# Your first ten minutes

Nine short steps, from install to "I trust this". Each takes about a minute.

## 1. Install

```console
$ curl -fsSL https://github.com/brancusi/thought-central-releases/releases/latest/download/install.sh | sh
thc 0.9.0 (aarch64-apple-darwin)
  verified: checksum, Apple signature, update signature
  installed ~/.local/bin/thc
Setting up thought-central
  ✓ vault       ~/thought (new)
  ✓ thc         ~/.local/bin/thc
  ✓ background  running · starts at login
  · agents      none found · thc setup claude --user to add one later

  Ready. Your notes live in ~/thought.

    thc j      write in today's journal
    thc        see what's due
    thc --help everything else
```

What happened: your notes folder is `~/thought`, the `thc` command is on your PATH, and a small
background service keeps reminders and live updates running. Open a new terminal window so the
command is found.

## 2. Write: `thc j`

```console
$ thc j
```

[render: a blank journal day, the caret on the first line, the footer `just type`]

You're in today's journal, and the cursor is ready. **Just type.** Enter starts a new line in
the same paragraph, and a blank line starts a new note. There's no save button: the footer says
`autosaved`, and it means it.

## 3. Make a task

Type `[ ] call the dentist`, or write the line and press **⌃T**. Press ⌃T again and it's done.
Once more and it's plain text again.

[render: a task line with `[ ]` in the margin, then `[x]` in green]

## 4. Give it a date

Add `due:fri` to the line. While you type it, it's underlined. When you move off the line, the
date tucks into the quiet column on the right: `due fri`. Try `sched:mon`, `!high`, `every:2w`
or `#health` too.

[render: the token underlined while typing, then folded into the meta column]

## 5. Link a page

Type `[[` and start a name: `[[Lisbon`. Pick a page from the list, or finish the name and type
`]]`. If the page doesn't exist yet, it does now. Put the caret on the link and press **⌃O** to
go there.

[render: the `[[` popup with pages and days]

## 6. Look at Today

Press **Esc**. It saves, and you're on **Today**: what's overdue, due today, scheduled and coming
up, from every page and day. `x` marks the selected row done, `d` changes its date, and `Enter`
opens it.

[render: Today with overdue, today and next 7 days]

Press **Space**. A panel lists everything you can do from here, grouped: `g` to go somewhere,
`f` to find, `n` for something new. It's the one key to remember when you forget the others.

[render: 10-leader]

## 7. Find anything

Press `/` and type. Search covers pages, tags and every note, and `Enter` opens the match where
it lives. `⌃O` from anywhere jumps to a page or day by name.

## 8. Let an agent help, and review what it did

```console
$ thc setup claude --user
```

Now Claude Code knows `thc`. Ask it to "add the three follow-ups from this call to my notes".
Its changes are marked `◆ claude`. In the TUI, press `7` (Log), then `r`: the review lane shows
each change, with `a` to accept and `u` to undo.

[render: the review lane with two claude changes]

## 9. Stay current

`thc` updates itself. When a new version is out, the footer says `update 0.9.1 · :update`. Type
`:update` and it's back in a moment, exactly where you were.

---

That's it. From here: [every view](views.md), [how writing works](writing.md), and
[all the keys](keys.md).
