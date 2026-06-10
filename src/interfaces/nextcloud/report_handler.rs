use axum::{
    body::{self, Body},
    http::{Request, StatusCode, header},
    response::Response,
};
use quick_xml::{
    Reader, Writer,
    events::{BytesEnd, BytesStart, Event},
};
use std::collections::HashSet;
use std::sync::Arc;

use crate::application::dtos::display_helpers::{
    category_for, format_file_size, icon_class_for, icon_special_class_for,
};
use crate::application::dtos::file_dto::FileDto;
use crate::application::dtos::folder_dto::FolderDto;
use crate::application::dtos::search_dto::SearchCriteriaDto;
use crate::application::ports::favorites_ports::FavoritesUseCase;
use crate::application::ports::file_ports::FileRetrievalUseCase;
use crate::application::ports::folder_ports::FolderUseCase;
use crate::application::ports::inbound::SearchUseCase;
use crate::common::di::AppState;
use crate::domain::entities::file::File;
use crate::interfaces::errors::AppError;
use crate::interfaces::middleware::auth::CurrentUser;
use crate::interfaces::nextcloud::webdav_handler::{
    batch_resolve_ids, format_oc_id, nc_href, write_file_response, write_folder_response,
};

/// Handle WebDAV REPORT and SEARCH methods for Nextcloud compatibility.
///
/// Dispatches based on the XML body:
/// - `oc:filter-files` -- list favorited items (REPORT)
/// - `d:searchrequest`  -- search files by name (SEARCH)
pub async fn handle_nc_report(
    state: Arc<AppState>,
    req: Request<Body>,
    user: &CurrentUser,
    _subpath: &str,
) -> Result<Response<Body>, AppError> {
    let body_bytes = body::to_bytes(req.into_body(), 64 * 1024)
        .await
        .map_err(|e| AppError::bad_request(format!("Failed to read body: {}", e)))?;

    let body_str = String::from_utf8_lossy(&body_bytes);

    if body_str.contains("filter-files") {
        handle_filter_files(state, &body_str, user).await
    } else if body_str.contains("searchrequest") {
        handle_search(state, &body_str, user).await
    } else {
        // Unknown REPORT type -- return empty multistatus.
        Ok(empty_multistatus())
    }
}

// ──────────────────── Favorites filter (oc:filter-files) ────────────────────

async fn handle_filter_files(
    state: Arc<AppState>,
    _body: &str,
    user: &CurrentUser,
) -> Result<Response<Body>, AppError> {
    let fav_svc = match state.favorites_service.as_ref() {
        Some(svc) => svc,
        None => return Ok(empty_multistatus()),
    };

    let favorites = fav_svc
        .get_favorites(user.id)
        .await
        .map_err(|e| AppError::internal_error(format!("Failed to get favorites: {}", e)))?;

    if favorites.is_empty() {
        return Ok(empty_multistatus());
    }

    let file_service = &state.applications.file_retrieval_service;
    let folder_service = &state.applications.folder_service;
    let nc = state.nextcloud.as_ref();
    let file_id_svc = nc.map(|n| &n.file_ids);

    // All items in this response are favorites.
    let favorite_ids: HashSet<String> = favorites.iter().map(|f| f.item_id.clone()).collect();

    let home_prefix = format!("My Folder - {}/", user.username);

    // Pass 1: fetch the favorited DTOs (the per-item fetch is a separate
    // concern from the oc:fileid resolution batched below).
    let mut files: Vec<FileDto> = Vec::new();
    let mut folders: Vec<FolderDto> = Vec::new();
    for fav in &favorites {
        match fav.item_type.as_str() {
            "file" => {
                if let Ok(f) = file_service.get_file(&fav.item_id).await {
                    files.push(f);
                }
            }
            "folder" => {
                if let Ok(f) = folder_service.get_folder(&fav.item_id).await {
                    folders.push(f);
                }
            }
            _ => {}
        }
    }

    // Pass 2: resolve every oc:fileid in two batch queries (was one per item).
    let file_uuids: Vec<String> = files.iter().map(|f| f.id.clone()).collect();
    let folder_uuids: Vec<String> = folders.iter().map(|f| f.id.clone()).collect();
    let (file_id_map, folder_id_map) =
        batch_resolve_ids(file_id_svc, &file_uuids, &folder_uuids).await;

    // Pass 3: write the multistatus XML (pure synchronous map lookups).
    let mut buf = Vec::new();
    {
        let mut xml = Writer::new(&mut buf);

        write_multistatus_start(&mut xml)?;

        for file in &files {
            let subpath = strip_home_prefix(&file.path, &home_prefix);
            let href = nc_href(&user.username, subpath);
            let fid = file_id_map.get(&file.id).copied();
            let oc_id = fid.map(|id| format_oc_id(id, file_id_svc));
            write_file_response(
                &mut xml,
                file,
                &href,
                fid,
                oc_id.as_deref(),
                &user.username,
                &favorite_ids,
            )
            .map_err(|e| AppError::internal_error(format!("XML write error: {}", e)))?;
        }

        for folder in &folders {
            let subpath = strip_home_prefix(&folder.path, &home_prefix);
            let href = format!("{}/", nc_href(&user.username, subpath));
            let fid = folder_id_map.get(&folder.id).copied();
            let oc_id = fid.map(|id| format_oc_id(id, file_id_svc));
            write_folder_response(
                &mut xml,
                folder,
                &href,
                fid,
                oc_id.as_deref(),
                &user.username,
                &favorite_ids,
            )
            .map_err(|e| AppError::internal_error(format!("XML write error: {}", e)))?;
        }

        xml.write_event(Event::End(BytesEnd::new("d:multistatus")))
            .map_err(|e| AppError::internal_error(format!("XML write error: {}", e)))?;
    }

    Ok(Response::builder()
        .status(StatusCode::MULTI_STATUS)
        .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
        .body(Body::from(buf))
        .unwrap())
}

// ──────────────────── Search (d:searchrequest) ────────────────────

async fn handle_search(
    state: Arc<AppState>,
    body: &str,
    user: &CurrentUser,
) -> Result<Response<Body>, AppError> {
    let search_svc = match state.applications.search_service.as_ref() {
        Some(svc) => svc,
        None => return Ok(empty_multistatus()),
    };

    let term = parse_literal(body).unwrap_or_default();
    if term.is_empty() {
        return Ok(empty_multistatus());
    }

    let nresults = parse_nresults(body).unwrap_or(100);

    // Resolve folder scope from <d:href> inside <d:scope>.
    let folder_id = resolve_scope_folder(&state, body, &user.username).await;

    let criteria = SearchCriteriaDto {
        name_contains: Some(term),
        recursive: true,
        limit: nresults,
        folder_id,
        ..Default::default()
    };

    let results = search_svc
        .search(criteria, user.id)
        .await
        .map_err(|e| AppError::internal_error(format!("Search failed: {}", e)))?;

    let nc = state.nextcloud.as_ref();
    let file_id_svc = nc.map(|n| &n.file_ids);
    let home_prefix = format!("My Folder - {}/", user.username);

    // No favorite checking for search results -- pass an empty set.
    let favorite_ids: HashSet<String> = HashSet::new();

    // Materialize DTOs, then resolve every oc:fileid in two batch queries
    // (was one INSERT round-trip per result).
    let files: Vec<FileDto> = results.files.iter().map(file_dto_from_search).collect();
    let folders: Vec<FolderDto> = results.folders.iter().map(folder_dto_from_search).collect();
    let file_uuids: Vec<String> = files.iter().map(|f| f.id.clone()).collect();
    let folder_uuids: Vec<String> = folders.iter().map(|f| f.id.clone()).collect();
    let (file_id_map, folder_id_map) =
        batch_resolve_ids(file_id_svc, &file_uuids, &folder_uuids).await;

    let mut buf = Vec::new();
    {
        let mut xml = Writer::new(&mut buf);

        write_multistatus_start(&mut xml)?;

        // Files.
        for file in &files {
            let subpath = strip_home_prefix(&file.path, &home_prefix);
            let href = nc_href(&user.username, subpath);
            let fid = file_id_map.get(&file.id).copied();
            let oc_id = fid.map(|id| format_oc_id(id, file_id_svc));
            write_file_response(
                &mut xml,
                file,
                &href,
                fid,
                oc_id.as_deref(),
                &user.username,
                &favorite_ids,
            )
            .map_err(|e| AppError::internal_error(format!("XML write error: {}", e)))?;
        }

        // Folders.
        for folder in &folders {
            let subpath = strip_home_prefix(&folder.path, &home_prefix);
            let href = format!("{}/", nc_href(&user.username, subpath));
            let fid = folder_id_map.get(&folder.id).copied();
            let oc_id = fid.map(|id| format_oc_id(id, file_id_svc));
            write_folder_response(
                &mut xml,
                folder,
                &href,
                fid,
                oc_id.as_deref(),
                &user.username,
                &favorite_ids,
            )
            .map_err(|e| AppError::internal_error(format!("XML write error: {}", e)))?;
        }

        xml.write_event(Event::End(BytesEnd::new("d:multistatus")))
            .map_err(|e| AppError::internal_error(format!("XML write error: {}", e)))?;
    }

    Ok(Response::builder()
        .status(StatusCode::MULTI_STATUS)
        .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
        .body(Body::from(buf))
        .unwrap())
}

// ──────────────────── DTO conversions ────────────────────

/// Build a `FileDto` from a search file result.
fn file_dto_from_search(fr: &crate::application::dtos::search_dto::SearchFileResultDto) -> FileDto {
    // Route ETag through `File::compute_etag` so REPORT/SEARCH hits
    // emit the same opaque token NC's sync client cached from the
    // earlier PROPFIND walk — without this, NC's conditional-request
    // logic on search results disagrees with its own cached state
    // and triggers a spurious re-fetch.
    let etag = if fr.blob_hash.is_empty() {
        String::new()
    } else {
        File::compute_etag(&fr.blob_hash, fr.modified_at)
    };
    FileDto {
        id: fr.id.clone(),
        name: fr.name.clone(),
        path: fr.path.clone(),
        size: fr.size,
        mime_type: fr.mime_type.clone().into(),
        folder_id: fr.folder_id.clone(),
        created_at: fr.created_at,
        modified_at: fr.modified_at,
        icon_class: icon_class_for(&fr.name, &fr.mime_type).to_string().into(),
        icon_special_class: icon_special_class_for(&fr.name, &fr.mime_type)
            .to_string()
            .into(),
        category: category_for(&fr.name, &fr.mime_type).to_string().into(),
        size_formatted: format_file_size(fr.size),
        owner_id: None,
        sort_date: None,
        content_hash: fr.blob_hash.clone(),
        etag,
    }
}

/// Build a `FolderDto` from a search folder result.
fn folder_dto_from_search(
    sr: &crate::application::dtos::search_dto::SearchFolderResultDto,
) -> FolderDto {
    FolderDto {
        etag: sr.id.clone(),
        id: sr.id.clone(),
        name: sr.name.clone(),
        path: sr.path.clone(),
        parent_id: sr.parent_id.clone(),
        owner_id: None,
        created_at: sr.created_at,
        modified_at: sr.modified_at,
        is_root: sr.is_root,
        icon_class: Arc::from("fas fa-folder"),
        icon_special_class: Arc::from("folder-icon"),
        category: Arc::from("Folder"),
    }
}

// ──────────────────── XML helpers ────────────────────

/// Write the opening `<d:multistatus>` element with namespace declarations.
fn write_multistatus_start<W: std::io::Write>(xml: &mut Writer<W>) -> Result<(), AppError> {
    let mut ms = BytesStart::new("d:multistatus");
    ms.push_attribute(("xmlns:d", "DAV:"));
    ms.push_attribute(("xmlns:oc", "http://owncloud.org/ns"));
    ms.push_attribute(("xmlns:nc", "http://nextcloud.org/ns"));
    xml.write_event(Event::Start(ms))
        .map_err(|e| AppError::internal_error(format!("XML write error: {}", e)))?;
    Ok(())
}

/// Build an empty 207 Multi-Status response.
fn empty_multistatus() -> Response<Body> {
    let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns" xmlns:nc="http://nextcloud.org/ns">
</d:multistatus>"#;

    Response::builder()
        .status(StatusCode::MULTI_STATUS)
        .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
        .body(Body::from(xml))
        .unwrap()
}

// ──────────────────── XML parsing helpers ────────────────────

/// Extract the search term from `<d:literal>%term%</d:literal>` using quick_xml.
fn parse_literal(body: &str) -> Option<String> {
    let text = xml_extract_text(body, b"literal")?;
    // Strip SQL-style % wildcards.
    let term = text.trim_matches('%').trim();
    if term.is_empty() {
        None
    } else {
        Some(term.to_string())
    }
}

/// Extract the result limit from `<d:nresults>100</d:nresults>` using quick_xml.
fn parse_nresults(body: &str) -> Option<usize> {
    let text = xml_extract_text(body, b"nresults")?;
    text.trim().parse::<usize>().ok()
}

/// Extract the scope href from `<d:href>` inside `<d:scope>` using quick_xml.
fn parse_scope_href(body: &str) -> Option<String> {
    let mut reader = Reader::from_str(body);
    let mut inside_scope = false;
    let mut inside_href = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let local = e.local_name();
                if local.as_ref() == b"scope" {
                    inside_scope = true;
                } else if inside_scope && local.as_ref() == b"href" {
                    inside_href = true;
                }
            }
            Ok(Event::Text(ref e)) if inside_href => {
                let text = e.decode().ok()?;
                let href = text.trim();
                if href.is_empty() {
                    return None;
                }
                return Some(href.to_string());
            }
            Ok(Event::End(ref e)) => {
                let local = e.local_name();
                if local.as_ref() == b"scope" {
                    inside_scope = false;
                } else if local.as_ref() == b"href" {
                    inside_href = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    None
}

/// Generic helper: extract text content from the first element matching a local name.
fn xml_extract_text(body: &str, local_name: &[u8]) -> Option<String> {
    let mut reader = Reader::from_str(body);
    let mut inside = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) if e.local_name().as_ref() == local_name => {
                inside = true;
            }
            Ok(Event::Text(ref e)) if inside => {
                return e.decode().ok().map(|s| s.to_string());
            }
            Ok(Event::End(ref e)) if e.local_name().as_ref() == local_name => {
                inside = false;
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    None
}

/// Resolve a scope href (e.g. `/files/username/Documents`) to a folder ID.
async fn resolve_scope_folder(state: &AppState, body: &str, username: &str) -> Option<String> {
    let href = parse_scope_href(body)?;

    // The href is typically `/files/{user}/subpath` or `/remote.php/dav/files/{user}/subpath`.
    let subpath = extract_subpath_from_scope(&href, username)?;
    if subpath.is_empty() {
        // Root scope -- no folder_id filter needed.
        return None;
    }

    let internal_path =
        crate::interfaces::nextcloud::webdav_handler::nc_to_internal_path(username, &subpath)
            .ok()?;

    let folder_service = &state.applications.folder_service;
    folder_service
        .get_folder_by_path(&internal_path)
        .await
        .ok()
        .map(|f| f.id)
}

/// Extract the subpath portion from a scope href.
///
/// Handles both short form `/files/{user}/sub` and full
/// `/remote.php/dav/files/{user}/sub`.
fn extract_subpath_from_scope(href: &str, username: &str) -> Option<String> {
    let patterns = [
        format!("/remote.php/dav/files/{}/", username),
        format!("/files/{}/", username),
        format!("/remote.php/dav/files/{}", username),
        format!("/files/{}", username),
    ];

    for pat in &patterns {
        if let Some(rest) = href.strip_prefix(pat.as_str()) {
            return Some(rest.trim_matches('/').to_string());
        }
    }

    None
}

/// Strip the `My Folder - {username}/` prefix to get the DAV subpath.
fn strip_home_prefix<'a>(path: &'a str, prefix: &str) -> &'a str {
    path.strip_prefix(prefix).unwrap_or(path)
}
