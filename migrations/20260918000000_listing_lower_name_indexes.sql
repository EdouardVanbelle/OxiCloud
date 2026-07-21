-- ════════════════════════════════════════════════════════════════════════════
-- Web-UI listing keyset — expression indexes for the default "name" sort
-- ════════════════════════════════════════════════════════════════════════════
-- `list_resources_paged` (SPA files view) sorts case-insensitively on
-- `LOWER(name)` with an id tie-breaker. The old query applied its keyset
-- cursor OUTSIDE the folders/files UNION-ALL on computed columns, so every
-- page rescanned and top-N-sorted the whole folder (28 ms/page on a
-- 20k-entry folder). The query now pushes the cursor into each branch as a
-- sargable row-value comparison `(LOWER(name), id) > ($str, $id)` — these
-- two partial expression indexes let each branch answer that with one
-- bounded, pre-ordered index-range read (1.3 ms/page, 19.5x;
-- benches/LISTING-KEYSET.md).
--
-- Sibling of `idx_files_folder_name (folder_id, name)` (migration
-- 20260917000000), which serves the byte-wise DAV ordering; the SPA orders
-- by LOWER(name), which that index cannot provide.

CREATE INDEX IF NOT EXISTS idx_files_folder_lname
    ON storage.files (folder_id, LOWER(name), id)
    WHERE NOT is_trashed;

CREATE INDEX IF NOT EXISTS idx_folders_parent_lname
    ON storage.folders (parent_id, LOWER(name), id)
    WHERE NOT is_trashed;
