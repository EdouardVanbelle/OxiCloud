-- ════════════════════════════════════════════════════════════════════════════
-- Trash listing — partial (drive_id, trashed_at) indexes on trashed rows
-- ════════════════════════════════════════════════════════════════════════════
-- The trash surface (`TrashDbRepository::list_resources_paged`, `clear_trash`,
-- `get_all_trashed_file_ids`) filters `drive_id = ANY($drives) AND
-- is_trashed = TRUE` and keysets on `trashed_at` / `deletion_date`
-- (`deletion_date` = `trashed_at` + a constant retention interval, so it is
-- strictly monotonic in `trashed_at`).
--
-- The historical `idx_{files,folders}_trashed (user_id, is_trashed)` indexes
-- were dropped with the `user_id` columns (migration 20260904000000), leaving
-- only:
--   • `idx_{files,folders}_drive_id (drive_id)` — seeks the drive but then
--     filter-scans every LIVE row of the drive to find the trashed few;
--   • `idx_{files,folders}_trash_expiry (trashed_at) WHERE is_trashed` —
--     trashed-only but keyed for the GLOBAL retention sweeper; a per-drive
--     listing scans every tenant's trash and filters.
--
-- These partial indexes bound the read to exactly the caller's drives'
-- trashed rows, pre-ordered for the trashed_at/deletion_date keysets.
-- The retention sweeper keeps `idx_*_trash_expiry` (global, no drive
-- predicate). Benchmark: benches/ROUND10.md (trash-listing section).

CREATE INDEX IF NOT EXISTS idx_files_drive_trashed
    ON storage.files (drive_id, trashed_at)
    WHERE is_trashed;

CREATE INDEX IF NOT EXISTS idx_folders_drive_trashed
    ON storage.folders (drive_id, trashed_at)
    WHERE is_trashed;
