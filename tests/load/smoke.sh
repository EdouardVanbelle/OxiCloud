#!/usr/bin/env bash
# Smoke (PR-tier) k6 load runner.
# Same shape as run.sh but: no seeder, only the smoke scenario, no regression
# diff. Goal is harness liveness - does the server still boot and does k6 still
# wire up - not perf gating.
#
# Usage (from repo root):
#   bash tests/load/smoke.sh
#
# Env overrides:
#   BUILD_TARGET=debug     # debug is fine; smoke doesn't measure timings
#
# Prerequisites: docker, cargo, k6 >= 0.46

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
COMMON="$REPO_ROOT/tests/common"
LOAD_DIR="$REPO_ROOT/tests/load"

# shellcheck source=test.env
source "$LOAD_DIR/test.env"

SERVER_PORT="${base_url##*:}"

log() { echo "[load-smoke] $*"; }
die() { echo "[load-smoke] ERROR: $*" >&2; exit 1; }

wait_for_http() {
  local url="$1" timeout="${2:-60}"
  local deadline=$(( $(date +%s) + timeout ))
  until curl -sf "$url" >/dev/null 2>&1; do
    [[ $(date +%s) -ge $deadline ]] && die "Timeout waiting for $url"
    sleep 1
  done
}

command -v k6 >/dev/null 2>&1 || die "k6 is required (https://k6.io/docs/get-started/installation/)"

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
# Server applies sqlx migrations on startup; don't double-apply via psql here.
# See run.sh for the gory details.

set -a
# shellcheck source=../common/server.env
source "$COMMON/server.env"
OXICLOUD_SERVER_PORT=$SERVER_PORT
OXICLOUD_STORAGE_PATH="$LOAD_DIR/storage"
set +a

rm -rf "$OXICLOUD_STORAGE_PATH"
mkdir -p "$OXICLOUD_STORAGE_PATH"

BUILD_TARGET="${BUILD_TARGET:-debug}"
OXICLOUD_BIN="$REPO_ROOT/target/$BUILD_TARGET/oxicloud"

if [[ -x "$OXICLOUD_BIN" ]]; then
  log "Starting pre-built OxiCloud server ($BUILD_TARGET) on port $SERVER_PORT..."
  "$OXICLOUD_BIN" &
else
  log "Building and starting OxiCloud server ($BUILD_TARGET) on port $SERVER_PORT..."
  cd "$REPO_ROOT"
  if [[ "$BUILD_TARGET" == "release" ]]; then
    cargo run --release &
  else
    cargo run &
  fi
fi
SERVER_PID=$!
wait_for_http "$base_url/ready" 120
log "Server ready."

# Bootstrap the admin account via /api/setup (one-shot — disabled once an
# admin exists, mirrors tests/api/setup.hurl). Without this the smoke scenario
# would have no one to log in as.
log "Creating admin via /api/setup..."
SETUP_BODY=$(printf '{"username":"%s","email":"%s","password":"%s"}' \
  "${username:-admin}" "${email:-admin@example.com}" "${password:-TestPassword1!}")
SETUP_STATUS=$(curl -sS -o /dev/null -w '%{http_code}' \
  -X POST "$base_url/api/setup" \
  -H 'Content-Type: application/json' \
  -d "$SETUP_BODY")
if [[ "$SETUP_STATUS" != "201" ]]; then
  die "/api/setup returned $SETUP_STATUS (expected 201)"
fi

export K6_BASE_URL="$base_url"
export K6_USERNAME="${username:-admin}"
export K6_PASSWORD="${password:-TestPassword1!}"

log "Running smoke scenario..."
k6 run \
  --summary-trend-stats="avg,min,med,max,p(90),p(95),p(99)" \
  --quiet \
  "$LOAD_DIR/scenarios/smoke.js"

log "Smoke OK."
