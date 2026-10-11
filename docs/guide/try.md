# Try it in one line

```sh
curl -fsSL https://thoughtcontrol.app/try.sh | sh                  # the latest thc
curl -fsSL https://thoughtcontrol.app/try.sh | sh -s -- v0.10.1    # a release tag, a branch or a commit
```

It fetches that version of thc into `~/.thc-try/<version>`, fills a sandbox with made-up notes
and opens thc's screen on it. On a release tag on a Mac it uses the release's binary (checked
against its published checksum); anything else is built from source with cargo, which takes a
few minutes the first time. Without cargo, the latest falls back to the newest release.

It opens `thc screen`, the new screen built on [fictty](https://github.com/brancusi/fictty),
when the version has one, and `thc tui` otherwise (it says which).

## What it touches

Only `~/.thc-try` (or `$THC_TRY_HOME`), plus one socket folder, `/tmp/thc-try-<uid>`. Each
version has its own sandbox, `~/.thc-try/<version>.sandbox`, with its own vault, HOME, config,
cache and temp folder, described in one file, `env.sh`.

- Your vaults, `~/.config/thought` and your daemon are never read or written.
- No daemon starts, nothing checks for updates, and nothing is installed or put on your PATH.
- Its sockets are its own, `launchctl` and `systemctl` are stubbed inside it, and the sandbox's
  `thc` refuses `daemon`, `setup`, `update` and `uninstall`, so nothing in it can reach a real
  thc's daemon or login item.

Delete `~/.thc-try` to remove everything.

## Be the agent

While the screen runs, a second terminal can play an agent and watch the screen change:

```sh
export PATH="$HOME/.thc-try/bin:$PATH" THC_ACTOR=claude
thc add "Water the ferns due:fri"
thc done <id>
thc ui ls
```

`~/.thc-try/bin/thc` is thc pointed at the sandbox of the version you ran last.

## For maintainers

- `THC_TRY_NO_RUN=1` sets up without opening the screen; `THC_TRY_REPO` fetches from another
  repository (a fork, or `file:///path/to/checkout`).
- `scripts/try-test.py` runs the one-liner the way a reader does, from an empty home and from
  this checkout (no network), renders a frame, plays the second terminal and checks your real
  thc is untouched. `--pty` also runs the screen in a pseudo-terminal.
- A page that shows a particular build links a frozen copy pinned to a commit or a release tag:
  `scripts/freeze-demo.sh <slug> <v0.x.y or commit> "<title>"` writes `web/public/try/<slug>.sh`.
  Never edit a frozen copy by hand; rerun the script with a note.
