//! Repository for [`Drive`] entities backed by `storage.drives`.
//!
//! Drives have no separate membership table — owner/editor/viewer
//! membership lives in `storage.role_grants` with
//! `resource_type='drive'`. That means **listing the drives a user can
//! reach goes through the role-grant query, not through this
//! repository**. This repo handles:
//!
//!   * Creating a drive (used by the user-creation lifecycle hook and
//!     by D3's shared-drive flow).
//!   * Looking up a single drive by id (used by the engine's owner_of /
//!     check paths, by `/api/drives/{id}`, and by the drive picker).
//!   * Finding the caller's default drive (used by the Photos / Music
//!     endpoints and by D1's redirect-from-`/` logic).
//!
//! Membership-flavoured queries (e.g. "list every drive user X can
//! read") live in `DriveListingService` (post-D0) which reads
//! `role_grants` and resolves the matching drive rows here.

use thiserror::Error;
use uuid::Uuid;

use crate::domain::entities::drive::{Drive, DriveKind};

#[derive(Debug, Error)]
pub enum DriveRepositoryError {
    #[error("Drive not found: {0}")]
    NotFound(String),
    /// A user already has a default drive set — partial unique index on
    /// `default_for_user` rejects a second one. Surfaces the constraint
    /// explicitly so the lifecycle hook can no-op idempotently.
    #[error("User already has a default drive: {0}")]
    DefaultDriveAlreadyExists(String),
    #[error("Invalid drive kind: {0}")]
    InvalidKind(String),
    #[error("Storage error: {0}")]
    StorageError(String),
}

/// A drive paired with the display name from its root folder.
///
/// `storage.drives` has no `name` column under the D0 design
/// (docs/plan/drive.md §3) — the display name lives on
/// `storage.folders.name` of the row pointed at by `drive.root_folder_id`.
/// Read paths join the two tables and hand callers this view-model so the
/// API surface can continue to expose a single "drive with name" shape
/// without a follow-up query per drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveWithRootName {
    pub drive: Drive,
    /// The drive's display name. Sourced from `storage.folders.name`
    /// of the root folder via JOIN at read time.
    pub root_folder_name: String,
    /// Highest role the calling user holds on this drive (direct OR
    /// group-mediated). Populated by `list_for_subjects` (which already
    /// JOINs `role_grants` for accessibility, so the role is in scope at
    /// query time). `None` for repo methods called without a caller
    /// context (`get_by_id`, `get_by_ids`, `find_default_for_user`,
    /// `create_personal_drive_atomic`) — the DTO layer omits the field
    /// via `#[serde(skip_serializing_if = "Option::is_none")]`.
    ///
    /// See [[project-caller-role-on-file-folder-dto]] for the pattern
    /// extension to FileDto/FolderDto with a perf warning.
    pub caller_role: Option<crate::domain::services::authorization::Role>,
}

#[async_trait::async_trait]
pub trait DriveRepository: Send + Sync + 'static {
    /// Atomically create a personal drive together with its root folder
    /// and the owner role_grant — all four DB writes in a single SQL
    /// statement (docs/plan/drive.md §3 "Atomic creation"). The
    /// statement runs as its own implicit transaction in autocommit mode
    /// so a server crash mid-statement leaves no half-row state.
    ///
    /// The root folder is created with name `"Personal"` (the canonical
    /// default) and `parent_id IS NULL`. The drive's `root_folder_id`
    /// is wired to point at it before the statement commits.
    ///
    /// Returns `DefaultDriveAlreadyExists` when the owner already has a
    /// default drive — relies on the partial UNIQUE index on
    /// `default_for_user`.
    async fn create_personal_drive_atomic(
        &self,
        owner_id: Uuid,
        quota_bytes: Option<i64>,
    ) -> Result<DriveWithRootName, DriveRepositoryError>;

    /// Atomically create a **shared** drive together with its root folder
    /// and the initial Owner-role grant. Mirrors
    /// `create_personal_drive_atomic` but with three differences:
    ///   - `kind='shared'`, `default_for_user=NULL`
    ///   - root folder name is caller-supplied (validated upstream)
    ///   - Owner role_grant subject is caller-supplied — either a
    ///     single `User` (becomes the sole drive Owner) or a `Group`
    ///     (the group's transitive user members all gain the Owner
    ///     role via subject expansion). Token subjects are refused at
    ///     the service edge.
    ///
    /// `granted_by` is recorded on the role_grant row + on the root
    /// folder's `created_by` / `updated_by` columns for audit
    /// traceability — the OxiCloud admin who provisioned the drive.
    ///
    /// **AuthZ contract**: this method performs no authorization. The
    /// service layer MUST verify the caller has the OxiCloud `admin`
    /// system role (D3a). If `owner_subject` is `Group`, the service
    /// MUST also have verified the group has ≥1 user member —
    /// otherwise the drive is created with no effective Owner-user
    /// and would breach the "drive must always have ≥1 effective
    /// Owner" invariant from day one.
    async fn create_shared_drive_atomic(
        &self,
        name: &str,
        owner_subject: crate::domain::services::authorization::Subject,
        quota_bytes: Option<i64>,
        granted_by: Uuid,
    ) -> Result<DriveWithRootName, DriveRepositoryError>;

    /// Fetch a drive by id together with its display name. `NotFound`
    /// when no row matches.
    async fn get_by_id(&self, id: Uuid) -> Result<DriveWithRootName, DriveRepositoryError>;

    /// Batch fetch — returns one row per existing id. Missing ids are
    /// silently dropped (matches the `get_files_by_ids` / `get_folders_by_ids`
    /// shape used by `list_shared_with_me`). Caller-side `HashMap<Uuid, _>`
    /// lookup gives `Option<Drive>` semantics for stale grants whose drive
    /// was deleted between listing and resolution.
    async fn get_by_ids(
        &self,
        ids: &[Uuid],
    ) -> Result<Vec<DriveWithRootName>, DriveRepositoryError>;

    /// Return the caller's default personal drive paired with its
    /// display name, or `NotFound` if they don't have one (e.g.
    /// external users; users created before the lifecycle hook fired).
    /// Drives the Photos timeline scope, the `/api/recent/*` scope, and
    /// D1's redirect-from-`/`.
    async fn find_default_for_user(
        &self,
        user_id: Uuid,
    ) -> Result<DriveWithRootName, DriveRepositoryError>;

    /// Canonical "what is this user's home root folder id?" lookup.
    ///
    /// Returns `Some(uuid)` for any internal user with a default personal
    /// drive (the lifecycle hook provisions one at registration), and
    /// `None` for users who have no default drive (external users; users
    /// created before the hook existed). The id identifies the user's
    /// home **by drive ownership** (`default_for_user == user_id`),
    /// never by folder name — users can rename their home, so any code
    /// that wants to ask "is this folder the user's home?" must compare
    /// folder ids, not names.
    ///
    /// Storage errors (DB unreachable, etc.) bubble up as `Err`; the
    /// "user simply has no home" case is `Ok(None)`, not an error.
    async fn home_root_folder_id_for(
        &self,
        user_id: Uuid,
    ) -> Result<Option<Uuid>, DriveRepositoryError> {
        match self.find_default_for_user(user_id).await {
            Ok(d) => Ok(Some(d.drive.root_folder_id)),
            Err(DriveRepositoryError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// List drives the caller can read, resolved via `role_grants` for
    /// `resource_type='drive'`. The caller's group memberships are
    /// expanded by the engine's `subject_match_set`; that expanded set
    /// is what this method's `subject_ids` argument carries.
    ///
    /// Returns rows in a stable order: default drive first (if any),
    /// then by display name. The `/api/drives` handler relies on that
    /// order for the picker UI without a follow-up sort.
    async fn list_for_subjects(
        &self,
        subject_types: &[&str],
        subject_ids: &[Uuid],
    ) -> Result<Vec<DriveWithRootName>, DriveRepositoryError>;

    /// List every drive on the system, regardless of caller membership.
    ///
    /// Used by the admin panel's `GET /api/admin/drives`. Distinct from
    /// `list_for_subjects` (which filters by `role_grants`) because an
    /// admin who creates a shared drive for someone else has no grant
    /// on it — but still needs to see, audit, and manage it. The HTTP
    /// gate (admin-only middleware) is what makes the unrestricted
    /// listing safe; no role-based filtering happens here.
    ///
    /// Returns rows ordered by display name. `caller_role` is left
    /// unset on the returned `DriveWithRootName` — the admin is not
    /// necessarily a member, so the per-drive role would be misleading
    /// here.
    async fn list_all(&self) -> Result<Vec<DriveWithRootName>, DriveRepositoryError>;
}

/// Convenience: convert the canonical kind discriminator from its SQL
/// form into the typed enum. Mirrored on the entity for symmetry.
impl DriveKind {
    pub fn from_sql(s: &str) -> Result<Self, DriveRepositoryError> {
        DriveKind::parse(s).ok_or_else(|| DriveRepositoryError::InvalidKind(s.to_owned()))
    }
}

/// Locate the user's home root folder within a generic list of items,
/// identifying it by **drive ownership** (never by folder name — users
/// can rename their home).
///
/// `id_fn` extracts a candidate `Uuid` from each item. The callsite
/// commonly works with `FolderDto` (whose `id` is a `String`); the
/// closure is `|f| Uuid::parse_str(&f.id).ok()`. Items whose ids can't
/// be parsed are simply skipped — `position` ignores them.
///
/// Defined as a free function (not a trait method) so the
/// `DriveRepository` trait stays `dyn`-compatible. Generic over both
/// the repo (`R`) and the item shape (`T`); accepts both concrete repo
/// types and `&dyn DriveRepository`.
///
/// Returns `None` when:
///   * The user has no default drive (external users, pre-hook accounts).
///   * The user's home root folder id isn't present in `items`.
///   * The repo lookup errored (storage error is swallowed to None —
///     callers wanting fail-loud semantics should call
///     `home_root_folder_id_for` directly).
pub async fn position_of_user_home_root_folder<R, T>(
    drive_repo: &R,
    user_id: Uuid,
    items: &[T],
    id_fn: impl Fn(&T) -> Option<Uuid>,
) -> Option<usize>
where
    R: DriveRepository + ?Sized,
{
    let home_id = drive_repo
        .home_root_folder_id_for(user_id)
        .await
        .ok()
        .flatten()?;
    items.iter().position(|item| id_fn(item) == Some(home_id))
}
