---
title: Screenshots and files
blurb: Paste a screenshot into a note. See it inline. Agents can read it.
icon: image
order: 11
---

A screenshot is a file in the vault and a Markdown image line in your note. Nothing more exotic than that.

- **<kbd>⌘V</kbd> pastes a screenshot.** Copy one (<kbd>⌃⇧⌘4</kbd> on a Mac), then <kbd>⌘V</kbd> while writing: it's attached as its own line, and <kbd>⌃Z</kbd> undoes it. Text pastes exactly as before.
- **<kbd>⌃V</kbd>** does the same in any terminal, because thc reads the clipboard itself. <kbd>⌥V</kbd> works too.
- **Drag a file onto the terminal** and it's attached at once. The bar says `attached shot.png · ⌃Z keep the path`: <kbd>⌃Z</kbd> turns it back into the path as text, and again removes that.
- **A chip** marks each attachment: `▣ caption · 1280×720 · ⌃O open`. Moving onto it never opens it; <kbd>⌃O</kbd> or a double-click opens it in your viewer, and <kbd>Enter</kbd> starts a new line after it.
- **Inline images** draw under the chip in WezTerm, iTerm2, kitty and Ghostty, sized to the text column and at most 16 lines tall. The space is kept while you type, so nothing jumps. Over SSH you get the chip; `[tui] images = "chips"` or `"off"` chooses that anywhere.
- **`thc shot`** takes a screenshot for you: pick an area of the screen, and it lands in today's journal (or `--under <id>`, with `--caption "…"`). macOS only.
- **From the command line,** or from an agent, one file or several at once:

```sh
$ thc attach 7gthv login-error.png --caption "export output"
attached files/2026/10/mwxqh-login-error.png · 1280×720 · 5 KB under 7gthv
```

- **Pictures are tidied as they come in:** location data is removed, a sideways photo is turned the right way up, and very large ones are scaled down.

- **Every attachment is something you can find.** Each image or file is also an item of its own: the same one wherever it appears, and on every device. Your notes' text doesn't change.

```sh
$ thc q is:image
tqe20   boiler label
$ thc show tqe20          # which notes use it
```

  `is:image`, `is:file` and `is:attachment` find them; `embeds:<id>` finds the notes that show one.
- **Agents can look.** `thc show <id> --json` lists attachments with their path, size, type and dimensions, so an agent can open the image itself:

```json
"attachments": [{
  "path": "files/2026/10/mwxqh-login-error.png",
  "caption": "export output",
  "mime": "image/png", "w": 1280, "h": 720, "missing": false
}]
```

- **Stored plainly:** `files/<yyyy>/<mm>/<id>-<name>`, written once, synced with the vault like everything else.
- **`thc doctor`** notices a missing attachment, or a file nothing refers to; `--fix --yes` moves strays aside and never deletes.
- **Files over 20 MB are refused** by default (`[attachments] max_mb`).
- **In WezTerm,** `thc setup wezterm --yes` once lets <kbd>⌘V</kbd> reach thc. Only while thc runs in the pane; your shell and SSH paste as usual.
