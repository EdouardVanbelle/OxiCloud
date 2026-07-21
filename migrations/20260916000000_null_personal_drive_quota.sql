-- ─────────────────────────────────────────────────────────────────────────
-- Heal + pin the "personal drives always have NULL quota_bytes"
-- invariant from docs/plan/drive.md §7.
--
-- Bug (#595): `folder_service.rs::PersonalDriveLifecycleHook` was
-- calling `create_personal_drive_atomic(user_id, Some(user.storage_quota_bytes()))`,
-- baking the user's envelope quota into `storage.drives.quota_bytes`
-- for every personal drive. Two conventions then collided at upload
-- time:
--
--   * User-envelope check (`check_storage_quota`) treats `0` as
--     unlimited (`quota <= 0 → Ok`).
--   * Drive-quota check (`check_drive_quota`) treats `NULL` as
--     unlimited but `Some(0)` as a literal zero-byte cap.
--
-- Setting user quota to 0 in the Admin UI ("unlimited" per the UI
-- convention) therefore stamped `drives.quota_bytes = 0` on the
-- personal drive at creation, and every subsequent upload was
-- rejected with 507 Insufficient Storage.
--
-- Rust-side fix: `folder_service.rs` now passes `None`. This
-- migration:
--
--   1. NULLs every existing personal drive's `quota_bytes` so already-
--      created users can upload immediately after deploy (Fix 2).
--   2. Adds a CHECK constraint so any future code path that tries to
--      write a non-NULL quota on a personal drive fails at the DB
--      layer instead of silently corrupting state (Fix 3).
--
-- Shared drives are untouched — their quota model is orthogonal and
-- the "NULL = unlimited, positive = numeric cap, 0 = literal zero"
-- semantics are the design (an admin can legitimately lock a shared
-- drive at 0 bytes, e.g. archive-only).

-- ── 1. Heal existing personal-drive rows ────────────────────────────────
--
-- Every row today with `kind = 'personal'` should carry NULL. Set them
-- to NULL unconditionally (a personal drive already at NULL is a no-op
-- under IS DISTINCT FROM). Idempotent on re-run.
UPDATE storage.drives
   SET quota_bytes = NULL
 WHERE kind = 'personal'
   AND quota_bytes IS DISTINCT FROM NULL;

-- ── 2. Pin the invariant at the schema layer ────────────────────────────
--
-- Uses `NOT VALID` + `VALIDATE CONSTRAINT` so the ALTER TABLE grabs
-- only the fast metadata lock instead of scanning the whole table
-- under an ACCESS EXCLUSIVE lock. The row heal above already satisfies
-- every existing row, so the subsequent VALIDATE completes without
-- error.
ALTER TABLE storage.drives
    ADD CONSTRAINT drives_personal_quota_null
    CHECK (kind <> 'personal' OR quota_bytes IS NULL)
    NOT VALID;

ALTER TABLE storage.drives
    VALIDATE CONSTRAINT drives_personal_quota_null;

-- ── 3. Post-flight sanity ───────────────────────────────────────────────
--
-- Refuse to finish if any personal drive still carries a non-NULL
-- quota (defense against a race where a concurrent transaction
-- inserted a bad row between the UPDATE and the VALIDATE — the
-- VALIDATE would already have failed in that case, but the explicit
-- check makes the failure mode obvious in logs).
DO $BODY$
DECLARE
    bad BIGINT;
BEGIN
    SELECT COUNT(*) INTO bad
      FROM storage.drives
     WHERE kind = 'personal'
       AND quota_bytes IS NOT NULL;
    IF bad > 0 THEN
        RAISE EXCEPTION
            'Migration 20260916000000 left % personal drive(s) with a non-NULL quota_bytes',
            bad;
    END IF;
END;
$BODY$;
