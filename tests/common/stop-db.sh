#!/usr/bin/env bash
set -euo pipefail

COMPOSE_FILE="$(dirname "$0")/docker-compose.test.yml"

echo "[teardown] Stopping test postgres..."
docker compose -f "$COMPOSE_FILE" down -v
