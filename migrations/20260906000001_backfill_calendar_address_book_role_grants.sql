-- ─────────────────────────────────────────────────────────────────────────
-- Round 3 Phase 2 — backfill role_grants from the legacy per-domain
-- share tables.
--
-- Companion to `20260906000000_role_grants_calendar_address_book.sql`
-- (Phase 1: CHECK constraint extension). This migration seeds the
-- unified `storage.role_grants` table with:
--
--   1. Owner grants for every existing calendar and address book —
--      replaces the implicit "owner via `caldav.calendars.owner_id`"
--      short-circuit that the bespoke `check_calendar_access`
--      helper used.
--   2. Non-owner grants translated from `caldav.calendar_shares` and
--      `carddav.address_book_shares` — the existing "shared with me"
--      relationships continue working after Phase 3's service
--      rewrite starts reading grants from `role_grants` only.
--
-- The legacy share tables stay in place through this PR for
-- rollback safety. They get dropped in a follow-up migration one
-- release later, once the new engine path bakes.
--
-- Idempotent: every INSERT uses `ON CONFLICT DO NOTHING` on the
-- `(subject_type, subject_id, resource_type, resource_id)` unique
-- key so a re-run (or a duplicate row in the legacy table where
-- someone shared with themselves) is a no-op.

-- ── 1. Owner grants for calendars ───────────────────────────────────────
--
-- One row per calendar in `caldav.calendars`. `granted_by = owner_id`
-- is the self-seeded creation event — the calendar's owner brought
-- themselves into existence as its owner, matching the pattern used
-- by the drive lifecycle hook for personal drives.
INSERT INTO storage.role_grants
    (subject_type, subject_id, resource_type, resource_id, role, granted_by)
SELECT 'user', c.owner_id, 'calendar', c.id, 'owner'::storage.grant_role, c.owner_id
  FROM caldav.calendars c
ON CONFLICT (subject_type, subject_id, resource_type, resource_id)
    DO NOTHING;

-- ── 2. Owner grants for address books ───────────────────────────────────
INSERT INTO storage.role_grants
    (subject_type, subject_id, resource_type, resource_id, role, granted_by)
SELECT 'user', a.owner_id, 'address_book', a.id, 'owner'::storage.grant_role, a.owner_id
  FROM carddav.address_books a
ON CONFLICT (subject_type, subject_id, resource_type, resource_id)
    DO NOTHING;

-- ── 3. Non-owner grants from calendar_shares ────────────────────────────
--
-- `caldav.calendar_shares.access_level` is a VARCHAR(10) with values
-- `'read'`, `'write'`, or `'owner'`. Map:
--   - `'read'`  → `viewer`   (bundle: Read only)
--   - `'write'` → `editor`   (bundle: Read + Update)
--   - `'owner'` → `owner`    (bundle: everything, including Share/Manage)
-- Anything else (defensive) falls through to `viewer` — losing
-- permission is safer than silently gaining permission if a stray
-- value slipped past the pre-D0 CHECK.
--
-- `granted_by` = calendar owner, since the legacy share table didn't
-- track the granter. Best available signal — the owner is the only
-- principal who could have created the share via the legacy code path.
INSERT INTO storage.role_grants
    (subject_type, subject_id, resource_type, resource_id, role, granted_by)
SELECT
    'user',
    s.user_id,
    'calendar',
    s.calendar_id,
    (CASE s.access_level
        WHEN 'write' THEN 'editor'
        WHEN 'owner' THEN 'owner'
        ELSE 'viewer'
     END)::storage.grant_role,
    c.owner_id
  FROM caldav.calendar_shares s
  JOIN caldav.calendars       c ON c.id = s.calendar_id
 WHERE s.user_id <> c.owner_id   -- skip self-shares (owner grant already covers them)
ON CONFLICT (subject_type, subject_id, resource_type, resource_id)
    DO NOTHING;

-- ── 4. Non-owner grants from address_book_shares ────────────────────────
--
-- `carddav.address_book_shares.can_write` is a BOOLEAN. Map:
--   - `false` → `viewer`
--   - `true`  → `editor`
INSERT INTO storage.role_grants
    (subject_type, subject_id, resource_type, resource_id, role, granted_by)
SELECT
    'user',
    s.user_id,
    'address_book',
    s.address_book_id,
    (CASE WHEN s.can_write THEN 'editor' ELSE 'viewer' END)::storage.grant_role,
    a.owner_id
  FROM carddav.address_book_shares s
  JOIN carddav.address_books       a ON a.id = s.address_book_id
 WHERE s.user_id <> a.owner_id
ON CONFLICT (subject_type, subject_id, resource_type, resource_id)
    DO NOTHING;

-- ── 5. Post-flight sanity ───────────────────────────────────────────────
--
-- Every calendar / address book must now have an owner role_grant.
-- If any row is missing one, the Phase 3 service rewrite would
-- lock owners out of their own resources — refuse to leave the
-- migration in that state.
DO $BODY$
DECLARE
    missing_cal_owners BIGINT;
    missing_ab_owners  BIGINT;
BEGIN
    SELECT COUNT(*) INTO missing_cal_owners
      FROM caldav.calendars c
     WHERE NOT EXISTS (
         SELECT 1 FROM storage.role_grants g
          WHERE g.subject_type  = 'user'
            AND g.subject_id    = c.owner_id
            AND g.resource_type = 'calendar'
            AND g.resource_id   = c.id
            AND g.role          = 'owner'::storage.grant_role
     );

    SELECT COUNT(*) INTO missing_ab_owners
      FROM carddav.address_books a
     WHERE NOT EXISTS (
         SELECT 1 FROM storage.role_grants g
          WHERE g.subject_type  = 'user'
            AND g.subject_id    = a.owner_id
            AND g.resource_type = 'address_book'
            AND g.resource_id   = a.id
            AND g.role          = 'owner'::storage.grant_role
     );

    IF missing_cal_owners > 0 THEN
        RAISE EXCEPTION
            'Round 3 backfill left % calendars without an Owner role_grant',
            missing_cal_owners;
    END IF;
    IF missing_ab_owners > 0 THEN
        RAISE EXCEPTION
            'Round 3 backfill left % address books without an Owner role_grant',
            missing_ab_owners;
    END IF;
END;
$BODY$;
