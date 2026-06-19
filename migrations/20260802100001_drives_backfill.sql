-- ════════════════════════════════════════════════════════════════════════════
-- D0 / M2 — Drive backfill: adopt wrappers + stamp drive_id + provenance
-- ════════════════════════════════════════════════════════════════════════════
-- Second of the D0 migration trio. Implements §A of the migration plan in
-- docs/plan/drive.md — the "rename-and-adopt" model:
--
--   * For every internal user with a root folder, create a Personal drive
--     (metadata only — no `name` column; the display name lives on the
--     root folder).
--   * The folder literally named `My Folder - <username>` is **adopted
--     in place** as the user's default Personal drive's root folder:
--     `drives.root_folder_id` points at it, its `drive_id` is stamped,
--     and it is renamed to `Personal`. The wrapper row is NOT deleted;
--     descendants are NOT promoted. The AFTER-UPDATE folder cascade
--     trigger rewrites descendant `path`/`lpath` automatically when the
--     wrapper rename fires — no bulk path UPDATE in this migration.
--   * Any sibling root folders become secondary Personal drives'
--     root folders (`default_for_user = NULL`, original folder name
--     preserved). Same adoption pattern: drive_id stamped, drives.root_folder_id
--     wired, no rename.
--   * One owner role_grants row per new drive.
--   * Every existing folder/file row gets a `drive_id` (cascaded down the
--     ltree from the wrapper).
--   * Every existing folder/file row gets `created_by` and `updated_by`
--     backfilled from the existing `user_id` column.
--
-- External users (`auth.users.is_external = TRUE`) are intentionally
-- skipped — they have no root folder of their own, only role_grants
-- against other users' resources.

-- ── Pre-flight 1: refuse on sibling root literally named 'drives' ──────────
-- 'drives' is a reserved URL segment on the native WebDAV surface
-- (`/webdav/drives/<uuid>/`). A folder named 'drives' would shadow the
-- drive-listing route once D1 ships. Surface the conflict now — operator
-- renames before retrying.

DO $BODY$
DECLARE
    bad_count BIGINT;
BEGIN
    SELECT count(*) INTO bad_count
    FROM storage.folders f
    JOIN auth.users u ON u.id = f.user_id
    WHERE f.parent_id IS NULL
      AND NOT f.is_trashed
      AND lower(f.name) = 'drives'
      AND NOT u.is_external;

    IF bad_count > 0 THEN
        RAISE EXCEPTION
            'D0 backfill refused: % root folder(s) literally named ''drives'' '
            'would collide with the reserved /webdav/drives/<uuid>/ URL segment '
            'once D1 ships. Rename the offending folders, then retry the '
            'migration. Query to inspect:  SELECT f.id, f.user_id, f.name '
            'FROM storage.folders f JOIN auth.users u ON u.id = f.user_id '
            'WHERE f.parent_id IS NULL AND NOT f.is_trashed AND lower(f.name) '
            '= ''drives'' AND NOT u.is_external;',
            bad_count;
    END IF;
END $BODY$;


-- ── Pre-flight 1b: refuse on rename collision with sibling root 'Personal' ─
-- The default-wrapper rename in step 4 changes `My Folder - <username>` →
-- `Personal`. The pre-M3 folder unique index is user_id-scoped
-- (`(name, user_id) WHERE parent_id IS NULL`), so a user who already has
-- a SQL-created sibling root literally named `Personal` would trip the
-- index when M2 tries to rename the wrapper. Surface the collision now —
-- operator renames the offending sibling before retrying, then it gets
-- adopted as a secondary drive with whatever new name it carries.

DO $BODY$
DECLARE
    collisions BIGINT;
BEGIN
    SELECT count(*) INTO collisions
    FROM auth.users u
    JOIN storage.folders wrapper
      ON wrapper.user_id   = u.id
     AND wrapper.parent_id IS NULL
     AND NOT wrapper.is_trashed
     AND wrapper.name      = 'My Folder - ' || u.username
    JOIN storage.folders sibling
      ON sibling.user_id   = u.id
     AND sibling.parent_id IS NULL
     AND NOT sibling.is_trashed
     AND sibling.id        != wrapper.id
     AND sibling.name      = 'Personal'
    WHERE NOT u.is_external;

    IF collisions > 0 THEN
        RAISE EXCEPTION
            'D0 backfill refused: % user(s) have both a `My Folder - <username>` '
            'wrapper AND a sibling root named ''Personal''. The wrapper rename '
            'step would collide on the user_id-scoped folder unique index. '
            'Rename the offending sibling first. Query to inspect: SELECT u.id, '
            'u.username FROM auth.users u JOIN storage.folders w ON w.user_id=u.id '
            'AND w.parent_id IS NULL AND w.name=''My Folder - ''||u.username '
            'JOIN storage.folders s ON s.user_id=u.id AND s.parent_id IS NULL '
            'AND s.id!=w.id AND s.name=''Personal'' WHERE NOT u.is_external;',
            collisions;
    END IF;
END $BODY$;


-- ── Pre-flight 2: report sibling-root distribution (informational) ─────────
-- Most users have exactly one root (`My Folder - <username>`). Some may
-- have SQL-added siblings — those become secondary drives. Surface the
-- count so operators can sanity-check before the migration commits.

DO $BODY$
DECLARE
    extras BIGINT;
BEGIN
    WITH counts AS (
        SELECT u.id AS user_id, count(*) AS root_count
        FROM auth.users u
        JOIN storage.folders f ON f.user_id = u.id
        WHERE f.parent_id IS NULL
          AND NOT f.is_trashed
          AND NOT u.is_external
        GROUP BY u.id
    )
    SELECT count(*) INTO extras FROM counts WHERE root_count > 1;

    IF extras > 0 THEN
        RAISE NOTICE
            'D0 backfill: % user(s) have more than one root folder. Their '
            'siblings will be promoted to secondary Personal drives. Inspect '
            'with: WITH c AS (SELECT u.id, u.username, count(*) cnt FROM '
            'auth.users u JOIN storage.folders f ON f.user_id=u.id WHERE '
            'f.parent_id IS NULL AND NOT f.is_trashed AND NOT u.is_external '
            'GROUP BY u.id, u.username) SELECT * FROM c WHERE cnt > 1;',
            extras;
    END IF;
END $BODY$;


-- ── 1. Plan every drive that needs to be created ──────────────────────────
-- Temp table is the cleanest way to pre-compute the new UUIDs once and
-- reuse them across the INSERT-drives, INSERT-grants, and UPDATE-folders
-- steps below. `gen_random_uuid()` in a CTE would re-evaluate on every
-- branch.
--
-- A row joins each existing root folder to its future drive_id. The
-- `is_default` flag is computed per-user as a window function so EVERY
-- internal user with at least one root folder ends up with exactly one
-- default drive — even if the user's wrapper was renamed away from
-- `My Folder - <username>` at some point. Preference order:
--   1. The folder literally named `My Folder - <username>` if it exists.
--   2. Otherwise the oldest root by `created_at`, tiebroken by `id`.

-- `ON COMMIT DROP` would race with the `[init-schema]` CI flow that
-- runs migrations via `psql \i` in autocommit mode: the CREATE statement
-- commits, the table drops, and the next statement (the DO block) can't
-- see it. The plain temp table survives until session end in autocommit
-- mode and until our explicit DROP at the bottom under `sqlx migrate`'s
-- single-tx mode. Works under both.
CREATE TEMPORARY TABLE _drive_plan AS
WITH root_folders AS (
    SELECT
        u.id                                AS user_id,
        u.username                          AS username,
        u.storage_quota_bytes               AS quota,
        f.id                                AS wrapper_id,
        f.name                              AS wrapper_name,
        f.created_at                        AS created_at,
        (f.name = 'My Folder - ' || u.username) AS name_matches_default
    FROM auth.users u
    JOIN storage.folders f
      ON f.user_id   = u.id
     AND f.parent_id IS NULL
     AND NOT f.is_trashed
    WHERE NOT u.is_external
)
SELECT
    user_id,
    username,
    quota,
    wrapper_id,
    wrapper_name,
    -- Rank candidates per user: name-matched root wins; otherwise oldest
    -- by created_at then by id (stable, deterministic). ROW_NUMBER() = 1
    -- becomes the default drive for that user.
    (ROW_NUMBER() OVER (
        PARTITION BY user_id
        ORDER BY name_matches_default DESC,
                 created_at ASC,
                 wrapper_id ASC
    ) = 1) AS is_default,
    gen_random_uuid() AS new_drive_id
FROM root_folders;


-- ── 1b. Log which users got an auto-picked default (no name match) ────────
-- Operational nicety: if a user's default came from oldest-root fallback
-- rather than the canonical `My Folder - <username>`, surface it so an
-- operator can DM the user and confirm the migration picked the right
-- root. Not a failure — just visibility.

DO $BODY$
DECLARE
    auto_picked BIGINT;
BEGIN
    SELECT count(*) INTO auto_picked
    FROM _drive_plan p
    WHERE p.is_default
      AND p.wrapper_name <> 'My Folder - ' || p.username;

    IF auto_picked > 0 THEN
        RAISE NOTICE
            'D0 backfill: % user(s) had no `My Folder - <username>` root; '
            'the oldest sibling root was auto-picked as their default '
            'Personal drive. Inspect with: SELECT user_id, username, '
            'wrapper_name FROM _drive_plan WHERE is_default AND '
            'wrapper_name <> ''My Folder - '' || username; '
            '(temp table only exists during the migration transaction.)',
            auto_picked;
    END IF;
END $BODY$;


-- ── 2. Insert the drive rows (metadata only — no `name` column) ───────────
-- Drives are pure metadata under the new design (docs/plan/drive.md §3).
-- The display name lives on the root folder; the wrapper is renamed in
-- step 4b for default drives and kept as-is for secondaries.

INSERT INTO storage.drives
    (id, kind, default_for_user, quota_bytes)
SELECT
    p.new_drive_id,
    'personal',
    CASE WHEN p.is_default THEN p.user_id ELSE NULL END,
    p.quota
FROM _drive_plan p;


-- ── 3. Insert one owner role_grants row per drive ─────────────────────────
-- Each user is the sole owner of every drive their wrappers produced.
-- The lifecycle hook (D0-9) will do the same for users created post-D0.

INSERT INTO storage.role_grants
    (subject_type, subject_id, resource_type, resource_id, role, granted_by)
SELECT 'user', p.user_id, 'drive', p.new_drive_id, 'owner', p.user_id
FROM _drive_plan p;


-- ── 4. Adopt the wrapper as the drive's root folder ───────────────────────
-- 4a. Stamp drive_id on each wrapper so the cascade in §5 can walk the
--     ltree subtree without a separate index.
-- 4b. Rename the default-drive wrapper from `My Folder - <username>` to
--     `Personal` (the canonical default name; renameable via the folder
--     API later). The BEFORE-UPDATE folder path trigger fires on the
--     rename and the AFTER-UPDATE cascade trigger rewrites every
--     descendant `path` / `lpath` automatically — no per-row UPDATE here.
-- 4c. Wire drives.root_folder_id to the wrapper. This is the adoption
--     step: the wrapper row IS the drive's root folder after M2 (no
--     wrapper-deletion, no descendant promotion).

UPDATE storage.folders f
   SET drive_id = p.new_drive_id,
       name     = CASE WHEN p.is_default THEN 'Personal' ELSE f.name END
  FROM _drive_plan p
 WHERE f.id = p.wrapper_id;

UPDATE storage.drives d
   SET root_folder_id = p.wrapper_id
  FROM _drive_plan p
 WHERE d.id = p.new_drive_id;


-- ── 5. Cascade drive_id down the folder tree ──────────────────────────────
-- For every folder descended from a wrapper, set drive_id to that
-- wrapper's. Uses the existing GiST index `idx_folders_lpath` for the
-- @> (ancestor-of) lookup. Trashed descendants get a drive_id too —
-- soft-deleted folders need a drive_id once M3 makes the column NOT NULL.

UPDATE storage.folders sub
   SET drive_id = wrapper.drive_id
  FROM storage.folders wrapper
 WHERE wrapper.id IN (SELECT wrapper_id FROM _drive_plan)
   AND sub.lpath <@ wrapper.lpath
   AND sub.id  != wrapper.id
   AND sub.drive_id IS NULL;


-- ── 6. Cascade drive_id to files (via their folder) ───────────────────────
-- Files inherit drive_id from their containing folder. A NULL folder_id
-- file is an orphan — left with NULL drive_id here; M3's NOT NULL
-- constraint will refuse the migration if any such orphans remain,
-- which is the right outcome (forces operator inspection).

UPDATE storage.files fi
   SET drive_id = fo.drive_id
  FROM storage.folders fo
 WHERE fi.folder_id = fo.id
   AND fi.drive_id IS NULL
   AND fo.drive_id IS NOT NULL;


-- ── 7. Provenance backfill ────────────────────────────────────────────────
-- Every pre-Drive row carries authentic provenance from day one: created_by
-- and updated_by both default to the user_id that we know created the
-- resource (that's exactly what user_id meant pre-D0). New writes during
-- the dual-write window populate both columns explicitly.

UPDATE storage.folders
   SET created_by = user_id,
       updated_by = user_id
 WHERE created_by IS NULL;

UPDATE storage.files
   SET created_by = user_id,
       updated_by = user_id
 WHERE created_by IS NULL;


-- ── 7b. Drop the planning temp table ──────────────────────────────────────
-- Explicit drop since we removed `ON COMMIT DROP` above. Idempotent
-- (`IF EXISTS`) so a partial re-run during development doesn't error.

DROP TABLE IF EXISTS _drive_plan;


-- ── 8. Post-flight consistency check ──────────────────────────────────────
-- The checks here REFUSE to commit if any invariant is violated, so a
-- successful migration is a verifiable migration.
--
--   8a. Every internal user with a root folder has exactly one
--       default drive.
--   8b. Every drive has at least one owner role_grants row.
--   8c. No NULL drive_id remains on a folder/file row whose owner is
--       a non-external user with a root folder (i.e. every row that
--       belongs to a drive must now declare which one).
--
-- M3 turns drive_id NOT NULL; the check below is a stricter pre-flight
-- so the failure mode is "migration refuses" rather than "M3 errors
-- with a NOT NULL violation halfway through".

DO $BODY$
DECLARE
    missing_default BIGINT;
    grantless_drives BIGINT;
    null_folder_drive_id BIGINT;
    null_file_drive_id BIGINT;
    rootless_drives BIGINT;
BEGIN
    SELECT count(*) INTO missing_default
    FROM auth.users u
    WHERE NOT u.is_external
      AND EXISTS (
          SELECT 1 FROM storage.folders f
           WHERE f.user_id = u.id AND f.parent_id IS NULL AND NOT f.is_trashed
      )
      AND NOT EXISTS (
          SELECT 1 FROM storage.drives d
           WHERE d.default_for_user = u.id
      );
    IF missing_default > 0 THEN
        RAISE EXCEPTION
            'D0 backfill consistency check failed: % internal user(s) with a '
            'root folder have no default Personal drive. Investigate before '
            'declaring the migration successful.',
            missing_default;
    END IF;

    SELECT count(*) INTO grantless_drives
    FROM storage.drives d
    WHERE NOT EXISTS (
        SELECT 1 FROM storage.role_grants g
         WHERE g.resource_type = 'drive'
           AND g.resource_id   = d.id
           AND g.role          = 'owner'
    );
    IF grantless_drives > 0 THEN
        RAISE EXCEPTION
            'D0 backfill consistency check failed: % drive(s) have no owner '
            'role_grants row. Investigate before declaring the migration '
            'successful.',
            grantless_drives;
    END IF;

    -- Root-folder adoption invariant (docs/plan/drive.md §3): every
    -- drive must point at a real folder row whose drive_id closes the
    -- cycle. The column is NULLable at the type level so the atomic
    -- CTE can write it mid-statement; this check enforces the data
    -- invariant after the migration.
    SELECT count(*) INTO rootless_drives
    FROM storage.drives d
    WHERE d.root_folder_id IS NULL
       OR NOT EXISTS (
           SELECT 1 FROM storage.folders f
            WHERE f.id        = d.root_folder_id
              AND f.drive_id  = d.id
              AND f.parent_id IS NULL
       );
    IF rootless_drives > 0 THEN
        RAISE EXCEPTION
            'D0 backfill consistency check failed: % drive(s) have no '
            'valid root_folder_id (NULL, or pointing at a folder that '
            'isn''t a root in this drive). Investigate before declaring '
            'the migration successful.',
            rootless_drives;
    END IF;

    SELECT count(*) INTO null_folder_drive_id
    FROM storage.folders f
    WHERE f.drive_id IS NULL
      AND EXISTS (
          SELECT 1 FROM auth.users u
           WHERE u.id = f.user_id AND NOT u.is_external
      );
    IF null_folder_drive_id > 0 THEN
        RAISE EXCEPTION
            'D0 backfill consistency check failed: % folder(s) belonging to '
            'an internal user still have NULL drive_id. M3 will refuse to '
            'add NOT NULL until these are resolved.',
            null_folder_drive_id;
    END IF;

    SELECT count(*) INTO null_file_drive_id
    FROM storage.files fi
    WHERE fi.drive_id IS NULL
      AND EXISTS (
          SELECT 1 FROM auth.users u
           WHERE u.id = fi.user_id AND NOT u.is_external
      );
    IF null_file_drive_id > 0 THEN
        RAISE EXCEPTION
            'D0 backfill consistency check failed: % file(s) belonging to an '
            'internal user still have NULL drive_id (likely orphans — '
            'folder_id pointing at a missing folder). Inspect with: SELECT '
            'fi.id, fi.user_id, fi.folder_id FROM storage.files fi JOIN '
            'auth.users u ON u.id = fi.user_id WHERE fi.drive_id IS NULL '
            'AND NOT u.is_external;',
            null_file_drive_id;
    END IF;
END $BODY$;
