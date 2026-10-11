#!/usr/bin/env bash
# Real-terminal (and headless) end-to-end runs of thc's editing, navigation and workflow goldens.
#
#   scripts/e2e/run.sh [target] [scenario…]       real WezTerm windows, one per target
#   scripts/e2e/run.sh --headless [target] […]    no terminal: fictty's Headless, `thc screen --render`
#   scripts/e2e/run.sh --keys [key…]              what WezTerm sends for each key, to thc and to others
#   scripts/e2e/run.sh --keep thc-tui E33          leave the window open on the first failure
#
# Targets: thc-tui, fictty-notes, thc-screen (default: all three). Scenarios are
# scripts/e2e/scenarios/*.json, by id (E33) or file stem; thc's headless half is
# `cargo test -p thc-tui --lib flows::e2e`. Builds land in target/e2e. The details, and what
# stays isolated, are in scripts/e2e/e2e.py.
#
# Environment: THC_BIN (default target/debug/thc), FICTTY_DIR (the fictty checkout, default
# ~/Documents/projects/fictty), FICTTY_REV (default its local main), THC_SCREEN_BIN or
# THC_SCREEN_REV / THC_SCREEN_FICTTY_REV (thc screen, default f8af743 (PR #95) on fictty 197ea78),
# WEZTERM (default /Applications/WezTerm.app/Contents/MacOS/wezterm), E2E_SETTLE (s between
# inputs, 0.12), E2E_GRACE (ms a check may take to come true, 600), E2E_FICTTY_KEYS (1: also install
# `fictty setup wezterm`'s module after thc's, the default for fictty-notes and thc-screen; 0: thc's only).
set -euo pipefail
cd "$(dirname "$0")/../.."
if [[ ! -x ${THC_BIN:-target/debug/thc} ]]; then
  cargo build -p thc
fi
exec python3 scripts/e2e/e2e.py "$@"
