use std::path::PathBuf;
use std::time::Duration;
use tokio::fs;

use crate::common::errors::{DomainError, Result};

/// In-RAM running byte counter per upload session (`user/upload_id` →
/// bytes accepted so far). The per-chunk quota gate used to recompute
/// this by listing the whole session directory and stat-ing every chunk
/// on EVERY chunk PUT — O(k) stats for chunk k, O(N²/2) over an upload
/// (~500k stats for a 10 GB / 1000-chunk upload). The counter makes the
/// gate O(1); a cache miss (process restart, eviction) lazily rebuilds
/// from the directory listing, so crash-correctness is unchanged
/// (benches/NC-CHUNK-GATE.md). Sessions are forgotten on cleanup; the
/// TTL reaps counters for sessions the client abandoned.
fn build_session_bytes_cache() -> moka::sync::Cache<String, u64> {
    moka::sync::Cache::builder()
        .max_capacity(100_000)
        .time_to_idle(Duration::from_secs(24 * 3600))
        .build()
}

#[derive(Clone)]
pub struct NextcloudChunkedUploadService {
    pub base_dir: PathBuf,
    /// See [`build_session_bytes_cache`]. Cloning the service shares the
    /// counter (moka `Cache` clones are handles to the same store).
    session_bytes: moka::sync::Cache<String, u64>,
}

impl NextcloudChunkedUploadService {
    pub fn new(base_dir: PathBuf) -> Self {
        Self {
            base_dir,
            session_bytes: build_session_bytes_cache(),
        }
    }

    pub fn new_stub() -> Self {
        Self {
            base_dir: PathBuf::from("./storage/.uploads/nextcloud"),
            session_bytes: build_session_bytes_cache(),
        }
    }

    fn bytes_key(user: &str, upload_id: &str) -> String {
        format!("{user}/{upload_id}")
    }

    /// Session bytes accepted so far, if the counter is warm.
    /// `None` = rebuild from the directory listing and call
    /// [`Self::set_session_bytes`].
    pub fn cached_session_bytes(&self, user: &str, upload_id: &str) -> Option<u64> {
        self.session_bytes.get(&Self::bytes_key(user, upload_id))
    }

    /// Seed / overwrite the session counter (post-rebuild or on MKCOL).
    pub fn set_session_bytes(&self, user: &str, upload_id: &str, bytes: u64) {
        self.session_bytes
            .insert(Self::bytes_key(user, upload_id), bytes);
    }

    /// Add an accepted chunk's bytes to the counter (no-op when cold —
    /// the next gate rebuilds from disk). Two racing PUTs on one session
    /// could drop an increment; the counter is a gate hint, and the
    /// MOVE-time quota check stays authoritative.
    pub fn bump_session_bytes(&self, user: &str, upload_id: &str, delta: u64) {
        let key = Self::bytes_key(user, upload_id);
        if let Some(current) = self.session_bytes.get(&key) {
            self.session_bytes
                .insert(key, current.saturating_add(delta));
        }
    }

    /// Drop the counter (session cleanup, or a chunk overwrite made the
    /// running total untrustworthy — rebuilt lazily on next use).
    pub fn forget_session_bytes(&self, user: &str, upload_id: &str) {
        self.session_bytes
            .invalidate(&Self::bytes_key(user, upload_id));
    }

    /// Validate that a path component contains no traversal characters.
    fn validate_path_component(name: &str, label: &str) -> Result<()> {
        if name.is_empty()
            || name.contains('/')
            || name.contains('\\')
            || name.contains("..")
            || name == "."
        {
            return Err(DomainError::validation_error(format!(
                "ChunkedUpload: invalid {}: contains path traversal characters",
                label
            )));
        }
        Ok(())
    }

    /// Build a session directory path and verify it's inside base_dir.
    fn safe_session_dir(&self, user: &str, upload_id: &str) -> Result<PathBuf> {
        Self::validate_path_component(user, "username")?;
        Self::validate_path_component(upload_id, "upload_id")?;
        Ok(self.base_dir.join(user).join(upload_id))
    }

    /// Create a new upload session directory.
    pub async fn create_session(&self, user: &str, upload_id: &str) -> Result<()> {
        let session_dir = self.safe_session_dir(user, upload_id)?;
        fs::create_dir_all(&session_dir)
            .await
            .map_err(|e| DomainError::internal_error("ChunkedUpload", e.to_string()))?;
        self.set_session_bytes(user, upload_id, 0);
        Ok(())
    }

    /// Resolve and validate the filesystem path for a chunk file.
    ///
    /// Public so the interface layer can stream an HTTP body straight into
    /// the chunk file without copying through the service. The service
    /// retains responsibility for path-component validation; the caller
    /// owns the I/O (open, write, fsync, size enforcement, cleanup on
    /// failure). All three `validate_path_component` calls run before the
    /// path is constructed, so a returned `PathBuf` is always inside
    /// `base_dir/{user}/{upload_id}`.
    pub fn safe_chunk_path(
        &self,
        user: &str,
        upload_id: &str,
        chunk_name: &str,
    ) -> Result<PathBuf> {
        Self::validate_path_component(chunk_name, "chunk_name")?;
        Ok(self.safe_session_dir(user, upload_id)?.join(chunk_name))
    }

    /// Store a chunk in the session directory. Buffers `data` in memory —
    /// use [`safe_chunk_path`](Self::safe_chunk_path) + the
    /// `interfaces/upload_ingest::stream_body_to_path` helper to stream the
    /// HTTP body directly to disk and avoid materialising the whole chunk
    /// in RAM.
    ///
    /// Uses `tokio::fs::write` (single `spawn_blocking` around
    /// `std::fs::write`) rather than manually driving
    /// `create + write_all` and letting the tokio handle drop close the
    /// fd. The manual shape leaked a race: `tokio::fs::File::drop`
    /// dispatches `close(2)` to the blocking pool without awaiting it,
    /// and until close completes the dirent update may not be visible
    /// to a subsequent `read_dir` — on macOS APFS routinely, on Linux
    /// under I/O contention. In practice that turned into
    /// `ordered_chunk_paths` silently missing a just-uploaded chunk;
    /// the NC assembly path (`handle_assemble` → `ordered_chunk_paths`)
    /// would then produce a truncated file with no error to the client.
    /// `std::fs::write` opens, writes, and synchronously closes before
    /// returning, so the dirent is guaranteed visible on `.await`.
    pub async fn store_chunk(
        &self,
        user: &str,
        upload_id: &str,
        chunk_name: &str,
        data: &[u8],
    ) -> Result<()> {
        let chunk_path = self.safe_chunk_path(user, upload_id, chunk_name)?;
        let overwrite = fs::metadata(&chunk_path).await.is_ok();
        fs::write(&chunk_path, data)
            .await
            .map_err(|e| DomainError::internal_error("ChunkedUpload", e.to_string()))?;
        if overwrite {
            // Retried chunk — running total is stale; rebuild lazily.
            self.forget_session_bytes(user, upload_id);
        } else {
            self.bump_session_bytes(user, upload_id, data.len() as u64);
        }
        Ok(())
    }

    /// List the session's chunk files in assembly (numeric) order.
    ///
    /// The caller streams these directly into the CDC chunk store
    /// (`interfaces::upload_ingest::stream_from_files`) — chunking, BLAKE3
    /// hashing and dedup checks happen in that single read pass, so no
    /// assembled temp file is ever written. The chunk parts stay on disk
    /// until [`cleanup`](Self::cleanup), keeping completion retryable.
    pub async fn ordered_chunk_paths(&self, user: &str, upload_id: &str) -> Result<Vec<PathBuf>> {
        let session_dir = self.safe_session_dir(user, upload_id)?;
        let mut entries: Vec<String> = Vec::new();

        let mut dir = fs::read_dir(&session_dir)
            .await
            .map_err(|e| DomainError::internal_error("ChunkedUpload", e.to_string()))?;

        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(|e| DomainError::internal_error("ChunkedUpload", e.to_string()))?
        {
            let name = entry.file_name().to_string_lossy().to_string();
            // `.file` is the NC-protocol assembly marker; `.assembled` is
            // the staging file older releases wrote — sessions in flight
            // across an upgrade may still contain one.
            if name == ".file" || name == ".assembled" {
                continue;
            }
            entries.push(name);
        }

        // Sort chunks numerically (Nextcloud sends them as "00001", "00002", ...).
        entries.sort();

        Ok(entries.iter().map(|n| session_dir.join(n)).collect())
    }

    /// Delete the upload session directory.
    pub async fn cleanup(&self, user: &str, upload_id: &str) -> Result<()> {
        let session_dir = self.safe_session_dir(user, upload_id)?;
        if session_dir.exists() {
            fs::remove_dir_all(&session_dir)
                .await
                .map_err(|e| DomainError::internal_error("ChunkedUpload", e.to_string()))?;
        }
        self.forget_session_bytes(user, upload_id);
        Ok(())
    }

    /// Check if a session directory exists.
    pub async fn session_exists(&self, user: &str, upload_id: &str) -> bool {
        self.safe_session_dir(user, upload_id)
            .map(|p| p.exists())
            .unwrap_or(false)
    }

    /// Enumerate the chunks already stored in a session, plus the
    /// session directory's own mtime. Used by the PROPFIND handler
    /// to drive NextCloud's resume-upload flow — the Android client
    /// (and several mobile clients) issue PROPFIND on the session
    /// URL to discover which chunks are already uploaded, then only
    /// PUT the missing ones.
    ///
    /// Returns `None` when the session directory doesn't exist
    /// (handler maps to 404). The `.file` and `.assembled` markers
    /// are filtered out — they're internal bookkeeping, not real
    /// chunks the client uploaded.
    pub async fn list_chunks(&self, user: &str, upload_id: &str) -> Result<Option<SessionListing>> {
        let session_dir = self.safe_session_dir(user, upload_id)?;
        if !session_dir.exists() {
            return Ok(None);
        }

        let session_meta = fs::metadata(&session_dir)
            .await
            .map_err(|e| DomainError::internal_error("ChunkedUpload", e.to_string()))?;
        let session_mtime = session_meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let mut chunks: Vec<ChunkInfo> = Vec::new();
        let mut dir = fs::read_dir(&session_dir)
            .await
            .map_err(|e| DomainError::internal_error("ChunkedUpload", e.to_string()))?;
        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(|e| DomainError::internal_error("ChunkedUpload", e.to_string()))?
        {
            let name = entry.file_name().to_string_lossy().to_string();
            // Filter internal markers — `.file` is the NC-protocol
            // assembly trigger target (it never reaches the disk
            // because MOVE redirects it), `.assembled` is the staging
            // file pre-streaming releases wrote (kept for sessions in
            // flight across an upgrade). Surfacing either to the
            // client would confuse its chunk-count check.
            if name == ".file" || name == ".assembled" {
                continue;
            }
            let meta = entry
                .metadata()
                .await
                .map_err(|e| DomainError::internal_error("ChunkedUpload", e.to_string()))?;
            let size = meta.len();
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            chunks.push(ChunkInfo { name, size, mtime });
        }
        // Sort by chunk name so PROPFIND output is deterministic
        // (clients don't strictly require this, but reproducible
        // listings make debugging from logs much easier).
        chunks.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(Some(SessionListing {
            session_mtime,
            chunks,
        }))
    }
}

/// One chunk file inside an upload session.
#[derive(Debug, Clone)]
pub struct ChunkInfo {
    pub name: String,
    pub size: u64,
    pub mtime: u64,
}

/// What `list_chunks` returns: the session's own mtime (for the
/// collection's `<d:getlastmodified>`) plus the list of stored
/// chunks.
#[derive(Debug, Clone)]
pub struct SessionListing {
    pub session_mtime: u64,
    pub chunks: Vec<ChunkInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_service() -> (NextcloudChunkedUploadService, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("create temp dir");
        let svc = NextcloudChunkedUploadService::new(dir.path().to_path_buf());
        (svc, dir)
    }

    #[tokio::test]
    async fn test_create_session() {
        let (svc, _dir) = test_service();
        svc.create_session("alice", "upload-001").await.unwrap();
        assert!(svc.session_exists("alice", "upload-001").await);
    }

    #[tokio::test]
    async fn test_session_not_exists_before_create() {
        let (svc, _dir) = test_service();
        assert!(!svc.session_exists("alice", "upload-999").await);
    }

    /// Concatenate the session's chunk files in assembly order — mirrors
    /// what `upload_ingest::stream_from_files` feeds the CDC store.
    async fn concat_chunks(svc: &NextcloudChunkedUploadService, user: &str, id: &str) -> Vec<u8> {
        let mut out = Vec::new();
        for path in svc.ordered_chunk_paths(user, id).await.unwrap() {
            out.extend_from_slice(&fs::read(&path).await.unwrap());
        }
        out
    }

    #[tokio::test]
    async fn test_store_and_order_chunks() {
        let (svc, _dir) = test_service();
        svc.create_session("alice", "upload-002").await.unwrap();

        svc.store_chunk("alice", "upload-002", "00001", b"Hello, ")
            .await
            .unwrap();
        svc.store_chunk("alice", "upload-002", "00002", b"World!")
            .await
            .unwrap();

        assert_eq!(
            concat_chunks(&svc, "alice", "upload-002").await,
            b"Hello, World!"
        );
    }

    #[tokio::test]
    async fn test_chunk_paths_sorted_regardless_of_upload_order() {
        let (svc, _dir) = test_service();
        svc.create_session("alice", "upload-003").await.unwrap();

        // Store out of order.
        svc.store_chunk("alice", "upload-003", "00003", b"C")
            .await
            .unwrap();
        svc.store_chunk("alice", "upload-003", "00001", b"A")
            .await
            .unwrap();
        svc.store_chunk("alice", "upload-003", "00002", b"B")
            .await
            .unwrap();

        // Chunks were stored in order 3,1,2 but must concatenate as "ABC".
        assert_eq!(concat_chunks(&svc, "alice", "upload-003").await, b"ABC");
    }

    #[tokio::test]
    async fn test_internal_markers_excluded_from_chunk_paths() {
        let (svc, _dir) = test_service();
        svc.create_session("alice", "upload-005").await.unwrap();

        svc.store_chunk("alice", "upload-005", "00001", b"data")
            .await
            .unwrap();
        // Stale staging file from a pre-streaming release.
        svc.store_chunk("alice", "upload-005", ".assembled", b"old")
            .await
            .unwrap();

        let paths = svc
            .ordered_chunk_paths("alice", "upload-005")
            .await
            .unwrap();
        assert_eq!(paths.len(), 1, "markers must be filtered out");
        assert!(paths[0].ends_with("00001"));
    }

    #[tokio::test]
    async fn test_cleanup_removes_session() {
        let (svc, _dir) = test_service();
        svc.create_session("alice", "upload-004").await.unwrap();
        assert!(svc.session_exists("alice", "upload-004").await);

        svc.cleanup("alice", "upload-004").await.unwrap();
        assert!(!svc.session_exists("alice", "upload-004").await);
    }

    #[tokio::test]
    async fn test_cleanup_nonexistent_session_is_ok() {
        let (svc, _dir) = test_service();
        // Should not error.
        svc.cleanup("alice", "nonexistent").await.unwrap();
    }
}
