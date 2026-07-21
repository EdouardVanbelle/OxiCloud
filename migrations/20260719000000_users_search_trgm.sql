-- Trigram indexes for the user search path (NC sharee autocomplete + admin
-- user search), which filters with a leading-wildcard `ILIKE '%q%'` that no
-- btree can serve — every keystroke was a full `auth.users` seq scan.
--
-- Mirrors the existing `gin_trgm_ops` indexes on contacts / files / folders
-- (pg_trgm is a hard startup requirement, see 20260307000000). Measured in
-- benches/ROUND12.md §1: 26-row sharee page over 3 000 users drops from
-- 2.37 ms (narrow read, seq scan) to 0.22 ms; the gap widens with user count.

CREATE INDEX IF NOT EXISTS idx_users_username_trgm
    ON auth.users USING gin (username gin_trgm_ops);

CREATE INDEX IF NOT EXISTS idx_users_email_trgm
    ON auth.users USING gin (email gin_trgm_ops);
