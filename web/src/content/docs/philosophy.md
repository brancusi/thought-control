---
title: Why it's like this
description: The few ideas behind thc. Knowing them makes the rest obvious.
order: 9
group: Guide
---

`thc` is a place to put things down: a thought, a todo, a date, a reminder. It's built for one
person, working with a few agents, from a terminal. These are the ideas behind it.

## A notebook in a terminal

Most note apps ask you to decide where something goes before you've finished thinking it.
`thc` doesn't. You open today's page and write. Paragraphs are paragraphs, a line starting
`[ ]` is a task, and `due:fri` is a date. You never fill in a form, pick a database or open a
dialog. The terminal is where you already are, so the notebook lives there too.

## The CLI is the interface

Every behaviour lives in the command line. The TUI is a comfortable way to drive it, and anything
you can do there, you can do with `thc` and a few words. Interfaces that grow features only they
can reach end up bending the data to fit; keeping one place where behaviour lives keeps the data
honest, and means a script can do whatever you can.

## Capture first, organise later (or never)

Everything lands in **today's journal** unless you say otherwise. That's a feature: the day is
the natural first place for a thought. Later you can link it to a page, move it, give it a date,
or leave it where it is. Today pulls what matters back in front of you, wherever it lives.

## Plain text, with structure underneath

What you see is text you could open in any editor. Underneath, each paragraph, task and list
item is a **note** with its own identity, history and fields. That's how `thc` can remind you of
one task, show what an agent changed in one paragraph, or undo one edit, while the page still
reads like a page.

The inline syntax (`Call dentist due:fri #health !high`) is read **once**, when you write it,
into typed fields. Stored text is never re-parsed to work out what something is.

## One primitive

Everything is a **node**. A page, a journal day, a todo, a reminder and a tag are all nodes used
in a particular way. "Task" isn't a type, just a `status` any note can have. "Reminder" isn't a
type, just an alert attached to a note. Fewer concepts, fewer edge cases, for people and agents
alike.

The fields are the ones every serious tool converged on: `scheduled` (when you plan to start)
separate from `due` (when it must be done), alerts as their own objects, and repeats that advance
the same item while keeping a record of each occurrence.

## Nothing is ever overwritten

The source of truth is an **append-only log**: one JSON line per change. Nothing is edited in
place, so undo always works, history is free, and "what did this look like last Tuesday?" is a
flag (`--as-of`). A mistake, yours or an agent's, is always recoverable. The fast local database
is only a view of that log: delete it and `thc rebuild` makes it again.

## Agents are collaborators you review

Claude, Codex or a script uses the exact same commands you do, with what they need to be
reliable: `--json` everywhere, stable exit codes, `--dry-run`, idempotent creates, short random
ids that never collide, and writes that are validated before they're accepted. Every change is
signed with who made it. You see what an agent did in **Review**, accept it with one key, or undo
it with another. Agents never resolve your conflicts or accept their own work.

## Local-first, yours forever

There's no server and no account. Your notes are a folder: sync it with iCloud, Dropbox or
Syncthing. Each device writes only its own files, so syncing never makes "conflicted copies".
Devices merge the same way every time: the newest change wins per field, and if two devices
edit the same text, both versions are kept and flagged rather than one being lost.

The log is plain JSONL you can `grep`, `jq`, or rebuild from in fifty lines of any language. The
Markdown export is regenerated as you go, and Obsidian can open it. If `thc` disappeared
tomorrow, your notes would still be complete and readable.

## Fast enough to disappear

Opening a day, typing, searching: all of it takes milliseconds, over SSH too. A query takes
about as long as the OS needs to start a process, so agents can call it in loops and people never
wait. That's why it's one Rust binary on SQLite, with no runtime to start.

## Small and boring

The boring, proven tool wins: SQLite over the newer database, a JSONL log over a CRDT framework,
one binary over a fleet of services. Complexity gets in only when it removes more than it adds.

## Calm by design

There's one accent colour, ember, and it means "here": your cursor, your focus, the one thing to
look at. Every other colour has exactly one meaning: overdue is red, today is amber, done is
green, an agent is blue, a conflict is violet. Nothing blinks for attention. The footer shows
only the few keys that work right now.
