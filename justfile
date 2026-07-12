set dotenv-load

default:
    @just --list

build:
    cargo build

release:
    # Build the SvelteKit SPA into static-dist/ (build.rs no longer bundles),
    # then compile the release binary which serves it.
    cd frontend && npm ci && npm run build
    cargo build --release

run:
    cargo run

# ── Docker image builds ──────────────────────────────────────────────────────
# Build the runtime image locally with BuildKit cargo cache mounts so repeat
# builds recompile only the crates that changed (true incremental). Routes the
# runtime stage to the `builder-cache` stage. A plain `docker build .` defaults
# to the slower `builder` stage that CI relies on for GitHub Actions layer
# caching — locally that recompiles the whole app crate on every source change,
# so prefer this recipe for iterating on the image. The cargo registry + target
# cache mounts persist in the local BuildKit cache across runs.
docker-build tag="oxicloud:dev":
    DOCKER_BUILDKIT=1 docker build \
        --build-arg BUILDER=builder-cache \
        --build-arg BIN_DIR=/app/bin \
        --tag {{tag}} \
        .

# Same incremental image but keeps the `data-testid` hooks (VITE_E2E=1) so the
# resulting image can back the Playwright e2e flow. Mirrors the build args the
# Testcontainers fixture passes (tests/e2e/fixtures/oxicloud-stack.ts).
docker-build-e2e tag="oxicloud-e2e:latest":
    DOCKER_BUILDKIT=1 docker build \
        --build-arg BUILDER=builder-cache \
        --build-arg BIN_DIR=/app/bin \
        --build-arg VITE_E2E=1 \
        --tag {{tag}} \
        .

run-debug:
    RUST_LOG=debug cargo run

test:
    cargo test --workspace

test-mocks:
    cargo test --features test_utils

# DB-dependent integration tests gated on `--cfg integration_tests`.
# Spins up the test postgres on port 5433 first. Requires one row in
# auth.users on the test DB (start the server against it once to seed).
#
# IMPORTANT: DATABASE_URL is pinned to the test container on port 5433
# so a stray DATABASE_URL in `.env` (which `set dotenv-load` at the top
# of this file would otherwise leak in) cannot point the tests at the
# real dev DB. The test pool helpers also refuse non-`oxicloud_test`
# URLs as defence in depth.
test-integration:
    bash tests/common/spawn-db.sh
    PGHOST=localhost PGPORT=5433 PGUSER=oxicloud_test PGPASSWORD=oxicloud_test \
      PGDATABASE=oxicloud_test \
      bash tests/common/init-test-schema.sh
    DATABASE_URL='postgres://oxicloud_test:oxicloud_test@localhost:5433/oxicloud_test' \
      RUSTFLAGS='--cfg integration_tests' \
      cargo test --workspace --tests
    bash tests/common/stop-db.sh

test-one name:
    cargo test {{name}}

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all --check

lint:
    cargo clippy --all-features --all-targets -- -D warnings

check:
    cargo fmt --all
    cargo clippy --all-features --all-targets -- -D warnings


wasm-check:
    cd wasm/oxicloud-hash; cargo fmt --all
    cd wasm/oxicloud-hash; cargo clippy --all-features --release -- -D warnings

wasm-test:
    cd wasm/oxicloud-hash; cargo test --release

# Run the host plugin-runtime tests (compiles the Extism/wasmtime runtime).
test-plugins:
    cargo test --features plugins

# Rebuild the committed plugin .wasm fixtures from wasm/oxicloud-plugin-hello/.
# Requires the wasm32 target (devenv provides it; else `rustup target add
# wasm32-unknown-unknown`). Commit the regenerated files; CI fails on drift.
plugin-build:
    bash scripts/build-plugin-hello.sh

# Build the example plugin and bundle plugin.toml + .wasm into an installable
# .zip at dist/oxicloud-plugin-hello.zip (upload via the admin Plugins tab).
plugin-example-zip:
    bash scripts/build-plugin-zip.sh

# fmt + clippy the example plugin crate (standalone workspace, wasm32 target).
plugin-check:
    cd wasm/oxicloud-plugin-hello; cargo fmt --all
    cd wasm/oxicloud-plugin-hello; cargo clippy --target wasm32-unknown-unknown --release -- -D warnings

# audit security (condition: cargo install cargo-audit)
audit:
    cargo audit

openapi:
    cargo run --bin generate-openapi

db:
    docker compose up -d postgres

db-down:
    docker compose down

# Frontend design-system / a11y guardrails (pure Node, no deps). The entry
# point the UI/UX workflow uses; delegates to `front-design`.
frontend-check: front-design

# end-to-end Playwright tests
front-test:
    cd tests/e2e && npm test

# update images snapshots
front-test-update-snapshot:
    cd tests/e2e && npm test -- --update-snapshots=all

# Records against a throwaway container stack (its own Postgres + the OxiCloud
# SPA). Each starting point is a file in tests/e2e/scenarios/codegen/ that sets
# up state then calls page.pause(); drop a new *.spec.ts there to add one — this
# menu discovers them automatically.

# Interactive Playwright codegen — pick a starting point, then record
front-codegen:
    bash tests/e2e/scripts/codegen.sh

# Frontend design-system guardrails — pure Node, no extra deps, run against the
# SvelteKit frontend (frontend/). Locale completeness, dead-token report, and
# brand-mark drift. For the full svelte-check/eslint/stylelint/prettier gate use
# `just fe-check` (needs the frontend devDependencies installed).
front-design:
    node scripts/check-locales.mjs
    node scripts/check-dead-tokens.mjs
    node scripts/check-brand-drift.mjs


# Hurl-driven functional tests (starts postgres + server, tears down after).
#
# Four runners — each isolated, brings up its own sidecars + server config:
#   * tests/api/run.sh              — REST API surface, default server.env
#   * tests/webdav/run.sh           — native WebDAV + NextCloud DAV, default server.env
#   * tests/webdav-drive-root/run.sh — WebDAV `OXICLOUD_WEBDAV_DRIVE_PATH=""`
#                                     variant (drive listing served at
#                                     `/webdav/` instead of `/webdav/@drive/`).
#                                     Server launched with
#                                     --config server-webdav-drive-root.env
#                                     so the default runners stay on the
#                                     `"@drive"` config.
#   * tests/oidc/run.sh             — OIDC SSO end-to-end against a fake IdP
#                                     (tests/oidc/fake_idp, a Node
#                                     panva/oidc-provider wrapper); server
#                                     launched with
#                                     --config server-with-oidc.env so the
#                                     api and webdav suites stay on the
#                                     OIDC-off config.
#
# Same chain runs in CI under the `api-test` job in
# .github/workflows/ci.yml; keep the order in sync so a local pass means
# CI passes.
api-test:
    #!/usr/bin/env bash
    set -x
    set -euo pipefail
    ./tests/api/run.sh
    ./tests/webdav/run.sh
    ./tests/webdav-drive-root/run.sh
    ./tests/oidc/run.sh
    if which litmus >/dev/null 2>/dev/null
    then
        ./tests/webdav/run-litmus.sh
    else
        echo "XXX litmus webdav not found, ignore test"
    fi

# ---------------------------------------------------------------------------
# SvelteKit frontend (frontend/) — the only frontend. These `fe-*` recipes
# drive its dev server, build, lint and tests.
# ---------------------------------------------------------------------------

# install frontend dependencies
fe-install:
    cd frontend && npm ci

# Vite dev server only (HMR) — backend must already be running on :8086
fe-dev:
    cd frontend && npm run dev

# build the SPA (Phase 0: -> frontend/build; Phase 5: -> static-dist)
fe-build:
    cd frontend && npm run build

# build the SPA for e2e — keeps the `data-testid` tile hooks the release build
# strips. Use before running the legacy webServer e2e flow against this binary.
fe-build-e2e:
    cd frontend && VITE_E2E=1 npm run build

# svelte-check + eslint + stylelint + prettier
fe-check:
    cd frontend && npm run check

# Vitest unit/component tests
fe-test:
    cd frontend && npm run test:unit

# Run backend (API) and the Vite dev server together; one Ctrl-C stops both.
dev:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo run &
    backend=$!
    trap 'kill $backend 2>/dev/null' EXIT INT TERM
    cd frontend && npm run dev

# k6 load suite — full scenarios + regression diff vs baseline/load.json.
# Used by the nightly workflow and on demand. Release build for fair timings.
load:
    bash tests/load/run.sh

# k6 smoke — single happy-path iteration. PR-tier liveness check, no gate.
load-smoke:
    bash tests/load/smoke.sh

# Re-run the full suite, then bake the latest summary into baseline/load.json.
# The leading `-` lets `run.sh`'s regression-exit not abort the bake step;
# review the diff and commit deliberately:
#   chore(load): accept new baseline for <reason>
load-baseline:
    -bash tests/load/run.sh
    node tests/load/bake-baseline.mjs

# Standalone seeder (poking around in psql). Needs the test DB up:
#   just db
load-seed:
    cargo run --bin load-seed -- --depth 5 --fanout 4 --files-per-leaf 3
