-- ════════════════════════════════════════════════════════════════════════════
-- PR-A / §15 — partial covering index for the drive-scoped Photos timeline
-- ════════════════════════════════════════════════════════════════════════════
-- Sibling of `idx_files_media_timeline` (initial_schema.sql:581), keyed on
-- `drive_id` instead of `user_id`. The Photos handler predicate is being
-- rewritten to `fi.drive_id IN (drives with include_in_photo_index = true
-- AND caller has Read)` — that subquery produces a small drive-id set,
-- and this index gives Postgres one IndexScan per drive_id already
-- ordered by `media_sort_date DESC`, so LIMIT stops the scan early.
-- Same O(LIMIT) shape as the pre-D7 user_id-keyed hot path.
--
-- The old `idx_files_media_timeline (user_id, media_sort_date DESC)` index
-- is intentionally kept for now — it still backs the dedup / storage sweep
-- paths that D7 will migrate separately. Once D7 drops the `user_id`
-- column those paths lose their backing index at the same moment; that PR
-- can drop the old index in the same migration.
--
-- Partial WHERE clause is identical to the existing sibling so the index
-- stays as compact as its predecessor: only image/video rows that aren't
-- trashed.

CREATE INDEX IF NOT EXISTS idx_files_media_timeline_by_drive
    ON storage.files (drive_id, media_sort_date DESC)
    WHERE NOT is_trashed
      AND (mime_type LIKE 'image/%' OR mime_type LIKE 'video/%');
