//! D2 — drive membership management service.
//!
//! Translates `POST/PATCH/DELETE /api/drives/{id}/members` into role-grant
//! writes on `resource_type='drive'`, layering D2's business rules on top:
//!
//! - **Personal-drive guard** (§2): drives with `kind='personal'` are
//!   single-user single-owner by invariant; any member mutation is refused
//!   at the service edge with `403`. Listing a personal drive's members is
//!   still allowed (returns exactly the owner row) so the UI can render the
//!   same shape across drive kinds without per-kind branching.
//!
//! - **Last-owner protection** (shared drives): removing or demoting the
//!   final `Role::Owner` would leave the drive unmanageable. Refused at the
//!   service edge — the caller has to promote someone else to Owner first,
//!   or delete the drive.
//!
//! - **Authorization**: caller must hold `Permission::Manage` on the drive
//!   to mutate; `Permission::Read` to list. `AuthorizationEngine::require`
//!   emits the canonical `authz.denied` audit line on rejection.

use std::sync::Arc;

use uuid::Uuid;

use crate::application::ports::authorization_ports::AuthorizationEngine;
use crate::common::errors::DomainError;
use crate::domain::repositories::drive_repository::DriveRepository;
use crate::domain::repositories::subject_group_repository::SubjectGroupRepository;
use crate::domain::services::authorization::{Grant, Permission, Resource, Role, Subject};
use crate::infrastructure::repositories::pg::DrivePgRepository;
use crate::infrastructure::repositories::pg::SubjectGroupPgRepository;
use crate::infrastructure::services::pg_acl_engine::PgAclEngine;

pub struct DriveManagementService {
    drive_repo: Arc<DrivePgRepository>,
    authz: Arc<PgAclEngine>,
    /// Needed to validate that a Group owner subject is non-empty at
    /// create-drive time — refusing creation with an empty group avoids
    /// constructing an orphan-owned drive (the "drive must always have
    /// ≥1 effective Owner-user" invariant from day one).
    group_repo: Arc<SubjectGroupPgRepository>,
}

impl DriveManagementService {
    pub fn new(
        drive_repo: Arc<DrivePgRepository>,
        authz: Arc<PgAclEngine>,
        group_repo: Arc<SubjectGroupPgRepository>,
    ) -> Self {
        Self {
            drive_repo,
            authz,
            group_repo,
        }
    }

    /// `POST /api/drives` — create a shared drive owned by a group.
    ///
    /// **AuthZ (D3a)**: OxiCloud-admin only. The plan (`drive.md §6`)
    /// reads "admin or group owner triggers" — D3a starts with the
    /// admin-only path; group-owner triggering can extend the gate
    /// later without changing the wire shape or the service method
    /// signature. `caller_is_admin` is resolved by the HTTP handler
    /// from `CurrentUser.role` and passed in; the service trusts it
    /// (defense-in-depth check stays at the route layer).
    ///
    /// Audit log: `drive.created` with the drive id, the owner group,
    /// and the granted_by (the admin caller).
    pub async fn create_shared_drive(
        &self,
        caller_id: Uuid,
        caller_is_admin: bool,
        name: &str,
        owner_subject: Subject,
        quota_bytes: Option<i64>,
    ) -> Result<crate::domain::repositories::drive_repository::DriveWithRootName, DomainError> {
        if !caller_is_admin {
            tracing::info!(
                target: "audit",
                event = "drive_create.rejected",
                reason = "not_admin",
                caller_id = %caller_id,
                owner_type = owner_subject.type_str(),
                owner_id = %owner_subject.id(),
                "👮🏻‍♂️ refused shared-drive create: caller is not an OxiCloud admin",
            );
            return Err(DomainError::access_denied(
                "Drive",
                "Only OxiCloud administrators can create shared drives.",
            ));
        }

        // Token subjects are share-link identities, not entities that can
        // own things. Refuse at the service edge.
        if matches!(owner_subject, Subject::Token(_)) {
            tracing::info!(
                target: "audit",
                event = "drive_create.rejected",
                reason = "invalid_owner_kind",
                caller_id = %caller_id,
                owner_type = "token",
                "👮🏻‍♂️ refused shared-drive create: owner cannot be a Token subject",
            );
            return Err(DomainError::validation_error(
                "Drive owner must be a user or a group, not a token.",
            ));
        }

        // Group owners must be non-empty — otherwise the drive is created
        // with no transitive Owner-user from day one. Per Ed's invariant:
        // "a drive must always remain with at least one Owner-user". User
        // owners trivially satisfy this.
        if let Subject::Group(gid) = owner_subject {
            let n = self.group_repo.count_members(gid).await.map_err(|e| {
                DomainError::internal_error("Drive", format!("group lookup failed: {e:?}"))
            })?;
            if n < 1 {
                tracing::info!(
                    target: "audit",
                    event = "drive_create.rejected",
                    reason = "owner_group_empty",
                    caller_id = %caller_id,
                    owner_group_id = %gid,
                    "👮🏻‍♂️ refused shared-drive create: owner group has no members",
                );
                return Err(DomainError::validation_error(
                    "Owner group has no members — the drive would have no effective Owner.",
                ));
            }
        }

        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(DomainError::validation_error("Drive name is required."));
        }

        let drive = self
            .drive_repo
            .create_shared_drive_atomic(trimmed, owner_subject, quota_bytes, caller_id)
            .await
            .map_err(|e| DomainError::internal_error("Drive", format!("create failed: {e:?}")))?;

        tracing::info!(
            target: "audit",
            event = "drive.created",
            kind = "shared",
            drive_id = %drive.drive.id,
            owner_type = owner_subject.type_str(),
            owner_id = %owner_subject.id(),
            granted_by = %caller_id,
            "🆕 shared drive created '{}' owned by {} {}",
            trimmed, owner_subject.type_str(), owner_subject.id(),
        );

        Ok(drive)
    }

    /// `GET /api/drives/{id}/members` — every role grant on the drive.
    pub async fn list_members(
        &self,
        caller_id: Uuid,
        drive_id: Uuid,
    ) -> Result<Vec<Grant>, DomainError> {
        let resource = Resource::Drive(drive_id);
        self.authz
            .require(Subject::User(caller_id), Permission::Read, resource)
            .await?;
        self.authz.list_grants_on_resource(resource).await
    }

    /// `POST /api/drives/{id}/members` (create) or
    /// `PATCH /api/drives/{id}/members/{subject_id}` (role change).
    ///
    /// `set_role` is idempotent — `(subject, resource)` is unique — so the
    /// two HTTP shapes share one service method. Returns the resulting grant.
    ///
    /// `caller_is_admin = true` skips the per-drive `Manage` check
    /// (used by `/api/admin/drives/{id}/members` so an admin who
    /// created the drive for someone else can still edit owners).
    /// Personal-drive guard and last-owner protection still apply —
    /// admin bypass is about *access*, not invariants. The audit log
    /// flags admin-driven changes via `via_admin = true` so a reader
    /// can tell what fired the mutation. The caller (HTTP handler) is
    /// authoritative for `caller_is_admin`; the route gate is the
    /// source of truth and the service trusts the flag.
    pub async fn set_member_role(
        &self,
        caller_id: Uuid,
        caller_is_admin: bool,
        drive_id: Uuid,
        subject: Subject,
        role: Role,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Grant, DomainError> {
        let resource = Resource::Drive(drive_id);
        if !caller_is_admin {
            self.authz
                .require(Subject::User(caller_id), Permission::Manage, resource)
                .await?;
        }

        self.refuse_if_personal(drive_id, "set_member_role").await?;

        // Demotion of the last owner = last-owner protection trips. A fresh
        // owner-role write or any non-owner subject is fine; only the case
        // "this subject is currently the only owner AND the new role is not
        // owner" is refused. Applies to admin-bypass too: orphaning a
        // shared drive is the same category error regardless of who fires
        // the request.
        if !matches!(role, Role::Owner) {
            self.refuse_if_last_owner_change(drive_id, subject, caller_id)
                .await?;
        }

        let grant = self
            .authz
            .set_role(caller_id, subject, role, resource, expires_at)
            .await?;

        if caller_is_admin {
            tracing::info!(
                target: "audit",
                event = "drive_membership.set_via_admin",
                drive_id = %drive_id,
                subject_type = subject.type_str(),
                subject_id = %subject.id(),
                role = role.as_str(),
                by = %caller_id,
                "👮🏻‍♂️ admin set drive member role bypassing Manage check",
            );
        }
        Ok(grant)
    }

    /// `DELETE /api/drives/{id}/members/{subject_id}`. Idempotent — removing
    /// a subject with no current grant succeeds (matches `clear_role`).
    ///
    /// `caller_is_admin` mirrors `set_member_role`: skips the per-drive
    /// `Manage` check but keeps personal-drive guard + last-owner
    /// protection. Audit emits `drive_membership.removed_via_admin`
    /// when the bypass fires.
    pub async fn remove_member(
        &self,
        caller_id: Uuid,
        caller_is_admin: bool,
        drive_id: Uuid,
        subject: Subject,
    ) -> Result<(), DomainError> {
        let resource = Resource::Drive(drive_id);
        if !caller_is_admin {
            self.authz
                .require(Subject::User(caller_id), Permission::Manage, resource)
                .await?;
        }

        self.refuse_if_personal(drive_id, "remove_member").await?;

        self.refuse_if_last_owner_change(drive_id, subject, caller_id)
            .await?;

        self.authz.clear_role(subject, resource).await?;

        if caller_is_admin {
            tracing::info!(
                target: "audit",
                event = "drive_membership.removed_via_admin",
                drive_id = %drive_id,
                subject_type = subject.type_str(),
                subject_id = %subject.id(),
                by = %caller_id,
                "👮🏻‍♂️ admin removed drive member bypassing Manage check",
            );
        }
        Ok(())
    }

    // ── Business rules ──────────────────────────────────────────────────────

    /// Personal drives are single-user single-owner; any member mutation is
    /// a category error (§2). Returns `Forbidden` with an audit line.
    async fn refuse_if_personal(&self, drive_id: Uuid, op: &str) -> Result<(), DomainError> {
        let drive = self.drive_repo.get_by_id(drive_id).await.map_err(|e| {
            DomainError::internal_error("Drive", format!("Failed to fetch drive: {e:?}"))
        })?;
        if drive.drive.is_personal() {
            tracing::info!(
                target: "audit",
                event = "drive_membership.rejected",
                reason = "personal_drive_immutable",
                operation = %op,
                drive_id = %drive_id,
                "👮🏻‍♂️ refused {op} on personal drive {drive_id}",
            );
            return Err(DomainError::operation_not_supported(
                "Drive",
                "Personal drives have a fixed single-owner membership and cannot be modified.",
            ));
        }
        Ok(())
    }

    /// Refuse the change if `subject` is currently the sole `Owner` on the
    /// drive and the operation would remove or demote them. A shared drive
    /// must always have at least one Owner — otherwise it becomes orphaned
    /// (no one can ever grant permissions again).
    async fn refuse_if_last_owner_change(
        &self,
        drive_id: Uuid,
        subject: Subject,
        caller_id: Uuid,
    ) -> Result<(), DomainError> {
        let resource = Resource::Drive(drive_id);
        let grants = self.authz.list_grants_on_resource(resource).await?;

        // `subject` must currently BE an owner — otherwise no demotion risk.
        let subject_is_owner = grants
            .iter()
            .any(|g| g.subject == subject && matches!(g.role, Role::Owner));
        if !subject_is_owner {
            return Ok(());
        }

        let owner_count = grants
            .iter()
            .filter(|g| matches!(g.role, Role::Owner))
            .count();
        if owner_count <= 1 {
            tracing::info!(
                target: "audit",
                event = "drive_membership.rejected",
                reason = "last_owner",
                drive_id = %drive_id,
                caller_id = %caller_id,
                subject_type = subject.type_str(),
                subject_id = %subject.id(),
                "👮🏻‍♂️ refused last-owner removal on drive {drive_id}",
            );
            return Err(DomainError::validation_error(
                "A shared drive must keep at least one Owner — promote another \
                 member to Owner first, or delete the drive.",
            ));
        }
        Ok(())
    }
}
