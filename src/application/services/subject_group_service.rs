//! Subject group application service.
//!
//! Orchestrates CRUD and membership for ReBAC subject groups on top of the
//! `SubjectGroupRepository`. This is where:
//!   - Name validation runs (defence-in-depth alongside the DB CHECK).
//!   - Virtual groups (e.g. `Internal`) are protected from mutation.
//!   - Audit events are emitted via `tracing::info!(target = "audit", ...)`.
//!   - Cascading delete of `storage.access_grants` rows referencing this
//!     group runs in the same transaction as the group delete.
//!
//! See `migrations/20260612000000_subject_groups.sql` for the schema.

use std::collections::HashSet;
use std::sync::Arc;

use sqlx::PgPool;
use uuid::Uuid;

use crate::common::errors::{DomainError, ErrorKind};
use crate::domain::entities::subject_group::{
    GroupMember, INTERNAL_GROUP_ID, SubjectGroup, SubjectGroupError,
};
use crate::domain::repositories::subject_group_repository::{
    SubjectGroupRepository, SubjectGroupRepositoryError,
};
use crate::infrastructure::repositories::pg::SubjectGroupPgRepository;

pub struct SubjectGroupService {
    repo: Arc<SubjectGroupPgRepository>,
    pool: Arc<PgPool>,
}

impl SubjectGroupService {
    pub fn new(repo: Arc<SubjectGroupPgRepository>, pool: Arc<PgPool>) -> Self {
        Self { repo, pool }
    }

    /// Create a new group. Validates the name (RFC 5321 local-part shape)
    /// at the domain layer before the round-trip; the DB CHECK constraint
    /// is the ultimate authority.
    pub async fn create(
        &self,
        name: &str,
        description: Option<String>,
        caller_id: Uuid,
    ) -> Result<SubjectGroup, DomainError> {
        let group = SubjectGroup::new(name, description).map_err(map_entity_err)?;
        let saved = self.repo.create(&group).await.map_err(map_repo_err)?;

        tracing::info!(
            target: "audit",
            event = "group.created",
            group_id = %saved.id,
            name = %saved.name,
            created_by = %caller_id,
        );

        Ok(saved)
    }

    pub async fn get_by_id(&self, id: Uuid) -> Result<SubjectGroup, DomainError> {
        match self.repo.get_by_id(id).await.map_err(map_repo_err)? {
            Some(g) => Ok(g),
            None => Err(DomainError::new(
                ErrorKind::NotFound,
                "SubjectGroup",
                format!("group {} not found", id),
            )),
        }
    }

    pub async fn list(
        &self,
        limit: u32,
        offset: u32,
        name_query: Option<&str>,
    ) -> Result<(Vec<SubjectGroup>, u64), DomainError> {
        self.repo
            .list(limit, offset, name_query)
            .await
            .map_err(map_repo_err)
    }

    /// Same as `list`, with the direct-member count attached to each row.
    /// Used by the management UI; the share-dialog search path stays on the
    /// lighter `search_for_share` which doesn't need counts.
    pub async fn list_with_counts(
        &self,
        limit: u32,
        offset: u32,
        name_query: Option<&str>,
    ) -> Result<(Vec<(SubjectGroup, i64)>, u64), DomainError> {
        self.repo
            .list_with_counts(limit, offset, name_query)
            .await
            .map_err(map_repo_err)
    }

    /// Direct-member count for a single group. Cheap (one `COUNT(*)`); used
    /// by create / get / update endpoints so the response DTO carries the
    /// same `member_count` field as the list view.
    pub async fn count_members(&self, id: Uuid) -> Result<i64, DomainError> {
        self.repo.count_members(id).await.map_err(map_repo_err)
    }

    /// Search by name prefix/substring. Virtual groups (Internal, plus any
    /// future predefined entries) are included so the share-dialog
    /// autocomplete picks them up automatically — no frontend change is
    /// needed when a new virtual group is added server-side. The repository
    /// returns virtual groups first so they're discoverable when the query
    /// is empty / short.
    pub async fn search_for_share(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<SubjectGroup>, DomainError> {
        let (rows, _total) = self
            .repo
            .list(limit, 0, Some(query))
            .await
            .map_err(map_repo_err)?;
        Ok(rows)
    }

    pub async fn rename(
        &self,
        id: Uuid,
        new_name: &str,
        caller_id: Uuid,
    ) -> Result<SubjectGroup, DomainError> {
        // Block mutation on virtual groups (the Internal sentinel).
        let existing = self.get_by_id(id).await?;
        if existing.is_virtual {
            return Err(DomainError::new(
                ErrorKind::AccessDenied,
                "SubjectGroup",
                "virtual groups cannot be modified".to_string(),
            ));
        }

        SubjectGroup::validate_name(new_name).map_err(map_entity_err)?;
        let renamed = self.repo.rename(id, new_name).await.map_err(map_repo_err)?;

        tracing::info!(
            target: "audit",
            event = "group.renamed",
            group_id = %renamed.id,
            old_name = %existing.name,
            new_name = %renamed.name,
            by = %caller_id,
        );

        Ok(renamed)
    }

    /// Delete the group; cascades to:
    ///   - `auth.subject_group_members` rows (FK CASCADE).
    ///   - `storage.access_grants` rows where `subject_type='group'` and
    ///     `subject_id = id` (handled here, no FK exists between
    ///     `access_grants` and `subject_groups`).
    pub async fn delete(&self, id: Uuid, caller_id: Uuid) -> Result<(), DomainError> {
        let existing = self.get_by_id(id).await?;
        if existing.is_virtual {
            return Err(DomainError::new(
                ErrorKind::AccessDenied,
                "SubjectGroup",
                "virtual groups cannot be modified".to_string(),
            ));
        }

        // Atomically delete grants pointing at this group, then the group
        // itself. If either fails, both roll back.
        let mut tx = self.pool.begin().await.map_err(|e| {
            DomainError::new(
                ErrorKind::InternalError,
                "SubjectGroup",
                format!("begin tx: {}", e),
            )
        })?;

        let grants_deleted = sqlx::query(
            "DELETE FROM storage.access_grants
              WHERE subject_type = 'group' AND subject_id = $1",
        )
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            DomainError::new(
                ErrorKind::InternalError,
                "SubjectGroup",
                format!("cascade-delete grants: {}", e),
            )
        })?
        .rows_affected();

        let removed = sqlx::query("DELETE FROM auth.subject_groups WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                DomainError::new(
                    ErrorKind::InternalError,
                    "SubjectGroup",
                    format!("delete group: {}", e),
                )
            })?
            .rows_affected();

        if removed == 0 {
            return Err(DomainError::new(
                ErrorKind::NotFound,
                "SubjectGroup",
                format!("group {} not found", id),
            ));
        }

        tx.commit().await.map_err(|e| {
            DomainError::new(
                ErrorKind::InternalError,
                "SubjectGroup",
                format!("commit: {}", e),
            )
        })?;

        tracing::info!(
            target: "audit",
            event = "group.deleted",
            group_id = %id,
            name = %existing.name,
            grants_cascade_deleted = grants_deleted,
            by = %caller_id,
        );

        Ok(())
    }

    pub async fn add_member(
        &self,
        group_id: Uuid,
        member: GroupMember,
        caller_id: Uuid,
    ) -> Result<(), DomainError> {
        if group_id == INTERNAL_GROUP_ID {
            return Err(DomainError::new(
                ErrorKind::AccessDenied,
                "SubjectGroup",
                "Internal group membership is implicit and cannot be edited".to_string(),
            ));
        }

        self.repo
            .add_member(group_id, member, caller_id)
            .await
            .map_err(|e| {
                // Emit a security-relevant audit event on cycle / depth
                // rejections so abusive admin behaviour is captured.
                match &e {
                    SubjectGroupRepositoryError::Cycle(msg) => {
                        tracing::info!(
                            target: "audit",
                            event = "group.cycle_rejected",
                            group_id = %group_id,
                            member = ?member,
                            detail = %msg,
                            by = %caller_id,
                        );
                    }
                    SubjectGroupRepositoryError::DepthExceeded(msg) => {
                        tracing::info!(
                            target: "audit",
                            event = "group.depth_exceeded",
                            group_id = %group_id,
                            member = ?member,
                            detail = %msg,
                            by = %caller_id,
                        );
                    }
                    _ => {}
                }
                map_repo_err(e)
            })?;

        tracing::info!(
            target: "audit",
            event = "group.member_added",
            group_id = %group_id,
            member = ?member,
            by = %caller_id,
        );

        Ok(())
    }

    pub async fn remove_member(
        &self,
        group_id: Uuid,
        member: GroupMember,
        caller_id: Uuid,
    ) -> Result<(), DomainError> {
        if group_id == INTERNAL_GROUP_ID {
            return Err(DomainError::new(
                ErrorKind::AccessDenied,
                "SubjectGroup",
                "Internal group membership is implicit and cannot be edited".to_string(),
            ));
        }

        self.repo
            .remove_member(group_id, member)
            .await
            .map_err(map_repo_err)?;

        tracing::info!(
            target: "audit",
            event = "group.member_removed",
            group_id = %group_id,
            member = ?member,
            by = %caller_id,
        );

        Ok(())
    }

    pub async fn list_direct_members(
        &self,
        group_id: Uuid,
    ) -> Result<Vec<GroupMember>, DomainError> {
        self.repo
            .list_direct_members(group_id)
            .await
            .map_err(map_repo_err)
    }

    pub async fn list_transitive_users(&self, group_id: Uuid) -> Result<Vec<Uuid>, DomainError> {
        self.repo
            .list_transitive_users(group_id)
            .await
            .map_err(map_repo_err)
    }

    /// Hot path used by `PgAclEngine::expand_subject`. Returns the set of
    /// groups `user_id` belongs to transitively (excluding the implicit
    /// `INTERNAL_GROUP_ID` — the caller adds that).
    pub async fn groups_for_user(&self, user_id: Uuid) -> Result<HashSet<Uuid>, DomainError> {
        self.repo
            .groups_for_user(user_id)
            .await
            .map_err(map_repo_err)
    }
}

fn map_entity_err(e: SubjectGroupError) -> DomainError {
    let (kind, msg) = match e {
        SubjectGroupError::InvalidName(m) => (ErrorKind::InvalidInput, m),
        SubjectGroupError::CycleDetected(m) => (ErrorKind::InvalidInput, m),
        SubjectGroupError::DepthExceeded(m) => (ErrorKind::InvalidInput, m),
        SubjectGroupError::VirtualImmutable(m) => (ErrorKind::AccessDenied, m),
        SubjectGroupError::ValidationError(m) => (ErrorKind::InvalidInput, m),
    };
    DomainError::new(kind, "SubjectGroup", msg)
}

fn map_repo_err(e: SubjectGroupRepositoryError) -> DomainError {
    let (kind, msg) = match e {
        SubjectGroupRepositoryError::NotFound(m) => (ErrorKind::NotFound, m),
        SubjectGroupRepositoryError::NameAlreadyExists(m) => (ErrorKind::AlreadyExists, m),
        SubjectGroupRepositoryError::InvalidName(m) => (ErrorKind::InvalidInput, m),
        SubjectGroupRepositoryError::Cycle(m) => (ErrorKind::InvalidInput, m),
        SubjectGroupRepositoryError::DepthExceeded(m) => (ErrorKind::InvalidInput, m),
        SubjectGroupRepositoryError::VirtualImmutable(m) => (ErrorKind::AccessDenied, m),
        SubjectGroupRepositoryError::MemberAlreadyPresent => (
            ErrorKind::AlreadyExists,
            "member already in group".to_string(),
        ),
        SubjectGroupRepositoryError::MemberNotPresent => {
            (ErrorKind::NotFound, "member not in group".to_string())
        }
        SubjectGroupRepositoryError::StorageError(m) => (ErrorKind::InternalError, m),
    };
    DomainError::new(kind, "SubjectGroup", msg)
}

// ────────────────────────────────────────────────────────────────────────────
// Integration tests — service layer behaviours that need a live DB.
//
// How to run:
//   bash tests/common/spawn-db.sh
//   RUSTFLAGS='--cfg integration_tests' cargo test \
//       -p oxicloud --lib subject_group_service::integration_tests
// ────────────────────────────────────────────────────────────────────────────
#[cfg(integration_tests)]
#[allow(dead_code)]
mod integration_tests {
    use super::*;
    // INTERNAL_GROUP_ID is already in scope via `super::*` (re-exported
    // through the file's top-level `use crate::domain::entities::subject_group::…`).
    use sqlx::Row;
    use sqlx::postgres::PgPoolOptions;

    const DEFAULT_TEST_DB: &str =
        "postgres://oxicloud_test:oxicloud_test@localhost:5433/oxicloud_test";

    async fn make_service() -> SubjectGroupService {
        let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_TEST_DB.to_string());
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect to test DB — run tests/common/spawn-db.sh first");
        let pool = Arc::new(pool);
        let repo = Arc::new(SubjectGroupPgRepository::new(pool.clone()));
        SubjectGroupService::new(repo, pool)
    }

    async fn first_admin(pool: &sqlx::PgPool) -> Uuid {
        let row = sqlx::query("SELECT id FROM auth.users LIMIT 1")
            .fetch_optional(pool)
            .await
            .expect("query")
            .expect("seed an admin user before running these tests");
        row.get::<Uuid, _>("id")
    }

    fn rand_name(test: &str) -> String {
        format!("rust-test-svc-{}-{}", test, &Uuid::new_v4().to_string()[..8])
    }

    // ── 9. Virtual group cannot be deleted ─────────────────────────────────
    #[tokio::test]
    async fn test_virtual_group_cannot_be_deleted() {
        let svc = make_service().await;
        let admin = first_admin(&svc.pool).await;

        let err = svc
            .delete(INTERNAL_GROUP_ID, admin)
            .await
            .expect_err("delete on Internal must be rejected");
        assert_eq!(err.kind, ErrorKind::AccessDenied);
    }

    #[tokio::test]
    async fn test_virtual_group_cannot_be_renamed() {
        let svc = make_service().await;
        let admin = first_admin(&svc.pool).await;

        let err = svc
            .rename(INTERNAL_GROUP_ID, "renamed", admin)
            .await
            .expect_err("rename on Internal must be rejected");
        assert_eq!(err.kind, ErrorKind::AccessDenied);
    }

    #[tokio::test]
    async fn test_virtual_group_cannot_add_member() {
        let svc = make_service().await;
        let admin = first_admin(&svc.pool).await;

        let err = svc
            .add_member(INTERNAL_GROUP_ID, GroupMember::User(admin), admin)
            .await
            .expect_err("add_member on Internal must be rejected");
        assert_eq!(err.kind, ErrorKind::AccessDenied);
    }

    // ── 13. Grants are revoked atomically when a group is deleted ──────────
    //
    // The plan said "FK CASCADE", but there's no FK between `access_grants`
    // and `subject_groups` (different schemas; the cascade is handled by the
    // service's transactional DELETE). This test pins that behaviour.
    #[tokio::test]
    async fn test_grants_revoked_when_group_deleted() {
        let svc = make_service().await;
        let admin = first_admin(&svc.pool).await;

        // Create a group and a fake grant referencing it.
        let group = svc
            .create(&rand_name("cleanup"), None, admin)
            .await
            .unwrap();
        let resource_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO storage.access_grants \
             (subject_type, subject_id, resource_type, resource_id, \
              permission, granted_by) \
             VALUES ('group', $1, 'folder', $2, 'read', $3)",
        )
        .bind(group.id)
        .bind(resource_id)
        .bind(admin)
        .execute(svc.pool.as_ref())
        .await
        .expect("insert grant row");

        // Sanity: the grant exists.
        let pre: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM storage.access_grants \
              WHERE subject_type = 'group' AND subject_id = $1",
        )
        .bind(group.id)
        .fetch_one(svc.pool.as_ref())
        .await
        .unwrap();
        assert_eq!(pre, 1);

        // Delete the group — the same transaction nukes the grant.
        svc.delete(group.id, admin).await.unwrap();

        let post: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM storage.access_grants \
              WHERE subject_type = 'group' AND subject_id = $1",
        )
        .bind(group.id)
        .fetch_one(svc.pool.as_ref())
        .await
        .unwrap();
        assert_eq!(post, 0, "grants must be revoked atomically with the group");
    }

    // Bonus: service-layer name validation runs before the DB round-trip.
    #[tokio::test]
    async fn test_service_rejects_invalid_name_locally() {
        let svc = make_service().await;
        let admin = first_admin(&svc.pool).await;

        let err = svc
            .create("name with space", None, admin)
            .await
            .expect_err("space must be rejected");
        assert_eq!(err.kind, ErrorKind::InvalidInput);
    }
}
