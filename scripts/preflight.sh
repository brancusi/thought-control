#!/usr/bin/env bash
# Preflight: fails if the files a change adds or modifies would leak or bloat this public repo.
# A build folder (target-clean/, 434 MB of artifacts with home paths) reached public main once
# (#83) and took a history rewrite to remove. This is the check that would have stopped it.
#
#   scripts/preflight.sh                 the branch: what HEAD adds over origin/main (before a push or merge)
#   scripts/preflight.sh --staged        the index (before a commit)
#   scripts/preflight.sh --range A..B    a commit range (CI, the pre-push hook)
#
# Checks each added or modified file:
#   1. path: no build output, dependency folders, vaults, editor or OS junk;
#   2. size: no file over PREFLIGHT_MAX_KB (default 512);
#   3. binary: no binary file outside the allowed folders (fixtures, goldens, web/public, docs);
#   4. content: no home folders, real temp or agent scratch paths, the local user name, private repo
#      URLs or claude.ai links (generic paths like /tmp or /private/tmp alone are fine);
#   5. content: nothing that looks like a credential.
# A path in .preflight-allow (one regex per line) skips checks 2–4 for it. Exit 1 lists every hit.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

mode=branch range=""
case "${1:-}" in
  --staged) mode=staged ;;
  --range) mode=range; range="${2:?--range needs A..B}" ;;
  "") ;;
  *) echo "usage: scripts/preflight.sh [--staged | --range A..B]" >&2; exit 2 ;;
esac

if [ "$mode" = branch ]; then
  base=$(git merge-base HEAD origin/main 2>/dev/null) || { echo "preflight: no origin/main (git fetch first)" >&2; exit 2; }
  range="$base..HEAD"
fi

# Added, copied, modified or renamed files (deletions can't leak).
if [ "$mode" = staged ]; then
  files=$(git diff --cached --name-only --diff-filter=ACMR)
  show() { git show ":$1"; }
else
  files=$(git diff --name-only --diff-filter=ACMR "$range")
  tip=${range##*..}
  show() { git show "$tip:$1"; }
fi
[ -n "$files" ] || { echo "preflight: nothing added"; exit 0; }

max_kb=${PREFLIGHT_MAX_KB:-512}
bad_path='(^|/)(target[^/]*|node_modules|dist|out|\.cache|\.wrangler|vault|\.thc-cache)/|(^|/)\.thc\.toml$|(^|/)\.DS_Store$|\.(rlib|rmeta|o|a|dylib|so|dSYM|swp|log)$|(^|/)(credentials|\.env)(\..*)?$'
binary_ok='^(crates/[^/]+/tests/(fixtures|golden)/|web/public/|docs/|web/src/assets/)'
user=$(id -un)
private="/Users/[A-Za-z][A-Za-z0-9._-]+/|/home/[a-z][a-z0-9_-]+/|/var/folders/[a-z0-9_]{2}/|/(private/)?tmp/claude-|github\.com/[A-Za-z0-9_-]+/[A-Za-z0-9_.-]+-internal|claude\.ai/|\\b${user}\\b${PREFLIGHT_EXTRA:+|$PREFLIGHT_EXTRA}"
secret='-----BEGIN [A-Z ]*PRIVATE KEY|ghp_[A-Za-z0-9]{30}|github_pat_[A-Za-z0-9_]{30}|gho_[A-Za-z0-9]{30}|sk-ant-[A-Za-z0-9_-]{20}|sk-[A-Za-z0-9]{40}|AKIA[0-9A-Z]{16}|xox[baprs]-[A-Za-z0-9-]{10}|cio[A-Za-z0-9]{30}'
allow=""
[ -f .preflight-allow ] && allow=$(grep -vE '^\s*(#|$)' .preflight-allow | paste -sd'|' -)

fail=0
hit() { echo "  $1: $2" >&2; fail=1; }
while IFS= read -r f; do
  [ -n "$f" ] || continue
  # This script names the patterns it hunts for.
  [ "$f" = scripts/preflight.sh ] && continue
  echo "$f" | grep -qE "$bad_path" && { hit "$f" "build output or local junk (never commit it; .gitignore it)"; continue; }
  [ -n "$allow" ] && echo "$f" | grep -qE "$allow" && continue
  size=$(show "$f" | wc -c | tr -d ' ')
  [ "$size" -gt $((max_kb * 1024)) ] && hit "$f" "$((size / 1024)) KB, over ${max_kb} KB"
  if show "$f" | head -c 8000 | perl -0777 -ne 'exit(/\x00/ ? 0 : 1)'; then
    echo "$f" | grep -qE "$binary_ok" || hit "$f" "binary file outside fixtures, goldens or web/public"
    continue
  fi
  m=$(show "$f" | grep -nEo "$private" | head -3 | tr '\n' ' ') || true
  [ -n "$m" ] && hit "$f" "private path or name: $m"
  m=$(show "$f" | grep -nEo -- "$secret" | head -1 | cut -c1-24) || true
  [ -n "$m" ] && hit "$f" "looks like a credential: ${m}…"
done <<< "$files"

if [ $fail = 1 ]; then
  echo "preflight: FAILED ($mode $range). Unstage or fix the files above; nothing here may reach the public repo." >&2
  exit 1
fi
echo "preflight: ok ($(echo "$files" | grep -c .) files, $mode${range:+ $range})"
