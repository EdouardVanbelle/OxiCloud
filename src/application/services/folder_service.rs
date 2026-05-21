use crate::application::dtos::folder_dto::{
    CreateFolderDto, FolderDto, MoveFolderDto, RenameFolderDto,
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

            async fn get_folder_by_path(&self, _path: &str) -> Result<FolderDto, DomainError> {
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

            async fn create_home_folder(
                &self,
                _user_id: Uuid,
                _name: String,
            ) -> Result<FolderDto, DomainError> {
                Ok(FolderDto::empty())
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
            .create_folder(dto.name, dto.parent_id)
            .await?;
        Ok(FolderDto::from(folder))
    }

    /// Creates a root-level home folder for a user during registration.
    async fn create_home_folder(
        &self,
        user_id: Uuid,
        name: String,
    ) -> Result<FolderDto, DomainError> {
        let folder = self
            .folder_storage
            .create_home_folder(user_id, name)
            .await
            .map_err(|e| {
                DomainError::internal_error(
                    "FolderStorage",
                    format!("Failed to create home folder: {}", e),
                )
            })?;

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

    /// Gets a folder by its path
    async fn get_folder_by_path(&self, path: &str) -> Result<FolderDto, DomainError> {
        // Convert the string path to StoragePath
        let storage_path = StoragePath::from_string(path);

        let folder = self
            .folder_storage
            .get_folder_by_path(&storage_path)
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
    /// Self-healing: if listing root folders and none exist, creates a home folder.
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
        } else {
            // No parent defined grab user's homes
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

            if folders.is_empty() {
                // Self-healing: if listing root folders and none exist, create a home folder
                // This ensures the frontend always gets a valid userHomeFolderId
                tracing::info!(
                    "No root folders found for user {}, creating home folder automatically",
                    caller_id
                );
                let owner_id_short = {
                    let s = caller_id.to_string();
                    s[..8.min(s.len())].to_string()
                };
                // TODO: what about i18n ?
                let folder_name = format!("My Folder - {}", owner_id_short);
                match self
                    .folder_storage
                    .create_home_folder(caller_id, folder_name.clone())
                    .await
                {
                    Ok(home_folder) => {
                        tracing::info!(
                            "Created home folder '{}' for user {}",
                            folder_name,
                            caller_id
                        );
                        return Ok(vec![FolderDto::from(home_folder)]);
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to create home folder for user {}: {}",
                            caller_id,
                            e
                        );
                        // Return empty list rather than failing - user might not have storage quota, etc.
                    }
                }
            }
            Ok(folders.into_iter().map(FolderDto::from).collect())
        }
    }
    // TODO: move self healing in other part (on account creation on or login ?)

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

        self.authz
            .require(
                Subject::User(caller_id),
                Permission::Update,
                Self::folder_resource(id)?,
            )
            .await?;

        let folder = self
            .folder_storage
            .rename_folder(id, dto.name)
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
            .move_folder(id, parent_ref)
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
