-- ─────────────────────────────────────────────────────────────────────────
-- Round 3 (Music) Phase 2 — backfill role_grants from the legacy
-- per-domain share table.
--
-- Companion to `20260910000000_role_grants_playlist.sql` (Phase 1:
-- CHECK constraint extension). This migration seeds
-- `storage.role_grants` with:
--
--   1. Owner grants for every existing playlist — replaces the
--      implicit "owner via `audio.playlists.owner_id`" short-circuit
--      that the bespoke `user_has_access` / `user_can_write` helpers
--      used.
--   2. Non-owner grants translated from `audio.playlist_shares` —
--      existing "shared with me" relationships keep working after the
--      Phase 3 service rewrite starts reading grants from
--      `role_grants` only.
--
-- The legacy `audio.playlist_shares` table stays in place through
-- this PR for rollback safety. It gets dropped in a follow-up
-- migration one release later, once the new engine path bakes.
--
-- Idempotent: every INSERT uses `ON CONFLICT DO NOTHING` on the
-- `(subject_type, subject_id, resource_type, resource_id)` unique
-- key so a re-run (or a duplicate row in the legacy table where
-- someone shared with themselves) is a no-op.

-- ── 1. Owner grants for playlists ───────────────────────────────────────
--
-- One row per playlist. `granted_by = owner_id` is the self-seeded
-- creation event — matches the pattern used by the calendar /
-- address-book backfill and by the drive lifecycle hook for personal
-- drives.
INSERT INTO storage.role_grants
    (subject_type, subject_id, resource_type, resource_id, role, granted_by)
SELECT 'user', p.owner_id, 'playlist', p.id, 'owner'::storage.grant_role, p.owner_id
  FROM audio.playlists p
ON CONFLICT (subject_type, subject_id, resource_type, resource_id)
    DO NOTHING;

-- ── 2. Non-owner grants from playlist_shares ────────────────────────────
--
-- `audio.playlist_shares.can_write` is a BOOLEAN. Map:
--   - `false` → `viewer` (bundle: Read only)
--   - `true`  → `editor` (bundle: Read + Update)
--
-- `granted_by` = playlist owner, since the legacy share table didn't
-- track the granter. Best available signal — the owner is the only
-- principal who could have created the share via the legacy code path.
INSERT INTO storage.role_grants
    (subject_type, subject_id, resource_type, resource_id, role, granted_by)
SELECT
    'user',
    s.user_id,
    'playlist',
    s.playlist_id,
    (CASE WHEN s.can_write THEN 'editor' ELSE 'viewer' END)::storage.grant_role,
    p.owner_id
  FROM audio.playlist_shares s
  JOIN audio.playlists       p ON p.id = s.playlist_id
 WHERE s.user_id <> p.owner_id   -- skip self-shares (owner grant already covers them)
ON CONFLICT (subject_type, subject_id, resource_type, resource_id)
    DO NOTHING;

-- ── 3. Post-flight sanity ───────────────────────────────────────────────
--
-- Every playlist must now have an owner role_grant. If any row is
-- missing one, the Phase 3 service rewrite would lock owners out of
-- their own resources — refuse to leave the migration in that state.
DO $BODY$
DECLARE
    missing_owners BIGINT;
BEGIN
    SELECT COUNT(*) INTO missing_owners
      FROM audio.playlists p
     WHERE NOT EXISTS (
         SELECT 1 FROM storage.role_grants g
          WHERE g.subject_type  = 'user'
            AND g.subject_id    = p.owner_id
            AND g.resource_type = 'playlist'
            AND g.resource_id   = p.id
            AND g.role          = 'owner'::storage.grant_role
     );

    IF missing_owners > 0 THEN
        RAISE EXCEPTION
            'Round 3 (Music) backfill left % playlists without an Owner role_grant',
            missing_owners;
    END IF;
END;
$BODY$;
