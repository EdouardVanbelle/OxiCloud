-- ════════════════════════════════════════════════════════════════════════════
-- D0 / M5 — storage.copy_folder_tree drive_id + provenance
-- ════════════════════════════════════════════════════════════════════════════
-- The `copy_folder_tree` SQL function (initial_schema.sql) batches a
-- recursive folder copy in PL/pgSQL — its own INSERTs into
-- `storage.folders` and `storage.files`. D0's M3 made `drive_id` NOT
-- NULL on both tables; the function's pre-D0 body doesn't write it,
-- so any `/api/batch/folders/copy` call errors with "null value in
-- column drive_id" until this migration lands.
--
-- The replacement preserves every other semantic of the original:
--   - level-by-level INSERTs so the BEFORE INSERT trigger
--     (`trg_folders_path`) can resolve the parent's path/lpath from
--     rows inserted in the previous level.
--   - One batched file INSERT (zero-copy via blob hash) at the end.
--   - Returns the same shape: (new_root_id::text, folders_copied,
--     files_copied).
--
-- New columns written:
--   - drive_id: pulled from the source row (intra-drive copy — cross-
--     drive copies are a D2+ feature; the function preserves the
--     source's drive_id for both folders and files).
--   - created_by / updated_by: set to the source row's user_id,
--     matching the dual-write convention used by the Rust repos.

CREATE OR REPLACE FUNCTION storage.copy_folder_tree(
    p_source_id UUID,
    p_target_parent_id UUID,       -- NULL = copy to root
    p_dest_name TEXT DEFAULT NULL   -- NULL = keep source folder name
) RETURNS TABLE(new_root_id TEXT, folders_copied BIGINT, files_copied BIGINT) AS $$
DECLARE
    v_root_lpath   ltree;
    v_root_depth   INT;
    v_max_depth    INT;
    v_level        INT;
    v_folders      BIGINT := 0;
    v_files        BIGINT := 0;
    v_inserted     BIGINT;
    v_new_root     UUID;
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
    -- inserted in the previous level. drive_id + provenance threaded
    -- through from the source row at each level.
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
               fo.drive_id,
               fo.user_id,
               fo.user_id
          FROM storage.folders fo
          JOIN _copy_map cm ON fo.id = cm.old_id
          LEFT JOIN _copy_map pm ON fo.parent_id = pm.old_id
         WHERE NOT fo.is_trashed
           AND nlevel(fo.lpath) = v_level;

        GET DIAGNOSTICS v_inserted = ROW_COUNT;
        v_folders := v_folders + v_inserted;
    END LOOP;

    -- ── Batch copy all files (zero-copy: same blob_hash) ──
    INSERT INTO storage.files(
        name, folder_id, user_id, blob_hash, size, mime_type,
        media_sort_date, drive_id, created_by, updated_by
    )
    SELECT f.name, cm.new_id, f.user_id, f.blob_hash, f.size, f.mime_type,
           f.media_sort_date, f.drive_id, f.user_id, f.user_id
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
