-- Statement-level rewrite of the folder-tree ETag bump triggers.
--
-- The per-row triggers from `20260625000002_folder_tree_modified_at`
-- executed one ancestor-chain UPDATE (`lpath @> target`) for EVERY
-- affected file/folder row:
--   * one upload            → 1 lpath SELECT + O(depth) folder updates,
--                             fired AGAIN by the EXIF media_sort_date
--                             sync UPDATE (double bump per image);
--   * emptying a 50k trash  → 50k trigger executions × chain UPDATEs
--                             inside one transaction (WAL storm, long
--                             lock hold);
--   * concurrent uploads under one root serialize on the root folder
--     row and lock overlapping chains in arbitrary per-row order
--     (deadlock-prone).
--
-- This migration replaces them with AFTER … FOR EACH STATEMENT triggers
-- using transition tables (PG ≥ 10): each DML statement pays exactly ONE
-- bump covering the distinct ancestor chains of all affected rows, and
-- ancestor rows are locked in deterministic id order.
--
-- Deliberate semantic deltas vs the per-row version:
--   * File moves now bump the OLD parent chain too. The per-row trigger
--     used COALESCE(NEW.folder_id, OLD.folder_id), so a move-out never
--     changed the source folder's ETag and sync clients watching the
--     source never saw the file disappear without a deep re-walk.
--   * UPDATEs that only touch storage.files.media_sort_date (the EXIF
--     denormalisation sync) no longer bump: the column is not visible
--     to DAV clients. This removes the double bump per image upload.
--   * Bumps fired from inside another trigger's DML (pg_trigger_depth
--     > 1: FK cascades of user/folder deletion, the lpath cascade
--     rewrite) are skipped on the FILE side as well — the folder-side
--     trigger of the outermost statement already covers the surviving
--     ancestors. The per-row file trigger had no such guard and burned
--     bumps on rows that were themselves being deleted.
--   * Ancestor rows already stamped with this transaction's NOW() are
--     skipped — pure WAL saving, the stored value would be identical.

-- ── File side: INSERT / DELETE ───────────────────────────────────────
-- Both triggers alias their transition table to `changed_rows`, so one
-- function body serves both events (PL/pgSQL resolves the name against
-- the tuplestore registered by whichever trigger fired).
CREATE OR REPLACE FUNCTION storage.bump_tree_from_files_stmt()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF pg_trigger_depth() > 1 THEN
        RETURN NULL;
    END IF;

    WITH targets AS (
        -- Distinct parent folders of all rows in this statement.
        -- Root-level files (folder_id IS NULL) have no ancestors; a
        -- vanished parent row simply drops out of the JOIN.
        SELECT DISTINCT fo.lpath
          FROM (SELECT DISTINCT folder_id
                  FROM changed_rows
                 WHERE folder_id IS NOT NULL) c
          JOIN storage.folders fo ON fo.id = c.folder_id
    ),
    victims AS (
        -- `lpath @> target` = the parent folder itself plus every
        -- ancestor up to the root (GiST-indexed). Lock in id order so
        -- concurrent bumps over overlapping chains cannot deadlock.
        SELECT f.id
          FROM storage.folders f
         WHERE EXISTS (SELECT 1 FROM targets t WHERE f.lpath @> t.lpath)
           AND f.tree_modified_at IS DISTINCT FROM NOW()
         ORDER BY f.id
           FOR UPDATE
    )
    UPDATE storage.folders f
       SET tree_modified_at = NOW()
      FROM victims v
     WHERE f.id = v.id;

    RETURN NULL;
END;
$$;

-- ── File side: UPDATE ────────────────────────────────────────────────
-- Bumps the union of OLD and NEW parent chains so a move invalidates
-- both the source and the destination collection ETags.
--
-- PostgreSQL forbids `AFTER UPDATE OF <cols>` together with transition
-- tables ("transition tables cannot be specified for triggers with
-- column lists"), so the DAV-visibility filter lives inside the
-- function instead: a row only counts when one of the observable
-- columns actually changed value. This is strictly better than a
-- column list — the EXIF media_sort_date sync and no-op UPDATEs
-- (same value re-written) no longer bump anything.
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
    ),
    targets AS (
        SELECT DISTINCT fo.lpath
          FROM (SELECT old_folder_id AS folder_id
                  FROM changed WHERE old_folder_id IS NOT NULL
                UNION
                SELECT new_folder_id
                  FROM changed WHERE new_folder_id IS NOT NULL) c
          JOIN storage.folders fo ON fo.id = c.folder_id
    ),
    victims AS (
        SELECT f.id
          FROM storage.folders f
         WHERE EXISTS (SELECT 1 FROM targets t WHERE f.lpath @> t.lpath)
           AND f.tree_modified_at IS DISTINCT FROM NOW()
         ORDER BY f.id
           FOR UPDATE
    )
    UPDATE storage.folders f
       SET tree_modified_at = NOW()
      FROM victims v
     WHERE f.id = v.id;

    RETURN NULL;
END;
$$;

-- ── Folder side: INSERT / DELETE ─────────────────────────────────────
-- Strict ancestors only (`lpath <> target` preserves the per-row
-- version's self-exclusion: a folder's own create/delete/rename does
-- not bump its own tree_modified_at, only its ancestors').
CREATE OR REPLACE FUNCTION storage.bump_tree_from_folders_stmt()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF pg_trigger_depth() > 1 THEN
        RETURN NULL;
    END IF;

    WITH targets AS (
        SELECT DISTINCT lpath FROM changed_rows WHERE lpath IS NOT NULL
    ),
    victims AS (
        SELECT f.id
          FROM storage.folders f
         WHERE EXISTS (SELECT 1 FROM targets t
                        WHERE f.lpath @> t.lpath AND f.lpath <> t.lpath)
           AND f.tree_modified_at IS DISTINCT FROM NOW()
         ORDER BY f.id
           FOR UPDATE
    )
    UPDATE storage.folders f
       SET tree_modified_at = NOW()
      FROM victims v
     WHERE f.id = v.id;

    RETURN NULL;
END;
$$;

-- ── Folder side: UPDATE ──────────────────────────────────────────────
-- Union of OLD and NEW lpaths: a move bumps the chain it left and the
-- chain it joined. Same value-based filter as the file side (column
-- lists are incompatible with transition tables); the descendant
-- lpath rewrites done by `trg_folders_cascade_path` run at trigger
-- depth 2 and are stopped by the depth guard, and they change none of
-- the compared columns anyway — exactly one bump per move statement.
CREATE OR REPLACE FUNCTION storage.bump_tree_from_folders_stmt_upd()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF pg_trigger_depth() > 1 THEN
        RETURN NULL;
    END IF;

    WITH changed AS (
        SELECT o.lpath AS old_lpath, n.lpath AS new_lpath
          FROM old_rows o
          JOIN new_rows n USING (id)
         WHERE (o.name, o.parent_id, o.is_trashed, o.updated_at)
               IS DISTINCT FROM
               (n.name, n.parent_id, n.is_trashed, n.updated_at)
    ),
    targets AS (
        SELECT DISTINCT lpath
          FROM (SELECT old_lpath AS lpath
                  FROM changed WHERE old_lpath IS NOT NULL
                UNION
                SELECT new_lpath
                  FROM changed WHERE new_lpath IS NOT NULL) c
    ),
    victims AS (
        SELECT f.id
          FROM storage.folders f
         WHERE EXISTS (SELECT 1 FROM targets t
                        WHERE f.lpath @> t.lpath AND f.lpath <> t.lpath)
           AND f.tree_modified_at IS DISTINCT FROM NOW()
         ORDER BY f.id
           FOR UPDATE
    )
    UPDATE storage.folders f
       SET tree_modified_at = NOW()
      FROM victims v
     WHERE f.id = v.id;

    RETURN NULL;
END;
$$;

-- ── Swap the triggers ────────────────────────────────────────────────
-- PG 13 compatibility: DROP-then-CREATE (no CREATE OR REPLACE TRIGGER).
DROP TRIGGER IF EXISTS files_bump_folder_tree_etag ON storage.files;
DROP TRIGGER IF EXISTS folders_bump_folder_tree_etag ON storage.folders;

DROP TRIGGER IF EXISTS files_bump_tree_etag_ins ON storage.files;
CREATE TRIGGER files_bump_tree_etag_ins
    AFTER INSERT ON storage.files
    REFERENCING NEW TABLE AS changed_rows
    FOR EACH STATEMENT EXECUTE FUNCTION storage.bump_tree_from_files_stmt();

DROP TRIGGER IF EXISTS files_bump_tree_etag_del ON storage.files;
CREATE TRIGGER files_bump_tree_etag_del
    AFTER DELETE ON storage.files
    REFERENCING OLD TABLE AS changed_rows
    FOR EACH STATEMENT EXECUTE FUNCTION storage.bump_tree_from_files_stmt();

-- No column list (incompatible with transition tables): the function
-- compares the DAV-observable columns (rename / move / content swap /
-- trash / restore / mtime touch) per row and ignores everything else,
-- including the EXIF media_sort_date sync.
DROP TRIGGER IF EXISTS files_bump_tree_etag_upd ON storage.files;
CREATE TRIGGER files_bump_tree_etag_upd
    AFTER UPDATE ON storage.files
    REFERENCING OLD TABLE AS old_rows NEW TABLE AS new_rows
    FOR EACH STATEMENT EXECUTE FUNCTION storage.bump_tree_from_files_stmt_upd();

DROP TRIGGER IF EXISTS folders_bump_tree_etag_ins ON storage.folders;
CREATE TRIGGER folders_bump_tree_etag_ins
    AFTER INSERT ON storage.folders
    REFERENCING NEW TABLE AS changed_rows
    FOR EACH STATEMENT EXECUTE FUNCTION storage.bump_tree_from_folders_stmt();

DROP TRIGGER IF EXISTS folders_bump_tree_etag_del ON storage.folders;
CREATE TRIGGER folders_bump_tree_etag_del
    AFTER DELETE ON storage.folders
    REFERENCING OLD TABLE AS changed_rows
    FOR EACH STATEMENT EXECUTE FUNCTION storage.bump_tree_from_folders_stmt();

-- The function's value filter ignores path/lpath rewrites (done only
-- by the depth-guarded `trg_folders_cascade_path` cascade) and this
-- trigger's own tree_modified_at writes — neither can re-fire a bump.
DROP TRIGGER IF EXISTS folders_bump_tree_etag_upd ON storage.folders;
CREATE TRIGGER folders_bump_tree_etag_upd
    AFTER UPDATE ON storage.folders
    REFERENCING OLD TABLE AS old_rows NEW TABLE AS new_rows
    FOR EACH STATEMENT EXECUTE FUNCTION storage.bump_tree_from_folders_stmt_upd();

-- Old per-row functions are no longer referenced by any trigger.
DROP FUNCTION IF EXISTS storage.bump_folder_tree_from_file();
DROP FUNCTION IF EXISTS storage.bump_folder_tree_from_folder();

-- ── Fix: descendant path/lpath cascade never fired ───────────────────
-- `trg_folders_cascade_path` was declared `AFTER UPDATE OF path, lpath`,
-- but `UPDATE OF` only matches columns named in the statement's SET
-- clause — and the app's rename/move statements SET `name`/`parent_id`
-- (path/lpath are rewritten by the BEFORE trigger, which does not
-- count). Net effect on deployments to date: renaming or moving a
-- folder silently left every DESCENDANT folder with a stale path and
-- lpath, corrupting subtree queries (deletes, search, ACL cascade,
-- tree-ETag chains) under the old location.
--
-- Repair order matters: drop the broken trigger, canonically rebuild
-- path/lpath for every folder from its parent chain (only stale rows
-- are written; the statement-level bump trigger above sees no
-- DAV-visible column change, so folder ETags are untouched), then
-- re-create the cascade with the column list the app actually hits.
DROP TRIGGER IF EXISTS trg_folders_cascade_path ON storage.folders;

WITH RECURSIVE canon AS (
    SELECT id,
           name::text AS path,
           replace(id::text, '-', '_')::ltree AS lpath
      FROM storage.folders
     WHERE parent_id IS NULL
    UNION ALL
    SELECT f.id,
           c.path || '/' || f.name,
           c.lpath || replace(f.id::text, '-', '_')::ltree
      FROM storage.folders f
      JOIN canon c ON f.parent_id = c.id
)
UPDATE storage.folders f
   SET path = c.path, lpath = c.lpath
  FROM canon c
 WHERE f.id = c.id
   AND (f.path IS DISTINCT FROM c.path OR f.lpath IS DISTINCT FROM c.lpath);

-- name/parent_id: what rename/move statements actually SET (the BEFORE
-- trigger has already recomputed this row's path/lpath by the time the
-- AFTER trigger compares OLD vs NEW). path/lpath stay listed for direct
-- writes. The cascade function's own pg_trigger_depth() guard still
-- stops its batch descendant rewrite from re-firing itself.
CREATE TRIGGER trg_folders_cascade_path
    AFTER UPDATE OF name, parent_id, path, lpath ON storage.folders
    FOR EACH ROW EXECUTE FUNCTION storage.cascade_folder_path();

-- ── Retention-purge support indexes ──────────────────────────────────
-- `delete_expired_bulk` now deletes in LIMIT-ed batches ordered by
-- trashed_at. These partial indexes (trashed rows only — tiny) turn
-- each batch's candidate scan into an index range scan instead of a
-- repeated sequential scan over the whole table.
CREATE INDEX IF NOT EXISTS idx_files_trash_expiry
    ON storage.files (trashed_at) WHERE is_trashed;
CREATE INDEX IF NOT EXISTS idx_folders_trash_expiry
    ON storage.folders (trashed_at) WHERE is_trashed;
