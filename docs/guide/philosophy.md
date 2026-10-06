# Why it's like this

Thought Central is built on a few ideas. Knowing them makes the rest of the guide obvious.

## A notebook in a terminal

Most note apps ask you to decide where something goes before you've finished thinking it.
`thc` doesn't. You open today's page and write. Paragraphs are paragraphs, a line starting
`[ ]` is a task, and `due:fri` is a date. You never fill in a form, pick a database or open a
dialog. The terminal is where you already are, so the notebook lives there too.

## Capture first, organise later (or never)

Everything lands in **today's journal** unless you say otherwise. That's a feature: the day is
the natural first place for a thought. Later you can link it to a page, move it, give it a date,
or leave it where it is. Today pulls what matters back in front of you, wherever it lives.

## Plain text, with structure underneath

What you see is text you could open in any editor. Underneath, each paragraph, task and list
item is a **note** with its own identity, history and fields. That's how `thc` can remind you of
one task, show what an agent changed in one paragraph, or undo one edit, while the page still
reads like a page. You never see the machinery unless you ask for it.

## Nothing is ever overwritten

Every change is appended to a log, and nothing is edited in place. So undo always works, history
is free, and a mistake (yours or an agent's) is always recoverable. The fast local database is
just a view of that log: delete it and it rebuilds.

## Agents are collaborators you review

Claude, Codex or a script uses the exact same commands you do. Every change is signed with who
made it. You see what an agent did in **Review**, accept it with one key, or undo it with
another. Agents never resolve your conflicts or accept their own work.

## Local-first, yours forever

There's no server and no account. Your notes are a folder: sync it with iCloud, Dropbox or
Syncthing. Each device writes only its own files, so syncing never makes "conflicted copies".
The log is plain text, and the export is Markdown. If `thc` disappeared tomorrow, your notes
would still be complete and readable.

## Fast enough to disappear

Opening a day, typing, searching: all of it takes milliseconds, over SSH too. A tool you think
in shouldn't make you wait.

## Calm by design

There's one accent colour (ember), and it means "here": your cursor, your focus, the one thing to
look at. Every other colour has exactly one meaning: overdue is red, today is amber, done is
green, an agent is blue, a conflict is violet. Nothing blinks for attention. The footer shows
only the few keys that work right now.
