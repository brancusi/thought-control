#!/usr/bin/env bash
# Time read commands against a generated vault (scripts/gen-vault.sh), for the SPEC §8 budgets
# (< 5 ms for `today --json` / `q …`). A stand-in for hyperfine: N runs per command after 10
# warm-ups, wall clock per process, reporting mean / p50 / p95. `/usr/bin/true` is timed the
# same way as the floor for starting a process.
#
# To compare builds, pass several as name=binary[@cache-dir]. Runs are interleaved (A B A B …)
# so thermal or background drift hits every build equally.
#
# Usage: scripts/bench-reads.sh <gen-dir> [runs] [name=thc[@cache] ...]
#   QUERIES="q1;q2" overrides the default thc q queries (separated by ;).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
DIR=${1:?usage: bench-reads.sh <gen-dir> [runs] [name=thc[@cache] ...]}
RUNS=${2:-300}
shift $(( $# >= 2 ? 2 : 1 ))
BUILDS=("$@")
[ ${#BUILDS[@]} -eq 0 ] && BUILDS=("thc=$ROOT/target/release/thc")
export THC_VAULT=$DIR/vault THC_NOW=2026-10-03T10:00
unset THC_ACTOR THC_CONTEXT
python3 - "$DIR" "$RUNS" "${QUERIES:-}" "${BUILDS[@]}" <<'PY'
import os, subprocess, sys, time, statistics
d, runs, custom, builds = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4:]
queries = [q for q in custom.split(";") if q] or ["status:open #work sort:due", "status:open due<=+3d", "is:ready"]
bs = []
for b in builds:
    name, rest = b.split("=", 1)
    exe, _, cache = rest.partition("@")
    env = dict(os.environ, THC_CACHE_DIR=cache or os.environ.get("THC_CACHE_DIR", f"{d}/cache"))
    bs.append((name, exe, env))
cmds = [("true", None), ("today --json", ["today", "--json"])] + [(f"q '{q}'", ["q", q, "--json"]) for q in queries]
def argv(exe, a):
    return ["/usr/bin/true"] if a is None else [exe] + a
print(f"{'command':<40}" + "".join(f"{n:>26}" for n, _, _ in bs) + "     (mean / p95 ms, n=%d)" % runs)
for label, a in cmds:
    ok = []
    for name, exe, env in bs:
        r = subprocess.run(argv(exe, a), capture_output=True, env=env)
        ok.append(r.returncode == 0)
        for _ in range(10 if r.returncode == 0 else 0):
            subprocess.run(argv(exe, a), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=env)
    ts = [[] for _ in bs]
    for _ in range(runs):
        for i, (name, exe, env) in enumerate(bs):
            if not ok[i]:
                continue
            t = time.perf_counter()
            subprocess.run(argv(exe, a), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=env)
            ts[i].append((time.perf_counter() - t) * 1000)
    cells = []
    for i in range(len(bs)):
        if not ok[i]:
            cells.append(f"{'(not supported)':>26}")
            continue
        s = sorted(ts[i])
        cells.append(f"{statistics.mean(s):>12.2f} / {s[int(len(s) * 0.95) - 1]:<11.2f}")
    print(f"{label:<40}" + "".join(cells), flush=True)
PY
