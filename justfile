set dotenv-load

default:
    @just --list

build:
    cargo build

release:
    cargo build --release
    # check that app is clean
    node --check static-dist/js/app.*.js

run:
    cargo run

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

# audit security (condition: cargo install cargo-audit)
audit:
    cargo audit

openapi:
    cargo run --bin generate-openapi

db:
    docker compose up -d postgres

db-down:
    docker compose down

# start OxiCloud with static assets in no-cache mode
front-dev:
    PROFILE=dev cargo run

# front: check all (linter, format, type, icons, translations...)
front-check: front-fmt front-lint front-type front-rules front-check-icons front-check-i18n

front-fmt:
    biome format static/

front-lint:
    biome lint static/

# test types (JSDOC), using typescript
front-type:
    tsc -p jsconfig.json --noEmit

# check CSS rules
front-rules:
    stylelint static/css/

front-check-icons:
    tools/check-icons.py --check-only

front-check-i18n:
    tools/check-missing-translations.py --check-only


# end-to-end Playwright tests
front-test:
    cd tests/e2e && npm test

# update images snapshots
front-test-update-snapshot:
    cd tests/e2e && npm test -- --update-snapshots=all

# Hurl API functional tests (starts postgres + server, tears down after)
api-test:
    bash tests/api/run.sh
    bash tests/webdav/run.sh

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
