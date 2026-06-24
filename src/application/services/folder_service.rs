use crate::application::dtos::cursor::PageCursor;
use crate::application::dtos::folder_dto::{
    CreateFolderDto, FolderDto, FolderResourceCursor, FolderResourceRow, ListResourcesOptions,
    MoveFolderDto, RenameFolderDto,
};
use crate::application::ports::authorization_ports::AuthorizationEngine;
use crate::application::ports::folder_ports::FolderUseCase;
use crate::common::errors::{DomainError, ErrorKind};
use crate::domain::repositories::folder_repository::FolderRepository;
use crate::domain::services::authorization::{Permission, Resource, Subject};
use crate::domain::services::path_service::{StoragePath, validate_storage_name};
use crate::infrastructure::repositories::pg::folder_db_repository::FolderDbRepository;
use crate::infrastructure::services::pg_acl_engine::PgAclEngine;
use std::sync::Arc;
use uuid::Uuid;

/// Implementation of the use case for folder operations
pub struct FolderService {
    folder_storage: Arc<FolderDbRepository>,
    authz: Arc<PgAclEngine>,
}

impl FolderService {
    /// Creates a new folder service
    pub fn new(folder_storage: Arc<FolderDbRepository>, authz: Arc<PgAclEngine>) -> Self {
        Self {
            folder_storage,
            authz,
        }
    }

    /// Batch counterpart of `get_folder`: resolve many folder ids in ONE
    /// query instead of one per id. Like `get_folder` it performs no
    /// per-folder authorization — both current callers (ACL grant listing,
    /// NextCloud favorites REPORT) resolve ids already vetted by the
    /// authorization engine or the favorites table. Missing or trashed ids
    /// are absent from the result; callers re-associate by `id`.
    pub async fn get_folders_by_ids(&self, ids: &[String]) -> Result<Vec<FolderDto>, DomainError> {
        let folders = self.folder_storage.get_folders_by_ids(ids).await?;
        Ok(folders.into_iter().map(FolderDto::from).collect())
    }

    /// Helper: parse a folder id string into a `Resource::Folder`. Returns
    /// `DomainError::not_found` on parse error (anti-enumeration — the same
    /// error as "folder does not exist").
    fn folder_resource(id: &str) -> Result<Resource, DomainError> {
        Uuid::parse_str(id)
            .map(Resource::Folder)
            .map_err(|_| DomainError::not_found("Folder", id))
    }

    /// Creates a stub implementation for testing and middleware
    pub fn new_stub() -> impl FolderUseCase {
        struct FolderServiceStub;

        impl FolderUseCase for FolderServiceStub {
            async fn require_permission(
                &self,
                _caller_id: Uuid,
                _permission: Permission,
                _folder_id: &str,
            ) -> Result<(), DomainError> {
                Ok(())
            }
            async fn create_folder_with_perms(
                &self,
                _dto: CreateFolderDto,
                _user_id: Uuid,
            ) -> Result<FolderDto, DomainError> {
                Ok(FolderDto::empty())
            }

            async fn get_folder(&self, _id: &str) -> Result<FolderDto, DomainError> {
                Ok(FolderDto::empty())
            }

            async fn get_folder_with_perms(
                &self,
                _id: &str,
                _caller_id: Uuid,
            ) -> Result<FolderDto, DomainError> {
                Ok(FolderDto::empty())
            }

            async fn get_folder_by_path(
                &self,
                _path: &str,
                _drive_id: Uuid,
            ) -> Result<FolderDto, DomainError> {
                Ok(FolderDto::empty())
            }

            async fn list_folders(
                &self,
                _parent_id: Option<&str>,
            ) -> Result<Vec<FolderDto>, DomainError> {
                Ok(vec![])
            }

            async fn list_folders_with_perms(
                &self,
                _parent_id: Option<&str>,
                _owner_id: Uuid,
            ) -> Result<Vec<FolderDto>, DomainError> {
                Ok(vec![])
            }

            async fn list_folders_paginated(
                &self,
                _parent_id: Option<&str>,
                _pagination: &crate::application::dtos::pagination::PaginationRequestDto,
            ) -> Result<
                crate::application::dtos::pagination::PaginatedResponseDto<FolderDto>,
                DomainError,
            > {
                Ok(
                    crate::application::dtos::pagination::PaginatedResponseDto::new(
                        vec![],
                        0,
                        10,
                        0,
                    ),
                )
            }

            async fn list_folders_paginated_with_perms(
                &self,
                _parent_id: Option<&str>,
                _owner_id: Uuid,
                _pagination: &crate::application::dtos::pagination::PaginationRequestDto,
            ) -> Result<
                crate::application::dtos::pagination::PaginatedResponseDto<FolderDto>,
                DomainError,
            > {
                Ok(
                    crate::application::dtos::pagination::PaginatedResponseDto::new(
                        vec![],
                        0,
                        10,
                        0,
                    ),
                )
            }

            async fn rename_folder_with_perms(
                &self,
                _id: &str,
                _dto: RenameFolderDto,
                _caller_id: Uuid,
            ) -> Result<FolderDto, DomainError> {
                Ok(FolderDto::empty())
            }

            async fn move_folder_with_perms(
                &self,
                _id: &str,
                _dto: MoveFolderDto,
                _caller_id: Uuid,
            ) -> Result<FolderDto, DomainError> {
                Ok(FolderDto::empty())
            }

            async fn delete_folder_with_perms(
                &self,
                _id: &str,
                _caller_id: Uuid,
            ) -> Result<(), DomainError> {
                Ok(())
            }
        }

        FolderServiceStub
    }
}

impl FolderUseCase for FolderService {
    /// Verifies the caller has the given permition on a resource
    /// `folder_id`. `None` is the caller's root namespace and always allowed.
    ///
    /// Returns `Ok(())` when permitted, `DomainError::not_found(...)` when not
    /// (anti-enumeration — same error as "folder doesn't exist").
    ///
    /// Used by handlers that need a fail-fast pre-check BEFORE spooling
    /// large request bodies (file upload, chunked upload). The authoritative
    /// check happens again inside the upload/management services before any
    /// DB write — this is a UX/resource optimization, not a security boundary.
    async fn require_permission(
        &self,
        caller_id: Uuid,
        permission: Permission,
        folder_id: &str,
    ) -> Result<(), DomainError> {
        let resource = Self::folder_resource(folder_id)?;
        self.authz
            .require(Subject::User(caller_id), permission, resource)
            .await
    }

    /// Creates a new folder
    async fn create_folder_with_perms(
        &self,
        dto: CreateFolderDto,
        caller_id: Uuid,
    ) -> Result<FolderDto, DomainError> {
        if let Err(reason) = validate_storage_name(&dto.name) {
            return Err(DomainError::validation_error(format!(
                "Invalid folder name '{}': {reason}",
                dto.name
            )));
        }

        let Some(parent_id) = dto.parent_id.as_deref() else {
            return Err(DomainError::validation_error(
                "Root folder creation is reserved for registration",
            ));
        };
        let parent_resource = Self::folder_resource(parent_id)?;
        self.authz
            .require(
                Subject::User(caller_id),
                Permission::Create,
                parent_resource,
            )
            .await?;

        let folder = self
            .folder_storage
            .create_folder(dto.name, dto.parent_id, caller_id)
            .await?;
        Ok(FolderDto::from(folder))
    }

    async fn list_subtree_folders(&self, folder_id: &str) -> Result<Vec<FolderDto>, DomainError> {
        let folders = self.folder_storage.list_subtree_folders(folder_id).await?;
        Ok(folders.into_iter().map(FolderDto::from).collect())
    }

    /// Gets a folder by its ID
    async fn get_folder(&self, id: &str) -> Result<FolderDto, DomainError> {
        let folder = self.folder_storage.get_folder(id).await.map_err(|e| {
            DomainError::internal_error(
                "FolderStorage",
                format!("Failed to get folder with ID: {}: {}", id, e),
            )
        })?;

        Ok(FolderDto::from(folder))
    }

    /// Gets a folder by its ID, enforcing that `caller_id` has `Read` access
    /// (via ownership or a grant — including cascading from ancestor folders).
    async fn get_folder_with_perms(
        &self,
        id: &str,
        caller_id: Uuid,
    ) -> Result<FolderDto, DomainError> {
        self.authz
            .require(
                Subject::User(caller_id),
                Permission::Read,
                Self::folder_resource(id)?,
            )
            .await?;
        self.get_folder(id).await
    }

    /// Gets a folder by its path, scoped to a drive.
    async fn get_folder_by_path(
        &self,
        path: &str,
        drive_id: Uuid,
    ) -> Result<FolderDto, DomainError> {
        let storage_path = StoragePath::from_string(path);

        let folder = self
            .folder_storage
            .get_folder_by_path(&storage_path, drive_id)
            .await
            .map_err(|e| {
                DomainError::internal_error(
                    "FolderStorage",
                    format!("Failed to get folder at path: {}: {}", path, e),
                )
            })?;

        Ok(FolderDto::from(folder))
    }

    /// Lists folders within a parent folder
    async fn list_folders(&self, parent_id: Option<&str>) -> Result<Vec<FolderDto>, DomainError> {
        let folders = self
            .folder_storage
            .list_folders(parent_id)
            .await
            .map_err(|e| {
                tracing::warn!("errror while fetching folders {}", e);
                DomainError::internal_error(
                    "FolderStorage",
                    format!("Failed to list folders in parent: {:?}: {}", parent_id, e),
                )
            })?;

        // Convert to DTOs
        Ok(folders.into_iter().map(FolderDto::from).collect())
    }

    /// Lists folders scoped to a specific owner.
    ///
    /// **Note (post PR 3):** the self-heal block that auto-created a
    /// home folder when listing returned empty has been removed.
    /// `PersonalDriveLifecycleHook` (registered on `UserLifecycleService`)
    /// now provisions the folder on `on_user_created` / `on_user_login`,
    /// idempotently, so the listing path no longer needs to self-heal.
    async fn list_folders_with_perms(
        &self,
        parent_id: Option<&str>,
        caller_id: Uuid,
    ) -> Result<Vec<FolderDto>, DomainError> {
        if let Some(parent_id_unwrapped) = parent_id {
            // check authorisation
            self.authz
                .require(
                    Subject::User(caller_id),
                    Permission::Read,
                    Self::folder_resource(parent_id_unwrapped)?,
                )
                .await?;
            return self.list_folders(parent_id).await;
        }
        // No parent → list the user's root folders.
        let folders = self
            .folder_storage
            .list_folders_by_owner(parent_id, caller_id)
            .await
            .map_err(|e| {
                DomainError::internal_error(
                    "FolderStorage",
                    format!(
                        "Failed to list folders for owner '{}' in parent {:?}: {}",
                        caller_id, parent_id, e
                    ),
                )
            })?;
        Ok(folders.into_iter().map(FolderDto::from).collect())
    }

    /// Lists folders with pagination
    async fn list_folders_paginated(
        &self,
        parent_id: Option<&str>,
        pagination: &crate::application::dtos::pagination::PaginationRequestDto,
    ) -> Result<crate::application::dtos::pagination::PaginatedResponseDto<FolderDto>, DomainError>
    {
        let pagination = pagination.validate_and_adjust();

        let (folders, total_items) = self
            .folder_storage
            .list_folders_paginated(parent_id, pagination.offset(), pagination.limit(), true)
            .await
            .map_err(|e| {
                DomainError::internal_error(
                    "FolderStorage",
                    format!(
                        "Failed to list folders with pagination in parent: {:?}: {}",
                        parent_id, e
                    ),
                )
            })?;

        let total = total_items.unwrap_or(folders.len());

        let response = crate::application::dtos::pagination::PaginatedResponseDto::new(
            folders.into_iter().map(FolderDto::from).collect(),
            pagination.page,
            pagination.page_size,
            total,
        );

        Ok(response)
    }

    /// Lists folders with pagination, scoped to a specific owner.
    async fn list_folders_paginated_with_perms(
        &self,
        parent_id: Option<&str>,
        owner_id: Uuid,
        pagination: &crate::application::dtos::pagination::PaginationRequestDto,
    ) -> Result<crate::application::dtos::pagination::PaginatedResponseDto<FolderDto>, DomainError>
    {
        let pagination = pagination.validate_and_adjust();

        if let Some(parent_id_unwrapped) = parent_id {
            self.authz
                .require(
                    Subject::User(owner_id),
                    Permission::Read,
                    Self::folder_resource(parent_id_unwrapped)?,
                )
                .await?;
            return self.list_folders_paginated(parent_id, &pagination).await;
        } else {
            let (folders, total_items) = self
            .folder_storage
            .list_folders_by_owner_paginated(
                parent_id,
                owner_id,
                pagination.offset(),
                pagination.limit(),
                true,
            )
            .await
            .map_err(|e| {
                DomainError::internal_error(
                    "FolderStorage",
                    format!(
                        "Failed to list folders for owner '{}' with pagination in parent {:?}: {}",
                        owner_id, parent_id, e
                    ),
                )
            })?;

            let total = total_items.unwrap_or(folders.len());

            let response = crate::application::dtos::pagination::PaginatedResponseDto::new(
                folders.into_iter().map(FolderDto::from).collect(),
                pagination.page,
                pagination.page_size,
                total,
            );

            Ok(response)
        }
    }

    /// Renames a folder after verifying the caller has `Update` permission.
    async fn rename_folder_with_perms(
        &self,
        id: &str,
        dto: RenameFolderDto,
        caller_id: Uuid,
    ) -> Result<FolderDto, DomainError> {
        if let Err(reason) = validate_storage_name(&dto.name) {
            return Err(DomainError::validation_error(format!(
                "Invalid folder name '{}': {reason}",
                dto.name
            )));
        }

        // Drive roots double as the drive's display name (per drive.md §3,
        // `drives.name` is sourced from `storage.folders.name` of the row
        // pointed at by `root_folder_id`). Per drive.md §6 the rename is
        // Owner-only — but with `Permission::Update` that's leaky because
        // every Editor of the drive has Update on every folder in the
        // drive, including the root. So we promote the requirement to
        // `Manage` for root folders. A root is identified by
        // `parent_id IS NULL`; that's the same property the drive seeder
        // and the drive-of-resource resolver rely on, so no schema-level
        // assumption shifts here.
        let folder = self.folder_storage.get_folder(id).await.map_err(|e| {
            DomainError::internal_error(
                "FolderStorage",
                format!("Failed to look up folder before rename: {id}: {e}"),
            )
        })?;
        let required_perm = if folder.parent_id().is_none() {
            Permission::Manage
        } else {
            Permission::Update
        };

        self.authz
            .require(
                Subject::User(caller_id),
                required_perm,
                Self::folder_resource(id)?,
            )
            .await?;

        let folder = self
            .folder_storage
            .rename_folder(id, dto.name, caller_id)
            .await
            .map_err(|e| {
                DomainError::internal_error(
                    "FolderStorage",
                    format!("Failed to rename folder with ID: {}: {}", id, e),
                )
            })?;

        Ok(FolderDto::from(folder))
    }

    /// Moves a folder to a new parent. Requires `Update` on the source and
    /// `Create` on the destination parent (if any).
    async fn move_folder_with_perms(
        &self,
        id: &str,
        dto: MoveFolderDto,
        caller_id: Uuid,
    ) -> Result<FolderDto, DomainError> {
        let source_resource = Self::folder_resource(id)?;
        self.authz
            .require(
                Subject::User(caller_id),
                Permission::Update,
                source_resource,
            )
            .await?;

        if let Some(parent_id) = &dto.parent_id {
            // Cannot move a folder into itself (cycle guard).
            if parent_id == id {
                return Err(DomainError::new(
                    ErrorKind::InvalidInput,
                    "Folder",
                    "Cannot move a folder into itself",
                ));
            }
            let parent_resource = Self::folder_resource(parent_id)?;
            self.authz
                .require(
                    Subject::User(caller_id),
                    Permission::Create,
                    parent_resource,
                )
                .await?;
            // TODO: full descendant-cycle check (moving a folder into one of its own descendants)
        }

        let parent_ref = dto.parent_id.as_deref();
        let folder = self
            .folder_storage
            .move_folder(id, parent_ref, caller_id)
            .await
            .map_err(|e| {
                DomainError::internal_error(
                    "FolderStorage",
                    format!("Failed to move folder with ID: {}: {}", id, e),
                )
            })?;

        Ok(FolderDto::from(folder))
    }

    /// Deletes a folder after verifying the caller has `Delete` permission.
    /// The DB trigger `trg_cleanup_grants_folder` cleans up `access_grants`
    /// rows targeting the deleted folder automatically.
    async fn delete_folder_with_perms(&self, id: &str, caller_id: Uuid) -> Result<(), DomainError> {
        self.authz
            .require(
                Subject::User(caller_id),
                Permission::Delete,
                Self::folder_resource(id)?,
            )
            .await?;

        self.folder_storage.delete_folder(id).await.map_err(|e| {
            DomainError::internal_error(
                "FolderStorage",
                format!("Failed to delete folder with ID: {}: {}", id, e),
            )
        })
    }
}

// ── FolderService — cursor-paginated resource listing ────────────────────────

impl FolderService {
    /// Cursor-paginated listing of sub-folders **and** files inside `parent_id`.
    ///
    /// Enforces `Permission::Read` on the parent folder before querying.
    /// `order_by` controls both the SQL `ORDER BY` and the cursor encoding.
    /// `kinds` filters the result to only the specified resource types.
    pub async fn list_resources_paged_with_perms(
        &self,
        parent_id: &str,
        caller_id: Uuid,
        opts: ListResourcesOptions<'_>,
    ) -> Result<(Vec<FolderResourceRow>, Option<String>), DomainError> {
        // 1. AuthZ — same check as list_folders_with_perms
        self.authz
            .require(
                Subject::User(caller_id),
                Permission::Read,
                Self::folder_resource(parent_id)?,
            )
            .await?;

        let pid =
            Uuid::parse_str(parent_id).map_err(|_| DomainError::not_found("Folder", parent_id))?;

        let ListResourcesOptions {
            limit,
            cursor,
            order_by,
            kinds,
            reverse,
        } = opts;

        // 2. Fetch limit+1 rows so we can detect has_next
        let mut rows = self
            .folder_storage
            .list_resources_paged(pid, limit + 1, cursor.as_ref(), order_by, kinds, reverse)
            .await?;

        // 3. Detect has_next, build encoded next cursor
        let next_cursor = if rows.len() > limit {
            let last = &rows[limit - 1];
            let c = build_folder_resource_cursor(last, order_by, reverse);
            rows.truncate(limit);
            Some(c.encode())
        } else {
            None
        };

        Ok((rows, next_cursor))
    }
}

/// Build the next-page cursor from the last row of the current page.
/// `reverse` is stored in the cursor so subsequent pages use the same order.
fn build_folder_resource_cursor(
    row: &FolderResourceRow,
    order_by: &str,
    reverse: bool,
) -> FolderResourceCursor {
    match order_by {
        "type" => FolderResourceCursor {
            order_by: "type".to_owned(),
            resource_id: row.id,
            sort_str: Some(row.sort_str.clone()),
            sort_int: Some(row.type_order),
            sort_ts: None,
            reverse,
        },
        "modified_at" => FolderResourceCursor {
            order_by: "modified_at".to_owned(),
            resource_id: row.id,
            sort_str: None,
            sort_int: None,
            sort_ts: Some(row.modified_at),
            reverse,
        },
        "created_at" => FolderResourceCursor {
            order_by: "created_at".to_owned(),
            resource_id: row.id,
            sort_str: None,
            sort_int: None,
            sort_ts: Some(row.created_at),
            reverse,
        },
        "size" => FolderResourceCursor {
            order_by: "size".to_owned(),
            resource_id: row.id,
            sort_str: None,
            sort_int: Some(row.size),
            sort_ts: None,
            reverse,
        },
        _ => FolderResourceCursor {
            // "name" (default): sort_int = folder_first (0 or 1)
            order_by: "name".to_owned(),
            resource_id: row.id,
            sort_str: Some(row.sort_str.clone()),
            sort_int: Some(i64::from(row.folder_first)),
            sort_ts: None,
            reverse,
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PersonalDriveLifecycleHook
//
// Owns home-folder provisioning policy. Replaces:
//   - the 4 eager `create_personal_folder` calls in AuthApplicationService
//     (register / setup_create_admin / admin_create_user / OIDC JIT)
//   - the self-heal at `list_folders_with_perms` when no root folders exist
//
// Lives in this file (not under a centralised `lifecycle/` directory)
// because the folder service owns home-folder policy — see the
// "owner-located convention" note in
// `docs/architecture/user-lifecycle.md`.
// ─────────────────────────────────────────────────────────────────────────────

use async_trait::async_trait;

use crate::application::ports::user_lifecycle::{DeletionMode, LogoutReason, UserLifecycleHook};
use crate::domain::entities::user::User;

/// Lifecycle hook: provisions a user's default Personal drive at first
/// login (replaces the legacy `My Folder - <username>` wrapper as of D0).
///
/// Two writes happen on first provisioning:
///   1. A row in `storage.drives` with `kind='personal'`,
///      `default_for_user=<uid>`, and the user's quota carried over from
///      `auth.users.storage_quota_bytes`.
///   2. An Owner role grant in `storage.role_grants` so the user can
///      read/write/manage their own drive (the engine's owner short-
///      circuit applies to folders/files but not drives — see
///      `pg_acl_engine::check_inner` D0-6 rewrite).
///
/// Both writes are idempotent: `find_default_for_user` short-circuits
/// when the drive already exists; `set_role` is an UPSERT that no-ops
/// when the Owner row is already present.
pub struct PersonalDriveLifecycleHook {
    drive_repo: Arc<dyn crate::domain::repositories::drive_repository::DriveRepository>,
    // The `AuthorizationEngine` trait isn't `dyn`-compatible (native
    // async-fn-in-trait methods are not object-safe), so we hold the
    // concrete engine. This matches the convention already used by
    // `AppState.authorization`. Only the idempotent-rerun path uses it
    // now; the create path goes through the repo's atomic CTE which
    // writes the role_grant inline.
    authorization: Arc<crate::infrastructure::services::pg_acl_engine::PgAclEngine>,
}

impl PersonalDriveLifecycleHook {
    pub fn new(
        drive_repo: Arc<dyn crate::domain::repositories::drive_repository::DriveRepository>,
        authorization: Arc<crate::infrastructure::services::pg_acl_engine::PgAclEngine>,
    ) -> Self {
        Self {
            drive_repo,
            authorization,
        }
    }

    /// Idempotent provisioning shared by `on_user_created` and
    /// `on_user_login`. External users are skipped per tip #2 in the
    /// trait docstring — they have no resources of their own, only
    /// grants on other users' resources.
    async fn provision_if_needed(&self, user: &User) -> Result<(), DomainError> {
        use crate::domain::repositories::drive_repository::DriveRepositoryError;
        use crate::domain::services::authorization::{Resource, Role, Subject};

        if user.is_external() {
            return Ok(());
        }

        // Idempotent shortcut: if the user already has a default drive,
        // the atomic CTE already ran on a prior turn. The CTE writes
        // the Owner role_grant inline, so there's nothing to repair —
        // but we still re-emit the grant via `set_role` (UPSERT-safe)
        // to cover the historical case where a pre-CTE provisioning
        // path partially completed (drive created, grant missing).
        match self.drive_repo.find_default_for_user(user.id()).await {
            Ok(drive_with_name) => {
                self.authorization
                    .set_role(
                        user.id(),
                        Subject::User(user.id()),
                        Role::Owner,
                        Resource::Drive(drive_with_name.drive.id),
                        None,
                    )
                    .await
                    .map(|_grant| ())?;
                return Ok(());
            }
            Err(DriveRepositoryError::NotFound(_)) => { /* fall through to create */ }
            Err(e) => {
                return Err(DomainError::internal_error(
                    "PersonalDriveHook",
                    format!("find_default lookup: {e}"),
                ));
            }
        }

        // One atomic CTE — drive row + root folder ("Personal",
        // parent_id=NULL, drive_id pinned) + drives.root_folder_id
        // wire-up + Owner role_grant. Single SQL statement, atomic
        // against server crash mid-sequence (docs/plan/drive.md §3).
        let drive_with_name = self
            .drive_repo
            .create_personal_drive_atomic(user.id(), Some(user.storage_quota_bytes()))
            .await
            .map_err(|e| {
                DomainError::internal_error(
                    "PersonalDriveHook",
                    format!("create_personal_drive_atomic: {e}"),
                )
            })?;

        tracing::info!(
            target: "user_lifecycle",
            hook = "personal_drive",
            user_id = %user.id(),
            drive_id = %drive_with_name.drive.id,
            root_folder_id = %drive_with_name.drive.root_folder_id,
            "Default personal drive + root folder + owner grant provisioned (atomic CTE)"
        );
        Ok(())
    }
}

#[async_trait]
impl UserLifecycleHook for PersonalDriveLifecycleHook {
    fn name(&self) -> &'static str {
        "personal_drive"
    }

    async fn on_user_created(&self, user: &User) -> Result<(), DomainError> {
        self.provision_if_needed(user).await
    }

    /// Login is the safety net — if `on_user_created` failed at any
    /// earlier point (or the user was created in a flow that pre-dated
    /// this hook), provisioning happens here on next login.
    async fn on_user_login(&self, user: &User) -> Result<(), DomainError> {
        self.provision_if_needed(user).await
    }

    async fn on_user_logout(&self, _user: &User, _reason: LogoutReason) -> Result<(), DomainError> {
        // Drives don't react to logout. Explicit no-op per the
        // "no defaults" convention.
        Ok(())
    }

    async fn on_user_deleted(
        &self,
        user: &User,
        mode: DeletionMode,
        _tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), DomainError> {
        // `storage.drives.default_for_user` has ON DELETE CASCADE
        // referencing `auth.users(id)`, and `storage.folders.drive_id`
        // / `storage.files.drive_id` both have ON DELETE CASCADE on
        // `storage.drives(id)` (M3). So a user delete cascades:
        // user → drive → folders → files in one transaction.
        //
        // The hook emits a per-mode tracing event so audit can tell
        // AdminDelete (currently recoverable only via DB-level rollback
        // before commit) from GdprPurge (no sweeper exists yet — the
        // variant is reserved for a future PR that adds retention).
        tracing::info!(
            target: "user_lifecycle",
            hook = "personal_drive",
            user_id = %user.id(),
            mode = ?mode,
            "Personal drive (and tree) will be removed via FK CASCADE on user delete"
        );
        Ok(())
    }
}
