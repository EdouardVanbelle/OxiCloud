use std::future::Future;
use std::pin::Pin;

/// Observer notified by [`DedupService`] when a blob's ref_count reaches zero
/// and it is permanently removed from storage.
///
/// Implement this trait on any service that needs to react to blob deletion
/// (e.g. thumbnail cleanup, CDN invalidation, audit logging).  Register with
/// [`DedupService::add_blob_hook`] during DI wiring.
///
/// The boxed-future return keeps the trait dyn-compatible so multiple
/// implementations can be stored as `Vec<Arc<dyn BlobDeletionHook>>`.
pub trait BlobDeletionHook: Send + Sync {
    /// Called after the blob file has been removed from disk.
    /// `blob_hash` is the BLAKE3 hex string identifying the blob.
    /// Must be best-effort — must not propagate errors.
    fn on_blob_deleted<'a>(
        &'a self,
        blob_hash: &'a str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}
