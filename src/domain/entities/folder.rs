use uuid::Uuid;

use crate::domain::services::path_service::{
    StoragePath, normalize_storage_name, validate_storage_name,
};

// Re-export entity errors from the centralized module
pub use super::entity_errors::{FolderError, FolderResult};

/// Represents a folder entity in the domain
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    /// Unique identifier for the folder
    id: String,

    /// Name of the folder
    name: String,

    /// Path to the folder in the domain model
    storage_path: StoragePath,

    /// String representation of the path (for API compatibility)
    path_string: String,

    /// Parent folder ID (None if it's a root folder)
    parent_id: Option<String>,

    /// Drive that owns this folder. Post-D0 every `storage.folders` row
    /// has `drive_id NOT NULL` (M3 migration). Path-based lookups scope
    /// by this axis (not by `user_id`, which is dropped in D7).
    /// `Uuid::nil()` only for stub/legacy in-memory folders that never
    /// touched the DB.
    drive_id: Uuid,

    /// Creation timestamp
    created_at: u64,

    /// Last modification timestamp of THIS folder row (rename, move,
    /// metadata change). Does NOT bump when descendants change —
    /// that signal lives on `tree_modified_at`.
    modified_at: u64,

    /// Latest `modified_at`-equivalent across the entire descendant
    /// subtree. Bumped by a PostgreSQL trigger on any file or folder
    /// write under this folder's ltree subtree. Source of the
    /// HTTP ETag emitted in PROPFIND/GET/HEAD responses — see
    /// [`Folder::etag`] for the formula and rationale.
    tree_modified_at: u64,

    /// User that originally created this folder. Stamped at INSERT
    /// from the caller's id and never updated afterwards (provenance,
    /// not ownership — see §14 of the Drive plan). `None` when the
    /// referenced user is later deleted (FK is `ON DELETE SET NULL`)
    /// or for stub/DTO-reconstructed folders that never touched the DB.
    created_by: Option<Uuid>,

    /// User that performed the most recent mutation that touched
    /// `updated_at` (rename, move, trash, restore, content overwrite).
    /// Authorship signal — does NOT propagate via the tree-ETag flush
    /// trigger. `None` when the referenced user is deleted or for
    /// stub/DTO-reconstructed folders.
    updated_by: Option<Uuid>,
}

// We no longer need this module, now we use a String directly

impl Default for Folder {
    fn default() -> Self {
        Self {
            id: "stub-id".to_string(),
            name: "stub-folder".to_string(),
            storage_path: StoragePath::from_string("/"),
            path_string: "/".to_string(),
            parent_id: None,
            drive_id: Uuid::nil(),
            created_at: 0,
            modified_at: 0,
            tree_modified_at: 0,
            created_by: None,
            updated_by: None,
        }
    }
}

impl Folder {
    /// Creates a new folder with validation.
    ///
    /// In-memory constructor: callers that don't supply a `drive_id`
    /// are by definition stub/legacy paths (tests, pre-D0 fixtures,
    /// DTO round-trips). Real DB-backed folders flow through
    /// [`Folder::with_timestamps_and_tree`] which propagates the
    /// drive scope and §14 provenance from the row.
    pub fn new(
        id: String,
        name: String,
        storage_path: StoragePath,
        parent_id: Option<String>,
    ) -> FolderResult<Self> {
        let name = normalize_storage_name(&name);
        if let Err(reason) = validate_storage_name(&name) {
            return Err(FolderError::InvalidFolderName(format!("{name}: {reason}")));
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let path_string = storage_path.to_string();

        Ok(Self {
            id,
            name,
            storage_path,
            path_string,
            parent_id,
            drive_id: Uuid::nil(),
            created_at: now,
            modified_at: now,
            tree_modified_at: now,
            created_by: None,
            updated_by: None,
        })
    }

    /// Creates a folder with specific timestamps (for reconstruction).
    /// `tree_modified_at` defaults to `modified_at` — appropriate for
    /// in-memory construction; database loads should always go via
    /// [`Folder::with_timestamps_and_tree`] so the rollup value
    /// reflects DB reality.
    pub fn with_timestamps(
        id: String,
        name: String,
        storage_path: StoragePath,
        parent_id: Option<String>,
        created_at: u64,
        modified_at: u64,
    ) -> FolderResult<Self> {
        Self::with_timestamps_and_tree(
            id,
            name,
            storage_path,
            parent_id,
            Uuid::nil(),
            created_at,
            modified_at,
            modified_at,
        )
    }

    /// Full constructor used by the PG repository when reading rows.
    /// `tree_modified_at` comes from the trigger-maintained column on
    /// `storage.folders` and feeds [`Folder::etag`]. `drive_id` is the
    /// post-D0 `storage.folders.drive_id NOT NULL` column — every
    /// path-based lookup scopes by this axis. `created_by` /
    /// `updated_by` are the §14 provenance columns; both are nullable
    /// because the M1 FK is `ON DELETE SET NULL` (a deleted user
    /// leaves authored rows in place).
    #[allow(clippy::too_many_arguments)]
    pub fn with_timestamps_and_tree(
        id: String,
        name: String,
        storage_path: StoragePath,
        parent_id: Option<String>,
        drive_id: Uuid,
        created_at: u64,
        modified_at: u64,
        tree_modified_at: u64,
    ) -> FolderResult<Self> {
        Self::with_timestamps_tree_and_provenance(
            id,
            name,
            storage_path,
            parent_id,
            drive_id,
            created_at,
            modified_at,
            tree_modified_at,
            None,
            None,
        )
    }

    /// Full constructor including the §14 provenance columns
    /// (`created_by` / `updated_by`). Direct PG-row callers use this
    /// to preserve authorship through the entity layer.
    #[allow(clippy::too_many_arguments)]
    pub fn with_timestamps_tree_and_provenance(
        id: String,
        name: String,
        storage_path: StoragePath,
        parent_id: Option<String>,
        drive_id: Uuid,
        created_at: u64,
        modified_at: u64,
        tree_modified_at: u64,
        created_by: Option<Uuid>,
        updated_by: Option<Uuid>,
    ) -> FolderResult<Self> {
        let name = normalize_storage_name(&name);
        if let Err(reason) = validate_storage_name(&name) {
            return Err(FolderError::InvalidFolderName(format!("{name}: {reason}")));
        }

        let path_string = storage_path.to_string();

        Ok(Self {
            id,
            name,
            storage_path,
            path_string,
            parent_id,
            drive_id,
            created_at,
            modified_at,
            tree_modified_at,
            created_by,
            updated_by,
        })
    }

    // Getters
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn storage_path(&self) -> &StoragePath {
        &self.storage_path
    }

    pub fn path_string(&self) -> &str {
        &self.path_string
    }

    pub fn parent_id(&self) -> Option<&str> {
        self.parent_id.as_deref()
    }

    pub fn created_at(&self) -> u64 {
        self.created_at
    }

    pub fn modified_at(&self) -> u64 {
        self.modified_at
    }

    /// Drive that owns this folder. Path-based lookups scope by
    /// this axis (post-D0 invariant: `storage.folders.drive_id`
    /// is `NOT NULL`).
    pub fn drive_id(&self) -> Uuid {
        self.drive_id
    }

    /// User that originally created this folder (§14 provenance).
    /// `None` when the referenced user has been deleted
    /// (FK is `ON DELETE SET NULL`) or for in-memory/DTO-reconstructed
    /// entities.
    pub fn created_by(&self) -> Option<Uuid> {
        self.created_by
    }

    /// User that performed the most recent mutation that bumped
    /// `updated_at`. Authorship signal — distinct from ownership.
    /// `None` when the referenced user has been deleted or for
    /// in-memory/DTO-reconstructed entities.
    pub fn updated_by(&self) -> Option<Uuid> {
        self.updated_by
    }

    /// Latest descendant-write timestamp. Statement-level Postgres
    /// triggers enqueue every file/folder write into
    /// `storage.tree_etag_dirty`; the background `TreeEtagFlushService`
    /// drains the queue (default every 500 ms) and bumps the ltree
    /// ancestor chain in one batched UPDATE. Eventually consistent:
    /// the bump lands within ~one flush interval AFTER the change is
    /// committed, never before. See migration
    /// `20260627000000_async_tree_etag_queue.sql` for the rationale
    /// (user-facing writes must take zero folder-row locks).
    pub fn tree_modified_at(&self) -> u64 {
        self.tree_modified_at
    }

    /// Opaque HTTP ETag string (raw, NOT HTTP-quoted). Handlers wrap
    /// in `"…"` themselves at the HTTP boundary.
    ///
    /// Thin instance-method wrapper around [`Folder::compute_etag`]
    /// — see that function for the formula and the rationale.
    /// Raw-row listings (favorites, trash, recents, search) call
    /// the static form so the same formula governs every code path.
    pub fn etag(&self) -> String {
        Self::compute_etag(&self.id, self.tree_modified_at)
    }

    /// Pure formula for the folder ETag, exposed as a static method
    /// so callers that don't have a fully-constructed `Folder` (raw
    /// SQL rows in listing handlers, search results, etc.) route
    /// through the same definition.
    ///
    /// **Formula**: `{id[..16]}-{tree_modified_at}`.
    ///
    /// - The 16-char UUID prefix gives the folder its identity
    ///   component — keeps two empty same-mtime folders distinct.
    /// - `tree_modified_at` (Unix seconds) is the actual signal:
    ///   bumped whenever ANY descendant (file or sub-folder, at any
    ///   depth) is created, modified, deleted, or moved. This is the
    ///   contract NextCloud's sync engine relies on — "did anything
    ///   change inside this collection since I last looked?". Until
    ///   this column existed, the answer was always "no" because the
    ///   folder UUID never changed; clients had to do periodic deep
    ///   PROPFIND walks to discover web-uploaded files.
    /// - The bump is asynchronous: triggers enqueue, the
    ///   `TreeEtagFlushService` applies (≤ ~one flush interval after
    ///   commit, monotonic — two flushes in the same wall-clock
    ///   second still yield distinct values). Clients only compare
    ///   etags across successive polls, so the short lag is
    ///   unobservable; what matters is the bump never precedes the
    ///   change becoming visible.
    /// - Renaming the folder itself does NOT change the etag's
    ///   identity portion (UUID is stable across renames). A rename
    ///   does enqueue the ancestor chain, so the PARENT's etag still
    ///   changes — which is correct, the parent collection's listing
    ///   changed; the folder's own value stays untouched
    ///   (self-exclusion).
    pub fn compute_etag(id: &str, tree_modified_at: u64) -> String {
        let prefix: String = id.chars().take(16).collect();
        format!("{}-{}", prefix, tree_modified_at)
    }

    /// Creates a new Folder instance from a DTO
    /// This function is primarily for conversions in batch handlers
    pub fn from_dto(
        id: String,
        name: String,
        path: String,
        parent_id: Option<String>,
        created_at: u64,
        modified_at: u64,
    ) -> Self {
        // Create storage_path from the string
        let storage_path = StoragePath::from_string(&path);

        // Create directly without validation to avoid errors in DTO
        // conversions. Still NFC-normalize so DTO-reconstructed
        // entities maintain the storage invariant.
        // `tree_modified_at` defaults to `modified_at`: DTO
        // round-trips lose the real rollup signal, so callers that
        // need a freshly-rolled-up etag must reload from the
        // repository.
        let name = normalize_storage_name(&name);
        Self {
            id,
            name,
            storage_path,
            path_string: path,
            parent_id,
            // DTO round-trips lose drive_id (FolderDto carries it,
            // but the legacy `from_dto` signature predates this
            // change). Callers that need real scoping must reload
            // through the repository.
            drive_id: Uuid::nil(),
            created_at,
            modified_at,
            tree_modified_at: modified_at,
            // DTO round-trips through this constructor lose
            // provenance; callers that need it reload through the repo.
            created_by: None,
            updated_by: None,
        }
    }

    // Methods to create new versions of the folder (immutable)

    /// Creates a new version of the folder with updated name
    pub fn with_name(&self, new_name: String) -> FolderResult<Self> {
        let new_name = normalize_storage_name(&new_name);
        if let Err(reason) = validate_storage_name(&new_name) {
            return Err(FolderError::InvalidFolderName(format!(
                "{new_name}: {reason}"
            )));
        }

        // Update path based on the name
        let parent_path = self.storage_path.parent();
        let new_storage_path = match parent_path {
            Some(parent) => parent.join(&new_name),
            None => StoragePath::from_string(&new_name),
        };

        // Update string representation
        let new_path_string = new_storage_path.to_string();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Ok(Self {
            id: self.id.clone(),
            name: new_name,
            storage_path: new_storage_path,
            path_string: new_path_string,
            parent_id: self.parent_id.clone(),
            drive_id: self.drive_id,
            created_at: self.created_at,
            modified_at: now,
            // Renaming bumps both self and descendant rollup —
            // ancestors' listings now show a new name, so the
            // collection has materially changed.
            tree_modified_at: now,
            // Provenance is preserved across the in-memory rebuild;
            // real persisted updates re-read from the DB.
            created_by: self.created_by,
            updated_by: self.updated_by,
        })
    }

    /// Creates a new version of the folder with updated parent
    pub fn with_parent(
        &self,
        parent_id: Option<String>,
        parent_path: Option<StoragePath>,
    ) -> FolderResult<Self> {
        // We need a folder path to update the path
        let new_storage_path = match parent_path {
            Some(path) => path.join(&self.name),
            None => StoragePath::from_string(&self.name), // Root
        };

        // Update string representation
        let new_path_string = new_storage_path.to_string();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Ok(Self {
            id: self.id.clone(),
            name: self.name.clone(),
            storage_path: new_storage_path,
            path_string: new_path_string,
            parent_id,
            drive_id: self.drive_id,
            created_at: self.created_at,
            modified_at: now,
            tree_modified_at: now,
            created_by: self.created_by,
            updated_by: self.updated_by,
        })
    }

    /// Returns an absolute path for this folder
    pub fn get_absolute_path<P: AsRef<std::path::Path>>(&self, root_path: P) -> std::path::PathBuf {
        let mut result = std::path::PathBuf::from(root_path.as_ref());

        // Skip leading '/' from path_string to avoid creating absolute path incorrectly
        let relative_path = if self.path_string.starts_with('/') {
            &self.path_string[1..]
        } else {
            &self.path_string
        };

        if !relative_path.is_empty() {
            result.push(relative_path);
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_folder_creation_with_valid_name() {
        let storage_path = StoragePath::from_string("/test/folder");
        let folder = Folder::new(
            "123".to_string(),
            "my_folder".to_string(),
            storage_path,
            None,
        );

        assert!(folder.is_ok());
    }

    #[test]
    fn test_folder_creation_with_invalid_name() {
        let storage_path = StoragePath::from_string("/test/invalid/folder");
        let folder = Folder::new(
            "123".to_string(),
            "folder/with/slash".to_string(), // Invalid name
            storage_path,
            None,
        );

        assert!(folder.is_err());
        match folder {
            Err(FolderError::InvalidFolderName(_)) => (),
            _ => panic!("Expected InvalidFolderName error"),
        }
    }

    #[test]
    fn test_folder_with_name() {
        let storage_path = StoragePath::from_string("/test/folder");
        let folder = Folder::new(
            "123".to_string(),
            "old_name".to_string(),
            storage_path,
            None,
        )
        .unwrap();

        let renamed = folder.with_name("new_name".to_string());
        assert!(renamed.is_ok());
        let renamed = renamed.unwrap();
        assert_eq!(renamed.name(), "new_name");
        assert_eq!(renamed.id(), "123"); // The ID doesn't change
    }

    /// The folder ETag is `{id[..16]}-{tree_modified_at}`. Two
    /// fixtures with identical id-prefix + tree_modified_at must
    /// produce byte-identical ETags — that's what NC's incremental
    /// sync relies on across PROPFIND cycles.
    #[test]
    fn test_etag_combines_id_prefix_and_tree_modified_at() {
        let folder = Folder::with_timestamps_and_tree(
            "0123456789abcdefZZZZZZZZ".to_string(),
            "folder".to_string(),
            StoragePath::from_string("/folder"),
            None,
            Uuid::nil(),
            1_000,
            2_000,
            5_000,
        )
        .unwrap();

        assert_eq!(folder.tree_modified_at(), 5_000);
        assert_eq!(folder.etag(), "0123456789abcdef-5000");
    }

    /// Two folders with the same `tree_modified_at` but different
    /// IDs must NOT collide on ETag — the id prefix is the identity
    /// portion that keeps them distinct.
    #[test]
    fn test_etag_distinct_folders_same_tree_mtime() {
        let a = Folder::with_timestamps_and_tree(
            "aaaaaaaaaaaaaaaaZZZZZZZZ".to_string(),
            "a".to_string(),
            StoragePath::from_string("/a"),
            None,
            Uuid::nil(),
            0,
            0,
            42,
        )
        .unwrap();
        let b = Folder::with_timestamps_and_tree(
            "bbbbbbbbbbbbbbbbZZZZZZZZ".to_string(),
            "b".to_string(),
            StoragePath::from_string("/b"),
            None,
            Uuid::nil(),
            0,
            0,
            42,
        )
        .unwrap();

        assert_ne!(a.etag(), b.etag());
    }

    /// `tree_modified_at` is the actual change-detection signal —
    /// the trigger bumps it for descendant writes. Renaming the
    /// folder bumps both `modified_at` and `tree_modified_at`
    /// (the parent collection's listing changed), and the etag
    /// must reflect that — otherwise NC won't notice the rename.
    #[test]
    fn test_etag_changes_when_tree_modified_at_changes() {
        let folder_a = Folder::with_timestamps_and_tree(
            "abcd1234efgh5678ZZZZZZZZ".to_string(),
            "folder".to_string(),
            StoragePath::from_string("/folder"),
            None,
            Uuid::nil(),
            1_000,
            2_000,
            3_000,
        )
        .unwrap();
        let folder_b = Folder::with_timestamps_and_tree(
            "abcd1234efgh5678ZZZZZZZZ".to_string(),
            "folder".to_string(),
            StoragePath::from_string("/folder"),
            None,
            Uuid::nil(),
            1_000,
            2_000,
            4_000,
        )
        .unwrap();

        assert_ne!(folder_a.etag(), folder_b.etag());
    }
}
