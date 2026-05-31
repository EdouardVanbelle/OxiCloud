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
# auth.users (start the server against the test DB once to seed).
test-integration:
    bash tests/common/spawn-db.sh
    RUSTFLAGS='--cfg integration_tests' cargo test --workspace --tests

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

# front: check all (linter, format, type, ...)
front-check: front-fmt front-lint front-type front-rules

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
