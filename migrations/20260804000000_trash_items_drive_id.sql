-- D2b — surface `drive_id` on the unified trash view.
--
-- `storage.trash_items` is the read-side projection of soft-deleted files and
-- folders. D0 added `drive_id` to both `storage.files` and `storage.folders`,
-- but the view shipped before that and still reflects only `user_id`. D2b's
-- per-drive trash authorisation needs `drive_id` per row so:
--
--   1. Listing can filter to drives the caller can read (the storage
--      precheck — `pg_acl_engine.caller_role_on_drive_cached` — replaces
--      the legacy `WHERE user_id = caller_id` scope).
--   2. The UI can group trash items by drive (per the D2b spec).
--   3. The trash sweeper / orphan reclamation paths (added later in D2b)
--      can operate per-drive instead of per-user.
--
-- The view is `CREATE OR REPLACE`, so this is a pure schema-shape change —
-- no data migration needed. The two source columns (`storage.files.drive_id`,
-- `storage.folders.drive_id`) are both `NOT NULL` after D0's
-- `20260802100002_drives_not_null.sql`, so the projection inherits NOT NULL
-- semantics automatically (no `COALESCE` fallback needed).

-- `CREATE OR REPLACE VIEW` is restrictive: it can ADD columns at the END
-- but never re-order or rename existing ones. `drive_id` goes after every
-- pre-existing column so PG doesn't read this as renaming `trashed_at` →
-- `drive_id`. Column ORDER on the view changes; consumers SELECT by name
-- so they're unaffected.

CREATE OR REPLACE VIEW storage.trash_items AS
    SELECT f.id, f.name, 'file' AS item_type, f.user_id, f.trashed_at,
           f.original_folder_id AS original_parent_id, f.created_at,
           f.drive_id
    FROM storage.files f
    WHERE f.is_trashed = TRUE
      AND (f.folder_id IS NULL
           OR NOT EXISTS (
               SELECT 1 FROM storage.folders p
                WHERE p.id = f.folder_id AND p.is_trashed = TRUE))
    UNION ALL
    SELECT fo.id, fo.name, 'folder' AS item_type, fo.user_id, fo.trashed_at,
           fo.original_parent_id, fo.created_at,
           fo.drive_id
    FROM storage.folders fo
    WHERE fo.is_trashed = TRUE
      AND (fo.parent_id IS NULL
           OR NOT EXISTS (
               SELECT 1 FROM storage.folders p
                WHERE p.id = fo.parent_id AND p.is_trashed = TRUE));

COMMENT ON VIEW storage.trash_items IS
    'Unified view of all trashed files and folders. `drive_id` added in D2b '
    'so callers can scope by accessible drives (the legacy per-user scope '
    'is being phased out alongside the user_id column in D7).';
