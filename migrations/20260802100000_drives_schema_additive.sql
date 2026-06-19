-- ════════════════════════════════════════════════════════════════════════════
-- D0 / M1 — Drive foundation: additive schema only
-- ════════════════════════════════════════════════════════════════════════════
-- First of three D0 migrations.
--   M1 (this file)    additive — creates storage.drives + nullable columns.
--   M2 (next)         backfill — promotes wrapper folders, fills drive_id.
--   M3 (final)        constraints — NOT NULL + FKs + drive_id indexes.
--
-- This file is **safe to run on a populated database without an outage**.
-- It only ADDs structure (new table, new nullable columns, extended CHECK
-- constraints, new FK targets). No row is modified; no existing query
-- needs to be aware of the new columns yet.
--
-- The migration is reversible at this stage: dropping the new table and
-- the new columns leaves the database identical to its pre-D0 state. The
-- dual-write / data-movement phase (M2) is where rollback becomes
-- progressively harder.

-- ── 1. storage.drives — the central drive entity ────────────────────────────

CREATE TABLE IF NOT EXISTS storage.drives (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Discriminant. Two kinds today; extending the set is a DROP + ADD
    -- CHECK constraint pair (no separate lookup table).
    --   'personal' = single-owner, no membership API
    --   'shared'   = multi-member, full role roster, group-aware
    kind                TEXT NOT NULL
        CHECK (kind IN ('personal', 'shared')),

    -- Set iff this is the user's default personal drive. The partial
    -- unique index below enforces "one default drive per user" without
    -- blocking secondaries (NULL means "not the default"); shared drives
    -- always have NULL here.
    default_for_user    UUID
        REFERENCES auth.users(id) ON DELETE CASCADE,

    -- The drive's mount-point folder. The display name lives here (drives
    -- have no `name` column — see docs/plan/drive.md §3). NULL at the
    -- column type level so the atomic creation CTE can INSERT the drive
    -- row before the root folder exists, then UPDATE this column from
    -- a later CTE branch within the same statement (a column-level
    -- NOT NULL would refuse that initial INSERT). The invariant
    -- "every drive has a root folder" is enforced by the CTE being
    -- the only creation path, plus the M2 backfill populating this
    -- column for migrated drives. Code reading this column may treat
    -- it as Uuid (not Option<Uuid>); a NULL here is a bug.
    root_folder_id      UUID
        REFERENCES storage.folders(id) ON DELETE CASCADE,

    -- Storage quota in bytes. NULL = no quota (admin override / system
    -- drives). Initial value on personal-drive creation is taken from
    -- the owner's `auth.users.storage_quota_bytes` at the application
    -- layer. **Mutation is OxiCloud-admin only** (docs/plan/drive.md §7) —
    -- not in the drive `owner` role bundle.
    quota_bytes         BIGINT,

    -- Running total of bytes consumed. Maintained by D4's incremental
    -- counters; on D0 backfilled from the per-user counters as a
    -- starting baseline.
    used_bytes          BIGINT NOT NULL DEFAULT 0,

    -- Capability flags / feature toggles bag (see docs/plan/drive.md §8
    -- and §15 for the known keys: forbid_public_links,
    -- forbid_external_sharing, include_in_photo_index, forbid_music_index,
    -- etc.). Unknown keys preserved verbatim — the schema is
    -- intentionally permissive so future flags land without migration.
    policies            JSONB NOT NULL DEFAULT '{}'::jsonb,

    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE storage.drives IS
    'Drive entity — pure metadata. The display name and mount point live '
    'on the root folder (root_folder_id). Membership lives in '
    'storage.role_grants with resource_type=''drive''. Replaces the '
    'per-user My Folder wrapper at D0 (see docs/plan/drive.md §3).';
COMMENT ON COLUMN storage.drives.kind IS
    'personal = single-owner (no add_member); shared = multi-member with full role roster.';
COMMENT ON COLUMN storage.drives.default_for_user IS
    'Set iff this is the user''s default personal drive. NULL on secondaries and shared drives.';
COMMENT ON COLUMN storage.drives.root_folder_id IS
    'Drive''s root folder. NULLable at the column level only so the '
    'atomic creation CTE can write it mid-statement; populated invariant '
    'enforced by application. Display name = SELECT name FROM '
    'storage.folders WHERE id = root_folder_id.';
COMMENT ON COLUMN storage.drives.policies IS
    'JSONB capability-flag bag; see docs/plan/drive.md §8 §15 for known keys.';

-- "One default drive per user." Partial unique index — NULLs (every
-- non-default row) are excluded from the constraint surface.
CREATE UNIQUE INDEX IF NOT EXISTS idx_drives_default_for_user_unique
    ON storage.drives (default_for_user)
    WHERE default_for_user IS NOT NULL;

-- Hot-path "what's this drive?" lookup by kind for admin / D3 flows
-- ("list every shared drive").
CREATE INDEX IF NOT EXISTS idx_drives_kind ON storage.drives (kind);


-- ── 2. drive_id columns on folders + files (NULL during M1) ────────────────
-- M2 fills these in for every existing row; M3 promotes them to NOT NULL
-- and adds the FK + index. Keep nullable here so the migration runs on a
-- populated DB without violating a constraint.

ALTER TABLE storage.folders ADD COLUMN IF NOT EXISTS drive_id UUID;
ALTER TABLE storage.files   ADD COLUMN IF NOT EXISTS drive_id UUID;


-- ── 3. Provenance columns: created_by / updated_by ─────────────────────────
-- D0 adds these on folders + files (see docs/plan/drive.md §14). FKs use
-- ON DELETE SET NULL so deleting a user nulls these out instead of
-- cascading the resource away. M2 backfills from the existing `user_id`
-- column so pre-Drive content carries authentic provenance from day one.

ALTER TABLE storage.folders
    ADD COLUMN IF NOT EXISTS created_by UUID
        REFERENCES auth.users(id) ON DELETE SET NULL;
ALTER TABLE storage.folders
    ADD COLUMN IF NOT EXISTS updated_by UUID
        REFERENCES auth.users(id) ON DELETE SET NULL;

ALTER TABLE storage.files
    ADD COLUMN IF NOT EXISTS created_by UUID
        REFERENCES auth.users(id) ON DELETE SET NULL;
ALTER TABLE storage.files
    ADD COLUMN IF NOT EXISTS updated_by UUID
        REFERENCES auth.users(id) ON DELETE SET NULL;

COMMENT ON COLUMN storage.folders.created_by IS
    'Who originally created the folder. NULL when the original creator''s '
    'auth.users row has since been deleted.';
COMMENT ON COLUMN storage.folders.updated_by IS
    'Who last touched the folder (rename, move, metadata change). Same '
    'write-path discipline as updated_at.';
COMMENT ON COLUMN storage.files.created_by IS
    'Who originally uploaded the file. NULL when the original uploader''s '
    'auth.users row has since been deleted.';
COMMENT ON COLUMN storage.files.updated_by IS
    'Who last touched the file (rename, move, overwrite, restore). Same '
    'write-path discipline as updated_at.';


-- ── 4. role_grants resource_type CHECK — admit 'drive' ─────────────────────
-- The D-Prep migration's CHECK only listed 'folder' and 'file'. Drives
-- need to be a valid resource_type so the lifecycle hook (D0-9) and the
-- membership API (D2) can write `role_grants` rows with
-- resource_type='drive'.

ALTER TABLE storage.role_grants
    DROP CONSTRAINT IF EXISTS role_grants_resource_type_check;
ALTER TABLE storage.role_grants
    ADD CONSTRAINT role_grants_resource_type_check
    CHECK (resource_type IN ('folder', 'file', 'drive'));


-- ── 5. updated_at trigger for storage.drives ───────────────────────────────
-- Mirror the convention from auth.users / storage.folders / storage.files
-- so rename / quota-change / policy-toggle bumps updated_at automatically.
-- Drive owners shouldn't have to remember to maintain this.

CREATE OR REPLACE FUNCTION storage.drives_touch_updated_at()
RETURNS trigger AS $$
BEGIN
    NEW.updated_at := now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_drives_touch_updated_at ON storage.drives;
CREATE TRIGGER trg_drives_touch_updated_at
    BEFORE UPDATE ON storage.drives
    FOR EACH ROW EXECUTE FUNCTION storage.drives_touch_updated_at();
