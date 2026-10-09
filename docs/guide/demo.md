# Learn THC by doing

In a build with the teaching demo enabled, run:

```sh
thc demo
```

You enter **the real THC editor**, with a little workshop made of synthetic notes. Arrows,
rings and spotlights point to what the guide is explaining. Nothing is played back over a
screenshot: you can type, follow links, edit a companion panel and complete tasks yourself.

The seven stops cover a safe workspace, writing and editing, page links, navigation, side
panels, tasks, and a final invitation to explore. The guide changes the view when **you**
advance; it never types for you or replaces your edits with the original examples.

## You set the pace

- **F2** advances; **Shift-F2** goes back. Some terminals send Shift-F2 as F14, which works too.
- **F3** dismisses the guide without closing your notes or undoing anything you wrote.
- **Alt-N / Alt-P / Alt-G** are alternatives for next, back and dismiss when function keys
  belong to your desktop. Your terminal must pass Alt as Meta.
- Click **back**, **next**, or **×** in the guide; click a step dot to jump to that stop.
- **Ctrl-Q** exits, including while writing.

All ordinary editing keys continue to work. **Ctrl-O** follows the page link under the
caret; **Ctrl-Alt-Left / Right** go back and forward in navigation history. **Alt-O** opens
a link beside you instead. **Alt-S** focuses the sidebar and **Alt-W** closes its active
panel. In the task list, **x** completes a task and **Shift-X** reopens it. The guide does
not introduce another Enter mode: this is the same editor used outside the demo.

Help, prompts and menus take precedence over the guide. Close them to see the same step
again. There is no timer or agent deciding when to advance.

## Any reasonable terminal

Try 120 columns for pages side by side. At narrower widths the panel uses THC's existing
drawer or replacement view. If an anchored guide box cannot fit, a compact text caption
shows its instructions and next/back/dismiss controls instead. The keyboard controls do
not depend on colour, arrow glyphs or mouse support. Very small terminals necessarily
truncate instructions; widen the window to read more.

## Your notebook stays untouched

Each launch creates a private scratch vault, HOME, configuration and working directory.
Your project's vault selection and agent/board settings are not used. There is no agent
socket, live daemon connection, automatic update, or ability to switch to another vault
from this session. Demo settings do not change your normal preferences.

By default, quitting removes the scratch session. To keep your experiments:

```sh
thc demo --keep
```

The preserved directory is printed when you exit. It contains the scratch notes and their
local settings, not a copy of your real notebook. A later `thc demo` starts a fresh workshop;
it does not overwrite a kept one. Abnormal termination can leave temporary scratch files
behind; no real notebook data is involved.

This demo is currently local-build-only: its coherent Caretline engine/layers/tour export
must be prepared explicitly before building. It is not an instruction to replace an
installed binary or float to a newer dependency branch.
