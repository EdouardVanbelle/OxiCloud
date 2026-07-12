#!/usr/bin/env bash
# Apply every migration in lexical order to a test database, then seed
# the minimum `auth.users` row that integration tests need.
#
# Connection parameters come from the libpq env vars (PGHOST, PGPORT,
# PGUSER, PGPASSWORD, PGDATABASE) so the same script works against:
#
#   - the local docker-compose-test postgres on port 5433
#     (PGHOST=localhost PGPORT=5433 PGUSER=oxicloud_test
#      PGPASSWORD=oxicloud_test PGDATABASE=oxicloud_test)
#
#   - the CI postgres service on port 5432
#     (PGHOST=localhost PGPORT=5432 PGUSER=postgres
#      PGPASSWORD=postgres PGDATABASE=oxicloud_test)
#
# The seed user is purely a placeholder so `first_admin()` in the Rust
# integration tests has a UUID to attach `added_by` to. The password
# hash is not a real argon2 hash — these tests never log in as this
# user, only reference its id.

set -euo pipefail

: "${PGHOST:?PGHOST must be set}"
: "${PGPORT:?PGPORT must be set}"
: "${PGUSER:?PGUSER must be set}"
: "${PGPASSWORD:?PGPASSWORD must be set}"
: "${PGDATABASE:?PGDATABASE must be set}"
export PGHOST PGPORT PGUSER PGPASSWORD PGDATABASE

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

echo "[init-schema] applying migrations to ${PGUSER}@${PGHOST}:${PGPORT}/${PGDATABASE}"
for f in "$REPO_ROOT"/migrations/*.sql; do
    echo "[init-schema]   $(basename "$f")"
    psql -v ON_ERROR_STOP=1 -f "$f" >/dev/null
done

echo "[init-schema] seeding ci-admin row (idempotent)"
psql -v ON_ERROR_STOP=1 -c "
    INSERT INTO auth.users (username, email, password_hash, role)
    VALUES ('ci-admin', 'ci-admin@example.test', 'placeholder-not-validated', 'admin')
    ON CONFLICT (username) DO NOTHING;
" >/dev/null

# The OxiCloud server normally provisions a default Personal drive +
# its root folder + Owner role_grant on user creation via
# PersonalDriveLifecycleHook (D0). This script bypasses that pipeline
# — it INSERTs directly into auth.users — so we mirror the hook's
# behaviour here. Without it, integration test fixtures that hand-roll
# INSERTs into storage.files fail with "drive_id not-null violation"
# (M3 made the column mandatory), and helpers that JOIN auth.users
# with storage.drives return RowNotFound.
#
# Four sequential writes inside one transaction (docs/plan/drive.md §3):
# drive + root folder + drives.root_folder_id wire-up + Owner role_grant.
# A single CTE would be more compact but doesn't work — PG's CTE
# sub-statements share an MVCC snapshot, so a later branch's UPDATE
# can't match a row inserted by an earlier branch. The transaction
# form is the production path's shape (DrivePgRepository::create_personal_drive_atomic).
# Idempotency: skipped on retry by the `default_for_user` precondition.
echo "[init-schema] provisioning ci-admin's default Personal drive (idempotent)"
psql -v ON_ERROR_STOP=1 <<'SQL' >/dev/null
DO $$
DECLARE
    admin_id   uuid;
    drive_id   uuid;
    folder_id  uuid;
BEGIN
    SELECT id INTO admin_id FROM auth.users WHERE username = 'ci-admin';
    IF EXISTS (SELECT 1 FROM storage.drives WHERE default_for_user = admin_id) THEN
        RETURN;  -- already provisioned, idempotent no-op
    END IF;

    INSERT INTO storage.drives (kind, default_for_user, quota_bytes)
    VALUES ('personal', admin_id, NULL)
    RETURNING id INTO drive_id;

    -- Post-D7: `storage.folders.user_id` dropped. Ownership lives on the
    -- drive-Owner role_grant below; provenance in `created_by`/`updated_by`.
    INSERT INTO storage.folders
        (name, parent_id, drive_id, created_by, updated_by)
    VALUES ('Personal', NULL, drive_id, admin_id, admin_id)
    RETURNING id INTO folder_id;

    UPDATE storage.drives SET root_folder_id = folder_id WHERE id = drive_id;

    INSERT INTO storage.role_grants
        (subject_type, subject_id, resource_type, resource_id, role, granted_by)
    VALUES ('user', admin_id, 'drive', drive_id, 'owner', admin_id);
END
$$;
SQL

echo "[init-schema] done"
