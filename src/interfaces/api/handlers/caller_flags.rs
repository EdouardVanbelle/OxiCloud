//! Shared enrichment helpers that populate the `is_favorite` and
//! `is_shared` wire-contract flags on `FileDto` / `FolderDto` before
//! Json emission.
//!
//! The two functions live here so single-item handlers across
//! `folder_handler`, `file_handler`, `delta_upload_handler`,
//! `photos_handler`, etc. all go through the same path — one place
//! to change if the enrichment strategy ever moves (e.g. batch
//! lookups, background prefetch).
//!
//! Every handler that returns a `FileDto` or `FolderDto` to the SPA
//! MUST call one of these helpers. Handlers that emit only to
//! WebDAV / NextCloud DAV surfaces (which drop these fields via the
//! XML property serializer) can skip enrichment — the default `false`
//! is never observable on those wires.

use std::sync::Arc;

use uuid::Uuid;

use crate::application::dtos::file_dto::FileDto;
use crate::application::dtos::folder_dto::FolderDto;
use crate::common::di::AppState as GlobalAppState;

/// Populate the `is_favorite` + `is_shared` flags on a `FolderDto`.
///
/// Silently leaves the flags at their default `false` when the
/// favorites service isn't wired (feature-off) or when the resource
/// id doesn't parse as a UUID — the DTO stays valid on the wire and
/// the misleading-`false` window closes as soon as the next listing
/// refetch runs.
pub async fn enrich_folder_flags(
    state: &Arc<GlobalAppState>,
    dto: &mut FolderDto,
    caller_id: Uuid,
) {
    let Some(favs) = state.favorites_service.as_ref() else {
        return;
    };
    let Ok(resource_id) = Uuid::parse_str(&dto.id) else {
        return;
    };
    if let Ok((fav, shr)) = favs.caller_flags(caller_id, "folder", resource_id).await {
        dto.is_favorite = fav;
        dto.is_shared = shr;
    }
}

/// File counterpart of [`enrich_folder_flags`] — see that doc.
pub async fn enrich_file_flags(state: &Arc<GlobalAppState>, dto: &mut FileDto, caller_id: Uuid) {
    let Some(favs) = state.favorites_service.as_ref() else {
        return;
    };
    let Ok(resource_id) = Uuid::parse_str(&dto.id) else {
        return;
    };
    if let Ok((fav, shr)) = favs.caller_flags(caller_id, "file", resource_id).await {
        dto.is_favorite = fav;
        dto.is_shared = shr;
    }
}

/// Batch variant: enrich every `FileDto` in a slice with per-item
/// `caller_flags`. Runs the lookups sequentially — for the bulk
/// endpoints (`get_files_by_ids`, `photos_handler`) this is one
/// round trip per item; if that becomes hot on a large fetch, the
/// callsite can be replaced with a single SQL query returning the
/// pairs. Kept simple for now; the DTO is `&mut`, no clones.
pub async fn enrich_file_flags_batch(
    state: &Arc<GlobalAppState>,
    dtos: &mut [FileDto],
    caller_id: Uuid,
) {
    for dto in dtos.iter_mut() {
        enrich_file_flags(state, dto, caller_id).await;
    }
}

/// Batch variant for folders — mirror of [`enrich_file_flags_batch`].
pub async fn enrich_folder_flags_batch(
    state: &Arc<GlobalAppState>,
    dtos: &mut [FolderDto],
    caller_id: Uuid,
) {
    for dto in dtos.iter_mut() {
        enrich_folder_flags(state, dto, caller_id).await;
    }
}
