---
title: Install
description: One command puts thc on your PATH, makes a vault, starts the background service and teaches your agents.
order: 1
group: Start
---

`thc` installs with one line, on macOS (Apple silicon or Intel):

```sh
$ curl -fsSL https://github.com/brancusi/thought-control-releases/releases/latest/download/install.sh | sh
```

It ends with a checklist, something like this:

```
thc 0.9.43 (aarch64-apple-darwin)
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

Open a new terminal window so your shell finds the command, then type `thc j`.

The installer is macOS-only for now. Releases, checksums and the installer itself live at
[thought-control-releases](https://github.com/brancusi/thought-control-releases/releases).

## What it sets up

| | Where | What for |
|---|---|---|
| **The `thc` command** | `~/.local/bin/thc` | Everything. It's one binary: CLI, TUI and background service |
| **Your vault** | `~/thought` | Your notes, as a folder. An existing vault is kept as it is |
| **The background service** | A login item (launchd) | Reminders, live updates between windows and agents, other devices' changes |
| **Agent skills** | Claude Code, Codex | Only for agents it finds. Teaches them `thc` and the house rules |

Some details worth knowing:

- **Every download is checked before anything is installed:** a checksum, Apple's code
  signature and the release's own update signature. If one fails, nothing changes.
- **Your PATH:** if a new terminal wouldn't find `~/.local/bin`, setup adds one marked line to
  your shell profile. A `thc` you already have from somewhere else is never replaced.
- **The vault is never a sync folder by default.** It's `~/thought`. To share one with a team or
  across machines, make a [vault](../vaults/) in a synced folder.
- **Agents later:** `thc setup claude --user` or `thc setup codex --user` whenever you like. See
  [Agents](../agents/).

`thc setup --status` shows what setup added, where it is, and whether it's current.

## WezTerm

If you use WezTerm, one more command gives you the Mac editing keys (`⌘←`, `⌥←`, `⌘⌫`) and
`⌘V` for screenshots:

```sh
$ thc setup wezterm --yes
```

It writes thc's own key file and adds one marked block to the config WezTerm loads, after
backing that file up. The keys only act while thc is running in the pane: in your shell, an
editor or an SSH session, they do what they did before. It also turns on WezTerm's kitty
keyboard mode, which is what lets <kbd>⇧Enter</kbd> make a line break. `thc setup`, run inside
WezTerm, offers this once and never changes your config without a yes.
`thc setup wezterm --undo --yes` takes it out again.

## Staying current

`thc` updates itself.

```sh
$ thc update            # fetch, verify, swap in place, restart the service on it
$ thc update --check    # only say whether there's a new version
$ thc update --rollback # put the previous version back
```

The background service checks for a new release every six hours. When there is one,
`thc today` mentions it and the TUI's footer says so: type `:update` and thc updates and comes
back exactly where you were. If you update from another window, an open thc notices and offers
`:update` to reload in place.

In `~/.config/thought/config.toml`, `update = "off"` stops the checks and `update = "auto"`
installs new versions on its own.

## When something seems off

```sh
$ thc doctor
```

`thc doctor` checks the vault and the local store, and says which vault it's using and why.
It notices days or tags that were made twice on two devices before they synced, empty tags,
missing attachments and files nothing refers to. `thc doctor --fix` shows what it would repair
without changing anything; `thc doctor --fix --yes` makes the repair as one step that
`thc undo` puts back. Unreferenced files are moved aside, never deleted.

Two more that help:

```sh
$ thc daemon status     # is the background service running? (exit 0 live, 3 not)
$ thc vault             # which vault you're in, and why
```

The local database is only a cache of the log. If it ever looks wrong, `thc rebuild` replays
the log and makes it again.

## Uninstall

```sh
$ thc setup wezterm --undo --yes    # only if you ran thc setup wezterm
$ thc setup --undo
```

The first removes thc's block and key file from your WezTerm config. The second removes what
setup added: the login item, the PATH line and the agent files (a skill
you've edited by hand is left alone, and it says so). Then remove the binary itself:

```sh
$ rm ~/.local/bin/thc
```

**None of this touches your notes.** Your vault stays in `~/thought`, a plain folder you can keep,
back up or delete yourself.
