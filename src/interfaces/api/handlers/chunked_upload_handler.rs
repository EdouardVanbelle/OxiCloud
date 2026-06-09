//! Chunked Upload Handler - TUS-like Protocol Endpoints
//!
//! Provides HTTP endpoints for resumable, parallel chunk uploads:
//! - POST   /api/uploads          → Create upload session
//! - PATCH  /api/uploads/:id      → Upload a chunk
//! - HEAD   /api/uploads/:id      → Get upload status
//! - POST   /api/uploads/:id/complete → Assemble and finalize
//! - DELETE /api/uploads/:id      → Cancel upload

use axum::{
    Json,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use crate::application::ports::chunked_upload_ports::ChecksumAlg;
use crate::application::ports::chunked_upload_ports::ChunkedUploadPort;
use crate::application::ports::chunked_upload_ports::DEFAULT_CHUNK_SIZE;
use crate::application::ports::file_ports::FileUploadUseCase;
use crate::application::ports::folder_ports::FolderUseCase;
use crate::application::ports::storage_ports::StorageUsagePort;
use crate::common::di::AppState;
use crate::domain::services::authorization::Permission;
use crate::interfaces::errors::AppError;
use crate::interfaces::middleware::auth::AuthUser;
use crate::interfaces::upload_spool::stream_body_to_path;

/// Request body for creating an upload session
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateUploadRequest {
    pub filename: String,
    pub folder_id: Option<String>,
    pub content_type: Option<String>,
    pub total_size: u64,
    pub chunk_size: Option<usize>,
}

/// Query params for chunk upload.
///
/// `checksumalg` is parsed via [`ChecksumAlg::parse`] and defaults to
/// `Md5` when absent — matching the legacy `Content-MD5` contract that
/// older clients rely on. Unknown algorithm names produce a 400 with the
/// offending value echoed back.
#[derive(Debug, Deserialize)]
pub struct ChunkUploadParams {
    pub chunk_index: usize,
    pub checksum: Option<String>,
    pub checksumalg: Option<String>,
}

/// Final response after completing upload
#[derive(Debug, Serialize, ToSchema)]
pub struct CompleteUploadResponse {
    pub file_id: String,
    pub filename: String,
    pub size: u64,
    pub path: String,
}

/// Optional body for `POST /api/uploads/{id}/complete`.
///
/// When the client supplies `checksum`, the server compares it against
/// the assembled file's hash BEFORE promoting the blob to storage —
/// failure aborts the upload atomically (no orphaned blob, no DB row).
/// This is the end-to-end integrity check: per-chunk MD5 proves each
/// chunk arrived intact, but only the final hash catches assembly /
/// promotion bugs and mis-ordered chunks.
///
/// **`blake3` is highly recommended** — it's the algorithm the server
/// already runs over the assembled file during hash-on-write
/// assembly, so verification is a string comparison with zero extra
/// I/O and zero extra CPU. It's also the same algorithm the server
/// uses for blob-storage addressing, so the value the client sends
/// equals the `content_hash` they'd later read back from
/// `GET /api/files/{id}`. `md5` and `sha256` are accepted for
/// compatibility with legacy client tooling but each triggers a
/// second hash pass over the assembled file (~30–100 ms depending
/// on size).
///
/// `Default` keeps the existing wire shape: clients that POST with no
/// body get today's behavior (no verification, server just returns
/// what it computed).
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct CompleteUploadRequest {
    /// Lowercase hex digest the client expects the assembled file to
    /// hash to. Compared case-insensitively. Omit to skip verification.
    pub checksum: Option<String>,
    /// Algorithm name. `blake3` is the recommended choice (default —
    /// matches the server's hash-on-write algorithm, zero extra cost).
    /// `md5`, `sha256` / `sha-256` are accepted but trigger an extra
    /// hash pass. Unknown values return 400.
    pub checksumalg: Option<String>,
}

/// Chunked Upload Handler
///
/// The handler struct exists as a named grouping. All route functions are free
/// functions at module scope — see the section below the impl block for the reason.
pub struct ChunkedUploadHandler;

impl ChunkedUploadHandler {
    // ── Why no #[utoipa::path] here? ─────────────────────────────────────────────
    // utoipa 5.4.0's proc macro generates helper structs / impls inside its expansion.
    // Rust allows struct definitions at module scope but forbids them inside impl blocks,
    // so `#[utoipa::path]` fails on every method in this impl block regardless of HTTP
    // verb or annotation content. The same macro works fine on FileHandler / FolderHandler
    // (root cause in utoipa unknown — likely a 5.4.x bug). All five route handlers are
    // therefore declared as free functions below, which delegate to these `*_impl` methods.
    // TODO: try removing free-function indirection after a utoipa upgrade.

    /// POST /api/uploads - Create a new upload session
    ///
    /// Request body:
    /// ```json
    /// {
    ///   "filename": "large-video.mp4",
    ///   "folder_id": "optional-folder-id",
    ///   "content_type": "video/mp4",
    ///   "total_size": 104857600,
    ///   "chunk_size": 5242880
    /// }
    /// ```
    ///
    /// Response:
    /// ```json
    /// {
    ///   "upload_id": "uuid",
    ///   "chunk_size": 5242880,
    ///   "total_chunks": 20,
    ///   "expires_at": 86400
    /// }
    /// ```
    pub(super) async fn create_upload_impl(
        State(state): State<Arc<AppState>>,
        auth_user: AuthUser,
        Json(request): Json<CreateUploadRequest>,
    ) -> impl IntoResponse {
        let chunked_service = &state.core.chunked_upload_service;

        // Validate request
        if request.filename.is_empty() {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "Filename is required"
                })),
            )
                .into_response();
        }

        if request.total_size == 0 {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "Total size must be greater than 0"
                })),
            )
                .into_response();
        }

        // ── Whole-file cap ──────────────────────────────────────────
        // Reject upfront, before any chunk is uploaded — wasting
        // bandwidth + server disk on an upload that's going to be
        // rejected at /complete is the worst-of-both-worlds outcome.
        // `max_upload_size` is the same ceiling that bounds direct
        // PUTs (per-byte during streaming there; declared per-session
        // here). When quotas are disabled, this is the only whole-file
        // limit for chunked uploads — without it a hostile client
        // could declare `total_size: 1 TB` and accumulate chunks
        // until disk fills.
        let max_upload = state.core.config.storage.max_upload_size as u64;
        if request.total_size > max_upload {
            tracing::warn!(
                "⛔ CHUNKED UPLOAD REJECTED (total_size cap): user={}, file={}, declared={}, max={}",
                auth_user.username,
                request.filename,
                request.total_size,
                max_upload
            );
            return AppError::payload_too_large(format!(
                "Declared total_size {} exceeds the server's `max_upload_size` cap ({} bytes). \
                 Raise OXICLOUD_MAX_UPLOAD_SIZE on the server if larger uploads are expected.",
                request.total_size, max_upload
            ))
            .into_response();
        }

        // ── Permission pre-check: caller must have Create on the target
        // folder BEFORE we allocate a session and accept chunks. The
        // upload service re-checks at finalize time, but failing here
        // avoids wasting client+server resources on chunks that will be
        // rejected. None = caller's root namespace, no check needed.
        if let Some(ref fid) = request.folder_id
            && let Err(err) = state
                .applications
                .folder_service_concrete
                .require_permission(auth_user.id, Permission::Create, fid)
                .await
        {
            tracing::warn!(
                "⛔ CHUNKED UPLOAD REJECTED (no perm): user='{}' folder='{}' err='{}'",
                auth_user.username,
                fid,
                err
            );
            return AppError::from(err).into_response();
        }

        // ── Quota enforcement ────────────────────────────────────
        if let Some(storage_svc) = state.storage_usage_service.as_ref()
            && let Err(err) = storage_svc
                .check_storage_quota(auth_user.id, request.total_size)
                .await
        {
            tracing::warn!(
                "⛔ CHUNKED UPLOAD REJECTED (quota): user={}, file={}, size={} — {}",
                auth_user.username,
                request.filename,
                request.total_size,
                err.message
            );
            return (
                StatusCode::INSUFFICIENT_STORAGE,
                Json(serde_json::json!({
                    "error": err.message,
                    "error_type": "QuotaExceeded"
                })),
            )
                .into_response();
        }

        // Validate chunk size if provided
        let chunk_size = request.chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE);
        if chunk_size < 1024 * 1024 {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "Chunk size must be at least 1MB"
                })),
            )
                .into_response();
        }

        let content_type = request
            .content_type
            .unwrap_or_else(|| "application/octet-stream".to_string());

        match chunked_service
            .create_session(
                auth_user.id,
                request.filename,
                request.folder_id,
                content_type,
                request.total_size,
                Some(chunk_size),
            )
            .await
        {
            Ok(response) => (StatusCode::CREATED, Json(response)).into_response(),
            Err(e) => {
                tracing::error!("Failed to create upload session: {}", e);
                AppError::internal_error(format!("Failed to create upload session: {}", e))
                    .into_response()
            }
        }
    }

    // PATCH /api/uploads/:upload_id — moved entirely to the free
    // function `upload_chunk` below so the body can be streamed
    // (axum::body::Body) instead of materialised as `Bytes` here.
    // The port-level `ChunkedUploadPort::upload_chunk` (Bytes-based)
    // remains for tests and any future caller that genuinely has the
    // bytes already in memory.

    /// HEAD /api/uploads/:upload_id - Get upload status
    ///
    /// Returns upload progress and pending chunks
    pub(super) async fn get_upload_status_impl(
        State(state): State<Arc<AppState>>,
        auth_user: AuthUser,
        Path(upload_id): Path<String>,
    ) -> impl IntoResponse {
        let chunked_service = &state.core.chunked_upload_service;

        match chunked_service.get_status(&upload_id, auth_user.id).await {
            Ok(status) => Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/json")
                .header("Upload-Offset", status.bytes_received.to_string())
                .header("Upload-Length", status.total_size.to_string())
                .header("Upload-Progress", format!("{:.2}", status.progress * 100.0))
                .header("Upload-Chunks-Total", status.total_chunks.to_string())
                .header(
                    "Upload-Chunks-Complete",
                    status.completed_chunks.to_string(),
                )
                .body(axum::body::Body::from(
                    serde_json::to_string(&status).unwrap(),
                ))
                .unwrap()
                .into_response(),
            Err(e) => AppError::from(e).into_response(),
        }
    }

    /// Compute the requested checksum of the assembled file.
    ///
    /// For `Blake3` the server already has the hash from hash-on-write
    /// assembly — we just return it (zero I/O, zero CPU). For `Md5` and
    /// `Sha256` we re-read the assembled file on the blocking pool and
    /// hash it; the cost (~30–100 ms for typical files) is the trade-off
    /// for accepting non-default algorithms.
    async fn compute_assembled_hash(
        assembled_path: &std::path::Path,
        alg: ChecksumAlg,
        blake3_already_computed: &str,
    ) -> Result<String, std::io::Error> {
        match alg {
            ChecksumAlg::Blake3 => Ok(blake3_already_computed.to_string()),
            ChecksumAlg::Md5 | ChecksumAlg::Sha256 => {
                let path = assembled_path.to_path_buf();
                tokio::task::spawn_blocking(move || -> Result<String, std::io::Error> {
                    use std::io::Read;
                    let mut file = std::fs::File::open(&path)?;
                    let mut buf = vec![0u8; 524_288];
                    match alg {
                        ChecksumAlg::Md5 => {
                            use md5::Digest as _;
                            let mut h = md5::Md5::new();
                            loop {
                                let n = file.read(&mut buf)?;
                                if n == 0 {
                                    break;
                                }
                                h.update(&buf[..n]);
                            }
                            Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
                        }
                        ChecksumAlg::Sha256 => {
                            use sha2::Digest as _;
                            let mut h = sha2::Sha256::new();
                            loop {
                                let n = file.read(&mut buf)?;
                                if n == 0 {
                                    break;
                                }
                                h.update(&buf[..n]);
                            }
                            Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
                        }
                        // Blake3 handled above — this branch is unreachable but
                        // keeps the match exhaustive without an else-clause.
                        ChecksumAlg::Blake3 => unreachable!(),
                    }
                })
                .await
                .map_err(|e| std::io::Error::other(format!("hash task join failed: {e}")))?
            }
        }
    }

    /// POST /api/uploads/:upload_id/complete - Finalize upload
    ///
    /// Assembles all chunks into the final file and creates the file record.
    /// When `body.checksum` is supplied, the assembled file's hash is
    /// verified before the blob is promoted to storage — mismatch
    /// returns 400 and the assembled temp is removed (the session
    /// itself is kept so the client can re-issue complete after
    /// diagnosing).
    pub(super) async fn complete_upload_impl(
        State(state): State<Arc<AppState>>,
        auth_user: AuthUser,
        Path(upload_id): Path<String>,
        body: CompleteUploadRequest,
    ) -> impl IntoResponse {
        let chunked_service = &state.core.chunked_upload_service;
        let upload_service = &state.applications.file_upload_service;

        // ── Parse the optional algorithm BEFORE assembly so a bad
        //    `checksumalg` doesn't waste the (potentially expensive)
        //    hash work on a request we'll reject anyway.
        let alg = match body.checksumalg.as_deref() {
            Some(name) => match ChecksumAlg::parse(name) {
                Some(a) => Some(a),
                None => {
                    return AppError::bad_request(format!(
                        "Unsupported checksumalg: {name} (supported: md5, sha256, blake3)"
                    ))
                    .into_response();
                }
            },
            None => None,
        };
        let expected_checksum = body.checksum.as_deref();

        // Assemble chunks (hash-on-write: BLAKE3 computed during assembly)
        let (assembled_path, filename, folder_id, content_type, total_size, hash) =
            match chunked_service
                .complete_upload(&upload_id, auth_user.id)
                .await
            {
                Ok(result) => result,
                Err(e) => {
                    return AppError::from(e).into_response();
                }
            };

        // ── End-to-end integrity verification ───────────────────────
        // Only fires when the client supplied an `expected` checksum.
        // For BLAKE3 (the documented preferred choice) this is a string
        // comparison against the hash assembly already produced. For
        // MD5/SHA-256 we re-hash the assembled file on the blocking pool.
        if let Some(expected) = expected_checksum {
            let alg = alg.unwrap_or(ChecksumAlg::Blake3);
            let computed = match Self::compute_assembled_hash(&assembled_path, alg, &hash).await {
                Ok(c) => c,
                Err(e) => {
                    let _ = tokio::fs::remove_file(&assembled_path).await;
                    return AppError::internal_error(format!(
                        "Failed to compute assembled checksum: {e}"
                    ))
                    .into_response();
                }
            };
            if !computed.eq_ignore_ascii_case(expected) {
                let _ = tokio::fs::remove_file(&assembled_path).await;
                tracing::warn!(
                    target: "audit",
                    event = "chunked_upload.checksum_mismatch",
                    reason = "final_checksum_mismatch",
                    upload_id = %upload_id,
                    user_id = %auth_user.id,
                    alg = alg.as_str(),
                    expected = %expected,
                    actual = %computed,
                    "👮🏻‍♂️ Chunked upload complete: client checksum mismatch — blob not promoted"
                );
                return AppError::bad_request(format!(
                    "Checksum mismatch ({}): expected {}, got {}",
                    alg.as_str(),
                    expected,
                    computed
                ))
                .into_response();
            }
        }

        // ── MIME detection (magic bytes + extension fallback) ─────
        let content_type = crate::common::mime_detect::refine_content_type_from_file(
            &assembled_path,
            &filename,
            &content_type,
        )
        .await;

        // Upload from assembled file on disk — zero extra RAM copies, hash pre-computed
        match upload_service
            .upload_file_from_path(
                filename.clone(),
                folder_id.clone(),
                content_type,
                &assembled_path,
                Some(hash),
            )
            .await
        {
            Ok(file) => {
                // Cleanup session
                let _ = chunked_service
                    .finalize_upload(&upload_id, auth_user.id)
                    .await;

                tracing::info!(
                    "✅ CHUNKED UPLOAD COMPLETE: {} (ID: {}, {} bytes)",
                    filename,
                    file.id,
                    total_size
                );

                (
                    StatusCode::CREATED,
                    Json(CompleteUploadResponse {
                        file_id: file.id,
                        filename: file.name,
                        size: total_size,
                        path: file.path,
                    }),
                )
                    .into_response()
            }
            Err(e) => {
                tracing::error!("Failed to create file from assembled upload: {:?}", e);
                AppError::internal_error(format!("Failed to create file: {}", e)).into_response()
            }
        }
    }

    /// DELETE /api/uploads/:upload_id - Cancel upload
    ///
    /// Cancels an in-progress upload and cleans up temp files
    pub(super) async fn cancel_upload_impl(
        State(state): State<Arc<AppState>>,
        auth_user: AuthUser,
        Path(upload_id): Path<String>,
    ) -> impl IntoResponse {
        let chunked_service = &state.core.chunked_upload_service;

        match chunked_service
            .cancel_upload(&upload_id, auth_user.id)
            .await
        {
            Ok(_) => StatusCode::NO_CONTENT.into_response(),
            Err(e) => AppError::from(e).into_response(),
        }
    }
}

// ── Route handlers (free functions) ──────────────────────────────────────────
//
// All five route functions live here rather than as methods on ChunkedUploadHandler
// because utoipa 5.4.0's #[utoipa::path] macro generates helper structs inside its
// expansion. Rust allows struct definitions at module scope but forbids them inside
// impl blocks — so every #[utoipa::path] annotation on a ChunkedUploadHandler method
// fails to compile regardless of HTTP verb or annotation content.
//
// FileHandler and FolderHandler are not affected (root cause in utoipa unknown, likely
// a 5.4.x regression). All logic lives in the ChunkedUploadHandler::*_impl methods
// above; these thin wrappers exist solely to carry the OpenAPI annotation at a scope
// where utoipa can generate its helper types.
//
// routes.rs calls these free functions directly.
// TODO: collapse back into the impl block after a utoipa upgrade resolves the issue.

#[utoipa::path(
    post,
    path = "/api/uploads",
    request_body(content = CreateUploadRequest, content_type = "application/json", description = "Upload session parameters"),
    responses(
        (status = 201, description = "Upload session created", body = crate::application::ports::chunked_upload_ports::CreateUploadResponseDto),
        (status = 400, description = "Invalid request (empty filename, zero size, chunk too small)"),
        (status = 507, description = "Storage quota exceeded"),
    ),
    tag = "uploads",
    security(("bearerAuth" = []))
)]
pub async fn create_upload(
    state: State<Arc<AppState>>,
    auth_user: AuthUser,
    request: Json<CreateUploadRequest>,
) -> impl IntoResponse {
    ChunkedUploadHandler::create_upload_impl(state, auth_user, request).await
}

#[utoipa::path(
    patch,
    path = "/api/uploads/{upload_id}",
    params(
        ("upload_id" = String, Path, description = "Upload session ID"),
        ("chunk_index" = usize, Query, description = "Zero-based chunk index"),
        (
            "checksum" = Option<String>,
            Query,
            description = "Optional hex-encoded checksum for integrity verification. \
                Computed incrementally during the streaming write. \
                Algorithm is selected by `checksumalg` (default `md5`). \
                Also accepted via the legacy `Content-MD5` request header."
        ),
        (
            "checksumalg" = Option<String>,
            Query,
            description = "Algorithm used by `checksum`. One of: `md5` (default, legacy), `sha256` / `sha-256`, `blake3`. \
                Unknown values return 400."
        ),
    ),
    request_body(content_type = "application/octet-stream", description = "Raw chunk bytes"),
    responses(
        (status = 200, description = "Chunk received", body = crate::application::ports::chunked_upload_ports::ChunkUploadResponseDto),
        (status = 400, description = "Invalid chunk, size mismatch, checksum mismatch, or unknown `checksumalg`"),
        (status = 404, description = "Upload session not found"),
        (status = 413, description = "Chunk exceeds `storage.chunk_max_bytes` cap"),
    ),
    tag = "uploads",
    security(("bearerAuth" = []))
)]
pub async fn upload_chunk(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path(upload_id): Path<String>,
    Query(params): Query<ChunkUploadParams>,
    headers: HeaderMap,
    request: Request,
) -> impl IntoResponse {
    let chunked_service = &state.core.chunked_upload_service;
    let max_chunk = state.core.config.storage.chunk_max_bytes;

    // ── Resolve the client's checksum + algorithm ────────────────────
    // Wire shape: `?checksum=<hex>&checksumalg=<name>` (or `Content-MD5`
    // header for older clients). When `checksumalg` is omitted we
    // default to MD5, matching the legacy contract — switching the
    // default would silently break any client still relying on
    // `Content-MD5` semantics.
    let expected_checksum = params.checksum.clone().or_else(|| {
        headers
            .get("Content-MD5")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    });
    let alg = match params.checksumalg.as_deref() {
        Some(name) => match ChecksumAlg::parse(name) {
            Some(a) => a,
            None => {
                return AppError::bad_request(format!(
                    "Unsupported checksumalg: {name} (supported: md5, sha256, blake3)"
                ))
                .into_response();
            }
        },
        None => ChecksumAlg::Md5,
    };
    // Only compute the hash when the client supplied an `expected_checksum`
    // to verify against — saves ~30 ms per chunk for clients that don't.
    let alg_to_compute = expected_checksum.as_ref().map(|_| alg);

    // ── Phase 1: prepare ─────────────────────────────────────────────
    // Validates session ownership + chunk index, returns the on-disk
    // path and the chunk's declared size. The handler streams the body
    // to that path; service finalises bookkeeping after the write.
    let (chunk_path, _expected_size) = match chunked_service
        .prepare_chunk(&upload_id, auth_user.id, params.chunk_index)
        .await
    {
        Ok(p) => p,
        Err(e) => return AppError::from(e).into_response(),
    };

    // ── Phase 2: stream the body straight to disk ────────────────────
    // Peak heap ~one HTTP frame (~64 KB) regardless of chunk size or
    // `chunk_max_bytes`. Optional incremental hashing happens here so
    // verification doesn't require reading the chunk file back.
    let streamed = match stream_body_to_path(
        request.into_body(),
        &chunk_path,
        max_chunk,
        alg_to_compute,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                error = ?e,
                upload_id = %upload_id,
                chunk_index = params.chunk_index,
                max_chunk,
                "Chunked upload PATCH rejected — streaming write failed (cap, transport, or IO)"
            );
            return e.into_response();
        }
    };

    // ── Phase 3: commit ──────────────────────────────────────────────
    // Size + checksum verification + session state update. Same RAM-only
    // DashMap shard ownership pattern as the legacy `upload_chunk_inner`
    // (held only for ~µs; bitmask persist done after release).
    let response = match chunked_service
        .commit_chunk(
            &upload_id,
            auth_user.id,
            params.chunk_index,
            streamed.bytes_written,
            streamed.checksum_hex,
            expected_checksum,
        )
        .await
    {
        Ok(r) => r,
        Err(e) => return AppError::from(e).into_response(),
    };

    let mut resp = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header("Upload-Offset", response.bytes_received.to_string())
        .header(
            "Upload-Progress",
            format!("{:.2}", response.progress * 100.0),
        );
    if response.is_complete {
        resp = resp.header("Upload-Complete", "true");
    }
    resp.body(axum::body::Body::from(
        serde_json::to_string(&response).unwrap(),
    ))
    .unwrap()
    .into_response()
}

#[utoipa::path(
    head,
    path = "/api/uploads/{upload_id}",
    params(
        ("upload_id" = String, Path, description = "Upload session ID"),
    ),
    responses(
        (status = 200, description = "Upload status in response headers and body", body = crate::application::ports::chunked_upload_ports::UploadStatusResponseDto),
        (status = 404, description = "Upload session not found"),
    ),
    tag = "uploads",
    security(("bearerAuth" = []))
)]
pub async fn get_upload_status(
    state: State<Arc<AppState>>,
    auth_user: AuthUser,
    path: Path<String>,
) -> impl IntoResponse {
    ChunkedUploadHandler::get_upload_status_impl(state, auth_user, path).await
}

#[utoipa::path(
    post,
    path = "/api/uploads/{upload_id}/complete",
    params(
        ("upload_id" = String, Path, description = "Upload session ID"),
    ),
    request_body(
        content = CompleteUploadRequest,
        content_type = "application/json",
        description = "Optional. End-to-end integrity verification of the assembled file. \
            **`blake3` is highly recommended** as the `checksumalg` value — the server already \
            computes BLAKE3 over the assembled file during hash-on-write assembly, so \
            verification is a string comparison with zero extra CPU/IO. \
            Picking `md5` or `sha256` is supported for legacy client tooling but triggers a \
            second full hash pass over the assembled file. \
            Clients that POST with no body (or with an empty JSON object) get today's \
            behavior: no verification, server returns the BLAKE3 it computed."
    ),
    responses(
        (status = 201, description = "File assembled and created", body = CompleteUploadResponse),
        (status = 400, description = "Unknown `checksumalg` or final-checksum mismatch"),
        (status = 404, description = "Upload session not found"),
        (status = 500, description = "Assembly, hashing, or file creation failed"),
    ),
    tag = "uploads",
    security(("bearerAuth" = []))
)]
pub async fn complete_upload(
    state: State<Arc<AppState>>,
    auth_user: AuthUser,
    path: Path<String>,
    // Empty body → `None` → default `CompleteUploadRequest`, preserving the
    // pre-checksum wire shape. Clients that DO send a body get strict
    // parsing (a malformed JSON returns 400 via the Json extractor).
    body: Option<Json<CompleteUploadRequest>>,
) -> impl IntoResponse {
    let req = body.map(|Json(r)| r).unwrap_or_default();
    ChunkedUploadHandler::complete_upload_impl(state, auth_user, path, req).await
}

#[utoipa::path(
    delete,
    path = "/api/uploads/{upload_id}",
    params(
        ("upload_id" = String, Path, description = "Upload session ID"),
    ),
    responses(
        (status = 204, description = "Upload cancelled and temp files cleaned up"),
        (status = 500, description = "Cancel failed"),
    ),
    tag = "uploads",
    security(("bearerAuth" = []))
)]
pub async fn cancel_upload(
    state: State<Arc<AppState>>,
    auth_user: AuthUser,
    path: Path<String>,
) -> impl IntoResponse {
    ChunkedUploadHandler::cancel_upload_impl(state, auth_user, path).await
}
