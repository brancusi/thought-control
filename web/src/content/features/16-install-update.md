---
title: Install and update
blurb: One line to install. Verified updates from the terminal, in place.
icon: update
order: 16
---

```sh
$ curl -fsSL https://github.com/brancusi/thought-central-releases/releases/latest/download/install.sh | sh
  verified: checksum, Apple signature, update signature
Setting up thought-central
  ✓ vault       ~/thought (new)
  ✓ thc         ~/.local/bin/thc
  ✓ background  running · starts at login
  · agents      none found · thc setup claude --user to add one later

  Ready. Your notes live in ~/thought.
```

- **Zero setup.** One command makes your vault (`~/thought`), puts `thc` on your PATH, starts the background service at login and teaches the agents it finds. A `thc` you already have is never replaced.
- **Every download is checked** (checksum, Apple signature, update signature) before anything is installed.
- **A friendly first run:** a new vault opens on a short welcome, and the first journal day says `just type`.
- **`thc setup --status`** shows what setup added and whether it's current. **`thc setup --undo`** removes exactly that. Your notes are never touched.

**Updates**

```sh
$ thc update               # fetch, verify, swap in place, restart the service on it
$ thc update --rollback    # put the previous version back
```

- **From inside the app:** when a release is out, the footer says `update <version> · :update`. `:update` updates and puts you back exactly where you were.
- **Choose how eager:** `update = "notify"` (the default) mentions new versions in `thc today`; `"auto"` installs them; `"off"` stays quiet. The service checks every 6 hours.
- **Agent files refresh too:** an update rewrites the skills it installed for Claude Code and Codex.
- **Open windows notice** an update run elsewhere, and `:update` reloads them in place.
- **See what changed, right in thc.** Click the version at the bottom right, or type `:about`: every release since you last looked, then this thc's version, vault, background service and terminal, then the whole changelog, searchable with `/`. After an update the version reads `· new` until you've had a look.

**When something's off**

- **`thc doctor`** checks the vault and the local store, and says which vault it used and why.
- **`thc daemon status`** says whether the service is live.
- **`thc rebuild`** replays the log into a fresh store.

The Mac menu bar app and the Mac window are paused; thc is the terminal for now.
