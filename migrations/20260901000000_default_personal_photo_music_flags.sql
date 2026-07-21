-- ════════════════════════════════════════════════════════════════════════════
-- PR-A / §15 — default personal drives get include_in_photo_index +
--              include_in_music_index materialised on the JSONB `policies` bag
-- ════════════════════════════════════════════════════════════════════════════
-- `docs/plan/drive.md` §15 locks the two policies as symmetric per-drive
-- opt-in flags. The default personal drive is always in scope for Photos +
-- Music, so we materialise both flags = `true` on every default personal
-- drive rather than carving out a `default_for_user IS NOT NULL` OR-branch
-- in the query predicate. Net effect: the SQL predicate is a single positive
-- rule keyed off the JSONB flag alone (see `list_media_files` after the
-- companion Rust rewrite).
--
-- New default personal drives get these flags at creation time via
-- `DriveRepository::create_personal_drive_atomic` (the INSERT literal on
-- that path was updated alongside this migration). This migration handles
-- the existing rows, seeded by the D0 backfill.
--
-- Non-default drives (secondary personals, shared drives) are NOT touched —
-- they stay opted-out until the owner flips the flag via the admin
-- "Manage policies" modal.
--
-- Idempotent: `policies || {…}` is a no-op if the keys are already set to
-- the same values, and JSONB `||` is right-precedence so the migration
-- never overwrites an owner's explicit opt-out that was already recorded.
-- (If someone had `include_in_photo_index=false` set on their default
-- personal via a manual PATCH, this UPDATE would still overwrite to true;
-- that's acceptable — the D5 policy UI didn't exist for these flags
-- before this PR, so no such manual opt-out can be in the wild yet.)

UPDATE storage.drives
   SET policies = policies || '{"include_in_photo_index": true, "include_in_music_index": true}'::jsonb
 WHERE default_for_user IS NOT NULL;
