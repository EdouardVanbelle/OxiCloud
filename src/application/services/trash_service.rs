use std::sync::Arc;
use tracing::{debug, error, info, instrument, warn};
use uuid::Uuid;

use crate::application::dtos::cursor::PageCursor;
use crate::application::dtos::display_helpers::{
    category_for, format_file_size, icon_class_for, icon_special_class_for,
};
use crate::application::dtos::file_dto::FileDto;
use crate::application::dtos::folder_dto::FolderDto;
use crate::application::dtos::grant_dto::{ResourceContentDto, ResourceTypeDto};
use crate::application::dtos::trash_dto::{
    TrashCursor, TrashResourceItemDto, TrashResourceRow, TrashedItemDto,
};
use crate::application::ports::authorization_ports::AuthorizationEngine;
use crate::application::ports::file_lifecycle::FileLifecycleHook;
use crate::application::ports::storage_ports::{FileReadPort, FileWritePort};
use crate::application::ports::trash_ports::TrashUseCase;
use crate::common::errors::{DomainError, ErrorKind, Result};
use crate::domain::entities::file::File;
use crate::domain::entities::trashed_item::{TrashedItem, TrashedItemType};
use crate::domain::repositories::drive_repository::DriveRepository;
use crate::domain::repositories::folder_repository::FolderRepository;
use crate::domain::repositories::trash_repository::TrashRepository;
use crate::domain::services::authorization::ResourceKind;
use crate::domain::services::authorization::{Permission, Resource, Subject};
use crate::infrastructure::repositories::pg::file_blob_read_repository::FileBlobReadRepository;
use crate::infrastructure::repositories::pg::file_blob_write_repository::FileBlobWriteRepository;
use crate::infrastructure::repositories::pg::folder_db_repository::FolderDbRepository;
use crate::infrastructure::repositories::pg::trash_db_repository::TrashDbRepository;
use crate::infrastructure::services::dedup_service::DedupService;
use crate::infrastructure::services::file_content_cache::FileContentCache;
use crate::infrastructure::services::pg_acl_engine::PgAclEngine;

/**
 * Application service for trash operations.
 *
 * The TrashService implements the trash management functionality in the application layer,
 * handling movement of files and folders to trash, restoration from trash, and permanent
 * deletion. It orchestrates interactions between the domain entities and infrastructure
 * repositories while enforcing business rules like retention policies.
 *
 * This service follows the Clean Architecture pattern by:
 * - Depending on application ports rather than domain/infrastructure traits
 * - Orchestrating domain operations without containing domain logic
 * - Exposing its functionality through the TrashUseCase port
 */
pub struct TrashService {
    /// Repository for trash-specific operations like listing and retrieving trashed items
    trash_repository: Arc<TrashDbRepository>,

    /// Port for file read operations (get file metadata)
    file_read_port: Arc<FileBlobReadRepository>,

    /// Port for file write operations (trash, restore, delete)
    file_write_port: Arc<FileBlobWriteRepository>,

    /// Port for folder operations (get folder, trash, restore, delete)
    folder_storage_port: Arc<FolderDbRepository>,

    /// Dedup service — garbage-collected after bulk trash empty to clean up
    /// orphaned blob files and thumbnails that the PG trigger cannot reach.
    dedup_service: Arc<DedupService>,

    /// Lifecycle hook dispatcher — fired on file permanently deleted.
    file_deleted_hook: Option<Arc<dyn FileLifecycleHook>>,

    /// Content cache — invalidated when files are permanently deleted from trash.
    content_cache: Option<Arc<FileContentCache>>,

    /// Authz engine
    authz: Arc<PgAclEngine>,

    /// Drive repository — D2b uses it to resolve "drives the caller can read"
    /// so trash listings filter by drive membership instead of the legacy
    /// per-user scope.
    drive_repo: Arc<crate::infrastructure::repositories::pg::DrivePgRepository>,

    /// Number of days items should be kept in trash before automatic cleanup
    retention_days: u32,
}

impl TrashService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        trash_repository: Arc<TrashDbRepository>,
        file_read_port: Arc<FileBlobReadRepository>,
        file_write_port: Arc<FileBlobWriteRepository>,
        folder_storage_port: Arc<FolderDbRepository>,
        retention_days: u32,
        dedup_service: Arc<DedupService>,
        content_cache: Option<Arc<FileContentCache>>,
        authz: Arc<PgAclEngine>,
        drive_repo: Arc<crate::infrastructure::repositories::pg::DrivePgRepository>,
    ) -> Self {
        Self {
            trash_repository,
            file_read_port,
            file_write_port,
            folder_storage_port,
            dedup_service,
            file_deleted_hook: None,
            content_cache,
            authz,
            drive_repo,
            retention_days,
        }
    }

    /// Sets the lifecycle hook dispatcher (thumbnails, audio metadata, …).
    pub fn with_file_deleted_hook(mut self, hook: Arc<dyn FileLifecycleHook>) -> Self {
        self.file_deleted_hook = Some(hook);
        self
    }

    /// Converts a TrashedItem entity to a DTO
    fn to_dto(&self, item: TrashedItem) -> TrashedItemDto {
        // Calculate days_until_deletion before moving item fields
        let days_until_deletion = item.days_until_deletion();

        // Determine display fields based on item type
        let (category, icon_class, icon_special_class) = match item.item_type() {
            TrashedItemType::Folder => (
                "Folder".to_string(),
                "fas fa-folder".to_string(),
                "folder-icon".to_string(),
            ),
            TrashedItemType::File => {
                let name = item.name();
                // Use empty MIME type to leverage extension fallback
                let category = category_for(name, "").to_string();
                let icon_class = icon_class_for(name, "").to_string();
                let icon_special_class = icon_special_class_for(name, "").to_string();
                (category, icon_class, icon_special_class)
            }
        };

        TrashedItemDto {
            id: item.id().to_string(),
            original_id: item.original_id().to_string(),
            item_type: match item.item_type() {
                TrashedItemType::File => "file".to_string(),
                TrashedItemType::Folder => "folder".to_string(),
            },
            name: item.name().to_string(),
            original_path: item.original_path().to_string(),
            trashed_at: item.trashed_at(),
            days_until_deletion,
            category,
            icon_class,
            icon_special_class,
        }
    }
}

impl TrashUseCase for TrashService {
    #[instrument(skip(self))]
    async fn get_trash_items(&self, user_id: Uuid) -> Result<Vec<TrashedItemDto>> {
        debug!("Getting trash items for user: {}", user_id);

        let items = self.trash_repository.get_trash_items(&user_id).await?;

        let dtos = items.into_iter().map(|item| self.to_dto(item)).collect();

        Ok(dtos)
    }

    // TODO: change item_type into Resource enum
    #[instrument(skip(self))]
    async fn move_to_trash(&self, item_id: &str, item_type: &str, user_id: Uuid) -> Result<()> {
        info!(
            "Moving to trash: type={}, id={}, user={}",
            item_type, item_id, user_id
        );
        debug!("User UUID validation: {}", user_id);

        // Note: We now verify file/folder ownership BEFORE moving to trash.
        // This prevents users from trashing items they do not own (IDOR).

        // Parse UUIDs with detailed error handling
        debug!("Validating item UUID: {}", item_id);
        let item_uuid = match Uuid::parse_str(item_id) {
            Ok(uuid) => {
                debug!("Valid item UUID: {}", uuid);
                uuid
            }
            Err(e) => {
                error!("Invalid item UUID: {} - Error: {}", item_id, e);
                return Err(DomainError::validation_error(format!(
                    "Invalid item ID: {}",
                    e
                )));
            }
        };

        let user_uuid = user_id;

        match item_type {
            "file" => {
                info!("Processing file to move to trash: {}", item_id);

                let file_id = Uuid::parse_str(item_id)
                    .map_err(|_| DomainError::not_found("File", item_id))?;
                self.authz
                    .require(
                        Subject::User(user_id),
                        Permission::Delete,
                        Resource::File(file_id),
                    )
                    .await?;

                // Authz already passed — use the non-owner-scoped read so that
                // grantees with Delete permission can trash files they don't own.
                // The file's user_id in storage.files is unchanged, so the item
                // will appear in the original owner's trash view.
                let file = match self.file_read_port.get_file(item_id).await {
                    Ok(file) => {
                        debug!("File found: {} ({})", file.name(), item_id);
                        file
                    }
                    Err(e) => {
                        error!("Error getting file: {} - {}", item_id, e);
                        return Err(DomainError::new(
                            ErrorKind::NotFound,
                            "File",
                            format!("Error retrieving file {}: {}", item_id, e),
                        ));
                    }
                };

                let original_path = file.storage_path().to_string();
                debug!("Original file path: {}", original_path);

                debug!("Creating TrashedItem object for the file");
                let trashed_item = TrashedItem::new(
                    item_uuid,
                    user_uuid,
                    TrashedItemType::File,
                    file.name().to_string(),
                    original_path,
                    self.retention_days,
                );
                debug!(
                    "TrashedItem created successfully: {} -> {}",
                    file.name(),
                    trashed_item.id()
                );

                // First add to trash index to register the item
                info!("Adding file {} to trash index", item_id);
                match self.trash_repository.add_to_trash(&trashed_item).await {
                    Ok(_) => {
                        debug!("File added to trash index successfully");
                    }
                    Err(e) => {
                        error!("Error adding file to trash index: {}", e);
                        return Err(DomainError::internal_error(
                            "TrashRepository",
                            format!("Failed to add file to trash: {}", e),
                        ));
                    }
                };

                // Then physically move the file to trash.
                // §14: caller_id stamps `updated_by` on the trashed row.
                info!("Physically moving file to trash: {}", item_id);
                match self.file_write_port.move_to_trash(item_id, user_id).await {
                    Ok(_) => {
                        debug!("File physically moved to trash successfully: {}", item_id);
                    }
                    Err(e) => {
                        error!("Error physically moving file to trash: {} - {}", item_id, e);
                        return Err(DomainError::new(
                            ErrorKind::InternalError,
                            "File",
                            format!("Error moving file {} to trash: {}", item_id, e),
                        ));
                    }
                }

                info!("File completely moved to trash: {}", item_id);
                Ok(())
            }
            "folder" => {
                // check deletion permition
                let folder_id = Uuid::parse_str(item_id)
                    .map_err(|_| DomainError::not_found("Folder", item_id))?;
                self.authz
                    .require(
                        Subject::User(user_id),
                        Permission::Delete,
                        Resource::Folder(folder_id),
                    )
                    .await?;

                let folder = self
                    .folder_storage_port
                    .get_folder(item_id)
                    .await
                    .map_err(|e| {
                        DomainError::new(
                            ErrorKind::NotFound,
                            "Folder",
                            format!("Error retrieving folder {}: {}", item_id, e),
                        )
                    })?;

                let original_path = folder.storage_path().to_string();

                let trashed_item = TrashedItem::new(
                    item_uuid,
                    user_uuid,
                    TrashedItemType::Folder,
                    folder.name().to_string(),
                    original_path,
                    self.retention_days,
                );

                // First add to trash index to register the item
                debug!("Adding folder {} to trash repository", item_id);
                match self.trash_repository.add_to_trash(&trashed_item).await {
                    Ok(_) => debug!("Successfully added folder to trash repository"),
                    Err(e) => {
                        error!("Failed to add folder to trash repository: {}", e);
                        return Err(DomainError::internal_error(
                            "TrashRepository",
                            format!("Failed to add folder to trash: {}", e),
                        ));
                    }
                };

                // Then physically move the folder to trash.
                // §14: caller_id stamps `updated_by` on every cascade-trashed row.
                self.folder_storage_port
                    .move_to_trash(item_id, user_id)
                    .await
                    .map_err(|e| {
                        DomainError::new(
                            ErrorKind::InternalError,
                            "Folder",
                            format!("Error moving folder {} to trash: {}", item_id, e),
                        )
                    })?;

                debug!("Folder moved to trash: {}", item_id);
                Ok(())
            }
            _ => Err(DomainError::validation_error(format!(
                "Invalid item type: {}",
                item_type
            ))),
        }
    }

    #[instrument(skip(self))]
    async fn restore_item(&self, trash_id: &str, user_id: Uuid) -> Result<()> {
        info!("Restoring item {} for user {}", trash_id, user_id);

        let trash_uuid = match Uuid::parse_str(trash_id) {
            Ok(id) => {
                info!("Trash UUID parsed successfully: {}", id);
                id
            }
            Err(e) => {
                error!("Invalid trash ID format: {} - {}", trash_id, e);
                return Err(DomainError::validation_error(format!(
                    "Invalid trash ID: {}",
                    e
                )));
            }
        };

        let user_uuid = user_id;

        // Get the trash item
        info!("Retrieving trash item from repository: ID={}", trash_id);
        let item_result = self.trash_repository.get_trash_item(&trash_uuid).await;

        match item_result {
            Ok(Some(item)) => {
                info!(
                    "Found item in trash: ID={}, Type={:?}, OriginalID={}",
                    trash_id,
                    item.item_type(),
                    item.original_id()
                );

                // D2b stage 3: gate the restore on Delete permission against
                // the item's original resource. The drive precheck in
                // `pg_acl_engine` resolves Owner-on-drive → Delete-permission,
                // so a drive Owner can restore items they didn't originally
                // trash (per `drive.md §12`). 404-on-deny via `authz.require`
                // matches the lookup-NotFound shape.
                let original = item.original_id();
                let resource = match item.item_type() {
                    TrashedItemType::File => Resource::File(original),
                    TrashedItemType::Folder => Resource::Folder(original),
                };
                self.authz
                    .require(Subject::User(user_id), Permission::Delete, resource)
                    .await?;

                // Restore based on type
                match item.item_type() {
                    TrashedItemType::File => {
                        // Restore the file to its original location
                        let file_id = item.original_id().to_string();
                        let original_path = item.original_path().to_string();

                        info!(
                            "Restoring file from trash: ID={}, OriginalPath={}",
                            file_id, original_path
                        );
                        match self
                            .file_write_port
                            .restore_from_trash(&file_id, &original_path, user_id)
                            .await
                        {
                            Ok(_) => {
                                info!("Successfully restored file from trash: {}", file_id);
                            }
                            Err(e) => {
                                // Check if the error is because the file is not found
                                if format!("{}", e).contains("not found") {
                                    info!(
                                        "File not found in trash, may already have been restored: {}",
                                        file_id
                                    );
                                    // We continue so we can clean up the trash entry
                                } else {
                                    // Return error for other kinds of errors
                                    error!("Error restoring file from trash: {} - {}", file_id, e);
                                    return Err(DomainError::new(
                                        ErrorKind::InternalError,
                                        "File",
                                        format!(
                                            "Error restoring file {} from trash: {}",
                                            file_id, e
                                        ),
                                    ));
                                }
                            }
                        }
                    }
                    TrashedItemType::Folder => {
                        // Restore the folder to its original location
                        let folder_id = item.original_id().to_string();
                        let original_path = item.original_path().to_string();

                        info!(
                            "Restoring folder from trash: ID={}, OriginalPath={}",
                            folder_id, original_path
                        );
                        match self
                            .folder_storage_port
                            .restore_from_trash(&folder_id, &original_path, user_id)
                            .await
                        {
                            Ok(_) => {
                                info!("Successfully restored folder from trash: {}", folder_id);
                            }
                            Err(e) => {
                                // Check if the error is because the folder is not found
                                if format!("{}", e).contains("not found") {
                                    info!(
                                        "Folder not found in trash, may already have been restored: {}",
                                        folder_id
                                    );
                                    // We continue so we can clean up the trash entry
                                } else {
                                    // Return error for other kinds of errors
                                    error!(
                                        "Error restoring folder from trash: {} - {}",
                                        folder_id, e
                                    );
                                    return Err(DomainError::new(
                                        ErrorKind::InternalError,
                                        "Folder",
                                        format!(
                                            "Error restoring folder {} from trash: {}",
                                            folder_id, e
                                        ),
                                    ));
                                }
                            }
                        }
                    }
                }

                // Always remove the item from the trash index to maintain consistency
                info!(
                    "Removing item from trash index after restoration: {}",
                    trash_id
                );
                match self
                    .trash_repository
                    .restore_from_trash(&trash_uuid, &user_uuid)
                    .await
                {
                    Ok(_) => {
                        info!("Successfully removed entry from trash index: {}", trash_id);
                    }
                    Err(e) => {
                        error!(
                            "Error removing entry from trash index: {} - {}",
                            trash_id, e
                        );
                        return Err(DomainError::new(
                            ErrorKind::InternalError,
                            "Trash",
                            format!("Error removing trash entry after restoration: {}", e),
                        ));
                    }
                }

                info!("Item successfully restored from trash: {}", trash_id);
                Ok(())
            }
            Ok(None) => {
                // If the item isn't found in trash, we can just return success
                info!(
                    "Item not found in trash index, considering as already restored: {}",
                    trash_id
                );
                Ok(())
            }
            Err(e) => {
                // Something went wrong with the repository
                error!(
                    "Error retrieving item from trash repository: {} - {}",
                    trash_id, e
                );
                Err(e)
            }
        }
    }

    #[instrument(skip(self))]
    async fn delete_permanently(&self, trash_id: &str, user_id: Uuid) -> Result<()> {
        info!(
            "Permanently deleting item {} for user {}",
            trash_id, user_id
        );

        let trash_uuid = match Uuid::parse_str(trash_id) {
            Ok(id) => {
                info!("Trash UUID parsed successfully: {}", id);
                id
            }
            Err(e) => {
                error!("Invalid trash ID format: {} - {}", trash_id, e);
                return Err(DomainError::validation_error(format!(
                    "Invalid trash ID: {}",
                    e
                )));
            }
        };

        let user_uuid = user_id;

        // Get the trash item
        info!("Retrieving trash item from repository: ID={}", trash_id);
        let item_result = self.trash_repository.get_trash_item(&trash_uuid).await;

        match item_result {
            Ok(Some(item)) => {
                info!(
                    "Found item in trash: ID={}, Type={:?}, OriginalID={}",
                    trash_id,
                    item.item_type(),
                    item.original_id()
                );

                // D2b stage 3: gate the permanent-delete on Delete permission
                // against the item's original resource (mirrors `restore_item`
                // above). Drive Owners can hard-delete shared-drive items
                // they didn't trash.
                let original = item.original_id();
                let resource = match item.item_type() {
                    TrashedItemType::File => Resource::File(original),
                    TrashedItemType::Folder => Resource::Folder(original),
                };
                self.authz
                    .require(Subject::User(user_id), Permission::Delete, resource)
                    .await?;

                // Permanently delete based on type
                match item.item_type() {
                    TrashedItemType::File => {
                        // Permanently delete the file
                        let file_id = item.original_id().to_string();

                        info!("Permanently deleting file: {}", file_id);
                        match self.file_write_port.delete_file_permanently(&file_id).await {
                            Ok(_) => {
                                info!("Successfully deleted file permanently: {}", file_id);
                                // Invalidate content cache for the deleted file.
                                if let Some(cc) = &self.content_cache {
                                    cc.invalidate(&file_id).await;
                                }
                            }
                            Err(e) => {
                                // File already gone — still remove the trash index
                                // entry. Match on the typed error kind, not the
                                // message text, so a reworded message can't
                                // silently turn this into a hard failure.
                                if e.kind == ErrorKind::NotFound {
                                    info!(
                                        "File not found, may already have been deleted: {}",
                                        file_id
                                    );
                                } else {
                                    // Return error for other types of errors
                                    error!("Error permanently deleting file: {} - {}", file_id, e);
                                    return Err(DomainError::new(
                                        ErrorKind::InternalError,
                                        "File",
                                        format!(
                                            "Error deleting file {} permanently: {}",
                                            file_id, e
                                        ),
                                    ));
                                }
                            }
                        }

                        if let Some(hook) = &self.file_deleted_hook {
                            hook.on_file_deleted(&file_id);
                        }
                    }
                    TrashedItemType::Folder => {
                        // Permanently delete the folder
                        let folder_id = item.original_id().to_string();

                        info!("Permanently deleting folder: {}", folder_id);
                        match self
                            .folder_storage_port
                            .delete_folder_permanently(&folder_id)
                            .await
                        {
                            Ok(_) => {
                                info!("Successfully deleted folder permanently: {}", folder_id);
                            }
                            Err(e) => {
                                // Folder already gone — still remove the trash
                                // index entry. Typed-kind match (see file branch).
                                if e.kind == ErrorKind::NotFound {
                                    info!(
                                        "Folder not found, may already have been deleted: {}",
                                        folder_id
                                    );
                                } else {
                                    // Return error for other types of errors
                                    error!(
                                        "Error permanently deleting folder: {} - {}",
                                        folder_id, e
                                    );
                                    return Err(DomainError::new(
                                        ErrorKind::InternalError,
                                        "Folder",
                                        format!(
                                            "Error deleting folder {} permanently: {}",
                                            folder_id, e
                                        ),
                                    ));
                                }
                            }
                        }
                    }
                }

                // Always remove the item from trash index to maintain consistency
                info!("Removing entry from trash index: {}", trash_id);
                match self
                    .trash_repository
                    .delete_permanently(&trash_uuid, &user_uuid)
                    .await
                {
                    Ok(_) => {
                        info!("Successfully removed entry from trash index: {}", trash_id);
                    }
                    Err(e) => {
                        error!(
                            "Error removing entry from trash index: {} - {}",
                            trash_id, e
                        );
                        return Err(DomainError::new(
                            ErrorKind::InternalError,
                            "Trash",
                            format!("Error removing trash entry: {}", e),
                        ));
                    }
                };

                info!("Item permanently deleted from trash: {}", trash_id);
                Ok(())
            }
            Ok(None) => {
                // If the item isn't found in trash, we can just return success
                info!(
                    "Item not found in trash, considering as already deleted: {}",
                    trash_id
                );
                Ok(())
            }
            Err(e) => {
                // Something went wrong with the repository
                error!(
                    "Error retrieving item from trash repository: {} - {}",
                    trash_id, e
                );
                Err(e)
            }
        }
    }

    #[instrument(skip(self))]
    async fn empty_trash(&self, user_id: Uuid) -> Result<()> {
        info!("Emptying trash for user {}", user_id);

        // D2b stage 3: resolve the set of drives the caller can permanently
        // delete content in. "Empty trash" means "drop every trashed item
        // I have Delete on" — Owner role's bundle includes Delete; Editor /
        // Viewer / Contributor / Commenter do not. So this filter picks out
        // the drives where the caller is effectively Owner (direct or via a
        // group). Single-drive users: this resolves to just their personal
        // drive, identical to the legacy `WHERE user_id = $1` scope.
        let (subject_types, subject_ids) = self
            .authz
            .expand_subject_for_listing(Subject::User(user_id))
            .await?;
        let drives = self
            .drive_repo
            .list_for_subjects(&subject_types, &subject_ids)
            .await
            .map_err(|e| {
                DomainError::internal_error(
                    "Trash",
                    format!("Failed to resolve accessible drives: {e:?}"),
                )
            })?;
        let drive_ids: Vec<Uuid> = drives
            .iter()
            .filter(|d| {
                d.caller_role
                    .is_some_and(|r| r.expand().contains(&Permission::Delete))
            })
            .map(|d| d.drive.id)
            .collect();

        if drive_ids.is_empty() {
            info!("empty_trash: caller has Delete on no drive — nothing to do");
            return Ok(());
        }

        // Collect ALL trashed file IDs BEFORE bulk-deleting so hooks (thumbnail
        // cleanup, etc.) can run afterward.  We use get_all_trashed_file_ids (not
        // get_trash_items) because the trash_items view excludes files inside a
        // trashed folder — those files will still be deleted by clear_trash via
        // the folder CASCADE, but their hooks would otherwise be missed.
        let trashed_file_ids: Vec<String> = if self.file_deleted_hook.is_some() {
            match self
                .trash_repository
                .get_all_trashed_file_ids(&drive_ids)
                .await
            {
                Ok(ids) => ids,
                Err(e) => {
                    warn!("Could not list trashed files for hook cleanup: {}", e);
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };

        // clear_trash() performs bulk SQL DELETEs in 2 queries (post D2b stage 3):
        //   1. DELETE FROM storage.files  WHERE drive_id = ANY($1) AND is_trashed = TRUE
        //   2. DELETE FROM storage.folders WHERE drive_id = ANY($1) AND is_trashed = TRUE
        //
        // Folder deletion cascades (FK ON DELETE CASCADE) to child folders and
        // their files. The PG trigger `trg_files_decrement_blob_ref` automatically
        // decrements blob ref_counts for every deleted file row.
        self.trash_repository.clear_trash(&drive_ids).await?;

        // The PG trigger decremented ref_counts but cannot delete disk files or
        // thumbnails.  Run garbage_collect() to remove any blobs whose ref_count
        // reached 0, along with their blob-keyed thumbnail files.
        if let Err(e) = self.dedup_service.garbage_collect().await {
            warn!("empty_trash: garbage_collect failed: {:?}", e);
        }

        // Invalidate content cache for all permanently deleted files.
        if let Some(cc) = &self.content_cache {
            for file_id in &trashed_file_ids {
                cc.invalidate(file_id).await;
            }
        }

        if let Some(hook) = &self.file_deleted_hook {
            for file_id in &trashed_file_ids {
                hook.on_file_deleted(file_id);
            }
        }

        info!("Trash emptied for user {}", user_id);
        Ok(())
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Cursor-paginated trash listing  (GET /api/trash/resources)
// ════════════════════════════════════════════════════════════════════════════
impl TrashService {
    /// Cursor-paginated list of the user's trashed resources.
    ///
    /// No `authz.require()` here — trashed items are strictly user-scoped and
    /// the repository enforces `WHERE user_id = $1`. This matches the pattern
    /// used by favorites and recent listing endpoints. Mutations (restore,
    /// delete permanently, move to trash) keep their per-item authz checks.
    ///
    /// Returns `(page items, next_cursor_encoded)`.
    pub async fn list_resources_paged(
        &self,
        user_id: Uuid,
        limit: usize,
        cursor: Option<TrashCursor>,
        order_by: &str,
        kinds: Option<&[ResourceKind]>,
        reverse: bool,
    ) -> Result<(Vec<TrashResourceItemDto>, Option<String>)> {
        // D2b: scope by drives the caller can read (resolved through
        // role_grants on resource_type='drive', including group-mediated
        // grants). Empty set → empty page without a SQL round-trip.
        let (subject_types, subject_ids) = self
            .authz
            .expand_subject_for_listing(Subject::User(user_id))
            .await?;
        let drive_ids: Vec<Uuid> = match self
            .drive_repo
            .list_for_subjects(&subject_types, &subject_ids)
            .await
        {
            Ok(drives) => drives.into_iter().map(|d| d.drive.id).collect(),
            Err(e) => {
                return Err(DomainError::internal_error(
                    "Trash",
                    format!("Failed to resolve accessible drives: {e:?}"),
                ));
            }
        };

        // Fetch one extra row to detect whether a next page exists.
        let mut rows = self
            .trash_repository
            .list_resources_paged(
                &drive_ids,
                limit + 1,
                cursor.as_ref(),
                order_by,
                kinds,
                reverse,
            )
            .await?;

        let next_cursor = if rows.len() > limit {
            let last = &rows[limit - 1];
            let c = build_trash_cursor(last, order_by, reverse);
            rows.truncate(limit);
            Some(c.encode())
        } else {
            None
        };

        let items: Vec<TrashResourceItemDto> = rows.into_iter().map(row_to_item_dto).collect();

        Ok((items, next_cursor))
    }
}

/// Build the next-page cursor from the last row of the current page.
/// `reverse` is stored in the cursor so subsequent pages use the same direction.
fn build_trash_cursor(row: &TrashResourceRow, order_by: &str, reverse: bool) -> TrashCursor {
    let order_by_owned = match order_by {
        "deletion_date" | "trashed_at" | "name" | "type" | "size" => order_by.to_owned(),
        _ => "deletion_date".to_owned(),
    };
    TrashCursor {
        order_by: order_by_owned,
        resource_id: row.resource_id,
        sort_str: row.sort_str.clone(),
        sort_int: row.sort_int,
        sort_ts: row.sort_ts,
        reverse,
    }
}

/// Convert a raw repository row into the API DTO.
fn row_to_item_dto(row: TrashResourceRow) -> TrashResourceItemDto {
    let path = row.path.clone().unwrap_or_default();
    if row.resource_type == "folder" {
        let resource_id = row.resource_id.to_string();
        let dto = FolderDto {
            etag: resource_id.clone(),
            id: resource_id,
            name: row.name.clone(),
            path,
            parent_id: row.parent_id.map(|u| u.to_string()),
            owner_id: Some(row.owner_id.to_string()),
            // D2b: the trash listing query now SELECTs `drive_id` (the
            // unified view exposes it). Surfaced so per-drive grouping
            // in the `/trash` UI doesn't need an extra lookup per row.
            drive_id: row.drive_id,
            created_at: row.resource_created_at.timestamp() as u64,
            modified_at: row.modified_at.timestamp() as u64,
            is_root: false,
            icon_class: std::sync::Arc::from("fas fa-folder"),
            icon_special_class: std::sync::Arc::from("folder-icon"),
            category: std::sync::Arc::from("Folder"),
            // §14 provenance not selected by the trash listing query.
            created_by: None,
            updated_by: None,
        };
        TrashResourceItemDto {
            resource_type: ResourceTypeDto::Folder,
            trashed_at: row.trashed_at,
            deletion_date: row.deletion_date,
            drive_id: row.drive_id,
            resource: ResourceContentDto::Folder(dto),
        }
    } else {
        let mime = row
            .mime_type
            .as_deref()
            .unwrap_or("application/octet-stream");
        let size_bytes = row.size.max(0) as u64;
        // Route ETag through `File::compute_etag` so trash items
        // match GET/HEAD/PROPFIND ETags — a client restoring a
        // file may conditional-request it immediately after.
        let modified_at_u = row.modified_at.timestamp() as u64;
        let content_hash = row.blob_hash.clone().unwrap_or_default();
        let etag = if content_hash.is_empty() {
            String::new()
        } else {
            File::compute_etag(&content_hash, modified_at_u)
        };
        let dto = FileDto {
            id: row.resource_id.to_string(),
            name: row.name.clone(),
            path,
            size: size_bytes,
            mime_type: std::sync::Arc::from(mime),
            folder_id: row.parent_id.map(|u| u.to_string()),
            created_at: row.resource_created_at.timestamp() as u64,
            modified_at: modified_at_u,
            icon_class: std::sync::Arc::from(icon_class_for(&row.name, mime)),
            icon_special_class: std::sync::Arc::from(icon_special_class_for(&row.name, mime)),
            category: std::sync::Arc::from(category_for(&row.name, mime)),
            size_formatted: format_file_size(size_bytes),
            owner_id: Some(row.owner_id.to_string()),
            sort_date: None,
            content_hash,
            etag,
            // §14 provenance not selected by the trash listing query.
            created_by: None,
            updated_by: None,
        };
        TrashResourceItemDto {
            resource_type: ResourceTypeDto::File,
            trashed_at: row.trashed_at,
            deletion_date: row.deletion_date,
            drive_id: row.drive_id,
            resource: ResourceContentDto::File(dto),
        }
    }
}
