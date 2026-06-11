//! Chunked Upload Port - Application layer abstraction for resumable chunked uploads.
//!
//! This module defines the port (trait) and DTOs for chunked/resumable upload
//! operations, keeping the application and interface layers independent of
//! the specific upload implementation (TUS-like protocol, S3 multipart, etc.).

use crate::common::errors::DomainError;
use bytes::Bytes;
use serde::Serialize;
use std::path::PathBuf;
use utoipa::ToSchema;
use uuid::Uuid;

/// Default chunk size (5 MB) — optimised for parallel transfers.
pub const DEFAULT_CHUNK_SIZE: usize = 5 * 1024 * 1024;

/// Minimum file size to use chunked upload (10 MB).
pub const CHUNKED_UPLOAD_THRESHOLD: usize = 10 * 1024 * 1024;

/// Algorithm used by the client-side chunk checksum.
///
/// The wire format is `?checksum=<hex>&checksumalg=<name>` (or the
/// equivalent header pair for older clients that send only `Content-MD5`).
/// Clients that omit `checksumalg` are assumed to mean MD5 — that's the
/// algorithm baked into the legacy `Content-MD5` header (RFC 1864), TUS-
/// like upload protocols, and S3 multipart ETags.
///
/// Three supported variants, all from already-declared dependencies:
/// - `Md5` — legacy default; weak cryptographically but fine for
///   transport-integrity checks under TLS.
/// - `Sha256` — industry-standard, FIPS-compliant, widely supported by
///   sync clients (AWS S3 also accepts SHA-256 trailers).
/// - `Blake3` — fastest of the three; already used by the blob-storage
///   layer, so the chunk-level integrity check and the assembled-file
///   dedup hash use the same algorithm when clients opt in.
///
/// Skipped intentionally: SHA-1 (deprecated, broken), CRC32 (too weak for
/// integrity claims). Both can be added if a real client need appears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumAlg {
    Md5,
    Sha256,
    Blake3,
}

impl ChecksumAlg {
    /// Parse a client-supplied algorithm name. Case-insensitive. Accepts
    /// `sha-256` as a synonym for `sha256` since both forms are common
    /// in HTTP headers. Unknown names return `None` so the handler can
    /// 400 with the offending value.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "md5" => Some(Self::Md5),
            "sha256" | "sha-256" => Some(Self::Sha256),
            "blake3" => Some(Self::Blake3),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Md5 => "md5",
            Self::Sha256 => "sha256",
            Self::Blake3 => "blake3",
        }
    }
}

/// Response returned when a new upload session is created.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CreateUploadResponseDto {
    pub upload_id: String,
    pub chunk_size: usize,
    pub total_chunks: usize,
    pub expires_at: u64,
}

/// Response returned after a single chunk is uploaded.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ChunkUploadResponseDto {
    pub chunk_index: usize,
    pub bytes_received: u64,
    pub progress: f64,
    pub is_complete: bool,
}

/// Response for querying upload session status.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct UploadStatusResponseDto {
    pub upload_id: String,
    pub filename: String,
    pub total_size: u64,
    pub bytes_received: u64,
    pub progress: f64,
    pub total_chunks: usize,
    pub completed_chunks: usize,
    pub pending_chunks: Vec<usize>,
    pub is_complete: bool,
}

/// A completed upload session, ready to be streamed into the blob store.
///
/// `chunk_paths` lists every chunk file in assembly order — the caller
/// concatenates them as one byte stream (CDC chunking + hashing happen in
/// that single pass). The files stay on disk until `finalize_upload`, so a
/// failed completion (e.g. checksum mismatch) remains retryable.
#[derive(Debug, Clone)]
pub struct CompletedUploadParts {
    pub chunk_paths: Vec<PathBuf>,
    pub filename: String,
    pub folder_id: Option<String>,
    pub content_type: String,
    pub total_size: u64,
}

/// Port for chunked/resumable upload operations.
///
/// Implementations manage upload sessions, chunk storage, reassembly,
/// and cleanup, while the application layer only interacts through
/// this abstraction.
pub trait ChunkedUploadPort: Send + Sync + 'static {
    /// Create a new upload session.
    ///
    /// Returns session metadata including the upload ID, chunk size,
    /// total number of chunks, and expiration timestamp.
    async fn create_session(
        &self,
        user_id: Uuid,
        filename: String,
        folder_id: Option<String>,
        content_type: String,
        total_size: u64,
        chunk_size: Option<usize>,
    ) -> Result<CreateUploadResponseDto, DomainError>;

    /// Upload a single chunk.
    ///
    /// `checksum` is an optional MD5 hex string for integrity verification.
    async fn upload_chunk(
        &self,
        upload_id: &str,
        user_id: Uuid,
        chunk_index: usize,
        data: Bytes,
        checksum: Option<String>,
    ) -> Result<ChunkUploadResponseDto, DomainError>;

    /// Get the current status of an upload session.
    async fn get_status(
        &self,
        upload_id: &str,
        user_id: Uuid,
    ) -> Result<UploadStatusResponseDto, DomainError>;

    /// Validate completion and hand back the ordered chunk parts.
    ///
    /// No assembled file is written — the caller streams the parts straight
    /// into the content-addressable store.
    async fn complete_upload(
        &self,
        upload_id: &str,
        user_id: Uuid,
    ) -> Result<CompletedUploadParts, DomainError>;

    /// Finalize upload: clean up the session and temporary files.
    async fn finalize_upload(&self, upload_id: &str, user_id: Uuid) -> Result<(), DomainError>;

    /// Cancel an upload and clean up all temporary data.
    async fn cancel_upload(&self, upload_id: &str, user_id: Uuid) -> Result<(), DomainError>;

    /// Check if a file size qualifies for chunked upload.
    fn should_use_chunked(&self, size: u64) -> bool;
}
