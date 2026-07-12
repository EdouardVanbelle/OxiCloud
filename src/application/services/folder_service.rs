use crate::application::dtos::cursor::PageCursor;
use crate::application::dtos::folder_dto::{
    CreateFolderDto, FolderDto, FolderResourceCursor, FolderResourceRow, ListResourcesOptions,
    MoveFolderDto, RenameFolderDto,
};
use crate::application::ports::authorization_ports::AuthorizationEngine;
use crate::application::ports::file_lifecycle::FileLifecycleHook;
use crate::application::ports::folder_ports::FolderUseCase;
use crate::application::services::file_lifecycle_service::FileLifecycleService;
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
    /// File lifecycle dispatcher. Carried so `delete_folder_with_perms`
    /// can fire `on_file_deleted` for every file the PG cascade is about
    /// to reap. Always present — the dispatcher itself is a no-op when
    /// no hooks are registered, so callers don't need an Option branch.
    file_lifecycle: Arc<FileLifecycleService>,
    /// Drive repository — used by D5's `forbid_cross_drive_move` gate
    /// on `move_folder_with_perms`. Optional so stubs / test factories
    /// can build the service without wiring the full drive repo; in
    /// that case the cross-drive move check is skipped (the policy is
    /// silently off). Production DI wires it via `with_drive_repo`.
    drive_repo: Option<Arc<dyn crate::domain::repositories::drive_repository::DriveRepository>>,
    /// Storage-usage service — used to pre-check the destination
    /// drive's `used_bytes + subtree_bytes ≤ quota_bytes` invariant
    /// on cross-drive MOVE. Silently skipped when unwired (stubs).
    storage_usage:
        Option<Arc<crate::application::services::storage_usage_service::StorageUsageService>>,
}

impl FolderService {
    /// Creates a new folder service
    pub fn new(
        folder_storage: Arc<FolderDbRepository>,
        authz: Arc<PgAclEngine>,
        file_lifecycle: Arc<FileLifecycleService>,
    ) -> Self {
        Self {
            folder_storage,
            authz,
            file_lifecycle,
            drive_repo: None,
            storage_usage: None,
        }
    }

    /// Wires the drive repository, enabling D5
    /// `forbid_cross_drive_move` enforcement on
    /// `move_folder_with_perms`. Without it, the gate is silently
    /// skipped.
    pub fn with_drive_repo(
        mut self,
        drive_repo: Arc<dyn crate::domain::repositories::drive_repository::DriveRepository>,
    ) -> Self {
        self.drive_repo = Some(drive_repo);
        self
    }

    /// Wires the storage-usage service so `move_folder_with_perms`
    /// can pre-check the destination drive's quota on cross-drive
    /// folder moves.
    pub fn with_storage_usage(
        mut self,
        storage_usage: Arc<
            crate::application::services::storage_usage_service::StorageUsageService,
        >,
    ) -> Self {
        self.storage_usage = Some(storage_usage);
        self
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
        // No parent → list the caller's readable root folders. The
        // predicate scopes by drive-membership grants (post-PR-B),
        // closing the pre-D7 gap where the legacy `user_id` filter
        // surfaced admin-created folders that admin had no role on.
        let folders = self
            .folder_storage
            .list_root_folders_for_caller(caller_id)
            .await
            .map_err(|e| {
                DomainError::internal_error(
                    "FolderStorage",
                    format!("Failed to list root folders for caller '{caller_id}': {e}"),
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
                .list_root_folders_for_caller_paginated(
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
                            "Failed to list root folders for caller '{}' with pagination: {}",
                            owner_id, e
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

        // D5 `forbid_cross_drive_move` + D6 `resource.moved_between_drives`
        // audit share the same src/dst lookup. Gate before the move,
        // audit after a successful move when the two drives differ.
        // Skipped for parent_id=None (root namespace, same-drive
        // semantics) and when drive_repo isn't wired (stubs/tests) —
        // same shape as `move_file_with_perms`.
        let mut cross_drive: Option<(Uuid, Uuid)> = None;
        if let Some(drive_repo) = &self.drive_repo
            && let Some(parent_id) = &dto.parent_id
        {
            let src_folder_uuid =
                Uuid::parse_str(id).map_err(|_| DomainError::not_found("Folder", id))?;
            let dst_folder_uuid = Uuid::parse_str(parent_id)
                .map_err(|_| DomainError::not_found("Folder", parent_id.as_str()))?;
            let (src_drive_id, src_policies) = drive_repo
                .get_drive_id_and_policies_for_folder(src_folder_uuid)
                .await
                .map_err(|e| {
                    DomainError::internal_error("Drive", format!("source drive lookup: {e:?}"))
                })?;
            let dst_drive_id = drive_repo
                .drive_id_for_folder(dst_folder_uuid)
                .await
                .map_err(|e| {
                    DomainError::internal_error("Drive", format!("destination drive lookup: {e:?}"))
                })?;
            if src_drive_id != dst_drive_id {
                src_policies.refuse_cross_drive_move(
                    crate::domain::entities::drive::CrossDriveMoveGateContext {
                        caller_id,
                        resource_type: "folder",
                        resource_id: src_folder_uuid,
                        src_drive_id,
                        dst_drive_id,
                    },
                )?;
                // Destination drive quota: sum the moved subtree's
                // non-trashed files and refuse if the destination
                // couldn't hold them. Same 507 shape as the file
                // path + upload path — DomainError::QuotaExceeded
                // maps at the AppError boundary.
                if let Some(storage_usage) = &self.storage_usage {
                    let subtree_bytes = storage_usage.folder_subtree_bytes(src_folder_uuid).await?;
                    if let Ok(subtree_u64) = u64::try_from(subtree_bytes) {
                        storage_usage
                            .check_drive_quota(dst_drive_id, subtree_u64)
                            .await?;
                    }
                }
                cross_drive = Some((src_drive_id, dst_drive_id));
            }
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

        // Cross-drive move flushes the authz engine's `owner_cache`
        // — every descendant's cached `Resource → drive_id` mapping
        // just got stale via the cascade trigger, and we don't (yet)
        // walk the subtree to invalidate individually. Small perf
        // cost (single JOIN per resource touched over the next
        // minute) versus a stale-authz bug where destination-drive
        // Owner cascades don't apply to moved content.
        if cross_drive.is_some() {
            self.authz.invalidate_owner_cache_all().await;
        }

        // D6 audit: only emit when the move crossed a drive boundary.
        // The cascade trigger has already propagated drive_id to the
        // subtree at this point (see migration
        // `20260807000000_cascade_drive_id_on_folder_move.sql`).
        if let Some((src_drive_id, dst_drive_id)) = cross_drive {
            tracing::info!(
                target: "audit",
                event = "resource.moved_between_drives",
                resource_type = "folder",
                resource_id = %folder.id(),
                src_drive_id = %src_drive_id,
                dst_drive_id = %dst_drive_id,
                by = %caller_id,
                "📦 folder moved between drives",
            );
        }

        Ok(FolderDto::from(folder))
    }

    /// Deletes a folder after verifying the caller has `Delete` permission.
    /// The DB trigger `trg_cleanup_grants_folder` cleans up `access_grants`
    /// rows targeting the deleted folder automatically.
    ///
    /// Enumerates the subtree's file ids BEFORE the bulk DELETE so
    /// `on_file_deleted` fires per file the PG cascade is about to reap —
    /// without this, file-id-keyed lifecycle data (e.g. `ext-{file_id}.jpg`
    /// video thumbnails, moka cache entries) leaks past the cascade.
    /// Same shape `clear_trash_in` uses (`trash_service.rs:804-846`).
    async fn delete_folder_with_perms(&self, id: &str, caller_id: Uuid) -> Result<(), DomainError> {
        self.authz
            .require(
                Subject::User(caller_id),
                Permission::Delete,
                Self::folder_resource(id)?,
            )
            .await?;

        // Snapshot the file ids BEFORE the bulk DELETE — the rows are gone
        // afterward. Failure to enumerate is non-fatal (logged in the repo
        // method); the delete proceeds and only file-id-keyed cleanup is
        // skipped (blob-keyed thumbnails still get reaped by GC).
        let cascaded_file_ids = self
            .folder_storage
            .list_file_ids_in_subtree(id)
            .await
            .unwrap_or_default();

        self.folder_storage.delete_folder(id).await.map_err(|e| {
            DomainError::internal_error(
                "FolderStorage",
                format!("Failed to delete folder with ID: {}: {}", id, e),
            )
        })?;

        for file_id in &cascaded_file_ids {
            self.file_lifecycle.on_file_deleted(file_id);
        }

        Ok(())
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

// ────────────────────────────────────────────────────────────────────────────
// Integration test — verifies the folder-cascade hook fix lands `on_file_deleted`
// for every file the PG cascade reaps when a folder is permanently deleted.
//
// Background: `delete_folder_with_perms` issues a bulk SQL DELETE that the PG
// `ON DELETE CASCADE` fans out to descendant folders + files. Without
// service-layer enumeration, file-id-keyed lifecycle data (thumbnails keyed
// on `ext-{file_id}.jpg`, moka cache entries, future per-file metadata)
// silently leaks. See [[bug-folder-cascade-hooks-missing]] in agent memory.
//
// How to run:
//   bash tests/common/spawn-db.sh
//   RUSTFLAGS='--cfg integration_tests' cargo test \
//       -p oxicloud --lib folder_service::cascade_hook_integration_tests
// ────────────────────────────────────────────────────────────────────────────
#[cfg(integration_tests)]
#[allow(dead_code)]
mod cascade_hook_integration_tests {
    use super::*;
    use crate::application::ports::blob_storage_ports::BlobStorageBackend;
    use crate::application::ports::file_lifecycle::FileLifecycleHook;
    use crate::infrastructure::repositories::pg::SubjectGroupPgRepository;
    use crate::infrastructure::repositories::pg::file_blob_read_repository::FileBlobReadRepository;
    use crate::infrastructure::services::dedup_service::DedupService;
    use crate::infrastructure::services::local_blob_backend::LocalBlobBackend;
    use crate::integration_test_support::{ensure_clean_test_db, test_db_url};
    use sqlx::Row;
    use sqlx::postgres::PgPoolOptions;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// Records every `on_file_deleted` call so the test can assert the
    /// exact set of file ids the cascade fired hooks for. Other lifecycle
    /// methods are no-ops — this fix only touches the deletion path.
    #[derive(Default)]
    struct RecordingHook {
        deleted: Mutex<Vec<String>>,
    }

    impl FileLifecycleHook for RecordingHook {
        fn on_file_created(
            &self,
            _file_id: &str,
            _blob_hash: &str,
            _content_type: &str,
            _is_new_blob: bool,
        ) {
        }
        fn on_file_copied(
            &self,
            _file_id: &str,
            _blob_hash: &str,
            _content_type: &str,
            _source_file_id: &str,
        ) {
        }
        fn on_file_updated(&self, _file_id: &str, _blob_hash: &str, _content_type: &str) {}
        fn on_file_deleted(&self, file_id: &str) {
            self.deleted.lock().unwrap().push(file_id.to_string());
        }
    }

    async fn test_pool() -> Arc<sqlx::PgPool> {
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&test_db_url())
            .await
            .expect("connect to test DB — run tests/common/spawn-db.sh first");
        ensure_clean_test_db(&pool).await;
        Arc::new(pool)
    }

    /// Returns `(user_id, drive_id, drive_root_folder_id)` — same default
    /// Personal drive every internal user gets post-D0 (provisioned by
    /// `PersonalDriveLifecycleHook`).
    async fn seed_user(pool: &sqlx::PgPool) -> (Uuid, Uuid, Uuid) {
        sqlx::query(
            "SELECT u.id AS user_id, d.id AS drive_id, d.root_folder_id
               FROM auth.users u
               JOIN storage.drives d ON d.default_for_user = u.id
              LIMIT 1",
        )
        .fetch_one(pool)
        .await
        .map(|r| {
            (
                r.get::<Uuid, _>("user_id"),
                r.get::<Uuid, _>("drive_id"),
                r.get::<Uuid, _>("root_folder_id"),
            )
        })
        .expect("auth.users + storage.drives must be seeded (init-test-schema.sh)")
    }

    /// Build a real `PgAclEngine` against the test pool so
    /// `delete_folder_with_perms` can actually evaluate Owner — the user
    /// from `seed_user` owns the default drive, so `Permission::Delete`
    /// on its descendants resolves through the Owner short-circuit.
    async fn build_authz(
        pool: Arc<sqlx::PgPool>,
        dir: &TempDir,
        folder_repo: Arc<FolderDbRepository>,
    ) -> Arc<PgAclEngine> {
        let backend = Arc::new(LocalBlobBackend::new(&dir.path().join("blobs")));
        backend.initialize().await.expect("init backend");
        let dedup = Arc::new(DedupService::new(backend, pool.clone(), pool.clone()));
        let file_repo = Arc::new(FileBlobReadRepository::new(
            pool.clone(),
            dedup,
            folder_repo.clone(),
        ));
        let group_repo = Arc::new(SubjectGroupPgRepository::new(pool.clone()));
        Arc::new(PgAclEngine::new(pool, folder_repo, file_repo, group_repo))
    }

    /// Seed a file row under `folder_id`. `blob_hash` is just a string —
    /// `storage.files.blob_hash` is VARCHAR(64) without a FK, so no blob
    /// row is required. The cascade decrement trigger no-ops when the
    /// hash is unknown.
    async fn seed_file_under(
        pool: &sqlx::PgPool,
        user_id: Uuid,
        drive_id: Uuid,
        folder_id: Uuid,
        label: &str,
    ) -> Uuid {
        let blob_hash = blake3::hash(format!("cascade-{label}-{}", Uuid::new_v4()).as_bytes())
            .to_hex()
            .to_string();
        // Post-D7: `user_id` omitted — the column is nullable and
        // provenance flows through `created_by` / `updated_by`.
        sqlx::query_scalar(
            "INSERT INTO storage.files
                 (name, drive_id, folder_id, blob_hash, size, created_by, updated_by)
             VALUES ($1, $2, $3, $4, $5, $6, $6)
             RETURNING id",
        )
        .bind(format!(
            "rust-test-cascade-{label}-{}",
            &Uuid::new_v4().to_string()[..8]
        ))
        .bind(drive_id)
        .bind(folder_id)
        .bind(&blob_hash)
        .bind(42i64)
        .bind(user_id)
        .fetch_one(pool)
        .await
        .expect("seed file row")
    }

    #[tokio::test]
    async fn delete_folder_with_perms_fires_hook_for_cascaded_files() {
        let pool = test_pool().await;
        let dir = TempDir::new().unwrap();
        let (user_id, drive_id, drive_root) = seed_user(&pool).await;

        let folder_repo = Arc::new(FolderDbRepository::new(pool.clone()));
        let authz = build_authz(pool.clone(), &dir, folder_repo.clone()).await;
        let recorder: Arc<RecordingHook> = Arc::new(RecordingHook::default());
        let fls = Arc::new(
            crate::application::services::file_lifecycle_service::FileLifecycleService::new()
                .with_hook(recorder.clone() as Arc<dyn FileLifecycleHook>),
        );
        let service = FolderService::new(folder_repo.clone(), authz, fls);

        // Build parent/child via the production create path — it stamps
        // provenance and computes paths the same way as live uploads.
        let parent = folder_repo
            .create_folder(
                format!(
                    "rust-test-cascade-parent-{}",
                    &Uuid::new_v4().to_string()[..8]
                ),
                Some(drive_root.to_string()),
                user_id,
            )
            .await
            .expect("create parent");
        let child = folder_repo
            .create_folder(
                format!(
                    "rust-test-cascade-child-{}",
                    &Uuid::new_v4().to_string()[..8]
                ),
                Some(parent.id().to_string()),
                user_id,
            )
            .await
            .expect("create child");
        let child_uuid = Uuid::parse_str(child.id()).expect("child uuid");

        // Two files: one directly under the parent, one nested under
        // child. The cascade should reap both; the hook must fire for both.
        let parent_uuid = Uuid::parse_str(parent.id()).expect("parent uuid");
        let direct_file = seed_file_under(&pool, user_id, drive_id, parent_uuid, "direct").await;
        let nested_file = seed_file_under(&pool, user_id, drive_id, child_uuid, "nested").await;

        // Act — the production code path under test.
        service
            .delete_folder_with_perms(parent.id(), user_id)
            .await
            .expect("delete_folder_with_perms");

        // Assert — every cascaded file id appears in the hook record.
        let captured = recorder.deleted.lock().unwrap().clone();
        assert!(
            captured.contains(&direct_file.to_string()),
            "expected on_file_deleted for direct-child file {direct_file}, got {captured:?}"
        );
        assert!(
            captured.contains(&nested_file.to_string()),
            "expected on_file_deleted for nested file {nested_file}, got {captured:?}"
        );
    }
}
