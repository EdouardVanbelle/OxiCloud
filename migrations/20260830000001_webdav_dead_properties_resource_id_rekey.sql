-- ════════════════════════════════════════════════════════════════════════════
-- WebDAV dead properties: rekey from (resource_path, user_id) to resource id
-- ════════════════════════════════════════════════════════════════════════════
-- The original schema (20260825000000) keyed dead properties on
-- `(resource_path, user_id, namespace, local_name)`. That model was wrong on
-- two counts:
--
--   1. Dead properties are RESOURCE state per RFC 4918 §4.2 — not user
--      state. Two users on a shared drive PROPFIND'ing the same resource
--      must see the same dead-properties. The user_id key siloed them.
--   2. Every non-WebDAV delete path (REST `DELETE /api/files/{id}`, bulk
--      delete, trash empty, folder cascade) operates on a resource id —
--      not a path. None of those code paths could cheaply call
--      `remove_resource(path, user_id)`, so they leaked dead-property
--      tombstones. WebDAV DELETE itself had a workaround explicit-cleanup
--      call, but the REST surface (which the SvelteKit web UI uses) is the
--      dominant delete path in practice.
--
-- This migration switches the key to a polymorphic resource reference:
-- exactly one of `folder_id` / `file_id` is set, each with `ON DELETE
-- CASCADE` to its owning table. After this lands every existing
-- delete code path — REST, WebDAV, NextCloud DAV, trash, folder
-- cascade — automatically reaps dead-property rows when the underlying
-- file or folder is removed, with no service-layer changes.
--
-- MOVE / RENAME also become no-ops at the dead-properties layer: a
-- folder's id is stable across renames, so its dead properties move
-- with it for free. The `rename_resource()` method on the store is
-- removed in the matching Rust change.
--
-- ── Migration shape ─────────────────────────────────────────────────────────
-- 1. ADD COLUMN folder_id / file_id (NULL-able for now).
-- 2. Backfill folder_id from any row whose resource_path matches a
--    folder row's `path` + `user_id`.
-- 3. Backfill file_id for the rest by joining through the parent folder
--    and matching `parent.path || '/' || fi.name`.
-- 4. Reap rows that didn't resolve — they're tombstones from before
--    the FK-cascade fix, and there's no resource left to attach them to.
-- 5. Add the CHECK constraint that exactly one column is set.
-- 6. Add two partial unique indexes (one per kind).
-- 7. DROP the old columns; PG drops the inline UNIQUE constraint and
--    the explicit path/user index along with them.
--
-- The migration runs in a single sqlx transaction. If any step fails
-- the schema rolls back to (20260825000000) intact.

ALTER TABLE storage.webdav_dead_properties
    ADD COLUMN folder_id UUID NULL REFERENCES storage.folders(id) ON DELETE CASCADE,
    ADD COLUMN file_id   UUID NULL REFERENCES storage.files(id)   ON DELETE CASCADE;

-- Backfill: every row whose resource_path matches an existing folder
-- row's `path` + `user_id` gets its folder_id stamped. `NOT is_trashed`
-- mirrors what the handler does at lookup time — trashed rows can't be
-- the live target of a PROPPATCH anyway, so any old row pointing at a
-- trashed folder is a tombstone (handled in step 4).
UPDATE storage.webdav_dead_properties d
   SET folder_id = fo.id
  FROM storage.folders fo
 WHERE fo.path    = d.resource_path
   AND fo.user_id = d.user_id
   AND NOT fo.is_trashed;

-- Backfill: any remaining row must be a file's properties. Match the
-- same path-computation the resolver uses for files —
-- `parent.path || '/' || fi.name` — so the rewrite mirrors the
-- handler's runtime behaviour exactly.
UPDATE storage.webdav_dead_properties d
   SET file_id = fi.id
  FROM storage.files fi
  JOIN storage.folders parent ON parent.id = fi.folder_id
 WHERE d.folder_id IS NULL
   AND fi.user_id  = d.user_id
   AND NOT fi.is_trashed
   AND parent.path || '/' || fi.name = d.resource_path;

-- Reap orphans. A row that didn't resolve to a folder or file is a
-- tombstone left by some pre-fix delete path: the resource is long
-- gone but the dead-property row was never reaped because the old
-- `(path, user_id)` key kept it disconnected from the resource's
-- lifecycle. The FK-cascade era makes this category structurally
-- impossible, so dropping them on migration is the right cleanup.
DELETE FROM storage.webdav_dead_properties
 WHERE folder_id IS NULL AND file_id IS NULL;

-- Exactly-one-is-set: defends against future code accidentally
-- writing both columns or neither. `<>` between two boolean
-- IS NULL probes is the idiomatic PG shape for XOR.
ALTER TABLE storage.webdav_dead_properties
    ADD CONSTRAINT webdav_dead_properties_one_resource_chk
        CHECK ((folder_id IS NULL) <> (file_id IS NULL));

-- Partial unique indexes — one per resource kind. PG's ON CONFLICT
-- can infer either via `(folder_id, namespace, local_name)
-- WHERE folder_id IS NOT NULL`, matching the partial index, so
-- upsert continues to work without quirky ON CONSTRAINT plumbing.
CREATE UNIQUE INDEX IF NOT EXISTS idx_webdav_dead_props_folder_unique
    ON storage.webdav_dead_properties (folder_id, namespace, local_name)
    WHERE folder_id IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_webdav_dead_props_file_unique
    ON storage.webdav_dead_properties (file_id, namespace, local_name)
    WHERE file_id IS NOT NULL;

-- Drop the old key columns. PG cascades the auto-named inline UNIQUE
-- constraint and the explicit `(resource_path, user_id)` lookup index
-- along with the columns (idx is on resource_path which is going away,
-- so CASCADE is required).
DROP INDEX IF EXISTS storage.idx_webdav_dead_properties_path_user;

ALTER TABLE storage.webdav_dead_properties
    DROP COLUMN resource_path CASCADE,
    DROP COLUMN user_id       CASCADE;
