#!/bin/sh
# Run every stress demo on the running screen, printing its counters. From the repo root:
#   cargo run --release -p thc-scene -- run          # one terminal
#   crates/scene/examples/stress/play.sh             # another
set -e
B=${THC_SCENE:-target/release/thc-scene}
D=crates/scene/examples/stress
stat() { printf '  %-24s' "$1"; $B get --stats | python3 -c 'import json,sys; s=json.load(sys.stdin); print("fps %(fps)6.1f   draw %(draw_ms)5.2fms   max %(max_ms)5.1fms   msgs/s %(msgs)6.0f" % s)'; }
for d in ticker plasma biglist; do
  $B push $D/$d.json >/dev/null; sleep "${HOLD:-8}"; stat "$d"
done
python3 $D/storm.py patch 1000 "${HOLD:-8}" >/dev/null & sleep $(( ${HOLD:-8} - 1 )); stat "patch storm 1000/s"; wait
python3 $D/storm.py push 60 "${HOLD:-8}" >/dev/null & sleep $(( ${HOLD:-8} - 1 )); stat "push storm 60/s"; wait
python3 $D/storm.py push 100000 "${HOLD:-8}" >/dev/null & sleep $(( ${HOLD:-8} - 1 )); stat "push storm flat out"; wait
