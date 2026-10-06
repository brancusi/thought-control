#!/usr/bin/env bash
# Regenerates the guide's screenshots (docs/design/guide-renders.md) from scratch vaults, each
# served by its own scratch daemon (never launchd), with the clock pinned. Commit the output.
# Usage: scripts/guide-renders.sh [out-dir]   (default docs/guide/renders; uses target/release/thc)
set -euo pipefail
OUT=${1:-docs/guide/renders}
THC=${THC:-$PWD/target/release/thc}
[ -x "$THC" ] || { echo "build first: cargo build --release -p thc" >&2; exit 1; }
S=$(mktemp -d -t thc-guide)
mkdir -p "$OUT" "$S/home"
PIDS=()
cleanup() { for p in "${PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null || true; done; sleep 0.3; rm -rf "$S"; }
trap cleanup EXIT
# A scratch HOME and config: nothing from the person's own setup shapes the renders.
export HOME="$S/home" XDG_CACHE_HOME="$S/xdg" THC_CONFIG_DIR="$S/config" THC_NO_UPDATE_CHECK=1 THC_SETUP_LOGIN=skip
export THC_NOW=2026-10-03T10:41 THC_TUI_RENDER=1
# The same vault on every run: the pinned clock stamps the log, ids derive from it and this
# device, and the seed keys its creates, so renders only change when the UI does.
export THC_FIXTURE_IDS=1 THC_DEVICE=guide
THC="$THC" THC_VAULT="$S/vault" THC_CACHE_DIR="$S/vaultcache" scripts/seed-sample.sh "$S/vault" >/dev/null
(cd "$S" && "$THC" init "$S/empty" >/dev/null)
for v in vault empty; do
  THC_VAULT="$S/$v" THC_CACHE_DIR="$S/${v}cache" "$THC" daemon run >"$S/$v-daemon.log" 2>&1 &
  PIDS+=($!)
done
for v in vault empty; do
  for _ in $(seq 1 50); do
    THC_VAULT="$S/$v" THC_CACHE_DIR="$S/${v}cache" "$THC" daemon status >/dev/null 2>&1 && break
    sleep 0.1
  done
done
render() { # name vault keys cursor(1|0) [env...]
  local name=$1 vault=$2 keys=$3 cursor=$4; shift 4
  for theme in ember-dark ember-light; do
    env "$@" THC_VAULT="$S/$vault" THC_CACHE_DIR="$S/${vault}cache" THC_THEME=$theme THC_TUI_SNAPSHOT=100x26 \
      THC_TUI_SNAPSHOT_FORMAT=html THC_TUI_SNAPSHOT_CURSOR=$cursor THC_TUI_KEYS="$keys" "$THC" tui |
      sed -n '/<pre>/,/<\/pre>/p' > "$OUT/$name-$theme.html"
  done
}
render 01-first-open empty "5" 1
render 02-writing vault "5Booked the flat in Lisbon. It faces the river, and the light in the mornings is extraordinary.<cr><cr>[ ] Call the landlord about the deposit due:tue !high #lisbon" 1
render 03-folded vault "5Booked the flat in Lisbon. It faces the river.<cr><cr>[ ] Call the landlord about the deposit due:tue !high #lisbon<cr>" 1
render 04-link vault "5Read back through [[Q" 1
render 05-today vault "1" 0
render 06-tasks vault "3" 0
render 07-review vault "7r" 0
render 08-focus vault "5<m-z>" 1 THC_TUI_FOCUS=writer
render 09-help vault "5<f1>" 0
render 10-leader vault "1<space>" 0
fail=0
for f in "$OUT"/*.html; do
  grep -q '<pre>' "$f" || { echo "no <pre>: $f" >&2; fail=1; }
  if grep -q 'daemon offline' "$f"; then echo "daemon offline: $f" >&2; fail=1; fi
  if grep -q 'THC_NOW is set' "$f"; then echo "pinned-clock warning: $f" >&2; fail=1; fi
done
n=$(ls "$OUT"/*.html | wc -l | tr -d ' ')
[ "$n" -ge 20 ] || { echo "expected 20 renders, got $n" >&2; fail=1; }
[ $fail = 0 ] && echo "wrote $n renders to $OUT"
exit $fail
