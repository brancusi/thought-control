#!/usr/bin/env bash
# Typing in a real `thc tui` on a pty, with a live daemon, end to end (scripts/bench-live.py):
# per-key latency from the key written to its frame flushed, through the writer thread, idle
# and line-leave saves and the daemon's pushes. A scratch vault and HOME; the daemon is this
# vault's own and stops on exit. Pages of 300 and 5,000 lines; then the same with no daemon.
# Exits 1 when a run's p99 in thc (its own key-to-frame) is over 4 ms, or end to end (the pty,
# the scheduler too, so noisier on a busy machine) over 8 ms.
# Usage: scripts/bench-live.sh   (builds target/release/thc first if needed; THC=… to pick one)
set -euo pipefail
THC=${THC:-$PWD/target/release/thc}
[ -x "$THC" ] || cargo build --release -q -p thc
HERE=$(cd "$(dirname "$0")" && pwd)
S=$(mktemp -d -t thc-bench-live)
export THC_VAULT="$S/vault" THC_CACHE_DIR="$S/cache" HOME="$S/home" THC_NO_UPDATE_CHECK=1 THC_NOTIFY=log
stop() { "$THC" daemon stop >/dev/null 2>&1 || true; rm -rf "$S"; }
trap stop EXIT
mkdir -p "$S/home"
(cd "$S" && "$THC" init "$S/vault" >/dev/null)
python3 - "$S/big.md" "$S/small.md" <<'PY'
import sys
words = "the quick brown fox jumps over a lazy dog while thinking about lunch and the weather".split()
def line(i, n=12):
    return " ".join(words[(i + k) % len(words)] for k in range(n))
for path, n in ((sys.argv[1], 5000), (sys.argv[2], 300)):
    with open(path, "w") as f:
        for i in range(n):
            depth = (i % 7 == 3) + (i % 21 == 10)
            f.write("  " * depth + "- " + ("[ ] " if i % 9 == 0 else "") + f"{line(i)} {i}\n")
PY
for t in "Big page:big" "Small page:small"; do
  id=$("$THC" --json page new "${t%%:*}" | python3 -c 'import sys,json;print(json.load(sys.stdin)["nodes"][0]["id"])')
  "$THC" import --under "$id" "$S/${t##*:}.md" >/dev/null
done
status=0
# In the TUI itself (THC_TUI_TRACE=1): its own key-to-frame time, and where each key over 4 ms
# went, phase by phase. End to end minus this is the terminal and the scheduler.
export THC_TUI_TRACE=1
run() {
  rm -f "$S/cache/tui-trace.log" "$S/cache/tui-trace-slow.log"
  python3 "$HERE/bench-live.py" "$THC" --budget 8 "$@" || status=1
  if [ -f "$S/cache/tui-trace.log" ]; then
    sed 's/^/   in thc: /' "$S/cache/tui-trace.log"
    p99=$(sed -E 's/.*p99 ([0-9.]+) ms.*/\1/' "$S/cache/tui-trace.log" | tail -1)
    if python3 -c "import sys; sys.exit(0 if float('$p99') <= 4 else 1)"; then :; else echo "   in thc p99 $p99 ms is over 4 ms"; status=1; fi
  fi
  if [ -f "$S/cache/tui-trace-slow.log" ]; then
    echo "   in thc over 4 ms: $(wc -l < "$S/cache/tui-trace-slow.log" | tr -d ' ') keys, slowest:"
    sort -rn "$S/cache/tui-trace-slow.log" | head -3 | sed 's/^/     /'
  fi
}
"$THC" daemon start >/dev/null
"$THC" daemon status --json | python3 -c 'import sys,json; d=json.load(sys.stdin); assert d["state"]=="live", d'
echo "== live daemon"
run --page "Small page" --label "300 lines, live daemon"
run --page "Big page" --label "5,000 lines, live daemon"
run --page "Big page" --pace 30 --label "5,000 lines, live daemon, fast"
"$THC" daemon stop >/dev/null
echo "== no daemon"
run --page "Small page" --label "300 lines"
run --page "Big page" --label "5,000 lines"
exit $status
