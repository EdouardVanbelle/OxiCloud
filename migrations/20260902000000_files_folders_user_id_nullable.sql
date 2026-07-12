-- ─────────────────────────────────────────────────────────────────────────
-- D7 step 5 — retire `user_id` as a write/uniqueness axis on
-- `storage.files` and `storage.folders`.
--
-- Every read that used to filter by `files.user_id = $caller` or
-- `folders.user_id = $caller` has already been migrated to a
-- drive-membership predicate (see D7-pass §6/§10 changes:
-- `file_blob_read_repository`, `folder_db_repository`,
-- `path_resolver_service`, `dedup_service`, plus `authz.require(Read, …)`
-- at every WebDAV consumer site). This migration removes the last
-- reason to keep binding `user_id` on writes:
--
--   1. Files uniqueness indexes swap from `(folder_id, name, user_id)` /
--      `(name, user_id)` → `(drive_id, folder_id, name)` /
--      `(drive_id, name)`. Post-D0 `files.drive_id` is `NOT NULL`, so
--      the drive-scoped form is strictly stronger — a file is unique
--      by its position within its drive, not by "who used to own it".
--      The folder side already got this treatment in D0
--      (`20260802100002_drives_not_null.sql`).
--
--   2. Dead user_id-leading indexes get dropped:
--        - `idx_files_user_id`, `idx_folders_user_id` — nothing scans by
--          `WHERE user_id = $1` any more.
--        - `idx_folders_trashed` — was `(user_id, is_trashed)`; the
--          trash listing moved to `(drive_id, is_trashed)` via the
--          same D7 rewrite.
--        - `idx_files_user_size_active` — was the per-user storage
--          usage summary; the reconciliation sweep now GROUPs by
--          `drive_id` (`storage_usage_service::update_all_drives_storage_usage`).
--
--   3. `ALTER COLUMN user_id DROP NOT NULL` on both tables. The
--      column stays for compat with the follow-up column-drop
--      migration (D7 step 6) but new INSERTs will leave it NULL.
--      Existing rows keep their backfilled values until the drop.
--
-- Steps 4-6 (Rust INSERT binds dropped + PL/pgSQL copy_folder_tree
-- update) ship in the same commit so no in-flight INSERT ever
-- tries to bind a NOT NULL that just went away.

-- ── 1. Swap files uniqueness indexes ─────────────────────────────────────
--
-- Pre-D7: name unique within (folder, user). Post-D7: name unique within
-- (drive, folder). Since a drive has exactly one root folder tree and
-- a given file lives in exactly one drive, this is a strict tightening.
--
-- The `IF EXISTS` guards let this migration re-run cleanly against a DB
-- that's already been partially migrated (dev workflow).

DROP INDEX IF EXISTS storage.idx_files_unique_name_in_folder;
DROP INDEX IF EXISTS storage.idx_files_unique_name_at_root;

CREATE UNIQUE INDEX IF NOT EXISTS idx_files_unique_name_in_folder
    ON storage.files (drive_id, folder_id, name)
    WHERE NOT is_trashed AND folder_id IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_files_unique_name_at_root
    ON storage.files (drive_id, name)
    WHERE NOT is_trashed AND folder_id IS NULL;

-- ── 2. Drop dead user_id-leading indexes ─────────────────────────────────

DROP INDEX IF EXISTS storage.idx_files_user_id;
DROP INDEX IF EXISTS storage.idx_files_user_size_active;
DROP INDEX IF EXISTS storage.idx_folders_user_id;
DROP INDEX IF EXISTS storage.idx_folders_trashed;

-- ── 3. Allow NULL user_id on both tables ─────────────────────────────────

ALTER TABLE storage.files  ALTER COLUMN user_id DROP NOT NULL;
ALTER TABLE storage.folders ALTER COLUMN user_id DROP NOT NULL;

-- ── 4. Post-flight sanity ────────────────────────────────────────────────

DO $BODY$
DECLARE
    files_nullable   BOOLEAN;
    folders_nullable BOOLEAN;
    new_files_uniq   BOOLEAN;
BEGIN
    SELECT is_nullable::boolean INTO files_nullable
      FROM information_schema.columns
     WHERE table_schema = 'storage'
       AND table_name   = 'files'
       AND column_name  = 'user_id';

    SELECT is_nullable::boolean INTO folders_nullable
      FROM information_schema.columns
     WHERE table_schema = 'storage'
       AND table_name   = 'folders'
       AND column_name  = 'user_id';

    SELECT EXISTS (
        SELECT 1 FROM pg_indexes
         WHERE schemaname = 'storage'
           AND indexname  = 'idx_files_unique_name_in_folder'
    ) INTO new_files_uniq;

    IF NOT files_nullable THEN
        RAISE EXCEPTION 'storage.files.user_id NOT NULL constraint did not drop';
    END IF;
    IF NOT folders_nullable THEN
        RAISE EXCEPTION 'storage.folders.user_id NOT NULL constraint did not drop';
    END IF;
    IF NOT new_files_uniq THEN
        RAISE EXCEPTION 'drive-scoped files uniqueness index did not land';
    END IF;
END;
$BODY$;
