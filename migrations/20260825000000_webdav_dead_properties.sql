-- WebDAV dead properties storage (RFC 4918 §9.2).
-- Stores arbitrary user-defined XML properties set via PROPPATCH.
-- Keyed by (resource_path, user_id, namespace, local_name) — the
-- same property on different resources or for different users is
-- a distinct row.

CREATE TABLE IF NOT EXISTS storage.webdav_dead_properties (
    id           UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    resource_path TEXT       NOT NULL,
    user_id      UUID        NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    namespace    TEXT        NOT NULL DEFAULT '',
    local_name   TEXT        NOT NULL,
    value        TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (resource_path, user_id, namespace, local_name)
);

CREATE INDEX IF NOT EXISTS idx_webdav_dead_properties_path_user
    ON storage.webdav_dead_properties (resource_path, user_id);
