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
use crate::common::mime_detect::filename_from_path;
use crate::interfaces::api::handlers::webdav_handler::PROPFIND_BATCH_SIZE;
use crate::interfaces::errors::AppError;
use crate::interfaces::range_requests::{not_modified_response, range_response};
use crate::interfaces::upload_ingest::ingest_body_to_cas;

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

/// Resolve the internal OxiCloud path from a NextCloud DAV subpath
/// and the storage chroot the request is confined to.
///
/// `chroot` is the storage path the request is "jailed" inside —
/// the route glue (`routes.rs::handle_dav_*`) computes it once per
/// request:
/// - Legacy `/files/{user}/…` or explicit `~{home_folder_uuid}` →
///   `"My Folder - {username}"` (no DB lookup needed).
/// - `~{some_other_folder_uuid}` → the folder's stored `path` after
///   a `get_folder_with_perms` check (404 if missing / no access).
///
/// By the time we get here `chroot` is known to be a legitimate
/// target — validation and permission live in the route layer, not
/// in the path mapper. This function stays sync and free of any
/// folder-service handle. The chroot's `path` is the canonical root
/// segment (e.g. `"Personal"` for default personal drives provisioned
/// by D0, the original sibling-root folder name for secondary drives).
/// Replaces the pre-D0 hardcoded `"My Folder - {username}/"` prefix.
pub fn nc_to_internal_path(chroot: &FolderDto, subpath: &str) -> Result<String, AppError> {
    let subpath = subpath.trim_matches('/');
    if subpath.is_empty() {
        return Ok(chroot.path.clone());
    }
    // Reject path traversal attempts.
    if subpath.split('/').any(|seg| seg == ".." || seg == ".") {
        return Err(AppError::bad_request("Invalid path: traversal not allowed"));
    }

    Ok(format!("{}/{}", chroot.path, subpath))
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
/// `session.chroot` is the storage path the request is confined to
/// — see [`nc_to_internal_path`] for what gets resolved upstream.
/// `session.raw_username` is the literal wire identifier — bare
/// `admin` for single-drive sync, composite `admin~{drive_uuid}` for
/// multi-drive. **Hrefs in every response MUST be built from
/// `session.raw_username`, not from `session.user.username`** — the
/// NC desktop client validates that PROPFIND/MOVE response hrefs
/// share the requested URL's prefix and aborts the parse otherwise
/// (`Invalid href "<…>" expected starting with "<requested-url>"`).
/// The bare `session.user.username` is still the right value for
/// the storage-side owner identity (`oc:owner-id`).
pub async fn handle_nc_webdav(
    state: Arc<AppState>,
    req: Request<Body>,
    session: crate::interfaces::nextcloud::session::NcSession,
    subpath: String,
) -> Result<Response<Body>, AppError> {
    // Validate up-front that we have a chroot — every method below is
    // path-scoped, so a missing chroot is a route-wiring bug we want to
    // surface as a 500 immediately rather than re-checking inside each
    // handler.
    session.require_chroot()?;
    let method = req.method().clone();
    match method.as_str() {
        "OPTIONS" => handle_options(),
        "PROPFIND" => handle_propfind(state, req, &session, &subpath).await,
        "GET" => handle_get(state, &session, &subpath, req.headers()).await,
        "PUT" => handle_put(state, req, &session, &subpath).await,
        "MKCOL" => handle_mkcol(state, &session, &subpath).await,
        "DELETE" => handle_delete(state, &session, &subpath).await,
        "MOVE" => handle_move(state, req, &session, &subpath).await,
        "HEAD" => handle_head(state, &session, &subpath).await,
        "PROPPATCH" => handle_proppatch(state, req, &session, &subpath).await,
        "REPORT" | "SEARCH" => {
            crate::interfaces::nextcloud::report_handler::handle_nc_report(
                state, req, &session, &subpath,
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
    session: &crate::interfaces::nextcloud::session::NcSession,
    subpath: &str,
) -> Result<Response<Body>, AppError> {
    let user = &session.user;
    let chroot = session.require_chroot()?;
    let url_user = &session.raw_username;
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

    let internal_path = nc_to_internal_path(chroot, subpath)?;

    let folder_service = &state.applications.folder_service;
    let file_service = &state.applications.file_retrieval_service;

    // Try to resolve as folder first.
    let folder_result = folder_service
        .get_folder_by_path(&internal_path, chroot.drive_id)
        .await;

    if let Ok(folder) = folder_result {
        // It's a folder — stream the multistatus: children are fetched in
        // pages and serialized chunk by chunk, so memory stays O(batch)
        // regardless of how many entries the folder holds.
        //
        // Multi-drive POC: the hrefs in the response must echo the
        // wire form (`{user}~{drive}`) the client requested, so we
        // pass `url_user` (not `user.username`) as the streaming
        // function's username arg. Refining the owner-id usages
        // back to the canonical username is deferred to the
        // NcSession commit.
        return Ok(build_nc_streaming_propfind(
            state.clone(),
            folder,
            depth,
            user.id,
            url_user.to_string(),
            subpath.to_string(),
        ));
    }

    // Not a folder — try as a file.
    let file_result = file_service
        .get_file_by_path(&internal_path, chroot.drive_id)
        .await;
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
            url_user,
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
    session: &crate::interfaces::nextcloud::session::NcSession,
    subpath: &str,
    headers: &axum::http::HeaderMap,
) -> Result<Response<Body>, AppError> {
    let chroot = session.require_chroot()?;
    // GET on root folder — NC clients use this as an existence check
    if subpath.is_empty() || subpath == "/" {
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header("DAV", "1, 3")
            .body(Body::empty())
            .unwrap());
    }

    let internal_path = nc_to_internal_path(chroot, subpath)?;
    let file_service = &state.applications.file_retrieval_service;
    let folder_service = &state.applications.folder_service;

    // Check if path is a folder first (NC clients use GET as existence check)
    if folder_service
        .get_folder_by_path(&internal_path, chroot.drive_id)
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
        .get_file_by_path(&internal_path, chroot.drive_id)
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

    // Recent recording deliberately does NOT fire here: NC's primary
    // client (Nextcloud desktop, davx5, mobile NC apps) is a sync
    // engine, and a first-time descent of a large library would push
    // every file into Recent, drowning out the SPA's "what I actually
    // opened" signal. See memory note
    // `project_recent_session_intent.md` — the planned session-intent
    // gate (interactive JWT vs app-password) will turn this back on
    // for human-driven NC web access in the same browser session.

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
    session: &crate::interfaces::nextcloud::session::NcSession,
    subpath: &str,
) -> Result<Response<Body>, AppError> {
    let chroot = session.require_chroot()?;
    // HEAD on root folder — NC clients use this as an existence check
    if subpath.is_empty() || subpath == "/" {
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header("DAV", "1, 3")
            .body(Body::empty())
            .unwrap());
    }

    let internal_path = nc_to_internal_path(chroot, subpath)?;
    let file_service = &state.applications.file_retrieval_service;
    let folder_service = &state.applications.folder_service;

    // Check if path is a folder (NC clients use HEAD as existence check)
    if folder_service
        .get_folder_by_path(&internal_path, chroot.drive_id)
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
        .get_file_by_path(&internal_path, chroot.drive_id)
        .await
        .map_err(|_| AppError::not_found("File not found"))?;

    let modified_at =
        chrono::DateTime::<Utc>::from_timestamp(timestamp_to_i64(file.modified_at), 0)
            .unwrap_or_else(Utc::now);

    // ETag comes from `FileDto::etag` — see the same comment block on
    // the GET handler. HEAD and GET must agree byte-for-byte; pulling
    // both from the same DTO field guarantees that.
    //
    // We deliberately do NOT set `Content-Length: file.size` here even
    // though RFC 7231 §4.3.2 says HEAD SHOULD return the same headers
    // GET would. Our body is `Body::empty()`, so declaring a non-zero
    // Content-Length tells the client "20 bytes are coming" — and on a
    // keep-alive connection the client waits forever for them. Hyper
    // derives `Content-Length: 0` from the empty body, which is honest
    // about what's actually on the wire. Clients that need the file
    // size use PROPFIND (which is what NC and Sabre clients do).
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, file.mime_type.as_ref())
        .header(header::ETAG, format!("\"{}\"", file.etag))
        .header(header::LAST_MODIFIED, modified_at.to_rfc2822())
        .body(Body::empty())
        .unwrap())
}

// ──────────────────── PROPPATCH ────────────────────

async fn handle_proppatch(
    state: Arc<AppState>,
    req: Request<Body>,
    session: &crate::interfaces::nextcloud::session::NcSession,
    subpath: &str,
) -> Result<Response<Body>, AppError> {
    let user = &session.user;
    let chroot = session.require_chroot()?;
    let url_user = &session.raw_username;
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
    let internal_path = nc_to_internal_path(chroot, subpath)?;
    let file_service = &state.applications.file_retrieval_service;
    let folder_service = &state.applications.folder_service;
    let resource = if let Ok(file) = file_service
        .get_file_by_path(&internal_path, chroot.drive_id)
        .await
    {
        Some((file.id, "file"))
    } else if let Ok(folder) = folder_service
        .get_folder_by_path(&internal_path, chroot.drive_id)
        .await
    {
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
        nc_collection_href(url_user, subpath)
    } else {
        nc_href(url_user, subpath)
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

/// Strip the optional `W/` weak prefix and surrounding double-quotes
/// from one ETag value in an `If-Match` / `If-None-Match` list. Returns
/// `(is_weak, inner)`.
fn parse_etag_value(raw: &str) -> (bool, &str) {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("W/") {
        (true, rest.trim().trim_matches('"'))
    } else {
        (false, trimmed.trim_matches('"'))
    }
}

/// RFC 7232 §3.2 — `If-None-Match` fails for PUT when:
///   - the header value is `*` and a current representation exists, OR
///   - any listed ETag matches the current representation (weak comparison
///     — weak validators in the request are equivalent to strong for the
///     match itself, only If-Match is required to be strong).
fn if_none_match_precondition_fails(header: &str, current_etag: Option<&str>) -> bool {
    let v = header.trim();
    if v == "*" {
        return current_etag.is_some();
    }
    let Some(current) = current_etag else {
        return false;
    };
    v.split(',').any(|tag| {
        let (_, parsed) = parse_etag_value(tag);
        !parsed.is_empty() && parsed == current
    })
}

/// RFC 7232 §3.1 — `If-Match` fails for PUT when:
///   - the resource doesn't currently exist (no strong validator to match), OR
///   - the header isn't `*` and no listed ETag strong-matches the current one
///     (weak validators in the request never satisfy a strong-match).
fn if_match_precondition_fails(header: &str, current_etag: Option<&str>) -> bool {
    let v = header.trim();
    let Some(current) = current_etag else {
        return true;
    };
    if v == "*" {
        return false;
    }
    !v.split(',').any(|tag| {
        let (is_weak, parsed) = parse_etag_value(tag);
        !is_weak && !parsed.is_empty() && parsed == current
    })
}

fn precondition_failed_response() -> Response<Body> {
    Response::builder()
        .status(StatusCode::PRECONDITION_FAILED)
        .body(Body::empty())
        .unwrap()
}

async fn handle_put(
    state: Arc<AppState>,
    req: Request<Body>,
    session: &crate::interfaces::nextcloud::session::NcSession,
    subpath: &str,
) -> Result<Response<Body>, AppError> {
    let chroot = session.require_chroot()?;
    let internal_path = nc_to_internal_path(chroot, subpath)?;
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

    // ── Conditional preconditions (RFC 7232 §3.1 / §3.2) ─────────────
    // Evaluated BEFORE body ingestion so a rejected PUT doesn't waste
    // bandwidth or disk I/O on a body the server is going to throw away.
    // The lookup is reused for the create-vs-update distinction below,
    // so this is also free of an extra DB hit.
    let existing = file_service
        .get_file_by_path(&internal_path, chroot.drive_id)
        .await
        .ok();
    let current_etag = existing.as_ref().map(|f| f.etag.as_str());

    if let Some(value) = req
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        && if_none_match_precondition_fails(value, current_etag)
    {
        return Ok(precondition_failed_response());
    }
    if let Some(value) = req
        .headers()
        .get(header::IF_MATCH)
        .and_then(|v| v.to_str().ok())
        && if_match_precondition_fails(value, current_etag)
    {
        return Ok(precondition_failed_response());
    }

    // ── Direct PUT cap ───────────────────────────────────────────────
    // We use `direct_put_max_bytes` (default 1 GiB), not `max_upload_size`
    // (default 10 GB). Larger files must come through the chunked upload
    // protocol (`/dav/uploads/...`) which is resumable on failure and
    // bounded per-request by `chunk_max_bytes`. Trying to stream a
    // multi-GB body through a single PUT is a footgun: a connection drop
    // at 95 % loses everything.
    let max_upload = state.core.config.storage.direct_put_max_bytes;

    // Stream the body straight into the CDC chunk store — never buffer the
    // full upload in RAM and never spool it to disk. Chunking, hashing,
    // dedup checks and MIME sniffing (magic bytes off the first frames)
    // all run while the body arrives; chunks the store already has are
    // never written at all. Shared with the native WebDAV PUT handler.
    //
    // `filename` is owned so we don't hold a borrow of the `subpath` param
    // across the await (which would make the handler future non-Send).
    let filename = filename_from_path(subpath).to_string();
    let ingested = ingest_body_to_cas(
        req.into_body(),
        &state.core.dedup_service,
        &filename,
        &claimed_type,
        max_upload,
    )
    .await?;
    let content_type = ingested.content_type.clone();

    // Distinguish create (201) vs update (204) for the response status,
    // using the lookup already done above for the precondition check.
    let existed = existing.is_some();

    // Single streaming path — handles both update and create internally,
    // swapping the file row onto the already-ingested blob.
    let stored = upload_service
        .update_file_streaming(
            &internal_path,
            chroot.drive_id,
            ingested.stored(),
            &content_type,
            oc_mtime,
            session.user.id,
        )
        .await
        .map_err(|e| AppError::internal_error(format!("Failed to store file: {}", e)))?;

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
    session: &crate::interfaces::nextcloud::session::NcSession,
    subpath: &str,
) -> Result<Response<Body>, AppError> {
    let user = &session.user;
    let chroot = session.require_chroot()?;
    use crate::application::dtos::folder_dto::CreateFolderDto;

    let folder_service = &state.applications.folder_service;
    let internal_path = nc_to_internal_path(chroot, subpath)?;

    // RFC 4918 §9.3.1:
    //   - target already exists                                → 405 Method Not Allowed
    //   - parent collection of the target does NOT exist       → 409 Conflict
    //   - parent exists and target does not                    → 201 Created
    //
    // Previous behaviour effectively performed `mkdir -p` and returned
    // 201 even when intermediate ancestors were missing. Sabre/DAV and
    // the actual NC server both return 409 here, so the legacy
    // auto-create deviated from the reference implementation. NC desktop
    // walks ancestors one MKCOL at a time anyway, so dropping the
    // auto-create doesn't break real clients.

    if folder_service
        .get_folder_by_path(&internal_path, chroot.drive_id)
        .await
        .is_ok()
    {
        return Ok(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body(Body::empty())
            .unwrap());
    }

    let segments: Vec<&str> = subpath.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return Err(AppError::bad_request(
            "MKCOL on the user root is not allowed",
        ));
    }
    let (target_name, parent_segments) = segments.split_last().expect("checked non-empty above");

    // Take POC's `chroot`-based root resolution (drive-aware mount
    // point) but keep HEAD's parent_path lookup pattern — the
    // continuation below uses `get_folder_by_path(&parent_path,
    // user.id)` (user-scoped lookup added in the D0 rewind).
    let user_root = nc_to_internal_path(chroot, "")?;
    let parent_path = if parent_segments.is_empty() {
        user_root.clone()
    } else {
        format!("{}/{}", user_root, parent_segments.join("/"))
    };

    let parent_folder = match folder_service
        .get_folder_by_path(&parent_path, chroot.drive_id)
        .await
    {
        Ok(folder) => folder,
        Err(_) => {
            return Ok(Response::builder()
                .status(StatusCode::CONFLICT)
                .body(Body::empty())
                .unwrap());
        }
    };

    let dto = CreateFolderDto {
        name: target_name.to_string(),
        parent_id: Some(parent_folder.id.clone()),
    };
    folder_service
        .create_folder_with_perms(dto, user.id)
        .await
        .map_err(|e| AppError::internal_error(format!("Failed to create folder: {}", e)))?;

    Ok(Response::builder()
        .status(StatusCode::CREATED)
        .body(Body::empty())
        .unwrap())
}

// ──────────────────── DELETE ────────────────────

async fn handle_delete(
    state: Arc<AppState>,
    session: &crate::interfaces::nextcloud::session::NcSession,
    subpath: &str,
) -> Result<Response<Body>, AppError> {
    let user = &session.user;
    let chroot = session.require_chroot()?;
    let internal_path = nc_to_internal_path(chroot, subpath)?;
    let folder_service = &state.applications.folder_service;
    let file_service = &state.applications.file_retrieval_service;

    // Prefer soft-delete (move to trash) when trash service is available.
    // This is what Nextcloud clients expect — items appear in the trashbin.
    if let Some(trash_svc) = state.trash_service.as_ref() {
        if let Ok(folder) = folder_service
            .get_folder_by_path(&internal_path, chroot.drive_id)
            .await
        {
            trash_svc
                .move_to_trash(&folder.id, "folder", user.id)
                .await
                .map_err(|e| AppError::internal_error(format!("Failed to trash folder: {}", e)))?;
            return Ok(Response::builder()
                .status(StatusCode::NO_CONTENT)
                .body(Body::empty())
                .unwrap());
        }
        if let Ok(file) = file_service
            .get_file_by_path(&internal_path, chroot.drive_id)
            .await
        {
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

    if let Ok(folder) = folder_service
        .get_folder_by_path(&internal_path, chroot.drive_id)
        .await
    {
        folder_service
            .delete_folder_with_perms(&folder.id, user.id)
            .await
            .map_err(|e| AppError::internal_error(format!("Failed to delete folder: {}", e)))?;

        return Ok(Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Body::empty())
            .unwrap());
    }

    if let Ok(file) = file_service
        .get_file_by_path(&internal_path, chroot.drive_id)
        .await
    {
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
    session: &crate::interfaces::nextcloud::session::NcSession,
    subpath: &str,
) -> Result<Response<Body>, AppError> {
    let user = &session.user;
    let chroot = session.require_chroot()?;
    let url_user = &session.raw_username;
    let destination = req
        .headers()
        .get("destination")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::bad_request("Missing Destination header"))?
        .to_string();

    // RFC 4918 §9.9.3: the `Overwrite` header has the default value `T`.
    // `F` MUST cause the request to fail with 412 when the destination
    // already exists; `T` (or absent) MUST replace the destination as if
    // it didn't exist (the response then drops from 201 Created to 204
    // No Content per §9.9.4 because the URI's resource was replaced
    // rather than newly created).
    let overwrite_forbidden = req
        .headers()
        .get("overwrite")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim().eq_ignore_ascii_case("F"))
        .unwrap_or(false);

    // Parse destination path: extract subpath after /remote.php/dav/files/{user}/
    // — the URL user-segment carries the drive marker on multi-drive
    // sessions, so we strip the *composite* prefix to find the real
    // subpath. Using `user.username` here would fail to match for any
    // request hitting a non-home drive.
    let dest_subpath = extract_nc_subpath_from_dest(&destination, url_user)
        .ok_or_else(|| AppError::bad_request("Invalid Destination URL"))?;

    let src_internal = nc_to_internal_path(chroot, subpath)?;
    let folder_service = &state.applications.folder_service;
    let file_service = &state.applications.file_retrieval_service;
    let file_mgmt = &state.applications.file_management_service;

    // ── Destination-collision precondition (RFC 4918 §9.9.4) ──────────
    // Resolved once up-front so the file/folder branches below don't
    // each have to repeat the check. `dest_existed_before` becomes the
    // 204-vs-201 selector at response time.
    let dest_internal_precheck = nc_to_internal_path(chroot, &dest_subpath)?;
    let dest_existing_file = file_service
        .get_file_by_path(&dest_internal_precheck, chroot.drive_id)
        .await
        .ok();
    let dest_existing_folder = folder_service
        .get_folder_by_path(&dest_internal_precheck, chroot.drive_id)
        .await
        .ok();
    let dest_existed_before = dest_existing_file.is_some() || dest_existing_folder.is_some();

    if dest_existed_before {
        if overwrite_forbidden {
            return Ok(Response::builder()
                .status(StatusCode::PRECONDITION_FAILED)
                .body(Body::empty())
                .unwrap());
        }
        // Overwrite: T (or absent) → delete the existing destination first,
        // then proceed with the move. Trashing is fine: per RFC the source
        // resource appears at the destination URI; what happens to the
        // overwritten one is up to the server.
        if let Some(existing_file) = &dest_existing_file {
            file_mgmt
                .delete_and_cleanup_with_perms(&existing_file.id, user.id)
                .await
                .map_err(|e| {
                    AppError::internal_error(format!("Failed to overwrite destination file: {}", e))
                })?;
        } else if let Some(existing_folder) = &dest_existing_folder {
            folder_service
                .delete_folder_with_perms(&existing_folder.id, user.id)
                .await
                .map_err(|e| {
                    AppError::internal_error(format!(
                        "Failed to overwrite destination folder: {}",
                        e
                    ))
                })?;
        }
    }

    let final_status = if dest_existed_before {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::CREATED
    };

    // Try as file first.
    if let Ok(file) = file_service
        .get_file_by_path(&src_internal, chroot.drive_id)
        .await
    {
        let (dest_parent_sub, dest_name) = match dest_subpath.rsplit_once('/') {
            Some((parent, name)) => (parent, name),
            None => ("", dest_subpath.as_str()),
        };
        let dest_parent_internal = nc_to_internal_path(chroot, dest_parent_sub)?;

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
                .get_folder_by_path(&dest_parent_internal, chroot.drive_id)
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
        // Take POC's chroot-based path resolution; keep HEAD's
        // final_status (201 vs 204 depending on whether the destination
        // existed — RFC 4918 §9.9.4 distinguishes create vs overwrite).
        let dest_internal = nc_to_internal_path(chroot, &dest_subpath)?;
        let mut builder = Response::builder().status(final_status);
        if let Ok(moved) = file_service
            .get_file_by_path(&dest_internal, chroot.drive_id)
            .await
        {
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
    if let Ok(folder) = folder_service
        .get_folder_by_path(&src_internal, chroot.drive_id)
        .await
    {
        let (dest_parent_sub, dest_name) = match dest_subpath.rsplit_once('/') {
            Some((parent, name)) => (parent, name),
            None => ("", dest_subpath.as_str()),
        };
        let dest_parent_internal = nc_to_internal_path(chroot, dest_parent_sub)?;

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
                .get_folder_by_path(&dest_parent_internal, chroot.drive_id)
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
            .status(final_status)
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
pub fn extract_nc_subpath_from_dest(dest: &str, username: &str) -> Option<String> {
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
    url_user: &str,
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
    // `url_user` is the wire identifier (may carry a `~{drive}`
    // marker); the NC client validates that the returned `<d:href>`
    // shares the requested URL's prefix. `username` is the canonical
    // identity for the `oc:owner-id` field.
    let href = nc_href(url_user, subpath);
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
    // Route through `FolderDto::etag` (= `Folder::etag()`: the
    // descendant-aware `{id[..16]}-{tree_modified_at}` — see the
    // entity for the formula and the async-bump freshness contract).
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
    //
    // The route glue resolves the `chroot` FolderDto once per request
    // (legacy/home → user's home folder DTO; explicit `~{folder_uuid}` →
    // folder's stored DTO after permission check). These tests cover only
    // the path-mapping function itself; the resolver logic lives in
    // `routes.rs::verify_url_user_and_resolve_chroot`.

    /// Build a stub `FolderDto` carrying only the `path` field (all the
    /// path mapper looks at). Keeps the tests focused on path mapping
    /// without dragging in folder-construction machinery.
    fn stub_folder(path: &str) -> FolderDto {
        FolderDto {
            id: "00000000-0000-0000-0000-000000000000".to_string(),
            name: path.rsplit('/').next().unwrap_or("").to_string(),
            path: path.to_string(),
            parent_id: None,
            owner_id: None,
            // Test stub — path mapper doesn't read drive_id.
            drive_id: uuid::Uuid::nil(),
            created_at: 0,
            modified_at: 0,
            is_root: false,
            icon_class: std::sync::Arc::from("fas fa-folder"),
            icon_special_class: std::sync::Arc::from("folder-icon"),
            category: std::sync::Arc::from("Folder"),
            etag: String::new(),
            // §14 provenance not relevant to path-mapper tests.
            created_by: None,
            updated_by: None,
        }
    }

    #[test]
    fn test_empty_subpath_returns_chroot() {
        let home = stub_folder("My Folder - alice");
        assert_eq!(nc_to_internal_path(&home, "").unwrap(), "My Folder - alice");
    }

    #[test]
    fn test_subpath_appended_to_chroot() {
        let home = stub_folder("My Folder - alice");
        assert_eq!(
            nc_to_internal_path(&home, "Documents/work").unwrap(),
            "My Folder - alice/Documents/work"
        );
    }

    #[test]
    fn test_strips_surrounding_slashes() {
        let home = stub_folder("My Folder - alice");
        assert_eq!(
            nc_to_internal_path(&home, "/Photos/").unwrap(),
            "My Folder - alice/Photos"
        );
    }

    #[test]
    fn test_rejects_dot_dot_traversal() {
        let home = stub_folder("My Folder - alice");
        assert!(nc_to_internal_path(&home, "../etc/passwd").is_err());
    }

    #[test]
    fn test_rejects_single_dot() {
        let home = stub_folder("My Folder - alice");
        assert!(nc_to_internal_path(&home, "foo/./bar").is_err());
    }

    /// Confines a subfolder chroot (the multi-drive form once
    /// resolved). Same path-mapping logic — only the chroot differs.
    #[test]
    fn test_subfolder_chroot_with_subpath() {
        let chroot = stub_folder("My Folder - alice/ext");
        assert_eq!(
            nc_to_internal_path(&chroot, "report.pdf").unwrap(),
            "My Folder - alice/ext/report.pdf"
        );
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
