use axum::{
    body::Body,
    http::{Request, StatusCode, header},
    response::Response,
};
use std::sync::Arc;

use crate::application::ports::file_ports::{FileRetrievalUseCase, FileUploadUseCase};
use crate::common::di::AppState;
use crate::common::mime_detect::filename_from_path;
use crate::interfaces::errors::AppError;
use crate::interfaces::middleware::auth::{AuthUser, CurrentUser};
use crate::interfaces::upload_ingest::{
    discard_ingested, ingest_stream_to_cas, stream_body_to_path, stream_from_files,
};

/// Dispatch Nextcloud chunked upload WebDAV requests.
///
/// Routes:
///   MKCOL    /remote.php/dav/uploads/{user}/{upload_id}             → create session
///   PUT      /remote.php/dav/uploads/{user}/{upload_id}/{chunk}     → store chunk
///   MOVE     /remote.php/dav/uploads/{user}/{upload_id}/.file       → assemble
///   DELETE   /remote.php/dav/uploads/{user}/{upload_id}             → abort
///   PROPFIND /remote.php/dav/uploads/{user}/{upload_id}             → list chunks (for resume)
pub async fn handle_nc_uploads(
    state: Arc<AppState>,
    req: Request<Body>,
    user: AuthUser,
    upload_id: String,
    rest: String, // chunk name or ".file" or empty
) -> Result<Response<Body>, AppError> {
    let method = req.method().clone();
    match method.as_str() {
        "MKCOL" => handle_mkcol(state, &user, &upload_id).await,
        "PUT" => handle_put_chunk(state, req, &user, &upload_id, &rest).await,
        "MOVE" => handle_assemble(state, req, &user, &upload_id).await,
        "DELETE" => handle_abort(state, &user, &upload_id).await,
        "PROPFIND" => handle_propfind_session(state, &user, &upload_id).await,
        _ => Ok(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body(Body::empty())
            .unwrap()),
    }
}

/// PROPFIND on an upload session — used by the NextCloud Android
/// client (and several mobile clients) to enumerate which chunks
/// are already uploaded before resuming an interrupted transfer.
/// Without this handler the client gets `405 METHOD_NOT_ALLOWED`
/// and falls back to either failing the upload or starting from
/// scratch — neither is acceptable on cellular / flaky links where
/// resume is the whole point of chunked upload.
///
/// Response shape: 207 Multi-Status with one `<d:response>` for the
/// session collection itself and one per chunk file. Properties
/// returned are the minimum the NC client reads: `resourcetype`,
/// `getcontentlength` (chunks only), and `getlastmodified` (so
/// clients can detect stale partial uploads). Depth is ignored —
/// we always return one level (the session + its direct chunks),
/// which matches NC server behaviour.
async fn handle_propfind_session(
    state: Arc<AppState>,
    user: &CurrentUser,
    upload_id: &str,
) -> Result<Response<Body>, AppError> {
    let nc = state
        .nextcloud
        .as_ref()
        .ok_or_else(|| AppError::internal_error("Nextcloud services unavailable"))?;

    let listing = nc
        .chunked_uploads
        .list_chunks(&user.username, upload_id)
        .await
        .map_err(|e| AppError::internal_error(format!("Failed to list chunks: {}", e)))?
        .ok_or_else(|| AppError::not_found("Upload session not found"))?;

    let session_href = format!("/remote.php/dav/uploads/{}/{}/", user.username, upload_id);
    let session_last_modified =
        chrono::DateTime::<chrono::Utc>::from_timestamp(listing.session_mtime as i64, 0)
            .unwrap_or_else(chrono::Utc::now)
            .to_rfc2822();

    let mut body = String::new();
    body.push_str(r#"<?xml version="1.0" encoding="utf-8"?>"#);
    body.push_str(r#"<d:multistatus xmlns:d="DAV:">"#);

    // Session collection itself.
    body.push_str("<d:response>");
    body.push_str(&format!("<d:href>{}</d:href>", xml_escape(&session_href)));
    body.push_str("<d:propstat><d:prop>");
    body.push_str("<d:resourcetype><d:collection/></d:resourcetype>");
    body.push_str(&format!(
        "<d:getlastmodified>{}</d:getlastmodified>",
        xml_escape(&session_last_modified)
    ));
    body.push_str("</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>");
    body.push_str("</d:response>");

    // One entry per chunk file.
    for chunk in &listing.chunks {
        let chunk_href = format!(
            "/remote.php/dav/uploads/{}/{}/{}",
            user.username, upload_id, chunk.name
        );
        let chunk_modified = chrono::DateTime::<chrono::Utc>::from_timestamp(chunk.mtime as i64, 0)
            .unwrap_or_else(chrono::Utc::now)
            .to_rfc2822();

        body.push_str("<d:response>");
        body.push_str(&format!("<d:href>{}</d:href>", xml_escape(&chunk_href)));
        body.push_str("<d:propstat><d:prop>");
        body.push_str("<d:resourcetype/>");
        body.push_str(&format!(
            "<d:getcontentlength>{}</d:getcontentlength>",
            chunk.size
        ));
        body.push_str(&format!(
            "<d:getlastmodified>{}</d:getlastmodified>",
            xml_escape(&chunk_modified)
        ));
        body.push_str("</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>");
        body.push_str("</d:response>");
    }

    body.push_str("</d:multistatus>");

    Ok(Response::builder()
        .status(StatusCode::MULTI_STATUS)
        .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
        .body(Body::from(body))
        .unwrap())
}

/// Minimal XML escape — every value we inject above is either a
/// well-formed RFC 2822 date, a number, or a path segment we
/// control, but defense-in-depth keeps the response well-formed
/// even if a chunk name ever contained an unexpected character.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// MKCOL — create upload session directory.
async fn handle_mkcol(
    state: Arc<AppState>,
    user: &CurrentUser,
    upload_id: &str,
) -> Result<Response<Body>, AppError> {
    let nc = state
        .nextcloud
        .as_ref()
        .ok_or_else(|| AppError::internal_error("Nextcloud services unavailable"))?;

    nc.chunked_uploads
        .create_session(&user.username, upload_id)
        .await
        .map_err(|e| AppError::internal_error(format!("Failed to create session: {}", e)))?;

    Ok(Response::builder()
        .status(StatusCode::CREATED)
        .body(Body::empty())
        .unwrap())
}

/// PUT — store a chunk.
///
/// Streams the request body straight to the chunk file with peak heap of
/// ~one HTTP frame, regardless of chunk size or the configured cap. The
/// `storage.chunk_max_bytes` config (env `OXICLOUD_CHUNK_MAX_BYTES`,
/// default 100 MB) bounds a single PUT — separate from `max_upload_size`
/// which governs whole-file uploads. Without this separation, a client
/// could submit a chunk up to the whole-file cap (10 GB default) and
/// monopolise server memory.
async fn handle_put_chunk(
    state: Arc<AppState>,
    req: Request<Body>,
    user: &CurrentUser,
    upload_id: &str,
    chunk_name: &str,
) -> Result<Response<Body>, AppError> {
    let nc = state
        .nextcloud
        .as_ref()
        .ok_or_else(|| AppError::internal_error("Nextcloud services unavailable"))?;

    let chunk_name = chunk_name.trim_matches('/');
    if chunk_name.is_empty() {
        return Err(AppError::bad_request("Missing chunk name"));
    }

    let chunk_path = nc
        .chunked_uploads
        .safe_chunk_path(&user.username, upload_id, chunk_name)
        .map_err(|e| AppError::bad_request(format!("Invalid chunk path: {}", e)))?;

    let max_chunk = state.core.config.storage.chunk_max_bytes;
    // No client-side integrity contract on the NC chunked surface — the
    // NC desktop client validates the assembled-file ETag against the
    // server-side `oc:checksums` after MOVE. So we skip per-chunk
    // hashing here (peak heap stays at ~one HTTP frame).
    stream_body_to_path(req.into_body(), &chunk_path, max_chunk, None).await?;

    Ok(Response::builder()
        .status(StatusCode::CREATED)
        .body(Body::empty())
        .unwrap())
}

/// MOVE — assemble chunks into final file.
///
/// The Destination header contains the final file path in the DAV files namespace.
async fn handle_assemble(
    state: Arc<AppState>,
    req: Request<Body>,
    user: &CurrentUser,
    upload_id: &str,
) -> Result<Response<Body>, AppError> {
    let nc = state
        .nextcloud
        .as_ref()
        .ok_or_else(|| AppError::internal_error("Nextcloud services unavailable"))?;

    // Parse Destination header to determine final file path.
    let destination = req
        .headers()
        .get("destination")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::bad_request("Missing Destination header"))?
        .to_string();

    let oc_mtime = req
        .headers()
        .get("x-oc-mtime")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok());

    let dest_subpath = extract_files_subpath(&destination, &user.username)
        .ok_or_else(|| AppError::bad_request("Invalid Destination URL"))?;

    // Stream the chunk parts, in order, straight into the CDC chunk store —
    // no assembled temp file is ever written. Chunking (FastCDC), BLAKE3
    // hashing, dedup checks and MIME sniffing (magic bytes off the first
    // part) all happen in that single read pass. The parts stay on disk
    // until the session cleanup below, so a failed completion is retryable.
    let chunk_paths = nc
        .chunked_uploads
        .ordered_chunk_paths(&user.username, upload_id)
        .await
        .map_err(|e| AppError::internal_error(format!("Failed to list chunks: {}", e)))?;

    let upload_service = &state.applications.file_upload_service;
    let file_service = &state.applications.file_retrieval_service;
    let folder_service = &state.applications.folder_service;

    let internal_path = format!(
        "My Folder - {}/{}",
        user.username,
        dest_subpath.trim_matches('/')
    );

    let filename = filename_from_path(&dest_subpath).to_string();
    let ingested = ingest_stream_to_cas(
        stream_from_files(chunk_paths),
        &state.core.dedup_service,
        &filename,
        "application/octet-stream",
        usize::MAX,
        None,
    )
    .await?;
    let content_type = ingested.content_type.clone();

    // Check if file exists (update vs create).
    let existing = file_service.get_file_by_path(&internal_path).await;

    let etag: Option<String> = if existing.is_ok() {
        let dto = upload_service
            .update_file_streaming(&internal_path, ingested.stored(), &content_type, oc_mtime)
            .await
            .map_err(|e| AppError::internal_error(format!("Failed to update file: {}", e)))?;

        Some(dto.etag)
    } else {
        // New-file branch: resolve the parent folder by path and register
        // the file row against the already-ingested blob.
        let (parent_sub, filename) = match dest_subpath.rsplit_once('/') {
            Some((p, n)) => (p, n),
            None => ("", dest_subpath.as_str()),
        };
        let parent_internal = format!(
            "My Folder - {}/{}",
            user.username,
            parent_sub.trim_matches('/')
        );
        let parent_internal = parent_internal.trim_end_matches('/');

        use crate::application::ports::folder_ports::FolderUseCase;
        let parent_folder = match folder_service.get_folder_by_path(parent_internal).await {
            Ok(folder) => folder,
            Err(e) => {
                discard_ingested(&state.core.dedup_service, &ingested).await;
                return Err(AppError::internal_error(format!(
                    "Parent folder lookup failed: {}",
                    e
                )));
            }
        };

        let dto = upload_service
            .upload_file_streaming(
                filename.to_string(),
                Some(parent_folder.id),
                content_type.to_string(),
                ingested.stored(),
            )
            .await
            .map_err(|e| AppError::internal_error(format!("Failed to create file: {}", e)))?;

        Some(dto.etag)
    };

    // Cleanup session.
    let _ = nc.chunked_uploads.cleanup(&user.username, upload_id).await;

    if let Some(tag) = etag {
        return Ok(Response::builder()
            .status(StatusCode::CREATED)
            .header(header::ETAG, format!("\"{}\"", tag))
            .header("oc-etag", format!("\"{}\"", tag))
            .body(Body::empty())
            .unwrap());
    }

    Ok(Response::builder()
        .status(StatusCode::CREATED)
        .body(Body::empty())
        .unwrap())
}

/// DELETE — abort an upload session.
async fn handle_abort(
    state: Arc<AppState>,
    user: &CurrentUser,
    upload_id: &str,
) -> Result<Response<Body>, AppError> {
    let nc = state
        .nextcloud
        .as_ref()
        .ok_or_else(|| AppError::internal_error("Nextcloud services unavailable"))?;

    nc.chunked_uploads
        .cleanup(&user.username, upload_id)
        .await
        .map_err(|e| AppError::internal_error(format!("Failed to abort upload: {}", e)))?;

    Ok(Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Body::empty())
        .unwrap())
}

/// Extract the file subpath from a Destination header pointing to the files DAV namespace.
///
/// For full URLs the host is ignored — only the path component is used.
fn extract_files_subpath(dest: &str, username: &str) -> Option<String> {
    let prefix = format!("/remote.php/dav/files/{}/", username);
    let path = if dest.starts_with("http://") || dest.starts_with("https://") {
        let after_scheme = dest.split_once("://")?.1;
        let path_start = after_scheme.find('/').unwrap_or(after_scheme.len());
        &after_scheme[path_start..]
    } else {
        dest
    };
    let decoded = urlencoding::decode(path).ok()?;
    let decoded = decoded.trim_end_matches('/');
    decoded
        .strip_prefix(prefix.trim_end_matches('/'))
        .map(|s| s.trim_start_matches('/').to_string())
}
