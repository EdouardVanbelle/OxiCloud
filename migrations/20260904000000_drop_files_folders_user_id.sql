-- ─────────────────────────────────────────────────────────────────────────
-- D7 step 6 — drop `user_id` from `storage.files` and `storage.folders`.
--
-- Companion / final step to:
--   • `20260902000000_files_folders_user_id_nullable.sql` — dropped NOT NULL,
--     swapped uniqueness indexes to drive-scoped, retired the user_id-leading
--     indexes.
--   • `20260902000001_copy_folder_tree_drop_user_id.sql` — stopped writing
--     the column from `storage.copy_folder_tree`.
--
-- All Rust writers already omit `user_id` from INSERTs (step 4). Every read
-- has been rewritten to drive-membership predicates (step 5). This migration
-- removes the column entirely so no future accidental read/write can bind it.
--
-- Ownership continues to live in `storage.role_grants` (drive-Owner role);
-- provenance in `created_by` / `updated_by` (§14).
--
-- ── Dependencies to unpin before ALTER ───────────────────────────────────
--
-- `storage.trash_items` is a VIEW that projects both `f.user_id` and
-- `fo.user_id`. `CREATE OR REPLACE VIEW` can only APPEND columns, never
-- drop or reorder — see `bug_create_or_replace_view_column_order`. So we
-- DROP the view, then recreate it without user_id after the column drop.
--
-- All remaining pre-D7 indexes that referenced `user_id`
-- (`idx_files_trashed`, `idx_files_media_timeline`, and any legacy
-- uniqueness holdovers) are dropped implicitly by `ALTER TABLE DROP
-- COLUMN`. The D0/D7 drive-keyed successors already exist
-- (`idx_files_media_timeline_by_drive`,
-- `idx_files_unique_name_in_folder`, `idx_files_unique_name_at_root`,
-- etc.), so the hot paths retain their O(LIMIT) shape.

-- ── 1. Drop dependent view so the column drop can proceed ────────────────

DROP VIEW IF EXISTS storage.trash_items;

-- ── 2. Drop the column ───────────────────────────────────────────────────

ALTER TABLE storage.files   DROP COLUMN IF EXISTS user_id;
ALTER TABLE storage.folders DROP COLUMN IF EXISTS user_id;

-- ── 3. Recreate the trash view without user_id ───────────────────────────
--
-- `drive_id` is still projected (D2b introduced it) and is the scope
-- column for per-drive trash listing; `caller_group_ids($1)` fans it
-- out to the caller's group memberships via role_grants.

CREATE VIEW storage.trash_items AS
    SELECT f.id, f.name, 'file' AS item_type, f.trashed_at,
           f.original_folder_id AS original_parent_id, f.created_at,
           f.drive_id
    FROM storage.files f
    WHERE f.is_trashed = TRUE
      AND (f.folder_id IS NULL
           OR NOT EXISTS (
               SELECT 1 FROM storage.folders p
                WHERE p.id = f.folder_id AND p.is_trashed = TRUE))
    UNION ALL
    SELECT fo.id, fo.name, 'folder' AS item_type, fo.trashed_at,
           fo.original_parent_id, fo.created_at,
           fo.drive_id
    FROM storage.folders fo
    WHERE fo.is_trashed = TRUE
      AND (fo.parent_id IS NULL
           OR NOT EXISTS (
               SELECT 1 FROM storage.folders p
                WHERE p.id = fo.parent_id AND p.is_trashed = TRUE));

COMMENT ON VIEW storage.trash_items IS
    'Unified view of all trashed files and folders. Post-D7: `user_id` '
    'projection removed — the source column is gone. Scope is `drive_id` '
    'via role_grants membership (see TrashDbRepository::get_trash_items).';

-- ── 4. Post-flight sanity ────────────────────────────────────────────────

DO $BODY$
DECLARE
    files_has_col   BOOLEAN;
    folders_has_col BOOLEAN;
    view_has_col    BOOLEAN;
BEGIN
    SELECT EXISTS (
        SELECT 1 FROM information_schema.columns
         WHERE table_schema = 'storage'
           AND table_name   = 'files'
           AND column_name  = 'user_id'
    ) INTO files_has_col;

    SELECT EXISTS (
        SELECT 1 FROM information_schema.columns
         WHERE table_schema = 'storage'
           AND table_name   = 'folders'
           AND column_name  = 'user_id'
    ) INTO folders_has_col;

    SELECT EXISTS (
        SELECT 1 FROM information_schema.columns
         WHERE table_schema = 'storage'
           AND table_name   = 'trash_items'
           AND column_name  = 'user_id'
    ) INTO view_has_col;

    IF files_has_col THEN
        RAISE EXCEPTION 'storage.files.user_id column did not drop';
    END IF;
    IF folders_has_col THEN
        RAISE EXCEPTION 'storage.folders.user_id column did not drop';
    END IF;
    IF view_has_col THEN
        RAISE EXCEPTION 'storage.trash_items still projects user_id — view recreate skipped';
    END IF;
END;
$BODY$;
