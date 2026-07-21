use uuid::Uuid;

use crate::application::dtos::trash_dto::TrashedItemDto;
use crate::common::errors::Result;

/// Port for trash-related use cases
pub trait TrashUseCase: Send + Sync {
    /// List items in the user's trash
    async fn get_trash_items(&self, user_id: Uuid) -> Result<Vec<TrashedItemDto>>;

    /// Move a file or folder to trash
    async fn move_to_trash(&self, item_id: &str, item_type: &str, user_id: Uuid) -> Result<()>;

    /// Restore an item from trash to its original location
    async fn restore_item(&self, trash_id: &str, user_id: Uuid) -> Result<()>;

    /// Permanently delete an item from trash
    async fn delete_permanently(&self, trash_id: &str, user_id: Uuid) -> Result<()>;

    /// Empty the trash for a specific user
    async fn empty_trash(&self, user_id: Uuid) -> Result<()>;

    /// Empty the trash within a single drive the caller can Delete in.
    ///
    /// Same destructive shape as `empty_trash`, but scoped to one drive
    /// — the Drive group-by on `/trash` exposes a per-row "Empty"
    /// affordance so multi-drive owners can clear one drive without
    /// touching the others. Refused (`NotFound`) when the caller has no
    /// Delete-bearing role on the named drive (anti-enum: same shape
    /// as if the drive didn't exist), or when the drive id is unknown.
    async fn empty_trash_for_drive(&self, user_id: Uuid, drive_id: Uuid) -> Result<()>;
}
