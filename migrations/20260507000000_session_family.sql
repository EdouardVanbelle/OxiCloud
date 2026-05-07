-- Add token family tracking to sessions.
--
-- family_id groups all refresh tokens issued from the same original login.
-- When a rotation detects a revoked token being replayed (possible theft),
-- the entire family is invalidated — forcing re-authentication on all devices
-- that shared that login event.
--
-- Existing sessions are seeded with family_id = id (each is its own family).

ALTER TABLE auth.sessions
    ADD COLUMN family_id UUID;

UPDATE auth.sessions
    SET family_id = id;

ALTER TABLE auth.sessions
    ALTER COLUMN family_id SET NOT NULL;

CREATE INDEX idx_sessions_family_id ON auth.sessions(family_id);
