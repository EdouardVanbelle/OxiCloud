-- ─────────────────────────────────────────────────────────────────────────
-- D7 step 5 — drop `user_id` from `storage.copy_folder_tree` INSERTs.
--
-- Companion to `20260902000000_files_folders_user_id_nullable.sql`. That
-- migration made both `storage.files.user_id` and `storage.folders.user_id`
-- nullable; this one stops writing to them from the copy-tree flow so
-- copied rows leave the column NULL — provenance moves entirely to the
-- `created_by` / `updated_by` §14 columns, which the PL/pgSQL already
-- preserved from source.
--
-- No behavioural change apart from the write-time projection: reads no
-- longer key on `files.user_id` (all migrated to drive-membership
-- predicates), the uniqueness constraints don't include `user_id`
-- (companion migration swapped them to drive-scoped), and provenance
-- was already flowing through `created_by` / `updated_by`.
--
-- Identical function signature and return shape — no caller update
-- needed.

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
    -- Validate source exists.
    SELECT fo.lpath, nlevel(fo.lpath)
      INTO v_root_lpath, v_root_depth
      FROM storage.folders fo
     WHERE fo.id = p_source_id AND NOT fo.is_trashed;

    IF v_root_lpath IS NULL THEN
        RAISE EXCEPTION 'Source folder not found: %', p_source_id
            USING ERRCODE = 'P0002';  -- no_data_found
    END IF;

    -- Resolve destination drive_id once up front (cross-drive copy path).
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
                USING ERRCODE = 'P0002';
        END IF;
    END IF;

    -- Temp mapping: every folder in the subtree → new UUID.
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

    SELECT cm.new_id INTO v_new_root
      FROM _copy_map cm WHERE cm.old_id = p_source_id;

    SELECT MAX(nlevel(fo.lpath))
      INTO v_max_depth
      FROM storage.folders fo
      JOIN _copy_map cm ON fo.id = cm.old_id;

    -- ── Insert folders level by level ──
    -- Post-D7: `user_id` intentionally omitted from the column list so
    -- copied rows leave the (now-nullable) column NULL. Provenance is
    -- carried by `created_by` / `updated_by` (§14 columns) — preserved
    -- from source so authorship survives the copy.
    FOR v_level IN v_root_depth .. v_max_depth LOOP
        INSERT INTO storage.folders(
            id, name, parent_id,
            drive_id, created_by, updated_by
        )
        SELECT cm.new_id,
               CASE WHEN fo.id = p_source_id AND p_dest_name IS NOT NULL
                    THEN p_dest_name ELSE fo.name END,
               CASE WHEN fo.id = p_source_id THEN p_target_parent_id
                    ELSE pm.new_id END,
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

    -- Temp mapping for files src→dst (dst ids pre-allocated so we can
    -- reference them in the dead-property duplication below).
    CREATE TEMP TABLE IF NOT EXISTS _copy_file_map(
        old_id UUID PRIMARY KEY,
        new_id UUID NOT NULL DEFAULT gen_random_uuid()
    ) ON COMMIT DROP;
    TRUNCATE _copy_file_map;

    INSERT INTO _copy_file_map(old_id)
    SELECT f.id
      FROM storage.files f
      JOIN _copy_map cm ON f.folder_id = cm.old_id
     WHERE NOT f.is_trashed;

    -- ── Batch copy all files (zero-copy: same blob_hash) ──
    -- Post-D7: `user_id` omitted. Provenance via `created_by`/`updated_by`.
    INSERT INTO storage.files(
        id, name, folder_id, blob_hash, size, mime_type,
        media_sort_date, drive_id, created_by, updated_by
    )
    SELECT fm.new_id, f.name, cm.new_id, f.blob_hash, f.size,
           f.mime_type, f.media_sort_date, v_dest_drive_id, f.created_by,
           f.updated_by
      FROM storage.files f
      JOIN _copy_map      cm ON f.folder_id = cm.old_id
      JOIN _copy_file_map fm ON fm.old_id   = f.id
     WHERE NOT f.is_trashed;

    GET DIAGNOSTICS v_files = ROW_COUNT;

    -- Batch increment blob ref_counts.
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

    -- Duplicate dead properties per RFC 4918 §8.8 — id-keyed store.
    INSERT INTO storage.webdav_dead_properties
        (folder_id, namespace, local_name, value)
    SELECT cm.new_id, dp.namespace, dp.local_name, dp.value
      FROM storage.webdav_dead_properties dp
      JOIN _copy_map cm ON dp.folder_id = cm.old_id;

    INSERT INTO storage.webdav_dead_properties
        (file_id, namespace, local_name, value)
    SELECT fm.new_id, dp.namespace, dp.local_name, dp.value
      FROM storage.webdav_dead_properties dp
      JOIN _copy_file_map fm ON dp.file_id = fm.old_id;

    RETURN QUERY SELECT v_new_root::text, v_folders, v_files;
END;
$$ LANGUAGE plpgsql;
