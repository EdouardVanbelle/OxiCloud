//! PostgreSQL-backed implementation of `AuthorizationEngine`.
//!
//! Stores grants in `storage.access_grants` (see migration
//! `20260520000000_rebac_access_grants.sql`). Cascading is resolved at check
//! time via PostgreSQL `ltree` `@>` (ancestor-of) on `storage.folders.lpath`,
//! using the existing GiST index for O(log N) traversal.
//!
//! Owner is implicit — `storage.folders.user_id` / `storage.files.user_id`
//! are checked first via dedicated helpers; if the caller is the owner, no
//! SQL against `access_grants` happens.
//!
//! ## Lifecycle cleanup
//!
//! In v1, cleanup of grant rows when a resource or subject is permanently
//! deleted is enforced by **DB triggers** (`trg_cleanup_grants_*` in the
//! migration). The application layer does not call `revoke_all_for_*`
//! explicitly today — the triggers are the canonical path because they
//! also catch bulk SQL maintenance, admin scripts, and any code path that
//! bypasses the service layer.
//!
//! The `revoke_all_for_resource` / `revoke_all_for_subject` methods exist
//! on the trait for future use cases:
//! - **Caching** (planned) — a `CachedAuthorizationEngine` decorator needs
//!   to see the invalidation event at the engine boundary, not just at the
//!   SQL level. When caching lands, services will start calling these
//!   methods explicitly before/around delete operations.
//! - **Alternate engines** (OpenFGA, future) — engines that don't share a
//!   DB transaction with the resource table need an explicit signal to
//!   delete their tuples.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use uuid::Uuid;

use moka::future::Cache;
use sqlx::PgPool;

use crate::application::ports::authorization_ports::AuthorizationEngine;
use crate::common::errors::DomainError;
use crate::domain::entities::subject_group::INTERNAL_GROUP_ID;
use crate::domain::repositories::subject_group_repository::SubjectGroupRepository;
use crate::domain::services::authorization::{
    Grant, GrantCursor, IncomingGrantSummary, OutgoingGrantEntry, OutgoingResourceSummary,
    Permission, Resource, ResourceKind, Subject,
};
use crate::infrastructure::repositories::pg::SubjectGroupPgRepository;
use crate::infrastructure::repositories::pg::file_blob_read_repository::FileBlobReadRepository;
use crate::infrastructure::repositories::pg::folder_db_repository::FolderDbRepository;

/// Per-call counters surfaced through `tracing::debug!` for performance
/// observability: cache hit-rate, SQL traffic, transitive expansion size.
///
/// Sub-microsecond cost when debug logging is off (one atomic write per
/// increment, no allocation, no formatting).
#[derive(Default)]
struct QueryCounters {
    cache_hit: AtomicU32,
    sql_queries: AtomicU32,
    expanded_groups: AtomicU32,
}

/// Defensive upper bound on the number of grant rows the *unbounded* list
/// methods (`list_incoming_grants`, `list_grants_on_resource`) will pull into
/// memory. These back management surfaces ("Manage sharing", "Shared with
/// me"), not the hot `require()` path, so a single resource or subject
/// realistically accumulates orders of magnitude fewer grants than this.
///
/// We fetch `MAX_GRANT_ROWS + 1` and *reject* when the cap is exceeded rather
/// than silently truncating: `apply_role` computes an add/remove diff from the
/// returned set, so a partial list would be acted on as if complete. Hitting
/// the cap signals pathological data and is surfaced to operators via audit.
const MAX_GRANT_ROWS: i64 = 10_000;

pub struct PgAclEngine {
    pool: Arc<PgPool>,
    folder_repo: Arc<FolderDbRepository>,
    file_repo: Arc<FileBlobReadRepository>,
    /// Group repository — `None` only in test stubs that don't exercise authz.
    group_repo: Option<Arc<SubjectGroupPgRepository>>,
    /// Memoise `user_id → transitive group set` for 30 s. Bounded to 50 000
    /// entries; eviction is LRU + TTL. Stale by up to TTL after a membership
    /// change — acceptable trade-off (see plan, "Cache TTL behaviour").
    user_groups_cache: Cache<Uuid, Arc<HashSet<Uuid>>>,
}

impl PgAclEngine {
    pub fn new(
        pool: Arc<PgPool>,
        folder_repo: Arc<FolderDbRepository>,
        file_repo: Arc<FileBlobReadRepository>,
        group_repo: Arc<SubjectGroupPgRepository>,
    ) -> Self {
        Self {
            pool,
            folder_repo,
            file_repo,
            group_repo: Some(group_repo),
            user_groups_cache: Cache::builder()
                .max_capacity(50_000)
                .time_to_live(Duration::from_secs(30))
                .build(),
        }
    }

    /// Creates a stub instance for tests that need to construct services
    /// without a real PostgreSQL pool. Connecting to the lazy pool will
    /// fail at runtime — only safe in tests that exercise types, not actual
    /// authz queries.
    #[cfg(test)]
    pub fn new_stub() -> Self {
        let pool = sqlx::pool::PoolOptions::<sqlx::Postgres>::new()
            .max_connections(1)
            .connect_lazy("postgres://invalid:5432/none")
            .unwrap();
        Self {
            pool: Arc::new(pool),
            folder_repo: Arc::new(FolderDbRepository::new_stub()),
            file_repo: Arc::new(FileBlobReadRepository::new_stub()),
            group_repo: None,
            user_groups_cache: Cache::builder()
                .max_capacity(1)
                .time_to_live(Duration::from_secs(1))
                .build(),
        }
    }

    /// Drop the cached transitive-group expansion for one user, forcing
    /// the next `expand_user(uid)` to walk the recursive CTE again.
    ///
    /// Called by [`AuthzCacheLifecycleHook`] on `on_user_logout` /
    /// `on_user_deleted` so a re-login (or a re-created account with the
    /// same id) doesn't observe stale memberships during the 30 s TTL
    /// window. Cheap — moka's `invalidate` is a single concurrent-map op.
    pub async fn invalidate_user_groups_cache(&self, user_id: Uuid) {
        self.user_groups_cache.invalidate(&user_id).await;
    }

    /// Expand a user subject into the set of subject UUIDs that should match
    /// in `access_grants`: the user's own UUID, every group the user is
    /// transitively a member of, and (for internal users only) the implicit
    /// `INTERNAL_GROUP_ID`.
    ///
    /// External users (`auth.users.is_external = TRUE`) do NOT belong to
    /// the Internal virtual group — they are grant-only recipients whose
    /// access is determined exclusively by explicit grants on their
    /// `user_id` or on subject groups they were explicitly added to.
    /// `SubjectGroupService::add_member` rejects externals, so the only
    /// path by which an external user reaches a resource is via a
    /// `subject_type='user'` grant.
    ///
    /// This is the **only** place transitive membership is walked. A future
    /// closure-table swap-in (Option 3 in the design doc) replaces just the
    /// `repo.groups_for_user` call below — every caller stays unchanged.
    async fn expand_user(
        &self,
        user_id: Uuid,
        counters: &QueryCounters,
    ) -> Result<Arc<HashSet<Uuid>>, DomainError> {
        if let Some(cached) = self.user_groups_cache.get(&user_id).await {
            counters.cache_hit.store(1, Ordering::Relaxed);
            counters
                .expanded_groups
                .store(cached.len() as u32, Ordering::Relaxed);
            return Ok(cached);
        }

        let mut set: HashSet<Uuid> = HashSet::new();
        set.insert(user_id);

        // Look up `is_external` for the caller — external users do not
        // belong to the Internal virtual group. Unknown user (no row) is
        // treated as external to fail closed: a deleted or bogus user_id
        // must not gain implicit Internal membership.
        counters.sql_queries.fetch_add(1, Ordering::Relaxed);
        let is_external: bool =
            sqlx::query_scalar("SELECT is_external FROM auth.users WHERE id = $1")
                .bind(user_id)
                .fetch_optional(self.pool.as_ref())
                .await
                .map_err(|e| {
                    DomainError::internal_error("PgAcl", format!("lookup is_external: {e}"))
                })?
                .unwrap_or(true);

        if !is_external {
            set.insert(INTERNAL_GROUP_ID);
        }

        if let Some(repo) = &self.group_repo {
            counters.sql_queries.fetch_add(1, Ordering::Relaxed);
            let direct = repo.groups_for_user(user_id).await.map_err(|e| {
                DomainError::internal_error("PgAcl", format!("groups_for_user: {e}"))
            })?;
            set.extend(direct);
        }

        counters
            .expanded_groups
            .store(set.len() as u32, Ordering::Relaxed);
        let arc = Arc::new(set);
        self.user_groups_cache.insert(user_id, arc.clone()).await;
        Ok(arc)
    }

    /// Expand a caller's `Subject` into the `(subject_types, subject_ids)`
    /// pair that should be matched in `storage.access_grants`. For User
    /// callers this is `(["user","group"], [uid, …transitive groups, INTERNAL])`;
    /// for any non-user subject (Token / External / Group as direct caller)
    /// it's a single-element pair with no cascade.
    ///
    /// Shared by `check_inner` (permission decision) and the
    /// `list_incoming_*` queries ("Shared with me") so that any folder/file
    /// the user can `read` via a group grant also appears in their incoming
    /// listing. Shares the `expand_user` Moka cache, so the listing call
    /// right after a permission check is a cache hit.
    async fn subject_match_set(
        &self,
        subject: Subject,
        counters: &QueryCounters,
    ) -> Result<(Vec<&'static str>, Vec<Uuid>), DomainError> {
        match subject {
            Subject::User(uid) => {
                let expanded = self.expand_user(uid, counters).await?;
                Ok((vec!["user", "group"], expanded.iter().copied().collect()))
            }
            _ => Ok((vec![subject.type_str()], vec![subject.id()])),
        }
    }

    /// Returns the owner UUID for any resource type.
    async fn owner_of(&self, resource: Resource) -> Result<Uuid, DomainError> {
        match resource {
            Resource::Folder(id) => self.folder_repo.get_folder_user_id(&id.to_string()).await,
            Resource::File(id) => self.file_repo.get_file_user_id(&id.to_string()).await,
        }
    }

    /// Cascading check for folders: is there a grant on any ancestor folder
    /// (including the target itself) for any of the given subject IDs and
    /// any of the given subject types?
    ///
    /// `subject_types` is `["user", "group"]` when the caller is a User
    /// (so we match both their own grants and their group-mediated grants),
    /// or a single-element slice for Token / External / Group-direct callers.
    /// `subject_ids` is the expanded set returned by `expand_user` (or a
    /// single-element vec for non-user callers).
    ///
    /// Uses the GiST index on `storage.folders.lpath` for O(log N) cascade.
    async fn folder_cascade_grant_exists(
        &self,
        subject_types: &[&str],
        subject_ids: &[Uuid],
        permission: Permission,
        folder_id: Uuid,
        counters: &QueryCounters,
    ) -> Result<bool, DomainError> {
        counters.sql_queries.fetch_add(1, Ordering::Relaxed);
        let exists: Option<i32> = sqlx::query_scalar(
            r#"
            SELECT 1
              FROM storage.access_grants g
              JOIN storage.folders gf ON gf.id = g.resource_id
             WHERE g.subject_type  = ANY($1)
               AND g.subject_id    = ANY($2)
               AND g.permission    = $3
               AND g.resource_type = 'folder'
               AND (g.expires_at IS NULL OR g.expires_at > NOW())
               AND gf.lpath @> (SELECT lpath FROM storage.folders WHERE id = $4)
             LIMIT 1
            "#,
        )
        .bind(subject_types)
        .bind(subject_ids)
        .bind(permission.as_str())
        .bind(folder_id)
        .fetch_optional(self.pool.as_ref())
        .await
        .map_err(|e| DomainError::internal_error("PgAcl", format!("folder cascade: {e}")))?;

        Ok(exists.is_some())
    }

    /// Cascading check for files: either a direct file grant OR a grant on
    /// any ancestor folder of the file's containing folder. See
    /// `folder_cascade_grant_exists` for the meaning of `subject_types` /
    /// `subject_ids`.
    async fn file_cascade_grant_exists(
        &self,
        subject_types: &[&str],
        subject_ids: &[Uuid],
        permission: Permission,
        file_id: Uuid,
        counters: &QueryCounters,
    ) -> Result<bool, DomainError> {
        counters.sql_queries.fetch_add(1, Ordering::Relaxed);
        let exists: Option<i32> = sqlx::query_scalar(
            r#"
            SELECT 1
              FROM (
                -- direct file grant
                SELECT 1
                  FROM storage.access_grants
                 WHERE subject_type = ANY($1)
                   AND subject_id   = ANY($2)
                   AND permission   = $3
                   AND resource_type = 'file' AND resource_id = $4
                   AND (expires_at IS NULL OR expires_at > NOW())
                UNION ALL
                -- cascading from any ancestor folder of the file's containing folder
                SELECT 1
                  FROM storage.access_grants g
                  JOIN storage.folders gf     ON gf.id = g.resource_id
                  JOIN storage.files target_f ON target_f.id = $4
                 WHERE g.subject_type  = ANY($1)
                   AND g.subject_id    = ANY($2)
                   AND g.permission    = $3
                   AND g.resource_type = 'folder'
                   AND (g.expires_at IS NULL OR g.expires_at > NOW())
                   AND target_f.folder_id IS NOT NULL
                   AND gf.lpath @> (SELECT lpath FROM storage.folders
                                     WHERE id = target_f.folder_id)
              ) any_match
             LIMIT 1
            "#,
        )
        .bind(subject_types)
        .bind(subject_ids)
        .bind(permission.as_str())
        .bind(file_id)
        .fetch_optional(self.pool.as_ref())
        .await
        .map_err(|e| DomainError::internal_error("PgAcl", format!("file cascade: {e}")))?;

        Ok(exists.is_some())
    }

    /// Look up a single grant by id. Returns `(resource, granted_by)` so
    /// the REST `DELETE /api/grants/{id}` handler can decide authorization
    /// without a second round-trip. Returns `Ok(None)` if no such grant.
    pub async fn find_grant_by_id(
        &self,
        grant_id: Uuid,
    ) -> Result<Option<(Resource, Uuid)>, DomainError> {
        let row: Option<(String, Uuid, Uuid)> = sqlx::query_as(
            "SELECT resource_type, resource_id, granted_by FROM storage.access_grants WHERE id = $1",
        )
        .bind(grant_id)
        .fetch_optional(self.pool.as_ref())
        .await
        .map_err(|e| DomainError::internal_error("PgAcl", format!("find_grant_by_id: {e}")))?;

        let Some((rt, rid, granter)) = row else {
            return Ok(None);
        };
        let res = Resource::from_parts(&rt, rid)
            .ok_or_else(|| DomainError::internal_error("PgAcl", "unknown resource_type"))?;
        Ok(Some((res, granter)))
    }

    /// Variant of `find_grant_by_id` that also returns the subject —
    /// needed by `POST /api/grants/{id}/notify` to resolve who to email.
    /// Returns `(subject, resource, granted_by)` or `None`.
    pub async fn find_grant_full_by_id(
        &self,
        grant_id: Uuid,
    ) -> Result<Option<(Subject, Resource, Uuid)>, DomainError> {
        let row: Option<(String, Uuid, String, Uuid, Uuid)> = sqlx::query_as(
            "SELECT subject_type, subject_id, resource_type, resource_id, granted_by \
             FROM storage.access_grants WHERE id = $1",
        )
        .bind(grant_id)
        .fetch_optional(self.pool.as_ref())
        .await
        .map_err(|e| DomainError::internal_error("PgAcl", format!("find_grant_full_by_id: {e}")))?;

        let Some((st, sid, rt, rid, granter)) = row else {
            return Ok(None);
        };
        let subject = Subject::from_parts(&st, sid)
            .ok_or_else(|| DomainError::internal_error("PgAcl", "unknown subject_type"))?;
        let resource = Resource::from_parts(&rt, rid)
            .ok_or_else(|| DomainError::internal_error("PgAcl", "unknown resource_type"))?;
        Ok(Some((subject, resource, granter)))
    }

    /// Row type for all full-grant SELECT queries:
    /// (id, subject_type, subject_id, resource_type, resource_id, permission, granted_by, granted_at, expires_at)
    #[allow(clippy::type_complexity)]
    fn row_to_grant(
        row: (
            Uuid,
            String,
            Uuid,
            String,
            Uuid,
            String,
            Uuid,
            chrono::DateTime<chrono::Utc>,
            Option<chrono::DateTime<chrono::Utc>>,
        ),
    ) -> Result<Grant, DomainError> {
        let subject = Subject::from_parts(&row.1, row.2)
            .ok_or_else(|| DomainError::internal_error("PgAcl", "unknown subject_type"))?;
        let resource = Resource::from_parts(&row.3, row.4)
            .ok_or_else(|| DomainError::internal_error("PgAcl", "unknown resource_type"))?;
        let permission = Permission::parse(&row.5)
            .ok_or_else(|| DomainError::internal_error("PgAcl", "unknown permission"))?;
        Ok(Grant {
            id: row.0,
            subject,
            resource,
            permission,
            granted_by: row.6,
            granted_at: row.7,
            expires_at: row.8,
        })
    }

    /// Reject an over-cap grant listing rather than returning a truncated set.
    /// The unbounded list methods fetch `MAX_GRANT_ROWS + 1` and pass the row
    /// count here; callers diff against the full result, so silently dropping
    /// rows would corrupt that diff. Emits an audit line before failing so the
    /// pathological resource/subject is visible to operators.
    fn guard_grant_row_cap(returned: usize, op: &str) -> Result<(), DomainError> {
        if returned as i64 > MAX_GRANT_ROWS {
            tracing::info!(
                target: "audit",
                event = "authz.grant_list_rejected",
                reason = "over_row_cap",
                op,
                cap = MAX_GRANT_ROWS,
                "👮🏻‍♂️ grant listing exceeded the row safety cap; refusing to return a partial set",
            );
            return Err(DomainError::internal_error(
                "PgAcl",
                format!("{op}: too many grants (cap {})", MAX_GRANT_ROWS),
            ));
        }
        Ok(())
    }

    /// The actual permission decision. Wrapped by `check()` which adds
    /// per-call instrumentation.
    async fn check_inner(
        &self,
        subject: Subject,
        permission: Permission,
        resource: Resource,
        counters: &QueryCounters,
    ) -> Result<bool, DomainError> {
        // Owner short-circuit (only for User subjects — groups/tokens/external
        // are never owners of resources).
        if let Subject::User(uid) = subject {
            counters.sql_queries.fetch_add(1, Ordering::Relaxed);
            match self.owner_of(resource).await {
                Ok(owner) if owner == uid => return Ok(true),
                Ok(_) => { /* not owner — fall through to grants */ }
                Err(e) if e.kind == crate::common::errors::ErrorKind::NotFound => {
                    // Resource doesn't exist — no permission. Return false
                    // rather than propagating NotFound; the caller (`require`)
                    // converts a false back to NotFound on its own.
                    return Ok(false);
                }
                Err(e) => return Err(e),
            }
        }

        // Expand the subject so group-mediated grants apply when the caller
        // is a User. See `subject_match_set` for the shared shape used by
        // both the cascade check and the "shared with me" listing queries.
        let (subject_types, subject_ids) = self.subject_match_set(subject, counters).await?;

        match resource {
            Resource::Folder(id) => {
                self.folder_cascade_grant_exists(
                    &subject_types,
                    &subject_ids,
                    permission,
                    id,
                    counters,
                )
                .await
            }
            Resource::File(id) => {
                self.file_cascade_grant_exists(
                    &subject_types,
                    &subject_ids,
                    permission,
                    id,
                    counters,
                )
                .await
            }
        }
    }
}

impl AuthorizationEngine for PgAclEngine {
    async fn check(
        &self,
        subject: Subject,
        permission: Permission,
        resource: Resource,
    ) -> Result<bool, DomainError> {
        let start = std::time::Instant::now();
        let counters = QueryCounters::default();

        let result = self
            .check_inner(subject, permission, resource, &counters)
            .await;

        // Single structured debug line per check. No-op when subscriber
        // filter is at INFO or above. See plan, "Debug instrumentation".
        tracing::debug!(
            target: "oxicloud::authz",
            event = "authz.check",
            subject = %subject,
            permission = %permission,
            resource = %resource,
            allowed = result.as_ref().copied().unwrap_or(false),
            duration_us = start.elapsed().as_micros() as u64,
            cache_hit = counters.cache_hit.load(Ordering::Relaxed) > 0,
            sql_queries = counters.sql_queries.load(Ordering::Relaxed),
            expanded_groups = counters.expanded_groups.load(Ordering::Relaxed),
        );

        result
    }

    async fn list_incoming_grants(
        &self,
        subject: Subject,
        permission_filter: Option<Permission>,
    ) -> Result<Vec<Grant>, DomainError> {
        let perm_str = permission_filter.map(|p| p.as_str().to_string());
        let counters = QueryCounters::default();
        let (subject_types, subject_ids) = self.subject_match_set(subject, &counters).await?;

        let rows = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                Uuid,
                String,
                Uuid,
                String,
                Uuid,
                chrono::DateTime<chrono::Utc>,
                Option<chrono::DateTime<chrono::Utc>>,
            ),
        >(
            r#"
            SELECT id, subject_type, subject_id, resource_type, resource_id,
                   permission, granted_by, granted_at, expires_at
              FROM storage.access_grants
             WHERE subject_type = ANY($1)
               AND subject_id   = ANY($2)
               AND ($3::text IS NULL OR permission = $3)
             ORDER BY granted_at DESC
             LIMIT $4
            "#,
        )
        .bind(&subject_types)
        .bind(&subject_ids)
        .bind(perm_str)
        .bind(MAX_GRANT_ROWS + 1)
        .fetch_all(self.pool.as_ref())
        .await
        .map_err(|e| DomainError::internal_error("PgAcl", format!("list incoming: {e}")))?;

        Self::guard_grant_row_cap(rows.len(), "list_incoming_grants")?;
        rows.into_iter().map(Self::row_to_grant).collect()
    }

    async fn list_incoming_resources_paged(
        &self,
        subject: Subject,
        kinds: &[ResourceKind],
        limit: u32,
        cursor: Option<GrantCursor>,
        sort_by: &str,
        reverse: bool,
    ) -> Result<(Vec<IncomingGrantSummary>, Option<GrantCursor>), DomainError> {
        // ── Common setup ──────────────────────────────────────────────────────
        let kind_strs: Option<Vec<&str>> = if kinds.is_empty() {
            None
        } else {
            Some(kinds.iter().map(|k| k.as_str()).collect())
        };
        let fetch_limit = (limit as i64) + 1;

        // Unified row type — the last two columns carry the sort key when present,
        // NULL otherwise.  This lets every sort mode share a single query_as call.
        //   0 resource_type  String
        //   1 resource_id    Uuid
        //   2 permissions    Vec<String>
        //   3 granted_at     DateTime<Utc>
        //   4 granted_by     Uuid
        //   5 sort_str       Option<String>  — resource_name (name/type) or owner_name (granted_by)
        //   6 sort_int       Option<i64>     — category_order (type) or file size in bytes (size)
        type Row = (
            String,
            Uuid,
            Vec<String>,
            chrono::DateTime<chrono::Utc>,
            Uuid,
            Option<String>,
            Option<i64>,
        );

        // Extract all cursor fields up-front; each branch uses the subset it needs.
        // Fixed parameter positions used in all SQL variants:
        //   $4 = cursor_str  (resource_name / owner_name)
        //   $5 = cursor_int  (type_order)
        //   $6 = cursor_at   (granted_at)
        //   $7 = cursor_id   (resource_id)
        //   $8 = fetch_limit
        let cursor_str = cursor.as_ref().and_then(|c| c.resource_name.clone());
        let cursor_int = cursor.as_ref().and_then(|c| c.sort_int);
        let cursor_at = cursor.as_ref().map(|c| c.granted_at);
        let cursor_id = cursor.as_ref().map(|c| c.resource_id);

        // ── agg CTE (identical in all branches) ───────────────────────────────
        // `subject_type`/`subject_id` are arrays here: for a User caller this
        // is `(["user","group"], [uid, …transitive groups, INTERNAL])` so the
        // listing includes every resource the user can reach via a group
        // grant (matching what `check()` allows). See `subject_match_set`.
        const AGG: &str = r#"agg AS (
            SELECT
                resource_type,
                resource_id,
                array_agg(DISTINCT permission ORDER BY permission) AS permissions,
                MIN(granted_at)                                    AS granted_at,
                (array_agg(granted_by ORDER BY granted_at))[1]    AS granted_by
            FROM storage.access_grants
            WHERE subject_type = ANY($1)
              AND subject_id   = ANY($2)
              AND ($3::text[] IS NULL OR resource_type = ANY($3))
            GROUP BY resource_type, resource_id
        )"#;

        // ── Build sort-specific SQL fragments ─────────────────────────────────
        // "name" and "type" share the same LEFT JOINs; only sort_int_expr,
        // the cursor WHERE condition, and ORDER BY differ.
        // Each branch emits two variants selected by `reverse`.
        let sql = match sort_by {
            "name" | "type" => {
                let sort_int_expr = if sort_by == "type" {
                    "CASE WHEN agg.resource_type = 'folder' THEN 0 ELSE fi.category_order::bigint END"
                } else {
                    "NULL::bigint"
                };
                // Normal vs reversed keyset + ORDER BY.
                let (where_clause, order_clause) = if sort_by == "type" {
                    if reverse {
                        (
                            r#"(  $5::integer IS NULL
                               OR sort_int < $5
                               OR (sort_int = $5 AND LOWER(sort_str) < $4)
                               OR (sort_int = $5 AND LOWER(sort_str) = $4 AND resource_id < $7::uuid))"#,
                            "sort_int DESC, LOWER(sort_str) DESC, resource_id DESC",
                        )
                    } else {
                        (
                            r#"(  $5::integer IS NULL
                               OR sort_int > $5
                               OR (sort_int = $5 AND LOWER(sort_str) > $4)
                               OR (sort_int = $5 AND LOWER(sort_str) = $4 AND resource_id > $7::uuid))"#,
                            "sort_int ASC, LOWER(sort_str) ASC, resource_id ASC",
                        )
                    }
                } else if reverse {
                    (
                        r#"(  $4::text IS NULL
                           OR LOWER(sort_str) < $4
                           OR (LOWER(sort_str) = $4 AND resource_id < $7::uuid))"#,
                        "LOWER(sort_str) DESC, resource_id DESC",
                    )
                } else {
                    (
                        r#"(  $4::text IS NULL
                           OR LOWER(sort_str) > $4
                           OR (LOWER(sort_str) = $4 AND resource_id > $7::uuid))"#,
                        "LOWER(sort_str) ASC, resource_id ASC",
                    )
                };
                format!(
                    r#"WITH {AGG},
                    named AS (
                        SELECT agg.*,
                            COALESCE(
                                CASE WHEN agg.resource_type = 'folder' THEN f.name  END,
                                CASE WHEN agg.resource_type = 'file'   THEN fi.name END
                            ) AS sort_str,
                            {sort_int_expr} AS sort_int
                        FROM agg
                        LEFT JOIN storage.folders f  ON f.id  = agg.resource_id AND agg.resource_type = 'folder'
                        LEFT JOIN storage.files   fi ON fi.id = agg.resource_id AND agg.resource_type = 'file'
                    )
                    SELECT resource_type, resource_id, permissions, granted_at, granted_by, sort_str, sort_int
                    FROM named
                    WHERE {where_clause}
                    ORDER BY {order_clause}
                    LIMIT $8"#
                )
            }
            "granted_by" => {
                // Joins auth.users to sort alphabetically by username.
                // Cursor encodes (owner_name=$4, granted_at=$6, resource_id=$7).
                let (where_clause, order_clause) = if reverse {
                    (
                        r#"(  $4::text IS NULL
                          OR sort_str < $4
                          OR (sort_str = $4 AND (
                                  $6::timestamptz IS NULL
                               OR granted_at > $6
                               OR (granted_at = $6 AND resource_id > $7::uuid))))"#,
                        "sort_str DESC, granted_at ASC, resource_id ASC",
                    )
                } else {
                    (
                        r#"(  $4::text IS NULL
                          OR sort_str > $4
                          OR (sort_str = $4 AND (
                                  $6::timestamptz IS NULL
                               OR granted_at < $6
                               OR (granted_at = $6 AND resource_id < $7::uuid))))"#,
                        "sort_str ASC, granted_at DESC, resource_id DESC",
                    )
                };
                format!(
                    r#"WITH {AGG},
                    owner_named AS (
                        SELECT agg.*,
                            LOWER(u.username) AS sort_str,
                            NULL::bigint AS sort_int
                        FROM agg
                        LEFT JOIN auth.users u ON u.id = agg.granted_by
                    )
                    SELECT resource_type, resource_id, permissions, granted_at, granted_by, sort_str, sort_int
                    FROM owner_named
                    WHERE {where_clause}
                    ORDER BY {order_clause}
                    LIMIT $8"#
                )
            }
            _ => {
                // Default: sort by grant date.
                // Normal = DESC (newest first); reversed = ASC (oldest first).
                // Cursor encodes (granted_at=$6, resource_id=$7); $4/$5 unused.
                let (where_clause, order_clause) = if reverse {
                    (
                        r#"(  $6::timestamptz IS NULL
                          OR granted_at > $6
                          OR (granted_at = $6 AND resource_id > $7::uuid))"#,
                        "granted_at ASC, resource_id ASC",
                    )
                } else {
                    (
                        r#"(  $6::timestamptz IS NULL
                          OR granted_at < $6
                          OR (granted_at = $6 AND resource_id < $7::uuid))"#,
                        "granted_at DESC, resource_id DESC",
                    )
                };
                format!(
                    r#"WITH {AGG}
                    SELECT resource_type, resource_id, permissions, granted_at, granted_by,
                           NULL::text   AS sort_str,
                           NULL::bigint AS sort_int
                    FROM agg
                    WHERE {where_clause}
                    ORDER BY {order_clause}
                    LIMIT $8"#
                )
            }
        };

        // Expand the caller so group-mediated grants surface in the listing,
        // mirroring `check()`. Shares the Moka cache (`expand_user`).
        let counters = QueryCounters::default();
        let (subject_types, subject_ids) = self.subject_match_set(subject, &counters).await?;

        // ── Execute — uniform 8 binds for every sort mode ─────────────────────
        let mut rows: Vec<Row> = sqlx::query_as::<_, Row>(&sql)
            .bind(&subject_types) // $1
            .bind(&subject_ids) // $2
            .bind(&kind_strs) // $3
            .bind(&cursor_str) // $4 sort_str cursor
            .bind(cursor_int) // $5 sort_int cursor
            .bind(cursor_at) // $6 granted_at cursor
            .bind(cursor_id) // $7 resource_id cursor
            .bind(fetch_limit) // $8
            .fetch_all(self.pool.as_ref())
            .await
            .map_err(|e| {
                DomainError::internal_error(
                    "PgAcl",
                    format!("list_incoming_resources_paged ({sort_by}): {e}"),
                )
            })?;

        // ── Pagination ────────────────────────────────────────────────────────
        let has_next = rows.len() > limit as usize;
        rows.truncate(limit as usize);

        let next_cursor = if has_next {
            rows.last().map(|r| {
                let sort_str_lc = r.5.as_deref().map(str::to_lowercase);
                match sort_by {
                    "name" => GrantCursor {
                        sort_by: "name".to_owned(),
                        granted_at: r.3,
                        resource_id: r.1,
                        resource_name: sort_str_lc,
                        sort_int: None,
                        reverse,
                    },
                    "type" => GrantCursor {
                        sort_by: "type".to_owned(),
                        granted_at: r.3,
                        resource_id: r.1,
                        resource_name: sort_str_lc,
                        sort_int: r.6,
                        reverse,
                    },
                    "granted_by" => GrantCursor {
                        sort_by: "granted_by".to_owned(),
                        granted_at: r.3,
                        resource_id: r.1,
                        resource_name: r.5.clone(), // already lowercased by SQL
                        sort_int: None,
                        reverse,
                    },
                    _ => GrantCursor {
                        sort_by: "granted_at".to_owned(),
                        granted_at: r.3,
                        resource_id: r.1,
                        resource_name: None,
                        sort_int: None,
                        reverse,
                    },
                }
            })
        } else {
            None
        };

        // ── Convert rows to domain summaries ──────────────────────────────────
        let summaries = rows
            .into_iter()
            .filter_map(|(rt, rid, perms_str, granted_at, granted_by, _, _)| {
                let resource_type = ResourceKind::parse(&rt)?;
                let permissions = perms_str
                    .into_iter()
                    .filter_map(|s| Permission::parse(&s))
                    .collect();
                Some(IncomingGrantSummary {
                    resource_type,
                    resource_id: rid,
                    permissions,
                    granted_at,
                    granted_by,
                })
            })
            .collect();

        Ok((summaries, next_cursor))
    }

    async fn list_grants_on_resource(&self, resource: Resource) -> Result<Vec<Grant>, DomainError> {
        let rows = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                Uuid,
                String,
                Uuid,
                String,
                Uuid,
                chrono::DateTime<chrono::Utc>,
                Option<chrono::DateTime<chrono::Utc>>,
            ),
        >(
            r#"
            SELECT id, subject_type, subject_id, resource_type, resource_id,
                   permission, granted_by, granted_at, expires_at
              FROM storage.access_grants
             WHERE resource_type = $1
               AND resource_id   = $2
             ORDER BY granted_at DESC
             LIMIT $3
            "#,
        )
        .bind(resource.type_str())
        .bind(resource.id())
        .bind(MAX_GRANT_ROWS + 1)
        .fetch_all(self.pool.as_ref())
        .await
        .map_err(|e| DomainError::internal_error("PgAcl", format!("list on resource: {e}")))?;

        Self::guard_grant_row_cap(rows.len(), "list_grants_on_resource")?;
        rows.into_iter().map(Self::row_to_grant).collect()
    }

    async fn list_outgoing_resources_paged(
        &self,
        granted_by: Uuid,
        limit: u32,
        cursor: Option<GrantCursor>,
        sort_by: &str,
        reverse: bool,
    ) -> Result<(Vec<OutgoingResourceSummary>, Option<GrantCursor>), DomainError> {
        let fetch_limit = (limit as i64) + 1;

        // Row shape — one row per (resource, subject, permission).
        // Columns:
        //   0  resource_type   String
        //   1  resource_id     Uuid
        //   2  first_shared_at DateTime<Utc>   — MIN(granted_at) across resource
        //   3  subject_type    String
        //   4  subject_id      Uuid
        //   5  subject_display String          — username or share item_name
        //   6  grant_id        Uuid
        //   7  granted_at      DateTime<Utc>   — this (subject, perm) row
        //   8  expires_at      Option<DateTime<Utc>>
        //   9  permission      String
        //  10  sort_str        Option<String>
        //  11  sort_int        Option<i64>
        //  12  has_password    bool            — token: shares.password_hash IS NOT NULL
        //  13  is_external     bool            — user: auth.users.is_external (PR N2);
        //                                       FALSE for token/group subjects.
        type Row = (
            String,
            Uuid,
            chrono::DateTime<chrono::Utc>,
            String,
            Uuid,
            String,
            Uuid,
            chrono::DateTime<chrono::Utc>,
            Option<chrono::DateTime<chrono::Utc>>,
            String,
            Option<String>,
            Option<i64>,
            bool,
            bool,
        );

        let cursor_str = cursor.as_ref().and_then(|c| c.resource_name.clone());
        let cursor_int = cursor.as_ref().and_then(|c| c.sort_int);
        let cursor_at = cursor.as_ref().map(|c| c.granted_at);
        let cursor_id = cursor.as_ref().map(|c| c.resource_id);

        // ── Resource-page CTE (one row per resource, cursor-paginated) ─────────
        // We page on resources (by first_shared_at + resource_id) so that the
        // limit/cursor semantics are consistent with the incoming endpoint.
        // All grants for each paged resource are then retrieved in the same query.
        //
        // $1 = granted_by
        // $2 = cursor_str   (resource_name for name/type, owner_name for granted_by)
        // $3 = cursor_int   (category_order for type, size for size)
        // $4 = cursor_at    (first_shared_at)
        // $5 = cursor_id    (resource_id)
        // $6 = fetch_limit
        let sql = match sort_by {
            "name" | "type" => {
                let sort_int_expr = if sort_by == "type" {
                    "CASE WHEN ag.resource_type = 'folder' THEN 0 ELSE fi.category_order::bigint END"
                } else {
                    "NULL::bigint"
                };
                let (page_where, page_order) = if sort_by == "type" {
                    if reverse {
                        (
                            r#"(  $3::integer IS NULL
                               OR sort_int < $3
                               OR (sort_int = $3 AND LOWER(sort_str) < $2)
                               OR (sort_int = $3 AND LOWER(sort_str) = $2 AND resource_id < $5::uuid))"#,
                            "sort_int DESC, LOWER(sort_str) DESC, resource_id DESC",
                        )
                    } else {
                        (
                            r#"(  $3::integer IS NULL
                               OR sort_int > $3
                               OR (sort_int = $3 AND LOWER(sort_str) > $2)
                               OR (sort_int = $3 AND LOWER(sort_str) = $2 AND resource_id > $5::uuid))"#,
                            "sort_int ASC, LOWER(sort_str) ASC, resource_id ASC",
                        )
                    }
                } else if reverse {
                    (
                        r#"(  $2::text IS NULL
                           OR LOWER(sort_str) < $2
                           OR (LOWER(sort_str) = $2 AND resource_id < $5::uuid))"#,
                        "LOWER(sort_str) DESC, resource_id DESC",
                    )
                } else {
                    (
                        r#"(  $2::text IS NULL
                           OR LOWER(sort_str) > $2
                           OR (LOWER(sort_str) = $2 AND resource_id > $5::uuid))"#,
                        "LOWER(sort_str) ASC, resource_id ASC",
                    )
                };
                format!(
                    r#"WITH resource_page AS (
                        SELECT ag.resource_type, ag.resource_id, MIN(ag.granted_at) AS first_shared_at,
                               COALESCE(
                                   CASE WHEN ag.resource_type = 'folder' THEN f.name  END,
                                   CASE WHEN ag.resource_type = 'file'   THEN fi.name END
                               ) AS sort_str,
                               {sort_int_expr} AS sort_int
                        FROM storage.access_grants ag
                        LEFT JOIN storage.folders f  ON f.id  = ag.resource_id AND ag.resource_type = 'folder'
                        LEFT JOIN storage.files   fi ON fi.id = ag.resource_id AND ag.resource_type = 'file'
                        WHERE ag.granted_by = $1
                        GROUP BY ag.resource_type, ag.resource_id, f.name, fi.name, fi.category_order
                    ),
                    rp AS (
                        SELECT * FROM resource_page
                        WHERE {page_where}
                        ORDER BY {page_order}
                        LIMIT $6
                    )
                    SELECT ag.resource_type, ag.resource_id, rp.first_shared_at,
                           ag.subject_type, ag.subject_id,
                           COALESCE(u.username, u.email, sg.name::text, sh.item_name, fi.name, fld.name, ag.subject_id::text) AS subject_display,
                           ag.id AS grant_id, ag.granted_at, ag.expires_at, ag.permission,
                           rp.sort_str, rp.sort_int,
                           (sh.password_hash IS NOT NULL) AS has_password,
                           COALESCE(u.is_external, FALSE) AS is_external
                    FROM rp
                    JOIN storage.access_grants ag
                      ON ag.resource_type = rp.resource_type AND ag.resource_id = rp.resource_id
                     AND ag.granted_by = $1
                    LEFT JOIN auth.users u   ON ag.subject_type = 'user'  AND u.id   = ag.subject_id
                    LEFT JOIN auth.subject_groups sg ON ag.subject_type = 'group' AND sg.id = ag.subject_id
                    LEFT JOIN storage.shares sh  ON ag.subject_type = 'token' AND sh.id  = ag.subject_id
                    LEFT JOIN storage.files fi   ON ag.subject_type = 'token' AND ag.resource_type = 'file'   AND fi.id  = ag.resource_id
                    LEFT JOIN storage.folders fld ON ag.subject_type = 'token' AND ag.resource_type = 'folder' AND fld.id = ag.resource_id
                    -- Per-resource grant ordering: groups → users → password-protected
                    -- links → public links (matches the "Shared with" subject sort).
                    -- Resource ordering comes from {page_order}; the CASE only
                    -- breaks ties within one resource.
                    ORDER BY {page_order},
                             CASE
                                 WHEN ag.subject_type = 'group' THEN 0
                                 WHEN ag.subject_type = 'user' THEN 1
                                 WHEN ag.subject_type = 'token' AND sh.password_hash IS NOT NULL THEN 2
                                 ELSE 3
                             END ASC,
                             LOWER(COALESCE(u.username, u.email, sg.name::text, sh.item_name, ag.subject_id::text)) ASC,
                             ag.granted_at"#
                )
            }
            "subject" => {
                // Page on (subject_type_order, subject_display, resource_id) triples so
                // every swimlane is always contiguous across cursor pages.
                //
                // subject_type_order: 0 = group, 1 = user, 2 = token with password,
                //                     3 = token without password
                // — picked so the My Shares "Shared with" view naturally renders the
                // higher-trust principals (groups, then named users) above the
                // lower-trust ones (anonymous link tokens).
                //
                // Cursor encodes: sort_int = subject_type_order, resource_name = LOWER(subject_display),
                // resource_id = last resource_id.
                let (page_where, page_order) = if reverse {
                    (
                        r#"(  $3::bigint IS NULL
                          OR sort_int < $3
                          OR (sort_int = $3 AND LOWER(subject_display) < $2)
                          OR (sort_int = $3 AND LOWER(subject_display) = $2 AND resource_id < $5::uuid))"#,
                        "sort_int DESC, LOWER(subject_display) DESC, resource_id DESC",
                    )
                } else {
                    (
                        r#"(  $3::bigint IS NULL
                          OR sort_int > $3
                          OR (sort_int = $3 AND LOWER(subject_display) > $2)
                          OR (sort_int = $3 AND LOWER(subject_display) = $2 AND resource_id > $5::uuid))"#,
                        "sort_int ASC, LOWER(subject_display) ASC, resource_id ASC",
                    )
                };
                format!(
                    r#"WITH pairs AS (
                        SELECT
                            ag.resource_type,
                            ag.resource_id,
                            ag.subject_type,
                            ag.subject_id,
                            MAX(COALESCE(u.username, u.email, sg.name::text, sh.item_name, ag.subject_id::text)) AS subject_display,
                            BOOL_OR(sh.password_hash IS NOT NULL) AS has_password,
                            COALESCE(BOOL_OR(u.is_external), FALSE) AS is_external,
                            MAX(CASE
                                WHEN ag.subject_type = 'group' THEN 0
                                WHEN ag.subject_type = 'user' THEN 1
                                WHEN ag.subject_type = 'token' AND sh.password_hash IS NOT NULL THEN 2
                                ELSE 3
                            END)::bigint AS sort_int,
                            MIN(ag.granted_at) AS first_granted_at
                        FROM storage.access_grants ag
                        LEFT JOIN auth.users u
                               ON ag.subject_type = 'user' AND u.id = ag.subject_id
                        LEFT JOIN auth.subject_groups sg
                               ON ag.subject_type = 'group' AND sg.id = ag.subject_id
                        LEFT JOIN storage.shares sh
                               ON ag.subject_type = 'token' AND sh.id = ag.subject_id
                        LEFT JOIN storage.files fi
                               ON ag.subject_type = 'token' AND ag.resource_type = 'file' AND fi.id = ag.resource_id
                        LEFT JOIN storage.folders fld
                               ON ag.subject_type = 'token' AND ag.resource_type = 'folder' AND fld.id = ag.resource_id
                        WHERE ag.granted_by = $1
                          AND (ag.expires_at IS NULL OR ag.expires_at > NOW())
                        GROUP BY ag.resource_type, ag.resource_id, ag.subject_type, ag.subject_id
                    ),
                    rp AS (
                        SELECT * FROM pairs
                        WHERE {page_where}
                        ORDER BY {page_order}
                        LIMIT $6
                    )
                    SELECT
                        ag.resource_type,
                        ag.resource_id,
                        rp.first_granted_at    AS first_shared_at,
                        ag.subject_type,
                        ag.subject_id,
                        rp.subject_display,
                        ag.id                  AS grant_id,
                        ag.granted_at,
                        ag.expires_at,
                        ag.permission,
                        LOWER(rp.subject_display) AS sort_str,
                        rp.sort_int,
                        rp.has_password,
                        rp.is_external
                    FROM rp
                    JOIN storage.access_grants ag
                      ON ag.resource_type = rp.resource_type
                     AND ag.resource_id   = rp.resource_id
                     AND ag.subject_type  = rp.subject_type
                     AND ag.subject_id    = rp.subject_id
                     AND ag.granted_by    = $1
                     AND (ag.expires_at IS NULL OR ag.expires_at > NOW())
                    ORDER BY {page_order}"#
                )
            }
            "role" => {
                // Page on (role_order, subject_display, resource_id) triples so that all
                // of one person's grants within a role are contiguous — enabling aggregation
                // ("Bob on Folder A, Folder B") to work correctly across cursor pages.
                // role_order: 0 = admin (has delete+share), 1 = editor (has create or update), 2 = viewer
                // Cursor: sort_int=role_order, resource_name=LOWER(subject_display), resource_id
                let (page_where, page_order) = if reverse {
                    (
                        r#"(  $3::bigint IS NULL
                          OR sort_int < $3
                          OR (sort_int = $3 AND LOWER(subject_display) < $2)
                          OR (sort_int = $3 AND LOWER(subject_display) = $2 AND resource_id < $5::uuid))"#,
                        "sort_int DESC, LOWER(subject_display) DESC, resource_id DESC",
                    )
                } else {
                    (
                        r#"(  $3::bigint IS NULL
                          OR sort_int > $3
                          OR (sort_int = $3 AND LOWER(subject_display) > $2)
                          OR (sort_int = $3 AND LOWER(subject_display) = $2 AND resource_id > $5::uuid))"#,
                        "sort_int ASC, LOWER(subject_display) ASC, resource_id ASC",
                    )
                };
                format!(
                    r#"WITH pairs AS (
                        SELECT
                            ag.resource_type,
                            ag.resource_id,
                            ag.subject_type,
                            ag.subject_id,
                            MAX(COALESCE(u.username, u.email, sh.item_name, ag.subject_id::text)) AS subject_display,
                            BOOL_OR(sh.password_hash IS NOT NULL) AS has_password,
                            COALESCE(BOOL_OR(u.is_external), FALSE) AS is_external,
                            CASE
                                WHEN BOOL_OR(ag.permission = 'delete')
                                 AND BOOL_OR(ag.permission = 'share')  THEN 0
                                WHEN BOOL_OR(ag.permission = 'create')
                                  OR BOOL_OR(ag.permission = 'update') THEN 1
                                ELSE 2
                            END::bigint AS sort_int,
                            MIN(ag.granted_at) AS first_granted_at
                        FROM storage.access_grants ag
                        LEFT JOIN auth.users u
                               ON ag.subject_type = 'user' AND u.id = ag.subject_id
                        LEFT JOIN storage.shares sh
                               ON ag.subject_type = 'token' AND sh.id = ag.subject_id
                        LEFT JOIN storage.files fi
                               ON ag.subject_type = 'token' AND ag.resource_type = 'file' AND fi.id = ag.resource_id
                        LEFT JOIN storage.folders fld
                               ON ag.subject_type = 'token' AND ag.resource_type = 'folder' AND fld.id = ag.resource_id
                        WHERE ag.granted_by = $1
                          AND (ag.expires_at IS NULL OR ag.expires_at > NOW())
                        GROUP BY ag.resource_type, ag.resource_id, ag.subject_type, ag.subject_id
                    ),
                    rp AS (
                        SELECT * FROM pairs
                        WHERE {page_where}
                        ORDER BY {page_order}
                        LIMIT $6
                    )
                    SELECT
                        ag.resource_type,
                        ag.resource_id,
                        rp.first_granted_at    AS first_shared_at,
                        ag.subject_type,
                        ag.subject_id,
                        rp.subject_display,
                        ag.id                  AS grant_id,
                        ag.granted_at,
                        ag.expires_at,
                        ag.permission,
                        LOWER(rp.subject_display) AS sort_str,
                        rp.sort_int,
                        rp.has_password,
                        rp.is_external
                    FROM rp
                    JOIN storage.access_grants ag
                      ON ag.resource_type = rp.resource_type
                     AND ag.resource_id   = rp.resource_id
                     AND ag.subject_type  = rp.subject_type
                     AND ag.subject_id    = rp.subject_id
                     AND ag.granted_by    = $1
                     AND (ag.expires_at IS NULL OR ag.expires_at > NOW())
                    ORDER BY {page_order}"#
                )
            }
            _ => {
                // Default: sort by first_shared_at DESC (newest resource shared first).
                let (page_where, page_order) = if reverse {
                    (
                        r#"(  $4::timestamptz IS NULL
                          OR first_shared_at > $4
                          OR (first_shared_at = $4 AND resource_id > $5::uuid))"#,
                        "first_shared_at ASC, resource_id ASC",
                    )
                } else {
                    (
                        r#"(  $4::timestamptz IS NULL
                          OR first_shared_at < $4
                          OR (first_shared_at = $4 AND resource_id < $5::uuid))"#,
                        "first_shared_at DESC, resource_id DESC",
                    )
                };
                format!(
                    r#"WITH resource_page AS (
                        SELECT resource_type, resource_id, MIN(granted_at) AS first_shared_at,
                               NULL::text   AS sort_str,
                               NULL::bigint AS sort_int
                        FROM storage.access_grants
                        WHERE granted_by = $1
                        GROUP BY resource_type, resource_id
                    ),
                    rp AS (
                        SELECT * FROM resource_page
                        WHERE {page_where}
                        ORDER BY {page_order}
                        LIMIT $6
                    )
                    SELECT ag.resource_type, ag.resource_id, rp.first_shared_at,
                           ag.subject_type, ag.subject_id,
                           COALESCE(u.username, u.email, sh.item_name, fi.name, fld.name, ag.subject_id::text) AS subject_display,
                           ag.id AS grant_id, ag.granted_at, ag.expires_at, ag.permission,
                           NULL::text AS sort_str, NULL::bigint AS sort_int,
                           (sh.password_hash IS NOT NULL) AS has_password,
                           COALESCE(u.is_external, FALSE) AS is_external
                    FROM rp
                    JOIN storage.access_grants ag
                      ON ag.resource_type = rp.resource_type AND ag.resource_id = rp.resource_id
                     AND ag.granted_by = $1
                    LEFT JOIN auth.users u    ON ag.subject_type = 'user'  AND u.id  = ag.subject_id
                    LEFT JOIN storage.shares sh   ON ag.subject_type = 'token' AND sh.id  = ag.subject_id
                    LEFT JOIN storage.files fi    ON ag.subject_type = 'token' AND ag.resource_type = 'file'   AND fi.id  = ag.resource_id
                    LEFT JOIN storage.folders fld ON ag.subject_type = 'token' AND ag.resource_type = 'folder' AND fld.id = ag.resource_id
                    ORDER BY {page_order}, ag.subject_id, ag.granted_at"#
                )
            }
        };

        let rows: Vec<Row> = sqlx::query_as::<_, Row>(&sql)
            .bind(granted_by) // $1
            .bind(&cursor_str) // $2 sort_str cursor
            .bind(cursor_int) // $3 sort_int cursor
            .bind(cursor_at) // $4 first_shared_at cursor
            .bind(cursor_id) // $5 resource_id cursor
            .bind(fetch_limit) // $6
            .fetch_all(self.pool.as_ref())
            .await
            .map_err(|e| {
                DomainError::internal_error(
                    "PgAcl",
                    format!("list_outgoing_resources_paged ({sort_by}): {e}"),
                )
            })?;

        // ── Subject / Role sorts: page on (resource_id, subject_id) pairs ───────
        // Each pair becomes one OutgoingResourceSummary with exactly one grant,
        // preserving the SQL-ordered swimlane sequence across cursor pages.
        if matches!(sort_by, "subject" | "role") {
            let mut seen_pairs: Vec<(Uuid, Uuid)> = Vec::new();
            let mut seen_pair_set: std::collections::HashSet<(Uuid, Uuid)> =
                std::collections::HashSet::new();
            for r in &rows {
                if seen_pair_set.insert((r.1, r.4)) {
                    seen_pairs.push((r.1, r.4));
                }
            }
            let has_next = seen_pairs.len() > limit as usize;
            seen_pairs.truncate(limit as usize);
            let keep: std::collections::HashSet<(Uuid, Uuid)> =
                seen_pairs.iter().copied().collect();

            let last_row = rows.iter().rfind(|r| keep.contains(&(r.1, r.4)));
            let next_cursor = if has_next {
                last_row.map(|r| {
                    let resource_name = r.10.clone(); // LOWER(subject_display) for both subject and role sort
                    GrantCursor {
                        sort_by: sort_by.to_owned(),
                        granted_at: r.2,
                        resource_id: r.1,
                        resource_name,
                        sort_int: r.11,
                        reverse,
                    }
                })
            } else {
                None
            };

            // Group rows: (resource_id, subject_id) → OutgoingGrantEntry.
            let mut entry_map: std::collections::HashMap<
                (Uuid, Uuid),
                (ResourceKind, OutgoingGrantEntry),
            > = std::collections::HashMap::new();
            for r in rows.into_iter().filter(|r| keep.contains(&(r.1, r.4))) {
                let (
                    rt_str,
                    resource_id,
                    _first_shared_at,
                    subj_type,
                    subj_id,
                    subj_display,
                    grant_id,
                    granted_at,
                    expires_at,
                    perm_str,
                    _,
                    _,
                    has_password,
                    is_external,
                ) = r;
                let Some(resource_type) = ResourceKind::parse(&rt_str) else {
                    continue;
                };
                let Some(perm) = Permission::parse(&perm_str) else {
                    continue;
                };
                let key = (resource_id, subj_id);
                let (_, entry) = entry_map.entry(key).or_insert_with(|| {
                    (
                        resource_type,
                        OutgoingGrantEntry {
                            grant_id,
                            subject_type: subj_type.clone(),
                            subject_id: subj_id,
                            subject_display: subj_display.clone(),
                            permissions: Vec::new(),
                            granted_at,
                            expires_at,
                            has_password,
                            is_external,
                        },
                    )
                });
                if !entry.permissions.contains(&perm) {
                    entry.permissions.push(perm);
                }
            }

            let summaries: Vec<OutgoingResourceSummary> = seen_pairs
                .into_iter()
                .filter_map(|(rid, sid)| {
                    let (resource_type, grant) = entry_map.remove(&(rid, sid))?;
                    Some(OutgoingResourceSummary {
                        resource_type,
                        resource_id: rid,
                        first_shared_at: grant.granted_at,
                        grants: vec![grant],
                    })
                })
                .collect();

            return Ok((summaries, next_cursor));
        }

        // ── All other sorts: page on distinct resource_ids ────────────────────
        let mut seen_resources: Vec<Uuid> = Vec::new();
        let mut seen_set: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
        for r in &rows {
            if seen_set.insert(r.1) {
                seen_resources.push(r.1);
            }
        }

        let has_next = seen_resources.len() > limit as usize;
        seen_resources.truncate(limit as usize);
        let keep: std::collections::HashSet<Uuid> = seen_resources.iter().copied().collect();

        let last_row = rows.iter().rfind(|r| keep.contains(&r.1));
        let next_cursor = if has_next {
            last_row.map(|r| {
                let sort_str_lc = r.10.as_deref().map(str::to_lowercase);
                match sort_by {
                    "name" => GrantCursor {
                        sort_by: "name".to_owned(),
                        granted_at: r.2,
                        resource_id: r.1,
                        resource_name: sort_str_lc,
                        sort_int: None,
                        reverse,
                    },
                    "type" => GrantCursor {
                        sort_by: "type".to_owned(),
                        granted_at: r.2,
                        resource_id: r.1,
                        resource_name: sort_str_lc,
                        sort_int: r.11,
                        reverse,
                    },
                    _ => GrantCursor {
                        sort_by: "first_shared_at".to_owned(),
                        granted_at: r.2,
                        resource_id: r.1,
                        resource_name: None,
                        sort_int: None,
                        reverse,
                    },
                }
            })
        } else {
            None
        };

        // Group flat rows by resource_id → (ResourceKind, first_shared_at, subjects).
        type ResourceEntry = (
            ResourceKind,
            chrono::DateTime<chrono::Utc>,
            std::collections::HashMap<Uuid, OutgoingGrantEntry>,
        );
        let mut resource_map: std::collections::HashMap<Uuid, ResourceEntry> =
            std::collections::HashMap::new();

        for r in rows.into_iter().filter(|r| keep.contains(&r.1)) {
            let (
                rt_str,
                resource_id,
                first_shared_at,
                subj_type,
                subj_id,
                subj_display,
                grant_id,
                granted_at,
                expires_at,
                perm_str,
                _,
                _,
                has_password,
                is_external,
            ) = r;
            let Some(resource_type) = ResourceKind::parse(&rt_str) else {
                continue;
            };
            let Some(perm) = Permission::parse(&perm_str) else {
                continue;
            };

            let (_, _, subj_map) = resource_map.entry(resource_id).or_insert_with(|| {
                (
                    resource_type,
                    first_shared_at,
                    std::collections::HashMap::new(),
                )
            });
            let entry = subj_map
                .entry(subj_id)
                .or_insert_with(|| OutgoingGrantEntry {
                    grant_id,
                    subject_type: subj_type.clone(),
                    subject_id: subj_id,
                    subject_display: subj_display.clone(),
                    permissions: Vec::new(),
                    granted_at,
                    expires_at,
                    has_password,
                    is_external,
                });
            if !entry.permissions.contains(&perm) {
                entry.permissions.push(perm);
            }
        }

        let summaries: Vec<OutgoingResourceSummary> = seen_resources
            .into_iter()
            .filter_map(|rid| {
                let (resource_type, first_shared_at, subj_map) = resource_map.remove(&rid)?;
                let mut grants: Vec<OutgoingGrantEntry> = subj_map.into_values().collect();
                // Per-resource subject ordering (matches the subject-sort
                // branch's SQL CASE):
                //   0 = group, 1 = user, 2 = token-with-password, 3 = token,
                //   4 = external.
                // Alphabetical tiebreak by display name. This intentionally
                // ignores role/permission tier — the share dialog renders
                // role as a separate pill; ordering by subject type is the
                // UX contract.
                let subject_rank = |e: &OutgoingGrantEntry| -> u8 {
                    match e.subject_type.as_str() {
                        "group" => 0,
                        "user" => 1,
                        "token" if e.has_password => 2,
                        "token" => 3,
                        _ => 4,
                    }
                };
                grants.sort_by(|a, b| {
                    subject_rank(a).cmp(&subject_rank(b)).then_with(|| {
                        a.subject_display
                            .to_lowercase()
                            .cmp(&b.subject_display.to_lowercase())
                    })
                });
                Some(OutgoingResourceSummary {
                    resource_type,
                    resource_id: rid,
                    first_shared_at,
                    grants,
                })
            })
            .collect();

        Ok((summaries, next_cursor))
    }

    async fn list_outgoing_grants(&self, granted_by: Uuid) -> Result<Vec<Grant>, DomainError> {
        let rows = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                Uuid,
                String,
                Uuid,
                String,
                Uuid,
                chrono::DateTime<chrono::Utc>,
                Option<chrono::DateTime<chrono::Utc>>,
            ),
        >(
            r#"
            SELECT id, subject_type, subject_id, resource_type, resource_id,
                   permission, granted_by, granted_at, expires_at
              FROM storage.access_grants
             WHERE granted_by = $1
             ORDER BY granted_at DESC
            "#,
        )
        .bind(granted_by)
        .fetch_all(self.pool.as_ref())
        .await
        .map_err(|e| DomainError::internal_error("PgAcl", format!("list outgoing: {e}")))?;

        rows.into_iter().map(Self::row_to_grant).collect()
    }

    async fn grant(
        &self,
        granted_by: Uuid,
        subject: Subject,
        permission: Permission,
        resource: Resource,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Grant, DomainError> {
        let row = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                Uuid,
                String,
                Uuid,
                String,
                Uuid,
                chrono::DateTime<chrono::Utc>,
                Option<chrono::DateTime<chrono::Utc>>,
            ),
        >(
            r#"
            INSERT INTO storage.access_grants
                (subject_type, subject_id, resource_type, resource_id, permission, granted_by, expires_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (subject_type, subject_id, resource_type, resource_id, permission)
            DO UPDATE SET expires_at = EXCLUDED.expires_at
            RETURNING id, subject_type, subject_id, resource_type, resource_id,
                      permission, granted_by, granted_at, expires_at
            "#,
        )
        .bind(subject.type_str())
        .bind(subject.id())
        .bind(resource.type_str())
        .bind(resource.id())
        .bind(permission.as_str())
        .bind(granted_by)
        .bind(expires_at)
        .fetch_one(self.pool.as_ref())
        .await
        .map_err(|e| DomainError::internal_error("PgAcl", format!("insert grant: {e}")))?;

        Self::row_to_grant(row)
    }

    async fn set_expiry_for_subject(
        &self,
        subject: Subject,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<(), DomainError> {
        sqlx::query(
            "UPDATE storage.access_grants SET expires_at = $3 WHERE subject_type = $1 AND subject_id = $2",
        )
        .bind(subject.type_str())
        .bind(subject.id())
        .bind(expires_at)
        .execute(self.pool.as_ref())
        .await
        .map_err(|e| DomainError::internal_error("PgAcl", format!("set_expiry_for_subject: {e}")))?;
        Ok(())
    }

    async fn set_expiry_on_resource(
        &self,
        subject: Subject,
        resource: Resource,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<(), DomainError> {
        sqlx::query(
            "UPDATE storage.access_grants SET expires_at = $3 \
             WHERE subject_type = $1 AND subject_id = $2 \
             AND resource_type = $4 AND resource_id = $5",
        )
        .bind(subject.type_str())
        .bind(subject.id())
        .bind(expires_at)
        .bind(resource.type_str())
        .bind(resource.id())
        .execute(self.pool.as_ref())
        .await
        .map_err(|e| {
            DomainError::internal_error("PgAcl", format!("set_expiry_on_resource: {e}"))
        })?;
        Ok(())
    }

    async fn revoke(&self, grant_id: Uuid) -> Result<(), DomainError> {
        sqlx::query("DELETE FROM storage.access_grants WHERE id = $1")
            .bind(grant_id)
            .execute(self.pool.as_ref())
            .await
            .map_err(|e| DomainError::internal_error("PgAcl", format!("revoke: {e}")))?;
        Ok(())
    }

    async fn revoke_all_for_resource(&self, resource: Resource) -> Result<usize, DomainError> {
        let result = sqlx::query(
            "DELETE FROM storage.access_grants WHERE resource_type = $1 AND resource_id = $2",
        )
        .bind(resource.type_str())
        .bind(resource.id())
        .execute(self.pool.as_ref())
        .await
        .map_err(|e| DomainError::internal_error("PgAcl", format!("revoke for resource: {e}")))?;

        Ok(result.rows_affected() as usize)
    }

    async fn revoke_all_for_subject(&self, subject: Subject) -> Result<usize, DomainError> {
        let result = sqlx::query(
            "DELETE FROM storage.access_grants WHERE subject_type = $1 AND subject_id = $2",
        )
        .bind(subject.type_str())
        .bind(subject.id())
        .execute(self.pool.as_ref())
        .await
        .map_err(|e| DomainError::internal_error("PgAcl", format!("revoke for subject: {e}")))?;

        Ok(result.rows_affected() as usize)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AuthzCacheLifecycleHook
//
// Owns invalidation of the `user_groups_cache` Moka entry when a user's
// state changes in ways that affect transitive-group expansion (logout
// — so a re-login with new group memberships doesn't observe a stale
// expansion during the 30 s TTL window; delete — so a re-created
// account with the same id doesn't inherit the old cached value).
//
// Lives in this file (not under a centralised `lifecycle/` directory)
// because the authz engine owns its own cache invariants. See the
// "owner-located convention" note in
// `docs/architecture/user-lifecycle.md`.
// ─────────────────────────────────────────────────────────────────────────────

use async_trait::async_trait;

use crate::application::ports::user_lifecycle::{DeletionMode, LogoutReason, UserLifecycleHook};
use crate::domain::entities::user::User;

/// Lifecycle hook: drops the `user_groups_cache` entry for one user on
/// logout / deletion so the next authz check rebuilds it from current
/// `subject_group_members` rows.
pub struct AuthzCacheLifecycleHook {
    engine: Arc<PgAclEngine>,
}

impl AuthzCacheLifecycleHook {
    pub fn new(engine: Arc<PgAclEngine>) -> Self {
        Self { engine }
    }
}

#[async_trait]
impl UserLifecycleHook for AuthzCacheLifecycleHook {
    fn name(&self) -> &'static str {
        "authz_cache"
    }

    async fn on_user_created(&self, _user: &User) -> Result<(), DomainError> {
        // New user can't have a stale cache entry (no prior `expand_user`
        // call has produced one). Explicit no-op per the trait convention.
        Ok(())
    }

    async fn on_user_login(&self, _user: &User) -> Result<(), DomainError> {
        // Login doesn't change group membership; the cache (if present)
        // is still correct.
        Ok(())
    }

    async fn on_user_logout(&self, user: &User, _reason: LogoutReason) -> Result<(), DomainError> {
        self.engine.invalidate_user_groups_cache(user.id()).await;
        Ok(())
    }

    async fn on_user_deleted(
        &self,
        user: &User,
        _mode: DeletionMode,
        _tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), DomainError> {
        // No DB writes here — just memory invalidation. `_tx` is
        // intentionally ignored.
        self.engine.invalidate_user_groups_cache(user.id()).await;
        Ok(())
    }
}
