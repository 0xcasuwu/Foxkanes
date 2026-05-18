#!/bin/bash
# Build all Foxkanes alkane crates to wasm32-unknown-unknown and copy the
# binaries into src/tests/wasm/ where vendor.rs picks them up via
# include_bytes!. Mirrors fire-misha/scripts/build-wasms.sh.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$SCRIPT_DIR/.."
WASM_DIR="$PROJECT_DIR/target/wasm32-unknown-unknown/release"
TEST_WASM_DIR="$PROJECT_DIR/src/tests/wasm"

echo "Building Foxkanes WASM contracts..."
cd "$PROJECT_DIR"

for contract in foxkanes-animal foxkanes-commitment foxkanes-game foxkanes-zap; do
    echo "Building $contract..."
    cargo build --release --target wasm32-unknown-unknown -p "$contract"
done

mkdir -p "$TEST_WASM_DIR"
for contract in foxkanes-animal foxkanes-commitment foxkanes-game foxkanes-zap; do
    underscored="${contract//-/_}"
    cp "$WASM_DIR/$underscored.wasm" "$TEST_WASM_DIR/$underscored.wasm"
    echo "Copied $underscored.wasm to test dir"
done

echo "Done. Test wasms ready in $TEST_WASM_DIR/"
ls -la "$TEST_WASM_DIR"/*.wasm
