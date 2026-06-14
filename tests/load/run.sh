#!/usr/bin/env bash
# Full k6 load-test runner.
# Starts postgres + OxiCloud server, seeds fixtures, runs k6 scenarios,
# compares results against baseline/load.json, tears everything down.
#
# Usage (from repo root):
#   bash tests/load/run.sh
#
# Env overrides:
#   BUILD_TARGET=release   # prefer release build for accurate timings
#   LOAD_DEPTH=8           # override seeder shape (otherwise read from test.env)
#   LOAD_FANOUT=3
#   LOAD_FILES_PER_LEAF=3
#   LOAD_EXTRA_USERS=20
#   LOAD_GROUP_DEPTH=3
#   LOAD_GROUP_FANOUT=5
#   K6_SUMMARY_OUT=path    # explicit output path (default: tests/load/results/<ts>.json)
#
# Prerequisites: docker, cargo, k6 >= 0.46, node >= 18

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
COMMON="$REPO_ROOT/tests/common"
LOAD_DIR="$REPO_ROOT/tests/load"

# shellcheck source=test.env
source "$LOAD_DIR/test.env"

SERVER_PORT="${base_url##*:}"

log() { echo "[load] $*"; }
die() { echo "[load] ERROR: $*" >&2; exit 1; }

wait_for_http() {
  local url="$1" timeout="${2:-120}"
  local deadline=$(( $(date +%s) + timeout ))
  until curl -sf "$url" >/dev/null 2>&1; do
    [[ $(date +%s) -ge $deadline ]] && die "Timeout waiting for $url"
    sleep 1
  done
}

command -v k6   >/dev/null 2>&1 || die "k6 is required (https://k6.io/docs/get-started/installation/)"
command -v node >/dev/null 2>&1 || die "node >= 18 is required for compare.mjs"

SERVER_PID=""
cleanup() {
  if [[ -n "$SERVER_PID" ]]; then
    log "Stopping OxiCloud server (pid $SERVER_PID)..."
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  bash "$COMMON/stop-db.sh" || true
}
trap cleanup EXIT

bash "$COMMON/spawn-db.sh"

# NOTE: do NOT run init-test-schema.sh here. The OxiCloud server applies
# sqlx migrations on startup; applying them via raw psql first leaves the
# server's _sqlx_migrations tracking table empty, which makes the second
# pass try to re-ALTER tables that already have the column (e.g. migration
# 20260507000000_session_family.sql), and the server panics on boot.

set -a
# shellcheck source=../common/server.env
source "$COMMON/server.env"
OXICLOUD_SERVER_PORT=$SERVER_PORT
OXICLOUD_STORAGE_PATH="$LOAD_DIR/storage"
set +a

rm -rf "$OXICLOUD_STORAGE_PATH"
mkdir -p "$OXICLOUD_STORAGE_PATH"

BUILD_TARGET="${BUILD_TARGET:-release}"
# Respect CARGO_TARGET_DIR for self-hosted runners that bind-mount target/
# outside the workspace (avoids actions/checkout EBUSY on the mount point).
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
OXICLOUD_BIN="$TARGET_DIR/$BUILD_TARGET/oxicloud"
SEED_BIN="$TARGET_DIR/$BUILD_TARGET/load-seed"

# Build both bins in one invocation. `load_seed_bin` is an empty marker
# feature that gates the load-seed bin without affecting oxicloud's dep
# graph, so cargo plans a single workspace build and oxicloud compiles
# exactly once. (See Cargo.toml comments on `load_seed_bin`.)
if [[ ! -x "$OXICLOUD_BIN" || ! -x "$SEED_BIN" ]]; then
  log "Building OxiCloud + load-seed ($BUILD_TARGET)..."
  if [[ "$BUILD_TARGET" == "release" ]]; then
    cargo build --release --features load_seed_bin --bin oxicloud --bin load-seed
  else
    cargo build --features load_seed_bin --bin oxicloud --bin load-seed
  fi
fi

# Start the server FIRST so its sqlx::migrate! populates _sqlx_migrations
# against the clean DB. The seeder then runs against the migrated schema
# while the server is still alive (it issues plain INSERTs, no DDL).
log "Starting OxiCloud server ($BUILD_TARGET) on port $SERVER_PORT..."
"$OXICLOUD_BIN" &
SERVER_PID=$!
wait_for_http "$base_url/ready" 120
log "Server ready."

log "Seeding fixtures..."
DEPTH="${LOAD_DEPTH:-${load_depth:-5}}"
FANOUT="${LOAD_FANOUT:-${load_fanout:-4}}"
FILES_PER_LEAF="${LOAD_FILES_PER_LEAF:-${load_files_per_leaf:-3}}"
EXTRA_USERS="${LOAD_EXTRA_USERS:-${load_extra_users:-20}}"
GROUP_DEPTH="${LOAD_GROUP_DEPTH:-${load_group_depth:-3}}"
GROUP_FANOUT="${LOAD_GROUP_FANOUT:-${load_group_fanout:-5}}"

mkdir -p "$LOAD_DIR/results"
MANIFEST_PATH="$LOAD_DIR/results/seed-manifest.json"

"$SEED_BIN" \
  --depth "$DEPTH" \
  --fanout "$FANOUT" \
  --files-per-leaf "$FILES_PER_LEAF" \
  --extra-users "$EXTRA_USERS" \
  --group-depth "$GROUP_DEPTH" \
  --group-fanout "$GROUP_FANOUT" \
  --password "${password:-TestPassword1!}" \
  --manifest "$MANIFEST_PATH"

TS="$(date +%s)"
SUMMARY_OUT="${K6_SUMMARY_OUT:-$LOAD_DIR/results/run-$TS.json}"

export K6_BASE_URL="$base_url"
export K6_USERNAME="${username:-admin}"
export K6_PASSWORD="${password:-TestPassword1!}"

# k6 only accepts one script per invocation, so each scenario runs separately
# and we merge the summaries afterwards. Per-scenario summaries also make it
# easier to attribute regressions when looking at raw artifacts in CI.
SCENARIOS=(folder_cascade share_cascade_rebac subject_group_nested)
PARTIAL_SUMMARIES=()
K6_FAILED=0

for name in "${SCENARIOS[@]}"; do
  partial="$LOAD_DIR/results/run-$TS-$name.json"
  PARTIAL_SUMMARIES+=("$partial")
  log "Running k6 scenario: $name"
  # --summary-trend-stats forces p(99) into the summary export; k6's default
  # only includes avg/min/med/max/p(90)/p(95), so without it baseline.p99 can
  # never be baked or diffed.
  k6 run \
    --summary-export="$partial" \
    --summary-trend-stats="avg,min,med,max,p(90),p(95),p(99)" \
    --quiet \
    "$LOAD_DIR/scenarios/$name.js" \
    || K6_FAILED=$?
done

log "Merging summaries -> $SUMMARY_OUT"
node "$LOAD_DIR/merge-summaries.mjs" "${PARTIAL_SUMMARIES[@]}" "$SUMMARY_OUT"

log "Comparing against baseline..."
COMPARE_RC=0
node "$LOAD_DIR/compare.mjs" "$SUMMARY_OUT" "$LOAD_DIR/baseline/load.json" || COMPARE_RC=$?

if [[ "$K6_FAILED" -ne 0 ]]; then
  log "k6 reported threshold failures (exit $K6_FAILED)."
  exit "$K6_FAILED"
fi
exit "$COMPARE_RC"
