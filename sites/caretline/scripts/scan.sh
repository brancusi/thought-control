#!/usr/bin/env bash
# Fails if the built site contains anything private: home paths, the local user name, emails,
# private repo names (anything ending -internal) or claude.ai links. SCAN_EXTRA adds a regex. Run after every build, before every deploy.
set -euo pipefail
cd "$(dirname "$0")/.."
private="/Users/|/home/[a-z]|$(id -un)|[a-z]-internal\b|claude\.ai${SCAN_EXTRA:+|$SCAN_EXTRA}"
email='[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+\.[A-Za-z0-9.-]*(com|dev|io|app|net|org)'
# Pagefind's own bundle credits its translators by email; that is third-party and public.
if grep -rIEon "$private" dist/ || grep -rIEon "$email" dist/ --exclude-dir=pagefind; then
  echo "scan: private material found in dist/ (above). Fix it before deploying." >&2
  exit 1
fi
# The wasm binary too: paths can leak into panic messages.
if grep -c -aE "/Users/|$(id -un)" public/wasm/caretline.wasm >/dev/null; then
  echo "scan: a home path is inside public/wasm/caretline.wasm" >&2
  exit 1
fi
echo "scan: dist/ is clean"
