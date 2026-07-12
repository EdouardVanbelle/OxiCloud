use axum::{
    body::Body,
    http::{Request, StatusCode, header},
    response::Response,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::application::ports::file_ports::{FileRetrievalUseCase, FileUploadUseCase};
use crate::application::ports::storage_ports::StorageUsagePort;
use crate::common::di::AppState;
use crate::common::mime_detect::filename_from_path;
use crate::interfaces::errors::AppError;
use crate::interfaces::upload_ingest::{
    discard_ingested, ingest_stream_to_cas, stream_body_to_path, stream_from_files,
};

/// Per-chunk quota gate (D4 / project_drive_quota_timing).
///
/// Pre-D4 the NC chunked path never declared a total size up front, so
/// quota only fired at the final MOVE — meaning a client could waste GB
/// of upload bandwidth before learning it was over. The drive is known
/// from the session's chroot and the user is on the session, so we can
/// gate at every wire moment now:
///
///   - MKCOL: refuse if either the drive or the user envelope is
///     already at quota (call with `additional = 0`).
///   - PUT  : refuse if `used + already_uploaded_for_session +
///     content-length` would breach either cap.
///     `already_uploaded_for_session` is the sum of chunk sizes the
///     session already holds on disk.
///   - MOVE : defence in depth via `file_upload_service`'s own gates.
///
/// Both checks run because the two caps cover different cases:
/// `check_drive_quota` is the per-drive `drives.quota_bytes` cap
/// (shared drives carry a value; personal drives are `NULL` and
/// short-circuit to OK). `check_storage_quota` is the user envelope
/// `users.storage_quota_bytes` that caps the SUM across the caller's
/// personal drives (shared-drive uploads short-circuit because the
/// envelope only sums personal drives — see
/// `project_user_envelope_quota_model`). Mirrors what every other
/// upload entry point (multipart, native chunked, delta, instant)
/// already does.
async fn refuse_if_over_quota(
    state: &AppState,
    user_id: Uuid,
    drive_id: Uuid,
    additional: u64,
) -> Result<(), AppError> {
    let Some(svc) = state.storage_usage_service.as_ref() else {
        // Quota tracking disabled in this config; MOVE-time gate
        // remains authoritative.
        return Ok(());
    };
    svc.check_storage_quota(user_id, additional)
        .await
        .map_err(AppError::from)?;
    svc.check_drive_quota(drive_id, additional)
        .await
        .map_err(AppError::from)
}

/// Sum of bytes already accepted into a chunked-upload session.
///
/// Reads the session directory once via `list_chunks` and totals every
/// chunk's on-disk size. O(N) stat calls per check, but N is the chunk
/// count (NC clients use 10 MB chunks by default — a 10 GB upload sits
/// around 1000 entries; PUT throughput dominates the cost). A
/// per-session counter file would amortise it to O(1) but adds a
/// separate write-and-sync path with its own crash semantics — defer
/// until profiling actually demands it.
async fn session_bytes_so_far(
    nc: &crate::common::di::NextcloudServices,
    username: &str,
    upload_id: &str,
) -> Result<u64, AppError> {
    let listing = nc
        .chunked_uploads
        .list_chunks(username, upload_id)
        .await
        .map_err(|e| AppError::internal_error(format!("Failed to list chunks: {}", e)))?;
    let Some(listing) = listing else {
        // Missing session — handler maps this elsewhere; treat as zero
        // here so the gate doesn't fire spuriously on the very first
        // chunk after MKCOL (race-tolerant).
        return Ok(0);
    };
    Ok(listing.chunks.iter().map(|c| c.size).sum())
}

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
    session: crate::interfaces::nextcloud::session::NcSession,
    upload_id: String,
    rest: String, // chunk name or ".file" or empty
) -> Result<Response<Body>, AppError> {
    let method = req.method().clone();
    match method.as_str() {
        "MKCOL" => handle_mkcol(state, &session, &upload_id).await,
        "PUT" => handle_put_chunk(state, req, &session, &upload_id, &rest).await,
        "MOVE" => handle_assemble(state, req, &session, &upload_id).await,
        "DELETE" => handle_abort(state, &session, &upload_id).await,
        "PROPFIND" => handle_propfind_session(state, &session, &upload_id).await,
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
    session: &crate::interfaces::nextcloud::session::NcSession,
    upload_id: &str,
) -> Result<Response<Body>, AppError> {
    let user = &session.user;
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

    // Href MUST use `session.raw_username` (composite `admin~<uuid>` on
    // non-home drives), NOT `user.username` (bare `admin`). The
    // `NcSession` extractor cross-checks the URL `{user}` segment
    // against `raw_username` and 403s on mismatch — a composite-cred
    // client that PROPFINDs, then MOVEs a chunk href back to us, would
    // otherwise 403 at the extractor before any handler runs. Same
    // fix shape as `trashbin_handler::handle_propfind` and
    // `handle_assemble`'s destination-URL parsing. Storage-side keying
    // stays on `user.username` — upload sessions are per-user, not
    // per-drive.
    let session_href = format!(
        "/remote.php/dav/uploads/{}/{}/",
        session.raw_username, upload_id
    );
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
            session.raw_username, upload_id, chunk.name
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
///
/// Quota gate (D4): refuse 507 if the bound drive is already at quota,
/// before allocating the session directory. The chunked path doesn't
/// declare a total size up front — `additional = 0` so the gate only
/// fires when the drive is already exactly full (or beyond, after a
/// burst of concurrent writes). Subsequent PUTs run the proper
/// "used + session_so_far + chunk" projection.
async fn handle_mkcol(
    state: Arc<AppState>,
    session: &crate::interfaces::nextcloud::session::NcSession,
    upload_id: &str,
) -> Result<Response<Body>, AppError> {
    let user = &session.user;
    let nc = state
        .nextcloud
        .as_ref()
        .ok_or_else(|| AppError::internal_error("Nextcloud services unavailable"))?;

    let chroot = session.require_chroot()?;
    refuse_if_over_quota(&state, user.id, chroot.drive_id, 0).await?;

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
    session: &crate::interfaces::nextcloud::session::NcSession,
    upload_id: &str,
    chunk_name: &str,
) -> Result<Response<Body>, AppError> {
    let user = &session.user;
    let nc = state
        .nextcloud
        .as_ref()
        .ok_or_else(|| AppError::internal_error("Nextcloud services unavailable"))?;

    let chunk_name = chunk_name.trim_matches('/');
    if chunk_name.is_empty() {
        return Err(AppError::bad_request("Missing chunk name"));
    }

    // Per-chunk quota gate (D4): refuse 507 BEFORE accepting body
    // bytes when `drive.used_bytes + session_so_far + chunk_size`
    // would cross the drive cap. Closes the wasted-bandwidth wart
    // where over-quota clients only learned at MOVE.
    //
    // Without a Content-Length we can't project ahead — fall back to
    // the assemble-time check. NC desktop / Android / iOS clients
    // always send CL on PUT chunks (they read the chunk file into a
    // length-known body), so this branch is rare in practice.
    let chroot = session.require_chroot()?;
    if let Some(chunk_size) = content_length_from(&req) {
        let so_far = session_bytes_so_far(nc, &user.username, upload_id).await?;
        let projected = so_far.saturating_add(chunk_size);
        refuse_if_over_quota(&state, user.id, chroot.drive_id, projected).await?;
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
    session: &crate::interfaces::nextcloud::session::NcSession,
    upload_id: &str,
) -> Result<Response<Body>, AppError> {
    let user = &session.user;
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

    // Strip the destination URL prefix using the SESSION's raw username
    // (`admin~<drive-uuid>` on non-home drives), NOT `user.username`
    // (bare `admin`). NC clients send `Destination: /remote.php/dav/files/
    // {raw_username}/…` — the URL user-segment mirrors the credential
    // they authenticated with. Passing bare `admin` here strips only
    // `admin/` from a `admin~<uuid>/…` destination, leaving the tilde
    // marker glued to the leading path segment; the write then targets
    // `<drive-root>/~<uuid>/…` and fails with a parent-folder lookup
    // error. Matches `webdav_handler::handle_move`'s call to
    // `extract_nc_subpath_from_dest(&destination, url_user)` where
    // `url_user = &session.raw_username` (webdav_handler.rs:1177).
    let dest_subpath = extract_files_subpath(&destination, &session.raw_username)
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

    // Path-based lookups below scope by `drive_id`. The NC session's
    // chroot is always populated for path-scoped handlers (see
    // `NcSession::require_chroot`); the FolderDto carries `drive_id`
    // post-D0.
    let chroot = session.require_chroot()?;
    let drive_id = chroot.drive_id;

    // Route through `nc_to_internal_path(chroot, …)` so the write
    // lands under the caller's actual default-drive root (not the
    // literal "Personal" folder). Post-D3 chroot resolution puts the
    // correct FolderDto — including the drive's real root name — on
    // the NcSession; secondary drives with SQL-provisioned sibling
    // root names now work.
    let internal_path =
        crate::interfaces::nextcloud::webdav_handler::nc_to_internal_path(chroot, &dest_subpath)?;

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
    let existing = file_service
        .get_file_by_path(&internal_path, drive_id)
        .await;

    let etag: Option<String> = if existing.is_ok() {
        let dto = upload_service
            .update_file_streaming_with_perms(
                &internal_path,
                drive_id,
                ingested.stored(),
                &content_type,
                oc_mtime,
                user.id,
            )
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
        let parent_internal =
            crate::interfaces::nextcloud::webdav_handler::nc_to_internal_path(chroot, parent_sub)?;
        let parent_internal = parent_internal.trim_end_matches('/');

        use crate::application::ports::folder_ports::FolderUseCase;
        let parent_folder = match folder_service
            .get_folder_by_path(parent_internal, drive_id)
            .await
        {
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
                user.id,
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
    session: &crate::interfaces::nextcloud::session::NcSession,
    upload_id: &str,
) -> Result<Response<Body>, AppError> {
    let user = &session.user;
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

/// Read `Content-Length` off a request as a `u64`. Returns `None` if
/// the header is absent or malformed — the PUT-chunk quota gate
/// (`handle_put_chunk`) treats that as "skip the early gate, the
/// stream cap + MOVE-time check will still catch over-quota writes".
fn content_length_from(req: &Request<Body>) -> Option<u64> {
    req.headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
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
