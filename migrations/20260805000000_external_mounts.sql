-- ════════════════════════════════════════════════════════════════════════════
-- External file mounts
-- ════════════════════════════════════════════════════════════════════════════
-- Admin-configured mounts that expose an external backend (raw host filesystem
-- in v1; SFTP/WebDAV/… as future provider `kind`s) as a folder inside a user's
-- drive. The mount ROOT is a normal `storage.folders` row (so it participates in
-- ltree, drive scoping, and ACL grants like any folder); everything BELOW it is
-- virtual — read live from the backend, never stored in `storage.files`.
--
-- This table maps a mount-root folder to its backend. `kind` selects the
-- provider implementation; `config` carries provider-specific connection data
-- (a stable JSONB bag so adding a new provider kind needs no schema change):
--   * local_fs        → {"path": "/mnt/share"}
--   * sftp  (future)  → {"host": "...", "port": 22, "user": "...", "base_path": "..."}
--
-- Mount contents are deliberately a LIMITED, SEPARATE storage type: no blob
-- dedup, no per-file sharing/favorites/trash/search in v1 (deletes are real,
-- permanent backend deletes). See docs / plan for the forward path.
-- ════════════════════════════════════════════════════════════════════════════

CREATE TABLE IF NOT EXISTS storage.external_mounts (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- The mount-root folder. Deleting that folder row removes the mount mapping.
    mount_folder_id UUID NOT NULL UNIQUE
                    REFERENCES storage.folders(id) ON DELETE CASCADE,
    -- Provider discriminator (selects the ExternalMountProvider implementation).
    kind            TEXT NOT NULL DEFAULT 'local_fs',
    -- Provider-specific connection config; shape depends on `kind`.
    config          JSONB NOT NULL DEFAULT '{}'::jsonb,
    -- Display name (mirrors the folder name; kept for admin listings).
    name            TEXT NOT NULL,
    -- Admin/user who owns the mount configuration.
    owner_id        UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    -- When true, the provider refuses all mutations (browse/download only).
    read_only       BOOLEAN NOT NULL DEFAULT FALSE,
    -- Visibility policy. 'owner' = only the owner's drive sees it (v1).
    -- Reserved for future 'shared' semantics.
    visibility      TEXT NOT NULL DEFAULT 'owner',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_external_mounts_folder
    ON storage.external_mounts(mount_folder_id);

COMMENT ON TABLE storage.external_mounts IS
    'Admin-configured external backends (local_fs/sftp/…) surfaced as a mount-root folder; contents are virtual and read live from the provider.';
