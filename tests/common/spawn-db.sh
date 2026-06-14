#!/usr/bin/env bash
set -euo pipefail

COMPOSE_FILE="$(dirname "$0")/docker-compose.test.yml"

wait_for_port() {
  local host="$1" port="$2" timeout="${3:-30}"
  local deadline=$(( $(date +%s) + timeout ))
  until nc -z "$host" "$port" 2>/dev/null; do
    [[ $(date +%s) -ge $deadline ]] && echo "Timeout waiting for $host:$port" >&2 && exit 1
    sleep 0.5
  done
}

# Postgres opens its TCP socket during `initdb` (well before the server can
# actually serve queries) and slams it back shut until startup completes —
# that's the classic "server closed the connection unexpectedly" race when
# a follow-up `psql` runs too quickly. The compose file has a `pg_isready`
# healthcheck; we mirror it here so this script is the single source of
# "the DB is ready" for callers (tests/api/run.sh, just test-integration).
#
# Tries the in-container pg_isready first (always available, no host deps),
# then falls back to a SELECT-1 probe via psql.
wait_for_postgres_ready() {
  local timeout="${1:-60}"
  local deadline=$(( $(date +%s) + timeout ))
  until docker compose -f "$COMPOSE_FILE" exec -T postgres-test \
          pg_isready -U oxicloud_test -d oxicloud_test -h 127.0.0.1 >/dev/null 2>&1; do
    [[ $(date +%s) -ge $deadline ]] && echo "Timeout waiting for postgres readiness" >&2 && exit 1
    sleep 0.5
  done
  # Belt-and-braces: pg_isready returns 0 as soon as the server accepts
  # connections, but a query may still race the very first request. One
  # successful SELECT confirms the round-trip works end-to-end.
  #
  # Retry the probe a handful of times — under CPU pressure (e.g. a parallel
  # cargo build hammering a self-hosted runner) the role/db init can complete
  # a beat after pg_isready returns success. A single-shot probe in that
  # window produces spurious "Postgres reported ready but a sample query
  # failed" failures. Show the last error if every retry fails so operators
  # see the actual psql diagnostic.
  local last_err
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    if last_err=$(PGPASSWORD=oxicloud_test psql -h 127.0.0.1 -p 5433 \
                    -U oxicloud_test -d oxicloud_test \
                    -v ON_ERROR_STOP=1 -c 'SELECT 1' 2>&1 >/dev/null); then
      return 0
    fi
    sleep 0.5
  done
  echo "Postgres reported ready but a sample query failed after 10 retries: $last_err" >&2
  exit 1
}

echo "[setup] Starting test postgres..."
docker compose -f "$COMPOSE_FILE" down -v 2>/dev/null || true
docker compose -f "$COMPOSE_FILE" up -d
echo "[setup] Waiting for postgres on port 5433..."
wait_for_port 127.0.0.1 5433
echo "[setup] Waiting for postgres to accept queries..."
wait_for_postgres_ready
echo "[setup] Postgres is ready."
