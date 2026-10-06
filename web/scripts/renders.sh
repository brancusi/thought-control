#!/usr/bin/env bash
# Real TUI frames for the public site (web/), rendered by `thc tui` as HTML from scratch vaults.
#
# Never touches a real vault: a scratch HOME, config and cache in a temp dir, seeded by
# scripts/seed-sample.sh with the clock pinned and fixture ids, each vault served by its own
# scratch daemon (never launchd). Output: src/renders/<name>-ember-{dark,light}.html.
#
# Usage (from web/):     npm run renders          uses ../../target/release/thc, else debug
#                          THC=/path/to/thc npm run renders
set -euo pipefail
SITE=$(cd "$(dirname "$0")/.." && pwd)
REPO=$(cd "$SITE/.." && pwd)
OUT="$SITE/src/renders"
THC=${THC:-}
if [ -z "$THC" ]; then
  for c in "$HOME/.local/bin/thc" "$REPO/target/release/thc" "$REPO/target/debug/thc"; do [ -x "$c" ] && { THC=$c; break; }; done
fi
[ -x "${THC:-}" ] || { echo "build first: cargo build --release -p thc" >&2; exit 1; }
THC=$(cd "$(dirname "$THC")" && pwd)/$(basename "$THC")
echo "rendering with $("$THC" --version)"

T=$(mktemp -d -t thc-site)
S=$T/personal   # the vault name shown in the header comes from here
mkdir -p "$OUT" "$S/home"
PIDS=()
cleanup() {
  # Ask each scratch daemon to stop (it knows its own HOME and vault), then kill what's left.
  for v in vault conflict; do THC_VAULT="$S/$v" THC_CACHE_DIR="$S/${v}cache" "$THC" daemon stop >/dev/null 2>&1 || true; done
  [ -d "$T/mhome" ] && env HOME="$T/mhome" XDG_CACHE_HOME="$T/mxdg" THC_CONFIG_DIR="$T/mconfig" THC_VAULT="$T/mv/personal" "$THC" daemon stop >/dev/null 2>&1 || true
  for p in "${PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null || true; done
  sleep 0.3; rm -rf "$T"
}
trap cleanup EXIT

export HOME="$S/home" XDG_CACHE_HOME="$S/xdg" THC_CONFIG_DIR="$S/config" THC_NO_UPDATE_CHECK=1 THC_SETUP_LOGIN=skip
export THC_NOW=2026-10-03T10:41 THC_TUI_RENDER=1 THC_FIXTURE_IDS=1 THC_DEVICE=docs
cd "$S"

THC="$THC" THC_VAULT="$S/vault" THC_CACHE_DIR="$S/vaultcache" "$REPO/scripts/seed-sample.sh" "$S/vault" >/dev/null
THC="$THC" THC_VAULT="$S/conflict" THC_CACHE_DIR="$S/conflictcache" "$REPO/scripts/seed-sample.sh" --conflict "$S/conflict" >/dev/null
for v in vault conflict; do
  THC_VAULT="$S/$v" THC_CACHE_DIR="$S/${v}cache" "$THC" daemon run >"$S/$v-daemon.log" 2>&1 &
  PIDS+=($!)
done
for v in vault conflict; do
  for _ in $(seq 1 50); do
    THC_VAULT="$S/$v" THC_CACHE_DIR="$S/${v}cache" "$THC" daemon status >/dev/null 2>&1 && break
    sleep 0.1
  done
done

render() { # name vault size keys cursor(1|0) [env...]
  local name=$1 vault=$2 size=$3 keys=$4 cursor=$5; shift 5
  for theme in ember-dark ember-light; do
    env "$@" THC_VAULT="$S/$vault" THC_CACHE_DIR="$S/${vault}cache" THC_THEME=$theme THC_TUI_SNAPSHOT=$size \
      THC_TUI_SNAPSHOT_FORMAT=html THC_TUI_SNAPSHOT_CURSOR=$cursor THC_TUI_KEYS="$keys" "$THC" tui |
      sed -n '/<pre>/,/<\/pre>/p' > "$OUT/$name-$theme.html"
  done
}

P='Booked the flat in Lisbon. It faces the river, and the light in the mornings is extraordinary.'
render today       vault    100x24 "1" 0
render journal     vault    100x24 "5$P<cr><cr>[ ] Call the landlord about the deposit due:tue !high #lisbon" 1
render link        vault    100x24 "5Read back through [[Q" 1
render page        vault    100x24 "4Q4<cr>" 1
render tasks       vault    100x24 "3" 0
render pages       vault    100x24 "4" 0
render search      vault    100x24 "6health" 0
render log         vault    100x24 "7" 0
render review      vault    100x24 "7r" 0
render conflict    conflict 100x24 "1" 0
render leader      vault    100x24 "1<space>" 0
render focus       vault    100x24 "5<m-z>" 1 THC_TUI_FOCUS=writer
render devlog      vault    100x24 '5<c-n>Shipped the parser fix. The cold start went from 2.1 ms to 1.4 ms.<cr><cr>[ ] Review the migration PR due:fri !high #api<cr>[ ] Write up the cache bug for [[Release checklist]]<cr>[ ] Pair on the flaky test sched:mon' 1
render compare     conflict 100x24 "1c" 0
render tokens      vault    100x24 '5<c-n>Write "due:fri" in quotes and it stays text. due:fri bare is a date, !high a priority, #lisbon a tag.' 1

# Several vaults (Today's vault column, the * scope picker, the ? recipe), in a HOME of their
# own so the frames above stay single-vault. The personal vault is seeded; acme and dev get a
# few keyed tasks.
multi() {
  env HOME="$T/mhome" XDG_CACHE_HOME="$T/mxdg" THC_CONFIG_DIR="$T/mconfig" "$@"
}
mkdir -p "$T/mhome" "$T/mv"
(cd "$T/mv" && multi env THC="$THC" THC_VAULT="$T/mv/personal" THC_CACHE_DIR="$T/mv/pcache" "$REPO/scripts/seed-sample.sh" "$T/mv/personal" >/dev/null)
(cd "$T/mv" && multi "$THC" init --global "$T/mv/personal" >/dev/null && multi "$THC" vault new acme >/dev/null && multi "$THC" vault new dev >/dev/null)
multi "$THC" todo --vault acme "Send the invoice" --due fri -p high --key web-a1 >/dev/null
multi "$THC" todo --vault acme "Draft the Q4 budget" --due today --key web-a2 >/dev/null
multi "$THC" todo --vault dev "Fix the flaky login test #api" --due today -p high --key web-d1 >/dev/null
multi "$THC" todo --vault dev "Review the migration PR" --due tue --key web-d2 >/dev/null
# Start the daemon with env directly (not through the multi function), so $! is the daemon itself.
env HOME="$T/mhome" XDG_CACHE_HOME="$T/mxdg" THC_CONFIG_DIR="$T/mconfig" THC_VAULT="$T/mv/personal" "$THC" daemon run >"$T/mdaemon.log" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 50); do multi env THC_VAULT="$T/mv/personal" "$THC" daemon status >/dev/null 2>&1 && break; sleep 0.1; done
render_multi() { # name keys
  for theme in ember-dark ember-light; do
    (cd "$T/mhome" && multi env THC_THEME=$theme THC_TUI_SNAPSHOT=110x24 THC_TUI_SNAPSHOT_FORMAT=html \
      THC_TUI_SNAPSHOT_CURSOR=0 THC_TUI_KEYS="$2" "$THC" tui) | sed -n '/<pre>/,/<\/pre>/p' > "$OUT/$1-$theme.html"
  done
}
render_multi vaults  "1"
render_multi scope   "1*"
render_multi recipe  "1?"

fail=0
for f in "$OUT"/*.html; do
  grep -q '<pre>' "$f" || { echo "no <pre>: $f" >&2; fail=1; }
  if grep -q 'THC_NOW is set' "$f"; then echo "pinned-clock warning: $f" >&2; fail=1; fi
done
[ $fail = 0 ] && echo "wrote $(ls "$OUT"/*.html | wc -l | tr -d ' ') frames to src/renders"
exit $fail
