#!/usr/bin/env bash
# Local-only build against an immutable engine export; never fetches or edits its source.
# Usage: scripts/with-caretline.sh ENGINE_REPO SCRATCH_EXPORT [cargo arguments...]
set -euo pipefail
if [ "$#" -lt 2 ]; then
  echo "usage: $0 ENGINE_REPO SCRATCH_EXPORT [cargo arguments...]" >&2
  exit 2
fi
root=$(cd "$(dirname "$0")/.." && pwd -P)
source_repo=$1
export_dir=$2
shift 2
revision=$(tr -d '\n' < "$root/caretline.rev")
[[ "$revision" =~ ^[0-9a-f]{40}$ ]] || { echo "invalid caretline.rev" >&2; exit 1; }
resolved=$(git -C "$source_repo" rev-parse "$revision^{commit}")
[ "$resolved" = "$revision" ] || exit 1
mkdir -p "$(dirname "$export_dir")"
parent=$(cd "$(dirname "$export_dir")" && pwd -P)
export_dir="$parent/$(basename "$export_dir")"
case "$export_dir/" in "$root/"*) echo "engine export must be outside the checkout" >&2; exit 1;; esac
checksums() {
  (cd "$export_dir"; find . -type f ! -name .caretline-revision ! -name .caretline-checksums | LC_ALL=C sort | while IFS= read -r file; do shasum -a 256 "$file"; done)
}
if [ -e "$export_dir" ]; then
  [ -f "$export_dir/.caretline-revision" ] && [ "$(tr -d '\n' < "$export_dir/.caretline-revision")" = "$revision" ] || { echo "refusing existing unverified export" >&2; exit 1; }
  [ -z "$(find "$export_dir" -type l -print -quit)" ] || { echo "refusing symlinks in export" >&2; exit 1; }
  checksums | cmp - "$export_dir/.caretline-checksums" || { echo "engine export changed; use a new scratch directory" >&2; exit 1; }
else
  mkdir "$export_dir"
  git -C "$source_repo" archive "$revision" | tar -x -C "$export_dir"
  [ -z "$(find "$export_dir" -type l -print -quit)" ] || { echo "refusing symlinks in export" >&2; exit 1; }
  printf '%s\n' "$revision" > "$export_dir/.caretline-revision"
  checksums > "$export_dir/.caretline-checksums"
fi
printf 'caretline: %s (verified immutable export)\n' "$revision" >&2
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-"$parent/thc-target"}
cd "$root"
exec cargo --config "patch.crates-io.caretline.path=\"$export_dir/crates/caretline\"" "$@"
