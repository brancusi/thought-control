#!/usr/bin/env bash
# Fails if the built site contains anything private: home paths, the local user name, emails,
# private repo names (anything ending -internal), claude.ai links or scratch paths from the
# render scripts. SCAN_EXTRA adds a regex. Run after every build, before every deploy.
set -euo pipefail
cd "$(dirname "$0")/.."
[ -d dist ] || { echo "scan: no dist/ (npm run build first)" >&2; exit 1; }
private="/Users/|/home/[a-z]|/var/folders/|/private/tmp|$(id -un)|[a-z]-internal\b|claude\.ai${SCAN_EXTRA:+|$SCAN_EXTRA}"
email='[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+\.[A-Za-z0-9.-]*(com|dev|io|app|net|org)'
fail=0
grep -rIEon "$private" dist/ && fail=1
# Public, intentional addresses can be listed here (none today).
grep -rIEon "$email" dist/ && fail=1
if [ $fail = 1 ]; then
  echo "scan: private material found in dist/ (above). Fix it before deploying." >&2
  exit 1
fi
echo "scan: dist/ is clean ($(find dist -type f | wc -l | tr -d ' ') files)"
