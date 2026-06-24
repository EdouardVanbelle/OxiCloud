use uuid::Uuid;

use crate::common::errors::Result;
use crate::domain::entities::trashed_item::TrashedItem;

pub trait TrashRepository: Send + Sync {
    async fn add_to_trash(&self, item: &TrashedItem) -> Result<()>;
    async fn get_trash_items(&self, user_id: &Uuid) -> Result<Vec<TrashedItem>>;
    /// Fetch a trashed item by its trash-row id.
    ///
    /// **Caller contract**: this method does NO authorization check. Callers
    /// MUST follow with
    /// `authz.require(Permission::Delete, Resource::File|Folder(item.original_id()))`
    /// before acting on the result. The trash service's `restore_item` and
    /// `delete_permanently` are the canonical examples.
    ///
    /// **Implementor contract**: do NOT re-introduce a `user_id` (or
    /// `drive_id`) scope filter at the SQL layer. Authorization is the
    /// service's job, not the repository's — adding a scope here would
    /// silently break drive-Owner-restores-another-user's-trashed-item
    /// (the canonical D2 use case). A direct-by-id lookup is the
    /// intended shape.
    async fn get_trash_item(&self, id: &Uuid) -> Result<Option<TrashedItem>>;
    async fn restore_from_trash(&self, id: &Uuid, user_id: &Uuid) -> Result<()>;
    async fn delete_permanently(&self, id: &Uuid, user_id: &Uuid) -> Result<()>;
    /// Bulk-delete all trashed files and folders in the given drives.
    ///
    /// **Caller contract**: pass only drive UUIDs the caller has
    /// `Permission::Delete` on (resolved by the service via
    /// `DriveRepository::list_for_subjects` + role-bundle filter). This
    /// repository performs no authorization — see
    /// `TrashService::empty_trash` for the canonical call site.
    async fn clear_trash(&self, drive_ids: &[Uuid]) -> Result<()>;

    /// All trashed file IDs across the given drives, regardless of parent
    /// folder trash status. Used by `empty_trash` for thumbnail cleanup —
    /// the trash_items view excludes files inside trashed folders, which
    /// would miss their thumbnails. Same caller contract as `clear_trash`.
    async fn get_all_trashed_file_ids(&self, drive_ids: &[Uuid]) -> Result<Vec<String>>;

    /// Bulk-delete all expired trash items (files + folders) in a single
    /// transaction.  Returns `(files_deleted, folders_deleted)`.
    async fn delete_expired_bulk(&self) -> Result<(u64, u64)>;
}
