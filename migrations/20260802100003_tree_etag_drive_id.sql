-- ════════════════════════════════════════════════════════════════════════════
-- D0 / M4 — tree_etag_dirty drive_id awareness
-- ════════════════════════════════════════════════════════════════════════════
-- Fourth D0 migration. The async tree-ETag queue (introduced in
-- `20260627000000_async_tree_etag_queue.sql`) walks `f.lpath @> t.lpath`
-- to bump every ancestor of a changed folder/file. Without a drive_id
-- filter, that walk can match folders in OTHER drives whose ltree
-- prefixes happen to align numerically — a silent cross-drive ETag
-- bump that nobody would notice until D2 ships shared drives.
--
-- This migration is preventive: it adds the column, teaches the
-- triggers to carry drive_id into the queue, and the Rust flush
-- service (`tree_etag_flush_service.rs`) gets the matching
-- `AND f.drive_id = t.drive_id` predicate so the cross-drive case is
-- closed end-to-end before drives can collide.

-- ── 1. drive_id column on the queue table ──────────────────────────────────
-- NULL-tolerant during the rollover: existing queue entries enqueued by
-- the old triggers have no drive_id. They drain on the next flush tick
-- with the old (no drive_id) semantics — which for D0 is still correct
-- because every personal drive's lpath is structurally disjoint from
-- every other user's. New entries enqueued by the updated triggers
-- below carry a non-NULL value.

ALTER TABLE storage.tree_etag_dirty ADD COLUMN IF NOT EXISTS drive_id UUID;


-- ── 2. File-side INSERT/DELETE trigger ─────────────────────────────────────
-- Source rows live in `storage.files` (changed_rows). Each file row
-- carries `drive_id` directly (D0-8 dual-write). Pull from the joined
-- folder row so the (lpath, folder_id, drive_id) triple is internally
-- consistent — a single source of truth per enqueued row.

CREATE OR REPLACE FUNCTION storage.bump_tree_from_files_stmt()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF pg_trigger_depth() > 1 THEN
        RETURN NULL;
    END IF;

    INSERT INTO storage.tree_etag_dirty (lpath, folder_id, drive_id)
    SELECT DISTINCT fo.lpath, fo.id, fo.drive_id
      FROM (SELECT DISTINCT folder_id
              FROM changed_rows
             WHERE folder_id IS NOT NULL) c
      JOIN storage.folders fo ON fo.id = c.folder_id;

    RETURN NULL;
END;
$$;


-- ── 3. File-side UPDATE trigger ────────────────────────────────────────────
-- Move case: the file changed parent. Bump both the old and the new
-- parent chains (each in its own drive — D0 has them equal, D2 can see
-- them diverge once cross-drive moves land).

CREATE OR REPLACE FUNCTION storage.bump_tree_from_files_stmt_upd()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF pg_trigger_depth() > 1 THEN
        RETURN NULL;
    END IF;

    WITH changed AS (
        SELECT o.folder_id AS old_folder_id, n.folder_id AS new_folder_id
          FROM old_rows o
          JOIN new_rows n USING (id)
         WHERE (o.name, o.folder_id, o.blob_hash, o.size,
                o.mime_type, o.is_trashed, o.updated_at)
               IS DISTINCT FROM
               (n.name, n.folder_id, n.blob_hash, n.size,
                n.mime_type, n.is_trashed, n.updated_at)
    )
    INSERT INTO storage.tree_etag_dirty (lpath, folder_id, drive_id)
    SELECT DISTINCT fo.lpath, fo.id, fo.drive_id
      FROM (SELECT old_folder_id AS folder_id
              FROM changed WHERE old_folder_id IS NOT NULL
            UNION
            SELECT new_folder_id
              FROM changed WHERE new_folder_id IS NOT NULL) c
      JOIN storage.folders fo ON fo.id = c.folder_id;

    RETURN NULL;
END;
$$;


-- ── 4. Folder-side INSERT/DELETE trigger ──────────────────────────────────
-- changed_rows are storage.folders rows, which carry drive_id directly
-- post-D0-7.

CREATE OR REPLACE FUNCTION storage.bump_tree_from_folders_stmt()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF pg_trigger_depth() > 1 THEN
        RETURN NULL;
    END IF;

    INSERT INTO storage.tree_etag_dirty (lpath, folder_id, drive_id)
    SELECT DISTINCT subpath(lpath, 0, nlevel(lpath) - 1), parent_id, drive_id
      FROM changed_rows
     WHERE lpath IS NOT NULL
       AND nlevel(lpath) > 1;

    RETURN NULL;
END;
$$;


-- ── 5. Folder-side UPDATE trigger ──────────────────────────────────────────
-- Same shape as the INSERT/DELETE case; we union OLD and NEW parents,
-- carrying each chain's drive_id from the matching row side.

CREATE OR REPLACE FUNCTION storage.bump_tree_from_folders_stmt_upd()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF pg_trigger_depth() > 1 THEN
        RETURN NULL;
    END IF;

    WITH changed AS (
        SELECT o.lpath     AS old_lpath,
               o.parent_id AS old_parent_id,
               o.drive_id  AS old_drive_id,
               n.lpath     AS new_lpath,
               n.parent_id AS new_parent_id,
               n.drive_id  AS new_drive_id
          FROM old_rows o
          JOIN new_rows n USING (id)
         WHERE (o.name, o.parent_id, o.is_trashed, o.updated_at)
               IS DISTINCT FROM
               (n.name, n.parent_id, n.is_trashed, n.updated_at)
    )
    INSERT INTO storage.tree_etag_dirty (lpath, folder_id, drive_id)
    SELECT DISTINCT subpath(c.lpath, 0, nlevel(c.lpath) - 1), c.parent_id, c.drive_id
      FROM (SELECT old_lpath AS lpath,
                   old_parent_id AS parent_id,
                   old_drive_id  AS drive_id
              FROM changed WHERE old_lpath IS NOT NULL
            UNION
            SELECT new_lpath, new_parent_id, new_drive_id
              FROM changed WHERE new_lpath IS NOT NULL) c
     WHERE nlevel(c.lpath) > 1;

    RETURN NULL;
END;
$$;
