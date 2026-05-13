use std::future::Future;
use std::pin::Pin;

/// Observer notified by [`FileUploadService`] when a new file record is created
/// (including dedup hits where the blob already exists).
///
/// Register with [`FileUploadService::with_file_created_hook`] during DI wiring.
pub trait FileCreatedHook: Send + Sync {
    /// Called after the file record has been persisted.
    /// `file_id` — opaque file UUID string.
    /// `blob_hash` — BLAKE3 hex of the blob (may already exist on disk for dedup hits).
    /// `content_type` — MIME type of the content.
    /// Must be best-effort — must not propagate errors.
    fn on_file_created<'a>(
        &'a self,
        file_id: &'a str,
        blob_hash: &'a str,
        content_type: &'a str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

/// Observer notified by [`FileUploadService`] when an existing file's blob is
/// replaced (WebDAV PUT overwrite, WOPI PutFile, Nextcloud chunked upload).
///
/// Implement this trait on any service that needs to react to a content swap
/// (e.g. thumbnail invalidation + regeneration, search index update).
/// Register with [`FileUploadService::with_file_updated_hook`] during DI wiring.
///
/// The boxed-future return keeps the trait dyn-compatible so multiple
/// implementations can be stored as `Vec<Arc<dyn FileUpdatedHook>>`.
pub trait FileUpdatedHook: Send + Sync {
    /// Called after the new blob has been stored and the file record updated.
    ///
    /// `file_id` is an opaque file UUID string, `blob_hash` is the BLAKE3 hex
    /// of the new blob, `content_type` is the MIME type of the new content.
    /// Must be best-effort — must not propagate errors.
    fn on_file_updated<'a>(
        &'a self,
        file_id: &'a str,
        blob_hash: &'a str,
        content_type: &'a str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

/// Observer notified by [`FileManagementService`] when a file is permanently
/// deleted (either directly or after being emptied from trash).
///
/// Register with [`FileManagementService::with_file_deleted_hook`] during DI wiring.
pub trait FileDeletedHook: Send + Sync {
    /// Called after the file record has been removed.
    /// `file_id` — opaque file UUID string.
    /// Must be best-effort — must not propagate errors.
    fn on_file_deleted<'a>(
        &'a self,
        file_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}
