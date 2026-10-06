#!/usr/bin/env bash
# Generate the reference pages' raw material from the thc binary, in a scratch HOME (never the
# real one): the keys table and the command help. Output: src/generated/.
# Usage (from web/): THC=/path/to/thc npm run gen   (default: ~/.local/bin/thc, else target/release)
set -euo pipefail
SITE=$(cd "$(dirname "$0")/.." && pwd)
THC=${THC:-}
for c in "$HOME/.local/bin/thc" "$SITE/../target/release/thc"; do [ -z "$THC" ] && [ -x "$c" ] && THC=$c; done
[ -x "${THC:-}" ] || { echo "no thc binary" >&2; exit 1; }
T=$(mktemp -d -t thc-web-gen); trap 'rm -rf "$T"' EXIT
OUT="$SITE/src/generated"; mkdir -p "$OUT"
run() { env -i HOME="$T/home" PATH=/usr/bin:/bin TERM=xterm-256color THC_CONFIG_DIR="$T/config" XDG_CACHE_HOME="$T/xdg" \
  THC_NO_UPDATE_CHECK=1 THC_SETUP_LOGIN=skip THC_VAULT="$T/vault" THC_CACHE_DIR="$T/cache" "$THC" "$@"; }
mkdir -p "$T/home"; (cd "$T" && run init "$T/vault" >/dev/null 2>&1) || true
run --version > "$OUT/version.txt"
run keys --markdown > "$OUT/keys.md"
run --help > "$OUT/help.txt"
for t in $(run instructions --help 2>/dev/null | sed -n 's/^  \([a-z-]*\) .*/\1/p' | head -0); do :; done
run instructions all > "$OUT/instructions.md" 2>/dev/null || true
# Scratch paths must never reach the repo: drop env annotations that name them, and any $T path.
for f in "$OUT"/*.txt "$OUT"/*.md; do sed -i.bak -E -e 's# \[env: THC_VAULT=[^]]*\]##g' -e "s#$T#~#g" "$f" && rm -f "$f.bak"; done
echo "generated from $(cat "$OUT/version.txt") into src/generated"
