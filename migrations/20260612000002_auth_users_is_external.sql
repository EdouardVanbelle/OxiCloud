-- ════════════════════════════════════════════════════════════════════════════
-- Add `is_external` flag to auth.users
-- ════════════════════════════════════════════════════════════════════════════
-- Distinguishes storage-owning internal users (default — `is_external = FALSE`)
-- from grant-only external users (`is_external = TRUE`). External users are
-- recipients of share-grants who do not have a home folder, do not consume
-- storage quota, and authenticate via magic-link / OIDC / OCM (future).
--
-- This migration is additive — all existing rows default to internal. The
-- backend code that consumes the flag lands in PR 3 (HomeFolderLifecycleHook
-- short-circuits when `is_external = TRUE`) and the magic-link flow ships
-- later still.
--
-- Subject::External(uuid) in the domain becomes redundant after this — every
-- principal is now Subject::User(uuid) with `is_external` as a property, not
-- a variant. The cleanup happens in a small follow-up after the flag has
-- been observed in production.

ALTER TABLE auth.users
    ADD COLUMN IF NOT EXISTS is_external BOOLEAN NOT NULL DEFAULT FALSE;

-- Partial index for the two query patterns that scan by this flag:
--   - admin "list external users" surface
--   - GDPR / cleanup sweepers that filter on `is_external = TRUE` and
--     last_login_at older than a threshold.
-- Internal-user queries don't go through this index — they ignore the
-- column entirely.
CREATE INDEX IF NOT EXISTS idx_users_is_external_login
    ON auth.users (is_external, last_login_at)
    WHERE is_external = TRUE;

-- Schema-level safety net: external users must not be charged for storage.
-- HomeFolderLifecycleHook (PR 3) short-circuits before creating a home
-- folder for them, so storage_used_bytes should stay at 0. This CHECK
-- catches any code path that bypasses the hook and tries to attribute
-- storage to an external user.
ALTER TABLE auth.users
    ADD CONSTRAINT users_external_no_storage
        CHECK (NOT is_external OR storage_used_bytes = 0);

-- Forbid external + admin combination. External users are grant-only
-- recipients authenticating via federated identity (magic-link, OIDC,
-- future OCM). Granting them the admin role would let a federated
-- principal manage the local instance — undesirable. To promote an
-- external user to admin: first flip is_external to FALSE (converting
-- them to internal), then update the role separately. The two steps
-- are intentional friction.
ALTER TABLE auth.users
    ADD CONSTRAINT users_external_not_admin
        CHECK (NOT (is_external AND role = 'admin'));

COMMENT ON COLUMN auth.users.is_external IS
    'TRUE for grant-only external recipients (magic-link, OIDC-only, OCM federated). FALSE for storage-owning internal users. Set at creation; can be flipped to FALSE by admin to convert external → internal (next login provisions the home folder via HomeFolderLifecycleHook).';
