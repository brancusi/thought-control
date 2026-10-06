#!/usr/bin/env bash
# Generate a large, deterministic vault for the SPEC §8 performance budgets: about 50k nodes
# and 200k events, with pages, nested tasks, journal days, tags, priorities, links and alerts,
# so structural queries (`under:`, `ancestor:(…)`, `has:child(…)`) have real trees to walk.
#
# It reads like two lived-in years: each chunk is written at its own THC_NOW, from
# 2024-10-03 up to 2026-10-03. Older work is almost all done, and open tasks cluster in
# the last weeks. So `today` shows dozens of rows, not thousands, as a real vault would.
#
# Usage: scripts/gen-vault.sh <empty-dir> [nodes]     (THC=path/to/thc to pick the binary)
# Never point it at a real vault: it refuses a directory that already has a vault in it.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
DIR=${1:?usage: gen-vault.sh <empty-dir> [nodes]}
NODES=${2:-50000}
THC=${THC:-$ROOT/target/release/thc}
[ -e "$DIR/vault/thc-vault.toml" ] && { echo "gen-vault: $DIR already has a vault; pick an empty dir" >&2; exit 2; }
mkdir -p "$DIR"
export THC_VAULT=$DIR/vault THC_CACHE_DIR=$DIR/cache THC_NOW=2026-10-03T10:00
unset THC_CONTEXT
(cd "$DIR" && "$THC" init vault >/dev/null)

# One JSONL plan per chunk; each chunk is one transaction. References (`as`/`$name`) only work
# within a chunk, so each chunk is a self-contained set of projects.
python3 - "$DIR/plans" "$NODES" <<'PY'
import datetime, json, os, random, sys
out, total = sys.argv[1], int(sys.argv[2])
start = datetime.date(2024, 10, 3)
os.makedirs(out, exist_ok=True)
rnd = random.Random(7)
tags = ["work", "home", "health", "admin", "reading", "q4", "client", "someday", "errand", "deep"]
words = "plan draft review call email book fix write ship collect schedule renew order clean read sketch".split()
nouns = "deck report budget invoice venue flights faucet metrics roadmap notes contract filter garden taxes slides".split()
def text():
    t = f"{rnd.choice(words).title()} the {rnd.choice(nouns)}"
    for _ in range(rnd.choice([0, 0, 1, 2])):
        t += f" #{rnd.choice(tags)}"
    return t
def rel():
    return f"{rnd.randint(-5, 30):+d}d"
n = chunk = 0
while n < total:
    lines = []
    # Dated by how far through the vault we are, so the last chunk lands in the final days.
    frac = min(1.0, n / total)
    now = start + datetime.timedelta(days=round(730 * frac))
    recent = (datetime.date(2026, 10, 3) - now).days <= 21
    p_open = 0.25 if recent else 0.002        # what's still open from that time
    for p in range(6):                        # 6 projects per chunk
        if n >= total: break
        page = f"p{chunk}_{p}"
        lines.append({"cmd": "add", "text": f"Project {chunk}-{p} #project", "inbox": True, "as": page}); n += 1
        for t in range(rnd.randint(20, 60)):  # tasks under the project, some with subtasks
            if n >= total: break
            tid = f"{page}_t{t}"
            op = {"cmd": "todo", "text": text(), "under": f"${page}", "as": tid}
            if rnd.random() < 0.6: op["due"] = rel()
            if rnd.random() < 0.3: op["sched"] = rel()
            if rnd.random() < 0.25: op["priority"] = rnd.choice(["high", "med", "low"])
            # Repeating tasks never close (done advances them), so only recent ones exist.
            if recent and rnd.random() < 0.05: op["repeat"] = rnd.choice(["every week", "every 3 days", "every month"])
            lines.append(op); n += 1
            for s in range(rnd.choice([0, 0, 0, 1, 2, 3])):
                if n >= total: break
                lines.append({"cmd": "todo", "text": text(), "under": f"${tid}", "as": f"{tid}_s{s}", **({"due": rel()} if rnd.random() < 0.4 else {})}); n += 1
                if rnd.random() > p_open: lines.append({"cmd": "done", "id": f"${tid}_s{s}"})
            # History, so the log is ~4 events per node like a lived-in vault.
            for _ in range(rnd.choice([2, 3, 4, 5, 6])):
                lines.append({"cmd": "set", "id": f"${tid}", "props": {"priority": rnd.choice(["high", "med", "low"])}})
            if rnd.random() > p_open: lines.append({"cmd": "done", "id": f"${tid}"})
            if recent and rnd.random() < 0.05: lines.append({"cmd": "alert", "id": f"${tid}", "before": "1d"} if "due" in op else {"cmd": "alert", "id": f"${tid}", "at": "today 5pm"})
            if t > 0 and rnd.random() < 0.05: lines.append({"cmd": "link", "a": f"{page}_t{t-1}", "b": f"${tid}", "rel": "blocks"})
    for j in range(150):                      # journal notes, the bulk of a real vault
        if n >= total: break
        day = (now - datetime.timedelta(days=rnd.randint(0, 6))).isoformat()
        lines.append({"cmd": "add", "text": text(), "journal": day}); n += 1
    with open(f"{out}/{chunk:05d}_{now.isoformat()}.jsonl", "w") as f:
        for l in lines:
            if l.get("cmd") == "link": l["a"] = "$" + l["a"]
            f.write(json.dumps(l) + "\n")
    chunk += 1
PY
for f in "$DIR"/plans/*.jsonl; do
    day=$(basename "$f" .jsonl); day=${day#*_}
    THC_NOW="${day}T10:00" "$THC" apply "$f" >/dev/null
done
"$THC" doctor --json 2>/dev/null | python3 -c 'import sys,json; d=json.load(sys.stdin); print("gen-vault:", {k: d[k] for k in ("nodes","events") if k in d})' || true
