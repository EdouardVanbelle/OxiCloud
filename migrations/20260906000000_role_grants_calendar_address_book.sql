-- ─────────────────────────────────────────────────────────────────────────
-- Round 3 — admit 'calendar' and 'address_book' into
-- `storage.role_grants.resource_type`.
--
-- Companion to the domain unblock in
-- `src/domain/services/authorization.rs` (Round 3 Phase 1). The
-- `Resource::Calendar(Uuid)` and `Resource::AddressBook(Uuid)`
-- variants can't be inserted into `role_grants` until the CHECK
-- constraint on `resource_type` permits their string discriminators.
--
-- CalDAV and CardDAV surfaces have historically enforced access via
-- dedicated per-domain share tables (`caldav.calendar_shares`,
-- `carddav.address_book_shares`) and bespoke `check_calendar_access`
-- / `check_address_book_access` helpers. Round 3 folds both into the
-- unified ReBAC engine so:
--
--   * A single ACL source of truth (`storage.role_grants`) covers
--     every OxiCloud resource type — files, folders, drives,
--     calendars, address books.
--   * Group subjects become a free feature on calendar/book shares
--     (falls out of `role_grants.subject_type='group'`).
--   * The `authz.require` audit line ("👮🏻‍♂️ perms: ⛔ …") fires on
--     denial with no per-domain retrofit.
--
-- Migration of existing rows from `caldav.calendar_shares` and
-- `carddav.address_book_shares` into `role_grants` happens in the
-- next migration (Phase 2). The legacy tables stay in place through
-- this PR for rollback safety; they get dropped one release later.

-- `resource_type` is a TEXT column with a CHECK constraint (not a PG
-- enum), so extending it is a DROP / ADD pair — no `ALTER TYPE` /
-- non-transactional migration issues.

ALTER TABLE storage.role_grants
    DROP CONSTRAINT IF EXISTS role_grants_resource_type_check;

ALTER TABLE storage.role_grants
    ADD CONSTRAINT role_grants_resource_type_check
    CHECK (resource_type IN ('folder', 'file', 'drive', 'calendar', 'address_book'));

-- Post-flight: introspect the live constraint definition and prove
-- both new values appear. Cheap read-only check with no INSERT.
DO $BODY$
DECLARE
    defn TEXT;
BEGIN
    SELECT pg_get_constraintdef(c.oid) INTO defn
      FROM pg_constraint c
      JOIN pg_class      t ON t.oid = c.conrelid
      JOIN pg_namespace  n ON n.oid = t.relnamespace
     WHERE n.nspname = 'storage'
       AND t.relname = 'role_grants'
       AND c.conname = 'role_grants_resource_type_check';

    IF defn IS NULL THEN
        RAISE EXCEPTION
            'role_grants_resource_type_check not found on storage.role_grants';
    END IF;
    IF position('calendar' IN defn) = 0 THEN
        RAISE EXCEPTION
            'CHECK constraint does not admit ''calendar'': %', defn;
    END IF;
    IF position('address_book' IN defn) = 0 THEN
        RAISE EXCEPTION
            'CHECK constraint does not admit ''address_book'': %', defn;
    END IF;
END;
$BODY$;
