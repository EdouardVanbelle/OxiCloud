-- ════════════════════════════════════════════════════════════════════════════
-- Fix: descendant path/lpath cascade silently stopped firing after D6
-- ════════════════════════════════════════════════════════════════════════════
-- The D6 migration `20260807000000_cascade_drive_id_on_folder_move.sql` was
-- written to add `drive_id` to the cascade trigger's column list. Its stated
-- intent (per its own comment) was "add `drive_id` to the column list", but
-- the re-registration replaced `name, parent_id, path, lpath` with
-- `path, lpath, drive_id` — dropping `name` and `parent_id` in the process:
--
--     -- D6 as shipped (BUG):
--     CREATE OR REPLACE TRIGGER trg_folders_cascade_path
--         AFTER UPDATE OF path, lpath, drive_id ON storage.folders
--         FOR EACH ROW EXECUTE FUNCTION storage.cascade_folder_path();
--
-- PostgreSQL's `UPDATE OF <cols>` predicate matches against the statement's
-- explicit SET clause — NOT against what a BEFORE trigger derives. The
-- rename SQL the app issues is `UPDATE storage.folders SET name = $1, ...`
-- and the move SQL is `UPDATE storage.folders SET parent_id = $1, ...`.
-- Neither touches path/lpath/drive_id in its SET list. Net effect of D6:
--
--   * Folder rename: BEFORE trigger (trg_folders_path) correctly rewrites
--     the renamed row's `path` and `lpath` columns directly. AFTER cascade
--     trigger never fires → every DESCENDANT folder retains its old `path`
--     and `lpath` indefinitely. Hidden until a path-keyed lookup misses.
--   * Folder move (intra-drive): same regression, same hidden state.
--   * Folder move (cross-drive): drive_id IS in the SET clause for some of
--     the cross-drive code paths, so D6's drive_id branch fires there. But
--     the path/lpath branch in the same function never fires on rename/move
--     because the trigger gate excludes the SET columns the app uses.
--
-- Discovery: litmus `copymove → move_coll` (test #10) — `DELETE
-- /webdav/litmus/mvdest/subcoll/` returns 404 because `subcoll`'s path
-- column is still `Personal/litmus/mvsrc/subcoll`. The 10 leaf files
-- foo.0..foo.9 directly under mvdest delete fine because their lookup
-- joins through their parent folder's row (mvdest itself), and the BEFORE
-- trigger DID update mvdest's own path correctly on rename. Only DESCENDANT
-- folder rows are affected.
--
-- Fix: re-register the trigger with the column list that covers every
-- statement the app actually issues against storage.folders:
--   - `name` — folder rename
--   - `parent_id` — folder move (intra-drive)
--   - `path`, `lpath` — direct rewrites (migrations, future tooling)
--   - `drive_id` — folder move (cross-drive); preserved from D6
--
-- The cascade function body itself is unchanged. The pg_trigger_depth() > 1
-- guard inside it still stops the descendant-rewrite UPDATE from
-- recursively re-firing the trigger on its own writes.

-- DROP-then-CREATE for PG 13 compatibility (no CREATE OR REPLACE TRIGGER
-- pre-14). Idempotent thanks to IF EXISTS / IF NOT EXISTS semantics.
DROP TRIGGER IF EXISTS trg_folders_cascade_path ON storage.folders;
CREATE TRIGGER trg_folders_cascade_path
    AFTER UPDATE OF name, parent_id, path, lpath, drive_id ON storage.folders
    FOR EACH ROW EXECUTE FUNCTION storage.cascade_folder_path();

-- ── Repair: rebuild stale descendant path/lpath on existing databases ────
-- Any folder rename or intra-drive move that happened between D6 deploying
-- and this fix landing left descendants stranded at their pre-rename path
-- and lpath. The same canonical-rebuild CTE used in
-- `20260730000001_statement_tree_etag.sql` heals the pile in a single
-- statement: walk the tree from each root, derive (path, lpath) from the
-- parent chain, write back only the stale rows.
--
-- Two safety properties of this repair:
--   * The repair UPDATE sets `path` and `lpath` directly. The newly-
--     correct trigger column list above DOES include those columns, but
--     `cascade_folder_path()` only descends to children when OLD differs
--     from NEW *for that row* — descendants are walked level by level by
--     the recursive CTE, so by the time the trigger fires on a child, the
--     child's parent already has its correct path and the child's row is
--     also being rewritten to its correct path. No double-write, no fan-
--     out: the CTE finishes before any trigger could redo the work.
--   * The statement-level tree-ETag bump triggers run their column filter
--     against `(name, parent_id, is_trashed, updated_at)` — none of which
--     change in this UPDATE — so existing sync clients see no spurious
--     ETag churn.

WITH RECURSIVE canon AS (
    SELECT id,
           name::text                              AS path,
           replace(id::text, '-', '_')::ltree      AS lpath
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
