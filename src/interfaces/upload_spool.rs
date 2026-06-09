//! Shared streaming upload spool: request body → temp file + incremental hash.
//!
//! Used by both the native WebDAV PUT handler and the NextCloud-compat PUT
//! handler so neither buffers the full request body in memory. Peak heap is
//! ~one HTTP frame regardless of file size; the body is written to a temp
//! file (off tmpfs when [`StorageConfig::upload_temp_dir`] is configured) and
//! BLAKE3-hashed on the fly so the dedup layer can short-circuit on a hit.

use std::path::{Path, PathBuf};

use axum::body::Body;
use http_body_util::BodyStream;
// The `Digest` trait (re-exported by both `md5` and `sha2` from the
// `digest` crate) gives `Md5` and `Sha256` their `new` / `update` /
// `finalize` methods. Importing once via `sha2` covers both —
// otherwise every call site would need fully-qualified
// `<md5::Md5 as md5::Digest>::…` syntax.
use sha2::Digest as _;
use tempfile::NamedTempFile;
use tokio::io::AsyncWriteExt;
use tokio_stream::StreamExt;

use crate::application::ports::chunked_upload_ports::ChecksumAlg;
use crate::common::temp::new_spool_temp_file;
use crate::interfaces::errors::AppError;

/// Outcome of spooling a request body to disk.
pub struct SpooledBody {
    /// The temp file holding the body. Kept alive by the caller (dropping it
    /// removes the file unless the dedup layer already consumed/moved it).
    pub temp: NamedTempFile,
    /// Hex-encoded BLAKE3 of the full body — matches `DedupService::hash_file`,
    /// so passing it as `pre_computed_hash` enables the dedup fast path.
    pub hash: String,
    /// Total bytes written.
    pub size: u64,
}

/// Stream an HTTP request body to a temp file, computing its BLAKE3 hash
/// incrementally and enforcing `max_upload` as a hard size limit.
///
/// Peak heap is ~one frame — the body is never fully buffered in RAM.
///
/// `temp_dir` is taken by value (not `&Path`) so the returned future captures
/// no borrowed lifetime — required for the handler future to stay `Send`.
pub async fn spool_body_to_temp(
    body: Body,
    max_upload: usize,
    temp_dir: Option<PathBuf>,
) -> Result<SpooledBody, AppError> {
    let temp = new_spool_temp_file(temp_dir.as_deref())
        .map_err(|e| AppError::internal_error(format!("Failed to create temp file: {e}")))?;
    let temp_path = temp.path().to_path_buf();

    let mut file = tokio::fs::File::create(&temp_path)
        .await
        .map_err(|e| AppError::internal_error(format!("Failed to open temp file: {e}")))?;

    let mut hasher = blake3::Hasher::new();
    let mut total_bytes: usize = 0;
    let mut stream = BodyStream::new(body);

    while let Some(frame_result) = stream.next().await {
        let frame = frame_result
            .map_err(|e| AppError::bad_request(format!("Failed to read request body: {e}")))?;
        if let Some(chunk) = frame.data_ref() {
            total_bytes += chunk.len();
            if total_bytes > max_upload {
                // Abort early — stop reading, delete temp file.
                drop(file);
                let _ = tokio::fs::remove_file(&temp_path).await;
                return Err(AppError::payload_too_large(format!(
                    "Upload body exceeds the direct-PUT cap ({max_upload} bytes). \
                     Use the chunked-upload protocol (REST: `/api/uploads/...`, \
                     NextCloud: `/remote.php/dav/uploads/...`) for files larger than this. \
                     Chunked uploads are resumable on transient failure."
                )));
            }
            hasher.update(chunk);
            file.write_all(chunk).await.map_err(|e| {
                AppError::internal_error(format!("Failed to write to temp file: {e}"))
            })?;
        }
    }
    file.flush()
        .await
        .map_err(|e| AppError::internal_error(format!("Failed to flush temp file: {e}")))?;
    drop(file);

    let hash = hasher.finalize().to_hex().to_string();
    Ok(SpooledBody {
        temp,
        hash,
        size: total_bytes as u64,
    })
}

/// Result of a streamed write to a caller-supplied path.
pub struct StreamedToPath {
    /// Total bytes written.
    pub bytes_written: u64,
    /// Lowercase hex digest, populated only when `checksum_alg=Some(_)`
    /// was passed. The algorithm is identified by [`StreamedToPath::alg`].
    pub checksum_hex: Option<String>,
    /// Algorithm used to compute `checksum_hex`. Echoed back so the
    /// caller can include it in audit logs or response headers.
    pub alg: Option<ChecksumAlg>,
}

/// Stream an HTTP request body directly to a known destination file,
/// enforcing `max_bytes` as a hard size limit.
///
/// Used by the chunked-upload PUT handlers — each chunk has a
/// deterministic on-disk path (`NextcloudChunkedUploadService::safe_chunk_path`
/// for the NC surface, `ChunkedUploadService::prepare_chunk` for the
/// REST surface), so there's no spool/move dance. Peak heap is ~one
/// HTTP frame regardless of chunk size or `max_bytes`.
///
/// `checksum_alg` is the optional client-requested integrity check
/// (default `md5` per the legacy `Content-MD5` contract; `blake3`
/// available for forward-compat). When `Some`, the hash is computed
/// incrementally during streaming — no extra disk read for verification.
///
/// On size overflow the partial file is removed before the function
/// returns, so a client retry against the same chunk name starts from
/// a clean slate. On any other I/O error the partial file is also
/// removed and the error surfaces — callers can assume the path is
/// either fully written or absent.
pub async fn stream_body_to_path(
    body: Body,
    path: &Path,
    max_bytes: usize,
    checksum_alg: Option<ChecksumAlg>,
) -> Result<StreamedToPath, AppError> {
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|e| AppError::internal_error(format!("Failed to open chunk file: {e}")))?;

    let mut total_bytes: usize = 0;
    let mut stream = BodyStream::new(body);
    let mut hasher = checksum_alg.map(IncrementalHasher::new);

    while let Some(frame_result) = stream.next().await {
        let frame = match frame_result {
            Ok(f) => f,
            Err(e) => {
                drop(file);
                let _ = tokio::fs::remove_file(path).await;
                return Err(AppError::bad_request(format!(
                    "Failed to read request body: {e}"
                )));
            }
        };
        if let Some(chunk) = frame.data_ref() {
            total_bytes += chunk.len();
            if total_bytes > max_bytes {
                drop(file);
                let _ = tokio::fs::remove_file(path).await;
                return Err(AppError::payload_too_large(format!(
                    "Chunk exceeds maximum size of {max_bytes} bytes"
                )));
            }
            if let Some(h) = hasher.as_mut() {
                h.update(chunk);
            }
            if let Err(e) = file.write_all(chunk).await {
                drop(file);
                let _ = tokio::fs::remove_file(path).await;
                return Err(AppError::internal_error(format!(
                    "Failed to write chunk: {e}"
                )));
            }
        }
    }
    file.flush()
        .await
        .map_err(|e| AppError::internal_error(format!("Failed to flush chunk file: {e}")))?;
    drop(file);

    Ok(StreamedToPath {
        bytes_written: total_bytes as u64,
        checksum_hex: hasher.map(IncrementalHasher::finalize_hex),
        alg: checksum_alg,
    })
}

/// Algorithm-agnostic incremental hasher used by [`stream_body_to_path`].
/// Per-frame `update` is sub-millisecond for all three algorithms at the
/// 64 KB frame sizes axum's body stream produces, so we don't need
/// `spawn_blocking` (which the old buffered path used because it hashed
/// the full multi-MB chunk in one shot).
enum IncrementalHasher {
    Md5(md5::Md5),
    Sha256(sha2::Sha256),
    // Boxing — blake3::Hasher is ~1.7 KB on the stack while md5::Md5
    // (~100 bytes) and sha2::Sha256 (~100 bytes) are tiny; boxing the
    // outlier keeps the enum size proportional to the common case
    // rather than the worst case.
    Blake3(Box<blake3::Hasher>),
}

impl IncrementalHasher {
    fn new(alg: ChecksumAlg) -> Self {
        match alg {
            ChecksumAlg::Md5 => Self::Md5(md5::Md5::new()),
            ChecksumAlg::Sha256 => Self::Sha256(sha2::Sha256::new()),
            ChecksumAlg::Blake3 => Self::Blake3(Box::new(blake3::Hasher::new())),
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        match self {
            Self::Md5(h) => h.update(bytes),
            Self::Sha256(h) => h.update(bytes),
            Self::Blake3(h) => {
                h.update(bytes);
            }
        }
    }

    fn finalize_hex(self) -> String {
        match self {
            Self::Md5(h) => h.finalize().iter().map(|b| format!("{b:02x}")).collect(),
            Self::Sha256(h) => h.finalize().iter().map(|b| format!("{b:02x}")).collect(),
            Self::Blake3(h) => h.finalize().to_hex().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    #[tokio::test]
    async fn stream_body_to_path_caps_oversized() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let path = temp_dir.path().join("chunk");

        // 5 MiB body, 4 MiB cap → must reject.
        let body = Body::from(Bytes::from(vec![0u8; 5 * 1024 * 1024]));
        let result = stream_body_to_path(body, &path, 4 * 1024 * 1024, None).await;
        assert!(
            result.is_err(),
            "expected PayloadTooLarge, got Ok(bytes_written={})",
            result.ok().map(|r| r.bytes_written).unwrap_or(0)
        );
        // Partial file must be removed on rejection.
        assert!(
            !path.exists(),
            "rejected chunk file should be removed, but {} still exists",
            path.display()
        );
    }

    #[tokio::test]
    async fn stream_body_to_path_accepts_under_cap() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let path = temp_dir.path().join("chunk");

        let body = Body::from(Bytes::from(vec![1u8; 1024 * 1024])); // 1 MiB
        let result = stream_body_to_path(body, &path, 4 * 1024 * 1024, None).await;
        let outcome = result.expect("should succeed");
        assert_eq!(outcome.bytes_written, 1024 * 1024);
        assert!(outcome.checksum_hex.is_none(), "no alg requested → no hash");
        assert!(path.exists());
    }

    #[tokio::test]
    async fn stream_body_to_path_caps_at_exact_boundary() {
        // Edge case: body exactly equal to cap should succeed; cap+1 must fail.
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let path = temp_dir.path().join("chunk");

        let body = Body::from(Bytes::from(vec![1u8; 100]));
        let outcome = stream_body_to_path(body, &path, 100, None)
            .await
            .expect("100 bytes at 100-byte cap should succeed");
        assert_eq!(outcome.bytes_written, 100);

        let path2 = temp_dir.path().join("chunk2");
        let body = Body::from(Bytes::from(vec![1u8; 101]));
        assert!(
            stream_body_to_path(body, &path2, 100, None).await.is_err(),
            "101 bytes at 100-byte cap must reject"
        );
        assert!(!path2.exists());
    }
}
