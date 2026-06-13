#!/usr/bin/env bash
# Rebuild the vendored BLAKE3 WASM module (static/js/vendors/hash-wasm/).
#
# The generated artifacts ARE committed — like the other vendored modules
# (pdf.js) — so regular frontend/backend builds never need the wasm
# toolchain. Re-run this script only when wasm/oxicloud-hash/ changes
# (e.g. bumping the blake3 crate, or adding FastCDC for the delta-sync
# client) and commit the regenerated files.
#
# Requirements (one-time):
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --locked
#
# wasm-bindgen-cli's version must match the crate's `wasm-bindgen`
# dependency; cargo prints a clear error when they drift.

set -euo pipefail
cd "$(dirname "$0")/.."

CRATE=wasm/oxicloud-hash
OUT=static/js/vendors/hash-wasm

if ! ( rustup target list --installed | grep -q wasm32-unknown-unknown )
then
    echo -e "install first wasm32-unknown-unknown via:\n\trustup target add wasm32-unknown-unknown"
    exit 1
fi

if ! which wasm-bindgen >/dev/null 2>/dev/null
then
    WASM_BINDGEN_VERSION=$(grep -A1 'name = "wasm-bindgen"' $CREATE/Cargo.lock | head -2 | grep version | sed 's/version = //')
    echo -e "wasm-bindgen not found, install it via:\n\tcargo install wasm-bindgen-cli --version $WASM_BINDGEN_VERSION"
    exit 1
fi

# SIMD128 is baseline in every evergreen browser (Chrome 91+, Firefox 89+,
# Safari 16.4+) and is worth ~3-4× in hashing throughput. Browsers without
# it fail instantiation; the frontend detects that and falls back to a
# plain byte upload.
RUSTFLAGS="-C target-feature=+simd128" \
    cargo build \
    --manifest-path "$CRATE/Cargo.toml" \
    --target wasm32-unknown-unknown \
    --release

wasm-bindgen \
    --target web \
    --no-typescript \
    --out-dir "$OUT" \
    "$CRATE/target/wasm32-unknown-unknown/release/oxicloud_hash_wasm.wasm"

echo "Vendored: $(ls -la "$OUT" | tail -n +2 | awk '{print $9, "("$5" bytes)"}' | xargs)"
