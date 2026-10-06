#!/usr/bin/env bash
# Conflict fixture (docs/design/daemon.md §4.1). Produces, in a seeded vault:
#   1. a text conflict on "Dentist appointment [[Health]]" (studio-mini/claude vs this device/human)
#   2. a move conflict: studio-mini moves "Collect last quarter's metrics" under "Review draft
#      with the team"; this device then moves the reverse, which replay rejects as a cycle
#   3. an agent-set reminder: "Stretch" at +2m by claude
# Usage: scripts/fixture-conflict.sh [vault-dir]   (never run against a vault you care about)
set -euo pipefail
THC=${THC:-thc}
VAULT=${1:-${THC_VAULT:-vault}}
export THC_VAULT="$VAULT"
find_id() { $THC --json q "text:\"$1\"" --limit 1 | python3 -c 'import sys,json; d=json.load(sys.stdin)["items"]; print(d[0]["id"] if d else "")'; }
DENTIST=$(find_id "Dentist appointment")
METRICS=$(find_id "Collect last quarter")
REVIEW=$(find_id "Review draft with the team")
for v in DENTIST METRICS REVIEW; do [ -n "${!v}" ] || { echo "fixture: seed the vault first (scripts/seed-sample.sh)"; exit 1; }; done

TMP=$(mktemp -d)
cp -R "$VAULT" "$TMP/vault"          # studio-mini starts from the same state
# studio-mini (claude) edits first, so its HLCs are lower.
studio() { THC_VAULT="$TMP/vault" THC_CACHE_DIR="$TMP/cache" THC_DEVICE="studio-mini" THC_ACTOR=claude "$THC" "$@" >/dev/null; }
studio text "$DENTIST" "Dentist appointment [[Health]], moved to 15:00"
studio mv "$METRICS" --under "$REVIEW"
sleep 0.05
# This device (human) edits the same base afterwards.
$THC text "$DENTIST" "Dentist appointment [[Health]], bring the insurance card" >/dev/null
$THC mv "$REVIEW" --under "$METRICS" >/dev/null
# "Sync": studio-mini's log arrives late.
mkdir -p "$VAULT/log/studio-mini"
cp "$TMP"/vault/log/studio-mini/*.jsonl "$VAULT/log/studio-mini/"
rm -rf "$TMP"
THC_ACTOR=claude $THC remind "Stretch" --at "+2min" >/dev/null
$THC conflict ls
