---
title: Writing
blurb: Plain text that becomes notes. Just type; thc keeps the structure.
icon: pen
order: 1
---

A journal day or a page is one plain-text document. You type; thc turns it into notes, each with its own dates and history, and keeps the machinery out of sight.

| You write | It's |
|---|---|
| A paragraph | One note. <kbd>Enter</kbd> inside it is a line break |
| A blank line | The end of a note |
| `- milk`, `1. first` | A list item, a numbered item. <kbd>Enter</kbd> continues the list |
| `[ ] call` / `[x] call` | A task, open or done |
| `# Title`, `## Section` | A heading |
| Quotes, code blocks, rules | What you'd expect, markers dimmed in the margin |

- **<kbd>⌃T</kbd> cycles a line:** text → `[ ]` → `[x]` → text, on one line or every selected line. <kbd>⌃Enter</kbd> does the same. In a paragraph of several lines it changes just the line you're on, and back to text it rejoins the paragraph.
- **Text never jumps.** A blank line you typed stays where it is when a line becomes a task, goes back to text or is nested. It's saved with the note, and the Markdown export keeps it.
- **Indent with <kbd>Tab</kbd>**, outdent with <kbd>⇧Tab</kbd>, move a line with its children with <kbd>⌥↑</kbd> <kbd>⌥↓</kbd>. <kbd>⇧Enter</kbd> or <kbd>⌃J</kbd> breaks a line inside a note.
- **`[[` links:** type `[[` and a picker offers pages and days. A new name makes the page when the line saves; a near miss (`[[Lisbn]]`) offers the page you meant, and <kbd>⌃O</kbd> takes it. A click on a link goes there (<kbd>Esc</kbd> comes back); click its brackets or <kbd>⌥</kbd>-click to edit it, and <kbd>⌃O</kbd> opens the link under the cursor.
- **Inline tokens:** `due:fri`, `sched:mon`, `at:"tue 2pm"`, `every:2w`, `!high`, `#tag`. They underline as you type, the right margin previews what they'll set, and when you leave the line the dates fold into a quiet column. Click one to change it.
- **Quoted text is never parsed.** Inside "double quotes" or `backticks`, `!high` and `#tag` are just words, so you can write about the syntax. Two of the same token on a line: the last one counts, and the hint says so.
- **Paste Markdown** and it becomes the outline, in one undo step. <kbd>⌘C</kbd> (or <kbd>⌃C</kbd>) copies as Markdown; <kbd>⌥V</kbd> makes the next paste plain.
- **Saving is automatic:** when you leave a line, after a pause, and when you switch away or quit. A crash or a closed window never loses what you typed.
- **Undo across saves** with <kbd>⌘Z</kbd> / <kbd>⇧⌘Z</kbd> (or <kbd>⌃Z</kbd> / <kbd>⌃Y</kbd>).
- **Selection works like a Mac text field:** <kbd>⌘A</kbd> selects all, and <kbd>←</kbd> / <kbd>→</kbd> with a selection go to its start or end and stop there.
- **Each page and day remembers where your caret was.**
- **Focus** (<kbd>⌥Z</kbd>) hides everything but your words. `:focus` composes it (day strip, month calendar, word count, clock, typewriter scrolling…) from three presets: `bare`, `writer`, `planner`.
- **Under a day,** *also today* lists what's due elsewhere. **Under a page,** *linked from* lists what mentions it.
- **Emoji, accents and CJK** behave as single characters.
- **`thc j`** opens today with the caret on a fresh line (`thc j fri`, `thc j 2026-10-02`). **`thc p Q4 Planning`** opens a page. <kbd>⌃P</kbd> / <kbd>⌃N</kbd> step through days.
