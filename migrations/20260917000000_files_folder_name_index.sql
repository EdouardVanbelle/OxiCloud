-- ════════════════════════════════════════════════════════════════════════════
-- Name-ordered folder listing index for streaming WebDAV PROPFIND
-- ════════════════════════════════════════════════════════════════════════════
-- `list_files_batch` walks a folder's children in `ORDER BY name` pages of
-- 500 (native + NextCloud PROPFIND). The only index on the filter column
-- was `idx_files_folder_id (folder_id)`, so EVERY page did a bitmap scan of
-- all N children plus a top-(offset+limit) sort — a quadratic full-folder
-- walk (the initial schema's `(folder_id, name, user_id)` index that served
-- this was dropped by 20260902000000 when user_id went nullable).
--
-- This composite index restores the ordered access path: combined with the
-- keyset cursor (`name > $last` — see `file_blob_read_repository.rs`
-- `list_files_batch`), each page is one O(page) index-range read with no
-- sort, regardless of folder size or scroll depth.  Benchmarked in
-- benches/DEAD-PROPS.md's companion doc benches/PROPFIND-PAGING.md.
--
-- Partial (`NOT is_trashed`) to match the listing predicate and keep the
-- index compact; trashed rows are never listed by PROPFIND.

CREATE INDEX IF NOT EXISTS idx_files_folder_name
    ON storage.files (folder_id, name)
    WHERE NOT is_trashed;
