//! Drive — the top-level container that owns a tree of folders/files.
//!
//! Drives replaced the per-user `My Folder - <username>` wrapper at D0.
//! Every folder and file row carries a `drive_id` (added by D0's
//! migration); a drive is the natural unit of quota, sharing, and
//! lifecycle. Membership is expressed through `storage.role_grants` rows
//! with `resource_type='drive'` — there is no separate `drive_members`
//! table.
//!
//! ## Kinds
//!
//! Two kinds today; the discriminant is the `kind` column with a CHECK
//! constraint.
//!
//! - **`personal`** — single-user, single-owner. The owner is captured
//!   by `default_for_user` (for the default Personal drive) or by an
//!   Owner role_grant on a secondary personal drive. Personal drives
//!   refuse `add_member`, `remove_member`, and `delete_drive` (when
//!   it's the user's only or default drive). A user can have multiple
//!   personal drives — one is marked default (`default_for_user =
//!   <uid>`), the others are secondaries (`default_for_user = NULL`,
//!   one Owner row in role_grants pinning them to the same user).
//!
//! - **`shared`** — multi-member, group-aware, full role roster
//!   (viewer / commenter / contributor / editor / owner). Members
//!   come from role_grants; group subjects expand transitively via
//!   the existing `subject_groups` machinery. Last-owner protection
//!   applies on member removal and drive deletion. Quota is set by
//!   the drive owner (or admin); `used_bytes` tracks consumption.
//!
//! Future kinds (e.g. `system` for built-in scratch space) drop in by
//! extending the CHECK + the `DriveKind` enum.
//!
//! ## Policies
//!
//! `policies` is a JSONB bag carrying feature flags / capability toggles
//! that drive owners can flip without a schema change. Known keys live in
//! `docs/plan/drive.md` §8 and §15 (e.g. `forbid_public_links`,
//! `include_in_photo_index`, `forbid_music_index`). Unknown keys are
//! preserved by the application — the schema is intentionally permissive
//! so future capability flags can land without a migration.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::common::errors::DomainError;
use crate::domain::services::authorization::Subject;

/// Drive kind discriminant. Mirrors the `storage.drives.kind` CHECK
/// constraint values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DriveKind {
    /// Single-owner storage compartment. Cannot have members added or
    /// removed via the membership API; the owner is fixed for the drive's
    /// lifetime.
    Personal,
    /// Multi-member drive supporting the full role roster. Membership is
    /// open to admin/owner-driven changes through the membership API.
    Shared,
}

impl DriveKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DriveKind::Personal => "personal",
            DriveKind::Shared => "shared",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "personal" => Some(DriveKind::Personal),
            "shared" => Some(DriveKind::Shared),
            _ => None,
        }
    }
}

/// Domain entity for a row in `storage.drives`.
///
/// Drives are pure metadata under the D0 design (docs/plan/drive.md §3):
/// no `name` column — the display name lives on the root folder pointed
/// at by `root_folder_id`. Code that needs the name pairs this struct
/// with a JOIN through `storage.folders`; see the repository's
/// `DriveWithRootName` view-model.
///
/// Field-level constraints are enforced at the SQL layer (CHECK on
/// `kind`, partial UNIQUE on `default_for_user`). The struct mirrors
/// the column set 1:1; behaviour beyond field access lives in
/// `DriveRepository` and `DriveService` (post-D0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drive {
    /// Stable identifier. Generated server-side at creation.
    pub id: Uuid,
    /// Discriminant — see [`DriveKind`].
    pub kind: DriveKind,
    /// Set iff this is the user's default personal drive (UNIQUE in SQL
    /// via a partial index `WHERE default_for_user IS NOT NULL`). NULL
    /// on shared drives and on secondary personal drives.
    pub default_for_user: Option<Uuid>,
    /// The drive's mount-point folder. The column is NULLable in SQL
    /// only because the atomic creation CTE writes it mid-statement
    /// (a column-level `NOT NULL` would refuse the initial drive INSERT
    /// — see docs/plan/drive.md §3). After any successful creation path,
    /// this is populated; code reading `Drive` may treat it as `Uuid`,
    /// not `Option<Uuid>`. A NULL at read time is a data-invariant bug.
    pub root_folder_id: Uuid,
    /// Soft cap on this drive's storage usage, in bytes. `None` means
    /// "no quota" (rare; reserved for admin overrides). The default
    /// initial quota for a fresh personal drive is taken from the
    /// owner's `auth.users.storage_quota_bytes` at creation time.
    /// **Mutation is OxiCloud-admin only** (docs/plan/drive.md §7) —
    /// not in the drive `owner` role bundle.
    pub quota_bytes: Option<i64>,
    /// Running total of bytes consumed. Maintained incrementally by
    /// upload/delete paths in D4; on D0 still reflects the pre-Drive
    /// per-user counters via the backfill.
    pub used_bytes: i64,
    /// Capability flags / feature toggles. Extensible JSONB — see
    /// `docs/plan/drive.md` §8 and §15 for the known keys.
    pub policies: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl Drive {
    /// `true` for the user's default personal drive (the only drive for
    /// which `default_for_user` is set to that user's id).
    pub fn is_default_for(&self, user_id: Uuid) -> bool {
        self.default_for_user == Some(user_id)
    }

    /// `true` if this drive is a personal drive of any kind (default or
    /// secondary). Encapsulates the kind check at the call site.
    pub fn is_personal(&self) -> bool {
        matches!(self.kind, DriveKind::Personal)
    }

    /// Typed view of `policies` for enforcement code. Lenient deserialise:
    /// unknown keys are preserved on disk (the column stays the canonical
    /// JSONB bag) but ignored here, missing keys default to `false`.
    /// See `docs/plan/drive.md` §8.
    pub fn typed_policies(&self) -> DrivePolicies {
        DrivePolicies::from_value(&self.policies)
    }
}

/// Typed mirror of the `policies` JSONB. Five known keys; the JSONB column
/// remains the source of truth and may carry unknown keys verbatim — this
/// struct is a read view for enforcement and a write view for the policy
/// PATCH endpoint. Every field defaults to `false` (everything allowed)
/// so a freshly-created drive doesn't need a populated policy bag.
///
/// See `docs/plan/drive.md` §8 for the enforcement matrix
/// (which callsite each key is checked at).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct DrivePolicies {
    /// Disables per-resource grants on resources in this drive. Drive-level
    /// membership (Owner/Editor/Viewer) still works. Enforced at
    /// `grant_handler::create_grant`.
    pub forbid_sharing: bool,
    /// Blocks grants whose subject has `users.is_external = true`. Enforced
    /// at `magic_link_invite_service::resolve_or_create_recipient` and
    /// `grant_handler::create_grant`.
    pub forbid_external_sharing: bool,
    /// Blocks anonymous-link (token-share) creation on resources in this
    /// drive. Enforced at `share_service::create_shared_link`.
    pub forbid_public_links: bool,
    /// Blocks MOVE when `src.drive_id != dst.drive_id`. Enforced at the
    /// move endpoints. Lands paired with D6's cross-drive move work.
    pub forbid_cross_drive_move: bool,
    /// Locks the Owner-role membership set: no owner can be added,
    /// removed, or demoted by another owner — only OxiCloud admin can
    /// change the Owner roster. Editor / Viewer mutations by remaining
    /// owners are unaffected. Personal drives are already
    /// single-owner-immutable via `refuse_if_personal`, so this policy
    /// only adds value on shared drives. Enforced at
    /// `DriveManagementService::set_member_role` (refuses Owner role
    /// writes) and `::remove_member` (refuses Owner removals) when the
    /// caller is non-admin.
    pub forbid_owner_role_change: bool,
    /// Opts this drive into the `/api/photos` timeline (§15). Non-default
    /// drives are omitted by default so a random shared folder full of
    /// screenshots doesn't bleed into the personal timeline; owners flip
    /// this on when the drive genuinely is a photo library (e.g. "Family
    /// Photos"). Default personal drives get `true` on creation via the
    /// `PersonalDriveLifecycleHook` + a one-shot backfill for existing
    /// rows, so the SQL predicate is a single positive rule with no
    /// per-kind carve-out. Read at `file_blob_read_repository::
    /// list_media_files` + `list_geo_clusters`. See §15 for the query
    /// shape and rationale.
    pub include_in_photo_index: bool,
    /// Same shape as `include_in_photo_index`, applied to the Music
    /// library surface (playlists today; a `/api/music/tracks` library
    /// view later). Symmetric opt-in — Music was originally cross-drive
    /// via a `forbid_music_index` opt-out, but that mixed-form naming
    /// created "one include-in, one forbid" confusion and the
    /// "shared audio is always intentional" claim didn't hold under
    /// scrutiny (voicemail MP3s in a work drive shouldn't bleed into
    /// the personal library). See §15.
    pub include_in_music_index: bool,
}

impl DrivePolicies {
    /// Parse from the raw JSONB. Lenient — unknown keys are dropped from
    /// the typed view but remain in the source `serde_json::Value`. A
    /// malformed bag (e.g. wrong type) falls back to the all-false default
    /// rather than refusing the read; enforcement code never panics on
    /// existing data.
    pub fn from_value(value: &serde_json::Value) -> Self {
        serde_json::from_value(value.clone()).unwrap_or_default()
    }

    /// D5 `forbid_public_links` gate, used by every entry point that
    /// mints an anonymous token-share on a resource in this drive
    /// (`share_service::create_shared_link` today; future protocol
    /// surfaces — e.g. NextCloud OCS share — must call this too). The
    /// gate owns the decision + audit + canonical error so the
    /// rejection shape stays in lockstep across surfaces. See
    /// `docs/plan/drive.md` §8.
    ///
    /// Returns `Ok(())` when the policy is off; emits a
    /// `share.rejected` audit line and returns
    /// `OperationNotSupported` when on.
    pub fn refuse_public_links(&self, ctx: PublicLinkGateContext) -> Result<(), DomainError> {
        if !self.forbid_public_links {
            return Ok(());
        }
        tracing::info!(
            target: "audit",
            event = "share.rejected",
            reason = "forbid_public_links",
            caller_id = %ctx.caller_id,
            item_type = ctx.item_type,
            item_id = %ctx.item_id,
            "👮🏻‍♂️ public-link creation refused: forbid_public_links",
        );
        Err(DomainError::operation_not_supported(
            "Share",
            "This drive does not allow public links.",
        ))
    }

    /// D5 `forbid_sharing` gate: refuses **per-resource** grants on
    /// resources in this drive when the policy is on. Drive-level
    /// membership stays unaffected — otherwise a drive that disables
    /// sharing would also become uneditable except by the original
    /// owner. The semantic the plan §8 commits to is "no fine-grained
    /// sharing of individual files; access happens through drive
    /// membership only."
    ///
    /// Enforced at `grant_handler::create_grant` for File / Folder
    /// resources. The Drive-resource branch of `/api/grants` and the
    /// `/api/drives/{id}/members` routes deliberately don't call this
    /// gate.
    ///
    /// Returns `Ok(())` when the policy is off; emits a
    /// `grant.rejected` audit line and returns `OperationNotSupported`
    /// when on.
    pub fn refuse_sharing(&self, ctx: SharingGateContext) -> Result<(), DomainError> {
        if !self.forbid_sharing {
            return Ok(());
        }
        tracing::info!(
            target: "audit",
            event = "grant.rejected",
            reason = "forbid_sharing",
            caller_id = %ctx.caller_id,
            resource_type = ctx.resource_type,
            resource_id = %ctx.resource_id,
            "👮🏻‍♂️ per-resource grant refused: forbid_sharing",
        );
        Err(DomainError::operation_not_supported(
            "Grant",
            "This drive does not allow per-resource sharing.",
        ))
    }

    /// D5 `forbid_owner_role_change` gate: refuses Owner-role mutations
    /// (adding a new Owner, demoting an existing Owner, or removing
    /// one) when the caller isn't OxiCloud admin and the policy is on.
    /// Membership of non-Owner roles is unaffected.
    ///
    /// Enforced at `DriveManagementService::set_member_role` (refuses
    /// Owner role writes) and `::remove_member` (refuses removing an
    /// Owner subject). Skipped when `caller_is_admin = true` — the
    /// policy exists to constrain owners, not the tenant operator.
    /// Personal drives never reach this gate because
    /// `refuse_if_personal` rejects every member mutation upstream.
    ///
    /// Returns `Ok(())` when the policy is off or the caller is admin;
    /// emits a `drive_membership.rejected` audit line and returns
    /// `OperationNotSupported` otherwise.
    pub fn refuse_owner_role_change(
        &self,
        ctx: OwnerRoleChangeGateContext,
    ) -> Result<(), DomainError> {
        if !self.forbid_owner_role_change {
            return Ok(());
        }
        if ctx.caller_is_admin {
            return Ok(());
        }
        tracing::info!(
            target: "audit",
            event = "drive_membership.rejected",
            reason = "forbid_owner_role_change",
            operation = ctx.operation,
            caller_id = %ctx.caller_id,
            drive_id = %ctx.drive_id,
            subject_type = ctx.subject_type,
            subject_id = %ctx.subject_id,
            "👮🏻‍♂️ owner-role mutation refused: forbid_owner_role_change",
        );
        Err(DomainError::operation_not_supported(
            "Drive",
            "This drive's Owner membership is locked — only OxiCloud admin can change owners.",
        ))
    }

    /// D5 `forbid_cross_drive_move` gate: refuses MOVE when
    /// `src.drive_id != dst.drive_id`. The policy lives on the SOURCE
    /// drive — its owner decides whether content can leave. Targets'
    /// owners already gate inbound moves via the `Create` permission
    /// on the destination folder, so a symmetric check would be
    /// redundant.
    ///
    /// Enforced at `file_management_service::move_file_with_perms`
    /// and `folder_service::move_folder_with_perms`. The handler
    /// doesn't see this gate — it lives in the service layer per
    /// the AuthZ architecture rule in CLAUDE.md.
    ///
    /// Returns `Ok(())` when the policy is off; emits a
    /// `move.rejected` audit line and returns `OperationNotSupported`
    /// when on.
    pub fn refuse_cross_drive_move(
        &self,
        ctx: CrossDriveMoveGateContext,
    ) -> Result<(), DomainError> {
        if !self.forbid_cross_drive_move {
            return Ok(());
        }
        tracing::info!(
            target: "audit",
            event = "move.rejected",
            reason = "forbid_cross_drive_move",
            caller_id = %ctx.caller_id,
            resource_type = ctx.resource_type,
            resource_id = %ctx.resource_id,
            src_drive_id = %ctx.src_drive_id,
            dst_drive_id = %ctx.dst_drive_id,
            "👮🏻‍♂️ cross-drive move refused: forbid_cross_drive_move",
        );
        Err(DomainError::operation_not_supported(
            "Move",
            "This drive does not allow moving content out to another drive.",
        ))
    }

    /// D5 `forbid_external_sharing` gate, shared by every entry point
    /// that creates a grant on a resource in this drive
    /// (`grant_handler::create_grant`,
    /// `DriveManagementService::set_member_role`). Each caller
    /// resolves `is_external` from whichever source naturally fits
    /// (the just-created `User` entity in the email path, a
    /// `get_user_flags` probe in the user-by-id path); the gate
    /// itself owns the decision + audit + canonical error so the
    /// shape stays in lockstep across surfaces. See `docs/plan/drive.md` §8.
    ///
    /// Returns `Ok(())` when the subject is allowed (policy off, subject
    /// is not a User, or the User is not external). Returns
    /// `OperationNotSupported` after emitting a `grant.rejected` audit
    /// line otherwise.
    pub fn refuse_external_sharing(
        &self,
        subject: Subject,
        is_external: bool,
        ctx: ExternalSharingGateContext,
    ) -> Result<(), DomainError> {
        if !self.forbid_external_sharing {
            return Ok(());
        }
        let Subject::User(uid) = subject else {
            return Ok(());
        };
        if !is_external {
            return Ok(());
        }
        tracing::info!(
            target: "audit",
            event = "grant.rejected",
            reason = "forbid_external_sharing",
            stage = ctx.stage,
            caller_id = %ctx.caller_id,
            subject_id = %uid,
            drive_id = ?ctx.drive_id,
            resource_type = ?ctx.resource_type,
            resource_id = ?ctx.resource_id,
            "👮🏻‍♂️ grant refused: forbid_external_sharing",
        );
        Err(DomainError::operation_not_supported(
            "Grant",
            "This drive does not allow external sharing.",
        ))
    }
}

/// Audit / identity context for [`DrivePolicies::refuse_owner_role_change`].
///
/// Carries the subject (the user/group whose Owner status is being
/// added, removed, or demoted) and the calling operation tag
/// (`"set_member_role"` or `"remove_member"`) so the audit log
/// pinpoints exactly which mutation the policy refused.
#[derive(Debug, Clone, Copy)]
pub struct OwnerRoleChangeGateContext {
    pub caller_id: Uuid,
    pub caller_is_admin: bool,
    pub drive_id: Uuid,
    pub operation: &'static str,
    pub subject_type: &'static str,
    pub subject_id: Uuid,
}

/// Audit / identity context for [`DrivePolicies::refuse_cross_drive_move`].
///
/// Carries the source and destination drive ids so the audit log
/// captures exactly which boundary the refused move would cross —
/// useful when investigating whether someone is probing the gate or
/// genuinely trying to organize content.
#[derive(Debug, Clone, Copy)]
pub struct CrossDriveMoveGateContext {
    pub caller_id: Uuid,
    /// `"file"` or `"folder"`.
    pub resource_type: &'static str,
    pub resource_id: Uuid,
    pub src_drive_id: Uuid,
    pub dst_drive_id: Uuid,
}

/// Audit / identity context for [`DrivePolicies::refuse_sharing`].
///
/// Only File / Folder resources reach this gate — the per-resource
/// grant surface. Drive-resource grants go through
/// `set_member_role` and aren't subject to `forbid_sharing`.
#[derive(Debug, Clone, Copy)]
pub struct SharingGateContext {
    pub caller_id: Uuid,
    /// `"file"` or `"folder"`.
    pub resource_type: &'static str,
    pub resource_id: Uuid,
}

/// Audit / identity context for [`DrivePolicies::refuse_public_links`].
///
/// Single callsite today (`share_service::create_shared_link`), but the
/// struct is the explicit contract so future surfaces (NextCloud OCS
/// share, WebDAV public-link sigil, …) land with the same shape.
#[derive(Debug, Clone, Copy)]
pub struct PublicLinkGateContext {
    pub caller_id: Uuid,
    /// `"file"` or `"folder"` — the share target's resource kind.
    pub item_type: &'static str,
    pub item_id: Uuid,
}

/// Audit / identity context for [`DrivePolicies::refuse_external_sharing`].
///
/// Two callsites with different identifiers naturally fill this in:
/// - `grant_handler` (File/Folder branch): `drive_id = None`,
///   `resource_type` + `resource_id` set
/// - `DriveManagementService::set_member_role`: `drive_id` set,
///   `resource_type` + `resource_id = None`
///
/// All three appear in the audit log so a single grep on
/// `grant.rejected reason=forbid_external_sharing` surfaces every
/// refusal regardless of entry point.
#[derive(Debug, Clone, Copy)]
pub struct ExternalSharingGateContext {
    pub caller_id: Uuid,
    /// Distinguishes the call site for log aggregators. Known values
    /// today: `"late_user"` (grant_handler), `"drive_member"`
    /// (set_member_role). New entry points pick a fresh string.
    pub stage: &'static str,
    pub drive_id: Option<Uuid>,
    pub resource_type: Option<&'static str>,
    pub resource_id: Option<Uuid>,
}
