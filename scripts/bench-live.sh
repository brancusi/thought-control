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
# Each variant runs RUNS times (default 3) and is gated on the median p99s: one run that a busy
# machine slowed down shows, but doesn't fail the smoke (vw384: the same build swung 2.0-6.3 ms
# p99 run to run at load 4-7, every phase slower together).
RUNS=${BENCH_LIVE_RUNS:-3}
median() { python3 -c 'import sys,statistics; print(f"{statistics.median(float(x) for x in sys.argv[1:]):.2f}")' "$@"; }
run() {
  local e2e=() inthc=() label=""
  local prev=""
  for a in "$@"; do [ "$prev" = "--label" ] && label=$a; prev=$a; done
  for i in $(seq 1 "$RUNS"); do
    rm -f "$S/cache/tui-trace.log" "$S/cache/tui-trace-slow.log"
    local out
    out=$(python3 "$HERE/bench-live.py" "$THC" --budget 8 "$@" 2>&1 || true)
    echo "$out" | sed "s/^/  [$i] /"
    echo "  [$i]   load:$(uptime | sed 's/.*load average[s]*://')"
    e2e+=("$(echo "$out" | sed -nE 's/.* p99 ([0-9.]+) ms.*/\1/p' | head -1 || true)")
    if [ -f "$S/cache/tui-trace.log" ]; then
      sed "s/^/  [$i]   in thc: /" "$S/cache/tui-trace.log"
      inthc+=("$(sed -E 's/.*p99 ([0-9.]+) ms.*/\1/' "$S/cache/tui-trace.log" | tail -1)")
    fi
    if [ -f "$S/cache/tui-trace-slow.log" ]; then
      echo "  [$i]   in thc over 4 ms: $(wc -l < "$S/cache/tui-trace-slow.log" | tr -d ' ') keys, slowest:"
      sort -rn "$S/cache/tui-trace-slow.log" | head -3 | sed 's/^/         /'  || true
    fi
  done
  local me mt=""
  me=$(median "${e2e[@]}")
  [ ${#inthc[@]} -gt 0 ] && mt=$(median "${inthc[@]}")
  local verdict=ok
  python3 -c "import sys; sys.exit(0 if float('$me') <= 8 else 1)" || { verdict="over: end to end median p99 $me ms > 8"; status=1; }
  if [ -n "$mt" ] && ! python3 -c "import sys; sys.exit(0 if float('$mt') <= 4 else 1)"; then verdict="over: in thc median p99 $mt ms > 4"; status=1; fi
  echo "$(printf '%-34s' "$label") median of $RUNS · p99 end to end $me ms (≤ 8) · in thc ${mt:-?} ms (≤ 4) · $verdict"
}
echo "load:$(uptime | sed 's/.*load average[s]*://')"
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
