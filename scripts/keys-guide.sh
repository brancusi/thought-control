#!/usr/bin/env bash
# Regenerates the key tables in docs/guide/keys.md from the keymap (`thc keys --markdown`,
# keymap.md §7), between the keys:begin / keys:end markers. Usage: scripts/keys-guide.sh [thc]
set -euo pipefail
THC=${1:-${THC:-$PWD/target/debug/thc}}
PAGE=docs/guide/keys.md
TABLES=$(mktemp)
trap 'rm -f "$TABLES"' EXIT
"$THC" keys --markdown > "$TABLES"
python3 - "$PAGE" "$TABLES" <<'PY'
import sys
page, tables = sys.argv[1], open(sys.argv[2]).read().rstrip("\n")
s = open(page).read()
a, b = "<!-- keys:begin (generated: scripts/keys-guide.sh) -->", "<!-- keys:end -->"
i, j = s.index(a) + len(a), s.index(b)
open(page, "w").write(s[:i] + "\n" + tables + "\n" + s[j:])
PY
echo "wrote $PAGE"
