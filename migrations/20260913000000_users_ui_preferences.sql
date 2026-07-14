-- Add opaque UI preferences bag to auth.users.
--
-- Purpose. Cross-device persistence of pure UI toggles (hide dotfiles,
-- view mode, group-by choice, sidebar collapse, …). The server NEVER
-- inspects the contents — this column exists solely so that the SPA can
-- fetch its own settings from `GET /api/auth/me` on a fresh browser and
-- write them back via `PATCH /api/auth/me/profile`.
--
-- Design rule. Preferences that ONLY affect the UI live here.
-- Preferences the SERVER reads (locale for magic-link templates,
-- notify_on_share for the notification pipeline, role for authz) stay as
-- typed columns. When a UI-only preference graduates to server-relevant,
-- promote it to a column and drop the JSON key in a follow-up migration.
--
-- Merge semantics. `PATCH /api/auth/me/profile` performs a SHALLOW
-- merge via `ui_preferences || $1::jsonb` in `pg_user_repository.rs`,
-- optionally stripping nulls (frontend convention: sending `{key: null}`
-- clears the key). Full replacement isn't offered — every operation is
-- additive so a partial write from Device A doesn't wipe prefs set on
-- Device B.
--
-- Size cap. Enforced via CHECK constraint: 16 KiB compressed JSONB is
-- generous for realistic UI prefs and prevents the endpoint from being
-- used as a scratch key-value store. `pg_column_size(ui_preferences)`
-- returns the on-disk byte size which is what actually consumes rows.
ALTER TABLE auth.users
    ADD COLUMN ui_preferences JSONB NOT NULL DEFAULT '{}'::jsonb;

-- Object shape only — arrays / scalars / null are rejected. The merge
-- semantics assume an object; a scalar in this column would break the
-- shallow-merge SQL. Cheap check (single jsonb_typeof call).
ALTER TABLE auth.users
    ADD CONSTRAINT users_ui_preferences_is_object
        CHECK (jsonb_typeof(ui_preferences) = 'object');

-- Size guard — 16 KiB is 16384 bytes. Realistic UI-toggle payloads are
-- well under 1 KiB; the cap exists to fence off misuse, not to be
-- tight.
ALTER TABLE auth.users
    ADD CONSTRAINT users_ui_preferences_size_cap
        CHECK (pg_column_size(ui_preferences) <= 16384);
