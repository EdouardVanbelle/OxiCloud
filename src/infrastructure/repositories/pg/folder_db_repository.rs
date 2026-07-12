//! PostgreSQL-backed folder repository.
//!
//! Implements `FolderRepository` (and thus `FolderStoragePort`) using the
//! `storage.folders` table.  Folders are purely virtual — no physical
//! directories are created on the filesystem.
//!
//! Folder paths are **materialized** in a `path TEXT` column maintained by
//! database triggers, so reading a folder's full path is always O(1) — no
//! recursive CTEs or N+1 queries.

use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

use super::transaction_utils::retry_on_deadlock;
use crate::application::dtos::folder_dto::{FolderResourceCursor, FolderResourceRow};
use crate::common::errors::DomainError;
use crate::domain::entities::folder::Folder;
use crate::domain::repositories::folder_repository::FolderRepository;
use crate::domain::services::authorization::ResourceKind;
use crate::domain::services::path_service::StoragePath;

/// Type alias for folder metadata rows from SQL queries.
/// Tuple order: id, name, path, parent_id, drive_id, created_at,
/// modified_at, tree_modified_at, created_by, updated_by.
/// The trailing `tree_modified_at` feeds [`Folder::etag`] — every
/// SELECT here must include `EXTRACT(EPOCH FROM tree_modified_at)::bigint`.
/// `drive_id` is the post-D0 `NOT NULL` scope axis for path-based
/// lookups. `created_by` / `updated_by` are the §14 provenance
/// columns, nullable because the FK is `ON DELETE SET NULL`.
///
/// Post-D7-step-6: `storage.folders.user_id` dropped, so the tuple
/// no longer carries it. The domain entity's `user_id` field is
/// populated with `None` at `row_to_folder` construction.
type FolderRow = (
    String,
    String,
    String,
    Option<String>,
    Uuid,
    i64,
    i64,
    i64,
    Option<Uuid>,
    Option<Uuid>,
);

/// Type alias for paginated folder rows (includes total_count as
/// the last element after the §14 provenance columns). Same
/// column set as [`FolderRow`] plus the trailing count.
type FolderRowPaginated = (
    String,
    String,
    String,
    Option<String>,
    Uuid,
    i64,
    i64,
    i64,
    Option<Uuid>,
    Option<Uuid>,
    i64,
);

/// SQL `EXISTS (…)` predicate — true when the caller (bound to `$1`) has
/// any active `role_grants` on the drive owning `fo` (the aliased folder
/// row). Group memberships (direct + transitive) are expanded inline via
/// `storage.caller_group_ids($1)` (recursive; see migration
/// `20260901000002_caller_group_ids_function.sql`).
///
/// Used by every drive-scoped folder query in this repo:
/// - `list_root_folders_for_caller` / `_paginated`
/// - `search_folders` (all three branches)
/// - `list_descendant_folders`
///
/// **Alias contract**: queries splicing this in MUST alias
/// `storage.folders` as `fo`. `$1` is reserved for `caller_id`; other
/// bind params start at `$2`.
///
/// This mirrors — but is not shared with — the drive-membership shape in
/// `drive_pg_repository::list_readable_by` (uses `JOIN` on `d.id`) and
/// the media-scoping subqueries in `file_blob_read_repository` (use
/// `IN (SELECT d.id …)` with the `include_in_photo_index` policy
/// filter). When the grant model changes, update all sites in parallel.
const CALLER_CAN_READ_DRIVE: &str = "EXISTS (\
        SELECT 1 \
          FROM storage.role_grants g \
         WHERE g.resource_type = 'drive' \
           AND g.resource_id   = fo.drive_id \
           AND (g.expires_at IS NULL OR g.expires_at > NOW()) \
           AND ( \
                 (g.subject_type = 'user'  AND g.subject_id = $1) \
              OR (g.subject_type = 'group' AND g.subject_id IN \
                      (SELECT storage.caller_group_ids($1))) \
               ) \
    )";

/// PostgreSQL-backed folder repository.
///
/// All folder metadata lives in the `storage.folders` table.  The physical
/// filesystem is never touched for folder operations.
pub struct FolderDbRepository {
    pool: Option<Arc<PgPool>>,
}

impl FolderDbRepository {
    pub fn new(pool: Arc<PgPool>) -> Self {
        Self { pool: Some(pool) }
    }

    /// Creates a stub instance for `AppState::default()`.
    /// This is never called in production — only used for route scaffolding.
    pub fn new_stub() -> Self {
        Self { pool: None }
    }

    /// Get the pool, panicking if stub.
    fn pool(&self) -> &PgPool {
        self.pool
            .as_deref()
            .expect("FolderDbRepository: pool not available (stub instance)")
    }

    // ── helpers ──────────────────────────────────────────────────

    /// Convert a database row into a `Folder` domain entity.
    ///
    /// The `path` comes directly from the materialized `path` column — no
    /// extra queries needed. `created_by` / `updated_by` carry the
    /// §14 provenance signal through the entity layer; both are
    /// `Option<Uuid>` because the FK is `ON DELETE SET NULL`.
    #[allow(clippy::too_many_arguments)]
    fn row_to_folder(
        id: String,
        name: String,
        path: String,
        parent_id: Option<String>,
        drive_id: Uuid,
        created_at: i64,
        modified_at: i64,
        tree_modified_at: i64,
        created_by: Option<Uuid>,
        updated_by: Option<Uuid>,
    ) -> Result<Folder, DomainError> {
        let storage_path = StoragePath::from_string(&path);
        Folder::with_timestamps_tree_and_provenance(
            id,
            name,
            storage_path,
            parent_id,
            drive_id,
            created_at as u64,
            modified_at as u64,
            tree_modified_at as u64,
            created_by,
            updated_by,
        )
        .map_err(|e| DomainError::internal_error("FolderDb", format!("entity: {e}")))
    }

    /// Batch-fetch folders by id — the by-ids counterpart of `get_folder`,
    /// resolving a page of ACL grants or favorites in ONE query instead of
    /// one per id. Same `NOT is_trashed` filter and column mapping as
    /// `get_folder`; missing or trashed ids drop out and callers re-associate
    /// by id; ordering is not guaranteed.
    pub async fn get_folders_by_ids(&self, ids: &[String]) -> Result<Vec<Folder>, DomainError> {
        let uuid_ids: Vec<Uuid> = ids.iter().filter_map(|id| id.parse().ok()).collect();
        if uuid_ids.is_empty() {
            return Ok(Vec::new());
        }

        let rows = sqlx::query_as::<_, FolderRow>(
            r#"
            SELECT id::text, name, path, parent_id::text, drive_id,
                   EXTRACT(EPOCH FROM created_at)::bigint,
                   EXTRACT(EPOCH FROM updated_at)::bigint,
                   EXTRACT(EPOCH FROM tree_modified_at)::bigint,
                   created_by, updated_by
              FROM storage.folders
             WHERE id = ANY($1) AND NOT is_trashed
            "#,
        )
        .bind(&uuid_ids)
        .fetch_all(self.pool())
        .await
        .map_err(|e| DomainError::internal_error("FolderDb", format!("get_folders_by_ids: {e}")))?;

        rows.into_iter()
            .map(|r| Self::row_to_folder(r.0, r.1, r.2, r.3, r.4, r.5, r.6, r.7, r.8, r.9))
            .collect()
    }
}

impl FolderRepository for FolderDbRepository {
    async fn create_folder(
        &self,
        name: String,
        parent_id: Option<String>,
        caller_id: Uuid,
    ) -> Result<Folder, DomainError> {
        // Derive `drive_id` from the parent folder. Root-level folders
        // are reserved for the atomic drive-creation transaction in
        // `DrivePgRepository::create_personal_drive_atomic` (see
        // `docs/plan/drive.md` §3) — the no-orphan-root-folder trigger
        // enforces this at the DB level.
        //
        // Post-D7: only `drive_id` is fetched from the parent. The
        // legacy `user_id` column is no longer written to on new rows
        // (migration `20260902000000_files_folders_user_id_nullable.sql`);
        // provenance flows through `created_by` / `updated_by` (§14).
        let drive_id: Uuid = if let Some(ref pid) = parent_id {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT drive_id FROM storage.folders WHERE id = $1::uuid",
            )
            .bind(pid)
            .fetch_optional(self.pool())
            .await
            .map_err(|e| DomainError::internal_error("FolderDb", format!("parent lookup: {e}")))?
            .ok_or_else(|| DomainError::not_found("Folder", pid))?
        } else {
            return Err(DomainError::internal_error(
                "FolderDb",
                "Cannot create root folder — root folders are reserved for the \
                 atomic drive-creation transaction in DrivePgRepository::\
                 create_personal_drive_atomic (docs/plan/drive.md §3). The \
                 no-orphan-root-folder trigger enforces this at the DB level.",
            ));
        };

        // Post-D7: no `user_id` in the INSERT column list — the column
        // is nullable and copied rows / new rows leave it NULL.
        // `created_by` / `updated_by` carry §14 provenance (both bind to
        // the caller — pre-D2 that's silently the parent's owner too,
        // but the distinction matters once shared drives let an Editor
        // mutate a folder owned by someone else).
        //
        // RETURNING surfaces the two provenance columns so the built
        // entity / DTO carries fresh values without a re-read.
        let row = sqlx::query_as::<_, (String, String, i64, i64, i64, Option<Uuid>, Option<Uuid>)>(
            r#"
            INSERT INTO storage.folders
                (name, parent_id, drive_id, created_by, updated_by)
            VALUES ($1, $2::uuid, $3, $4, $4)
            RETURNING id::text,
                      path,
                      EXTRACT(EPOCH FROM created_at)::bigint,
                      EXTRACT(EPOCH FROM updated_at)::bigint,
                      EXTRACT(EPOCH FROM tree_modified_at)::bigint,
                      created_by,
                      updated_by
            "#,
        )
        .bind(&name)
        .bind(&parent_id)
        .bind(drive_id)
        .bind(caller_id)
        .fetch_one(self.pool())
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(ref db_err) = e
                && db_err.code().as_deref() == Some("23505")
            {
                return DomainError::already_exists(
                    "Folder",
                    format!("{name} already exists in parent"),
                );
            }
            DomainError::internal_error("FolderDb", format!("insert: {e}"))
        })?;

        Self::row_to_folder(
            row.0, name, row.1, parent_id, drive_id, row.2, row.3, row.4,
            // Fresh from RETURNING — caller_id was bound to both columns.
            row.5, row.6,
        )
    }

    async fn get_folder(&self, id: &str) -> Result<Folder, DomainError> {
        let row = sqlx::query_as::<_, FolderRow>(
            r#"
            SELECT id::text, name, path, parent_id::text, drive_id,
                   EXTRACT(EPOCH FROM created_at)::bigint,
                   EXTRACT(EPOCH FROM updated_at)::bigint,
                   EXTRACT(EPOCH FROM tree_modified_at)::bigint,
                     created_by, updated_by
              FROM storage.folders
             WHERE id = $1::uuid AND NOT is_trashed
            "#,
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await
        .map_err(|e| DomainError::internal_error("FolderDb", format!("get: {e}")))?
        .ok_or_else(|| DomainError::not_found("Folder", id))?;

        Self::row_to_folder(
            row.0, row.1, row.2, row.3, row.4, row.5, row.6, row.7, row.8, row.9,
        )
    }

    async fn get_folder_by_path(
        &self,
        storage_path: &StoragePath,
        drive_id: Uuid,
    ) -> Result<Folder, DomainError> {
        let path_str = storage_path.to_string();
        // Strip leading '/' if present — DB stores "Home - user/Docs", not "/Home - user/Docs"
        let lookup = path_str.strip_prefix('/').unwrap_or(&path_str);

        if lookup.is_empty() {
            return Err(DomainError::not_found("Folder", "empty path"));
        }

        // Scoped by drive_id: post-D0 `storage.folders.path` is unique
        // only within a single drive. Root-folder names like
        // `"Personal"` repeat across drives, so without the drive_id
        // filter the planner returns a non-deterministic row — which
        // breaks owner-short-circuit checks and crosses drive
        // boundaries (the AuthZ axis that replaces the old per-user
        // wrapper scoping post-D0).
        let row = sqlx::query_as::<_, FolderRow>(
            r#"
            SELECT id::text, name, path, parent_id::text, drive_id,
                   EXTRACT(EPOCH FROM created_at)::bigint,
                   EXTRACT(EPOCH FROM updated_at)::bigint,
                       EXTRACT(EPOCH FROM tree_modified_at)::bigint,
                     created_by, updated_by
              FROM storage.folders
             WHERE path = $1 AND drive_id = $2 AND NOT is_trashed
            "#,
        )
        .bind(lookup)
        .bind(drive_id)
        .fetch_optional(self.pool())
        .await
        .map_err(|e| DomainError::internal_error("FolderDb", format!("path lookup: {e}")))?
        .ok_or_else(|| DomainError::not_found("Folder", lookup))?;

        Self::row_to_folder(
            row.0, row.1, row.2, row.3, row.4, row.5, row.6, row.7, row.8, row.9,
        )
    }

    #[allow(clippy::type_complexity)]
    async fn list_folders(&self, parent_id: Option<&str>) -> Result<Vec<Folder>, DomainError> {
        let rows: Vec<FolderRow> = if let Some(pid) = parent_id {
            sqlx::query_as(
                r#"
                SELECT id::text, name, path, parent_id::text, drive_id,
                       EXTRACT(EPOCH FROM created_at)::bigint,
                       EXTRACT(EPOCH FROM updated_at)::bigint,
                       EXTRACT(EPOCH FROM tree_modified_at)::bigint,
                         created_by, updated_by
                  FROM storage.folders
                 WHERE parent_id = $1::uuid AND NOT is_trashed
                 ORDER BY name
                "#,
            )
            .bind(pid)
            .fetch_all(self.pool())
            .await
        } else {
            sqlx::query_as(
                r#"
                SELECT id::text, name, path, parent_id::text, drive_id,
                       EXTRACT(EPOCH FROM created_at)::bigint,
                       EXTRACT(EPOCH FROM updated_at)::bigint,
                       EXTRACT(EPOCH FROM tree_modified_at)::bigint,
                         created_by, updated_by
                  FROM storage.folders
                 WHERE parent_id IS NULL AND NOT is_trashed
                 ORDER BY name
                "#,
            )
            .fetch_all(self.pool())
            .await
        }
        .map_err(|e| DomainError::internal_error("FolderDb", format!("list: {e}")))?;

        rows.into_iter()
            .map(|(id, name, path, pid, did, ca, ma, tma, cb, ub)| {
                Self::row_to_folder(id, name, path, pid, did, ca, ma, tma, cb, ub)
            })
            .collect()
    }

    async fn list_root_folders_for_caller(
        &self,
        caller_id: Uuid,
    ) -> Result<Vec<Folder>, DomainError> {
        // Drive-scoped root-folder listing: return every root folder
        // whose drive the caller has any role_grant on. Group
        // memberships (direct + transitive) resolve inline via
        // `storage.caller_group_ids($1)`.
        //
        // Closes `bug_root_folder_listing_legacy_user_id`: pre-D7 this
        // query filtered on `folders.user_id = $caller`, which returned
        // rows admin had created for other users' drives without ever
        // getting a role on them. The drive-membership predicate below
        // makes the "admin's own listing" correct without a separate
        // filter.
        //
        // `caller_role` is NOT surfaced here — see the memory
        // `project_caller_role_on_file_folder_dto` and the note at the
        // top of `folder_repository.rs`. Frontend cross-references
        // `/api/drives::caller_role` via `folder.drive_id`.
        let sql = format!(
            "SELECT fo.id::text, fo.name, fo.path, fo.parent_id::text, \
                    fo.drive_id, \
                    EXTRACT(EPOCH FROM fo.created_at)::bigint, \
                    EXTRACT(EPOCH FROM fo.updated_at)::bigint, \
                    EXTRACT(EPOCH FROM fo.tree_modified_at)::bigint, \
                    fo.created_by, fo.updated_by \
               FROM storage.folders fo \
              WHERE fo.parent_id IS NULL \
                AND NOT fo.is_trashed \
                AND {CALLER_CAN_READ_DRIVE} \
              ORDER BY fo.name"
        );
        let rows: Vec<FolderRow> = sqlx::query_as(&sql)
            .bind(caller_id)
            .fetch_all(self.pool())
            .await
            .map_err(|e| {
                DomainError::internal_error("FolderDb", format!("list_root_folders: {e}"))
            })?;

        rows.into_iter()
            .map(|(id, name, path, pid, did, ca, ma, tma, cb, ub)| {
                Self::row_to_folder(id, name, path, pid, did, ca, ma, tma, cb, ub)
            })
            .collect()
    }

    /// Paginated folder listing — single query with `COUNT(*) OVER()` window
    /// function so the total matching count comes back alongside the data rows,
    /// eliminating a separate COUNT round-trip.
    #[allow(clippy::type_complexity)]
    async fn list_folders_paginated(
        &self,
        parent_id: Option<&str>,
        offset: usize,
        limit: usize,
        include_total: bool,
    ) -> Result<(Vec<Folder>, Option<usize>), DomainError> {
        let rows: Vec<FolderRowPaginated> = if let Some(pid) = parent_id {
            sqlx::query_as(
                r#"
                SELECT id::text, name, path, parent_id::text, drive_id,
                       EXTRACT(EPOCH FROM created_at)::bigint,
                       EXTRACT(EPOCH FROM updated_at)::bigint,
                       EXTRACT(EPOCH FROM tree_modified_at)::bigint,
                       created_by, updated_by,
                       COUNT(*) OVER() AS total_count
                  FROM storage.folders
                 WHERE parent_id = $1::uuid AND NOT is_trashed
                 ORDER BY name
                 LIMIT $2 OFFSET $3
                "#,
            )
            .bind(pid)
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(self.pool())
            .await
        } else {
            sqlx::query_as(
                r#"
                SELECT id::text, name, path, parent_id::text, drive_id,
                       EXTRACT(EPOCH FROM created_at)::bigint,
                       EXTRACT(EPOCH FROM updated_at)::bigint,
                       EXTRACT(EPOCH FROM tree_modified_at)::bigint,
                       created_by, updated_by,
                       COUNT(*) OVER() AS total_count
                  FROM storage.folders
                 WHERE parent_id IS NULL AND NOT is_trashed
                 ORDER BY name
                 LIMIT $1 OFFSET $2
                "#,
            )
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(self.pool())
            .await
        }
        .map_err(|e| DomainError::internal_error("FolderDb", format!("paginate: {e}")))?;

        // total_count is identical in every row; 0 when the result set is empty.
        let total = if include_total {
            Some(rows.first().map_or(0, |r| r.10) as usize)
        } else {
            None
        };

        let folders: Result<Vec<Folder>, DomainError> = rows
            .into_iter()
            .map(|(id, name, path, pid, did, ca, ma, tma, cb, ub, _total)| {
                Self::row_to_folder(id, name, path, pid, did, ca, ma, tma, cb, ub)
            })
            .collect();
        Ok((folders?, total))
    }

    /// Paginated companion to `list_root_folders_for_caller` — same
    /// drive-membership predicate, adds LIMIT/OFFSET and an optional
    /// window-function COUNT so total pages can be surfaced without a
    /// second round-trip.
    async fn list_root_folders_for_caller_paginated(
        &self,
        caller_id: Uuid,
        offset: usize,
        limit: usize,
        include_total: bool,
    ) -> Result<(Vec<Folder>, Option<usize>), DomainError> {
        let sql = format!(
            "SELECT fo.id::text, fo.name, fo.path, fo.parent_id::text, \
                    fo.drive_id, \
                    EXTRACT(EPOCH FROM fo.created_at)::bigint, \
                    EXTRACT(EPOCH FROM fo.updated_at)::bigint, \
                    EXTRACT(EPOCH FROM fo.tree_modified_at)::bigint, \
                    fo.created_by, fo.updated_by, \
                    COUNT(*) OVER() AS total_count \
               FROM storage.folders fo \
              WHERE fo.parent_id IS NULL \
                AND NOT fo.is_trashed \
                AND {CALLER_CAN_READ_DRIVE} \
              ORDER BY fo.name \
              LIMIT $2 OFFSET $3"
        );
        let rows: Vec<FolderRowPaginated> = sqlx::query_as(&sql)
            .bind(caller_id)
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(self.pool())
            .await
            .map_err(|e| {
                DomainError::internal_error("FolderDb", format!("list_root_folders_paginated: {e}"))
            })?;

        let total = if include_total {
            Some(rows.first().map_or(0, |r| r.10) as usize)
        } else {
            None
        };

        let folders: Result<Vec<Folder>, DomainError> = rows
            .into_iter()
            .map(|(id, name, path, pid, did, ca, ma, tma, cb, ub, _total)| {
                Self::row_to_folder(id, name, path, pid, did, ca, ma, tma, cb, ub)
            })
            .collect();
        Ok((folders?, total))
    }

    async fn rename_folder(
        &self,
        id: &str,
        new_name: String,
        caller_id: Uuid,
    ) -> Result<Folder, DomainError> {
        // The BEFORE UPDATE trigger recomputes path/lpath for this row;
        // the AFTER UPDATE cascade trigger then batch-updates all
        // descendants in a single UPDATE using the GiST lpath index.
        // That multi-row rewrite can deadlock against the tree-ETag
        // flusher's id-ordered ancestor bump — retry instead of failing
        // the user's operation (40P01 only; 23505 still maps below).
        //
        // §14: `updated_by = $3` (caller_id) — the caller mutated this
        // row, not the row's owner. In D2 a shared-drive member can
        // rename a row they don't own; the previous `updated_by = user_id`
        // would have silently recorded the wrong principal.
        let row = retry_on_deadlock("folders.rename", || {
            sqlx::query_as::<_, FolderRow>(
                r#"
                UPDATE storage.folders
                   SET name = $1, updated_at = NOW(), updated_by = $3
                 WHERE id = $2::uuid AND NOT is_trashed
                RETURNING id::text, name, path, parent_id::text, drive_id,
                          EXTRACT(EPOCH FROM created_at)::bigint,
                          EXTRACT(EPOCH FROM updated_at)::bigint,
                           EXTRACT(EPOCH FROM tree_modified_at)::bigint,
                             created_by, updated_by
                "#,
            )
            .bind(&new_name)
            .bind(id)
            .bind(caller_id)
            .fetch_optional(self.pool())
        })
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(ref db_err) = e
                && db_err.code().as_deref() == Some("23505")
            {
                return DomainError::already_exists("Folder", format!("{new_name} already exists"));
            }
            DomainError::internal_error("FolderDb", format!("rename: {e}"))
        })?
        .ok_or_else(|| DomainError::not_found("Folder", id))?;

        Self::row_to_folder(
            row.0, row.1, row.2, row.3, row.4, row.5, row.6, row.7, row.8, row.9,
        )
    }

    async fn move_folder(
        &self,
        id: &str,
        new_parent_id: Option<&str>,
        caller_id: Uuid,
    ) -> Result<Folder, DomainError> {
        // The BEFORE UPDATE trigger recomputes path/lpath for this row;
        // the AFTER UPDATE cascade trigger then batch-updates all
        // descendants in a single UPDATE using the GiST lpath index.
        // Retried on deadlock vs the tree-ETag flusher (see rename_folder).
        //
        // §14: `updated_by = $3` (caller_id), see rename_folder.
        //
        // D6: also sync `drive_id` from the destination parent on
        // cross-drive moves. The CTE-derived `dest.drive_id` is
        // assigned via COALESCE so a root-level move (no destination —
        // `new_parent_id = NULL`) keeps the existing drive_id, mirroring
        // the file move path. The cascade trigger
        // (`cascade_folder_path`) then propagates the new drive_id to
        // every descendant folder + file in the subtree — see
        // `migrations/20260807000000_cascade_drive_id_on_folder_move.sql`.
        let row = retry_on_deadlock("folders.move", || {
            sqlx::query_as::<_, FolderRow>(
                r#"
                WITH dest AS (
                    SELECT drive_id FROM storage.folders WHERE id = $1::uuid
                )
                UPDATE storage.folders f
                   SET parent_id  = $1::uuid,
                       drive_id   = COALESCE((SELECT drive_id FROM dest), f.drive_id),
                       updated_at = NOW(),
                       updated_by = $3
                 WHERE f.id = $2::uuid AND NOT f.is_trashed
                RETURNING f.id::text, f.name, f.path, f.parent_id::text, f.drive_id,
                          EXTRACT(EPOCH FROM f.created_at)::bigint,
                          EXTRACT(EPOCH FROM f.updated_at)::bigint,
                          EXTRACT(EPOCH FROM f.tree_modified_at)::bigint,
                          f.created_by, f.updated_by
                "#,
            )
            .bind(new_parent_id)
            .bind(id)
            .bind(caller_id)
            .fetch_optional(self.pool())
        })
        .await
        .map_err(|e| DomainError::internal_error("FolderDb", format!("move: {e}")))?
        .ok_or_else(|| DomainError::not_found("Folder", id))?;

        Self::row_to_folder(
            row.0, row.1, row.2, row.3, row.4, row.5, row.6, row.7, row.8, row.9,
        )
    }

    async fn delete_folder(&self, id: &str) -> Result<(), DomainError> {
        // Delete all files whose folder is anywhere in the subtree.
        // Uses the GiST-indexed ltree `<@` operator — O(log N) vs the
        // O(depth × N) recursive CTE it replaces.
        // Both statements retried on deadlock vs the tree-ETag flusher's
        // id-ordered ancestor bump (multi-row exclusive locks).
        retry_on_deadlock("folders.delete_files", || {
            sqlx::query(
                "DELETE FROM storage.files \
                  WHERE folder_id IN ( \
                      SELECT id FROM storage.folders \
                       WHERE lpath <@ (SELECT lpath FROM storage.folders WHERE id = $1::uuid) \
                  )",
            )
            .bind(id)
            .execute(self.pool())
        })
        .await
        .map_err(|e| DomainError::internal_error("FolderDb", format!("delete files: {e}")))?;

        // Then delete the folder (CASCADE will remove descendant folders)
        let result = retry_on_deadlock("folders.delete", || {
            sqlx::query("DELETE FROM storage.folders WHERE id = $1::uuid")
                .bind(id)
                .execute(self.pool())
        })
        .await
        .map_err(|e| DomainError::internal_error("FolderDb", format!("delete: {e}")))?;

        if result.rows_affected() == 0 {
            return Err(DomainError::not_found("Folder", id));
        }
        Ok(())
    }

    async fn folder_exists(
        &self,
        storage_path: &StoragePath,
        drive_id: Uuid,
    ) -> Result<bool, DomainError> {
        let path_str = storage_path.to_string();
        let lookup = path_str.strip_prefix('/').unwrap_or(&path_str);

        // Post-D0 `storage.folders.path` repeats across drives —
        // filter by `drive_id` to scope the existence check.
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM storage.folders \
             WHERE path = $1 AND drive_id = $2 AND NOT is_trashed)",
        )
        .bind(lookup)
        .bind(drive_id)
        .fetch_one(self.pool())
        .await
        .map_err(|e| DomainError::internal_error("FolderDb", format!("exists: {e}")))?;

        Ok(exists)
    }

    async fn get_folder_path(&self, id: &str) -> Result<StoragePath, DomainError> {
        let path: String =
            sqlx::query_scalar("SELECT path FROM storage.folders WHERE id = $1::uuid")
                .bind(id)
                .fetch_optional(self.pool())
                .await
                .map_err(|e| DomainError::internal_error("FolderDb", format!("get_path: {e}")))?
                .ok_or_else(|| DomainError::not_found("Folder", id))?;

        Ok(StoragePath::from_string(&path))
    }

    // ── Trash operations ──

    async fn move_to_trash(&self, folder_id: &str, caller_id: Uuid) -> Result<(), DomainError> {
        // Soft-delete the whole subtree in one statement: the root flips
        // `is_trashed` and records `original_parent_id` so restore knows
        // where to put it back; every descendant (folder or file) that
        // wasn't already in trash flips `is_trashed` too but leaves the
        // `original_*` column NULL. That NULL is the marker the restore
        // path uses to tell "cascade-trashed with the root" from
        // "independently trashed earlier" — the latter must stay in
        // trash even when the root is restored.
        //
        // Without this cascade, descendants used to remain `is_trashed = false`
        // and stay directly addressable by their full path (PROPFIND on
        // `/g9-tree/file.txt` still resolved 207 even though the parent
        // collection was gone) — a class of data-integrity drift that
        // confused desktop-sync tree walks.
        //
        // §14: all three CTE branches stamp `updated_by = $2`
        // (caller_id). The cascade is "the caller trashed this
        // subtree", not "each owner trashed their own row".
        let result = retry_on_deadlock("folders.trash", || {
            sqlx::query_scalar::<_, i64>(
                r#"
                WITH trash_root AS (
                    UPDATE storage.folders
                       SET is_trashed = TRUE,
                           trashed_at = NOW(),
                           original_parent_id = parent_id,
                           updated_at = NOW(),
                           updated_by = $2
                     WHERE id = $1::uuid AND NOT is_trashed
                    RETURNING id, lpath
                ),
                trash_descendant_folders AS (
                    UPDATE storage.folders f
                       SET is_trashed = TRUE,
                           trashed_at = NOW(),
                           updated_at = NOW(),
                           updated_by = $2
                      FROM trash_root tr
                     WHERE f.lpath <@ tr.lpath
                       AND f.id != tr.id
                       AND NOT f.is_trashed
                    RETURNING 1
                ),
                trash_descendant_files AS (
                    UPDATE storage.files fi
                       SET is_trashed = TRUE,
                           trashed_at = NOW(),
                           updated_at = NOW(),
                           updated_by = $2
                      FROM trash_root tr
                      JOIN storage.folders f ON f.lpath <@ tr.lpath
                     WHERE fi.folder_id = f.id
                       AND NOT fi.is_trashed
                    RETURNING 1
                )
                SELECT COUNT(*) FROM trash_root
                "#,
            )
            .bind(folder_id)
            .bind(caller_id)
            .fetch_one(self.pool())
        })
        .await
        .map_err(|e| DomainError::internal_error("FolderDb", format!("trash: {e}")))?;

        if result == 0 {
            return Err(DomainError::not_found("Folder", folder_id));
        }

        Ok(())
    }

    async fn restore_from_trash(
        &self,
        folder_id: &str,
        _original_path: &str,
        caller_id: Uuid,
    ) -> Result<(), DomainError> {
        // Inverse of the cascade in `move_to_trash`: restore the root
        // (BEFORE UPDATE trigger recomputes path/lpath via the parent_id
        // change), then un-trash every descendant whose `original_*`
        // column is NULL — those are the rows we cascade-trashed
        // ourselves. Descendants that were independently trashed
        // *before* this folder went to trash have `original_*` set, so
        // they correctly stay in trash and continue to show up as
        // top-level trash entries via `storage.trash_items`.
        //
        // §14: all three CTE branches stamp `updated_by = $2`
        // (caller_id). Restoration is "the caller restored this
        // subtree", regardless of who originally owned each row.
        let result = retry_on_deadlock("folders.restore", || {
            sqlx::query_scalar::<_, i64>(
                r#"
                WITH restore_root AS (
                    UPDATE storage.folders
                       SET is_trashed = FALSE,
                           trashed_at = NULL,
                           parent_id = COALESCE(original_parent_id, parent_id),
                           original_parent_id = NULL,
                           updated_at = NOW(),
                           updated_by = $2
                     WHERE id = $1::uuid AND is_trashed
                    RETURNING id, lpath
                ),
                restore_descendant_folders AS (
                    UPDATE storage.folders f
                       SET is_trashed = FALSE,
                           trashed_at = NULL,
                           updated_at = NOW(),
                           updated_by = $2
                      FROM restore_root rr
                     WHERE f.lpath <@ rr.lpath
                       AND f.id != rr.id
                       AND f.is_trashed
                       AND f.original_parent_id IS NULL
                    RETURNING 1
                ),
                restore_descendant_files AS (
                    UPDATE storage.files fi
                       SET is_trashed = FALSE,
                           trashed_at = NULL,
                           updated_at = NOW(),
                           updated_by = $2
                      FROM restore_root rr
                      JOIN storage.folders f ON f.lpath <@ rr.lpath
                     WHERE fi.folder_id = f.id
                       AND fi.is_trashed
                       AND fi.original_folder_id IS NULL
                    RETURNING 1
                )
                SELECT COUNT(*) FROM restore_root
                "#,
            )
            .bind(folder_id)
            .bind(caller_id)
            .fetch_one(self.pool())
        })
        .await
        .map_err(|e| DomainError::internal_error("FolderDb", format!("restore: {e}")))?;

        if result == 0 {
            return Err(DomainError::not_found("Folder", folder_id));
        }

        Ok(())
    }

    async fn list_file_ids_in_subtree(&self, folder_id: &str) -> Result<Vec<String>, DomainError> {
        // Same GiST subtree predicate the bulk DELETEs in `delete_folder` /
        // `delete_folder_permanently` use — single index scan on
        // `storage.folders.lpath`. Returns the file ids the cascade is
        // about to reap so the caller can fire `on_file_deleted` per id.
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT f.id::text FROM storage.files f \
              WHERE f.folder_id IN ( \
                  SELECT id FROM storage.folders \
                   WHERE lpath <@ (SELECT lpath FROM storage.folders WHERE id = $1::uuid) \
              )",
        )
        .bind(folder_id)
        .fetch_all(self.pool())
        .await
        .map_err(|e| DomainError::internal_error("FolderDb", format!("list subtree files: {e}")))?;
        Ok(rows)
    }

    async fn delete_folder_permanently(&self, folder_id: &str) -> Result<(), DomainError> {
        // Delete all files whose folder is anywhere in the subtree
        // (GiST ltree index, same pattern as delete_folder — both
        // statements retried on deadlock vs the tree-ETag flusher).
        retry_on_deadlock("folders.perm_delete_files", || {
            sqlx::query(
                "DELETE FROM storage.files \
                  WHERE folder_id IN ( \
                      SELECT id FROM storage.folders \
                       WHERE lpath <@ (SELECT lpath FROM storage.folders WHERE id = $1::uuid) \
                  )",
            )
            .bind(folder_id)
            .execute(self.pool())
        })
        .await
        .map_err(|e| DomainError::internal_error("FolderDb", format!("perm delete files: {e}")))?;

        // Then permanently delete folder — CASCADE handles descendant folders
        let result = retry_on_deadlock("folders.perm_delete", || {
            sqlx::query("DELETE FROM storage.folders WHERE id = $1::uuid")
                .bind(folder_id)
                .execute(self.pool())
        })
        .await
        .map_err(|e| DomainError::internal_error("FolderDb", format!("perm delete: {e}")))?;

        if result.rows_affected() == 0 {
            return Err(DomainError::not_found("Folder", folder_id));
        }
        Ok(())
    }

    /// Lists every folder in a subtree rooted at `folder_id` (inclusive).
    ///
    /// Single GiST-indexed query: `fo.lpath <@ (root's lpath)`.
    /// Ordered by `fo.path` so callers can iterate in directory order.
    #[allow(clippy::type_complexity)]
    async fn list_subtree_folders(&self, folder_id: &str) -> Result<Vec<Folder>, DomainError> {
        let sql = "SELECT fo.id::text, fo.name, fo.path, fo.parent_id::text, \
                          fo.drive_id, \
                          EXTRACT(EPOCH FROM fo.created_at)::bigint, \
                          EXTRACT(EPOCH FROM fo.updated_at)::bigint, \
                          EXTRACT(EPOCH FROM fo.tree_modified_at)::bigint, \
                        fo.created_by, fo.updated_by \
                     FROM storage.folders fo \
                    WHERE fo.is_trashed = false \
                      AND fo.lpath <@ (SELECT lpath FROM storage.folders WHERE id = $1::uuid) \
                    ORDER BY fo.path";

        let rows: Vec<FolderRow> = sqlx::query_as(sql)
            .bind(folder_id)
            .fetch_all(self.pool())
            .await
            .map_err(|e| {
                DomainError::internal_error("FolderDb", format!("subtree folders: {e}"))
            })?;

        rows.into_iter()
            .map(|(id, name, path, pid, did, ca, ma, tma, cb, ub)| {
                Self::row_to_folder(id, name, path, pid, did, ca, ma, tma, cb, ub)
            })
            .collect()
    }

    /// SQL-level folder search with name filter, user isolation, and
    /// recursive / non-recursive modes.
    ///
    /// - Non-recursive: `WHERE parent_id = $1 AND user_id = $2 [AND LIKE]`
    /// - Recursive + folder_id: delegates to `list_descendant_folders`
    /// - Recursive + no folder_id: drive-scoped `EXISTS role_grants` [AND LIKE]
    ///
    /// Post-PR-B: filters by drive-membership grants inline (via
    /// `caller_group_ids`) instead of `user_id = $caller`.
    #[allow(clippy::type_complexity)]
    async fn search_folders(
        &self,
        parent_id: Option<&str>,
        name_contains: Option<&str>,
        caller_id: Uuid,
        recursive: bool,
    ) -> Result<Vec<Folder>, DomainError> {
        // Recursive with folder scope → existing optimised ltree scan
        if recursive && let Some(fid) = parent_id {
            return self
                .list_descendant_folders(fid, name_contains, caller_id)
                .await;
        }

        // Build optional name filter — use ILIKE (case-insensitive) so the
        // GIN trigram index idx_folders_name_trgm is used instead of a seq scan.
        let (name_clause, name_pattern) = match name_contains {
            Some(name) if name.len() >= 3 => (
                if recursive {
                    " AND fo.name ILIKE $2"
                } else {
                    " AND fo.name ILIKE $3"
                },
                Some(super::like_escape(name)),
            ),
            _ => ("", None),
        };

        if recursive {
            // Recursive, no folder scope → ALL folders in caller's readable drives
            let sql = format!(
                "SELECT fo.id::text, fo.name, fo.path, fo.parent_id::text, \
                        fo.drive_id, \
                        EXTRACT(EPOCH FROM fo.created_at)::bigint, \
                        EXTRACT(EPOCH FROM fo.updated_at)::bigint, \
                          EXTRACT(EPOCH FROM fo.tree_modified_at)::bigint, \
                      fo.created_by, fo.updated_by \
                   FROM storage.folders fo \
                  WHERE {CALLER_CAN_READ_DRIVE} \
                    AND fo.is_trashed = false \
                    {name_clause} \
                  ORDER BY fo.name"
            );

            let rows: Vec<FolderRow> = if let Some(ref pattern) = name_pattern {
                sqlx::query_as(&sql)
                    .bind(caller_id)
                    .bind(pattern)
                    .fetch_all(self.pool())
                    .await
            } else {
                sqlx::query_as(&sql)
                    .bind(caller_id)
                    .fetch_all(self.pool())
                    .await
            }
            .map_err(|e| DomainError::internal_error("FolderDb", format!("search_folders: {e}")))?;

            return rows
                .into_iter()
                .map(|(id, name, path, pid, did, ca, ma, tma, cb, ub)| {
                    Self::row_to_folder(id, name, path, pid, did, ca, ma, tma, cb, ub)
                })
                .collect();
        }

        // Non-recursive: direct children of parent_id, restricted to drives
        // the caller can read (parent_id already establishes the subtree).
        let sql = if parent_id.is_some() {
            format!(
                "SELECT fo.id::text, fo.name, fo.path, fo.parent_id::text, \
                        fo.drive_id, \
                        EXTRACT(EPOCH FROM fo.created_at)::bigint, \
                        EXTRACT(EPOCH FROM fo.updated_at)::bigint, \
                          EXTRACT(EPOCH FROM fo.tree_modified_at)::bigint, \
                      fo.created_by, fo.updated_by \
                   FROM storage.folders fo \
                  WHERE fo.parent_id = $2::uuid \
                    AND {CALLER_CAN_READ_DRIVE} \
                    AND fo.is_trashed = false \
                    {name_clause} \
                  ORDER BY fo.name"
            )
        } else {
            // Root folders: parent_id IS NULL, params ($1=caller_id, $2=pattern)
            let name_clause_root = match name_contains {
                Some(name) if name.len() >= 3 => " AND fo.name ILIKE $2",
                _ => "",
            };
            format!(
                "SELECT fo.id::text, fo.name, fo.path, fo.parent_id::text, \
                        fo.drive_id, \
                        EXTRACT(EPOCH FROM fo.created_at)::bigint, \
                        EXTRACT(EPOCH FROM fo.updated_at)::bigint, \
                          EXTRACT(EPOCH FROM fo.tree_modified_at)::bigint, \
                      fo.created_by, fo.updated_by \
                   FROM storage.folders fo \
                  WHERE fo.parent_id IS NULL \
                    AND {CALLER_CAN_READ_DRIVE} \
                    AND fo.is_trashed = false \
                    {name_clause_root} \
                  ORDER BY fo.name"
            )
        };

        let rows: Vec<FolderRow> = if let Some(pid) = parent_id {
            if let Some(ref pattern) = name_pattern {
                sqlx::query_as(&sql)
                    .bind(caller_id)
                    .bind(pid)
                    .bind(pattern)
                    .fetch_all(self.pool())
                    .await
            } else {
                sqlx::query_as(&sql)
                    .bind(caller_id)
                    .bind(pid)
                    .fetch_all(self.pool())
                    .await
            }
        } else if let Some(ref pattern) = name_pattern {
            sqlx::query_as(&sql)
                .bind(caller_id)
                .bind(pattern)
                .fetch_all(self.pool())
                .await
        } else {
            sqlx::query_as(&sql)
                .bind(caller_id)
                .fetch_all(self.pool())
                .await
        }
        .map_err(|e| DomainError::internal_error("FolderDb", format!("search_folders: {e}")))?;

        rows.into_iter()
            .map(|(id, name, path, pid, did, ca, ma, tma, cb, ub)| {
                Self::row_to_folder(id, name, path, pid, did, ca, ma, tma, cb, ub)
            })
            .collect()
    }

    /// Lists all descendant folders in a subtree using ltree GiST index,
    /// scoped to drives the caller can read.
    ///
    /// Single SQL query: `fo.lpath <@ (root's lpath)` fetches the entire
    /// subtree in one indexed scan. Post-PR-B: drive-membership filter
    /// inline via `caller_group_ids`, replacing the legacy
    /// `fo.user_id = $caller` predicate.
    #[allow(clippy::type_complexity)]
    async fn list_descendant_folders(
        &self,
        folder_id: &str,
        name_contains: Option<&str>,
        caller_id: Uuid,
    ) -> Result<Vec<Folder>, DomainError> {
        let (where_extra, name_pattern) = match name_contains {
            Some(name) if name.len() >= 3 => {
                (" AND fo.name ILIKE $3", Some(super::like_escape(name)))
            }
            _ => ("", None),
        };

        let sql = format!(
            "SELECT fo.id::text, fo.name, fo.path, fo.parent_id::text, \
                    fo.drive_id, \
                    EXTRACT(EPOCH FROM fo.created_at)::bigint, \
                    EXTRACT(EPOCH FROM fo.updated_at)::bigint, \
                    EXTRACT(EPOCH FROM fo.tree_modified_at)::bigint, \
                    fo.created_by, fo.updated_by \
               FROM storage.folders fo \
              WHERE {CALLER_CAN_READ_DRIVE} \
                AND fo.is_trashed = false \
                AND fo.lpath <@ (SELECT lpath FROM storage.folders WHERE id = $2::uuid) \
                AND fo.id != $2::uuid \
                {where_extra} \
              ORDER BY fo.name"
        );

        let rows: Vec<FolderRow> = if let Some(ref pattern) = name_pattern {
            sqlx::query_as(&sql)
                .bind(caller_id)
                .bind(folder_id)
                .bind(pattern)
                .fetch_all(self.pool())
                .await
        } else {
            sqlx::query_as(&sql)
                .bind(caller_id)
                .bind(folder_id)
                .fetch_all(self.pool())
                .await
        }
        .map_err(|e| DomainError::internal_error("FolderDb", format!("descendant search: {e}")))?;

        rows.into_iter()
            .map(|(id, name, path, pid, did, ca, ma, tma, cb, ub)| {
                Self::row_to_folder(id, name, path, pid, did, ca, ma, tma, cb, ub)
            })
            .collect()
    }

    #[allow(clippy::type_complexity)]
    async fn suggest_folders_by_name(
        &self,
        parent_id: Option<&str>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<Folder>, DomainError> {
        let pattern = super::like_escape(query);
        let limit_i64 = limit as i64;

        let rows: Vec<FolderRow> = if let Some(pid) = parent_id {
            sqlx::query_as(
                r#"
                SELECT id::text, name, path, parent_id::text, drive_id,
                       EXTRACT(EPOCH FROM created_at)::bigint,
                       EXTRACT(EPOCH FROM updated_at)::bigint,
                       EXTRACT(EPOCH FROM tree_modified_at)::bigint,
                         created_by, updated_by
                  FROM storage.folders
                 WHERE parent_id = $1::uuid
                   AND NOT is_trashed
                   AND name ILIKE $2
                 ORDER BY CASE
                            WHEN name ILIKE $3 THEN 0
                            WHEN name ILIKE $3 || '%' THEN 1
                            ELSE 2
                          END,
                          name
                 LIMIT $4
                "#,
            )
            .bind(pid)
            .bind(&pattern)
            .bind(query)
            .bind(limit_i64)
            .fetch_all(self.pool())
            .await
        } else {
            sqlx::query_as(
                r#"
                SELECT id::text, name, path, parent_id::text, drive_id,
                       EXTRACT(EPOCH FROM created_at)::bigint,
                       EXTRACT(EPOCH FROM updated_at)::bigint,
                       EXTRACT(EPOCH FROM tree_modified_at)::bigint,
                         created_by, updated_by
                  FROM storage.folders
                 WHERE parent_id IS NULL
                   AND NOT is_trashed
                   AND name ILIKE $1
                 ORDER BY CASE
                            WHEN name ILIKE $2 THEN 0
                            WHEN name ILIKE $2 || '%' THEN 1
                            ELSE 2
                          END,
                          name
                 LIMIT $3
                "#,
            )
            .bind(&pattern)
            .bind(query)
            .bind(limit_i64)
            .fetch_all(self.pool())
            .await
        }
        .map_err(|e| DomainError::internal_error("FolderDb", format!("suggest: {e}")))?;

        rows.into_iter()
            .map(|(id, name, path, pid, did, ca, ma, tma, cb, ub)| {
                Self::row_to_folder(id, name, path, pid, did, ca, ma, tma, cb, ub)
            })
            .collect()
    }

    async fn is_folder_in_subtree(
        &self,
        candidate_folder_id: &str,
        root_folder_id: &str,
    ) -> Result<bool, DomainError> {
        let (Ok(candidate_uuid), Ok(root_uuid)) = (
            Uuid::parse_str(candidate_folder_id),
            Uuid::parse_str(root_folder_id),
        ) else {
            return Ok(false);
        };

        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (\
                 SELECT 1 \
                 FROM storage.folders c, storage.folders r \
                 WHERE c.id = $1 \
                   AND r.id = $2 \
                   AND c.is_trashed = false \
                   AND r.is_trashed = false \
                   AND c.lpath <@ r.lpath \
             )",
        )
        .bind(candidate_uuid)
        .bind(root_uuid)
        .fetch_one(self.pool())
        .await
        .map_err(|e| {
            DomainError::internal_error("FolderDb", format!("is_folder_in_subtree: {e}"))
        })?;
        Ok(exists)
    }

    async fn is_file_in_subtree(
        &self,
        file_id: &str,
        root_folder_id: &str,
    ) -> Result<bool, DomainError> {
        let (Ok(file_uuid), Ok(root_uuid)) =
            (Uuid::parse_str(file_id), Uuid::parse_str(root_folder_id))
        else {
            return Ok(false);
        };

        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (\
                 SELECT 1 \
                 FROM storage.files f \
                 JOIN storage.folders parent ON f.folder_id = parent.id \
                 JOIN storage.folders root   ON root.id = $2 \
                 WHERE f.id = $1 \
                   AND f.is_trashed = false \
                   AND parent.is_trashed = false \
                   AND root.is_trashed = false \
                   AND parent.lpath <@ root.lpath \
             )",
        )
        .bind(file_uuid)
        .bind(root_uuid)
        .fetch_one(self.pool())
        .await
        .map_err(|e| DomainError::internal_error("FolderDb", format!("is_file_in_subtree: {e}")))?;
        Ok(exists)
    }
}

// ── Extra helpers for blob-storage bootstrap ──

impl FolderDbRepository {
    /// Returns `drive_id` for a given folder. Drives the new permission-floor
    /// short-circuit in `PgAclEngine::check_inner` (a caller with any role
    /// on the folder's drive automatically passes the check — drive
    /// membership is the baseline floor per `drive.md §5`).
    pub async fn get_folder_drive_id(&self, folder_id: &str) -> Result<Uuid, DomainError> {
        sqlx::query_scalar::<_, Uuid>("SELECT drive_id FROM storage.folders WHERE id = $1::uuid")
            .bind(folder_id)
            .fetch_optional(self.pool())
            .await
            .map_err(|e| DomainError::internal_error("FolderDb", format!("drive_id lookup: {e}")))?
            .ok_or_else(|| DomainError::not_found("Folder", folder_id))
    }

    /// Cursor-paginated combined listing of sub-folders and files inside
    /// `parent_id`, sorted by `order_by`.
    ///
    /// **Authorization must be verified by the caller** before invoking this
    /// method — no ownership filter is applied here.
    ///
    /// Fetches `limit` rows (caller should pass `desired_page_size + 1` to
    /// detect the existence of a next page).  Returns raw [`FolderResourceRow`]
    /// values; the handler / service layer converts them to DTOs.
    pub async fn list_resources_paged(
        &self,
        parent_id: Uuid,
        limit: usize,
        cursor: Option<&FolderResourceCursor>,
        order_by: &str,
        kinds: Option<&[ResourceKind]>,
        reverse: bool,
    ) -> Result<Vec<FolderResourceRow>, DomainError> {
        let include_folders = kinds.is_none_or(|k| k.contains(&ResourceKind::Folder));
        let include_files = kinds.is_none_or(|k| k.contains(&ResourceKind::File));

        if !include_folders && !include_files {
            return Ok(Vec::new());
        }

        // ── CTE branches ────────────────────────────────────────────────────
        let folder_branch = r#"
            SELECT
                'folder'::text            AS resource_type,
                f.id,
                f.name,
                f.parent_id               AS folder_id,
                NULL::text                AS mime_type,
                -1::bigint                AS size,
                f.created_at,
                f.updated_at              AS modified_at,
                f.drive_id,
                NULL::text                AS blob_hash,
                LOWER(f.name)             AS sort_str,
                0::bigint                 AS type_order,
                0::int                    AS folder_first
            FROM storage.folders f
            WHERE f.parent_id = $1::uuid AND NOT f.is_trashed
        "#;

        let file_branch = r#"
            SELECT
                'file'::text              AS resource_type,
                fm.id,
                fm.name,
                fm.folder_id,
                fm.mime_type,
                fm.size::bigint,
                fm.created_at,
                fm.updated_at             AS modified_at,
                fm.drive_id,
                fm.blob_hash,
                LOWER(fm.name)            AS sort_str,
                fm.category_order::bigint AS type_order,
                1::int                    AS folder_first
            FROM storage.files fm
            WHERE fm.folder_id = $1::uuid AND NOT fm.is_trashed
        "#;

        let cte_inner = match (include_folders, include_files) {
            (true, true) => format!("{folder_branch} UNION ALL {file_branch}"),
            (true, false) => folder_branch.to_owned(),
            (false, true) => file_branch.to_owned(),
            (false, false) => unreachable!(),
        };

        // ── Cursor binds ─────────────────────────────────────────────────────
        // $1 = parent_id   $2 = cursor_str   $3 = cursor_int
        // $4 = cursor_ts   $5 = cursor_id    $6 = limit
        let cursor_str = cursor.and_then(|c| c.sort_str.clone());
        let cursor_int = cursor.and_then(|c| c.sort_int);
        let cursor_ts = cursor.and_then(|c| c.sort_ts);
        let cursor_id = cursor.map(|c| c.resource_id);

        // ── Sort-specific WHERE + ORDER BY ───────────────────────────────────
        // Each arm produces two variants based on `reverse`.
        // For "name": folder_first stays ASC in both directions (folders always
        // precede files); only the alpha order within each group flips.
        let (where_clause, order_clause) = match order_by {
            "type" => {
                if reverse {
                    (
                        r#"WHERE ($3::bigint IS NULL)
                              OR (type_order < $3)
                              OR (type_order = $3 AND sort_str < $2)
                              OR (type_order = $3 AND sort_str = $2 AND id < $5::uuid)"#,
                        "ORDER BY type_order DESC, sort_str DESC, id DESC",
                    )
                } else {
                    (
                        r#"WHERE ($3::bigint IS NULL)
                              OR (type_order > $3)
                              OR (type_order = $3 AND sort_str > $2)
                              OR (type_order = $3 AND sort_str = $2 AND id > $5::uuid)"#,
                        "ORDER BY type_order ASC, sort_str ASC, id ASC",
                    )
                }
            }
            "modified_at" => {
                if reverse {
                    (
                        r#"WHERE ($4::timestamptz IS NULL)
                              OR (modified_at > $4)
                              OR (modified_at = $4 AND id > $5::uuid)"#,
                        "ORDER BY modified_at ASC, id ASC",
                    )
                } else {
                    (
                        r#"WHERE ($4::timestamptz IS NULL)
                              OR (modified_at < $4)
                              OR (modified_at = $4 AND id < $5::uuid)"#,
                        "ORDER BY modified_at DESC, id DESC",
                    )
                }
            }
            "created_at" => {
                if reverse {
                    (
                        r#"WHERE ($4::timestamptz IS NULL)
                              OR (created_at > $4)
                              OR (created_at = $4 AND id > $5::uuid)"#,
                        "ORDER BY created_at ASC, id ASC",
                    )
                } else {
                    (
                        r#"WHERE ($4::timestamptz IS NULL)
                              OR (created_at < $4)
                              OR (created_at = $4 AND id < $5::uuid)"#,
                        "ORDER BY created_at DESC, id DESC",
                    )
                }
            }
            "size" => {
                if reverse {
                    (
                        r#"WHERE ($3::bigint IS NULL)
                              OR (size < $3)
                              OR (size = $3 AND id < $5::uuid)"#,
                        "ORDER BY size DESC, id DESC",
                    )
                } else {
                    (
                        r#"WHERE ($3::bigint IS NULL)
                              OR (size > $3)
                              OR (size = $3 AND id > $5::uuid)"#,
                        "ORDER BY size ASC, id ASC",
                    )
                }
            }
            _ => {
                // "name" (default): folder_first stays ASC so folders always precede
                // files; only the alpha order within each group flips when reversed.
                if reverse {
                    (
                        r#"WHERE ($3::bigint IS NULL)
                              OR (folder_first::bigint > $3)
                              OR (folder_first::bigint = $3 AND sort_str < $2)
                              OR (folder_first::bigint = $3 AND sort_str = $2 AND id < $5::uuid)"#,
                        "ORDER BY folder_first ASC, sort_str DESC, id DESC",
                    )
                } else {
                    (
                        r#"WHERE ($3::bigint IS NULL)
                              OR (folder_first::bigint > $3)
                              OR (folder_first::bigint = $3 AND sort_str > $2)
                              OR (folder_first::bigint = $3 AND sort_str = $2 AND id > $5::uuid)"#,
                        "ORDER BY folder_first ASC, sort_str ASC, id ASC",
                    )
                }
            }
        };

        let sql = format!(
            "WITH resources AS ({cte_inner}) \
             SELECT resource_type, id, name, folder_id, mime_type, size, \
                    created_at, modified_at, drive_id, blob_hash, \
                    sort_str, type_order, folder_first \
             FROM resources \
             {where_clause} \
             {order_clause} \
             LIMIT $6"
        );

        // Row: (resource_type, id, name, folder_id, mime_type, size,
        //        created_at, modified_at, drive_id, blob_hash,
        //        sort_str, type_order, folder_first)
        type Row = (
            String,
            Uuid,
            String,
            Option<Uuid>,
            Option<String>,
            i64,
            chrono::DateTime<chrono::Utc>,
            chrono::DateTime<chrono::Utc>,
            Uuid, // drive_id
            Option<String>,
            String,
            i64,
            i32,
        );

        let rows = sqlx::query_as::<_, Row>(&sql)
            .bind(parent_id)
            .bind(cursor_str)
            .bind(cursor_int)
            .bind(cursor_ts)
            .bind(cursor_id)
            .bind(limit as i64)
            .fetch_all(self.pool())
            .await
            .map_err(|e| {
                DomainError::internal_error("FolderDb", format!("list_resources_paged: {e}"))
            })?;

        Ok(rows
            .into_iter()
            .map(|r| FolderResourceRow {
                resource_type: r.0,
                id: r.1,
                name: r.2,
                parent_id: r.3,
                mime_type: r.4,
                size: r.5,
                created_at: r.6,
                modified_at: r.7,
                drive_id: r.8,
                blob_hash: r.9,
                sort_str: r.10,
                type_order: r.11,
                folder_first: r.12,
            })
            .collect())
    }
}
