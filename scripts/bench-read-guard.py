#!/usr/bin/env python3
"""SPEC §8 read guard: deterministic 50k scratch vault, warm per-process timings.

Prints JSON (rows, mean/p50/p95/stdev ms) and fails on errors or a coarse mean
threshold. The threshold is a regression gate, not the developer-machine budget.
THC selects a built release binary; --fixture reuses an existing scratch gen-dir.
"""
import argparse
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", type=Path)
    parser.add_argument("--runs", type=int, default=60)
    parser.add_argument("--max-ms", type=float, default=60)
    args = parser.parse_args()
    if args.runs < 20 or args.max_ms <= 0:
        parser.error("use at least 20 runs and a positive threshold")
    repo = Path(__file__).resolve().parent.parent
    binary = Path(os.environ.get("THC", repo / "target/release/thc")).resolve()
    with tempfile.TemporaryDirectory(prefix="thc-read-guard-") as tmp:
        root = Path(tmp)
        fixture = args.fixture.resolve() if args.fixture else root / "fixture"
        if args.fixture and not any(fixture.is_relative_to(p.resolve()) for p in (Path(tempfile.gettempdir()), Path("/tmp"))):
            parser.error("--fixture must be in a temporary directory (scratch vaults only)")
        env = dict(os.environ, HOME=str(root / "home"), THC_CONFIG_DIR=str(root / "config"),
                   THC_TEST="1", THC_TEST_ROOT=str(fixture.parent), THC_NO_UPDATE_CHECK="1",
                   THC_VAULT=str(fixture / "vault"), THC_CACHE_DIR=str(fixture / "cache"),
                   THC_NOW="2026-10-03T10:00", THC_ACTOR=os.environ.get("THC_ACTOR", "read-benchmark"),
                   THC=str(binary))
        for name in ("THC_BOARD", "THC_CONTEXT", "THC_ROLE", "THC_SESSION_ID", "THC_TEST_PARENT"):
            env.pop(name, None)
        for name in ("home", "config"):
            (root / name).mkdir()
        # Large fixture writes are permitted only in this disposable config/vault.
        (root / "config/config.toml").write_text('[actors.default-agent]\ntier = "full"\n')
        if not args.fixture:
            subprocess.run(["bash", str(repo / "scripts/gen-vault.sh"), str(fixture), "50000"],
                           env=env, cwd=root, check=True, stdout=subprocess.DEVNULL)
        def run(argv):
            result = subprocess.run(argv, env=env, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            if result.returncode:
                raise RuntimeError(f"{argv}: exit {result.returncode}: {result.stderr.decode()}")
            return result.stdout
        size = json.loads(run([str(binary), "doctor", "--json"]))
        if size["nodes"] < 50000 or size["events"] < 190000:
            raise RuntimeError(f"fixture too small: {size['nodes']} nodes / {size['events']} events")
        cases = [("process_floor", ["/usr/bin/true"]), ("empty", [str(binary), "q", "title=zzzz", "--json"]),
                 ("tagged", [str(binary), "q", "status:open #work sort:due", "--json", "--limit", "100"]),
                 ("due", [str(binary), "q", "status:open due<=+3d", "--json", "--limit", "100"]),
                 ("ready", [str(binary), "q", "is:ready", "--json", "--limit", "100"]),
                 ("today", [str(binary), "today", "--json"])]
        samples = {name: [] for name, _ in cases}
        rows = {}
        for name, argv in cases:
            for _ in range(10):
                data = run(argv)
            if name != "process_floor":
                listing = json.loads(data)
                rows[name] = listing.get("count", sum(len(listing.get(k, [])) for k in ("overdue", "today", "done")))
        if rows["empty"] != 0 or not all(rows[k] > 0 for k in ("tagged", "due", "ready", "today")):
            raise RuntimeError(f"fixture queries returned unexpected row counts: {rows}")
        # Round-robin, reversing order each round, to share background/thermal drift.
        for i in range(args.runs):
            for name, argv in (cases if i % 2 == 0 else reversed(cases)):
                start = time.perf_counter()
                run(argv)
                samples[name].append((time.perf_counter() - start) * 1000)
        result = {"platform": platform.platform(), "machine": platform.machine(),
                  "nodes": size["nodes"], "events": size["events"], "runs": args.runs,
                  "threshold_ms": args.max_ms, "commands": {}}
        failed = []
        for name, values in samples.items():
            ordered = sorted(values)
            mean = statistics.mean(values)
            result["commands"][name] = dict(rows=rows.get(name), mean_ms=mean,
                                           p50_ms=statistics.median(values), p95_ms=ordered[int(.95 * len(values)) - 1],
                                           stdev_ms=statistics.stdev(values))
            if name != "process_floor" and mean > args.max_ms:
                failed.append(name)
        result["failed"] = failed
        print(json.dumps(result, indent=2))
        return bool(failed)


if __name__ == "__main__":
    raise SystemExit(main())
