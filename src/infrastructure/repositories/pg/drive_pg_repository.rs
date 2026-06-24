//! PostgreSQL implementation of [`DriveRepository`].
//!
//! The repo deals only with the `storage.drives` table itself. Drive
//! membership lives in `storage.role_grants` (`resource_type='drive'`)
//! and is queried through the engine's existing grant paths;
//! `list_for_subjects` below resolves `role_grants` → `storage.drives`
//! via a single join.
//!
//! See `migrations/20260802000000_drives_schema_additive.sql` for the
//! schema and `docs/plan/drive.md` §3 / §15 for the locked design.

use std::sync::Arc;

use sqlx::{PgPool, Row, types::Uuid};

use crate::domain::entities::drive::{Drive, DriveKind};
use crate::domain::repositories::drive_repository::{
    DriveRepository, DriveRepositoryError, DriveWithRootName,
};

pub struct DrivePgRepository {
    pool: Arc<PgPool>,
}

impl DrivePgRepository {
    pub fn new(pool: Arc<PgPool>) -> Self {
        Self { pool }
    }

    fn map_sqlx_err(context: &'static str, e: sqlx::Error) -> DriveRepositoryError {
        if let sqlx::Error::Database(ref dberr) = e
            && let Some(code) = dberr.code()
            && code.as_ref() == "23505"
        {
            // unique_violation. With drives, the only relevant unique is
            // the partial index `idx_drives_default_for_user_unique` —
            // surface the typed variant so the lifecycle hook can detect
            // idempotent re-runs (D0-9 calls create_personal_drive_atomic
            // during user provisioning).
            return DriveRepositoryError::DefaultDriveAlreadyExists(dberr.to_string());
        }
        DriveRepositoryError::StorageError(format!("{context}: {e}"))
    }

    /// Map a row carrying both the drive's columns AND a `root_folder_name`
    /// column (sourced via JOIN with `storage.folders`) into the view-model.
    /// `caller_role` is left `None` — only the listing path (which
    /// JOINs `role_grants` for accessibility) has it in scope; see
    /// `row_to_drive_with_name_and_role`.
    fn row_to_drive_with_name(
        row: &sqlx::postgres::PgRow,
    ) -> Result<DriveWithRootName, DriveRepositoryError> {
        let kind_str: String = row.get("kind");
        let kind = DriveKind::from_sql(&kind_str)?;
        let drive = Drive {
            id: row.get("id"),
            kind,
            default_for_user: row.get("default_for_user"),
            root_folder_id: row.get("root_folder_id"),
            quota_bytes: row.get("quota_bytes"),
            used_bytes: row.get("used_bytes"),
            policies: row.get("policies"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        };
        Ok(DriveWithRootName {
            drive,
            root_folder_name: row.get("root_folder_name"),
            caller_role: None,
        })
    }

    /// Same as `row_to_drive_with_name` but reads `caller_role` from the
    /// listing query — `MIN(g.role)::text`. The `storage.grant_role` ENUM
    /// is declared owner→viewer (strongest→weakest), so `MIN` picks the
    /// strongest of the caller's grants on the drive (direct +
    /// group-mediated collapsed by GROUP BY). Used only by
    /// `list_for_subjects`.
    fn row_to_drive_with_name_and_role(
        row: &sqlx::postgres::PgRow,
    ) -> Result<DriveWithRootName, DriveRepositoryError> {
        use crate::domain::services::authorization::Role;
        let mut dwr = Self::row_to_drive_with_name(row)?;
        let role_str: Option<String> = row.try_get("caller_role").ok();
        dwr.caller_role = role_str.as_deref().and_then(Role::parse);
        Ok(dwr)
    }
}

#[async_trait::async_trait]
impl DriveRepository for DrivePgRepository {
    async fn create_personal_drive_atomic(
        &self,
        owner_id: Uuid,
        quota_bytes: Option<i64>,
    ) -> Result<DriveWithRootName, DriveRepositoryError> {
        // Four writes wrapped in a single transaction so either all
        // commit or none does (docs/plan/drive.md §3). A single CTE
        // statement would be cleaner on paper but doesn't work in
        // PostgreSQL: CTE sub-statements share an MVCC snapshot, so
        // `UPDATE storage.drives WHERE id = …` cannot match a row
        // inserted by an earlier CTE branch. We use plain sequential
        // statements inside `pool.begin()` instead — each statement
        // sees the prior ones' writes (transaction-local visibility),
        // and FK constraints are satisfied at insert time because the
        // referenced rows already exist.
        //
        // Rollback semantics: any error before `tx.commit()` (FK
        // violation, unique_violation on `default_for_user`, server
        // crash) discards every partial write. No orphan drive, no
        // folder without a drive, no drive without an owner.
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| Self::map_sqlx_err("create_personal_drive_atomic.begin", e))?;

        // 1. Drive row (root_folder_id NULL — populated in step 3).
        let drive_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO storage.drives
                (kind, default_for_user, quota_bytes, policies)
            VALUES ('personal', $1, $2, '{}'::jsonb)
            RETURNING id
            "#,
        )
        .bind(owner_id)
        .bind(quota_bytes)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| Self::map_sqlx_err("create_personal_drive_atomic.drive", e))?;

        // 2. Root folder. `parent_id IS NULL` makes it a root in the
        //    drive; `drive_id` closes the FK in this direction.
        let folder_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO storage.folders
                (name, parent_id, user_id, drive_id, created_by, updated_by)
            VALUES ('Personal', NULL, $1, $2, $1, $1)
            RETURNING id
            "#,
        )
        .bind(owner_id)
        .bind(drive_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| Self::map_sqlx_err("create_personal_drive_atomic.folder", e))?;

        // 3. Close the other side of the circular reference.
        sqlx::query(r#"UPDATE storage.drives SET root_folder_id = $1 WHERE id = $2"#)
            .bind(folder_id)
            .bind(drive_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| Self::map_sqlx_err("create_personal_drive_atomic.wire", e))?;

        // 4. Owner role_grant — the caller becomes the drive's sole
        //    owner (single-user invariant on personal drives, §2).
        sqlx::query(
            r#"
            INSERT INTO storage.role_grants
                (subject_type, subject_id, resource_type, resource_id,
                 role, granted_by)
            VALUES ('user', $1, 'drive', $2, 'owner', $1)
            "#,
        )
        .bind(owner_id)
        .bind(drive_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| Self::map_sqlx_err("create_personal_drive_atomic.grant", e))?;

        // Fetch the row in its final state so the caller gets a
        // consistent view (including DB-computed defaults like
        // `created_at`, `used_bytes`).
        let row = sqlx::query(
            r#"
            SELECT d.id, d.kind, d.default_for_user, d.root_folder_id,
                   d.quota_bytes, d.used_bytes, d.policies,
                   d.created_at, d.updated_at,
                   f.name AS root_folder_name
              FROM storage.drives d
              JOIN storage.folders f ON f.id = d.root_folder_id
             WHERE d.id = $1
            "#,
        )
        .bind(drive_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| Self::map_sqlx_err("create_personal_drive_atomic.read", e))?;

        tx.commit()
            .await
            .map_err(|e| Self::map_sqlx_err("create_personal_drive_atomic.commit", e))?;

        Self::row_to_drive_with_name(&row)
    }

    async fn create_shared_drive_atomic(
        &self,
        name: &str,
        owner_subject: crate::domain::services::authorization::Subject,
        quota_bytes: Option<i64>,
        granted_by: Uuid,
    ) -> Result<DriveWithRootName, DriveRepositoryError> {
        // Same four-write transaction shape as `create_personal_drive_atomic`
        // (see that method for the why-not-CTE explanation). Differences:
        //   - `kind='shared'`, `default_for_user=NULL`.
        //   - Root folder name is caller-supplied.
        //   - Owner grant subject is caller-supplied — either a single
        //     User (becomes the sole drive Owner) or a Group (transitive
        //     members inherit Owner via subject expansion).
        //   - `granted_by` is the OxiCloud admin who provisioned the drive;
        //     same value goes onto the folder's `created_by`/`updated_by`
        //     for §14 provenance.
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| Self::map_sqlx_err("create_shared_drive_atomic.begin", e))?;

        // 1. Drive row (root_folder_id NULL — populated in step 3).
        let drive_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO storage.drives
                (kind, default_for_user, quota_bytes, policies)
            VALUES ('shared', NULL, $1, '{}'::jsonb)
            RETURNING id
            "#,
        )
        .bind(quota_bytes)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| Self::map_sqlx_err("create_shared_drive_atomic.drive", e))?;

        // 2. Root folder. The folder's `user_id` carries the admin (legacy
        //    column still NOT NULL during the dual-write window — D7
        //    drops it once `drive_id` is the canonical ownership signal).
        let folder_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO storage.folders
                (name, parent_id, user_id, drive_id, created_by, updated_by)
            VALUES ($1, NULL, $2, $3, $2, $2)
            RETURNING id
            "#,
        )
        .bind(name)
        .bind(granted_by)
        .bind(drive_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| Self::map_sqlx_err("create_shared_drive_atomic.folder", e))?;

        // 3. Close the circular reference (drive ↔ root folder).
        sqlx::query(r#"UPDATE storage.drives SET root_folder_id = $1 WHERE id = $2"#)
            .bind(folder_id)
            .bind(drive_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| Self::map_sqlx_err("create_shared_drive_atomic.wire", e))?;

        // 4. Owner role_grant — subject_type chosen from the caller's input.
        //    Group subjects expand transitively via `subject_match_set` so
        //    every member inherits Owner; User subjects are the single
        //    admin case.
        sqlx::query(
            r#"
            INSERT INTO storage.role_grants
                (subject_type, subject_id, resource_type, resource_id,
                 role, granted_by)
            VALUES ($1, $2, 'drive', $3, 'owner', $4)
            "#,
        )
        .bind(owner_subject.type_str())
        .bind(owner_subject.id())
        .bind(drive_id)
        .bind(granted_by)
        .execute(&mut *tx)
        .await
        .map_err(|e| Self::map_sqlx_err("create_shared_drive_atomic.grant", e))?;

        // Fetch final state so the caller sees DB-computed defaults.
        let row = sqlx::query(
            r#"
            SELECT d.id, d.kind, d.default_for_user, d.root_folder_id,
                   d.quota_bytes, d.used_bytes, d.policies,
                   d.created_at, d.updated_at,
                   f.name AS root_folder_name
              FROM storage.drives d
              JOIN storage.folders f ON f.id = d.root_folder_id
             WHERE d.id = $1
            "#,
        )
        .bind(drive_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| Self::map_sqlx_err("create_shared_drive_atomic.read", e))?;

        tx.commit()
            .await
            .map_err(|e| Self::map_sqlx_err("create_shared_drive_atomic.commit", e))?;

        Self::row_to_drive_with_name(&row)
    }

    async fn get_by_id(&self, id: Uuid) -> Result<DriveWithRootName, DriveRepositoryError> {
        let row = sqlx::query(
            r#"
            SELECT d.id, d.kind, d.default_for_user, d.root_folder_id,
                   d.quota_bytes, d.used_bytes, d.policies,
                   d.created_at, d.updated_at,
                   f.name AS root_folder_name
              FROM storage.drives d
              JOIN storage.folders f ON f.id = d.root_folder_id
             WHERE d.id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(self.pool.as_ref())
        .await
        .map_err(|e| Self::map_sqlx_err("get_by_id", e))?
        .ok_or_else(|| DriveRepositoryError::NotFound(id.to_string()))?;

        Self::row_to_drive_with_name(&row)
    }

    async fn get_by_ids(
        &self,
        ids: &[Uuid],
    ) -> Result<Vec<DriveWithRootName>, DriveRepositoryError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            r#"
            SELECT d.id, d.kind, d.default_for_user, d.root_folder_id,
                   d.quota_bytes, d.used_bytes, d.policies,
                   d.created_at, d.updated_at,
                   f.name AS root_folder_name
              FROM storage.drives d
              JOIN storage.folders f ON f.id = d.root_folder_id
             WHERE d.id = ANY($1)
            "#,
        )
        .bind(ids)
        .fetch_all(self.pool.as_ref())
        .await
        .map_err(|e| Self::map_sqlx_err("get_by_ids", e))?;

        rows.iter().map(Self::row_to_drive_with_name).collect()
    }

    async fn find_default_for_user(
        &self,
        user_id: Uuid,
    ) -> Result<DriveWithRootName, DriveRepositoryError> {
        let row = sqlx::query(
            r#"
            SELECT d.id, d.kind, d.default_for_user, d.root_folder_id,
                   d.quota_bytes, d.used_bytes, d.policies,
                   d.created_at, d.updated_at,
                   f.name AS root_folder_name
              FROM storage.drives d
              JOIN storage.folders f ON f.id = d.root_folder_id
             WHERE d.default_for_user = $1
            "#,
        )
        .bind(user_id)
        .fetch_optional(self.pool.as_ref())
        .await
        .map_err(|e| Self::map_sqlx_err("find_default_for_user", e))?
        .ok_or_else(|| DriveRepositoryError::NotFound(user_id.to_string()))?;

        Self::row_to_drive_with_name(&row)
    }

    async fn list_for_subjects(
        &self,
        subject_types: &[&str],
        subject_ids: &[Uuid],
    ) -> Result<Vec<DriveWithRootName>, DriveRepositoryError> {
        // Joining role_grants → drives → folders returns every drive the
        // expanded subject set can read, paired with its display name.
        // ORDER BY puts default drives first (so the picker UI doesn't
        // need a follow-up sort), then alphabetical by name. GROUP BY
        // collapses duplicate role_grants on the same drive (direct +
        // group-mediated) and sidesteps PostgreSQL's "ORDER BY
        // expression must appear in select list" rule that SELECT
        // DISTINCT imposes.
        // `MIN(g.role)` picks the caller's strongest role on each drive:
        // `storage.grant_role` is declared `owner → viewer` (strongest →
        // weakest), so MIN returns the strongest. Cast `::text` matches
        // the codebase convention for reading enum columns into Rust
        // (see `pg_acl_engine.rs`); `Role::parse` handles the trip back.
        // Collapses direct + group-mediated grants on the same drive
        // into one row alongside the existing GROUP BY.
        let rows = sqlx::query(
            r#"
            SELECT d.id, d.kind, d.default_for_user, d.root_folder_id,
                   d.quota_bytes, d.used_bytes, d.policies,
                   d.created_at, d.updated_at,
                   f.name AS root_folder_name,
                   MIN(g.role)::text AS caller_role
              FROM storage.drives d
              JOIN storage.folders f ON f.id = d.root_folder_id
              JOIN storage.role_grants g
                ON g.resource_type = 'drive'
               AND g.resource_id   = d.id
             WHERE g.subject_type = ANY($1)
               AND g.subject_id   = ANY($2)
               AND (g.expires_at IS NULL OR g.expires_at > NOW())
             GROUP BY d.id, d.kind, d.default_for_user, d.root_folder_id,
                      d.quota_bytes, d.used_bytes, d.policies,
                      d.created_at, d.updated_at, f.name
             ORDER BY (d.default_for_user IS NULL) ASC,
                      LOWER(f.name) ASC
            "#,
        )
        .bind(
            subject_types
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        )
        .bind(subject_ids)
        .fetch_all(self.pool.as_ref())
        .await
        .map_err(|e| Self::map_sqlx_err("list_for_subjects", e))?;

        rows.iter()
            .map(Self::row_to_drive_with_name_and_role)
            .collect()
    }

    async fn list_all(&self) -> Result<Vec<DriveWithRootName>, DriveRepositoryError> {
        // No subject filter: every drive on the system. The HTTP layer
        // (admin guard on `/api/admin/drives`) is the access control —
        // adding a role filter here would defeat the point of the
        // endpoint (an admin without explicit membership wouldn't see
        // the drives they created for other users).
        let rows = sqlx::query(
            r#"
            SELECT d.id, d.kind, d.default_for_user, d.root_folder_id,
                   d.quota_bytes, d.used_bytes, d.policies,
                   d.created_at, d.updated_at,
                   f.name AS root_folder_name
              FROM storage.drives d
              JOIN storage.folders f ON f.id = d.root_folder_id
             ORDER BY LOWER(f.name) ASC
            "#,
        )
        .fetch_all(self.pool.as_ref())
        .await
        .map_err(|e| Self::map_sqlx_err("list_all", e))?;

        rows.iter().map(Self::row_to_drive_with_name).collect()
    }
}
