#!/bin/sh
# Builds src/native/pill/model.rs (via wasm/) to public/pill.wasm.
set -e
cd "$(dirname "$0")/wasm"
CARGO="${CARGO:-$HOME/.cargo/bin/cargo}"
[ -x "$CARGO" ] || CARGO=cargo
CARGO_TARGET_DIR=target taskpolicy -c utility "$CARGO" build --release --target wasm32-unknown-unknown --offline -j 2
mkdir -p ../public
cp target/wasm32-unknown-unknown/release/pill_wasm.wasm ../public/pill.wasm
