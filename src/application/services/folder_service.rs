use crate::application::dtos::cursor::PageCursor;
use crate::application::dtos::folder_dto::{
    CreateFolderDto, FolderDto, FolderResourceCursor, FolderResourceRow, ListResourcesOptions,
    MoveFolderDto, RenameFolderDto,
};
use crate::application::ports::authorization_ports::AuthorizationEngine;
use crate::application::ports::external_mount_ports::MountEntry;
use crate::application::ports::folder_ports::FolderUseCase;
use crate::application::services::external_mount_router::MountRouter;
use crate::application::services::mount_registry::MountConfig;
use crate::common::errors::{DomainError, ErrorKind};
use crate::domain::repositories::folder_repository::FolderRepository;
use crate::domain::services::authorization::{Permission, Resource, ResourceKind, Subject};
use crate::domain::services::external_mount_id::NodeId;
use crate::domain::services::path_service::{StoragePath, validate_storage_name};
use crate::infrastructure::repositories::pg::folder_db_repository::FolderDbRepository;
use crate::infrastructure::services::pg_acl_engine::PgAclEngine;
use std::sync::Arc;
use uuid::Uuid;

/// Implementation of the use case for folder operations
pub struct FolderService {
    folder_storage: Arc<FolderDbRepository>,
    authz: Arc<PgAclEngine>,
    /// External-mount classifier. Lets folder operations branch a mount-root or
    /// `ext:` id onto the provider instead of the PostgreSQL repositories.
    mount_router: Arc<MountRouter>,
}

impl FolderService {
    /// Creates a new folder service
    pub fn new(
        folder_storage: Arc<FolderDbRepository>,
        authz: Arc<PgAclEngine>,
        mount_router: Arc<MountRouter>,
    ) -> Self {
        Self {
            folder_storage,
            authz,
            mount_router,
        }
    }

    /// Borrow the external-mount classifier (handlers branch on this before
    /// treating an id as a native UUID).
    pub fn mount_router(&self) -> &MountRouter {
        &self.mount_router
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

    /// List one directory inside an external mount (the mount root when
    /// `node_id` is empty, or a nested virtual folder otherwise).
    ///
    /// Authorization collapses onto the mount-root folder: a caller who may
    /// `Read` the mount root may browse everything inside it. The provider
    /// reads the live backend; entries are sorted in memory and paginated with
    /// a name-keyset cursor (directories are bounded, see provider cap).
    ///
    /// Returns the page of raw [`MountEntry`]s plus an encoded next cursor; the
    /// handler maps each entry to a `FolderResourceItemDto` with a synthetic
    /// `ext:` id.
    pub async fn list_mount_dir_with_perms(
        &self,
        cfg: &MountConfig,
        node_id: &NodeId,
        caller_id: Uuid,
        opts: ListResourcesOptions<'_>,
    ) -> Result<(Vec<MountEntry>, Option<String>), DomainError> {
        // AuthZ — everything in the mount is gated by the mount-root folder.
        self.authz
            .require(
                Subject::User(caller_id),
                Permission::Read,
                Resource::Folder(cfg.mount_id),
            )
            .await?;

        let entries = cfg.provider.list_dir(node_id).await?;
        let cursor_name = opts.cursor.as_ref().and_then(|c| c.sort_str.as_deref());
        Ok(paginate_mount_entries(
            entries,
            opts.kinds,
            opts.order_by,
            opts.reverse,
            opts.limit,
            cursor_name,
        ))
    }
}

/// Filter, sort, and page a directory's worth of mount entries, returning the
/// page plus an encoded next cursor. Pure (no I/O / authz) so it can be tested
/// exhaustively.
///
/// The cursor is a **name keyset**: names are unique within a directory, so the
/// last emitted name is a stable resume key under any sort dimension. Resume is
/// best-effort — if the cursor's entry was deleted out-of-band the page restarts
/// from the top (documented; avoids an infinite loop).
fn paginate_mount_entries(
    mut entries: Vec<MountEntry>,
    kinds: Option<&[ResourceKind]>,
    order_by: &str,
    reverse: bool,
    limit: usize,
    cursor_name: Option<&str>,
) -> (Vec<MountEntry>, Option<String>) {
    if let Some(kinds) = kinds {
        let want_files = kinds.contains(&ResourceKind::File);
        let want_folders = kinds.contains(&ResourceKind::Folder);
        entries.retain(|e| if e.is_dir { want_folders } else { want_files });
    }

    sort_mount_entries(&mut entries, order_by, reverse);

    let start = match cursor_name {
        Some(name) => entries
            .iter()
            .position(|e| name.eq_ignore_ascii_case(&e.name))
            .map(|i| i + 1)
            .unwrap_or(0),
        None => 0,
    };

    let has_more = entries.len() > start + limit;
    let page: Vec<MountEntry> = entries.into_iter().skip(start).take(limit).collect();

    let next_cursor = if has_more {
        page.last().map(|last| {
            FolderResourceCursor {
                order_by: order_by.to_owned(),
                resource_id: Uuid::nil(),
                sort_str: Some(last.name.clone()),
                sort_int: None,
                sort_ts: None,
                reverse,
            }
            .encode()
        })
    } else {
        None
    };

    (page, next_cursor)
}

/// Sort mount entries in place. Folders sort before files for the `name`/`type`
/// dimensions; otherwise by the requested key with name as the tie-breaker.
/// `reverse` flips the final order.
fn sort_mount_entries(entries: &mut [MountEntry], order_by: &str, reverse: bool) {
    use std::cmp::Ordering;
    let name_key = |e: &MountEntry| e.name.to_lowercase();
    entries.sort_by(|a, b| {
        let primary = match order_by {
            "modified_at" => a.modified_at.cmp(&b.modified_at),
            "created_at" => a.created_at.cmp(&b.created_at),
            "size" => a.size.cmp(&b.size),
            // "name" / "type" / anything else: folders first, then by name.
            _ => b.is_dir.cmp(&a.is_dir),
        };
        let ord = primary.then_with(|| name_key(a).cmp(&name_key(b)));
        if ord == Ordering::Equal {
            Ordering::Equal
        } else if reverse {
            ord.reverse()
        } else {
            ord
        }
    });
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

#[cfg(test)]
mod mount_listing_tests {
    use super::{paginate_mount_entries, sort_mount_entries};
    use crate::application::dtos::cursor::PageCursor;
    use crate::application::dtos::folder_dto::FolderResourceCursor;
    use crate::application::ports::external_mount_ports::MountEntry;
    use crate::domain::services::authorization::ResourceKind;
    use crate::domain::services::external_mount_id::NodeId;

    fn entry(name: &str, is_dir: bool, size: u64, modified: u64) -> MountEntry {
        MountEntry {
            name: name.to_string(),
            node_id: NodeId(name.to_string()),
            is_dir,
            size,
            modified_at: modified,
            created_at: modified,
        }
    }

    fn names(entries: &[MountEntry]) -> Vec<String> {
        entries.iter().map(|e| e.name.clone()).collect()
    }

    #[test]
    fn sorts_folders_first_then_name_case_insensitive() {
        let mut e = vec![
            entry("Banana.txt", false, 1, 1),
            entry("apple", true, 0, 1),
            entry("Cherry", true, 0, 1),
            entry("almond.txt", false, 1, 1),
        ];
        sort_mount_entries(&mut e, "name", false);
        assert_eq!(names(&e), ["apple", "Cherry", "almond.txt", "Banana.txt"]);
    }

    #[test]
    fn reverse_flips_order() {
        let mut e = vec![
            entry("a", false, 1, 1),
            entry("b", false, 1, 1),
            entry("d", true, 0, 1),
        ];
        sort_mount_entries(&mut e, "name", true);
        // folders-first then name, reversed.
        assert_eq!(names(&e), ["b", "a", "d"]);
    }

    #[test]
    fn sorts_by_size_modified_created() {
        let mut by_size = vec![
            entry("big", false, 100, 1),
            entry("small", false, 1, 1),
            entry("mid", false, 50, 1),
        ];
        sort_mount_entries(&mut by_size, "size", false);
        assert_eq!(names(&by_size), ["small", "mid", "big"]);

        let mut by_mtime = vec![
            entry("new", false, 1, 300),
            entry("old", false, 1, 100),
            entry("mid", false, 1, 200),
        ];
        sort_mount_entries(&mut by_mtime, "modified_at", false);
        assert_eq!(names(&by_mtime), ["old", "mid", "new"]);

        let mut by_ctime = vec![entry("z", false, 1, 9), entry("a", false, 1, 5)];
        sort_mount_entries(&mut by_ctime, "created_at", false);
        assert_eq!(names(&by_ctime), ["a", "z"]);
    }

    #[test]
    fn filters_by_kind() {
        let make = || vec![entry("dir", true, 0, 1), entry("file.txt", false, 1, 1)];
        let (files_only, _) =
            paginate_mount_entries(make(), Some(&[ResourceKind::File]), "name", false, 50, None);
        assert_eq!(names(&files_only), ["file.txt"]);

        let (folders_only, _) = paginate_mount_entries(
            make(),
            Some(&[ResourceKind::Folder]),
            "name",
            false,
            50,
            None,
        );
        assert_eq!(names(&folders_only), ["dir"]);

        let (both, _) = paginate_mount_entries(
            make(),
            Some(&[ResourceKind::File, ResourceKind::Folder]),
            "name",
            false,
            50,
            None,
        );
        assert_eq!(both.len(), 2);
    }

    #[test]
    fn paginates_with_name_keyset_cursor() {
        let all = || {
            vec![
                entry("a", false, 1, 1),
                entry("b", false, 1, 1),
                entry("c", false, 1, 1),
                entry("d", false, 1, 1),
                entry("e", false, 1, 1),
            ]
        };

        // Page 1: limit 2 → [a, b], cursor present.
        let (p1, c1) = paginate_mount_entries(all(), None, "name", false, 2, None);
        assert_eq!(names(&p1), ["a", "b"]);
        let c1 = c1.expect("cursor after first page");
        let decoded = FolderResourceCursor::decode(&c1).expect("decodes");
        assert_eq!(decoded.sort_str.as_deref(), Some("b"));
        assert!(!decoded.reverse);

        // Page 2: resume after "b" → [c, d], cursor present.
        let (p2, c2) = paginate_mount_entries(all(), None, "name", false, 2, Some("b"));
        assert_eq!(names(&p2), ["c", "d"]);
        assert!(c2.is_some());

        // Page 3: resume after "d" → [e], no further cursor.
        let (p3, c3) = paginate_mount_entries(all(), None, "name", false, 2, Some("d"));
        assert_eq!(names(&p3), ["e"]);
        assert!(c3.is_none());
    }

    #[test]
    fn no_cursor_when_page_is_last() {
        let e = vec![entry("a", false, 1, 1), entry("b", false, 1, 1)];
        let (page, cursor) = paginate_mount_entries(e, None, "name", false, 50, None);
        assert_eq!(page.len(), 2);
        assert!(cursor.is_none());
    }

    #[test]
    fn deleted_cursor_entry_restarts_best_effort() {
        // Cursor names "zzz" which is not present → start from the top.
        let e = vec![entry("a", false, 1, 1), entry("b", false, 1, 1)];
        let (page, _) = paginate_mount_entries(e, None, "name", false, 50, Some("zzz"));
        assert_eq!(names(&page), ["a", "b"]);
    }

    #[test]
    fn empty_directory_yields_empty_page() {
        let (page, cursor) = paginate_mount_entries(vec![], None, "name", false, 50, None);
        assert!(page.is_empty());
        assert!(cursor.is_none());
    }

    #[test]
    fn limit_larger_than_len_returns_all_without_cursor() {
        let e = vec![entry("a", false, 1, 1), entry("b", false, 1, 1)];
        let (page, cursor) = paginate_mount_entries(e, None, "name", false, 100, None);
        assert_eq!(page.len(), 2);
        assert!(cursor.is_none());
    }

    #[test]
    fn kind_filter_excluding_all_yields_empty() {
        let e = vec![entry("only_dir", true, 0, 1)];
        let (page, cursor) =
            paginate_mount_entries(e, Some(&[ResourceKind::File]), "name", false, 50, None);
        assert!(page.is_empty());
        assert!(cursor.is_none());
    }

    #[test]
    fn cursor_preserves_reverse_flag() {
        let e = vec![
            entry("a", false, 1, 1),
            entry("b", false, 1, 1),
            entry("c", false, 1, 1),
        ];
        let (_p, c) = paginate_mount_entries(e, None, "name", true, 1, None);
        let decoded = FolderResourceCursor::decode(&c.unwrap()).unwrap();
        assert!(decoded.reverse);
        assert_eq!(decoded.order_by, "name");
    }
}

#[cfg(all(test, integration_tests))]
mod mount_authz_integration {
    use super::*;
    use crate::application::dtos::folder_dto::ListResourcesOptions;
    use crate::application::services::external_mount_router::{MountRouter, ResolvedId};
    use crate::application::services::file_retrieval_service::FileRetrievalService;
    use crate::application::services::mount_registry::MountRegistry;
    use crate::domain::services::external_mount_id::{NodeId, encode_child_id};
    use crate::infrastructure::repositories::pg::{
        ExternalMountPgRepository, FileBlobReadRepository, SubjectGroupPgRepository,
    };
    use crate::infrastructure::services::mount_provider_factory::DefaultMountProviderFactory;
    use crate::mount_it_support::{fresh_db, insert_mount, make_user, provision_folder};
    use std::sync::Arc;

    fn opts<'a>() -> ListResourcesOptions<'a> {
        ListResourcesOptions {
            limit: 50,
            cursor: None,
            order_by: "name",
            kinds: None,
            reverse: false,
        }
    }

    /// Build a real PgAclEngine over the live pool. The folder-ancestry cascade
    /// uses the engine's own pool; the file repo is a stub (not exercised by
    /// folder checks).
    fn acl(pool: &Arc<sqlx::PgPool>) -> Arc<PgAclEngine> {
        Arc::new(PgAclEngine::new(
            pool.clone(),
            Arc::new(FolderDbRepository::new(pool.clone())),
            Arc::new(FileBlobReadRepository::new_stub()),
            Arc::new(SubjectGroupPgRepository::new(pool.clone())),
        ))
    }

    /// Full read path: owner can list a mount's live contents; a stranger with
    /// no grant is denied. Exercises the REAL authorization cascade
    /// (`authz.require(Resource::Folder(mount_id))`) over ltree ancestry.
    #[tokio::test]
    async fn owner_lists_mount_contents_stranger_denied() {
        let (_c, pool) = fresh_db().await;

        // Real host directory the mount points at.
        let host = tempfile::tempdir().unwrap();
        std::fs::write(host.path().join("a.txt"), b"hello").unwrap();
        std::fs::create_dir(host.path().join("sub")).unwrap();

        let p = provision_folder(&pool, "owner", "Media").await;
        insert_mount(&pool, &p, host.path().to_str().unwrap()).await;

        // Build the registry from the DB (also exercises reload + provider build).
        let registry = Arc::new(MountRegistry::empty());
        registry
            .reload(
                &ExternalMountPgRepository::new(pool.clone()),
                &DefaultMountProviderFactory::new(),
            )
            .await;
        let router = Arc::new(MountRouter::new(registry.clone()));
        let folder_service = FolderService::new(
            Arc::new(FolderDbRepository::new(pool.clone())),
            acl(&pool),
            router.clone(),
        );

        let cfg = registry.get(&p.mount_folder_id).expect("mount registered");

        // The mount root UUID classifies as a MountRoot.
        assert!(matches!(
            router.classify(&p.mount_folder_id.to_string()),
            ResolvedId::MountRoot { .. }
        ));

        // Owner lists the live directory contents.
        let (entries, _cursor) = folder_service
            .list_mount_dir_with_perms(&cfg, &NodeId::default(), p.owner_id, opts())
            .await
            .expect("owner may list");
        let mut names: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
        names.sort();
        assert_eq!(names, ["a.txt", "sub"]);

        // A stranger with no grant on the mount-root folder is denied
        // (NotFound — anti-enumeration).
        let stranger = make_user(&pool, "stranger").await;
        let err = folder_service
            .list_mount_dir_with_perms(&cfg, &NodeId::default(), stranger, opts())
            .await
            .expect_err("stranger must be denied");
        assert_eq!(err.kind, crate::domain::errors::ErrorKind::NotFound);
    }

    /// Download path authz: owner can stat/open a mount file; stranger denied.
    #[tokio::test]
    async fn owner_reads_mount_file_stranger_denied() {
        let (_c, pool) = fresh_db().await;

        let host = tempfile::tempdir().unwrap();
        std::fs::write(host.path().join("doc.txt"), b"payload").unwrap();

        let p = provision_folder(&pool, "owner", "Media").await;
        insert_mount(&pool, &p, host.path().to_str().unwrap()).await;

        let registry = Arc::new(MountRegistry::empty());
        registry
            .reload(
                &ExternalMountPgRepository::new(pool.clone()),
                &DefaultMountProviderFactory::new(),
            )
            .await;
        let cfg = registry.get(&p.mount_folder_id).expect("registered");

        let retrieval = FileRetrievalService::new_with_authz_for_test(
            Arc::new(FileBlobReadRepository::new_stub()),
            acl(&pool),
        );

        let node = NodeId::from("doc.txt");

        // Owner: stat succeeds with the real size.
        let stat = retrieval
            .stat_mount_file_with_perms(&cfg, &node, p.owner_id)
            .await
            .expect("owner may stat");
        assert_eq!(stat.size, 7);
        assert!(!stat.is_dir);

        // Owner: open succeeds (smoke — stream is consumed elsewhere).
        assert!(
            retrieval
                .open_mount_file_with_perms(&cfg, &node, p.owner_id, None)
                .await
                .is_ok()
        );

        // The synthetic id for this file round-trips through the router.
        let ext_id = encode_child_id(p.mount_folder_id, "doc.txt");
        assert!(matches!(
            MountRouter::new(registry.clone()).classify(&ext_id),
            ResolvedId::MountChild { .. }
        ));

        // Stranger: denied.
        let stranger = make_user(&pool, "stranger").await;
        let err = retrieval
            .stat_mount_file_with_perms(&cfg, &node, stranger)
            .await
            .expect_err("stranger denied");
        assert_eq!(err.kind, crate::domain::errors::ErrorKind::NotFound);
    }
}
