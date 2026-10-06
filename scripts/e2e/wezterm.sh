#!/usr/bin/env bash
# Real-terminal E2E: thc j in a fresh WezTerm process (its own config and class, a scratch vault),
# typed into through WezTerm's key encoder with the kitty protocol on; prints the screen and what
# was saved. Opens a window for ~10 s. Usage: scripts/e2e/wezterm.sh [thc-binary]
#
# Limit: SendKey makes synthetic key events. They go through WezTerm's encoder, but not the raw
# macOS event path (where 20240203 lost Shift under "report all keys"), nor a multiplexer or
# Karabiner in between. A pass here proves the encoder path; real key presses need a person, or
# System Events keystrokes with Accessibility permission.
set -euo pipefail
THC_BIN=${1:-$PWD/target/debug/thc}
WEZ=${WEZTERM:-/Applications/WezTerm.app/Contents/MacOS/wezterm}
ROOT=$(mktemp -d -t thc-e2e-wez)
mkdir -p "$ROOT/home"
export THC_BIN E2E_ROOT="$ROOT" E2E_HOME="$ROOT/home" E2E_VAULT="$ROOT/vault" E2E_CACHE="$ROOT/cache" E2E_OUT="$ROOT/screen.txt"
(cd "$ROOT" && HOME="$ROOT/home" THC_TEST=1 THC_TEST_ROOT="$ROOT" "$THC_BIN" init "$ROOT/vault" >/dev/null)
"$WEZ" --config-file "$(dirname "$0")/wezterm.lua" start --always-new-process --class dev.thought.e2e >/dev/null 2>&1 || true
echo "--- screen"
grep -v '^ *$' "$E2E_OUT" || echo "(no screen captured)"
echo "--- saved"
HOME="$ROOT/home" THC_TEST=1 THC_TEST_ROOT="$ROOT" THC_VAULT="$ROOT/vault" THC_CACHE_DIR="$ROOT/cache" "$THC_BIN" --json q 'journal=today' |
  python3 -c 'import sys,json; [print(repr(i["text"])) for i in json.load(sys.stdin)["items"]]'
rm -rf "$ROOT"
