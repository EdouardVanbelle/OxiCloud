use std::sync::Arc;

use crate::application::dtos::file_dto::FileDto;
use crate::application::ports::authorization_ports::AuthorizationEngine;
use crate::application::ports::file_lifecycle::FileLifecycleHook;
use crate::application::ports::file_ports::FileManagementUseCase;
use crate::application::ports::resource_access_hook::ResourceAccessHook;
use crate::application::ports::storage_ports::{CopyFolderTreeResult, FileWritePort};
use crate::application::ports::trash_ports::TrashUseCase;
use crate::application::services::trash_service::TrashService;
use crate::common::errors::DomainError;
use crate::domain::services::authorization::{Permission, Resource, Subject};
use crate::domain::services::path_service::validate_storage_name;
use crate::infrastructure::repositories::pg::file_blob_read_repository::FileBlobReadRepository;
use crate::infrastructure::repositories::pg::file_blob_write_repository::FileBlobWriteRepository;
use crate::infrastructure::repositories::pg::folder_db_repository::FolderDbRepository;
use crate::infrastructure::services::file_content_cache::FileContentCache;
use crate::infrastructure::services::pg_acl_engine::PgAclEngine;
use tracing::{error, info, warn};
use uuid::Uuid;

/// Service for file management operations (move, delete).
///
/// Blob ref_count bookkeeping on deletion is handled by the PG trigger
/// `trg_files_decrement_blob_ref` (fires on DELETE FROM storage.files).
/// This service only orchestrates trash vs. permanent delete — it never
/// touches ref_count directly.
pub struct FileManagementService {
    file_repository: Arc<FileBlobWriteRepository>,
    trash_service: Option<Arc<TrashService>>,
    content_cache: Option<Arc<FileContentCache>>,
    authz: Arc<PgAclEngine>,
    /// Lifecycle hook dispatcher — fired on file created (copy) and deleted.
    file_lifecycle_hook: Option<Arc<dyn FileLifecycleHook>>,
    /// Read/write access hook — fired so Recent reflects "this is the file
    /// I just copied / renamed / moved", same way the read paths surface
    /// downloads. Distinct from the lifecycle hook because lifecycle hooks
    /// don't carry the `caller_id` the recording side needs.
    resource_access_hook: Option<Arc<dyn ResourceAccessHook>>,
    /// Drive repository — used by D5's `forbid_cross_drive_move` gate
    /// on `move_file_with_perms`. Optional so stubs / test factories
    /// can build the service without wiring the full drive repo; in
    /// that case the cross-drive move check is skipped (the policy
    /// is silently off). Production DI wires it in.
    drive_repo: Option<Arc<dyn crate::domain::repositories::drive_repository::DriveRepository>>,
    /// Storage-usage service — used to pre-check the destination
    /// drive's `used_bytes + delta ≤ quota_bytes` invariant on
    /// cross-drive MOVE, matching the pre-write check the upload path
    /// already performs. Without it, the check is silently skipped
    /// (stub/test builders); production DI wires it in.
    storage_usage:
        Option<Arc<crate::application::services::storage_usage_service::StorageUsageService>>,
}

impl FileManagementService {
    /// Creates a FileManagementService with a trash service, content cache
    /// and the ReBAC authorization engine. File/folder owner lookups (used
    /// for owner short-circuit inside the engine) are now the engine's
    /// responsibility — this service no longer holds direct repo references
    /// for ownership.
    pub fn with_trash(
        file_repository: Arc<FileBlobWriteRepository>,
        trash_service: Option<Arc<TrashService>>,
        _file_read: Option<Arc<FileBlobReadRepository>>,
        _folder_repo: Option<Arc<FolderDbRepository>>,
        content_cache: Option<Arc<FileContentCache>>,
        authz: Arc<PgAclEngine>,
    ) -> Self {
        Self {
            file_repository,
            trash_service,
            content_cache,
            authz,
            file_lifecycle_hook: None,
            resource_access_hook: None,
            drive_repo: None,
            storage_usage: None,
        }
    }

    /// Sets the lifecycle hook dispatcher (thumbnails, audio metadata, …).
    pub fn with_file_lifecycle_hook(mut self, hook: Arc<dyn FileLifecycleHook>) -> Self {
        self.file_lifecycle_hook = Some(hook);
        self
    }

    /// Registers the read/write access hook (Recent list recorder).
    pub fn with_resource_access_hook(mut self, hook: Arc<dyn ResourceAccessHook>) -> Self {
        self.resource_access_hook = Some(hook);
        self
    }

    /// Internal helper: fire the access hook if registered.
    fn notify_file_accessed(&self, caller_id: Uuid, file_id: &str) {
        if let Some(hook) = &self.resource_access_hook {
            hook.on_file_accessed(caller_id, file_id);
        }
    }

    /// Wires the drive repository, enabling D5 `forbid_cross_drive_move`
    /// enforcement on `move_file_with_perms`. Without it, the gate is
    /// silently skipped.
    pub fn with_drive_repo(
        mut self,
        drive_repo: Arc<dyn crate::domain::repositories::drive_repository::DriveRepository>,
    ) -> Self {
        self.drive_repo = Some(drive_repo);
        self
    }

    /// Wires the storage-usage service so `move_file_with_perms` can
    /// pre-check the destination drive's quota on cross-drive moves.
    pub fn with_storage_usage(
        mut self,
        storage_usage: Arc<
            crate::application::services::storage_usage_service::StorageUsageService,
        >,
    ) -> Self {
        self.storage_usage = Some(storage_usage);
        self
    }

    /// Engine check for a file resource. Parses the id into a `Uuid` and
    /// requires the specified permission.
    async fn require_file_perm(
        &self,
        file_id: &str,
        perm: Permission,
        caller_id: Uuid,
    ) -> Result<(), DomainError> {
        let uuid = Uuid::parse_str(file_id).map_err(|_| DomainError::not_found("File", file_id))?;
        self.authz
            .require(Subject::User(caller_id), perm, Resource::File(uuid))
            .await
    }

    /// Engine check for a target folder. `None` is allowed (root namespace,
    /// implicitly owned by the caller).
    async fn require_target_folder_perm(
        &self,
        folder_id: Option<&str>,
        perm: Permission,
        caller_id: Uuid,
    ) -> Result<(), DomainError> {
        let Some(target) = folder_id else {
            return Ok(());
        };
        let uuid = Uuid::parse_str(target).map_err(|_| DomainError::not_found("Folder", target))?;
        self.authz
            .require(Subject::User(caller_id), perm, Resource::Folder(uuid))
            .await
    }

    //impl FileManagementPrivateUseCase for FileManagementService {
    async fn move_file(
        &self,
        file_id: &str,
        folder_id: Option<String>,
        caller_id: Uuid,
    ) -> Result<FileDto, DomainError> {
        info!(
            "Moving file with ID: {} to folder: {:?}",
            file_id, folder_id
        );

        let moved_file = self
            .file_repository
            .move_file(file_id, folder_id, caller_id)
            .await
            .map_err(|e| {
                error!("Error moving file (ID: {}): {}", file_id, e);
                e
            })?;

        info!(
            "File moved successfully: {} (ID: {}) to folder: {:?}",
            moved_file.name(),
            moved_file.id(),
            moved_file.folder_id()
        );

        Ok(FileDto::from(moved_file))
    }

    async fn copy_file(
        &self,
        file_id: &str,
        target_folder_id: Option<String>,
        new_name: Option<&str>,
        caller_id: Uuid,
    ) -> Result<FileDto, DomainError> {
        info!(
            "Copying file with ID: {} to folder: {:?} as {:?}",
            file_id, target_folder_id, new_name
        );

        let copied_file = self
            .file_repository
            .copy_file(file_id, target_folder_id, new_name, caller_id)
            .await
            .map_err(|e| {
                error!("Error copying file (ID: {}): {}", file_id, e);
                e
            })?;

        info!(
            "File copied successfully: {} (ID: {}) to folder: {:?}",
            copied_file.name(),
            copied_file.id(),
            copied_file.folder_id()
        );

        let dto = FileDto::from(copied_file);
        if let Some(hook) = &self.file_lifecycle_hook {
            hook.on_file_copied(&dto.id, &dto.content_hash, &dto.mime_type, file_id);
        }
        // The caller just spawned a fresh file — show it in their Recent
        // list. The source file isn't recorded; only the visible target.
        self.notify_file_accessed(caller_id, &dto.id);
        Ok(dto)
    }

    async fn rename_file(
        &self,
        file_id: &str,
        new_name: &str,
        caller_id: Uuid,
    ) -> Result<FileDto, DomainError> {
        if let Err(reason) = validate_storage_name(new_name) {
            return Err(DomainError::validation_error(format!(
                "Invalid file name '{new_name}': {reason}"
            )));
        }

        info!("Renaming file with ID: {} to \"{}\"", file_id, new_name);

        let renamed_file = self
            .file_repository
            .rename_file(file_id, new_name, caller_id)
            .await
            .map_err(|e| {
                error!("Error renaming file (ID: {}): {}", file_id, e);
                e
            })?;

        info!(
            "File renamed successfully: {} (ID: {})",
            renamed_file.name(),
            renamed_file.id()
        );

        Ok(FileDto::from(renamed_file))
    }

    async fn delete_file(&self, id: &str) -> Result<(), DomainError> {
        warn!("Permanently deleting file: {}", id);
        self.file_repository.delete_file(id).await?;
        if let Some(cc) = &self.content_cache {
            cc.invalidate(id).await;
        }
        if let Some(hook) = &self.file_lifecycle_hook {
            hook.on_file_deleted(id);
        }
        info!("File permanently deleted: {}", id);
        Ok(())
    }

    async fn copy_folder_tree(
        &self,
        source_folder_id: &str,
        target_parent_id: Option<String>,
        dest_name: Option<String>,
    ) -> Result<CopyFolderTreeResult, DomainError> {
        info!(
            "Copying folder tree: source={}, target_parent={:?}, dest_name={:?}",
            source_folder_id, target_parent_id, dest_name
        );

        let result = self
            .file_repository
            .copy_folder_tree(source_folder_id, target_parent_id, dest_name)
            .await
            .map_err(|e| {
                error!(
                    "Error copying folder tree (source: {}): {}",
                    source_folder_id, e
                );
                e
            })?;

        info!(
            "Folder tree copied: {} folders, {} files (new root: {})",
            result.folders_copied, result.files_copied, result.new_root_folder_id
        );

        Ok(result)
    }
}

impl FileManagementUseCase for FileManagementService {
    async fn require_permission(
        &self,
        caller_id: Uuid,
        permission: Permission,
        file_id: &str,
    ) -> Result<(), DomainError> {
        let uuid = Uuid::parse_str(file_id).map_err(|_| DomainError::not_found("File", file_id))?;
        self.authz
            .require(Subject::User(caller_id), permission, Resource::File(uuid))
            .await
    }

    async fn move_file_with_perms(
        &self,
        file_id: &str,
        caller_id: Uuid,
        folder_id: Option<String>,
    ) -> Result<FileDto, DomainError> {
        // Move = Update on the file + Create on the target folder (if any).
        self.require_file_perm(file_id, Permission::Update, caller_id)
            .await?;
        self.require_target_folder_perm(folder_id.as_deref(), Permission::Create, caller_id)
            .await?;

        // D5 `forbid_cross_drive_move` + D6 `resource.moved_between_drives` audit
        // share the same src/dst drive_id lookup: the gate refuses
        // before the move; the audit fires after a successful move
        // when the two drives differ. Silently skipped if the drive
        // repo isn't wired (stub builders) or the move target is None
        // (root namespace — same-drive semantics).
        let mut cross_drive: Option<(Uuid, Uuid)> = None;
        if let Some(drive_repo) = &self.drive_repo
            && let Some(target_folder_id) = folder_id.as_deref()
        {
            let file_uuid =
                Uuid::parse_str(file_id).map_err(|_| DomainError::not_found("File", file_id))?;
            let dst_folder_uuid = Uuid::parse_str(target_folder_id)
                .map_err(|_| DomainError::not_found("Folder", target_folder_id))?;
            let (src_drive_id, src_policies) = drive_repo
                .get_drive_id_and_policies_for_file(file_uuid)
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
                        resource_type: "file",
                        resource_id: file_uuid,
                        src_drive_id,
                        dst_drive_id,
                    },
                )?;
                // Destination drive quota: same pre-write check the
                // upload path already runs (`file_upload_service.rs`
                // `check_storage_quota`), applied here so a caller
                // can't sneak content past the drive cap via MOVE.
                // Denial → `DomainError::QuotaExceeded` → 507
                // Insufficient Storage. Skipped when `storage_usage`
                // isn't wired (stub builders) — same shape as the
                // upload path's skip semantics.
                if let Some(storage_usage) = &self.storage_usage
                    && let Some(size_bytes) = storage_usage.file_bytes(file_uuid).await?
                    && let Ok(size_u64) = u64::try_from(size_bytes)
                {
                    storage_usage
                        .check_drive_quota(dst_drive_id, size_u64)
                        .await?;
                }
                cross_drive = Some((src_drive_id, dst_drive_id));
            }
        }

        let dto = self.move_file(file_id, folder_id, caller_id).await?;

        // Cross-drive move invalidates the file's `owner_cache` entry
        // in the authz engine — the cache assumed drive_id stability
        // that no longer holds. Without this call the drive-role
        // precheck at `check_inner` steers to the (stale) source
        // drive and legitimate Delete/Update by a destination-drive
        // role-holder returns 404 for up to the cache TTL.
        if cross_drive.is_some()
            && let Ok(file_uuid) = Uuid::parse_str(file_id)
        {
            self.authz
                .invalidate_owner_cache_for_resource(Resource::File(file_uuid))
                .await;
        }

        // D6 §11 audit: emit only when the move actually crossed a
        // drive boundary. Same-drive moves are too noisy to audit at
        // info — operators care about the cross-drive case for
        // exfiltration / quota tracking.
        if let Some((src_drive_id, dst_drive_id)) = cross_drive {
            tracing::info!(
                target: "audit",
                event = "resource.moved_between_drives",
                resource_type = "file",
                resource_id = %dto.id,
                src_drive_id = %src_drive_id,
                dst_drive_id = %dst_drive_id,
                by = %caller_id,
                "📦 file moved between drives",
            );
        }
        Ok(dto)
    }

    async fn copy_file_with_perms(
        &self,
        file_id: &str,
        caller_id: Uuid,
        target_folder_id: Option<String>,
        new_name: Option<String>,
    ) -> Result<FileDto, DomainError> {
        // Copy = Read on the source file + Create on the target folder.
        self.require_file_perm(file_id, Permission::Read, caller_id)
            .await?;
        self.require_target_folder_perm(target_folder_id.as_deref(), Permission::Create, caller_id)
            .await?;

        // Destination drive quota: COPY creates a new file row that
        // counts against the destination drive's `used_bytes` even
        // though blob dedup means no new bytes hit the store. Same
        // pre-flight shape the delta-upload path already uses.
        // Skipped when `storage_usage` isn't wired (stub builders) or
        // `target_folder_id` is None (root namespace — same-drive
        // semantics inherit the source's cap coverage). Denial →
        // `QuotaExceeded` → 507.
        if let (Some(storage_usage), Some(target_folder)) =
            (&self.storage_usage, target_folder_id.as_deref())
        {
            let file_uuid =
                Uuid::parse_str(file_id).map_err(|_| DomainError::not_found("File", file_id))?;
            let target_folder_uuid = Uuid::parse_str(target_folder)
                .map_err(|_| DomainError::not_found("Folder", target_folder))?;
            if let Some(size_bytes) = storage_usage.file_bytes(file_uuid).await?
                && let Ok(size_u64) = u64::try_from(size_bytes)
            {
                storage_usage
                    .check_drive_quota_by_folder(target_folder_uuid, size_u64)
                    .await?;
            }
        }

        self.copy_file(file_id, target_folder_id, new_name.as_deref(), caller_id)
            .await
    }

    async fn rename_file_with_perms(
        &self,
        file_id: &str,
        caller_id: Uuid,
        new_name: &str,
    ) -> Result<FileDto, DomainError> {
        self.require_file_perm(file_id, Permission::Update, caller_id)
            .await?;
        self.rename_file(file_id, new_name, caller_id).await
    }

    async fn delete_file_with_perms(&self, id: &str, caller_id: Uuid) -> Result<(), DomainError> {
        self.require_file_perm(id, Permission::Delete, caller_id)
            .await?;
        self.delete_file(id).await
    }

    /// Smart delete: trash-first with dedup reference cleanup.
    ///
    /// Blob ref_count bookkeeping is handled entirely by the PG trigger
    /// `trg_files_decrement_blob_ref` which fires on DELETE FROM storage.files.
    /// We do NOT decrement here — trashing is a soft-delete (UPDATE, not DELETE)
    /// so the blob must remain referenced until the file is permanently deleted.
    async fn delete_and_cleanup_with_perms(
        &self,
        id: &str,
        caller_id: Uuid,
    ) -> Result<bool, DomainError> {
        self.require_file_perm(id, Permission::Delete, caller_id)
            .await?;
        // Step 1: Try trash (soft delete — file row stays, blob stays referenced)
        if let Some(trash) = &self.trash_service {
            info!("Moving file to trash: {}", id);
            match trash.move_to_trash(id, "file", caller_id).await {
                Ok(_) => {
                    info!("File successfully moved to trash: {}", id);
                    // Invalidate content cache — trashed files must not be served.
                    if let Some(cc) = &self.content_cache {
                        cc.invalidate(id).await;
                    }
                    // Do NOT decrement blob ref here — the file row still exists
                    // (is_trashed = TRUE). The trigger will decrement when the
                    // row is actually DELETEd during trash emptying.
                    return Ok(true); // trashed
                }
                Err(err) => {
                    error!("Could not move file to trash: {:?}", err);
                    warn!("Falling back to permanent delete");
                    // fall through
                }
            }
        } else {
            warn!("Trash service not available, using permanent delete");
        }

        // Step 2: Permanent delete — trigger handles blob ref_count

        self.delete_file(id).await?;

        Ok(false) // permanently deleted
    }

    async fn copy_folder_tree_with_perms(
        &self,
        source_folder_id: &str,
        caller_id: Uuid,
        target_parent_id: Option<String>,
        dest_name: Option<String>,
    ) -> Result<CopyFolderTreeResult, DomainError> {
        // copy_folder_tree = Read on the source folder + Create on the target parent.
        self.require_target_folder_perm(Some(source_folder_id), Permission::Read, caller_id)
            .await?;
        self.require_target_folder_perm(target_parent_id.as_deref(), Permission::Create, caller_id)
            .await?;

        // Destination drive quota: sum the subtree's non-trashed files
        // and refuse if the destination couldn't hold them. Skipped
        // when `storage_usage` isn't wired or the target is root
        // (same rationale as `copy_file_with_perms`).
        if let (Some(storage_usage), Some(target_parent)) =
            (&self.storage_usage, target_parent_id.as_deref())
        {
            let source_uuid = Uuid::parse_str(source_folder_id)
                .map_err(|_| DomainError::not_found("Folder", source_folder_id))?;
            let target_parent_uuid = Uuid::parse_str(target_parent)
                .map_err(|_| DomainError::not_found("Folder", target_parent))?;
            let subtree_bytes = storage_usage.folder_subtree_bytes(source_uuid).await?;
            if let Ok(subtree_u64) = u64::try_from(subtree_bytes) {
                storage_usage
                    .check_drive_quota_by_folder(target_parent_uuid, subtree_u64)
                    .await?;
            }
        }

        self.copy_folder_tree(source_folder_id, target_parent_id, dest_name)
            .await
    }
}
