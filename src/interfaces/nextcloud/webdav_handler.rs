use axum::{
    body::{self, Body},
    http::{HeaderName, Request, StatusCode, header},
    response::Response,
};
use bytes::{Buf, Bytes};
use chrono::Utc;
use quick_xml::{
    Writer,
    events::{BytesEnd, BytesStart, BytesText, Event},
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use uuid::Uuid;

use crate::application::adapters::webdav_adapter::{PropFindRequest, WebDavAdapter};
use crate::application::dtos::pagination::PaginationRequestDto;
use crate::application::ports::favorites_ports::FavoritesUseCase;
use crate::application::ports::file_ports::{
    FileManagementUseCase, FileRetrievalUseCase, FileUploadUseCase,
};
use crate::application::ports::folder_ports::FolderUseCase;
use crate::application::ports::trash_ports::TrashUseCase;
use crate::common::di::AppState;
use crate::common::mime_detect::{filename_from_path, refine_content_type_from_file};
use crate::interfaces::api::handlers::webdav_handler::PROPFIND_BATCH_SIZE;
use crate::interfaces::errors::AppError;
use crate::interfaces::middleware::auth::{AuthUser, CurrentUser};
use crate::interfaces::range_requests::{not_modified_response, range_response};
use crate::interfaces::upload_spool::spool_body_to_temp;

/// Extension trait to map XML write errors to `String` concisely.
trait XmlResultExt<T> {
    fn xml_err(self) -> Result<T, String>;
}

impl<T, E: std::fmt::Display> XmlResultExt<T> for Result<T, E> {
    fn xml_err(self) -> Result<T, String> {
        self.map_err(|e| e.to_string())
    }
}

/// Convert a `u64` timestamp to `i64` safely, falling back to 0 on overflow.
fn timestamp_to_i64(ts: u64) -> i64 {
    i64::try_from(ts).unwrap_or(0)
}

const HEADER_DAV: HeaderName = HeaderName::from_static("dav");

/// Resolve the internal OxiCloud path from a Nextcloud DAV subpath.
///
/// Nextcloud: /remote.php/dav/files/{user}/{subpath}
/// Internal:  My Folder - {username}/{subpath}
///
/// An empty subpath maps to the user's home folder root.
pub fn nc_to_internal_path(username: &str, subpath: &str) -> Result<String, AppError> {
    let home = format!("My Folder - {}", username);
    let subpath = subpath.trim_matches('/');
    if subpath.is_empty() {
        return Ok(home);
    }
    // Reject path traversal attempts.
    if subpath.split('/').any(|seg| seg == ".." || seg == ".") {
        return Err(AppError::bad_request("Invalid path: traversal not allowed"));
    }
    Ok(format!("{}/{}", home, subpath))
}

/// Build the Nextcloud DAV href for a **collection** (folder). Always
/// terminates with `/` — RFC 4918 §5.2 requires collection URLs to end
/// in a slash, and the Nextcloud desktop client strictly enforces this
/// for the "own entry" href in PROPFIND multi-status responses: a
/// PROPFIND on `/remote.php/dav/files/admin/ext/` whose first response
/// `<d:href>` doesn't end in `/` aborts the parse with
/// `Invalid href "<…>" expected starting with "<requested-url>"` and
/// surfaces as `Network request error "Erreur inconnue" HTTP status
/// 207` in the client log. Files use [`nc_href`] (no trailing slash).
pub fn nc_collection_href(username: &str, subpath: &str) -> String {
    let h = nc_href(username, subpath);
    if h.ends_with('/') {
        h
    } else {
        format!("{}/", h)
    }
}

/// Build the Nextcloud DAV href for a resource.
///
/// Each path segment is URL-encoded individually so filenames with spaces,
/// `#`, `%`, or non-ASCII characters produce valid PROPFIND hrefs.
///
/// Returns NO trailing slash for non-empty subpaths. Callers rendering
/// a **collection** must use [`nc_collection_href`] (or append `/`
/// manually) to satisfy RFC 4918 §5.2 and the NC client's parser.
pub fn nc_href(username: &str, subpath: &str) -> String {
    let subpath = subpath.trim_matches('/');
    let encoded_user = urlencoding::encode(username);
    if subpath.is_empty() {
        format!("/remote.php/dav/files/{}/", encoded_user)
    } else {
        let encoded_segments: Vec<_> = subpath
            .split('/')
            .map(|seg| urlencoding::encode(seg))
            .collect();
        format!(
            "/remote.php/dav/files/{}/{}",
            encoded_user,
            encoded_segments.join("/")
        )
    }
}

/// Dispatch Nextcloud WebDAV request to the appropriate handler.
///
/// `subpath` is everything after `/remote.php/dav/files/{user}/`.
pub async fn handle_nc_webdav(
    state: Arc<AppState>,
    req: Request<Body>,
    user: AuthUser,
    subpath: String,
) -> Result<Response<Body>, AppError> {
    let method = req.method().clone();
    match method.as_str() {
        "OPTIONS" => handle_options(),
        "GET" => handle_get(state, &user, &subpath, req.headers()).await,
        "PROPFIND" => handle_propfind(state, req, &user, &subpath).await,
        "PUT" => handle_put(state, req, &user, &subpath).await,
        "MKCOL" => handle_mkcol(state, &user, &subpath).await,
        "DELETE" => handle_delete(state, &user, &subpath).await,
        "MOVE" => handle_move(state, req, &user, &subpath).await,
        "HEAD" => handle_head(state, &user, &subpath).await,
        "PROPPATCH" => handle_proppatch(state, req, &user, &subpath).await,
        "REPORT" | "SEARCH" => {
            crate::interfaces::nextcloud::report_handler::handle_nc_report(
                state, req, &user, &subpath,
            )
            .await
        }
        _ => Ok(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body(Body::empty())
            .unwrap()),
    }
}

// ──────────────────── OPTIONS ────────────────────

fn handle_options() -> Result<Response<Body>, AppError> {
    // Advertise WebDAV compliance classes 1 + 3 only.
    // Class 2 (LOCK/UNLOCK) is intentionally omitted because the NC
    // surface has no LOCK/UNLOCK dispatch arm — claiming class 2
    // would invite clients (notably the NC desktop sync engine) to
    // start sending LOCK requests we then 405. Class 3 covers the
    // weak-resource-validators behaviour PROPFIND already implements.
    // If LOCK is ever wired in here, restore "1, 2, 3" in the same
    // commit as the LOCK arm — never split the advertisement from
    // the implementation.
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(HEADER_DAV, "1, 3")
        .header(
            header::ALLOW,
            "OPTIONS, GET, HEAD, PUT, DELETE, MKCOL, MOVE, PROPFIND, PROPPATCH, REPORT, SEARCH",
        )
        .body(Body::empty())
        .unwrap())
}

// ──────────────────── PROPFIND ────────────────────

async fn handle_propfind(
    state: Arc<AppState>,
    req: Request<Body>,
    user: &CurrentUser,
    subpath: &str,
) -> Result<Response<Body>, AppError> {
    let depth = req
        .headers()
        .get("depth")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("1")
        .to_string();

    // Parse the PROPFIND XML body (or assume allprop if empty).
    let body_bytes = body::to_bytes(req.into_body(), 64 * 1024)
        .await
        .map_err(|e| AppError::bad_request(format!("Failed to read body: {}", e)))?;

    // Parse (and thereby validate) the PROPFIND body. The NC response
    // always emits the full property set, so the parsed request is not
    // consulted further — but malformed XML must still fail with 400.
    let _propfind = if body_bytes.is_empty() {
        PropFindRequest {
            prop_find_type: crate::application::adapters::webdav_adapter::PropFindType::AllProp,
        }
    } else {
        WebDavAdapter::parse_propfind(body_bytes.reader())
            .map_err(|e| AppError::bad_request(format!("Invalid PROPFIND XML: {}", e)))?
    };

    let internal_path = nc_to_internal_path(&user.username, subpath)?;
    let folder_service = &state.applications.folder_service;
    let file_service = &state.applications.file_retrieval_service;

    // Try to resolve as folder first.
    let folder_result = folder_service.get_folder_by_path(&internal_path).await;

    if let Ok(folder) = folder_result {
        // It's a folder — stream the multistatus: children are fetched in
        // pages and serialized chunk by chunk, so memory stays O(batch)
        // regardless of how many entries the folder holds.
        return Ok(build_nc_streaming_propfind(
            state.clone(),
            folder,
            depth,
            user.id,
            user.username.clone(),
            subpath.to_string(),
        ));
    }

    // Not a folder — try as a file.
    let file_result = file_service.get_file_by_path(&internal_path).await;
    if let Ok(file) = file_result {
        // Batch-check favorites for this single file.
        let favorite_ids = if let Some(fav_svc) = state.favorites_service.as_ref() {
            let items: Vec<(&str, &str)> = vec![(&file.id, "file")];
            fav_svc
                .batch_check_favorites(user.id, &items)
                .await
                .unwrap_or_default()
        } else {
            HashSet::new()
        };

        let nc = state.nextcloud.as_ref();
        let file_id_svc = nc.map(|n| &n.file_ids);

        let mut buf = Vec::new();
        write_nc_file_multistatus(
            &mut buf,
            &file,
            &user.username,
            subpath,
            file_id_svc,
            &favorite_ids,
        )
        .await
        .map_err(|e| AppError::internal_error(format!("XML generation failed: {}", e)))?;

        return Ok(Response::builder()
            .status(StatusCode::MULTI_STATUS)
            .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
            .body(Body::from(buf))
            .unwrap());
    }

    Err(AppError::not_found("Resource not found"))
}

// ──────────────────── GET ────────────────────

async fn handle_get(
    state: Arc<AppState>,
    user: &CurrentUser,
    subpath: &str,
    headers: &axum::http::HeaderMap,
) -> Result<Response<Body>, AppError> {
    // GET on root folder — NC clients use this as an existence check
    if subpath.is_empty() || subpath == "/" {
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header("DAV", "1, 3")
            .body(Body::empty())
            .unwrap());
    }

    let internal_path = nc_to_internal_path(&user.username, subpath)?;
    let file_service = &state.applications.file_retrieval_service;
    let folder_service = &state.applications.folder_service;

    // Check if path is a folder first (NC clients use GET as existence check)
    if folder_service
        .get_folder_by_path(&internal_path)
        .await
        .is_ok()
    {
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header("DAV", "1, 3")
            .body(Body::empty())
            .unwrap());
    }

    let file = file_service
        .get_file_by_path(&internal_path)
        .await
        .map_err(|_| AppError::not_found("File not found"))?;

    // ETag comes from `FileDto::etag` (populated from `File::etag()`
    // in the `From<File>` impl) — single source of truth, so GET,
    // HEAD, PUT-response, MOVE, and PROPFIND all emit byte-identical
    // values for the same file. NC's sync engine compares cached
    // PROPFIND ETags against GET/HEAD responses; using `file.id` here
    // (a UUID) while PROPFIND emitted the blob hash made NC see
    // every file as "remotely changed" on first descent.
    let etag = format!("\"{}\"", file.etag);

    // Conditional GET — sync clients revalidating get a 304, not the body.
    if let Some(resp) = not_modified_response(headers, &etag) {
        return Ok(resp);
    }

    // Range Requests — serve 206/416 instead of the whole file on seeks.
    if let Some(resp) = range_response(headers, &file, &etag, file_service).await {
        return Ok(resp);
    }

    let stream = file_service
        .get_file_stream(&file.id)
        .await
        .map_err(|e| AppError::internal_error(format!("Failed to read file: {}", e)))?;

    let modified_at =
        chrono::DateTime::<Utc>::from_timestamp(timestamp_to_i64(file.modified_at), 0)
            .unwrap_or_else(Utc::now);

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, file.mime_type.as_ref())
        .header(header::CONTENT_LENGTH, file.size)
        .header(header::ETAG, etag)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::LAST_MODIFIED, modified_at.to_rfc2822())
        .body(Body::from_stream(std::pin::Pin::from(stream)))
        .unwrap())
}

// ──────────────────── HEAD ────────────────────

async fn handle_head(
    state: Arc<AppState>,
    user: &CurrentUser,
    subpath: &str,
) -> Result<Response<Body>, AppError> {
    // HEAD on root folder — NC clients use this as an existence check
    if subpath.is_empty() || subpath == "/" {
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header("DAV", "1, 3")
            .body(Body::empty())
            .unwrap());
    }

    let internal_path = nc_to_internal_path(&user.username, subpath)?;
    let file_service = &state.applications.file_retrieval_service;
    let folder_service = &state.applications.folder_service;

    // Check if path is a folder (NC clients use HEAD as existence check)
    if folder_service
        .get_folder_by_path(&internal_path)
        .await
        .is_ok()
    {
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header("DAV", "1, 3")
            .body(Body::empty())
            .unwrap());
    }

    let file = file_service
        .get_file_by_path(&internal_path)
        .await
        .map_err(|_| AppError::not_found("File not found"))?;

    let modified_at =
        chrono::DateTime::<Utc>::from_timestamp(timestamp_to_i64(file.modified_at), 0)
            .unwrap_or_else(Utc::now);

    // ETag comes from `FileDto::etag` — see the same comment block on
    // the GET handler. HEAD and GET must agree byte-for-byte; pulling
    // both from the same DTO field guarantees that.
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, file.mime_type.as_ref())
        .header(header::CONTENT_LENGTH, file.size)
        .header(header::ETAG, format!("\"{}\"", file.etag))
        .header(header::LAST_MODIFIED, modified_at.to_rfc2822())
        .body(Body::empty())
        .unwrap())
}

// ──────────────────── PROPPATCH ────────────────────

async fn handle_proppatch(
    state: Arc<AppState>,
    req: Request<Body>,
    user: &CurrentUser,
    subpath: &str,
) -> Result<Response<Body>, AppError> {
    let body_bytes = body::to_bytes(req.into_body(), 64 * 1024)
        .await
        .map_err(|e| AppError::bad_request(format!("Failed to read body: {}", e)))?;

    let body_str = String::from_utf8_lossy(&body_bytes);

    // Resolve the target resource once — needed for two things:
    //  1. Applying the oc:favorite mutation when the PROPPATCH body
    //     carries one (`item_type` distinguishes file vs folder rows
    //     in the favorites table).
    //  2. Picking the right `<d:href>` shape in the multi-status
    //     response: collection (folder) hrefs MUST end in `/` per
    //     RFC 4918 §5.2 — see `nc_collection_href` for the full
    //     reasoning. Without this distinction the NC desktop client
    //     parser aborted on PROPFIND; PROPPATCH would hit the same
    //     wall the moment the user favourited a folder.
    //
    // When the resource is missing we tolerate it for the no-op
    // PROPPATCH path (no favorite directive in the body) — matches
    // the prior behaviour. A PROPPATCH that *does* try to set
    // favorite on a missing resource still returns NotFound.
    let internal_path = nc_to_internal_path(&user.username, subpath)?;
    let file_service = &state.applications.file_retrieval_service;
    let folder_service = &state.applications.folder_service;
    let resource = if let Ok(file) = file_service.get_file_by_path(&internal_path).await {
        Some((file.id, "file"))
    } else if let Ok(folder) = folder_service.get_folder_by_path(&internal_path).await {
        Some((folder.id, "folder"))
    } else {
        None
    };
    let is_collection = matches!(resource, Some((_, "folder")));

    // Parse oc:favorite value from PROPPATCH XML.
    let favorite_value = parse_proppatch_favorite(&body_str);

    if let Some(value) = favorite_value {
        let Some((item_id, item_type)) = resource else {
            return Err(AppError::not_found("Resource not found"));
        };

        if let Some(fav_svc) = state.favorites_service.as_ref() {
            if value == 1 {
                fav_svc
                    .add_to_favorites(user.id, &item_id, item_type)
                    .await
                    .map_err(|e| {
                        AppError::internal_error(format!("Failed to add favorite: {}", e))
                    })?;
            } else {
                fav_svc
                    .remove_from_favorites(user.id, &item_id, item_type)
                    .await
                    .map_err(|e| {
                        AppError::internal_error(format!("Failed to remove favorite: {}", e))
                    })?;
            }
        }
    }

    // Return 207 Multi-Status with success response using quick_xml
    // for safe escaping. Collection vs file href chosen by resource
    // type to satisfy the RFC 4918 §5.2 trailing-slash invariant —
    // see the comment block at the top of this function.
    let href = if is_collection {
        nc_collection_href(&user.username, subpath)
    } else {
        nc_href(&user.username, subpath)
    };
    let mut buf = Vec::new();
    {
        let mut xml = Writer::new(&mut buf);
        xml.write_event(Event::Text(BytesText::new(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>",
        )))
        .map_err(|e| AppError::internal_error(format!("XML write failed: {}", e)))?;

        let mut ms = BytesStart::new("d:multistatus");
        ms.push_attribute(("xmlns:d", "DAV:"));
        ms.push_attribute(("xmlns:oc", "http://owncloud.org/ns"));
        xml.write_event(Event::Start(ms))
            .map_err(|e| AppError::internal_error(format!("XML: {}", e)))?;

        xml.write_event(Event::Start(BytesStart::new("d:response")))
            .map_err(|e| AppError::internal_error(format!("XML: {}", e)))?;
        write_text_element(&mut xml, "d:href", &href)
            .map_err(|e| AppError::internal_error(format!("XML: {}", e)))?;
        xml.write_event(Event::Start(BytesStart::new("d:propstat")))
            .map_err(|e| AppError::internal_error(format!("XML: {}", e)))?;
        xml.write_event(Event::Start(BytesStart::new("d:prop")))
            .map_err(|e| AppError::internal_error(format!("XML: {}", e)))?;
        xml.write_event(Event::Empty(BytesStart::new("oc:favorite")))
            .map_err(|e| AppError::internal_error(format!("XML: {}", e)))?;
        xml.write_event(Event::End(BytesEnd::new("d:prop")))
            .map_err(|e| AppError::internal_error(format!("XML: {}", e)))?;
        write_text_element(&mut xml, "d:status", "HTTP/1.1 200 OK")
            .map_err(|e| AppError::internal_error(format!("XML: {}", e)))?;
        xml.write_event(Event::End(BytesEnd::new("d:propstat")))
            .map_err(|e| AppError::internal_error(format!("XML: {}", e)))?;
        xml.write_event(Event::End(BytesEnd::new("d:response")))
            .map_err(|e| AppError::internal_error(format!("XML: {}", e)))?;
        xml.write_event(Event::End(BytesEnd::new("d:multistatus")))
            .map_err(|e| AppError::internal_error(format!("XML: {}", e)))?;
    }

    Ok(Response::builder()
        .status(StatusCode::MULTI_STATUS)
        .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
        .body(Body::from(buf))
        .unwrap())
}

/// Parse the oc:favorite value from a PROPPATCH XML body using quick_xml.
fn parse_proppatch_favorite(body: &str) -> Option<u8> {
    use quick_xml::Reader;

    let mut reader = Reader::from_str(body);
    let mut inside_favorite = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let local = e.local_name();
                if local.as_ref() == b"favorite" {
                    inside_favorite = true;
                }
            }
            Ok(Event::Text(ref e)) if inside_favorite => {
                let text = e.decode().ok()?;
                return text.trim().parse::<u8>().ok();
            }
            Ok(Event::End(ref e)) if e.local_name().as_ref() == b"favorite" => {
                inside_favorite = false;
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    None
}

// ──────────────────── PUT ────────────────────

async fn handle_put(
    state: Arc<AppState>,
    req: Request<Body>,
    user: &CurrentUser,
    subpath: &str,
) -> Result<Response<Body>, AppError> {
    let internal_path = nc_to_internal_path(&user.username, subpath)?;
    let file_service = &state.applications.file_retrieval_service;
    let upload_service = &state.applications.file_upload_service;

    let claimed_type = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    let oc_mtime = req
        .headers()
        .get("x-oc-mtime")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok());

    // ── Direct PUT cap ───────────────────────────────────────────────
    // We use `direct_put_max_bytes` (default 1 GiB), not `max_upload_size`
    // (default 10 GB). Larger files must come through the chunked upload
    // protocol (`/dav/uploads/...`) which is resumable on failure and
    // bounded per-request by `chunk_max_bytes`. Trying to stream a
    // multi-GB body through a single PUT is a footgun: a connection drop
    // at 95 % loses everything.
    let max_upload = state.core.config.storage.direct_put_max_bytes;

    // Stream the body to a temp file + incremental hash — never buffer the
    // full upload in RAM.  The old `body::to_bytes` path loaded the entire
    // file (e.g. an 800 MB ISO) into anonymous memory before any dedup logic,
    // OOMKilling the process even on dedup hits.  Shared with the native
    // WebDAV PUT handler; peak heap ~one frame regardless of file size.
    let spooled = spool_body_to_temp(
        req.into_body(),
        max_upload,
        state.core.config.storage.upload_temp_dir.clone(),
    )
    .await?;

    // Detect real MIME type from the first bytes on disk (no full read).
    // `filename` is owned so we don't hold a borrow of the `subpath` param
    // across the await (which would make the handler future non-Send).
    let filename = filename_from_path(subpath).to_string();
    let content_type =
        refine_content_type_from_file(spooled.temp.path(), &filename, &claimed_type).await;

    // Distinguish create (201) vs update (204) for the response status.
    let existed = file_service.get_file_by_path(&internal_path).await.is_ok();

    // Single streaming path — handles both update and create internally,
    // passing the precomputed hash so the dedup fast path can short-circuit
    // without re-reading the file.
    let stored = upload_service
        .update_file_streaming(
            &internal_path,
            spooled.temp.path(),
            spooled.size,
            &content_type,
            Some(spooled.hash),
            oc_mtime,
        )
        .await
        .map_err(|e| AppError::internal_error(format!("Failed to store file: {}", e)))?;

    // dedup may have already moved the temp on a new-blob store; ignore error.
    let _ = tokio::fs::remove_file(spooled.temp.path()).await;

    let status = if existed {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::CREATED
    };

    Ok(Response::builder()
        .status(status)
        .header(header::ETAG, format!("\"{}\"", stored.etag))
        .header("oc-etag", format!("\"{}\"", stored.etag))
        .body(Body::empty())
        .unwrap())
}

// ──────────────────── MKCOL ────────────────────

async fn handle_mkcol(
    state: Arc<AppState>,
    user: &CurrentUser,
    subpath: &str,
) -> Result<Response<Body>, AppError> {
    use crate::application::dtos::folder_dto::CreateFolderDto;

    let folder_service = &state.applications.folder_service;
    let internal_path = nc_to_internal_path(&user.username, subpath)?;

    // If the folder already exists, return 405 per RFC 4918 §9.3.1
    if folder_service
        .get_folder_by_path(&internal_path)
        .await
        .is_ok()
    {
        return Ok(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body(Body::empty())
            .unwrap());
    }

    // Collect path segments that need to be created (walk from root to leaf)
    let segments: Vec<&str> = subpath.split('/').filter(|s| !s.is_empty()).collect();

    let user_root = nc_to_internal_path(&user.username, "")?;
    let mut current_path = user_root.clone();
    let mut parent_id = folder_service
        .get_folder_by_path(&user_root)
        .await
        .map_err(|_| AppError::not_found("User root folder not found"))?
        .id
        .clone();

    for segment in &segments {
        current_path = format!("{}/{}", current_path, segment);
        match folder_service.get_folder_by_path(&current_path).await {
            Ok(existing) => {
                parent_id = existing.id.clone();
            }
            Err(_) => {
                let dto = CreateFolderDto {
                    name: segment.to_string(),
                    parent_id: Some(parent_id.clone()),
                };
                match folder_service.create_folder_with_perms(dto, user.id).await {
                    Ok(created) => {
                        parent_id = created.id.clone();
                    }
                    Err(e)
                        if e.message.contains("already exists")
                            || e.message.contains("Already Exists") =>
                    {
                        // Race condition — folder created concurrently
                        let folder = folder_service
                            .get_folder_by_path(&current_path)
                            .await
                            .map_err(|_| {
                                AppError::internal_error("Folder exists but cannot be found")
                            })?;
                        parent_id = folder.id.clone();
                    }
                    Err(e) => {
                        return Err(AppError::internal_error(format!(
                            "Failed to create folder: {}",
                            e
                        )));
                    }
                }
            }
        }
    }

    Ok(Response::builder()
        .status(StatusCode::CREATED)
        .body(Body::empty())
        .unwrap())
}

// ──────────────────── DELETE ────────────────────

async fn handle_delete(
    state: Arc<AppState>,
    user: &CurrentUser,
    subpath: &str,
) -> Result<Response<Body>, AppError> {
    let internal_path = nc_to_internal_path(&user.username, subpath)?;
    let folder_service = &state.applications.folder_service;
    let file_service = &state.applications.file_retrieval_service;

    // Prefer soft-delete (move to trash) when trash service is available.
    // This is what Nextcloud clients expect — items appear in the trashbin.
    if let Some(trash_svc) = state.trash_service.as_ref() {
        if let Ok(folder) = folder_service.get_folder_by_path(&internal_path).await {
            trash_svc
                .move_to_trash(&folder.id, "folder", user.id)
                .await
                .map_err(|e| AppError::internal_error(format!("Failed to trash folder: {}", e)))?;
            return Ok(Response::builder()
                .status(StatusCode::NO_CONTENT)
                .body(Body::empty())
                .unwrap());
        }
        if let Ok(file) = file_service.get_file_by_path(&internal_path).await {
            trash_svc
                .move_to_trash(&file.id, "file", user.id)
                .await
                .map_err(|e| AppError::internal_error(format!("Failed to trash file: {}", e)))?;
            return Ok(Response::builder()
                .status(StatusCode::NO_CONTENT)
                .body(Body::empty())
                .unwrap());
        }
        return Err(AppError::not_found("Resource not found"));
    }

    // Fallback: hard delete when trash service is not available.
    let file_mgmt = &state.applications.file_management_service;

    if let Ok(folder) = folder_service.get_folder_by_path(&internal_path).await {
        folder_service
            .delete_folder_with_perms(&folder.id, user.id)
            .await
            .map_err(|e| AppError::internal_error(format!("Failed to delete folder: {}", e)))?;

        return Ok(Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Body::empty())
            .unwrap());
    }

    if let Ok(file) = file_service.get_file_by_path(&internal_path).await {
        file_mgmt
            .delete_file_with_perms(&file.id, user.id)
            .await
            .map_err(|e| AppError::internal_error(format!("Failed to delete file: {}", e)))?;

        return Ok(Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Body::empty())
            .unwrap());
    }

    Err(AppError::not_found("Resource not found"))
}

// ──────────────────── MOVE ────────────────────

async fn handle_move(
    state: Arc<AppState>,
    req: Request<Body>,
    user: &CurrentUser,
    subpath: &str,
) -> Result<Response<Body>, AppError> {
    let destination = req
        .headers()
        .get("destination")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::bad_request("Missing Destination header"))?
        .to_string();

    // Parse destination path: extract subpath after /remote.php/dav/files/{user}/
    let dest_subpath = extract_nc_subpath_from_dest(&destination, &user.username)
        .ok_or_else(|| AppError::bad_request("Invalid Destination URL"))?;

    let src_internal = nc_to_internal_path(&user.username, subpath)?;
    let folder_service = &state.applications.folder_service;
    let file_service = &state.applications.file_retrieval_service;
    let file_mgmt = &state.applications.file_management_service;

    // Try as file first.
    if let Ok(file) = file_service.get_file_by_path(&src_internal).await {
        let (dest_parent_sub, dest_name) = match dest_subpath.rsplit_once('/') {
            Some((parent, name)) => (parent, name),
            None => ("", dest_subpath.as_str()),
        };
        let dest_parent_internal = nc_to_internal_path(&user.username, dest_parent_sub)?;

        // Rename if only the name changes (same parent).
        let src_parent_sub = match subpath.rsplit_once('/') {
            Some((parent, _)) => parent,
            None => "",
        };

        if src_parent_sub == dest_parent_sub {
            // Same parent → rename.
            file_mgmt
                .rename_file_with_perms(&file.id, user.id, dest_name)
                .await
                .map_err(|e| AppError::internal_error(format!("Rename failed: {}", e)))?;
        } else {
            // Different parent → move.
            let dest_parent = folder_service
                .get_folder_by_path(&dest_parent_internal)
                .await
                .map_err(|_| AppError::not_found("Destination folder not found"))?;

            file_mgmt
                .move_file_with_perms(&file.id, user.id, Some(dest_parent.id.clone()))
                .await
                .map_err(|e| AppError::internal_error(format!("Move failed: {}", e)))?;

            // If the filename changed too, rename after move.
            if file.name != dest_name {
                file_mgmt
                    .rename_file_with_perms(&file.id, user.id, dest_name)
                    .await
                    .map_err(|e| AppError::internal_error(format!("Rename failed: {}", e)))?;
            }
        }

        // Return ETag and OC-ETag so Nextcloud clients can track the moved file.
        let dest_internal = nc_to_internal_path(&user.username, &dest_subpath)?;
        let mut builder = Response::builder().status(StatusCode::CREATED);
        if let Ok(moved) = file_service.get_file_by_path(&dest_internal).await {
            // Route through `FileDto::etag` so the MOVE response
            // matches what a subsequent PROPFIND on the destination
            // will return — `moved.id` (UUID) would differ from the
            // blob hash and trigger NC's "remote changed" detection.
            builder = builder
                .header(header::ETAG, format!("\"{}\"", moved.etag))
                .header("oc-etag", format!("\"{}\"", moved.etag));
        }

        return Ok(builder.body(Body::empty()).unwrap());
    }

    // Try as folder.
    if let Ok(folder) = folder_service.get_folder_by_path(&src_internal).await {
        let (dest_parent_sub, dest_name) = match dest_subpath.rsplit_once('/') {
            Some((parent, name)) => (parent, name),
            None => ("", dest_subpath.as_str()),
        };
        let dest_parent_internal = nc_to_internal_path(&user.username, dest_parent_sub)?;

        let src_parent_sub = match subpath.rsplit_once('/') {
            Some((parent, _)) => parent,
            None => "",
        };

        if src_parent_sub == dest_parent_sub {
            // Same parent → rename.
            use crate::application::dtos::folder_dto::RenameFolderDto;
            folder_service
                .rename_folder_with_perms(
                    &folder.id,
                    RenameFolderDto {
                        name: dest_name.to_string(),
                    },
                    user.id,
                )
                .await
                .map_err(|e| AppError::internal_error(format!("Rename failed: {}", e)))?;
        } else {
            // Different parent → move.
            let dest_parent = folder_service
                .get_folder_by_path(&dest_parent_internal)
                .await
                .map_err(|_| AppError::not_found("Destination parent not found"))?;

            use crate::application::dtos::folder_dto::MoveFolderDto;
            folder_service
                .move_folder_with_perms(
                    &folder.id,
                    MoveFolderDto {
                        parent_id: Some(dest_parent.id.clone()),
                    },
                    user.id,
                )
                .await
                .map_err(|e| AppError::internal_error(format!("Move failed: {}", e)))?;

            // If the name changed too, rename.
            if folder.name != dest_name {
                use crate::application::dtos::folder_dto::RenameFolderDto;
                folder_service
                    .rename_folder_with_perms(
                        &folder.id,
                        RenameFolderDto {
                            name: dest_name.to_string(),
                        },
                        user.id,
                    )
                    .await
                    .map_err(|e| AppError::internal_error(format!("Rename failed: {}", e)))?;
            }
        }

        return Ok(Response::builder()
            .status(StatusCode::CREATED)
            .body(Body::empty())
            .unwrap());
    }

    Err(AppError::not_found("Source resource not found"))
}

/// Extract the subpath from a Destination header URL.
///
/// Only accepts relative paths or absolute URLs whose path starts with the
/// expected DAV prefix.  For full URLs the host is ignored — the path alone is
/// used — so an attacker cannot redirect the server to a different host.
fn extract_nc_subpath_from_dest(dest: &str, username: &str) -> Option<String> {
    let prefix = format!("/remote.php/dav/files/{}/", username);
    // For full URLs, extract the path portion (everything after the authority).
    let path = if dest.starts_with("http://") || dest.starts_with("https://") {
        // Find the start of the path after "scheme://host".
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

// ────────────── Nextcloud PROPFIND XML Generation ──────────────

use crate::application::dtos::file_dto::FileDto;
use crate::application::dtos::folder_dto::FolderDto;
use crate::application::services::nextcloud_file_id_service::NextcloudFileIdService;

/// Write the `<d:multistatus>` opening tag with the full NC namespace set.
/// Shared by the streaming folder PROPFIND and the single-file variant so
/// the namespace list can never diverge between the two.
fn write_nc_multistatus_open<W: std::io::Write>(xml: &mut Writer<W>) -> Result<(), String> {
    let mut ms = BytesStart::new("d:multistatus");
    ms.push_attribute(("xmlns:d", "DAV:"));
    ms.push_attribute(("xmlns:oc", "http://owncloud.org/ns"));
    ms.push_attribute(("xmlns:nc", "http://nextcloud.org/ns"));
    ms.push_attribute(("xmlns:ocs", "http://open-collaboration-services.org/ns"));
    xml.write_event(Event::Start(ms)).xml_err()
}

/// Generate the multistatus XML for a single-file PROPFIND. The folder
/// case streams via [`build_nc_streaming_propfind`] instead.
async fn write_nc_file_multistatus<W: std::io::Write>(
    writer: W,
    file: &FileDto,
    username: &str,
    subpath: &str,
    file_id_svc: Option<&Arc<NextcloudFileIdService>>,
    favorite_ids: &HashSet<String>,
) -> Result<(), String> {
    let (file_id_map, _) =
        batch_resolve_ids(file_id_svc, std::slice::from_ref(&file.id), &[]).await;

    let mut xml = Writer::new(writer);
    write_nc_multistatus_open(&mut xml)?;

    // Single-file PROPFIND — subpath already points to the file.
    let href = nc_href(username, subpath);
    let file_id = file_id_map.get(&file.id).copied();
    let oc_id = file_id.map(|id| format_oc_id(id, file_id_svc));
    write_file_response(
        &mut xml,
        file,
        &href,
        file_id,
        oc_id.as_deref(),
        username,
        favorite_ids,
    )?;

    xml.write_event(Event::End(BytesEnd::new("d:multistatus")))
        .xml_err()?;

    Ok(())
}

/// Build a streaming 207 Multi-Status response for a folder PROPFIND.
///
/// Mirrors the native WebDAV handler's `build_streaming_propfind_response`:
/// children are fetched in pages of [`PROPFIND_BATCH_SIZE`], each page's
/// favorites and `oc:fileid`s are resolved with two batch queries, and the
/// XML is yielded chunk by chunk — memory stays O(batch) and the response
/// starts flowing immediately, instead of materializing the full listing
/// plus its entire multistatus (~2 KB/entry) in RAM before the first byte.
fn build_nc_streaming_propfind(
    state: Arc<AppState>,
    folder: FolderDto,
    depth: String,
    user_id: Uuid,
    username: String,
    subpath: String,
) -> Response<Body> {
    let stream = async_stream::try_stream! {
        let file_id_svc = state.nextcloud.as_ref().map(|n| &n.file_ids);
        let fav_svc = state.favorites_service.as_ref();
        let folder_service = &state.applications.folder_service;
        let file_service = &state.applications.file_retrieval_service;

        // ── <d:multistatus> + the folder's own entry ─────────────────
        // Collection hrefs MUST end in `/` (RFC 4918 §5.2 + strict
        // NC-client enforcement — see `nc_collection_href`).
        let folder_favs = if let Some(fav) = fav_svc {
            fav.batch_check_favorites(user_id, &[(folder.id.as_str(), "folder")])
                .await
                .unwrap_or_default()
        } else {
            HashSet::new()
        };
        let (_, folder_id_map) =
            batch_resolve_ids(file_id_svc, &[], std::slice::from_ref(&folder.id)).await;

        let mut buf = Vec::with_capacity(4096);
        {
            let mut xml = Writer::new(&mut buf);
            write_nc_multistatus_open(&mut xml).map_err(std::io::Error::other)?;
            let href = nc_collection_href(&username, &subpath);
            let fid = folder_id_map.get(&folder.id).copied();
            let oc_id = fid.map(|id| format_oc_id(id, file_id_svc));
            write_folder_response(&mut xml, &folder, &href, fid, oc_id.as_deref(), &username, &folder_favs)
                .map_err(std::io::Error::other)?;
        }
        yield Bytes::from(buf);

        // ── Children (only if Depth != 0) ────────────────────────────
        if depth != "0" {
            // Files in pages.
            let mut offset: i64 = 0;
            loop {
                let batch = file_service
                    .list_files_batch_with_perms(Some(&folder.id), user_id, offset, PROPFIND_BATCH_SIZE)
                    .await
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
                if batch.is_empty() {
                    break;
                }
                let batch_len = batch.len();

                // Per-page enrichment: favorites + oc:fileids, two batch queries.
                let favs = if let Some(fav) = fav_svc {
                    let items: Vec<(&str, &str)> =
                        batch.iter().map(|f| (f.id.as_str(), "file")).collect();
                    fav.batch_check_favorites(user_id, &items).await.unwrap_or_default()
                } else {
                    HashSet::new()
                };
                let file_uuids: Vec<String> = batch.iter().map(|f| f.id.clone()).collect();
                let (file_id_map, _) = batch_resolve_ids(file_id_svc, &file_uuids, &[]).await;

                let mut chunk = Vec::with_capacity(batch_len * 1024);
                {
                    let mut xml = Writer::new(&mut chunk);
                    for file in &batch {
                        let child_sub = if subpath.is_empty() {
                            file.name.clone()
                        } else {
                            format!("{}/{}", subpath.trim_end_matches('/'), file.name)
                        };
                        let href = nc_href(&username, &child_sub);
                        let fid = file_id_map.get(&file.id).copied();
                        let oc_id = fid.map(|id| format_oc_id(id, file_id_svc));
                        write_file_response(&mut xml, file, &href, fid, oc_id.as_deref(), &username, &favs)
                            .map_err(std::io::Error::other)?;
                    }
                }
                yield Bytes::from(chunk);

                if (batch_len as i64) < PROPFIND_BATCH_SIZE {
                    break;
                }
                offset += batch_len as i64;
            }

            // Subfolders in pages — also collections, same trailing-slash rule.
            let mut page = 0usize;
            loop {
                let pag = PaginationRequestDto {
                    page,
                    page_size: PROPFIND_BATCH_SIZE as usize,
                };
                let result = folder_service
                    .list_folders_paginated_with_perms(Some(&folder.id), user_id, &pag)
                    .await
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
                if result.items.is_empty() {
                    break;
                }

                let favs = if let Some(fav) = fav_svc {
                    let items: Vec<(&str, &str)> =
                        result.items.iter().map(|sf| (sf.id.as_str(), "folder")).collect();
                    fav.batch_check_favorites(user_id, &items).await.unwrap_or_default()
                } else {
                    HashSet::new()
                };
                let folder_uuids: Vec<String> = result.items.iter().map(|sf| sf.id.clone()).collect();
                let (_, sub_id_map) = batch_resolve_ids(file_id_svc, &[], &folder_uuids).await;

                let mut chunk = Vec::with_capacity(result.items.len() * 1024);
                {
                    let mut xml = Writer::new(&mut chunk);
                    for sf in &result.items {
                        let child_sub = if subpath.is_empty() {
                            sf.name.clone()
                        } else {
                            format!("{}/{}", subpath.trim_end_matches('/'), sf.name)
                        };
                        let href = nc_collection_href(&username, &child_sub);
                        let fid = sub_id_map.get(&sf.id).copied();
                        let oc_id = fid.map(|id| format_oc_id(id, file_id_svc));
                        write_folder_response(&mut xml, sf, &href, fid, oc_id.as_deref(), &username, &favs)
                            .map_err(std::io::Error::other)?;
                    }
                }
                let has_more = result.pagination.has_next;
                yield Bytes::from(chunk);

                if !has_more {
                    break;
                }
                page += 1;
            }
        }

        // ── </d:multistatus> ─────────────────────────────────────────
        let mut buf = Vec::with_capacity(32);
        {
            let mut xml = Writer::new(&mut buf);
            xml.write_event(Event::End(BytesEnd::new("d:multistatus")))
                .map_err(|e| std::io::Error::other(e.to_string()))?;
        }
        yield Bytes::from(buf);
    };

    use futures::TryStreamExt;
    let stream = stream
        .map_err(|e: std::io::Error| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) });

    Response::builder()
        .status(StatusCode::MULTI_STATUS)
        .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
        .body(Body::from_stream(stream))
        .unwrap()
}

pub fn write_folder_response<W: std::io::Write>(
    xml: &mut Writer<W>,
    folder: &FolderDto,
    href: &str,
    file_id: Option<i64>,
    oc_id: Option<&str>,
    owner: &str,
    favorite_ids: &HashSet<String>,
) -> Result<(), String> {
    xml.write_event(Event::Start(BytesStart::new("d:response")))
        .xml_err()?;

    // href
    write_text_element(xml, "d:href", href)?;

    xml.write_event(Event::Start(BytesStart::new("d:propstat")))
        .xml_err()?;
    xml.write_event(Event::Start(BytesStart::new("d:prop")))
        .xml_err()?;

    // resourcetype
    xml.write_event(Event::Start(BytesStart::new("d:resourcetype")))
        .xml_err()?;
    xml.write_event(Event::Empty(BytesStart::new("d:collection")))
        .xml_err()?;
    xml.write_event(Event::End(BytesEnd::new("d:resourcetype")))
        .xml_err()?;

    write_text_element(xml, "d:displayname", &folder.name)?;

    let created_at =
        chrono::DateTime::<Utc>::from_timestamp(timestamp_to_i64(folder.created_at), 0)
            .unwrap_or_else(Utc::now);
    let modified_at =
        chrono::DateTime::<Utc>::from_timestamp(timestamp_to_i64(folder.modified_at), 0)
            .unwrap_or_else(Utc::now);

    write_text_element(xml, "d:getlastmodified", &modified_at.to_rfc2822())?;
    // Route through `FolderDto::etag` (= `Folder::etag()`, currently
    // the folder UUID — see the entity for the documented v1 formula
    // and the follow-up plan to make it descendant-aware).
    write_text_element(xml, "d:getetag", &format!("\"{}\"", folder.etag))?;
    write_text_element(xml, "d:getcontenttype", "httpd/unix-directory")?;
    write_text_element(xml, "d:getcontentlength", "0")?;
    write_text_element(xml, "d:creationdate", &created_at.to_rfc3339())?;

    // Nextcloud/ownCloud properties
    if let Some(id) = file_id {
        write_text_element(xml, "oc:fileid", &id.to_string())?;
    }
    if let Some(oid) = oc_id {
        write_text_element(xml, "oc:id", oid)?;
    }
    write_text_element(xml, "oc:permissions", "RGDNVCK")?;
    // Numeric share-permissions bitmask: Read=1 + Update=2 + Create=4 + Delete=8 + Share=16 = 31
    write_text_element(xml, "ocs:share-permissions", "31")?;
    write_text_element(xml, "oc:size", "0")?;
    write_text_element(xml, "oc:owner-id", owner)?;
    write_text_element(xml, "oc:owner-display-name", owner)?;
    write_text_element(xml, "nc:has-preview", "false")?;
    write_text_element(xml, "nc:is-encrypted", "0")?;
    write_text_element(xml, "nc:mount-type", "")?;

    let is_fav = if favorite_ids.contains(&folder.id) {
        "1"
    } else {
        "0"
    };
    write_text_element(xml, "oc:favorite", is_fav)?;
    // Empty share-types (no sharing API yet)
    xml.write_event(Event::Empty(BytesStart::new("oc:share-types")))
        .xml_err()?;

    xml.write_event(Event::End(BytesEnd::new("d:prop")))
        .xml_err()?;
    write_text_element(xml, "d:status", "HTTP/1.1 200 OK")?;
    xml.write_event(Event::End(BytesEnd::new("d:propstat")))
        .xml_err()?;

    xml.write_event(Event::End(BytesEnd::new("d:response")))
        .xml_err()?;

    Ok(())
}

pub fn write_file_response<W: std::io::Write>(
    xml: &mut Writer<W>,
    file: &FileDto,
    href: &str,
    file_id: Option<i64>,
    oc_id: Option<&str>,
    owner: &str,
    favorite_ids: &HashSet<String>,
) -> Result<(), String> {
    xml.write_event(Event::Start(BytesStart::new("d:response")))
        .xml_err()?;

    write_text_element(xml, "d:href", href)?;

    xml.write_event(Event::Start(BytesStart::new("d:propstat")))
        .xml_err()?;
    xml.write_event(Event::Start(BytesStart::new("d:prop")))
        .xml_err()?;

    // resourcetype (empty for files)
    xml.write_event(Event::Empty(BytesStart::new("d:resourcetype")))
        .xml_err()?;

    write_text_element(xml, "d:displayname", &file.name)?;
    write_text_element(xml, "d:getcontenttype", &file.mime_type)?;
    write_text_element(xml, "d:getcontentlength", &file.size.to_string())?;

    let created_at = chrono::DateTime::<Utc>::from_timestamp(timestamp_to_i64(file.created_at), 0)
        .unwrap_or_else(Utc::now);
    let modified_at =
        chrono::DateTime::<Utc>::from_timestamp(timestamp_to_i64(file.modified_at), 0)
            .unwrap_or_else(Utc::now);

    write_text_element(xml, "d:getlastmodified", &modified_at.to_rfc2822())?;
    write_text_element(xml, "d:getetag", &format!("\"{}\"", file.etag))?;
    write_text_element(xml, "d:creationdate", &created_at.to_rfc3339())?;

    // Nextcloud/ownCloud properties
    if let Some(id) = file_id {
        write_text_element(xml, "oc:fileid", &id.to_string())?;
    }
    if let Some(oid) = oc_id {
        write_text_element(xml, "oc:id", oid)?;
    }
    write_text_element(xml, "oc:permissions", "RGDNVW")?;
    // Numeric share-permissions bitmask: Read=1 + Update=2 + Delete=8 + Share=16 = 27
    write_text_element(xml, "ocs:share-permissions", "27")?;
    write_text_element(xml, "oc:size", &file.size.to_string())?;
    write_text_element(xml, "oc:owner-id", owner)?;
    write_text_element(xml, "oc:owner-display-name", owner)?;

    let is_fav = if favorite_ids.contains(&file.id) {
        "1"
    } else {
        "0"
    };
    write_text_element(xml, "oc:favorite", is_fav)?;
    // Empty share-types (no sharing API yet)
    xml.write_event(Event::Empty(BytesStart::new("oc:share-types")))
        .xml_err()?;

    // Check if file is an image that can have previews
    let has_preview = matches!(
        &*file.mime_type,
        "image/jpeg" | "image/jpg" | "image/png" | "image/gif" | "image/webp"
    );
    write_text_element(
        xml,
        "nc:has-preview",
        if has_preview { "true" } else { "false" },
    )?;

    write_text_element(xml, "nc:is-encrypted", "0")?;
    write_text_element(xml, "nc:mount-type", "")?;
    write_text_element(xml, "nc:creation_time", &file.created_at.to_string())?;
    write_text_element(xml, "nc:upload_time", &file.modified_at.to_string())?;

    xml.write_event(Event::End(BytesEnd::new("d:prop")))
        .xml_err()?;
    write_text_element(xml, "d:status", "HTTP/1.1 200 OK")?;
    xml.write_event(Event::End(BytesEnd::new("d:propstat")))
        .xml_err()?;

    xml.write_event(Event::End(BytesEnd::new("d:response")))
        .xml_err()?;

    Ok(())
}

pub fn write_text_element<W: std::io::Write>(
    xml: &mut Writer<W>,
    tag: &str,
    value: &str,
) -> Result<(), String> {
    xml.write_event(Event::Start(BytesStart::new(tag)))
        .xml_err()?;
    xml.write_event(Event::Text(BytesText::new(value)))
        .xml_err()?;
    xml.write_event(Event::End(BytesEnd::new(tag))).xml_err()?;
    Ok(())
}

/// Resolve every `oc:fileid` for a listing in two batch queries (one per
/// object type) instead of one INSERT round-trip per child. Returns
/// `(file_map, folder_map)` keyed by object UUID; entries are absent when the
/// service is disabled or an id can't be resolved, mirroring the previous
/// per-call `Option` behaviour. The two batches run concurrently.
pub async fn batch_resolve_ids(
    svc: Option<&Arc<NextcloudFileIdService>>,
    file_uuids: &[String],
    folder_uuids: &[String],
) -> (HashMap<String, i64>, HashMap<String, i64>) {
    let Some(svc) = svc else {
        return (HashMap::new(), HashMap::new());
    };
    let (files, folders) = tokio::join!(
        svc.get_or_create_file_ids(file_uuids),
        svc.get_or_create_folder_ids(folder_uuids),
    );
    (files.unwrap_or_default(), folders.unwrap_or_default())
}

pub fn format_oc_id(id: i64, svc: Option<&Arc<NextcloudFileIdService>>) -> String {
    match svc {
        Some(s) => s.format_oc_id(id),
        None => format!("{:08}ocnca", id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── nc_to_internal_path ──

    #[test]
    fn test_empty_subpath_returns_home() {
        assert_eq!(
            nc_to_internal_path("alice", "").unwrap(),
            "My Folder - alice"
        );
    }

    #[test]
    fn test_subpath_appended() {
        assert_eq!(
            nc_to_internal_path("alice", "Documents/work").unwrap(),
            "My Folder - alice/Documents/work"
        );
    }

    #[test]
    fn test_strips_surrounding_slashes() {
        assert_eq!(
            nc_to_internal_path("alice", "/Photos/").unwrap(),
            "My Folder - alice/Photos"
        );
    }

    #[test]
    fn test_rejects_dot_dot_traversal() {
        assert!(nc_to_internal_path("alice", "../etc/passwd").is_err());
    }

    #[test]
    fn test_rejects_single_dot() {
        assert!(nc_to_internal_path("alice", "foo/./bar").is_err());
    }

    // ── nc_href ──

    #[test]
    fn test_href_root() {
        assert_eq!(nc_href("alice", ""), "/remote.php/dav/files/alice/");
    }

    #[test]
    fn test_href_encodes_spaces() {
        assert_eq!(
            nc_href("alice", "My Photos/vacation pic.jpg"),
            "/remote.php/dav/files/alice/My%20Photos/vacation%20pic.jpg"
        );
    }

    #[test]
    fn test_href_encodes_special_chars() {
        let href = nc_href("alice", "file#1.txt");
        assert!(href.contains("file%231.txt"));
    }

    // ── nc_collection_href ──
    // RFC 4918 §5.2 requires a collection URL to end in '/'. The NC
    // desktop client at `networkjobs.cpp:234` aborts the PROPFIND
    // parse with `Invalid href "<…>" expected starting with
    // "<requested-url>"` if the own-entry href is missing the slash.
    // These tests pin the helper's behaviour so the regression can't
    // come back silently.

    #[test]
    fn test_collection_href_appends_slash_when_missing() {
        assert_eq!(
            nc_collection_href("alice", "ext"),
            "/remote.php/dav/files/alice/ext/"
        );
    }

    #[test]
    fn test_collection_href_idempotent_at_root() {
        // Root subpath already ends in '/' — don't double-append.
        assert_eq!(
            nc_collection_href("alice", ""),
            "/remote.php/dav/files/alice/"
        );
    }

    #[test]
    fn test_collection_href_preserves_encoding() {
        // Wrapping must not re-encode or double-encode already-encoded
        // segments.
        assert_eq!(
            nc_collection_href("alice", "My Photos/2024"),
            "/remote.php/dav/files/alice/My%20Photos/2024/"
        );
    }

    // ── extract_nc_subpath_from_dest ──

    #[test]
    fn test_extract_relative_path() {
        let result = extract_nc_subpath_from_dest(
            "/remote.php/dav/files/alice/Documents/moved.txt",
            "alice",
        );
        assert_eq!(result.as_deref(), Some("Documents/moved.txt"));
    }

    #[test]
    fn test_extract_absolute_url() {
        let result = extract_nc_subpath_from_dest(
            "https://cloud.example.com/remote.php/dav/files/alice/new.txt",
            "alice",
        );
        assert_eq!(result.as_deref(), Some("new.txt"));
    }

    #[test]
    fn test_extract_url_encoded() {
        let result = extract_nc_subpath_from_dest(
            "/remote.php/dav/files/alice/My%20Folder/file.txt",
            "alice",
        );
        assert_eq!(result.as_deref(), Some("My Folder/file.txt"));
    }

    #[test]
    fn test_extract_wrong_user_returns_none() {
        let result = extract_nc_subpath_from_dest("/remote.php/dav/files/bob/secret.txt", "alice");
        assert!(result.is_none());
    }

    // ── timestamp_to_i64 ──

    #[test]
    fn test_timestamp_normal() {
        assert_eq!(timestamp_to_i64(1700000000), 1700000000i64);
    }

    #[test]
    fn test_timestamp_overflow_returns_zero() {
        assert_eq!(timestamp_to_i64(u64::MAX), 0);
    }
}
