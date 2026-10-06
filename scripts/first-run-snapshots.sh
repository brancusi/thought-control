#!/usr/bin/env bash
# First-run review snapshots, in a
# scratch HOME: no real vault, launchd or PATH is touched.
# Usage: scripts/first-run-snapshots.sh <out-dir>   (uses ./target/debug/thc)
set -euo pipefail
OUT=${1:?out dir}
THC=${THC:-$PWD/target/debug/thc}
ROOT=$(mktemp -d -t thc-first-run)
mkdir -p "$OUT" "$ROOT/home"
run() { # a clean environment, as a new person's terminal would be
  (cd / && env -i HOME="$ROOT/home" PATH=/usr/bin:/bin SHELL=/bin/zsh TERM=xterm-256color LANG=en_US.UTF-8 \
    THC_TEST=1 THC_TEST_ROOT="$ROOT" THC_SETUP_LOGIN=skip THC_SETUP_APP=none THC_NO_UPDATE_CHECK=1 "$@")
}
run "$THC" setup --all --yes --agents none > "$OUT/setup-fresh.txt" 2>&1
run "$THC" setup --all --yes --agents none > "$OUT/setup-rerun.txt" 2>&1
run "$THC" > "$OUT/thc-piped-empty.txt" 2>&1 < /dev/null
run "$THC" prime > "$OUT/prime-empty.txt" 2>&1
for theme in ember-dark ember-light; do
  run env THC_THEME=$theme THC_TUI_SNAPSHOT=120x36 THC_TUI_SNAPSHOT_FORMAT=html "$THC" tui > "$OUT/tui-today-empty-first-run-$theme.html"
  run env THC_THEME=$theme THC_TUI_SNAPSHOT=120x36 THC_TUI_SNAPSHOT_FORMAT=html "$THC" j --no-focus > "$OUT/tui-journal-first-open-$theme.html"
done
run env THC_TUI_SNAPSHOT=120x36 "$THC" tui > "$OUT/tui-today-empty-first-run.txt"
run env THC_TUI_SNAPSHOT=120x36 "$THC" j --no-focus > "$OUT/tui-journal-first-open.txt"
run env THC_TUI_SNAPSHOT=120x36 "$THC" j > "$OUT/tui-journal-first-open-focus.txt"
rm -rf "$ROOT"
echo "wrote $(ls "$OUT" | wc -l | tr -d ' ') files to $OUT"
