#!/usr/bin/env bash
# Builds the caretline engine (crates/caretline) to WebAssembly for the playground and
# copies it to public/wasm/. Needs the wasm32-unknown-unknown target:
#   rustup target add wasm32-unknown-unknown
set -euo pipefail
cd "$(dirname "$0")/../wasm"
repo="$(cd ../../.. && pwd)"
# Keep local paths out of the binary (panic locations): the repo becomes ., home becomes ~.
export RUSTFLAGS="--remap-path-prefix=$repo=. --remap-path-prefix=$HOME=~ ${RUSTFLAGS:-}"
cargo build --release --target wasm32-unknown-unknown
mkdir -p ../public/wasm
cp target/wasm32-unknown-unknown/release/caretline_wasm.wasm ../public/wasm/caretline.wasm
ls -l ../public/wasm/caretline.wasm
