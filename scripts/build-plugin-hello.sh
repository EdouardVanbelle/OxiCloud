#!/usr/bin/env bash
# Rebuild the committed plugin .wasm fixtures used by the plugin-runtime tests
# (src/infrastructure/services/plugins/runtime_test.rs).
#
# The generated artifacts ARE committed — like the vendored BLAKE3 module — so
# regular builds and `cargo test --features plugins` never need the wasm
# toolchain. Re-run this only when wasm/oxicloud-plugin-hello/ changes, and
# commit the regenerated files. CI rebuilds them and fails on any diff.
#
# Requirements (one-time):
#   - the wasm32-unknown-unknown target. In the devenv this is provided by
#     `languages.rust.targets` in devenv.nix; otherwise:
#         rustup target add wasm32-unknown-unknown

set -euo pipefail
cd "$(dirname "$0")/.."

CRATE=wasm/oxicloud-plugin-hello
OUT=tests/fixtures/plugins
# Pin the wasm build to the crate-local target dir. The devenv sets a global
# CARGO_TARGET_DIR (outside the repo) which would otherwise relocate the
# artifact away from the path below.
export CARGO_TARGET_DIR="$PWD/$CRATE/target"
ARTIFACT="$CRATE/target/wasm32-unknown-unknown/release/oxicloud_plugin_hello.wasm"

# Reproducibility: the CI plugins job runs `git diff --exit-code` against
# the committed fixtures, so any environment drift (absolute paths in
# debug info, incremental caches, host-specific codegen) breaks the check.
#   * `--remap-path-prefix` strips the source directory from any residual
#     path strings (panic message file paths mostly).
#   * `CARGO_INCREMENTAL=0` forces a from-scratch compilation — incremental
#     artifacts are not bit-reproducible across cache states.
# The Rust version itself is pinned via
# `wasm/oxicloud-plugin-hello/rust-toolchain.toml`; keep it in sync with
# the CI action tag in `.github/workflows/ci.yml` (`plugins` job).
export CARGO_INCREMENTAL=0
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$PWD/$CRATE=."

# Needs the wasm32-unknown-unknown target's std. In the devenv this comes from
# `languages.rust.targets` in devenv.nix; otherwise run
# `rustup target add wasm32-unknown-unknown`. cargo emits a clear "can't find
# crate for `std`" error below if it is missing.

mkdir -p "$OUT"

build() {
    local variant="$1"; shift
    echo "building $variant.wasm ${*:+(features: ${*#--features })}"
    cargo build \
        --manifest-path "$CRATE/Cargo.toml" \
        --target wasm32-unknown-unknown \
        --release "$@"
    cp "$ARTIFACT" "$OUT/$variant.wasm"
}

build hello
build panic      --features panic
build sleep      --features sleep
build net        --features net
build wrong_abi  --features wrong_abi
build omit_login --features omit_login

echo "Built fixtures:"
ls -la "$OUT"/*.wasm | awk '{print "  " $9 " (" $5 " bytes)"}'
