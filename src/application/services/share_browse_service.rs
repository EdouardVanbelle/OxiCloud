//! Share-scoped folder browsing for public folder shares.

use std::sync::Arc;

use uuid::Uuid;

use crate::application::dtos::folder_listing_dto::FolderListingDto;
use crate::application::ports::file_ports::FileRetrievalUseCase;
use crate::application::ports::inbound::FolderUseCase;
use crate::application::services::file_retrieval_service::FileRetrievalService;
use crate::application::services::folder_service::FolderService;
use crate::application::services::share_service::ShareService;
use crate::common::errors::DomainError;
use crate::domain::repositories::folder_repository::FolderRepository;
use crate::infrastructure::repositories::pg::folder_db_repository::FolderDbRepository;

struct ResolvedFolderShare {
    root_folder_id: String,
    owner_id: Uuid,
    display_name: String,
}

pub struct ZipTarget {
    pub folder_id: String,
    pub display_name: String,
}

pub struct ShareBrowseService {
    share_service: Arc<ShareService>,
    folder_service: Arc<FolderService>,
    file_retrieval: Arc<FileRetrievalService>,
    folder_repo: Arc<FolderDbRepository>,
}

impl ShareBrowseService {
    pub fn new(
        share_service: Arc<ShareService>,
        folder_service: Arc<FolderService>,
        file_retrieval: Arc<FileRetrievalService>,
        folder_repo: Arc<FolderDbRepository>,
    ) -> Self {
        Self {
            share_service,
            folder_service,
            file_retrieval,
            folder_repo,
        }
    }

    async fn resolve_folder_share(
        &self,
        token: &str,
        unlock_jwt: Option<&str>,
    ) -> Result<ResolvedFolderShare, DomainError> {
        let share = self
            .share_service
            .get_shared_link_with_unlock(token, unlock_jwt)
            .await?;

        if share.item_type != "folder" {
            return Err(DomainError::validation_error(
                "This endpoint is only valid for folder shares",
            ));
        }

        let owner_id = Uuid::parse_str(&share.created_by).map_err(|_| {
            DomainError::internal_error(
                "Share",
                format!("Share has invalid created_by UUID: {}", share.created_by),
            )
        })?;

        let display_name = match share.item_name {
            Some(name) => name,
            None => self
                .folder_service
                .get_folder(&share.item_id)
                .await
                .map(|f| f.name)
                .unwrap_or_else(|_| "Shared folder".to_string()),
        };

        Ok(ResolvedFolderShare {
            root_folder_id: share.item_id,
            owner_id,
            display_name,
        })
    }

    pub async fn list_root(
        &self,
        token: &str,
        unlock_jwt: Option<&str>,
    ) -> Result<FolderListingDto, DomainError> {
        let resolved = self.resolve_folder_share(token, unlock_jwt).await?;
        self.list_inner(&resolved.root_folder_id, resolved.owner_id)
            .await
    }

    pub async fn list_subfolder(
        &self,
        token: &str,
        folder_id: &str,
        unlock_jwt: Option<&str>,
    ) -> Result<FolderListingDto, DomainError> {
        let resolved = self.resolve_folder_share(token, unlock_jwt).await?;

        if !self
            .folder_repo
            .is_folder_in_subtree(folder_id, &resolved.root_folder_id)
            .await?
        {
            return Err(DomainError::not_found("Folder", folder_id));
        }

        self.list_inner(folder_id, resolved.owner_id).await
    }

    pub async fn assert_file_in_share(
        &self,
        token: &str,
        file_id: &str,
        unlock_jwt: Option<&str>,
    ) -> Result<(), DomainError> {
        let resolved = self.resolve_folder_share(token, unlock_jwt).await?;

        if !self
            .folder_repo
            .is_file_in_subtree(file_id, &resolved.root_folder_id)
            .await?
        {
            return Err(DomainError::not_found("File", file_id));
        }
        Ok(())
    }

    pub async fn resolve_zip_target(
        &self,
        token: &str,
        folder_id: Option<&str>,
        unlock_jwt: Option<&str>,
    ) -> Result<ZipTarget, DomainError> {
        let resolved = self.resolve_folder_share(token, unlock_jwt).await?;

        let target_folder_id = match folder_id {
            None => resolved.root_folder_id.clone(),
            Some(id) => {
                if !self
                    .folder_repo
                    .is_folder_in_subtree(id, &resolved.root_folder_id)
                    .await?
                {
                    return Err(DomainError::not_found("Folder", id));
                }
                id.to_string()
            }
        };

        let display_name = if folder_id.is_none() {
            resolved.display_name
        } else {
            self.folder_service
                .get_folder(&target_folder_id)
                .await
                .map(|f| f.name)
                .unwrap_or_else(|_| "shared".to_string())
        };

        Ok(ZipTarget {
            folder_id: target_folder_id,
            display_name,
        })
    }

    async fn list_inner(
        &self,
        parent_folder_id: &str,
        owner_id: Uuid,
    ) -> Result<FolderListingDto, DomainError> {
        let (folders_res, files_res) = tokio::join!(
            self.folder_service
                .list_folders_for_owner(Some(parent_folder_id), owner_id),
            self.file_retrieval
                .list_files_owned(Some(parent_folder_id), owner_id),
        );
        Ok(FolderListingDto {
            folders: folders_res?,
            files: files_res?,
        })
    }
}
