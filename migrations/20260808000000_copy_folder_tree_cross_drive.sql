-- ════════════════════════════════════════════════════════════════════════════
-- D6 — storage.copy_folder_tree cross-drive support
-- ════════════════════════════════════════════════════════════════════════════
-- D0/M5 (`20260802100004_copy_folder_tree_drive_id.sql`) introduced drive_id
-- into this function but pulled it from the SOURCE folder for every level —
-- a deliberate "intra-drive only" limitation called out in that migration's
-- header. After D6 landed cross-drive moves end-to-end (cascade trigger +
-- WITH dest CTE on file_move/folder_move) copies were the lone holdout: a
-- batch-copy of a folder tree into another drive left every new row with
-- the SOURCE's drive_id while parent_id pointed into the DESTINATION drive.
-- Net effect: the per-drive quota sweep (`SUM(size) WHERE drive_id = d.id`)
-- charged the SOURCE drive for size physically living under the dest tree.
--
-- The fix mirrors `copy_file` SQL in
-- `infrastructure/repositories/pg/file_blob_write_repository.rs::copy_file`
-- (the single-file copy path already gets drive_id from the destination via
-- a `dest_folder` CTE) — here we resolve the destination drive ONCE at the
-- top of the function and bind it for every level of folders + every file.
--
-- Provenance contract: `created_by` / `updated_by` on the copied rows STAY
-- as the source row's values. A copy is a duplicate, not a new authoring
-- event; preserving the original author across copies is the correct
-- semantic. Subsequent edits to the copy bump `updated_by` through the
-- normal write path. This makes the previously-deferred caller_id thread
-- (memory: project_copy_folder_tree_caller_id.md) unnecessary — drive_id
-- is the only field that needs the destination's perspective.
--
-- Preserved semantics from the prior body:
--   - level-by-level folder INSERTs so trg_folders_path can resolve
--     parent's path/lpath from rows inserted in the previous level.
--   - One batched file INSERT (zero-copy via blob hash) at the end.
--   - Returns the same shape: (new_root_id::text, folders_copied, files_copied).
--   - Error codes (P0002 missing source, 23505 duplicate name) unchanged.

CREATE OR REPLACE FUNCTION storage.copy_folder_tree(
    p_source_id UUID,
    p_target_parent_id UUID,       -- NULL = copy to root (keeps source drive)
    p_dest_name TEXT DEFAULT NULL   -- NULL = keep source folder name
) RETURNS TABLE(new_root_id TEXT, folders_copied BIGINT, files_copied BIGINT) AS $$
DECLARE
    v_root_lpath    ltree;
    v_root_depth    INT;
    v_max_depth     INT;
    v_level         INT;
    v_folders       BIGINT := 0;
    v_files         BIGINT := 0;
    v_inserted      BIGINT;
    v_new_root      UUID;
    v_dest_drive_id UUID;
BEGIN
    -- Validate source exists
    SELECT fo.lpath, nlevel(fo.lpath)
      INTO v_root_lpath, v_root_depth
      FROM storage.folders fo
     WHERE fo.id = p_source_id AND NOT fo.is_trashed;

    IF v_root_lpath IS NULL THEN
        RAISE EXCEPTION 'Source folder not found: %', p_source_id
            USING ERRCODE = 'P0002';  -- no_data_found
    END IF;

    -- Resolve the destination drive_id ONCE up front. The whole copied
    -- subtree lands in this drive; pulling it per-row from `fo.drive_id`
    -- (the previous body) was the cross-drive bug.
    --
    -- When p_target_parent_id is NULL the caller asked for "copy to
    -- root" — there is no global root in the multi-drive world, so we
    -- preserve the source's drive_id (legacy behaviour, defensive).
    -- Real API call sites always pass a concrete target folder.
    IF p_target_parent_id IS NULL THEN
        SELECT fo.drive_id INTO v_dest_drive_id
          FROM storage.folders fo
         WHERE fo.id = p_source_id;
    ELSE
        SELECT fo.drive_id INTO v_dest_drive_id
          FROM storage.folders fo
         WHERE fo.id = p_target_parent_id AND NOT fo.is_trashed;
        IF v_dest_drive_id IS NULL THEN
            RAISE EXCEPTION 'Target parent folder not found: %', p_target_parent_id
                USING ERRCODE = 'P0002';  -- no_data_found
        END IF;
    END IF;

    -- Temp mapping: every folder in the subtree → new UUID
    CREATE TEMP TABLE IF NOT EXISTS _copy_map(
        old_id UUID PRIMARY KEY,
        new_id UUID NOT NULL DEFAULT gen_random_uuid()
    ) ON COMMIT DROP;
    TRUNCATE _copy_map;

    INSERT INTO _copy_map(old_id)
    SELECT fo.id
      FROM storage.folders fo
     WHERE NOT fo.is_trashed
       AND fo.lpath <@ v_root_lpath;

    -- Remember new root ID
    SELECT cm.new_id INTO v_new_root
      FROM _copy_map cm WHERE cm.old_id = p_source_id;

    -- Max depth for level iteration
    SELECT MAX(nlevel(fo.lpath))
      INTO v_max_depth
      FROM storage.folders fo
      JOIN _copy_map cm ON fo.id = cm.old_id;

    -- ── Insert folders level by level ──
    -- Each level is a separate INSERT so the BEFORE INSERT trigger
    -- (trg_folders_path) can resolve the parent's path/lpath from rows
    -- inserted in the previous level. drive_id is the destination's
    -- (resolved once above); user_id + provenance preserved from source.
    FOR v_level IN v_root_depth .. v_max_depth LOOP
        INSERT INTO storage.folders(
            id, name, parent_id, user_id,
            drive_id, created_by, updated_by
        )
        SELECT cm.new_id,
               CASE WHEN fo.id = p_source_id AND p_dest_name IS NOT NULL
                    THEN p_dest_name ELSE fo.name END,
               CASE WHEN fo.id = p_source_id THEN p_target_parent_id
                    ELSE pm.new_id END,
               fo.user_id,
               v_dest_drive_id,
               fo.created_by,
               fo.updated_by
          FROM storage.folders fo
          JOIN _copy_map cm ON fo.id = cm.old_id
          LEFT JOIN _copy_map pm ON fo.parent_id = pm.old_id
         WHERE NOT fo.is_trashed
           AND nlevel(fo.lpath) = v_level;

        GET DIAGNOSTICS v_inserted = ROW_COUNT;
        v_folders := v_folders + v_inserted;
    END LOOP;

    -- ── Batch copy all files (zero-copy: same blob_hash) ──
    -- drive_id from destination; everything else (user_id, created_by,
    -- updated_by) preserved from source so authorship survives the copy.
    INSERT INTO storage.files(
        name, folder_id, user_id, blob_hash, size, mime_type,
        media_sort_date, drive_id, created_by, updated_by
    )
    SELECT f.name, cm.new_id, f.user_id, f.blob_hash, f.size, f.mime_type,
           f.media_sort_date, v_dest_drive_id, f.created_by, f.updated_by
      FROM storage.files f
      JOIN _copy_map cm ON f.folder_id = cm.old_id
     WHERE NOT f.is_trashed;

    GET DIAGNOSTICS v_files = ROW_COUNT;

    -- ── Batch increment blob ref_counts ──
    IF v_files > 0 THEN
        UPDATE storage.blobs b
           SET ref_count = ref_count + hc.cnt
          FROM (
              SELECT f.blob_hash, COUNT(*)::int AS cnt
                FROM storage.files f
                JOIN _copy_map cm ON f.folder_id = cm.new_id
               WHERE NOT f.is_trashed
               GROUP BY f.blob_hash
          ) hc
         WHERE b.hash = hc.blob_hash;
    END IF;

    RETURN QUERY SELECT v_new_root::text, v_folders, v_files;
END;
$$ LANGUAGE plpgsql;
