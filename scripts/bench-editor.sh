#!/usr/bin/env bash
# The editor against tui-editor.md §10's budgets, in release mode, on a scratch vault: a
# 5,000-line page and a 300-line journal day. Prints the first frame, keystroke p50/p99 while
# typing (at the end and in the middle), holding PgDn, and a 1,000-line paste.
# Usage: scripts/bench-editor.sh   (builds target/release/thc first if needed)
set -euo pipefail
THC=${THC:-$PWD/target/release/thc}
[ -x "$THC" ] || cargo build --release -q -p thc
S=$(mktemp -d -t thc-bench)
trap 'rm -rf "$S"' EXIT
export THC_VAULT="$S/vault" THC_CACHE_DIR="$S/cache" HOME="$S/home" THC_NO_UPDATE_CHECK=1 THC_TUI_RENDER=1
mkdir -p "$S/home"
(cd "$S" && "$THC" init "$S/vault" >/dev/null)
page=$("$THC" --json page new "Big page" | python3 -c 'import sys,json;print(json.load(sys.stdin)["nodes"][0]["id"])')
python3 - "$S/page.md" "$S/day.md" "$S/paste.md" <<'PY'
import sys
words = "the quick brown fox jumps over a lazy dog while thinking about lunch and the weather".split()
def line(i, n=12):
    return " ".join(words[(i + k) % len(words)] for k in range(n))
with open(sys.argv[1], "w") as f:
    for i in range(5000):
        depth = (i % 7 == 3) + (i % 21 == 10)
        f.write("  " * depth + "- " + ("[ ] " if i % 9 == 0 else "") + f"{line(i)} {i}\n")
with open(sys.argv[2], "w") as f:
    for i in range(300):
        f.write("- " + ("[ ] " if i % 5 == 0 else "") + f"{line(i, 9)} {i}\n")
with open(sys.argv[3], "w") as f:
    f.write("\\n".join(f"- pasted line {i} {line(i, 6)}" for i in range(1000)))
PY
"$THC" import --under "$page" "$S/page.md" >/dev/null
"$THC" import --journal today "$S/day.md" >/dev/null
typing=$(python3 -c 'print("typing a sentence into the document, word by word. " * 4)')
run() { # label keys thc-args...
  local label=$1 keys=$2; shift 2
  THC_TUI_TRACE=1 THC_TUI_SNAPSHOT=120x40 THC_TUI_KEYS="$keys" "$THC" "$@" 2>&1 >/dev/null | grep "thc tui trace" | sed "s/^thc tui trace:/$label/" | tr '\n' ' '
  echo
}
echo "== 5,000-line page"
run "open         " "" p "Big page"
run "type at top  " "$typing" p "Big page"
run "type middle  " "$(printf '<pgdn>%.0s' $(seq 1 125))$typing" p "Big page"
run "PgDn x200    " "$(printf '<pgdn>%.0s' $(seq 1 200))" p "Big page"
run "paste 1000   " "<paste:$(cat "$S/paste.md")>" p "Big page"
echo "== 300-line journal day"
run "open         " "" j
run "type at end  " "$typing" j
