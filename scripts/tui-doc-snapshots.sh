#!/usr/bin/env bash
# Review snapshots of the TUI document editor (docs/design/tui-editor.md §12), in a scratch vault.
# Usage: scripts/tui-doc-snapshots.sh <out-dir>   (uses ./target/debug/thc; never a real vault)
set -euo pipefail
OUT=${1:?out dir}
THC=${THC:-$PWD/target/debug/thc}
ROOT=$(mktemp -d -t thc-doc-snap)
mkdir -p "$OUT" "$ROOT/home"
export HOME="$ROOT/home" XDG_CACHE_HOME="$ROOT/cache" THC_VAULT="$ROOT/vault" THC_CACHE_DIR="$ROOT/cache-thc"
export THC_NO_UPDATE_CHECK=1 THC_SETUP_LOGIN=skip THC_TEST=1 THC_TEST_ROOT="$ROOT"
THC="$THC" scripts/seed-sample.sh "$ROOT/vault" >/dev/null
# A long paragraph on today's journal (written as a paragraph, as the editor writes them).
pid=$("$THC" --json add "Lisbon is starting to feel real. The flat has a balcony that faces the river, and the light in the morning is extraordinary; I keep thinking about how the days will feel once the boxes are gone." | python3 -c 'import sys,json; print(json.load(sys.stdin)["nodes"][0]["id"])')
"$THC" set "$pid" style=para >/dev/null
MD='# Offsite notes\n\nWe meet in Lisbon for three days.\n\n## Agenda\n\n1. Where we are\n2. Three bets for Q1\n\n- Logistics\n  - [ ] Book the venue due:2026-10-09\n  - Dinner on the second night\n\n> Keep it small.\n\n---'
snap() { # name size keys (THC_TUI_RESUME is used once: copy it per render)
  local resume="${THC_TUI_RESUME:-}"
  for theme in ember-dark ember-light; do
    [ -n "$resume" ] && cp "$ROOT/resume.json" "$ROOT/resume-$theme.json" && export THC_TUI_RESUME="$ROOT/resume-$theme.json"
    THC_THEME=$theme THC_TUI_SNAPSHOT=$2 THC_TUI_SNAPSHOT_FORMAT=html THC_TUI_KEYS="$3" "$THC" tui > "$OUT/$1-$theme.html"
  done
  [ -n "$resume" ] && cp "$ROOT/resume.json" "$ROOT/resume-txt.json" && export THC_TUI_RESUME="$ROOT/resume-txt.json"
  THC_TUI_SNAPSHOT=$2 THC_TUI_KEYS="$3" "$THC" tui > "$OUT/$1.txt"
  unset THC_TUI_RESUME
}
snap doc-journal-write   120x36 "5"
snap doc-journal-80      80x24  "5"
snap doc-page-title      120x36 "4Q4 Planning<cr>"
snap doc-typing-tokens   120x36 "5Call Sam due:mon !high #lisbon"
snap doc-just-left       120x36 "5Call Sam due:mon !high #lisbon<cr>"
snap doc-kept-as-text    120x36 "5Call the bank due:fryday<cr>"
snap doc-selection       120x36 "5<up><s-up><s-up>"
snap doc-paste-md        120x36 "5<paste:$MD>"
snap doc-wrap-prose      120x36 "5<up>"
"$THC" page new "Offsite notes" >/dev/null
snap doc-line-forms      120x36 "4Offsite notes<cr><paste:$MD><up>"
snap doc-link-popup      120x36 "5See [[q4"
snap doc-link-guard      120x36 "5See [[Zzz"
snap doc-remote-current  120x36 "5<up><remote:$pid:Lisbon feels real now. The boxes are nearly gone.>"
snap doc-conflict        120x36 "5<up> Truly.<remote:$pid:Lisbon feels real now.><down>"
snap doc-compare         120x36 "5<up> Truly.<remote:$pid:Lisbon feels real now.><down><up><c-o>"
snap doc-focus           120x36 "5<m-z>"
THC_TUI_FOCUS_DIM=1 snap doc-focus-dim 120x36 "5<up>"
# The keys footer (§4.5) and composable Focus (§8).
snap doc-footer-write-120 120x36 "5"
snap doc-footer-write-80  80x24  "5"
# A save lands just after the frame: one more key shows its state.
THC_TUI_FAKE_SAVE_FAIL=1 snap doc-footer-not-saved 120x36 "5<up> more<down><right>"
THC_TUI_FOCUS=bare    snap focus-bare        120x36 "5<m-z>"
THC_TUI_FOCUS=writer  snap focus-writer      120x36 "5<m-z>"
THC_TUI_FOCUS=planner snap focus-planner-120 120x36 "5<m-z>"
THC_TUI_FOCUS=planner snap focus-planner-140 140x36 "5<m-z>"
THC_TUI_FOCUS=planner snap focus-planner-100 100x36 "5<m-z>"
THC_TUI_FOCUS=writer,+month snap focus-writer-month 120x36 "5<m-z>"
snap focus-overlay        120x36 "5<m-z><m-:>focus<cr>"
snap focus-overlay-custom 120x36 "5<m-z><m-:>focus<cr>cf"
snap help-in-document     120x36 "5<f1>"
# Writing v2 (writing.md): paragraphs and blank lines, a task cycle, a list ending, the near miss.
snap doc-v2-paragraphs   120x36 "5Slept badly.<cr>Rain all night.<cr><cr>Coffee first."
snap doc-v2-task-cycle   120x36 "5Call the bank<c-t>"
snap doc-v2-list-end     120x36 "5- milk<cr>- eggs<cr><cr>That's all."
snap doc-v2-near-miss    120x36 "5See [[Q4 Planing]]<cr><cr>x"
# The mouse (mouse.md §9), with the cursor drawn. On the seeded day the typed line is y=17 (0-based),
# its text from column 14 (the box at 10-12).
P="The quick brown fox jumps over the lazy dog and keeps running far past the old wooden fence, then a little further still."
THC_TUI_SNAPSHOT_CURSOR=1 snap mouse-click-caret     120x36 "5$P<click:30,18>"
THC_TUI_SNAPSHOT_CURSOR=1 snap mouse-drag-select     120x36 "5first para<cr><cr>second para<drag:20,17,20,19>"
THC_TUI_SNAPSHOT_CURSOR=1 snap mouse-dclick-word     120x36 "5hello world<dclick:22,17>"
THC_TUI_SNAPSHOT_CURSOR=1 snap mouse-task-toggle     120x36 "5[ ] call<click:11,17>"
LONG="- line 1<cr>- line 2<cr>- line 3<cr>- line 4<cr>- line 5<cr>- line 6<cr>- line 7<cr>- line 8<cr>- line 9<cr>- line 10<cr>- line 11<cr>- line 12<cr>- line 13<cr>- line 14<cr>- line 15<cr>- line 16<cr>- line 17<cr>- line 18<cr>- line 19<cr>- line 20<cr>- line 21<cr>- line 22<cr>- line 23<cr>- line 24<cr>- line 25<cr>- line 26<cr>- line 27<cr>- line 28<cr>- line 29<cr>- line 30<cr>- line 31<cr>- line 32<cr>- line 33<cr>- line 34<cr>- line 35<cr>- line 36<cr>- line 37<cr>- line 38<cr>- line 39<cr>- line 40<cr>"
THC_TUI_SNAPSHOT_CURSOR=1 snap mouse-wheel-caret-stays 120x36 "5${LONG}<wheel:up:15>"
# a 漢 at x=15-16 (after "a" at 14): the right half puts the caret after it
THC_TUI_SNAPSHOT_CURSOR=1 snap mouse-click-cjk       120x36 "5a漢字b<click:16,17>"
THC_TUI_SNAPSHOT_CURSOR=1 snap mouse-tclick-note     120x36 "5hello there world<tclick:20,17>"
THC_TUI_SNAPSHOT_CURSOR=1 snap mouse-sclick-extend   120x36 "5hello there world<click:14,17><sclick:25,17>"
snap mouse-link-cclick 120x36 "5see [[Q4 Planning]] now<cclick:22,17>"
# The chrome (mouse.md §5): a tab, a footer key, the palette closed by an outside click.
snap mouse-tab-switch    120x36 "<click:31,0>"
snap mouse-footer-key    120x36 "5<click:113,35>"
snap mouse-outside-closes 120x36 ":<click:2,30>"
# 256 colours (tui-handoff.md §1.4).
snap256() { # name size keys
  for theme in ember-dark-256 ember-light-256; do
    THC_THEME=$theme THC_TUI_SNAPSHOT=$2 THC_TUI_SNAPSHOT_FORMAT=html THC_TUI_KEYS="$3" "$THC" tui > "$OUT/$1-$theme.html"
  done
}
snap256 c256-today         120x36 "1"
snap256 c256-journal       120x36 "5"
snap256 c256-conflict      120x36 "5<up> Truly.<remote:$pid:Lisbon feels real now.><down>"
snap256 c256-help          120x36 "5<f1>"
THC_TUI_FOCUS=planner snap256 c256-focus-planner 120x36 "5<m-z>"
mkdir -p "$XDG_CACHE_HOME/thc"
printf '%s' '{"checked":"x","latest":"0.8.1","notes":"- the editor"}' > "$XDG_CACHE_HOME/thc/update.json"
snap bar-update-available 120x36 "1"
THC_TUI_FAKE_APPLYING=1 snap bar-updating 120x36 ""
printf '%s' '{"view":"Journal","updated_to":"0.8.1"}' > "$ROOT/resume.json"
THC_TUI_RESUME="$ROOT/resume.json" snap toast-updated 120x36 ""
rm -f "$XDG_CACHE_HOME/thc/update.json"
rm -rf "$ROOT"
echo "wrote $(ls "$OUT" | wc -l | tr -d ' ') files to $OUT"
