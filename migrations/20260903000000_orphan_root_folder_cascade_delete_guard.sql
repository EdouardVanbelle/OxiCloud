-- ═══════════════════════════════════════════════════════════════════════════
-- D0-step-8 companion — cascade-delete guard for the orphan-root check.
--
-- Fixes a latent bug in `storage.check_no_orphan_root_folder` that
-- surfaced during user-delete tests. Repro (verified against a fresh
-- test DB with no other data):
--
--   INSERT INTO auth.users … one user
--   Run the atomic personal-drive create (drive + root folder +
--     drives.root_folder_id wire-up + owner role_grant)
--   DELETE FROM auth.users WHERE id = <that user>
--   → ERROR: Orphan root folder rejected …
--
-- Root cause — the FK columns `storage.folders.created_by` and
-- `storage.folders.updated_by` are declared
-- `REFERENCES auth.users(id) ON DELETE SET NULL` (D0/M1 migration
-- `20260802100000_drives_schema_additive.sql`, lines 117-129). So when
-- `DELETE FROM auth.users` runs, PostgreSQL cascades a SET NULL
-- update onto every folder row referencing that user — including that
-- user's own personal-drive root folder. That UPDATE fires the
-- DEFERRED `trg_no_orphan_root_folder` constraint trigger, which queues
-- a check on the row's `NEW` state.
--
-- Cascade order (all inside the same transaction) is: SET NULL on the
-- folder → cascade DELETE storage.drives (default_for_user FK) →
-- cascade DELETE storage.folders (drive_id FK). By COMMIT, the drive
-- and the folder are both gone. When the deferred trigger fires, its
-- query `EXISTS (drive d WHERE d.id = NEW.drive_id AND d.root_folder_id
-- = NEW.id)` finds no drive, so it raises. The check is correct in
-- isolation — but the row it's checking no longer exists, so the
-- invariant it's protecting no longer applies.
--
-- Fix: add an existence guard before the drive lookup. If the row has
-- been deleted in the same transaction, skip the check — a deleted row
-- can't be an orphan by definition.
--
-- This preserves the original invariant on all live rows:
--   * The atomic four-write create transaction still gets checked at
--     COMMIT and still requires the drive→folder wire-up (the folder
--     row exists at COMMIT because we didn't delete it).
--   * Direct SQL that tries to insert an orphan root folder is still
--     rejected (the INSERT queues a check, the row exists at COMMIT,
--     the drive lookup fails, exception raised).
--   * The only new behaviour is "if this row was deleted before COMMIT,
--     silently skip" — which is what the caller wanted anyway.
--
-- No table changes, no data changes, no reverse migration needed —
-- `CREATE OR REPLACE FUNCTION` is idempotent, and every future call
-- of the trigger picks up the new body immediately.

CREATE OR REPLACE FUNCTION storage.check_no_orphan_root_folder()
RETURNS trigger AS $$
BEGIN
    -- Non-root rows are guaranteed correct by their parent_id FK.
    IF NEW.parent_id IS NOT NULL THEN
        RETURN NULL;
    END IF;

    -- Trashed root folders are soft-deleted in place — the resolver
    -- never lands on them, and they were valid roots before they got
    -- trashed. Skip enforcement; the row's history is preserved.
    IF NEW.is_trashed THEN
        RETURN NULL;
    END IF;

    -- Cascade-delete guard (NEW in this migration).
    --
    -- The trigger is DEFERRABLE INITIALLY DEFERRED — it fires at COMMIT
    -- with `NEW` captured at trigger-queue time. If the row was
    -- subsequently deleted in the same transaction (e.g. the cascade
    -- path from `DELETE FROM auth.users` → SET NULL on created_by /
    -- updated_by → cascade DELETE storage.drives → cascade DELETE
    -- storage.folders), the invariant no longer applies: there's no
    -- orphan because the row itself is gone.
    IF NOT EXISTS (SELECT 1 FROM storage.folders WHERE id = NEW.id) THEN
        RETURN NULL;
    END IF;

    -- The core check: some drive must point at this row as its
    -- root_folder_id, AND that drive must be the same one carrying
    -- our drive_id (the 1:1 bidirectional invariant from §3).
    IF NOT EXISTS (
        SELECT 1 FROM storage.drives d
         WHERE d.id = NEW.drive_id
           AND d.root_folder_id = NEW.id
    ) THEN
        RAISE EXCEPTION
            'Orphan root folder rejected: storage.folders id=% has '
            'parent_id IS NULL and drive_id=%, but no drive has '
            'root_folder_id pointing at it. Root folders must be '
            'created via the atomic four-write transaction (see '
            'docs/plan/drive.md §3 and DrivePgRepository::'
            'create_personal_drive_atomic); direct SQL is not '
            'supported.',
            NEW.id, NEW.drive_id;
    END IF;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION storage.check_no_orphan_root_folder() IS
    'DB-level guard for the "every root folder belongs to a drive" '
    'invariant. Wired as a DEFERRABLE INITIALLY DEFERRED constraint '
    'trigger so the atomic create transaction (folder INSERTed before '
    'drive UPDATEd) commits cleanly. Skips the check on rows that were '
    'deleted in the same tx (cascade path from user delete). See '
    'docs/plan/drive.md §3.';
