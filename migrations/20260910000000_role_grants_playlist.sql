-- ─────────────────────────────────────────────────────────────────────────
-- Round 3 (Music) — admit 'playlist' into
-- `storage.role_grants.resource_type`.
--
-- Companion to the domain unblock in
-- `src/domain/services/authorization.rs`: uncomments
-- `Resource::Playlist(Uuid)` and its `type_str` / `id` / `from_parts`
-- arms. Nothing can insert `('playlist', …)` into `role_grants` until
-- the CHECK constraint permits the discriminator.
--
-- The music surface historically enforced access via a dedicated
-- `audio.playlist_shares` table and bespoke
-- `MusicStorageAdapter::{user_has_access, user_can_write}` helpers.
-- Round 3 folds them into the unified ReBAC engine, giving playlists
-- the same treatment already applied to calendars and address books:
--
--   * A single ACL source of truth (`storage.role_grants`) covers
--     every OxiCloud resource type — files, folders, drives,
--     calendars, address books, playlists.
--   * Group subjects become a free feature on playlist shares.
--   * The `authz.require` audit line ("👮🏻‍♂️ perms: ⛔ …") fires on
--     denial with no per-domain retrofit.
--
-- Owner + share backfill from `audio.playlist_shares` happens in the
-- companion migration. The legacy table stays in place through this
-- PR for rollback safety; a follow-up migration one release later
-- drops it.

-- `resource_type` is a TEXT column with a CHECK constraint (not a PG
-- enum), so extending it is a DROP / ADD pair — no `ALTER TYPE` /
-- non-transactional migration issues.

ALTER TABLE storage.role_grants
    DROP CONSTRAINT IF EXISTS role_grants_resource_type_check;

ALTER TABLE storage.role_grants
    ADD CONSTRAINT role_grants_resource_type_check
    CHECK (resource_type IN ('folder', 'file', 'drive', 'calendar', 'address_book', 'playlist'));

-- Post-flight: introspect the live constraint definition and prove
-- 'playlist' appears. Cheap read-only check with no INSERT.
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
    IF position('playlist' IN defn) = 0 THEN
        RAISE EXCEPTION
            'CHECK constraint does not admit ''playlist'': %', defn;
    END IF;
END;
$BODY$;
