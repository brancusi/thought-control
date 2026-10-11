#!/bin/sh
# Try Thought Control (thc) in one line, on made-up notes, without touching your own:
#
#   curl -fsSL https://thoughtcontrol.app/try.sh | sh                  # the latest thc
#   curl -fsSL https://thoughtcontrol.app/try.sh | sh -s -- v0.10.1    # a tag, a branch or a full commit
#
# It fetches that version of thc into ~/.thc-try/<version> (or $THC_TRY_HOME/<version>), uses
# the release binary when the version is a release tag (macOS) and builds from source with cargo
# otherwise, then opens thc's screen on a sandbox seeded with made-up notes.
#
# The sandbox, ~/.thc-try/<version>.sandbox, has its own vault, HOME, config, cache and temp
# folder. Nothing starts a daemon or checks for updates, its sockets are its own, and launchctl
# and systemctl are stubbed inside it, so it can't reach a real thc's daemon. Your own vaults,
# config and daemon are never read or written. Delete ~/.thc-try to remove everything.
#
# A second terminal can play an agent and watch the screen change: ~/.thc-try/bin/thc is thc
# pointed at the sandbox (`thc add "…"`, `thc done <id>`, `thc ui ls`).
#
# Pages link a frozen copy of this script pinned to one commit (https://thoughtcontrol.app/try/),
# so the demo a page shows stays exactly that demo.
#
# Needs git. Release tags have binaries for macOS; anything else builds with cargo
# (https://rustup.rs). Without cargo, the latest (`main`) falls back to the newest release.
# python3 seeds the sample notes. THC_TRY_NO_RUN=1 sets up without running.
# THC_TRY_REPO fetches the code from elsewhere (a fork, or file:///path/to/a/checkout).
set -eu

ref="${1:-${THC_TRY_REF:-main}}"
repo="${THC_TRY_REPO:-https://github.com/brancusi/thought-control}"
releases=https://github.com/brancusi/thought-control-releases
home="${THC_TRY_HOME:-$HOME/.thc-try}"
# The screen it opens: `thc screen` (the fictty-based UI), or `thc tui` where there's no screen yet.
screen="${THC_TRY_SCREEN:-screen}"

say() { printf '\033[36m▸\033[0m %s\n' "$1" >&2; }
die() { printf '\033[31m✗\033[0m %s\n' "$1" >&2; exit 1; }
q() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }

# The latest without Rust: main can only be built, so take the newest release, which has binaries.
if [ "$ref" = main ] && ! command -v cargo >/dev/null 2>&1 && command -v curl >/dev/null 2>&1; then
  latest=$(curl -fsSL "$releases/releases/latest/download/latest.json" 2>/dev/null |
    sed -n 's/.*"version": *"\([^"]*\)".*/\1/p' | head -1)
  if [ -n "$latest" ]; then
    say "Rust isn't installed, so this runs the latest release, v$latest, instead of building main"
    ref="v$latest"
  fi
fi
src="$home/$(printf '%s' "$ref" | tr -c 'A-Za-z0-9._-' '-' | cut -c1-40)"
sb="$src.sandbox"
bin="$src/target/try/thc"

command -v git >/dev/null 2>&1 || die "thc needs git. Install it, then run this again."

# This version of the code: a branch moves on each run, a tag or a commit never does.
say "Fetching thc $ref into $src"
mkdir -p "$src"
[ -d "$src/.git" ] || { git init -q "$src" && git -C "$src" remote add origin "$repo"; }
git -C "$src" remote set-url origin "$repo"
git -C "$src" fetch -q --depth 1 origin "$ref" || die "Couldn't fetch $ref from $repo."
git -C "$src" checkout -q --detach FETCH_HEAD

# A release binary, only when the version is a release tag (so it always matches the code), and
# only when its checksum matches the one published beside it.
target=""
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target=aarch64-apple-darwin ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
esac
got=""
case "$ref" in
  v[0-9]*)
    if [ -n "$target" ] && command -v curl >/dev/null 2>&1; then
      name="thc-${ref#v}-$target.tar.gz"
      tmp="$src/$name"
      if curl -fsSL "$releases/releases/download/$ref/$name" -o "$tmp" 2>/dev/null; then
        want=$(curl -fsSL "$releases/releases/download/$ref/$name.sha256" 2>/dev/null | cut -d' ' -f1)
        have=$({ shasum -a 256 "$tmp" 2>/dev/null || sha256sum "$tmp"; } | cut -d' ' -f1)
        if [ -n "$want" ] && [ "$want" = "$have" ]; then
          mkdir -p "$(dirname "$bin")"
          tar -xzf "$tmp" -C "$(dirname "$bin")" thc && got=yes
          [ -n "$got" ] && say "Using the $ref release binary for $target"
        else
          say "The $ref download didn't match its checksum, so this builds from source instead"
        fi
        rm -f "$tmp"
      fi
    fi
    ;;
esac
if [ -z "$got" ]; then
  command -v cargo >/dev/null 2>&1 ||
    die "Building thc needs Rust. Install it from https://rustup.rs, then run this again."
  say "Building thc (a few minutes the first time)"
  (cd "$src" && cargo build --release --quiet -p thc) || die "The build failed; the output above says why."
  # A new file, never a copy over the old one: macOS kills a binary rewritten in place.
  mkdir -p "$(dirname "$bin")"
  cp "${CARGO_TARGET_DIR:-$src/target}/release/thc" "$bin.new" && mv "$bin.new" "$bin"
fi

# The sandbox: everything thc reads or writes lives here. env.sh is the one description of it;
# this script and the ~/.thc-try/bin/thc wrapper both source it.
mkdir -p "$sb/home" "$sb/config" "$sb/cache" "$sb/data" "$sb/state" "$sb/runtime" "$sb/tmp" "$sb/shims"
# The daemon socket: a folder of its own, never the real daemon's /tmp/thc-<uid>. Short, because
# socket paths are limited to about 100 bytes.
sock="/tmp/thc-try-$(id -u)/$(printf '%s' "$sb" | cksum | cut -d' ' -f1).sock"
mkdir -p "$(dirname "$sock")" && chmod 700 "$(dirname "$sock")"
for tool in launchctl systemctl; do
  printf '#!/bin/sh\necho "%s is switched off in the thc try sandbox" >&2\nexit 1\n' "$tool" >"$sb/shims/$tool"
  chmod +x "$sb/shims/$tool"
done
{
  echo "# The thc try sandbox for $ref, written by try.sh. Sourcing it points thc here, never at"
  echo "# your own vaults, config or daemon. Inherited THC_* settings are dropped except these few."
  echo 'for v in $(env | sed -n "s/^\(THC_[A-Za-z0-9_]*\)=.*/\1/p"); do'
  echo '  case $v in THC_ACTOR | THC_ROLE | THC_SESSION_ID | THC_THEME | THC_GLYPHS | THC_NOW | THC_TUI_SNAPSHOT | THC_TUI_SNAPSHOT_FORMAT | THC_TUI_KEYS) ;; *) unset "$v" ;; esac'
  echo 'done'
  echo "export HOME=$(q "$sb/home") TMPDIR=$(q "$sb/tmp")"
  echo "export THC_VAULT=$(q "$sb/sample-notes") THC_CONFIG_DIR=$(q "$sb/config") THC_CACHE_DIR=$(q "$sb/cache") THC_SOCKET=$(q "$sock")"
  echo "export XDG_CONFIG_HOME=$(q "$sb/config") XDG_CACHE_HOME=$(q "$sb/cache") XDG_DATA_HOME=$(q "$sb/data") XDG_STATE_HOME=$(q "$sb/state") XDG_RUNTIME_DIR=$(q "$sb/runtime")"
  echo "export THC_NO_UPDATE_CHECK=1 THC_NO_SETUP=1 THC_SETUP_LOGIN=skip THC_SETUP_APP=none"
  echo "export PATH=$(q "$sb/shims"):\"\$PATH\""
} >"$sb/env.sh"

# Made-up notes (the repo's own sample seed), once per sandbox. Creates are keyed, so a seed
# that stopped halfway just runs again.
if [ ! -f "$sb/.seeded" ]; then
  if [ -f "$src/scripts/seed-sample.sh" ] && command -v python3 >/dev/null 2>&1 && command -v bash >/dev/null 2>&1; then
    say "Seeding the sandbox with made-up notes"
    (. "$sb/env.sh" && cd "$sb" && THC="$bin" bash "$src/scripts/seed-sample.sh" "$THC_VAULT") >"$sb/seed.log" 2>&1 ||
      die "Seeding the sandbox failed; $sb/seed.log says why."
  else
    say "python3 isn't installed, so the sandbox starts empty"
    (. "$sb/env.sh" && cd "$sb" && "$bin" init "$THC_VAULT") >"$sb/seed.log" 2>&1 ||
      die "Making the sandbox vault failed; $sb/seed.log says why."
  fi
  : >"$sb/.seeded"
fi

if [ "$screen" = screen ] && ! (. "$sb/env.sh" && "$bin" screen --help) >/dev/null 2>&1; then
  say "This thc has no \`thc screen\` (the fictty-based UI) yet, so it opens \`thc tui\`"
  screen=tui
fi

# `thc` for a second terminal: this version, in this sandbox. Machine-level commands are refused.
mkdir -p "$home/bin"
{
  echo '#!/bin/sh'
  echo "# thc $ref in the try sandbox $sb (written by try.sh)."
  echo ". $(q "$sb/env.sh")"
  echo 'for a in "$@"; do'
  echo '  case $a in'
  echo '    -*) continue ;;'
  echo '    daemon | setup | update | uninstall) echo "thc $a is switched off in the try sandbox" >&2; exit 2 ;;'
  echo '  esac'
  echo '  break'
  echo 'done'
  echo "exec $(q "$bin") \"\$@\""
} >"$home/bin/thc"
chmod +x "$home/bin/thc"

agent="In a second terminal: export PATH=\"$home/bin:\$PATH\" THC_ACTOR=claude, then thc add \"Water the ferns due:fri\", thc done <id> or thc ui ls"
if [ "${THC_TRY_NO_RUN:-}" = 1 ]; then
  say "Ready. Run it with: $home/bin/thc $screen"
  say "$agent"
  exit 0
fi
[ -r /dev/tty ] || die "This needs a terminal to run in."
if size=$(stty size </dev/tty 2>/dev/null); then
  rows=${size% *}; cols=${size#* }
  if [ "$cols" -lt 100 ] || [ "$rows" -lt 30 ]; then
    say "Your terminal is ${cols}×${rows}; thc looks best at 120×32 or more."
  fi
fi
say "Opening thc $screen on made-up notes. Ctrl-Q quits."
say "$agent"
sleep 2
# Hand the screen the terminal's own device. Under `curl | sh` our input is the pipe, and the
# generic /dev/tty won't do: macOS can't wait on it (kqueue), so the screen would hang on its
# first read. ps names the real one (ttys003, pts/1).
term="/dev/$(ps -o tty= -p $$ 2>/dev/null | tr -d ' ')"
[ -c "$term" ] || term=/dev/tty
status=0
(. "$sb/env.sh" && unset THC_ACTOR && cd "$sb" && exec "$bin" "$screen" <"$term" >"$term") || status=$?
# A daemon started from inside the screen belongs to the sandbox: stop it on the way out.
(. "$sb/env.sh" && "$bin" daemon stop) >/dev/null 2>&1 || true
say "Your notes in the sandbox stay in $sb for next time. Delete $home to remove everything."
exit $status
