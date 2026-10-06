#!/usr/bin/env bash
# Fill a vault with sample entries. Usage: scripts/seed-sample.sh [--conflict] [vault-dir]
# Uses THC_VAULT if set, otherwise ./vault (created if missing).
# --conflict also runs scripts/fixture-conflict.sh (docs/design/daemon.md §4.1).
set -euo pipefail
THC=${THC:-thc}
CONFLICT=0
if [ "${1:-}" = "--conflict" ]; then CONFLICT=1; shift; fi
VAULT=${1:-${THC_VAULT:-vault}}
# init writes a .thc.toml where it runs: next to the vault, never in the caller's folder (a
# stray /tmp/.thc.toml once captured every command under /tmp).
mkdir -p "$VAULT"
VAULT=$(cd "$VAULT" && pwd)
[ -f "$VAULT/thc-vault.toml" ] || (cd "$(dirname "$VAULT")" && $THC init "$VAULT" >/dev/null)
export THC_VAULT="$VAULT"
id() { $THC --json "$@" | python3 -c 'import sys,json; print(json.load(sys.stdin)["nodes"][0]["id"])'; }
# Creates carry a key made from their arguments, so every seeded vault gets the same ids (pages,
# tags and days are keyed already) and the guide renders only change when the UI does.
K() { $THC "$@" --key "seed:$*"; }
idk() { id "$@" --key "seed:$*"; }

# Pages
health=$(id page new "Health" -t personal)
home=$(id page new "Home" -t personal)
q4=$(id page new "Q4 Planning" -t work)
reading=$(id page new "Reading List" -t personal)

# Page contents
K add --under "$health" "Dr. Patel, 555-0100, office on 4th St" >/dev/null
K todo --under "$health" "Schedule annual physical" --due "+10d" >/dev/null
K todo --under "$health" "Refill prescription" --repeat "every month on the 15th" -p med >/dev/null
K add --under "$home" "Wifi: guest network password is on the fridge" >/dev/null
K todo --under "$home" "Water plants" --repeat "every 3 days" >/dev/null
K todo --under "$home" "Replace furnace filter" --repeat "every! 3 months" >/dev/null
K todo --under "$home" "Fix leaky kitchen faucet" -p low >/dev/null
okr=$(idk todo --under "$q4" "Draft Q4 OKRs" --scheduled mon --due "+7d" -p high)
K todo --under "$okr" "Collect last quarter's metrics" --due "+3d" >/dev/null
K todo --under "$okr" "Review draft with the team" --due "+6d" >/dev/null
K add --under "$q4" "Theme for the quarter: fewer, deeper bets" >/dev/null
K add --under "$reading" "[[Atomic Habits]]: finished, notes on habit stacking" >/dev/null
K todo --under "$reading" "Read The Pragmatic Programmer, ch. 1-3" -t reading >/dev/null

# Journal: the last few days
K add --journal=-2d "Kickoff for Q4 planning, see [[Q4 Planning]]" >/dev/null
K add --journal=-2d "idea: a weekly review ritual every friday afternoon" >/dev/null
K add --journal=-1d "Called the bank about the wire, waiting on a fax #finance" >/dev/null
done_id=$(idk add --journal=-1d "[ ] Renew passport #admin")
$THC done "$done_id" >/dev/null
K add "Morning: slept well, 30 min run #health" >/dev/null
K add "Call dentist to reschedule due:fri #health !high" >/dev/null
K add "Dentist appointment at:\"tue 2pm\" [[Health]]" >/dev/null
K remind "Pay rent" --at "+3d 9am" --repeat "every month on the 1st" >/dev/null
K remind "Mom's birthday, call her" --at "+12d 10am" --no-task >/dev/null

# Inbox (as if dropped from a phone)
K add --inbox "Look into a standing desk" >/dev/null
K add --inbox "Gift idea: a good chef's knife #gifts" >/dev/null

# Something an agent did
THC_ACTOR=claude K todo "Summarize unread newsletters into [[Reading List]]" --due tomorrow -t reading >/dev/null

$THC export >/dev/null
echo "seeded $VAULT"
$THC doctor | tail -3
if [ "$CONFLICT" = 1 ]; then
  THC="$THC" "$(dirname "$0")/fixture-conflict.sh" "$VAULT"
fi
