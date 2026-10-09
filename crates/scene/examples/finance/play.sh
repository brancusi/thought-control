#!/bin/sh
# Show the dashboard on the running screen and run the analyst's tour. From the repo root:
#   cargo run --release -p thc-scene -- run        # one terminal
#   crates/scene/examples/finance/play.sh          # another
set -e
B=${THC_SCENE:-target/release/thc-scene}
$B push crates/scene/examples/finance/dashboard.json >/dev/null
sleep 2
python3 crates/scene/examples/finance/tour.py "${STEP:-6}"
