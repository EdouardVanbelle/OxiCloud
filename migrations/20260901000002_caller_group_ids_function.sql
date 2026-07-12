-- ════════════════════════════════════════════════════════════════════════════
-- PR-B — storage.caller_group_ids: recursive group-membership expansion in SQL
-- ════════════════════════════════════════════════════════════════════════════
-- Every listing surface that scopes by "drives the caller can Read" (Photos,
-- Places, GET /api/drives, Trash, Search, root-folder listing) needs the
-- caller's *effective subject set* = caller_id ∪ every group they belong to
-- transitively.
--
-- Pre-Option-A the Rust-side `PgAclEngine::expand_subject_for_listing` did
-- the walk once (via `WITH RECURSIVE` in `subject_group_pg_repository.rs::
-- groups_for_user`) and cached the result in a Moka table with 30-second
-- TTL; every listing handler then passed the two parallel arrays
-- `subject_types` + `subject_ids` into the SQL. That leaked the expansion
-- ceremony into every caller (7+ sites).
--
-- Option A pushes the walk into a `STABLE` SQL function so each listing
-- query embeds the expansion inline:
--
--   WHERE (g.subject_type = 'user'  AND g.subject_id = $caller)
--      OR (g.subject_type = 'group' AND g.subject_id IN
--              (SELECT storage.caller_group_ids($caller)))
--
-- Callers pass a bare `caller_id: Uuid` — no more expand-then-bind
-- ceremony. Postgres re-runs the walk per listing (~1-3 ms against the
-- indexed `auth.subject_group_members` table); we lose the Moka cache
-- benefit but gain a single audit trail for "how does group access
-- cascade" (this function) and drop ~15 lines of Rust glue per listing.
--
-- Cycle safety: `subject_group_pg_repository.rs::add_member` enforces
-- an INSERT-time cycle check via `WITH RECURSIVE descendants`, so the
-- membership DAG is guaranteed acyclic. Depth is capped at
-- MAX_GROUP_DEPTH by the same INSERT path. The recursion below always
-- terminates.
--
-- `STABLE`: the function reads DB state but never modifies it, and the
-- result is deterministic within a transaction. Postgres can memoise
-- calls within a single query plan (e.g. multiple references in the
-- same SELECT) and inline the CTE into the surrounding query where
-- beneficial. Marking it `VOLATILE` would forbid both optimisations.
--
-- `LEAKPROOF` is deliberately NOT set: the function reads a private
-- auth table, so it must not be pushed below a security barrier.
--
-- `SECURITY INVOKER` (the default) — runs with the calling role's
-- permissions, so RLS on `auth.subject_group_members` (if ever added)
-- applies consistently.

CREATE OR REPLACE FUNCTION storage.caller_group_ids(caller UUID)
RETURNS SETOF UUID
LANGUAGE sql
STABLE
AS $$
    WITH RECURSIVE user_groups AS (
        -- Direct memberships: groups the caller is listed in as a user.
        SELECT group_id
          FROM auth.subject_group_members
         WHERE member_user_id = caller

        UNION

        -- Transitive memberships: groups that contain a group the caller
        -- already belongs to. Repeats until no new rows are produced.
        SELECT m.group_id
          FROM auth.subject_group_members m
          JOIN user_groups ug ON m.member_group_id = ug.group_id
    )
    SELECT group_id FROM user_groups;
$$;

-- Backing indexes used by the recursion. Already present from
-- 20260307000000_initial_schema.sql on
-- `auth.subject_group_members (member_user_id)` and
-- `auth.subject_group_members (member_group_id)` — no additional
-- indexes needed here.
