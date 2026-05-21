/// Primary port for folder operations
use uuid::Uuid;

use crate::application::dtos::folder_dto::{
    CreateFolderDto, FolderDto, MoveFolderDto, RenameFolderDto,
};

use crate::common::errors::DomainError;
use crate::domain::services::authorization::Permission;

pub trait FolderUseCase: Send + Sync + 'static {
    async fn require_permission(
        &self,
        caller_id: Uuid,
        permission: Permission,
        folder_id: &str,
    ) -> Result<(), DomainError>;

    /// Creates a new folder
    async fn create_folder_with_perms(
        &self,
        dto: CreateFolderDto,
        caller_id: Uuid,
    ) -> Result<FolderDto, DomainError>;

    /// Gets a folder by its ID
    async fn get_folder(&self, id: &str) -> Result<FolderDto, DomainError>;

    /// Gets a folder by its ID, enforcing that `caller_id` is the owner.
    ///
    /// Returns `NotFound` if the folder does not exist **or** belongs to
    /// another user.  All user-facing handlers should use this method.
    async fn get_folder_with_perms(
        &self,
        id: &str,
        caller_id: Uuid,
    ) -> Result<FolderDto, DomainError>;

    /// Gets a folder by its path
    async fn get_folder_by_path(&self, path: &str) -> Result<FolderDto, DomainError>;

    /// Lists folders within a parent folder
    async fn list_folders(&self, parent_id: Option<&str>) -> Result<Vec<FolderDto>, DomainError>;

    /// Lists folders scoped to a specific owner (for user-facing endpoints).
    /// At root level, only returns folders belonging to this user.
    async fn list_folders_with_perms(
        &self,
        parent_id: Option<&str>,
        owner_id: Uuid,
    ) -> Result<Vec<FolderDto>, DomainError>;

    /// Lists folders with pagination
    async fn list_folders_paginated(
        &self,
        parent_id: Option<&str>,
        pagination: &crate::application::dtos::pagination::PaginationRequestDto,
    ) -> Result<crate::application::dtos::pagination::PaginatedResponseDto<FolderDto>, DomainError>;

    /// Lists folders with pagination, scoped to a specific owner.
    async fn list_folders_paginated_with_perms(
        &self,
        parent_id: Option<&str>,
        owner_id: Uuid,
        pagination: &crate::application::dtos::pagination::PaginationRequestDto,
    ) -> Result<crate::application::dtos::pagination::PaginatedResponseDto<FolderDto>, DomainError>;

    /// Renames a folder (ownership verified against caller_id)
    async fn rename_folder_with_perms(
        &self,
        id: &str,
        dto: RenameFolderDto,
        caller_id: Uuid,
    ) -> Result<FolderDto, DomainError>;

    /// Moves a folder to another parent (ownership verified against caller_id)
    async fn move_folder_with_perms(
        &self,
        id: &str,
        dto: MoveFolderDto,
        caller_id: Uuid,
    ) -> Result<FolderDto, DomainError>;

    /// Deletes a folder (ownership verified against caller_id)
    async fn delete_folder_with_perms(&self, id: &str, caller_id: Uuid) -> Result<(), DomainError>;

    /// Creates a root-level home folder for a user during registration.
    async fn create_home_folder(
        &self,
        user_id: Uuid,
        name: String,
    ) -> Result<FolderDto, DomainError>;

    /// Lists every folder in a subtree rooted at `folder_id` (inclusive),
    /// ordered by path.  Uses ltree `<@` — single GiST-indexed query.
    ///
    /// Default: returns an empty vec (stubs / mocks).
    async fn list_subtree_folders(&self, folder_id: &str) -> Result<Vec<FolderDto>, DomainError> {
        let _ = folder_id;
        Ok(Vec::new())
    }
}
