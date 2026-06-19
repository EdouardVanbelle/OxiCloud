-- ════════════════════════════════════════════════════════════════════════════
-- D0 / M3 — Drive constraints: NOT NULL, FK, indexes
-- ════════════════════════════════════════════════════════════════════════════
-- Third of the D0 migration trio. Runs only after M2 (the backfill) has
-- populated every folder/file row with a `drive_id`. This migration is
-- the point of no easy rollback — once `drive_id` is NOT NULL and
-- foreign-keyed to `storage.drives`, dropping the column requires the
-- application code to first stop reading it.
--
-- What lands here:
--   * NOT NULL on `storage.folders.drive_id` and `storage.files.drive_id`.
--   * Foreign keys from both to `storage.drives(id)` with ON DELETE
--     CASCADE (deleting a drive removes its tree — matches the
--     post-D2 lifecycle plan).
--   * Indexes on `drive_id` for both tables (hot path: list-by-drive,
--     drive-aware Tantivy reseed, drive-quota counters).
--
-- The `user_id` column is intentionally left in place: dual-write during
-- the D0 release cycle is the rollback safety net. D7 drops user_id once
-- the new model has baked.

-- ── 1. NOT NULL on drive_id ────────────────────────────────────────────────
-- M2's post-flight check refused to commit if any row was missing
-- drive_id, so this should never fail. The check at column promotion
-- time is the belt; M2's pre-commit assertion was the suspenders.

ALTER TABLE storage.folders
    ALTER COLUMN drive_id SET NOT NULL;

ALTER TABLE storage.files
    ALTER COLUMN drive_id SET NOT NULL;


-- ── 2. Foreign keys to storage.drives ──────────────────────────────────────
-- ON DELETE CASCADE: when a drive is deleted (D3 ships the delete-drive
-- flow), every folder and file row carrying that drive_id is removed in
-- the same transaction. Trash retention does not apply — drive deletion
-- is the explicit "I'm done with this storage" gesture.

ALTER TABLE storage.folders
    ADD CONSTRAINT folders_drive_id_fkey
    FOREIGN KEY (drive_id) REFERENCES storage.drives(id) ON DELETE CASCADE;

ALTER TABLE storage.files
    ADD CONSTRAINT files_drive_id_fkey
    FOREIGN KEY (drive_id) REFERENCES storage.drives(id) ON DELETE CASCADE;


-- ── 3. Indexes on drive_id ─────────────────────────────────────────────────
-- The hot path that ranks every drive-aware query: "list folders in
-- drive X", "files in drive X for Tantivy reindex", "per-drive quota
-- aggregation". The existing `user_id` indexes are kept during dual-
-- write and dropped in D7 alongside the column.

CREATE INDEX IF NOT EXISTS idx_folders_drive_id ON storage.folders (drive_id);
CREATE INDEX IF NOT EXISTS idx_files_drive_id   ON storage.files   (drive_id);


-- ── 3b. Drive-scoped folder uniqueness indexes ─────────────────────────────
-- Pre-D0 the "no duplicate folder name under the same parent for the same
-- user" constraint was user_id-scoped (docs/plan/drive.md §10). The
-- semantics users actually want is "no duplicate names *within a drive*"
-- — a folder named "Reports" in your Personal drive shouldn't preclude
-- another "Reports" in a shared "Team" drive. Flip the scope here, now
-- that every row has a drive_id.
--
-- Same partial predicate as the originals (NOT is_trashed, plus the
-- root-vs-non-root split via parent_id IS NULL).

DROP INDEX IF EXISTS storage.idx_folders_unique_name;
DROP INDEX IF EXISTS storage.idx_folders_unique_name_root;

CREATE UNIQUE INDEX IF NOT EXISTS idx_folders_unique_name
    ON storage.folders(parent_id, name, drive_id)
    WHERE NOT is_trashed AND parent_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_folders_unique_name_root
    ON storage.folders(name, drive_id)
    WHERE NOT is_trashed AND parent_id IS NULL;


-- ── 4. Post-flight: confirm constraints landed ────────────────────────────
-- Belt-and-suspenders verification that the NOT NULL + FK actually
-- exist after the ALTERs above. Any failure here means PostgreSQL
-- silently no-op'd one of the constraint changes, which would be a
-- bug worth surfacing immediately.

DO $BODY$
DECLARE
    folder_not_null  BOOLEAN;
    file_not_null    BOOLEAN;
    folder_fk_exists BOOLEAN;
    file_fk_exists   BOOLEAN;
BEGIN
    SELECT NOT is_nullable::boolean INTO folder_not_null
    FROM information_schema.columns
    WHERE table_schema = 'storage'
      AND table_name   = 'folders'
      AND column_name  = 'drive_id';

    SELECT NOT is_nullable::boolean INTO file_not_null
    FROM information_schema.columns
    WHERE table_schema = 'storage'
      AND table_name   = 'files'
      AND column_name  = 'drive_id';

    SELECT EXISTS (
        SELECT 1 FROM information_schema.table_constraints
         WHERE table_schema    = 'storage'
           AND table_name      = 'folders'
           AND constraint_name = 'folders_drive_id_fkey'
           AND constraint_type = 'FOREIGN KEY'
    ) INTO folder_fk_exists;

    SELECT EXISTS (
        SELECT 1 FROM information_schema.table_constraints
         WHERE table_schema    = 'storage'
           AND table_name      = 'files'
           AND constraint_name = 'files_drive_id_fkey'
           AND constraint_type = 'FOREIGN KEY'
    ) INTO file_fk_exists;

    IF NOT folder_not_null OR NOT file_not_null
       OR NOT folder_fk_exists OR NOT file_fk_exists THEN
        RAISE EXCEPTION
            'D0 M3 post-flight failed: '
            'folder NOT NULL=%, file NOT NULL=%, folder FK=%, file FK=%. '
            'All four must be true after this migration commits.',
            folder_not_null, file_not_null, folder_fk_exists, file_fk_exists;
    END IF;
END $BODY$;
