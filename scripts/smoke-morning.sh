#!/usr/bin/env bash
# Before installing a build: release thc from this checkout (meant for overnight/main), the
# flow suite, the perf flows, scripts/bench-editor.sh and scripts/bench-live.sh, then one screen
# of summary. Full logs stay in a folder it names. Exits 1 if anything failed or went over budget.
# Usage: scripts/smoke-morning.sh        (CARGO_TARGET_DIR is honoured; TMPDIR too)
set -uo pipefail
cd "$(git rev-parse --show-toplevel)"
CARGO=(cargo)
command -v cargo >/dev/null || CARGO=(mise exec -- cargo)
TARGET=${CARGO_TARGET_DIR:-$PWD/target}
LOGS=$(mktemp -d -t thc-smoke)
branch=$(git rev-parse --abbrev-ref HEAD)
rev=$(git rev-parse --short HEAD)
fail=0
step() { printf '%-12s' "$1"; }
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fail=1; }

echo "thc smoke · $branch @ $rev · $(date '+%a %d %b %H:%M') · logs: $LOGS"
[ "$branch" = overnight/main ] || echo "(not overnight/main: this is $branch)"
[ -z "$(git status --porcelain --untracked-files=no)" ] || echo "(uncommitted changes in this checkout)"
echo

step build
if "${CARGO[@]}" build --release -p thc >"$LOGS/build.log" 2>&1; then
  ok "$("$TARGET/release/thc" --version 2>/dev/null)"
else
  bad "cargo build --release -p thc (build.log)"
fi

step flows
"${CARGO[@]}" test -p thc-tui flows >"$LOGS/flows.log" 2>&1
r=$(grep -E "^test result" "$LOGS/flows.log" | head -1)
if grep -q "^test result: ok" "$LOGS/flows.log"; then
  ok "$(echo "$r" | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1 passed, \2 failed, \3 ignored/')"
else
  bad "${r:-no result} (flows.log)"
  grep -E "^test .* FAILED" "$LOGS/flows.log" | head -5 | sed 's/^/             /'
fi

step perf
"${CARGO[@]}" test --release -p thc-tui flows::perf -- --ignored --nocapture --test-threads=1 >"$LOGS/perf.log" 2>&1
over=$(grep -c "over budget" "$LOGS/perf.log")
worst=$(grep -E "^perf |\.\.\. perf " "$LOGS/perf.log" | sed -E 's/.*perf (.*) n=.*p50 +([0-9.]+) ms +p99 +([0-9.]+) ms +\(budget ([0-9.]+) ms\).*/\3 \4 \1 (p50 \2)/' | awk '{print $1/$2, $0}' | sort -rn | head -1 | cut -d' ' -f2-)
if grep -q "^test result: ok" "$LOGS/perf.log"; then
  ok "all within budget · closest: p99 $(echo "$worst" | awk '{printf "%s of %s ms:", $1, $2; $1=$2=""; print}')"
else
  bad "$over over budget (perf.log)"
  grep -A0 -E "over budget|p99 .* over" "$LOGS/perf.log" | head -4 | sed 's/^/             /'
fi
grep -E "^perf |\.\.\. perf " "$LOGS/perf.log" | sed -E 's/^test [^ ]+ \.\.\. //; s/^perf /             /; s/ +n=[0-9]+ +/  /; s/ +\(budget/ (budget/'

step bench
if THC="$TARGET/release/thc" bash scripts/bench-editor.sh >"$LOGS/bench-editor.log" 2>&1; then
  ok "bench-editor.sh"
else
  bad "bench-editor.sh (bench-editor.log)"
fi
# One line per run: its section, what it did, its p50 / p99 / max.
awk '/^==/{sub(/^== /,""); s=$0; next} /keys ·/{line=$0; sub(/first frame [0-9.]+ ms /,"",line); gsub(/  +/," ",line); print "             " s ": " line}' "$LOGS/bench-editor.log" | head -8

step live
if THC="$TARGET/release/thc" bash scripts/bench-live.sh >"$LOGS/bench-live.log" 2>&1; then
  ok "bench-live.sh (a real tui on a pty, live daemon)"
else
  bad "bench-live.sh (bench-live.log)"
fi
grep -E "keys ·|in thc:|over 4 ms" "$LOGS/bench-live.log" | grep -vE "^ +in thc over" | sed -E 's/^   in thc: /               in thc: /; s/^([0-9,]+ lines)/             \1/' | head -12

echo
if [ $fail = 0 ]; then echo "SMOKE OK · $branch @ $rev"; else echo "SMOKE FAILED · $branch @ $rev · see $LOGS"; fi
exit $fail
