#!/bin/bash
# Diff thc's own TUI against the scene UI, cell by cell, after each key sequence. Run from the
# repo root with a sample vault and a pinned clock:
#   scripts/seed-sample.sh /tmp/v; THC_VAULT=/tmp/v THC_NOW=2026-10-09T09:30 THC_DEVICE=demo \
#   THC_FIXTURE_IDS=1 crates/scene/examples/thc/compare.sh
S=${1:-$(mktemp -d)}; UI=${2:-crates/scene/examples/thc/today.json}; total=0; same=0
for keys in "" j jj jjj jjjj jjjjj jjjjjj jjjjjjj jjjjjjjj jjjjjjjjj jjjjjjjjjj jjjjjjjjjjjk jjjjjjjjjjjkkkkk; do
  THC_THEME=ember-dark THC_TUI_SNAPSHOT=150x40 THC_TUI_SNAPSHOT_FORMAT=ansi THC_TUI_KEYS="$keys" thc tui > $S/t.ansi 2>&1
  args=(); [ -n "$keys" ] && args=(--keys "$(echo $keys | sed 's/./&,/g; s/,$//')")
  ${THC_SCENE:-target/release/thc-scene} render "$UI" --size 150x40 --ansi "${args[@]}" > $S/s.ansi
  printf '%-18s ' "${keys:-(start)}"; python3 crates/scene/examples/thc/diff.py crates/scene/examples/thc $S/t.ansi $S/s.ansi 150 40 | head -${3:-1}
done
