#!/bin/sh
set -e

# Fix ownership of mounted volumes.
# When Docker creates named volumes they are owned by root, but the
# application runs as the unprivileged "oxicloud" user (UID 1001).
# This script runs as root, fixes permissions, then drops privileges.

STORAGE_DIR="/app/storage"
STATIC_DIR="/app/static"

# Recursively chown DIR to the oxicloud user, but only when its top-level
# entry is not already owned by that user. The storage volume is a
# content-addressable blob store that can hold millions of objects; a blind
# "chown -R" on every boot would re-stat and rewrite the inode of every blob,
# turning startup into minutes of disk I/O. Checking the root entry is the
# cheap idempotent guard: the first boot fixes a freshly mounted (root-owned)
# volume, and every later boot is a no-op.
ensure_owned() {
    dir="$1"
    if [ -d "$dir" ] && [ "$(stat -c %u "$dir")" != "$OXI_UID" ]; then
        chown -R oxicloud:oxicloud "$dir"
    fi
}

# Only root can chown; when started unprivileged the volume permissions are
# assumed to be correct already.
if [ "$(id -u)" -eq 0 ]; then
    OXI_UID="$(id -u oxicloud)"
    ensure_owned "$STORAGE_DIR"
    ensure_owned "$STATIC_DIR"
fi

# Drop privileges and exec the main binary (or whatever was passed as CMD)
if [ "$(id -u)" -eq 0 ]; then
    exec su-exec oxicloud "$@"
else
    "$@"
fi
