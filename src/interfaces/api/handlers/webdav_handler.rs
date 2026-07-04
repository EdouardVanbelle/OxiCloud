/**
 * WebDAV Handler Module
 *
 * This module implements the WebDAV protocol (RFC 4918) endpoints for OxiCloud.
 * It provides a complete WebDAV server implementation that allows clients to
 * perform file operations over HTTP, including reading, writing, and manipulating
 * files and directories.
 */
use axum::{
    Router,
    body::{self, Body},
    http::{HeaderName, Request, StatusCode, header},
    response::Response,
};
use bytes::{Buf, Bytes};
use chrono::Utc;
use quick_xml::Writer;
use uuid::Uuid;

use crate::application::adapters::webdav_adapter::{
    LockInfo, PropFindRequest, PropPatchOp, QualifiedName, WebDavAdapter,
};
use crate::application::dtos::file_dto::FileDto;
use crate::application::dtos::folder_dto::FolderDto;
use crate::application::ports::authorization_ports::AuthorizationEngine;
use crate::application::ports::file_ports::FileRetrievalUseCase;
use crate::application::ports::file_ports::{FileManagementUseCase, FileUploadUseCase};
use crate::application::ports::folder_ports::FolderUseCase;
use crate::application::ports::storage_ports::StorageUsagePort;
use crate::application::services::file_retrieval_service::FileRetrievalService;
use crate::application::services::folder_service::FolderService;
use crate::common::di::AppState;
use crate::domain::repositories::drive_repository::DriveRepository;
use crate::domain::services::authorization::{Permission, Resource, Subject};
use crate::infrastructure::services::path_resolver_service::ResolvedResource;
use crate::infrastructure::services::webdav_dead_property_store::{DeadPropertyStore, ResourceRef};
use crate::interfaces::errors::AppError;
use crate::interfaces::middleware::auth::{AuthUser, CurrentUser};
use crate::interfaces::range_requests::{not_modified_response, range_response};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use std::sync::Arc;

/// Characters that MUST NOT be percent-encoded inside a URI path segment.
/// RFC 3986 §3.3 pchar = unreserved / pct-encoded / sub-delims / ":" / "@"
///   unreserved = ALPHA / DIGIT / "-" / "." / "_" / "~"
///   sub-delims = "!" / "$" / "&" / "'" / "(" / ")" / "*" / "+" / "," / ";" / "="
const PATH_SEGMENT_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'!')
    .remove(b'$')
    .remove(b'&')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')')
    .remove(b'*')
    .remove(b'+')
    .remove(b',')
    .remove(b';')
    .remove(b'=')
    .remove(b':')
    .remove(b'@');

/// Percent-encode a single URI path segment (folder/file name).
fn encode_path_segment(segment: &str) -> String {
    utf8_percent_encode(segment, PATH_SEGMENT_ENCODE_SET).to_string()
}

/// Percent-encode a full slash-separated path, encoding each segment individually.
pub(crate) fn encode_uri_path(path: &str) -> String {
    use std::fmt::Write as _;
    // `utf8_percent_encode` returns a `Display` adapter, so write each encoded
    // segment straight into `out` — avoids a String per segment and the joined
    // Vec the previous `.map(...).collect::<Vec<_>>().join("/")` allocated on
    // every PROPFIND href.
    let mut out = String::with_capacity(path.len() + 8);
    for (i, segment) in path.split('/').enumerate() {
        if i > 0 {
            out.push('/');
        }
        let _ = write!(
            out,
            "{}",
            utf8_percent_encode(segment, PATH_SEGMENT_ENCODE_SET)
        );
    }
    out
}

/// Build the `<D:href>` value for a non-collection (file) resource.
///
/// RFC 4918 §5.2 distinguishes collection (folder) URLs from
/// non-collection URLs by a trailing `/`. Files use NO trailing
/// slash. Mirror of [`webdav_collection_href`] — keep both arms
/// of the choice on the same screen so an "is it a file or a
/// folder?" reviewer can verify both branches at once.
fn webdav_href(path: &str) -> String {
    format!("/webdav/{}", encode_uri_path(path))
}

/// Build the `<D:href>` value for a collection (folder) resource.
///
/// Always terminates with `/` — RFC 4918 §5.2 requires collection
/// URLs to end in a slash, and strict WebDAV clients (notably the
/// NextCloud desktop sync engine, which also speaks to this
/// endpoint) abort multi-status parses with
/// `Invalid href "<…>" expected starting with "<requested-url>"`
/// when the response's own-entry href is missing the trailing `/`.
/// PROPPATCH and LOCK responses on folders MUST use this — using
/// [`webdav_href`] for a folder is the bug class this helper
/// exists to prevent.
fn webdav_collection_href(path: &str) -> String {
    let h = webdav_href(path);
    if h.ends_with('/') {
        h
    } else {
        format!("{}/", h)
    }
}

// Create a custom DAV header since it's not in the standard headers
const HEADER_DAV: HeaderName = HeaderName::from_static("dav");
const HEADER_LOCK_TOKEN: HeaderName = HeaderName::from_static("lock-token");
// const HEADER_IF: HeaderName = HeaderName::from_static("if");

/// Maximum body size for XML-based WebDAV requests (PROPFIND, PROPPATCH, LOCK).
/// 1 MB is generous — a typical PROPFIND body is < 1 KB.
const MAX_XML_BODY: usize = 1_048_576;

/// Maximum body size for MKCOL requests (RFC 4918: body must be empty).
const MAX_MKCOL_BODY: usize = 4096;

/// Batch size for streaming PROPFIND — files and folders are fetched in pages
/// of this size to keep memory constant regardless of folder contents.
/// `pub(crate)` so the NextCloud PROPFIND handler streams with the same
/// page size.
pub(crate) const PROPFIND_BATCH_SIZE: i64 = 500;

// ────────────────────────────────────────────────────────────────────────
// Security helpers (Sol.1 — handler-level user extraction & ownership guard)
// ────────────────────────────────────────────────────────────────────────

/// Extract the authenticated [`CurrentUser`] from the request extensions.
///
/// Every mutating or data-returning WebDAV handler **must** call this so
/// that the real `user.id` is available for ownership checks and for the
/// user-scoped `PathResolverService` methods.
fn extract_user(req: &Request<Body>) -> Result<AuthUser, AppError> {
    req.extensions()
        .get::<Arc<CurrentUser>>()
        .cloned()
        .map(AuthUser)
        .ok_or_else(|| AppError::unauthorized("Authentication required"))
}

/**
 * Creates and returns the WebDAV router with all required endpoints.
 *
 * This function sets up all WebDAV method handlers following RFC 4918,
 * mapping HTTP methods to appropriate WebDAV operations.
 *
 * @return Router configured with WebDAV endpoints
 */
pub fn webdav_routes() -> Router<Arc<AppState>> {
    // Three explicit routes to avoid Axum trailing-slash gaps
    // (same pattern used for CalDAV/CardDAV)
    Router::new()
        .route("/webdav/{*path}", axum::routing::any(handle_webdav_methods))
        .route("/webdav/", axum::routing::any(handle_webdav_methods_root))
        .route("/webdav", axum::routing::any(handle_webdav_methods_root))
}

/// Reject paths that contain path-traversal segments (`.` or `..`).
///
/// Although deeper layers (PathResolver, StoragePath) also filter these out,
/// blocking them at the HTTP boundary provides defense-in-depth and ensures
/// no handler ever receives a traversal attempt.
fn reject_path_traversal(path: &str) -> Result<(), AppError> {
    for segment in path.split('/') {
        if segment == ".." || segment == "." {
            return Err(AppError::bad_request(
                "Path must not contain '.' or '..' segments",
            ));
        }
    }
    Ok(())
}

/// Extract the resource path from the request URI, stripping the `/webdav/` prefix
/// and percent-decoding the result so that folder/file names with spaces and
/// special characters match the values stored in the database.
fn extract_webdav_path(uri: &axum::http::Uri) -> String {
    let raw = uri.path();
    let encoded = if let Some(rest) = raw.strip_prefix("/webdav/") {
        rest.trim_end_matches('/')
    } else if raw == "/webdav" {
        ""
    } else {
        // Fallback: split-based extraction
        let trimmed = raw.strip_prefix('/').unwrap_or(raw);
        trimmed.trim_end_matches('/')
    };
    // Decode percent-encoded characters (e.g. %20 → space)
    percent_decode_str(encoded).decode_utf8_lossy().into_owned()
}

async fn handle_webdav_methods_root(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    req: Request<Body>,
) -> Result<Response<Body>, AppError> {
    handle_webdav_dispatch(state, req, String::new()).await
}

async fn handle_webdav_methods(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    req: Request<Body>,
) -> Result<Response<Body>, AppError> {
    let path = extract_webdav_path(req.uri());
    reject_path_traversal(&path)?;
    handle_webdav_dispatch(state, req, path).await
}

/// If `path` doesn't already start with the user's home folder name, prepend
/// the home folder path so downstream services can find the resource in the DB.
/// Returns `None` when the path already includes the prefix or resolution fails.
async fn resolve_webdav_path(state: &Arc<AppState>, user_id: Uuid, path: &str) -> Option<String> {
    let folder_service = &state.applications.folder_service;
    let home_folders = folder_service
        .list_folders_with_perms(None, user_id)
        .await
        .ok()?;
    let home = home_folders.first()?;

    if path.starts_with(&home.name) {
        None // Already prefixed
    } else {
        Some(format!("{}/{}", home.path, path))
    }
}

/// Native WebDAV protocol entry: resolve the caller's default drive
/// once per handler so every downstream path-based lookup
/// (`get_folder_by_path`, `get_file_by_path`, `update_file_streaming`)
/// can pass the same `drive_id` scope.
///
/// Post-D0 `storage.{folders,files}.path` repeats across drives — the
/// scope is mandatory. Native WebDAV today lives in a single-drive
/// surface (one default drive per user), so the lookup is unambiguous.
/// Multi-drive support via path segments (`/webdav/drives/<uuid>/…`)
/// is tracked separately and will derive `drive_id` directly from the
/// URL instead of going through `find_default_for_user`.
async fn resolve_drive_id_for_native_webdav(
    state: &Arc<AppState>,
    user_id: Uuid,
) -> Result<Uuid, AppError> {
    state
        .drive_repo
        .find_default_for_user(user_id)
        .await
        .map(|d| d.drive.id)
        .map_err(|e| AppError::internal_error(format!("Failed to resolve default drive: {:?}", e)))
}

async fn handle_webdav_dispatch(
    state: Arc<AppState>,
    req: Request<Body>,
    path: String,
) -> Result<Response<Body>, AppError> {
    let method = req.method().clone();

    // Translate WebDAV path → DB path by prepending user's home folder
    // prefix when the path doesn't already include it.
    // Extract user_id before any async call to keep the future Send.
    let path = if !path.is_empty() && method.as_str() != "OPTIONS" {
        let user_id = req.extensions().get::<Arc<CurrentUser>>().map(|u| u.id);
        if let Some(uid) = user_id {
            resolve_webdav_path(&state, uid, &path)
                .await
                .unwrap_or(path)
        } else {
            path
        }
    } else {
        path
    };

    match method.as_str() {
        "OPTIONS" => handle_options(path).await,
        "GET" => handle_get(state, req, path).await,
        "HEAD" => handle_head(state, req, path).await,
        "PUT" => handle_put(state, req, path).await,
        "MKCOL" => handle_mkcol(state, req, path).await,
        "DELETE" => handle_delete(state, req, path).await,
        "MOVE" => handle_move(state, req, path).await,
        "COPY" => handle_copy(state, req, path).await,
        "PROPFIND" => handle_propfind(state, req, path).await,
        "PROPPATCH" => handle_proppatch(state, req, path).await,
        "LOCK" => handle_lock(state, req, path).await,
        "UNLOCK" => handle_unlock(state, req, path).await,
        _ => Err(AppError::method_not_allowed(format!(
            "Method not allowed: {}",
            method
        ))),
    }
}

/**
 * Handles OPTIONS requests to advertise WebDAV capabilities.
 *
 * This handler responds with the DAV header indicating WebDAV compliance
 * level and the methods supported by this WebDAV server.
 *
 * @param state The application state containing service dependencies
 * @param path The requested resource path
 * @return HTTP response with appropriate WebDAV headers
 */
async fn handle_options(_path: String) -> Result<Response<Body>, AppError> {
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(HEADER_DAV, "1, 2") // Class 1 and 2 WebDAV support
        .header(
            header::ALLOW,
            "OPTIONS, GET, HEAD, PUT, DELETE, PROPFIND, PROPPATCH, MKCOL, COPY, MOVE, LOCK, UNLOCK",
        )
        .body(Body::empty())
        .unwrap())
}

/**
 * Handles PROPFIND requests to retrieve resource properties.
 *
 * This handler processes WebDAV PROPFIND requests according to RFC 4918,
 * retrieving properties of files and folders in the specified path.
 *
 * **Security hardening (Sol.2):** `Depth: infinity` is rejected with
 * `403 Forbidden` and the RFC 4918 `propfind-finite-depth` precondition
 * error body.  The default depth when the header is absent is `1`.
 *
 * **Streaming response (Sol.3):** For `Depth: 1`, files and sub-folders
 * are fetched in batches of `PROPFIND_BATCH_SIZE` and the XML response
 * is written incrementally to a streaming body.  Memory usage is O(batch)
 * regardless of how many children the folder contains.
 *
 * @param state The application state containing service dependencies
 * @param req   The HTTP request containing the PROPFIND XML body
 * @param path  The requested resource path
 * @return      207 Multi-Status XML response with resource properties
 */
async fn handle_propfind(
    state: Arc<AppState>,
    req: Request<Body>,
    path: String,
) -> Result<Response<Body>, AppError> {
    // ── 1. Extract and validate Depth header ─────────────────────
    let depth = req
        .headers()
        .get("Depth")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("1");

    // RFC 4918 §9.1: servers MAY reject Depth:infinity with 403
    if depth == "infinity" {
        let body = r#"<?xml version="1.0" encoding="utf-8"?>
<D:error xmlns:D="DAV:">
  <D:propfind-finite-depth/>
</D:error>"#;
        return Ok(Response::builder()
            .status(StatusCode::FORBIDDEN)
            .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
            .body(Body::from(body))
            .unwrap());
    }

    // Normalize: anything other than "0" or "1" is treated as "0"
    let depth = match depth {
        "0" | "1" => depth,
        _ => "0",
    };
    let depth_owned = depth.to_string();

    // ── 2. Authenticate ──────────────────────────────────────────
    let user = extract_user(&req)?;

    // Client-facing path for href construction — must be extracted before
    // req.into_body() consumes the request. The `path` parameter already has
    // the home-folder prefix prepended (e.g. `admin/docs`) so it's correct for
    // DB lookups but wrong for WebDAV hrefs (clients see `/webdav/docs`).
    let client_path = extract_webdav_path(req.uri());

    // ── 3. Parse PROPFIND XML body ───────────────────────────────
    let body_bytes = {
        let body = req.into_body();
        body::to_bytes(body, MAX_XML_BODY)
            .await
            .map_err(|e| AppError::bad_request(format!("Failed to read request body: {}", e)))?
    };

    let propfind_request = if body_bytes.is_empty() {
        PropFindRequest {
            prop_find_type: crate::application::adapters::webdav_adapter::PropFindType::AllProp,
        }
    } else {
        WebDavAdapter::parse_propfind(body_bytes.reader()).map_err(|e| {
            AppError::bad_request(format!("Failed to parse PROPFIND request: {}", e))
        })?
    };

    // ── 4. Services ──────────────────────────────────────────────
    let folder_service = state.applications.folder_service.clone();
    let file_retrieval_service = state.applications.file_retrieval_service.clone();

    // Use client-facing path for hrefs so responses match the request URL.
    let base_href = if client_path.is_empty() || client_path == "/" {
        "/webdav/".to_string()
    } else {
        format!("/webdav/{}/", encode_uri_path(&client_path))
    };

    // ── 5. Determine target resource ─────────────────────────────
    if path.is_empty() || path == "/" {
        // Root folder
        let root_folder = FolderDto {
            id: "root".to_string(),
            etag: "root".to_string(),
            name: "".to_string(),
            path: "".to_string(),
            parent_id: None,
            // Synthetic root folder for PROPFIND on `/`; not an
            // actual DB row, so drive_id has no meaningful value.
            drive_id: Uuid::nil(),
            created_at: Utc::now().timestamp() as u64,
            modified_at: Utc::now().timestamp() as u64,
            is_root: true,
            icon_class: Arc::from("fas fa-folder"),
            icon_special_class: Arc::from("folder-icon"),
            category: Arc::from("Folder"),
            // §14 provenance not applicable to the synthetic root.
            created_by: None,
            updated_by: None,
        };

        return build_streaming_propfind_response(
            root_folder,
            None, // folder_id = None → root children
            &depth_owned,
            &base_href,
            propfind_request,
            folder_service,
            file_retrieval_service,
            user.id,
            state.webdav_dead_props.clone(),
        )
        .await;
    }

    // `drive_id` is mandatory post-D0 for path-based lookups. Native
    // WebDAV resolves it once from the caller's default drive and
    // reuses it for the resolver / fallback probes below.
    let drive_id = resolve_drive_id_for_native_webdav(&state, user.id).await?;

    // Single-query path resolution: folder OR file in one DB round-trip.
    //
    // Post-D7 the resolver is drive-scoped (not owner-scoped), so we
    // explicitly `authz.require(Read, …)` on the returned resource
    // before rendering the multistatus. The streaming children of a
    // Folder branch are separately per-item authorised inside
    // `build_streaming_propfind_response` via `_with_perms` service
    // methods.
    if let Some(resolver) = &state.path_resolver {
        match resolver.resolve_path_in_drive(&path, drive_id).await {
            Ok(ResolvedResource::Folder(folder)) => {
                let folder_uuid = Uuid::parse_str(&folder.id)
                    .map_err(|_| AppError::not_found(format!("Resource not found: {}", path)))?;
                state
                    .authorization
                    .require(
                        Subject::User(user.id),
                        Permission::Read,
                        Resource::Folder(folder_uuid),
                    )
                    .await?;
                let folder_id = folder.id.clone();
                return build_streaming_propfind_response(
                    folder,
                    Some(folder_id),
                    &depth_owned,
                    &base_href,
                    propfind_request,
                    folder_service,
                    file_retrieval_service,
                    user.id,
                    state.webdav_dead_props.clone(),
                )
                .await;
            }
            Ok(ResolvedResource::File(file)) => {
                let file_uuid = Uuid::parse_str(&file.id)
                    .map_err(|_| AppError::not_found(format!("Resource not found: {}", path)))?;
                state
                    .authorization
                    .require(
                        Subject::User(user.id),
                        Permission::Read,
                        Resource::File(file_uuid),
                    )
                    .await?;
                let dead_props = file_dead_props(&state, &file).await;
                let file_href = webdav_href(&client_path);
                let mut buf = Vec::with_capacity(1024);
                {
                    let mut xml_writer = Writer::new(&mut buf);
                    WebDavAdapter::write_multistatus_start(&mut xml_writer)
                        .map_err(|e| AppError::internal_error(format!("XML write error: {}", e)))?;
                    WebDavAdapter::write_file_entry_with_dead_props(
                        &mut xml_writer,
                        &file,
                        &propfind_request,
                        &file_href,
                        &dead_props,
                    )
                    .map_err(|e| AppError::internal_error(format!("XML write error: {}", e)))?;
                    WebDavAdapter::write_multistatus_end(&mut xml_writer)
                        .map_err(|e| AppError::internal_error(format!("XML write error: {}", e)))?;
                }
                return Ok(Response::builder()
                    .status(StatusCode::MULTI_STATUS)
                    .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
                    .body(Body::from(buf))
                    .unwrap());
            }
            Err(_) => {}
        }
    } else {
        // Fallback: legacy double-query path when PathResolver is unavailable.
        if let Ok(folder) = folder_service.get_folder_by_path(&path, drive_id).await {
            let folder_uuid = Uuid::parse_str(&folder.id)
                .map_err(|_| AppError::not_found(format!("Resource not found: {}", path)))?;
            state
                .authorization
                .require(
                    Subject::User(user.id),
                    Permission::Read,
                    Resource::Folder(folder_uuid),
                )
                .await?;
            let folder_id = folder.id.clone();
            return build_streaming_propfind_response(
                folder,
                Some(folder_id),
                &depth_owned,
                &base_href,
                propfind_request,
                folder_service,
                file_retrieval_service,
                user.id,
                state.webdav_dead_props.clone(),
            )
            .await;
        }
        if let Ok(file) = file_retrieval_service
            .get_file_by_path(&path, drive_id)
            .await
        {
            let file_uuid = Uuid::parse_str(&file.id)
                .map_err(|_| AppError::not_found(format!("Resource not found: {}", path)))?;
            state
                .authorization
                .require(
                    Subject::User(user.id),
                    Permission::Read,
                    Resource::File(file_uuid),
                )
                .await?;
            let dead_props = file_dead_props(&state, &file).await;
            let file_href = webdav_href(&client_path);
            let mut buf = Vec::with_capacity(1024);
            {
                let mut xml_writer = Writer::new(&mut buf);
                WebDavAdapter::write_multistatus_start(&mut xml_writer)
                    .map_err(|e| AppError::internal_error(format!("XML write error: {}", e)))?;
                WebDavAdapter::write_file_entry_with_dead_props(
                    &mut xml_writer,
                    &file,
                    &propfind_request,
                    &file_href,
                    &dead_props,
                )
                .map_err(|e| AppError::internal_error(format!("XML write error: {}", e)))?;
                WebDavAdapter::write_multistatus_end(&mut xml_writer)
                    .map_err(|e| AppError::internal_error(format!("XML write error: {}", e)))?;
            }
            return Ok(Response::builder()
                .status(StatusCode::MULTI_STATUS)
                .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
                .body(Body::from(buf))
                .unwrap());
        }
    }

    Err(AppError::not_found(format!("Resource not found: {}", path)))
}

/// Builds a streaming 207 Multi-Status PROPFIND response.
///
/// The XML is written incrementally: first the folder itself, then children
/// (sub-folders and files) are fetched in batches of `PROPFIND_BATCH_SIZE`.
/// Each batch is serialised to XML and sent as a chunk, so memory stays
/// constant at O(batch_size) regardless of the total number of children.
#[allow(clippy::too_many_arguments)]
async fn build_streaming_propfind_response(
    folder: FolderDto,
    folder_id: Option<String>,
    depth: &str,
    base_href: &str,
    propfind_request: PropFindRequest,
    folder_service: std::sync::Arc<FolderService>,
    file_retrieval_service: std::sync::Arc<FileRetrievalService>,
    user_id: Uuid,
    dead_props_store: Arc<DeadPropertyStore>,
) -> Result<Response<Body>, AppError> {
    let depth = depth.to_string();
    let base_href = base_href.to_string();
    let propfind_request = Arc::new(propfind_request);

    let stream = async_stream::try_stream! {
        // ── XML header + <D:multistatus> + folder entry ──────────
        //
        // Dead-property lookups key on the resource's stable id, so we
        // pass each FolderDto / FileDto to a small helper that parses
        // its `id` field into a `ResourceRef` and queries the store.
        // The synthetic root folder (id = "root") fails to parse and
        // the helper returns an empty list — correct, since the root
        // has no DB row to anchor properties on.
        let folder_dead = folder_dead_props(&dead_props_store, &folder).await;
        let mut buf = Vec::with_capacity(4096);
        {
            let mut w = Writer::new(&mut buf);
            WebDavAdapter::write_multistatus_start(&mut w)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            WebDavAdapter::write_folder_entry_with_dead_props(&mut w, &folder, &propfind_request, &base_href, &folder_dead)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
        }
        yield Bytes::from(buf);

        // ── Children (only if Depth == 1) ────────────────────────
        if depth == "1" {
            let pagination = crate::application::dtos::pagination::PaginationRequestDto {
                page: 0,
                page_size: PROPFIND_BATCH_SIZE as usize,
            };
            let fid_ref = folder_id.as_deref();

            // Stream sub-folders in pages (user-scoped)
            let mut page = 0usize;
            loop {
                let pag = crate::application::dtos::pagination::PaginationRequestDto {
                    page,
                    page_size: pagination.page_size,
                };
                let result = folder_service
                    .list_folders_paginated_with_perms(fid_ref, user_id, &pag)
                    .await
                    .map_err(|e| std::io::Error::other(e.to_string()))?;

                if result.items.is_empty() {
                    break;
                }

                // Materialise dead-props for the whole page before
                // we start writing — keeps the borrow checker happy
                // (the writer borrows the FolderDto and the dead-props
                // vec for the duration of write_folder_entry_*).
                let mut subfolder_deads = Vec::with_capacity(result.items.len());
                for subfolder in &result.items {
                    subfolder_deads.push(folder_dead_props(&dead_props_store, subfolder).await);
                }

                let mut chunk = Vec::with_capacity(result.items.len() * 800);
                {
                    let mut w = Writer::new(&mut chunk);
                    for (subfolder, child_dead) in result.items.iter().zip(subfolder_deads.iter()) {
                        let href = format!("{}{}/", base_href, encode_path_segment(&subfolder.name));
                        WebDavAdapter::write_folder_entry_with_dead_props(&mut w, subfolder, &propfind_request, &href, child_dead)
                            .map_err(|e| std::io::Error::other(e.to_string()))?;
                    }
                }
                let has_more = result.pagination.has_next;
                yield Bytes::from(chunk);

                if !has_more {
                    break;
                }
                page += 1;
            }

            // Stream files in pages (user-scoped)
            let mut offset: i64 = 0;
            loop {
                let batch: Vec<FileDto> = file_retrieval_service
                    .list_files_batch_with_perms(fid_ref, user_id, offset, PROPFIND_BATCH_SIZE)
                    .await
                    .map_err(|e| std::io::Error::other(e.to_string()))?;

                if batch.is_empty() {
                    break;
                }

                let batch_len = batch.len();
                let mut file_deads = Vec::with_capacity(batch_len);
                for file in &batch {
                    file_deads.push(streamed_file_dead_props(&dead_props_store, file).await);
                }

                let mut chunk = Vec::with_capacity(batch_len * 800);
                {
                    let mut w = Writer::new(&mut chunk);
                    for (file, child_dead) in batch.iter().zip(file_deads.iter()) {
                        let href = format!("{}{}", base_href, encode_path_segment(&file.name));
                        WebDavAdapter::write_file_entry_with_dead_props(&mut w, file, &propfind_request, &href, child_dead)
                            .map_err(|e| std::io::Error::other(e.to_string()))?;
                    }
                }
                yield Bytes::from(chunk);

                if (batch_len as i64) < PROPFIND_BATCH_SIZE {
                    break;
                }
                offset += batch_len as i64;
            }
        }

        // ── Close </D:multistatus> ───────────────────────────────
        let mut buf = Vec::with_capacity(32);
        {
            let mut w = Writer::new(&mut buf);
            WebDavAdapter::write_multistatus_end(&mut w)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
        }
        yield Bytes::from(buf);
    };

    use futures::TryStreamExt;
    let stream = stream
        .map_err(|e: std::io::Error| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) });

    Ok(Response::builder()
        .status(StatusCode::MULTI_STATUS)
        .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
        .body(Body::from_stream(stream))
        .unwrap())
}

/**
 * Handles PROPPATCH requests to set or remove resource properties.
 *
 * This handler processes WebDAV PROPPATCH requests according to RFC 4918,
 * modifying properties of files and folders in the specified path.
 *
 * @param state The application state containing service dependencies
 * @param user The authenticated user information
 * @param path The requested resource path
 * @param req The HTTP request containing the PROPPATCH XML body
 * @return XML response with property modification results
 */
async fn handle_proppatch(
    state: Arc<AppState>,
    req: Request<Body>,
    path: String,
) -> Result<Response<Body>, AppError> {
    let user = extract_user(&req)?;
    // Client-facing path for href construction (without home folder prefix).
    let client_path = extract_webdav_path(req.uri());

    // Active-lock guard (RFC 4918 §9.10.4): PROPPATCH writes properties,
    // so a lock on the target must release them via `If:`. Captured
    // before the body is consumed below so a rejected request doesn't
    // even parse the XML.
    let if_header_owned = req
        .headers()
        .get("If")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    if let Some(resp) = enforce_native_lock(
        &state.webdav_lock_store,
        if_header_owned.as_deref(),
        &path,
        None,
    ) {
        return Ok(resp);
    }

    // Resolve the target resource BEFORE consuming the body. We need
    // the resolved kind for two reasons:
    //
    //   1. The store key is the resource id (folder_id XOR file_id)
    //      after migration 20260830000001; we need to know which one
    //      to bind into `ResourceRef`.
    //   2. The href shape in the multi-status response differs for
    //      collections vs leaves — RFC 4918 §5.2 + strict WebDAV-
    //      client parser rules require a trailing `/` for collection
    //      hrefs, and emitting `/webdav/foo` for a folder breaks
    //      NC-desktop / Cyberduck / other multi-status consumers.
    //
    // PROPPATCH on a non-existent resource returns 404. This is a
    // tighter contract than the pre-rekey code, which silently
    // wrote a dead-prop row keyed by the ghost path — that was a
    // foot-gun, not a feature.
    let (resource_ref, is_collection) = if path.is_empty() || path == "/" {
        // The synthetic root has no DB row to anchor properties on.
        // Treat it as a collection for href shaping; reject the
        // PROPPATCH itself below so we don't fabricate a target.
        (None, true)
    } else {
        match resolve_or_legacy(&state, &path, user.id).await {
            Some(ResolvedResource::Folder(folder)) => {
                let id = Uuid::parse_str(&folder.id).map_err(|e| {
                    AppError::internal_error(format!("Folder id is not a UUID: {e}"))
                })?;
                (Some(ResourceRef::Folder(id)), true)
            }
            Some(ResolvedResource::File(file)) => {
                let id = Uuid::parse_str(&file.id)
                    .map_err(|e| AppError::internal_error(format!("File id is not a UUID: {e}")))?;
                (Some(ResourceRef::File(id)), false)
            }
            None => return Err(AppError::not_found(format!("Resource not found: {}", path))),
        }
    };
    let resource_ref = resource_ref
        .ok_or_else(|| AppError::forbidden("PROPPATCH on the WebDAV root is not supported"))?;

    // Read request body (XML — bounded to 1 MB)
    let body_bytes = body::to_bytes(req.into_body(), MAX_XML_BODY)
        .await
        .map_err(|e| {
            AppError::payload_too_large(format!("PROPPATCH body too large or unreadable: {}", e))
        })?;
    let ops = WebDavAdapter::parse_proppatch(body_bytes.reader())
        .map_err(|e| AppError::bad_request(format!("Failed to parse PROPPATCH request: {}", e)))?;

    // Apply operations in document order (RFC 4918 §9.2).
    let dead_props = &state.webdav_dead_props;
    let mut results: Vec<(&QualifiedName, bool)> = Vec::new();
    for op in &ops {
        match op {
            PropPatchOp::Set(pv) => {
                dead_props
                    .set(resource_ref, pv.name.clone(), pv.value.clone())
                    .await
                    .map_err(|e| {
                        AppError::internal_error(format!("Failed to store dead property: {e}"))
                    })?;
                results.push((&pv.name, true));
            }
            PropPatchOp::Remove(name) => {
                dead_props.remove(resource_ref, name).await.map_err(|e| {
                    AppError::internal_error(format!("Failed to remove dead property: {e}"))
                })?;
                results.push((name, true));
            }
        }
    }

    // Generate response — use client-facing path so href matches the request URL.
    let href = if is_collection {
        webdav_collection_href(&client_path)
    } else {
        webdav_href(&client_path)
    };
    let mut response_body = Vec::new();
    WebDavAdapter::generate_proppatch_response(&mut response_body, &href, &results).map_err(
        |e| AppError::internal_error(format!("Failed to generate PROPPATCH response: {}", e)),
    )?;

    Ok(Response::builder()
        .status(StatusCode::MULTI_STATUS)
        .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
        .body(Body::from(response_body))
        .unwrap())
}

/**
 * Handles GET requests to retrieve file contents.
 *
 * This handler retrieves the contents of a file at the specified path.
 *
 * @param state The application state containing service dependencies
 * @param user The authenticated user information
 * @param path The requested resource path
 * @return HTTP response with file contents
 */
async fn handle_get(
    state: Arc<AppState>,
    req: Request<Body>,
    path: String,
) -> Result<Response<Body>, AppError> {
    let user = extract_user(&req)?;

    // Get file service from state
    let file_retrieval_service = &state.applications.file_retrieval_service;

    // Check if path is empty (root folder)
    if path.is_empty() || path == "/" {
        return Err(AppError::bad_request("Cannot GET a directory"));
    }

    // `drive_id` is the path-lookup scope post-D0 (paths repeat across
    // drives), derived once from the caller's default drive and reused
    // by both the resolver + legacy fallback.
    let drive_id = resolve_drive_id_for_native_webdav(&state, user.id).await?;

    // Resolve file — drive-scoped when PathResolver is available.
    // Post-D7 both branches enforce `Read` on the resolved file
    // explicitly. The download stream call below also passes
    // `caller_id`, so `get_file_stream_with_perms` re-verifies as
    // defence-in-depth.
    //
    // (Fix, 2026-07-02: the legacy fallback branch previously used
    // `Permission::Update`, a stale mapping from the retired
    // `assert_owner` helper. That would have locked Viewers out of
    // downloads once shared drives were exposed via WebDAV. Both
    // branches now share the correct `Read` permission — see the
    // post-D7 second AuthZ audit memo.)
    let file = if let Some(resolver) = &state.path_resolver {
        match resolver.resolve_path_in_drive(&path, drive_id).await {
            Ok(ResolvedResource::File(f)) => {
                let file_uuid = Uuid::parse_str(&f.id)
                    .map_err(|_| AppError::not_found(format!("File not found: {}", path)))?;
                state
                    .authorization
                    .require(
                        Subject::User(user.id),
                        Permission::Read,
                        Resource::File(file_uuid),
                    )
                    .await?;
                f
            }
            Ok(ResolvedResource::Folder(_)) => {
                return Err(AppError::bad_request("Cannot GET a directory"));
            }
            Err(_) => {
                return Err(AppError::not_found(format!("File not found: {}", path)));
            }
        }
    } else {
        // Legacy fallback — fetch + AuthZ check.
        let f = file_retrieval_service
            .get_file_by_path(&path, drive_id)
            .await
            .map_err(|_e| AppError::not_found(format!("File not found: {}", path)))?;
        let file_uuid = Uuid::parse_str(&f.id)
            .map_err(|_| AppError::not_found(format!("File not found: {}", path)))?;
        state
            .authorization
            .require(
                Subject::User(user.id),
                Permission::Read,
                Resource::File(file_uuid),
            )
            .await?;
        f
    };

    let etag = format!("\"{}\"", file.etag);

    // Conditional GET — clients revalidating a cached copy get a 304
    // instead of the full body.
    if let Some(resp) = not_modified_response(req.headers(), &etag) {
        return Ok(resp);
    }

    // Recent recording deliberately does NOT fire here: native WebDAV
    // is overwhelmingly a sync-engine surface (rclone, davfs2, Finder
    // mounts) and a first descent would push every synced file into
    // Recent, drowning out the SPA's "what I actually opened" signal.
    // See memory note `project_recent_session_intent.md` — the planned
    // session-intent gate (interactive JWT vs app-password) will turn
    // this back on for the rare human-driven DAV access.

    // Range Requests — mount-style clients (rclone, davfs2, Finder) read
    // by ranges; serve 206/416 instead of re-sending the whole file on
    // every seek or resume.
    if let Some(resp) = range_response(req.headers(), &file, &etag, file_retrieval_service).await {
        return Ok(resp);
    }

    // Stream file content — constant ~64 KB memory regardless of file size
    let stream = file_retrieval_service
        .get_file_stream(&file.id)
        .await
        .map_err(|e| AppError::internal_error(format!("Failed to stream file: {}", e)))?;

    // Build streaming response using Content-Length from metadata
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, &*file.mime_type)
        .header(header::CONTENT_LENGTH, file.size)
        .header(header::ETAG, etag)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(
            header::LAST_MODIFIED,
            chrono::DateTime::<Utc>::from_timestamp(file.created_at as i64, 0)
                .unwrap_or_else(Utc::now)
                .to_rfc2822(),
        )
        .body(Body::from_stream(Box::into_pin(stream)))
        .unwrap())
}

/**
 * Handles HEAD requests — same as GET but returns only headers, no body.
 */
async fn handle_head(
    state: Arc<AppState>,
    req: Request<Body>,
    path: String,
) -> Result<Response<Body>, AppError> {
    let user = extract_user(&req)?;
    let file_retrieval_service = &state.applications.file_retrieval_service;
    let folder_service = &state.applications.folder_service;

    if path.is_empty() || path == "/" {
        // Root folder — return collection headers
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "httpd/unix-directory")
            .header(header::CONTENT_LENGTH, 0)
            .body(Body::empty())
            .unwrap());
    }

    // `drive_id` is the path-lookup scope post-D0 — derive once and
    // reuse across the resolver + fallback branches below.
    let drive_id = resolve_drive_id_for_native_webdav(&state, user.id).await?;

    // Single-query path resolution (drive-scoped). Both branches
    // enforce `Read` on the resolved resource before emitting the
    // metadata response — same permission as the legacy fallback below.
    if let Some(resolver) = &state.path_resolver {
        match resolver.resolve_path_in_drive(&path, drive_id).await {
            Ok(ResolvedResource::Folder(folder)) => {
                let folder_uuid = Uuid::parse_str(&folder.id)
                    .map_err(|_| AppError::not_found(format!("Resource not found: {}", path)))?;
                state
                    .authorization
                    .require(
                        Subject::User(user.id),
                        Permission::Read,
                        Resource::Folder(folder_uuid),
                    )
                    .await?;
                return Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "httpd/unix-directory")
                    .header(header::CONTENT_LENGTH, 0)
                    .header(header::ETAG, format!("\"{}\"", folder.etag))
                    .body(Body::empty())
                    .unwrap());
            }
            Ok(ResolvedResource::File(file)) => {
                let file_uuid = Uuid::parse_str(&file.id)
                    .map_err(|_| AppError::not_found(format!("Resource not found: {}", path)))?;
                state
                    .authorization
                    .require(
                        Subject::User(user.id),
                        Permission::Read,
                        Resource::File(file_uuid),
                    )
                    .await?;
                return Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, &*file.mime_type)
                    .header(header::CONTENT_LENGTH, file.size)
                    .header(header::ETAG, format!("\"{}\"", file.etag))
                    .header(
                        header::LAST_MODIFIED,
                        chrono::DateTime::<Utc>::from_timestamp(file.created_at as i64, 0)
                            .unwrap_or_else(Utc::now)
                            .to_rfc2822(),
                    )
                    .body(Body::empty())
                    .unwrap());
            }
            Err(_) => return Err(AppError::not_found(format!("Resource not found: {}", path))),
        }
    }

    // Fallback: legacy double-query path (with ownership check).
    if let Ok(folder) = folder_service.get_folder_by_path(&path, drive_id).await {
        let folder_uuid = Uuid::parse_str(&folder.id)
            .map_err(|_| AppError::not_found(format!("Resource not found: {}", path)))?;
        state
            .authorization
            .require(
                Subject::User(user.id),
                Permission::Read,
                Resource::Folder(folder_uuid),
            )
            .await?;
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "httpd/unix-directory")
            .header(header::CONTENT_LENGTH, 0)
            .header(header::ETAG, format!("\"{}\"", folder.etag))
            .body(Body::empty())
            .unwrap());
    }

    // Try as file — use metadata only, never load content for HEAD
    let file = file_retrieval_service
        .get_file_by_path(&path, drive_id)
        .await
        .map_err(|_e| AppError::not_found(format!("Resource not found: {}", path)))?;
    let file_uuid = Uuid::parse_str(&file.id)
        .map_err(|_| AppError::not_found(format!("Resource not found: {}", path)))?;
    state
        .authorization
        .require(
            Subject::User(user.id),
            Permission::Read,
            Resource::File(file_uuid),
        )
        .await?;

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, &*file.mime_type)
        .header(header::CONTENT_LENGTH, file.size)
        .header(header::ETAG, format!("\"{}\"", file.etag))
        .header(
            header::LAST_MODIFIED,
            chrono::DateTime::<Utc>::from_timestamp(file.created_at as i64, 0)
                .unwrap_or_else(Utc::now)
                .to_rfc2822(),
        )
        .body(Body::empty())
        .unwrap())
}

/// Resolve `path` to a user-owned resource using the optimized
/// PathResolver first, falling back to the legacy `get_folder_by_path` /
/// `get_file_by_path` lookups (the same ones GET uses) when the
/// optimized resolver returns NotFound.
///
/// **Why the fallback exists**: the optimized resolver and the read-side
/// `get_*_by_path` repositories don't always agree on what a "path"
/// looks like. The drive-refactor migration rewrote the `path` column
/// to strip the `My Folder - <user>/` prefix that the WebDAV dispatcher
/// (`resolve_webdav_path`) still prepends — leaving an inconsistency
/// where files PUT through the WebDAV surface stay reachable by GET
/// (legacy lookup) but invisible to the optimized resolver (strict
/// path-match). MOVE / DELETE / COPY previously 404'd on every
/// root-level file because they only used the optimized resolver.
///
/// Cross-drive isolation is enforced in both branches: the optimized
/// resolver scopes by `drive_id`; the fallback derives the caller's
/// default `drive_id` and `get_folder_by_path` / `get_file_by_path`
/// scope by that. Callsites in the fallback additionally run
/// `authz.require(Permission::…, Resource::…)` per operation, matching
/// the modern AuthZ path.
async fn resolve_or_legacy(
    state: &Arc<AppState>,
    path: &str,
    user_id: Uuid,
) -> Option<ResolvedResource> {
    // Path-lookup scope post-D0 — derive the caller's default drive
    // once and reuse across both probes. `find_default_for_user`
    // returning Err (e.g. external user, or boot before the lifecycle
    // hook fired) means no resolution is possible: return None.
    let drive_id = state
        .drive_repo
        .find_default_for_user(user_id)
        .await
        .ok()?
        .drive
        .id;

    if let Some(resolver) = &state.path_resolver
        && let Ok(r) = resolver.resolve_path_in_drive(path, drive_id).await
    {
        return Some(r);
    }

    let folder_service = &state.applications.folder_service;
    if let Ok(folder) = folder_service.get_folder_by_path(path, drive_id).await {
        return Some(ResolvedResource::Folder(folder));
    }
    let file_retrieval = &state.applications.file_retrieval_service;
    if let Ok(file) = file_retrieval.get_file_by_path(path, drive_id).await {
        return Some(ResolvedResource::File(file));
    }
    None
}

/// Fetch a file's dead properties for a PROPFIND response.
///
/// Lenient on every failure mode: malformed id, DB error → empty list.
/// PROPFIND must still emit the resource's live properties even when
/// the dead-prop lookup is broken; surfacing a 500 here would mask the
/// resource entirely from sync clients. The legacy path-keyed lookup
/// behaved the same way (`.unwrap_or_default()`); we preserve it.
async fn file_dead_props(
    state: &Arc<AppState>,
    file: &FileDto,
) -> Vec<(QualifiedName, Option<String>)> {
    let Ok(file_id) = Uuid::parse_str(&file.id) else {
        return Vec::new();
    };
    state
        .webdav_dead_props
        .get_all(ResourceRef::File(file_id))
        .await
        .unwrap_or_default()
}

/// Same shape as `file_dead_props` but for folder rows. Used by the
/// streaming PROPFIND walker.
async fn folder_dead_props(
    store: &DeadPropertyStore,
    folder: &FolderDto,
) -> Vec<(QualifiedName, Option<String>)> {
    let Ok(folder_id) = Uuid::parse_str(&folder.id) else {
        return Vec::new();
    };
    store
        .get_all(ResourceRef::Folder(folder_id))
        .await
        .unwrap_or_default()
}

/// File-leaf variant for the streaming walker (takes a `&DeadPropertyStore`
/// rather than the full `&Arc<AppState>` so it can be called from inside
/// the async-stream future without cloning state).
async fn streamed_file_dead_props(
    store: &DeadPropertyStore,
    file: &FileDto,
) -> Vec<(QualifiedName, Option<String>)> {
    let Ok(file_id) = Uuid::parse_str(&file.id) else {
        return Vec::new();
    };
    store
        .get_all(ResourceRef::File(file_id))
        .await
        .unwrap_or_default()
}

/// A single condition inside a `List` of the WebDAV `If:` header
/// (RFC 4918 §10.4.2 grammar):
/// `Condition = ["Not"] (State-token | "[" entity-tag "]")`.
#[derive(Debug, Clone, PartialEq)]
enum IfCondition {
    StateToken { negated: bool, token: String },
    EntityTag { negated: bool, etag: String },
}

/// A parsed `If:` header — outer Vec is OR of `List`s, inner Vec is AND
/// of `Condition`s (RFC 4918 §10.4.2). Tagged-list `Resource` URIs are
/// accepted by the parser but their scoping is ignored — every List is
/// treated as applying to the current request URI. That's a
/// simplification adequate for litmus and NC clients; a real Tagged-list
/// implementation would map each List to its preceding Resource.
type IfLists = Vec<Vec<IfCondition>>;

/// Best-effort parser for RFC 4918 §10.4.2 `If:` headers. Malformed
/// input yields whatever Lists could be recovered; downstream evaluation
/// treats an empty result as "no header".
fn parse_if_header(header: &str) -> IfLists {
    let mut lists: IfLists = Vec::new();
    let bytes = header.as_bytes();
    let n = bytes.len();
    let mut i = 0;

    while i < n {
        match bytes[i] {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'<' => {
                // Tagged-list Resource prefix — skip past the closing '>'.
                i += 1;
                while i < n && bytes[i] != b'>' {
                    i += 1;
                }
                if i < n {
                    i += 1;
                }
            }
            b'(' => {
                i += 1;
                let mut list: Vec<IfCondition> = Vec::new();
                loop {
                    while i < n && matches!(bytes[i], b' ' | b'\t' | b'\r' | b'\n') {
                        i += 1;
                    }
                    if i >= n {
                        break;
                    }
                    if bytes[i] == b')' {
                        i += 1;
                        break;
                    }

                    // Optional "Not" prefix.
                    let mut negated = false;
                    if i + 3 <= n
                        && bytes[i..i + 3].eq_ignore_ascii_case(b"Not")
                        && (i + 3 == n
                            || matches!(bytes[i + 3], b' ' | b'\t' | b'<' | b'[' | b'\r' | b'\n'))
                    {
                        negated = true;
                        i += 3;
                        while i < n && matches!(bytes[i], b' ' | b'\t' | b'\r' | b'\n') {
                            i += 1;
                        }
                    }
                    if i >= n {
                        break;
                    }

                    match bytes[i] {
                        b'<' => {
                            i += 1;
                            let start = i;
                            while i < n && bytes[i] != b'>' {
                                i += 1;
                            }
                            let token = std::str::from_utf8(&bytes[start..i])
                                .unwrap_or_default()
                                .to_string();
                            if i < n {
                                i += 1;
                            }
                            list.push(IfCondition::StateToken { negated, token });
                        }
                        b'[' => {
                            i += 1;
                            let start = i;
                            while i < n && bytes[i] != b']' {
                                i += 1;
                            }
                            let etag = std::str::from_utf8(&bytes[start..i])
                                .unwrap_or_default()
                                .trim()
                                .trim_matches('"')
                                .to_string();
                            if i < n {
                                i += 1;
                            }
                            list.push(IfCondition::EntityTag { negated, etag });
                        }
                        _ => {
                            // Malformed — skip a byte and continue trying.
                            i += 1;
                        }
                    }
                }
                if !list.is_empty() {
                    lists.push(list);
                }
            }
            _ => i += 1,
        }
    }
    lists
}

/// Evaluate a parsed `If:` header against the current resource state.
///
/// Returns `(header_true, submitted_active_lock)`:
/// - `header_true` — at least one `List` (AND of Conditions) evaluates
///   true, so the OR-across-Lists holds and the precondition passes.
/// - `submitted_active_lock` — some non-negated `State-token` Condition
///   presented the resource's actual lock token. Used to distinguish
///   412 (precondition failed) from 423 (locked resource, no matching
///   token submitted) per RFC 4918 §10.4.9 / §6.6.
///
/// Empty `lists` (parse failed / header absent) → treated as
/// vacuously true. Callers handle the "no If: header on a locked
/// resource" case separately.
fn evaluate_if_header(
    lists: &IfLists,
    active_lock_token: Option<&str>,
    current_etag: Option<&str>,
) -> (bool, bool) {
    if lists.is_empty() {
        return (true, false);
    }

    // First pass — scan every non-negated State-token so we can flag
    // "the caller did submit the lock" even for Lists that fail on other
    // Conditions. This drives the 412-vs-423 discrimination downstream.
    let mut submitted_active_lock = false;
    if let Some(active) = active_lock_token {
        for list in lists {
            for cond in list {
                if let IfCondition::StateToken {
                    negated: false,
                    token,
                } = cond
                    && token == active
                {
                    submitted_active_lock = true;
                }
            }
        }
    }

    // Second pass — AND within each List, OR across Lists.
    let any_list_true = lists.iter().any(|list| {
        list.iter().all(|cond| {
            let (negated, natural) = match cond {
                IfCondition::StateToken { negated, token } => {
                    let is_active = active_lock_token == Some(token.as_str());
                    (*negated, is_active)
                }
                IfCondition::EntityTag {
                    negated,
                    etag: cond_etag,
                } => {
                    let matches = current_etag
                        .map(|c| c.trim().trim_matches('"') == cond_etag.trim().trim_matches('"'))
                        .unwrap_or(false);
                    (*negated, matches)
                }
            };
            natural ^ negated
        })
    });

    (any_list_true, submitted_active_lock)
}

/// RFC 4918 §9.10.4 / §10.4 — evaluate the caller's `If:` header
/// against the resource's active lock and current ETag.
///
/// Returns `Some(Response)` when the request must be rejected —
/// **412 Precondition Failed** when the header's conditions can't be
/// satisfied by the current state, **423 Locked** when the resource is
/// locked and no submitted alternative includes its lock token (per
/// §10.4.9 / §6.6). Returns `None` when the request may proceed.
///
/// `current_etag` is the resource's ETag (raw, unquoted) if it exists;
/// pass `None` for a not-yet-existing target. Callers that don't have
/// the resolved etag at the call site pass `None` — any `[etag]`
/// condition then evaluates false, which is the correct fail-closed
/// behaviour for the "resource doesn't exist yet" case.
///
/// Shared by `handle_put`, `handle_delete`, `handle_move`,
/// `handle_copy`, and `handle_proppatch`.
fn enforce_native_lock(
    lock_store: &crate::infrastructure::services::webdav_lock_service::WebDavLockStore,
    if_header: Option<&str>,
    path: &str,
    current_etag: Option<&str>,
) -> Option<Response<Body>> {
    // Check the exact path, then walk up parent collections for depth-infinity
    // locks (RFC 4918 §6.1: a lock on a collection with Depth: infinity also
    // covers all descendant members).
    let entry = lock_store.get_by_path(path).or_else(|| {
        let mut p = path;
        loop {
            let idx = p.rfind('/')?;
            p = &p[..idx];
            if p.is_empty() {
                return None;
            }
            if let Some(e) = lock_store.get_by_path(p)
                && e.info.depth.eq_ignore_ascii_case("infinity")
            {
                return Some(e);
            }
        }
    });

    let active_lock_token = entry.as_ref().map(|e| e.info.token.as_str());
    let locked = entry.is_some();

    let lists = if_header.map(parse_if_header).unwrap_or_default();

    // No parseable If: header at all.
    if lists.is_empty() {
        // Locked without an If: header → 423 Locked (§9.10.4).
        if locked {
            return Some(
                Response::builder()
                    .status(StatusCode::LOCKED)
                    .body(Body::empty())
                    .unwrap(),
            );
        }
        return None;
    }

    let (header_true, submitted_active_lock) =
        evaluate_if_header(&lists, active_lock_token, current_etag);

    if header_true {
        // Header preconditions satisfied. If the resource is locked but
        // the satisfying List did so via etag / Not-token alone (never
        // presenting the real lock token), still return 423 — §10.4.9's
        // "matching lock token" rule.
        if locked && !submitted_active_lock {
            return Some(
                Response::builder()
                    .status(StatusCode::LOCKED)
                    .body(Body::empty())
                    .unwrap(),
            );
        }
        return None;
    }

    // Header evaluates false. 423 if the resource is locked and the
    // caller never presented its lock token; otherwise 412.
    if locked && !submitted_active_lock {
        return Some(
            Response::builder()
                .status(StatusCode::LOCKED)
                .body(Body::empty())
                .unwrap(),
        );
    }
    Some(
        Response::builder()
            .status(StatusCode::PRECONDITION_FAILED)
            .body(Body::empty())
            .unwrap(),
    )
}

/**
 * Handles PUT requests to create or update files.
 *
 * **Streaming implementation**: the request body is streamed straight into
 * the CDC chunk store (FastCDC + BLAKE3 while the bytes arrive — no spool
 * file, no re-read; peak RAM is bounded regardless of file size), then the
 * file row is atomically swapped onto the ingested blob via
 * `update_file_streaming`.
 *
 * @param state The application state containing service dependencies
 * @param path The requested resource path
 * @param req The HTTP request containing the file contents
 * @return HTTP response indicating success
 */
async fn handle_put(
    state: Arc<AppState>,
    req: Request<Body>,
    path: String,
) -> Result<Response<Body>, AppError> {
    use crate::interfaces::upload_ingest;

    let user = extract_user(&req)?;

    let file_upload_service = &state.applications.file_upload_service;

    if path.is_empty() || path == "/" {
        return Err(AppError::bad_request("Cannot PUT to root folder"));
    }

    // RFC 4918 §9.7.1: a server MUST NOT partially CREATE or UPDATE a resource
    // based on a PUT request containing a Content-Range header.
    if req.headers().contains_key(header::CONTENT_RANGE) {
        return Err(AppError::bad_request(
            "PUT with Content-Range is not allowed (RFC 4918 §9.7.1)",
        ));
    }

    // Extract all headers before consuming `req` into the body stream.
    let if_header_owned = req
        .headers()
        .get("If")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let if_none_match = req
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string());
    let if_match = req
        .headers()
        .get(header::IF_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string());
    let content_type = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let max_upload = state.core.config.storage.direct_put_max_bytes;

    // `drive_id` is the path-lookup scope post-D0 — resolve once from
    // the caller's default drive, reused by the resolver checks below
    // and by the atomic-store call further down.
    let drive_id = resolve_drive_id_for_native_webdav(&state, user.id).await?;

    // ── Existence check ───────────────────────────────────────────────
    // Resolves to: File(existing), Folder(wrong), or Err(new file).
    // Sets `file_existed` for 201 vs 204 and `current_etag` for If-Match.
    //
    // Post-D7 the resolver is drive-scoped, not owner-scoped, so we
    // explicitly `authz.require(Read, …)` on every returned resource
    // before consuming it. The actual overwrite is authorised as
    // `Update` inside `update_file_streaming`; this Read check is the
    // defence-in-depth existence-proof (see project_webdav_authz_second_audit).
    let mut file_existed = false;
    let mut current_etag: Option<String> = None;
    if let Some(resolver) = &state.path_resolver {
        match resolver.resolve_path_in_drive(&path, drive_id).await {
            Ok(ResolvedResource::File(f)) => {
                let file_uuid = Uuid::parse_str(&f.id)
                    .map_err(|_| AppError::not_found(format!("File not found: {}", path)))?;
                state
                    .authorization
                    .require(
                        Subject::User(user.id),
                        Permission::Read,
                        Resource::File(file_uuid),
                    )
                    .await?;
                file_existed = true;
                current_etag = Some(f.etag.clone());
            }
            Ok(ResolvedResource::Folder(_)) => {
                return Err(AppError::bad_request("Cannot PUT to a directory"));
            }
            Err(_) => {
                // File doesn't exist — verify parent. RFC 4918 §9.7.1: missing
                // parent MUST produce 409 Conflict, not 404.
                let parent_path = path.rfind('/').map(|i| &path[..i]).unwrap_or("");
                if !parent_path.is_empty() {
                    let parent = resolver
                        .resolve_path_in_drive(parent_path, drive_id)
                        .await
                        .map_err(|_| {
                            AppError::conflict(format!("Parent folder not found: {}", parent_path))
                        })?;
                    if let ResolvedResource::Folder(folder) = parent {
                        let folder_uuid = Uuid::parse_str(&folder.id).map_err(|_| {
                            AppError::conflict(format!("Parent folder not found: {}", parent_path))
                        })?;
                        state
                            .authorization
                            .require(
                                Subject::User(user.id),
                                Permission::Read,
                                Resource::Folder(folder_uuid),
                            )
                            .await
                            .map_err(|_| {
                                AppError::conflict(format!(
                                    "Parent folder not found: {}",
                                    parent_path
                                ))
                            })?;
                    } else {
                        return Err(AppError::conflict(format!(
                            "Parent path is a file, not a collection: {}",
                            parent_path
                        )));
                    }
                }
            }
        }
    }

    // ── Active-lock guard + RFC 4918 §10.4 If: evaluation ─────────────
    // Deferred until after the existence check so `current_etag` is
    // available to the If: header's `[etag]` conditions. `handle_put`
    // is the only site where litmus exercises the full §10.4 grammar
    // (locks/fail_complex_cond_put); other handlers pass `None` and
    // fall back to the existence-only semantics.
    if let Some(resp) = enforce_native_lock(
        &state.webdav_lock_store,
        if_header_owned.as_deref(),
        &path,
        current_etag.as_deref(),
    ) {
        return Ok(resp);
    }

    // ── RFC 7232 conditional preconditions ────────────────────────────
    // Evaluated before ingesting the body to save bandwidth on doomed requests.
    if let Some(ref inm) = if_none_match {
        // If-None-Match: * → fail if resource exists (prevent overwrite)
        if inm == "*" && file_existed {
            return Err(AppError::precondition_failed(
                "If-None-Match: * — resource already exists",
            ));
        }
    }
    if let Some(ref im) = if_match {
        if im == "*" {
            // If-Match: * → fail if resource does not exist
            if !file_existed {
                return Err(AppError::precondition_failed(
                    "If-Match: * — resource does not exist",
                ));
            }
        } else {
            // If-Match: <etag> → strong comparison against current ETag
            match &current_etag {
                None => {
                    return Err(AppError::precondition_failed(
                        "If-Match — resource does not exist",
                    ));
                }
                Some(etag) => {
                    let client_tag = im.trim_matches('"');
                    let server_tag = etag.trim_matches('"');
                    if client_tag != server_tag {
                        return Err(AppError::precondition_failed("If-Match — ETag mismatch"));
                    }
                }
            }
        }
    }

    // ── Streaming ingest ──────────────────────────────────────────────
    let filename = crate::common::mime_detect::filename_from_path(&path).to_string();
    let ingested = upload_ingest::ingest_body_to_cas(
        req.into_body(),
        &state.core.dedup_service,
        &filename,
        &content_type,
        max_upload,
    )
    .await?;

    // ── Quota enforcement ─────────────────────────────────────────────
    if let Some(storage_svc) = state.storage_usage_service.as_ref()
        && let Err(err) = storage_svc
            .check_storage_quota(user.id, ingested.size)
            .await
    {
        upload_ingest::discard_ingested(&state.core.dedup_service, &ingested).await;
        tracing::warn!(
            "⛔ WEBDAV PUT REJECTED (quota): user={}, file={}, size={}",
            user.id,
            path,
            ingested.size
        );
        return Err(AppError::new(
            StatusCode::INSUFFICIENT_STORAGE,
            err.message,
            "QuotaExceeded",
        ));
    }

    // ── Atomic store ──────────────────────────────────────────────────
    // `drive_id` was resolved above (existence-check block) and is
    // reused here — the same drive that scoped the resolver scopes the
    // write. `update_file_streaming` enforces `Permission::Update`
    // internally via its `_with_perms` shape.
    let content_type = ingested.content_type.clone();
    let result = file_upload_service
        .update_file_streaming_with_perms(
            &path,
            drive_id,
            ingested.stored(),
            &content_type,
            None,
            user.id,
        )
        .await;

    match result {
        Ok(file_dto) => {
            // RFC 4918 §9.7.1: 201 Created for new resources, 204 No Content
            // for overwrites. Always include ETag so clients can use it for
            // subsequent conditional requests without a round-trip HEAD.
            let status = if file_existed {
                StatusCode::NO_CONTENT
            } else {
                StatusCode::CREATED
            };
            Ok(Response::builder()
                .status(status)
                .header(header::ETAG, &file_dto.etag)
                .body(Body::empty())
                .unwrap())
        }
        Err(e) => Err(AppError::internal_error(format!(
            "Failed to put file: {}",
            e
        ))),
    }
}

/**
 * Handles MKCOL requests to create folders.
 *
 * This handler creates a new folder at the specified path.
 *
 * @param state The application state containing service dependencies
 * @param user The authenticated user information
 * @param path The requested resource path
 * @return HTTP response indicating success
 */
async fn handle_mkcol(
    state: Arc<AppState>,
    req: Request<Body>,
    path: String,
) -> Result<Response<Body>, AppError> {
    let user = extract_user(&req)?;
    let folder_service = &state.applications.folder_service;

    if path.is_empty() || path == "/" {
        return Err(AppError::conflict("Root folder already exists"));
    }

    // Extract content-type before consuming the body.
    let req_content_type = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // RFC 4918 §9.3.1: MKCOL body MUST be empty. A non-empty body with a
    // recognised XML content-type is 400 Bad Request (malformed MKCOL body);
    // a non-empty body with an unrecognised content-type is 415 Unsupported
    // Media Type. We read up to MAX_MKCOL_BODY bytes to distinguish the two.
    let body_bytes = {
        let body = req.into_body();
        body::to_bytes(body, MAX_MKCOL_BODY)
            .await
            .map_err(|e| AppError::payload_too_large(format!("MKCOL body too large: {}", e)))?
    };

    if !body_bytes.is_empty() {
        // A body whose content-type looks like XML → 400 (client sent a MKCOL
        // extended request we don't support); anything else → 415.
        let ct = req_content_type.as_deref().unwrap_or("");
        if ct.contains("xml") {
            return Err(AppError::bad_request(
                "MKCOL with XML body is not supported",
            ));
        }
        return Err(AppError::unsupported_media_type(
            "MKCOL request body must be empty",
        ));
    }

    // RFC 4918 §9.3.1: MKCOL on an existing URL MUST return 405.
    // RFC 4918 §9.3.1: MKCOL without an existing parent MUST return 409.
    // This handler only creates a single collection (the last path segment).
    // It does NOT auto-create intermediate ancestors ("mkdir -p" semantics
    // violate the RFC and were causing the test failures).
    let drive_id = resolve_drive_id_for_native_webdav(&state, user.id).await?;
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    if segments.is_empty() {
        return Err(AppError::conflict("Root folder already exists"));
    }

    // Check whether the target itself already exists (file or folder → 405).
    // `exists_in_drive` is an existence-only probe (returns a bool), so no
    // per-resource authz is applicable here — the create call below is
    // authorised via `Permission::Create` on the parent.
    if let Some(resolver) = &state.path_resolver {
        if resolver
            .exists_in_drive(&path, drive_id)
            .await
            .unwrap_or(false)
        {
            return Err(AppError::new(
                StatusCode::METHOD_NOT_ALLOWED,
                "Collection already exists",
                "AlreadyExists",
            ));
        }
    } else if folder_service
        .get_folder_by_path(&path, drive_id)
        .await
        .is_ok()
    {
        return Err(AppError::new(
            StatusCode::METHOD_NOT_ALLOWED,
            "Collection already exists",
            "AlreadyExists",
        ));
    }

    // Resolve the parent path. RFC 4918 §9.3.1: if the parent does not
    // exist, return 409 Conflict. If the parent exists but is a file, also
    // return 409 (cannot create a collection inside a file).
    let new_segment = *segments.last().unwrap();
    let parent_segments = &segments[..segments.len() - 1];

    let parent_id = if parent_segments.is_empty() {
        // Top-level creation — no parent required; the root folder acts as parent.
        None
    } else {
        let parent_path = parent_segments.join("/");
        // Parent must be a folder, not a file. Post-D7 the resolver is
        // drive-scoped so we explicitly `authz.require(Read, Folder)` on the
        // returned parent — the actual create then runs under
        // `Permission::Create` on the same folder inside `create_folder_with_perms`.
        if let Some(resolver) = &state.path_resolver {
            match resolver.resolve_path_in_drive(&parent_path, drive_id).await {
                Ok(ResolvedResource::Folder(f)) => {
                    let folder_uuid = Uuid::parse_str(&f.id).map_err(|_| {
                        AppError::conflict(format!("Parent folder not found: {}", parent_path))
                    })?;
                    state
                        .authorization
                        .require(
                            Subject::User(user.id),
                            Permission::Read,
                            Resource::Folder(folder_uuid),
                        )
                        .await
                        .map_err(|_| {
                            AppError::conflict(format!("Parent folder not found: {}", parent_path))
                        })?;
                    Some(f.id)
                }
                Ok(ResolvedResource::File(_)) => {
                    return Err(AppError::conflict(
                        "Parent path is a file, not a collection",
                    ));
                }
                Err(_) => {
                    return Err(AppError::conflict(format!(
                        "Parent folder not found: {}",
                        parent_path
                    )));
                }
            }
        } else {
            match folder_service
                .get_folder_by_path(&parent_path, drive_id)
                .await
            {
                Ok(f) => Some(f.id),
                Err(_) => {
                    return Err(AppError::conflict(format!(
                        "Parent folder not found: {}",
                        parent_path
                    )));
                }
            }
        }
    };

    let create_dto = crate::application::dtos::folder_dto::CreateFolderDto {
        name: new_segment.to_string(),
        parent_id,
    };
    folder_service
        .create_folder_with_perms(create_dto, user.id)
        .await
        .map_err(AppError::from)?;

    Ok(Response::builder()
        .status(StatusCode::CREATED)
        .body(Body::empty())
        .unwrap())
}

/**
 * Handles DELETE requests to remove files or folders.
 *
 * This handler deletes a file or folder at the specified path.
 *
 * @param state The application state containing service dependencies
 * @param user The authenticated user information
 * @param path The requested resource path
 * @return HTTP response indicating success
 */
async fn handle_delete(
    state: Arc<AppState>,
    req: Request<Body>,
    path: String,
) -> Result<Response<Body>, AppError> {
    let user = extract_user(&req)?;

    // Active-lock guard (RFC 4918 §9.10.4).
    let if_header_owned = req
        .headers()
        .get("If")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    if let Some(resp) = enforce_native_lock(
        &state.webdav_lock_store,
        if_header_owned.as_deref(),
        &path,
        None,
    ) {
        return Ok(resp);
    }

    // Get services from state
    let file_retrieval_service = &state.applications.file_retrieval_service;
    let file_management_service = &state.applications.file_management_service;
    let folder_service = &state.applications.folder_service;

    // Check if path is empty (root folder)
    if path.is_empty() || path == "/" {
        return Err(AppError::forbidden("Cannot delete root folder"));
    }

    // Resolve via optimized resolver, falling back to the legacy
    // double-query lookup (the one GET uses). Necessary because the
    // optimized resolver and the read repositories disagree on path
    // shape for some files; see `resolve_or_legacy` docs.
    let _ = file_retrieval_service; // present for legacy fallback if needed elsewhere
    match resolve_or_legacy(&state, &path, user.id).await {
        Some(ResolvedResource::Folder(folder)) => {
            folder_service
                .delete_folder_with_perms(&folder.id, user.id)
                .await
                .map_err(|e| AppError::internal_error(format!("Failed to delete folder: {}", e)))?;
        }
        Some(ResolvedResource::File(file)) => {
            file_management_service
                .delete_file_with_perms(&file.id, user.id)
                .await
                .map_err(|e| AppError::internal_error(format!("Failed to delete file: {}", e)))?;
        }
        None => return Err(AppError::not_found(format!("Resource not found: {}", path))),
    }

    // Dead-property rows attached to the deleted file/folder are reaped
    // automatically by `storage.webdav_dead_properties.{folder,file}_id`
    // ON DELETE CASCADE (migration 20260830000001). Same guarantee
    // applies to every other delete code path — REST `DELETE
    // /api/files/{id}`, bulk delete, trash empty, folder cascade —
    // without any service-layer call. No explicit cleanup needed here.

    Ok(Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Body::empty())
        .unwrap())
}

/**
 * Handles MOVE requests to rename or relocate files or folders.
 *
 * This handler moves a file or folder from one path to another.
 *
 * @param state The application state containing service dependencies
 * @param user The authenticated user information
 * @param path The source resource path
 * @param req The HTTP request containing the destination path
 * @return HTTP response indicating success
 */
async fn handle_move(
    state: Arc<AppState>,
    req: Request<Body>,
    path: String,
) -> Result<Response<Body>, AppError> {
    let user = extract_user(&req)?;
    let source_path = path;

    // Captured up front so a rejected MOVE doesn't run any DB work.
    let if_header_owned = req
        .headers()
        .get("If")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Active-lock guard on the SOURCE (RFC 4918 §9.10.4): the move
    // removes the source resource, which counts as modifying it.
    if let Some(resp) = enforce_native_lock(
        &state.webdav_lock_store,
        if_header_owned.as_deref(),
        &source_path,
        None,
    ) {
        return Ok(resp);
    }

    // Get destination from Destination header
    let destination = req
        .headers()
        .get("Destination")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::bad_request("Destination header required"))?
        .to_string();

    // Overwrite header (RFC 4918 §9.8.4): T = overwrite, F = fail if exists
    let overwrite = req
        .headers()
        .get("Overwrite")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("T")
        != "F";

    // Extract destination path from URL and percent-decode it
    let destination_path = if let Some(webdav_prefix) = destination.find("/webdav/") {
        let after_prefix = &destination[webdav_prefix + 8..];
        let trimmed = after_prefix.trim_end_matches('/');
        percent_decode_str(trimmed).decode_utf8_lossy().into_owned()
    } else {
        return Err(AppError::bad_request("Invalid destination URL"));
    };

    // SECURITY: reject path-traversal in destination
    reject_path_traversal(&destination_path)?;

    // Normalize destination through the SAME path-prefixing that
    // `resolve_webdav_path` applied to `source_path` during dispatch.
    // Without this, comparing source_parent_path (already prefixed with
    // the user's home folder name) against dest_parent_path (raw from
    // the URL, no prefix) always reports "different parent" — even for a
    // pure rename at the same level — and breaks the move/rename branch
    // selection below.
    let destination_path = resolve_webdav_path(&state, user.id, &destination_path)
        .await
        .unwrap_or(destination_path);

    // RFC 4918 §9.9.3: MOVE to self MUST return 403 Forbidden.
    if destination_path == source_path {
        return Err(AppError::forbidden("Cannot MOVE a resource to itself"));
    }

    // Destination lock guard: MOVE also creates/replaces a resource at
    // the destination. If that path is locked, the same If: header must
    // satisfy it.
    if let Some(resp) = enforce_native_lock(
        &state.webdav_lock_store,
        if_header_owned.as_deref(),
        &destination_path,
        None,
    ) {
        return Ok(resp);
    }

    let file_retrieval_service = &state.applications.file_retrieval_service;
    let file_management_service = &state.applications.file_management_service;
    let folder_service = &state.applications.folder_service;

    let drive_id = resolve_drive_id_for_native_webdav(&state, user.id).await?;

    // Probe destination existence for Overwrite semantics and 201 vs 204.
    let dest_existed = if let Some(resolver) = &state.path_resolver {
        resolver
            .exists_in_drive(&destination_path, drive_id)
            .await
            .unwrap_or(false)
    } else {
        folder_service
            .get_folder_by_path(&destination_path, drive_id)
            .await
            .is_ok()
            || file_retrieval_service
                .get_file_by_path(&destination_path, drive_id)
                .await
                .is_ok()
    };

    if dest_existed {
        if !overwrite {
            return Err(AppError::precondition_failed(
                "Destination already exists and Overwrite is F",
            ));
        }
        // RFC 4918 §9.9.3: when Overwrite: T, perform a DELETE on the
        // destination before moving. Without this the rename/move fails
        // on a unique-index conflict (same name in same parent).
        match resolve_or_legacy(&state, &destination_path, user.id).await {
            Some(ResolvedResource::Folder(f)) => {
                folder_service
                    .delete_folder_with_perms(&f.id, user.id)
                    .await
                    .map_err(|e| {
                        AppError::internal_error(format!(
                            "Failed to delete existing destination: {}",
                            e
                        ))
                    })?;
            }
            Some(ResolvedResource::File(f)) => {
                file_management_service
                    .delete_file_with_perms(&f.id, user.id)
                    .await
                    .map_err(|e| {
                        AppError::internal_error(format!(
                            "Failed to delete existing destination: {}",
                            e
                        ))
                    })?;
            }
            None => {}
        }
    }

    let _ = file_retrieval_service;
    let resolved = resolve_or_legacy(&state, &source_path, user.id)
        .await
        .ok_or_else(|| AppError::not_found(format!("Resource not found: {}", source_path)))?;

    let dest_name = destination_path
        .rsplit('/')
        .next()
        .unwrap_or(&destination_path);
    let dest_parent_path = destination_path
        .rfind('/')
        .map(|i| &destination_path[..i])
        .unwrap_or("");
    let source_parent_path = source_path
        .rfind('/')
        .map(|i| &source_path[..i])
        .unwrap_or("");

    match resolved {
        ResolvedResource::Folder(folder) => {
            // RFC 4918 §9.9.5: missing destination parent → 409 Conflict.
            let target_parent_id = if dest_parent_path.is_empty() {
                None
            } else {
                match folder_service
                    .get_folder_by_path(dest_parent_path, drive_id)
                    .await
                {
                    Ok(parent) => {
                        let parent_uuid = Uuid::parse_str(&parent.id).map_err(|_| {
                            AppError::conflict(format!(
                                "Destination parent not found: {}",
                                dest_parent_path
                            ))
                        })?;
                        state
                            .authorization
                            .require(
                                Subject::User(user.id),
                                Permission::Create,
                                Resource::Folder(parent_uuid),
                            )
                            .await?;
                        Some(parent.id)
                    }
                    Err(_) => {
                        return Err(AppError::conflict(format!(
                            "Destination parent not found: {}",
                            dest_parent_path
                        )));
                    }
                }
            };

            let move_dto = crate::application::dtos::folder_dto::MoveFolderDto {
                parent_id: target_parent_id,
            };

            folder_service
                .move_folder_with_perms(&folder.id, move_dto, user.id)
                .await
                .map_err(AppError::from)?;

            if folder.name != dest_name {
                let rename_dto = crate::application::dtos::folder_dto::RenameFolderDto {
                    name: dest_name.to_string(),
                };
                folder_service
                    .rename_folder_with_perms(&folder.id, rename_dto, user.id)
                    .await
                    .map_err(AppError::from)?;
            }
        }
        ResolvedResource::File(file) => {
            if source_parent_path != dest_parent_path {
                // RFC 4918 §9.9.5: missing destination parent → 409 Conflict.
                let target_parent_id = if dest_parent_path.is_empty() {
                    None
                } else {
                    let parent = folder_service
                        .get_folder_by_path(dest_parent_path, drive_id)
                        .await
                        .map_err(|_| {
                            AppError::conflict(format!(
                                "Destination parent not found: {}",
                                dest_parent_path
                            ))
                        })?;
                    let parent_uuid = Uuid::parse_str(&parent.id).map_err(|_| {
                        AppError::conflict(format!(
                            "Destination parent not found: {}",
                            dest_parent_path
                        ))
                    })?;
                    state
                        .authorization
                        .require(
                            Subject::User(user.id),
                            Permission::Create,
                            Resource::Folder(parent_uuid),
                        )
                        .await?;
                    Some(parent.id)
                };
                file_management_service
                    .move_file_with_perms(&file.id, user.id, target_parent_id)
                    .await
                    .map_err(AppError::from)?;
            }
            if file.name != dest_name {
                file_management_service
                    .rename_file_with_perms(&file.id, user.id, dest_name)
                    .await
                    .map_err(AppError::from)?;
            }
        }
    }

    // Dead properties follow the resource automatically across MOVE
    // and RENAME: the rows in `storage.webdav_dead_properties` key on
    // the underlying folder/file id, which is stable across both
    // operations (BEFORE trigger rewrites path on the row, AFTER
    // cascade rewrites descendants' path/lpath — but no id ever
    // changes). RFC 4918 §9.9 "MOVE preserves properties" satisfied
    // by the database invariant, no store call needed.

    // RFC 4918 §9.9.5: 201 Created when destination is new, 204 when overwritten.
    let status = if dest_existed {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::CREATED
    };
    Ok(Response::builder()
        .status(status)
        .body(Body::empty())
        .unwrap())
}

/**
 * Handles COPY requests to duplicate files or folders.
 *
 * This handler copies a file or folder from one path to another.
 *
 * @param state The application state containing service dependencies
 * @param user The authenticated user information
 * @param path The source resource path
 * @param req The HTTP request containing the destination path
 * @return HTTP response indicating success
 */
async fn handle_copy(
    state: Arc<AppState>,
    req: Request<Body>,
    path: String,
) -> Result<Response<Body>, AppError> {
    let user = extract_user(&req)?;
    let source_path = path;

    // Captured up front (cheap; used below for the destination lock guard).
    // COPY doesn't mutate the source, so no source lock check — only the
    // destination needs to clear (RFC 4918 §9.10.4).
    let if_header_owned = req
        .headers()
        .get("If")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Get destination from Destination header
    let destination = req
        .headers()
        .get("Destination")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::bad_request("Destination header required"))?
        .to_string();

    // Overwrite header (RFC 4918 §9.8.4): T = overwrite, F = fail if exists
    let overwrite = req
        .headers()
        .get("Overwrite")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("T")
        != "F";

    // Extract destination path from URL and percent-decode it
    let destination_path = if let Some(webdav_prefix) = destination.find("/webdav/") {
        let after_prefix = &destination[webdav_prefix + 8..];
        let trimmed = after_prefix.trim_end_matches('/');
        percent_decode_str(trimmed).decode_utf8_lossy().into_owned()
    } else {
        return Err(AppError::bad_request("Invalid destination URL"));
    };

    // SECURITY: reject path-traversal in destination
    reject_path_traversal(&destination_path)?;

    // Normalize through the same path-prefixing the dispatcher applied
    // to source_path. See the long comment in handle_move for why this
    // matters — same root-cause class of asymmetric-path bugs.
    let destination_path = resolve_webdav_path(&state, user.id, &destination_path)
        .await
        .unwrap_or(destination_path);

    // RFC 4918 §9.8.5: COPY to self MUST return 403 Forbidden.
    if destination_path == source_path {
        return Err(AppError::forbidden("Cannot COPY a resource to itself"));
    }

    // Active-lock guard on the destination (RFC 4918 §9.10.4).
    if let Some(resp) = enforce_native_lock(
        &state.webdav_lock_store,
        if_header_owned.as_deref(),
        &destination_path,
        None,
    ) {
        return Ok(resp);
    }

    // Get depth from Depth header
    let depth = req
        .headers()
        .get("Depth")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("infinity");

    // Get services from state
    let file_retrieval_service = &state.applications.file_retrieval_service;
    let folder_service = &state.applications.folder_service;
    let file_management_service = &state.applications.file_management_service;

    let drive_id = resolve_drive_id_for_native_webdav(&state, user.id).await?;

    // Probe destination existence for Overwrite semantics and 201 vs 204.
    let dest_existed = if let Some(resolver) = &state.path_resolver {
        resolver
            .exists_in_drive(&destination_path, drive_id)
            .await
            .unwrap_or(false)
    } else {
        folder_service
            .get_folder_by_path(&destination_path, drive_id)
            .await
            .is_ok()
            || file_retrieval_service
                .get_file_by_path(&destination_path, drive_id)
                .await
                .is_ok()
    };

    if dest_existed {
        if !overwrite {
            return Err(AppError::precondition_failed(
                "Destination already exists and Overwrite is F",
            ));
        }
        // RFC 4918 §9.8.4: when Overwrite: T, the server MUST perform a
        // DELETE on the destination before the copy. Without this the copy
        // service returns a unique-index conflict (500).
        match resolve_or_legacy(&state, &destination_path, user.id).await {
            Some(ResolvedResource::Folder(f)) => {
                folder_service
                    .delete_folder_with_perms(&f.id, user.id)
                    .await
                    .map_err(|e| {
                        AppError::internal_error(format!(
                            "Failed to delete existing destination: {}",
                            e
                        ))
                    })?;
            }
            Some(ResolvedResource::File(f)) => {
                file_management_service
                    .delete_file_with_perms(&f.id, user.id)
                    .await
                    .map_err(|e| {
                        AppError::internal_error(format!(
                            "Failed to delete existing destination: {}",
                            e
                        ))
                    })?;
            }
            None => {}
        }
    }

    let _ = file_retrieval_service;
    let resolved = resolve_or_legacy(&state, &source_path, user.id)
        .await
        .ok_or_else(|| AppError::not_found(format!("Resource not found: {}", source_path)))?;

    let dest_name = destination_path
        .rsplit('/')
        .next()
        .unwrap_or(&destination_path);
    let dest_parent_path = destination_path
        .rfind('/')
        .map(|i| &destination_path[..i])
        .unwrap_or("");

    // RFC 4918 §9.8.5: if the destination parent does not exist, return 409.
    let target_parent_id = if dest_parent_path.is_empty() {
        None
    } else {
        match folder_service
            .get_folder_by_path(dest_parent_path, drive_id)
            .await
        {
            Ok(parent) => {
                let parent_uuid = Uuid::parse_str(&parent.id).map_err(|_| {
                    AppError::conflict(format!(
                        "Destination parent not found: {}",
                        dest_parent_path
                    ))
                })?;
                state
                    .authorization
                    .require(
                        Subject::User(user.id),
                        Permission::Create,
                        Resource::Folder(parent_uuid),
                    )
                    .await?;
                Some(parent.id)
            }
            Err(_) => {
                return Err(AppError::conflict(format!(
                    "Destination parent not found: {}",
                    dest_parent_path
                )));
            }
        }
    };

    match resolved {
        ResolvedResource::Folder(folder) => {
            let recursive = depth != "0";
            if recursive {
                file_management_service
                    .copy_folder_tree_with_perms(
                        &folder.id,
                        user.id,
                        target_parent_id,
                        Some(dest_name.to_string()),
                    )
                    .await
                    .map_err(|e| {
                        AppError::internal_error(format!("Failed to copy folder tree: {}", e))
                    })?;
            } else {
                let create_dto = crate::application::dtos::folder_dto::CreateFolderDto {
                    name: dest_name.to_string(),
                    parent_id: target_parent_id,
                };
                folder_service
                    .create_folder_with_perms(create_dto, user.id)
                    .await
                    .map_err(|e| {
                        AppError::internal_error(format!(
                            "Failed to create destination folder: {}",
                            e
                        ))
                    })?;
            }
        }
        ResolvedResource::File(file) => {
            let copy_name = (file.name != dest_name).then(|| dest_name.to_string());
            file_management_service
                .copy_file_with_perms(&file.id, user.id, target_parent_id, copy_name)
                .await
                .map_err(|e| AppError::internal_error(format!("Failed to copy file: {}", e)))?;
        }
    }

    // RFC 4918 §9.8.5: 201 Created when destination is new, 204 when overwritten.
    let status = if dest_existed {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::CREATED
    };
    Ok(Response::builder()
        .status(status)
        .body(Body::empty())
        .unwrap())
}

/**
 * Handles LOCK requests to lock resources.
 *
 * This handler processes WebDAV LOCK requests according to RFC 4918,
 * creating a lock on a file or folder.
 *
 * @param state The application state containing service dependencies
 * @param user The authenticated user information
 * @param path The requested resource path
 * @param req The HTTP request containing the LOCK XML body
 * @return XML response with lock information
 */
async fn handle_lock(
    state: Arc<AppState>,
    req: Request<Body>,
    path: String,
) -> Result<Response<Body>, AppError> {
    let user = extract_user(&req)?;

    // Determine collection-vs-file for href shape. Root + known
    // folders → collection; everything else (existing files,
    // lock-null on a non-existent path) → file. RFC 4918 §9.10.1
    // allows LOCK on a non-existent resource (the "lock-null
    // resource" pattern used by Office save flows) — that arm
    // falls through to the file href shape, matching the
    // request-line shape clients send.
    let is_collection = if path.is_empty() || path == "/" {
        true
    } else {
        let drive_id = resolve_drive_id_for_native_webdav(&state, user.id).await?;
        state
            .applications
            .folder_service
            .get_folder_by_path(&path, drive_id)
            .await
            .is_ok()
    };

    // Get the headers that we need
    let depth = req
        .headers()
        .get("Depth")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("infinity")
        .to_string();

    let timeout = req
        .headers()
        .get("Timeout")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let if_header_value = req
        .headers()
        .get("If")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Extract the body separately to avoid borrow issues
    let body_bytes = {
        // Convert the request into a body
        let body = req.into_body();

        // Read request body (LOCK is XML, 1 MB is more than enough)
        body::to_bytes(body, MAX_XML_BODY)
            .await
            .map_err(|e| AppError::bad_request(format!("Failed to read request body: {}", e)))?
    };

    let lock_store = &state.webdav_lock_store;

    // Check if this is a lock refresh (If header with a lock token)
    if let Some(if_header) = if_header_value {
        // Extract lock token from If header
        let token = if_header
            .trim()
            .trim_start_matches("(<")
            .trim_end_matches(">)")
            .to_string();

        // Refresh the lock in the store (extends TTL)
        let entry = lock_store
            .refresh(&token, timeout.as_deref())
            .ok_or_else(|| {
                AppError::precondition_failed(format!("Lock token not found or expired: {}", token))
            })?;

        // Generate response — collection vs file href chosen above.
        let href = if is_collection {
            webdav_collection_href(&path)
        } else {
            webdav_href(&path)
        };
        let mut response_body = Vec::new();
        WebDavAdapter::generate_lock_response(&mut response_body, &entry.info, &href).map_err(
            |e| AppError::internal_error(format!("Failed to generate LOCK response: {}", e)),
        )?;

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
            .header(HEADER_LOCK_TOKEN, format!("<{}>", entry.info.token))
            .body(Body::from(response_body))
            .unwrap())
    } else if !body_bytes.is_empty() {
        // Parse lock request
        let (scope, type_, owner) = WebDavAdapter::parse_lockinfo(body_bytes.reader())
            .map_err(|e| AppError::bad_request(format!("Failed to parse LOCK request: {}", e)))?;

        let token = format!("opaquelocktoken:{}", Uuid::new_v4());
        let lock_info = LockInfo {
            token,
            owner: owner.or(Some(user.id.to_string())),
            depth: depth.to_string(),
            timeout,
            scope,
            type_,
        };

        // Try to acquire the lock (conflict detection via moka store)
        let entry = lock_store.acquire(&path, lock_info).map_err(|existing| {
            AppError::locked(format!(
                "Resource already locked by token {}",
                existing.info.token
            ))
        })?;

        // Generate response — collection vs file href chosen above.
        let href = if is_collection {
            webdav_collection_href(&path)
        } else {
            webdav_href(&path)
        };
        let mut response_body = Vec::new();
        WebDavAdapter::generate_lock_response(&mut response_body, &entry.info, &href).map_err(
            |e| AppError::internal_error(format!("Failed to generate LOCK response: {}", e)),
        )?;

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
            .header(HEADER_LOCK_TOKEN, format!("<{}>", entry.info.token))
            .body(Body::from(response_body))
            .unwrap())
    } else {
        Err(AppError::bad_request("Invalid LOCK request"))
    }
}

/**
 * Handles UNLOCK requests to remove locks from resources.
 *
 * This handler processes WebDAV UNLOCK requests according to RFC 4918,
 * removing a lock from a file or folder.
 *
 * @param state The application state containing service dependencies
 * @param user The authenticated user information
 * @param path The requested resource path
 * @param req The HTTP request containing the lock token
 * @return HTTP response indicating success
 */
async fn handle_unlock(
    state: Arc<AppState>,
    req: Request<Body>,
    _path: String,
) -> Result<Response<Body>, AppError> {
    let _user = extract_user(&req)?;

    // Get lock token from Lock-Token header
    let lock_token = req
        .headers()
        .get("Lock-Token")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::bad_request("Lock-Token header required"))?;

    // Extract token from header value (format: <token>)
    let token = lock_token
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_string();

    // Remove the lock from the store
    if !state.webdav_lock_store.release(&token) {
        // RFC 4918 §9.11.1: If the lock does not exist, return 409 Conflict
        return Err(AppError::conflict(format!(
            "Lock token not found or already expired: {}",
            token
        )));
    }

    Ok(Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Body::empty())
        .unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── RFC 4918 §10.4 If: header parser + evaluator ────────────────

    #[test]
    fn parse_if_simple_state_token() {
        let lists = parse_if_header("(<opaquelocktoken:xyz>)");
        assert_eq!(
            lists,
            vec![vec![IfCondition::StateToken {
                negated: false,
                token: "opaquelocktoken:xyz".to_string(),
            }]]
        );
    }

    #[test]
    fn parse_if_no_tag_list_two_lists() {
        let lists = parse_if_header("(<T1>) (Not <DAV:no-lock>)");
        assert_eq!(lists.len(), 2);
        assert_eq!(
            lists[0],
            vec![IfCondition::StateToken {
                negated: false,
                token: "T1".to_string(),
            }]
        );
        assert_eq!(
            lists[1],
            vec![IfCondition::StateToken {
                negated: true,
                token: "DAV:no-lock".to_string(),
            }]
        );
    }

    #[test]
    fn parse_if_state_token_and_etag() {
        let lists = parse_if_header("(<T1> [\"abc123\"])");
        assert_eq!(
            lists[0],
            vec![
                IfCondition::StateToken {
                    negated: false,
                    token: "T1".to_string(),
                },
                IfCondition::EntityTag {
                    negated: false,
                    etag: "abc123".to_string(),
                },
            ]
        );
    }

    #[test]
    fn parse_if_tagged_list_resource_ignored() {
        // Tagged-list — the Resource prefix `<http://…/foo>` scopes the
        // following Lists; our parser accepts but doesn't honour scoping.
        let lists = parse_if_header("<http://example.com/foo> (<T1>)");
        assert_eq!(lists.len(), 1);
        assert_eq!(
            lists[0],
            vec![IfCondition::StateToken {
                negated: false,
                token: "T1".to_string(),
            }]
        );
    }

    #[test]
    fn parse_if_complex_two_lists_token_and_etag() {
        // The litmus fail_complex_cond_put shape.
        let lists =
            parse_if_header("(<opaquelocktoken:xyz> [\"etag1\"]) (Not <DAV:no-lock> [\"etag2\"])");
        assert_eq!(lists.len(), 2);
        assert_eq!(lists[0].len(), 2);
        assert_eq!(lists[1].len(), 2);
        assert_eq!(
            lists[1][0],
            IfCondition::StateToken {
                negated: true,
                token: "DAV:no-lock".to_string(),
            }
        );
    }

    // Litmus `cond_put`: locked resource, `(<token> [etag])`, matches
    // both → header true, submitted the lock → proceed.
    #[test]
    fn evaluate_cond_put_success() {
        let lists = parse_if_header("(<opaquelocktoken:xyz> [\"abc\"])");
        let (matches, submitted) =
            evaluate_if_header(&lists, Some("opaquelocktoken:xyz"), Some("abc"));
        assert!(matches);
        assert!(submitted);
    }

    // Litmus `fail_cond_put`: locked resource, bogus token, valid etag
    // → List has token=FALSE AND etag=TRUE → FALSE. No matching token
    // submitted → 423 (caller returns Locked).
    #[test]
    fn evaluate_fail_cond_put_bogus_token_valid_etag() {
        let lists = parse_if_header("(<DAV:no-lock> [\"abc\"])");
        let (matches, submitted) =
            evaluate_if_header(&lists, Some("opaquelocktoken:xyz"), Some("abc"));
        assert!(!matches);
        assert!(!submitted);
    }

    // Litmus `fail_cond_put_unlocked`: unlocked resource, bogus token
    // → List fails. No lock to submit → 412.
    #[test]
    fn evaluate_fail_cond_put_unlocked() {
        let lists = parse_if_header("(<DAV:no-lock>)");
        let (matches, submitted) = evaluate_if_header(&lists, None, None);
        assert!(!matches);
        assert!(!submitted);
    }

    // Litmus `cond_put_with_not`: locked, `(<token>) (Not <DAV:no-lock>)`
    // → List 1 true (token match). Header true. Submitted → proceed.
    #[test]
    fn evaluate_cond_put_with_not() {
        let lists = parse_if_header("(<opaquelocktoken:xyz>) (Not <DAV:no-lock>)");
        let (matches, submitted) = evaluate_if_header(&lists, Some("opaquelocktoken:xyz"), None);
        assert!(matches);
        assert!(submitted);
    }

    // Litmus `cond_put_corrupt_token`: locked, `(<corrupt>) (Not <DAV:no-lock>)`
    // → List 2 (Not <DAV:no-lock>) is TRUE, header matches. But no
    // active-lock token submitted → 423 per §10.4.9.
    #[test]
    fn evaluate_cond_put_corrupt_token() {
        let lists = parse_if_header("(<opaquelocktoken:corrupt>) (Not <DAV:no-lock>)");
        let (matches, submitted) = evaluate_if_header(&lists, Some("opaquelocktoken:xyz"), None);
        assert!(matches);
        assert!(!submitted);
    }

    // Litmus `complex_cond_put`: locked, `(<token> [etag]) (Not <no-lock> [etag])`
    // with the CORRECT etag → List 1 true. Header true. Submitted → proceed.
    #[test]
    fn evaluate_complex_cond_put_success() {
        let lists =
            parse_if_header("(<opaquelocktoken:xyz> [\"abc\"]) (Not <DAV:no-lock> [\"abc\"])");
        let (matches, submitted) =
            evaluate_if_header(&lists, Some("opaquelocktoken:xyz"), Some("abc"));
        assert!(matches);
        assert!(submitted);
    }

    // Litmus `fail_complex_cond_put`: locked, `(<token> [corrupt]) (Not <no-lock> [corrupt])`
    // → both Lists AND to false (etag mismatch). Header FALSE. Token
    // WAS submitted (in List 1) → 412 not 423.
    #[test]
    fn evaluate_fail_complex_cond_put() {
        let lists = parse_if_header(
            "(<opaquelocktoken:xyz> [\"corrupt\"]) (Not <DAV:no-lock> [\"corrupt\"])",
        );
        let (matches, submitted) =
            evaluate_if_header(&lists, Some("opaquelocktoken:xyz"), Some("abc"));
        assert!(!matches);
        assert!(
            submitted,
            "the valid token IS submitted, even though etag conditions fail"
        );
    }

    #[test]
    fn evaluate_etag_with_quotes_matches_raw() {
        // The stored etag is raw (unquoted). The If: header quotes it.
        // trim_matches should normalise both sides.
        let lists = parse_if_header("([\"abc123-1234\"])");
        let (matches, _) = evaluate_if_header(&lists, None, Some("abc123-1234"));
        assert!(matches);
    }

    #[test]
    fn parse_if_empty_returns_no_lists() {
        assert_eq!(parse_if_header("").len(), 0);
    }

    #[test]
    fn evaluate_empty_lists_is_true() {
        let (matches, submitted) = evaluate_if_header(&Vec::new(), None, None);
        assert!(matches);
        assert!(!submitted);
    }

    #[test]
    fn test_webdav_href_no_trailing_slash() {
        assert_eq!(
            webdav_href("Documents/report.pdf"),
            "/webdav/Documents/report.pdf"
        );
        assert_eq!(webdav_href("file.txt"), "/webdav/file.txt");
    }

    #[test]
    fn test_webdav_collection_href_appends_slash_when_missing() {
        assert_eq!(webdav_collection_href("Documents"), "/webdav/Documents/");
        assert_eq!(
            webdav_collection_href("Documents/subfolder"),
            "/webdav/Documents/subfolder/"
        );
    }

    #[test]
    fn test_webdav_collection_href_idempotent_when_already_slashed() {
        // `encode_uri_path` never emits a trailing `/` of its own
        // because the path argument is already trimmed by routing,
        // but the helper still has to be robust to a path that
        // happens to end in `/` — exercise the idempotence path.
        assert_eq!(webdav_collection_href("Documents/"), "/webdav/Documents/");
    }

    #[test]
    fn test_webdav_href_preserves_url_encoding() {
        // Spaces and Unicode must percent-encode at the segment level,
        // not get a verbatim `%20` re-encoded as `%2520`.
        assert_eq!(
            webdav_href("My Photos/vacation pic.jpg"),
            "/webdav/My%20Photos/vacation%20pic.jpg"
        );
        assert_eq!(
            webdav_collection_href("My Photos/2024"),
            "/webdav/My%20Photos/2024/"
        );
    }
}
