#!/usr/bin/env bash
# Points git at .githooks/ for this clone and all its worktrees, so every push runs
# scripts/preflight.sh. Run once per clone.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
git config core.hooksPath .githooks
echo "hooks: core.hooksPath=.githooks (pre-push runs scripts/preflight.sh)"
