#!/usr/bin/env bash
# Shared helper: wipe a test-storage directory.
#
# Source this file (don't execute it):
#
#   source "$COMMON/wipe-storage.sh"
#   wipe_storage "$OXICLOUD_STORAGE_PATH"
#
# The path must end in `tests/<name>/storage` (lowercase alphanumeric
# `<name>`). Anything else is rejected — defends against a typo'd or
# unexpanded env var feeding the wrong path to `rm -rf`. Examples that
# pass:
#
#   /home/dev/oxicloud/tests/api/storage
#   /home/dev/oxicloud/tests/webdav/storage
#   /home/dev/oxicloud/tests/e2e/storage
#
# Examples that the sanity check rejects (each would refuse the wipe):
#
#   /              — no tests/<x>/storage suffix
#   $HOME          — same
#   tests/api      — missing /storage
#   /tests/storage — missing the <name> segment
#
# Postgres state is wiped by `spawn-db.sh` (`docker compose down -v`);
# this helper is the filesystem equivalent.

wipe_storage() {
  local path="$1"

  if [[ -z "$path" ]]; then
    echo "[wipe_storage] ERROR: missing path arg" >&2
    return 1
  fi

  # Sanity check: must end in tests/<name>/storage where <name> is
  # lowercase alphanumeric (hyphens allowed so multi-word runner names
  # like `webdav-drive-root` pass). Stops `rm -rf` from ever running
  # against an unexpected expansion of a caller's path.
  if [[ ! "$path" =~ /tests/[a-z0-9][a-z0-9-]*/storage$ ]]; then
    echo "[wipe_storage] ERROR: '$path' does not match .../tests/<name>/storage — refusing to wipe" >&2
    return 1
  fi

  echo "[wipe_storage] Wipe $path to ensure clean startup"
  rm -rf "$path"
  mkdir -p "$path"
}
