use std::sync::Arc;

use crate::application::dtos::cursor::{CursorListResponse, CursorQuery, PageCursor};
use crate::application::dtos::display_helpers::intern_display;
use crate::application::dtos::grant_dto::{ResourceContentDto, ResourceTypeDto};
use crate::domain::entities::folder::Folder;
use crate::domain::services::authorization::ResourceKind;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

/// DTO for folder creation requests
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateFolderDto {
    /// Name of the folder to create
    pub name: String,

    /// Parent folder ID (None for root level)
    pub parent_id: Option<String>,
}

/// DTO for folder rename requests
#[derive(Debug, Deserialize, ToSchema)]
pub struct RenameFolderDto {
    /// New name for the folder
    pub name: String,
}

/// DTO for folder move requests
#[derive(Debug, Deserialize, ToSchema)]
pub struct MoveFolderDto {
    /// New parent folder ID (None for root level)
    pub parent_id: Option<String>,
}

/// DTO for folder responses
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FolderDto {
    /// Folder ID
    pub id: String,

    /// Folder name
    pub name: String,

    /// Path to the folder (relative)
    pub path: String,

    /// Parent folder ID
    pub parent_id: Option<String>,

    /// Drive that owns this folder. The scope axis for path-based
    /// lookups across REST / WebDAV / NextCloud / CalDAV / CardDAV.
    /// Post-D0 `storage.folders.drive_id` is `NOT NULL`; stub /
    /// DTO-reconstructed folders carry `Uuid::nil()`.
    pub drive_id: Uuid,

    /// Creation timestamp
    pub created_at: u64,

    /// Last modification timestamp
    pub modified_at: u64,

    /// Whether this is a root folder
    pub is_root: bool,

    // ── Pre-computed display fields (Arc<str>: always identical values) ──
    /// FontAwesome icon CSS class (always "fas fa-folder")
    #[schema(value_type = String)]
    pub icon_class: Arc<str>,

    /// Extra CSS class for icon styling (always "folder-icon")
    #[schema(value_type = String)]
    pub icon_special_class: Arc<str>,

    /// Human-readable category (always "Folder")
    #[schema(value_type = String)]
    pub category: Arc<str>,

    /// Opaque ETag for HTTP responses. Populated from `Folder::etag()`
    /// at conversion time so every WebDAV / NextCloud handler emits
    /// the same value, and exposed in REST JSON so the frontend can
    /// pass it back through `If-Match` on rename / move endpoints
    /// without a separate HEAD round-trip.
    pub etag: String,

    /// §14 provenance: user that originally created this folder.
    /// `None` when the referenced user has been deleted (FK is
    /// `ON DELETE SET NULL`) or for stub/legacy folders.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<Uuid>,

    /// §14 provenance: user that performed the most recent mutation
    /// that bumped `updated_at`. Authorship signal — distinct from
    /// `owner_id`. `None` when the referenced user is deleted or for
    /// stub/legacy folders.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<Uuid>,
}

impl From<Folder> for FolderDto {
    fn from(folder: Folder) -> Self {
        // Consume the entity by moving all fields — zero heap allocations
        // for id, name, path, parent_id (previously 3-4× .to_string()).
        let parts = folder.into_parts();

        let is_root = parts.parent_id.is_none();
        // Single-allocation ETag straight from the owned parts. The old
        // shape (`folder.etag().to_string()`) built the String and then
        // cloned it — a pure double-alloc.
        let etag = Folder::compute_etag(&parts.id, parts.tree_modified_at);

        Self {
            id: parts.id,
            name: parts.name,
            path: parts.storage_path.into_joined(),
            parent_id: parts.parent_id,
            drive_id: parts.drive_id,
            created_at: parts.created_at,
            modified_at: parts.modified_at,
            is_root,
            // Constant display fields: refcount bump on interned statics
            // instead of 3 fresh Arc allocations per row.
            icon_class: intern_display("fas fa-folder"),
            icon_special_class: intern_display("folder-icon"),
            category: intern_display("Folder"),
            etag,
            created_by: parts.created_by,
            updated_by: parts.updated_by,
        }
    }
}

// To convert from FolderDto to Folder for batch handlers
impl From<FolderDto> for Folder {
    fn from(dto: FolderDto) -> Self {
        // Display fields (icon_class, icon_special_class, category)
        // are not part of the domain entity and are ignored.
        Folder::from_dto(
            dto.id,
            dto.name,
            dto.path,
            dto.parent_id,
            dto.created_at,
            dto.modified_at,
        )
    }
}

impl FolderDto {
    /// Returns a copy of this DTO with the `path` field cleared.
    ///
    /// Used when a folder is returned to a share recipient: `path` reveals the
    /// full folder hierarchy above the shared folder which the recipient may
    /// not have access to.  `parent_id` is intentionally kept — it's needed
    /// for sub-folder navigation (covered by the cascade grant).
    #[must_use]
    pub fn without_hierarchy_info(self) -> Self {
        Self {
            path: String::new(),
            ..self
        }
    }

    /// Creates an empty folder DTO for stub implementations
    pub fn empty() -> Self {
        Self {
            id: "stub-id".to_string(),
            name: "stub-folder".to_string(),
            path: "/stub/path".to_string(),
            parent_id: None,
            drive_id: Uuid::nil(),
            created_at: 0,
            modified_at: 0,
            is_root: true,
            icon_class: intern_display("fas fa-folder"),
            icon_special_class: intern_display("folder-icon"),
            category: intern_display("Folder"),
            etag: String::new(),
            created_by: None,
            updated_by: None,
        }
    }
}

impl Default for FolderDto {
    fn default() -> Self {
        Self::empty()
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Cursor-paginated folder resources  (GET /api/folders/{id}/resources)
// ════════════════════════════════════════════════════════════════════════════

/// Raw row returned by the UNION ALL query that combines `storage.folders` and
/// `storage.files` for a given parent folder.  Used internally between the
/// repository and service/handler layers — never serialised directly.
pub struct FolderResourceRow {
    pub resource_type: String, // "folder" | "file"
    pub id: Uuid,
    pub name: String,
    /// Parent folder UUID (for both resource types).
    pub parent_id: Option<Uuid>,
    /// `None` for folders.
    pub mime_type: Option<String>,
    /// `-1` sentinel for folders (no physical size).
    pub size: i64,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
    /// Drive that owns this row. Same column as
    /// `storage.folders.drive_id` / `storage.files.drive_id`. Surfaced
    /// on the listing so a UI can tell when a child lives in a
    /// different drive than its parent (post-D6 cross-drive moves +
    /// copies make this reachable).
    pub drive_id: Uuid,
    /// Raw BLAKE3 content hash. `Some(_)` for file rows, `None` for
    /// folder rows. Populates `FileDto::content_hash` + `FileDto::etag`
    /// on the REST `/api/folders/{id}/resources` listing so API
    /// consumers can issue conditional requests against listed files.
    pub blob_hash: Option<String>,
    // Pre-computed sort fields — returned by the SQL for cursor construction.
    /// `LOWER(name)` used by `name`/`type` sorts.
    pub sort_str: String,
    /// `category_order` for files, `0` for folders.
    pub type_order: i64,
    /// `0` for folders, `1` for files (used by `name` sort to keep folders first).
    pub folder_first: i32,
}

/// Opaque keyset-pagination cursor for `/api/folders/{id}/resources`.
///
/// Encoded as base64url-JSON (same scheme as [`GrantCursor`]).
/// Fields are sparse: only the sort-relevant ones are serialised.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderResourceCursor {
    /// Sort dimension active when this cursor was produced.
    #[serde(default = "FolderResourceCursor::default_order")]
    pub order_by: String,
    /// UUID of the last item on the previous page (tie-breaker).
    pub resource_id: Uuid,
    /// `LOWER(name)` for `name`/`type` sorts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_str: Option<String>,
    /// Multipurpose integer sort key:
    /// - `name`:  `folder_first` (0 = folder, 1 = file)
    /// - `type`:  `category_order` (0 = Folder, 100 = Image …)
    /// - `size`:  file size in bytes, -1 for folders
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_int: Option<i64>,
    /// Timestamp for `modified_at` / `created_at` sorts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_ts: Option<DateTime<Utc>>,
    /// Whether the result set was reversed when this cursor was produced.
    /// Must be passed unchanged on subsequent page requests.
    #[serde(default)]
    pub reverse: bool,
}

impl FolderResourceCursor {
    fn default_order() -> String {
        "name".to_owned()
    }
}

impl PageCursor for FolderResourceCursor {}

/// Query parameters for `GET /api/folders/{id}/resources`.
#[derive(Debug, Deserialize, IntoParams)]
pub struct FolderResourcesQuery {
    /// Maximum items per page (1–200, default 50).
    #[serde(default = "CursorQuery::default_limit")]
    pub limit: u32,
    /// Opaque cursor from a previous response. Omit to start from the top.
    pub cursor: Option<String>,
    /// Sort / group-by dimension. Supported: `"name"` (default), `"type"`,
    /// `"modified_at"`, `"created_at"`, `"size"`.
    pub order_by: Option<String>,
    /// Comma-separated resource types to include, e.g. `"file,folder"`.
    /// Omit to include both.
    pub resource_types: Option<String>,
    /// Reverse the sort order. Default `false` (normal order).
    /// Must be the same on all pages of the same result set — the cursor
    /// carries this flag so the server can validate consistency.
    #[serde(default)]
    pub reverse: bool,
}

impl FolderResourcesQuery {
    /// Returns `limit` clamped to `[1, 200]`.
    pub fn limit_clamped(&self) -> usize {
        self.limit.clamp(1, 200) as usize
    }

    /// Decode the optional cursor string. Invalid cursor → start from top.
    pub fn decode_cursor(&self) -> Option<FolderResourceCursor> {
        self.cursor
            .as_deref()
            .and_then(FolderResourceCursor::decode)
    }

    /// Parse `resource_types` into a `Vec<ResourceKind>`.
    /// Returns `None` when the field is absent (= include all types).
    pub fn resource_kinds(&self) -> Option<Vec<ResourceKind>> {
        self.resource_types.as_deref().map(|s| {
            s.split(',')
                .filter_map(|t| ResourceKind::parse(t.trim()))
                .collect()
        })
    }
}

/// Options for [`FolderService::list_resources_paged_with_perms`].
///
/// Groups the optional parameters so the function stays within clippy's
/// `too_many_arguments` limit while remaining easy to extend.
pub struct ListResourcesOptions<'a> {
    pub limit: usize,
    pub cursor: Option<FolderResourceCursor>,
    pub order_by: &'a str,
    pub kinds: Option<&'a [ResourceKind]>,
    pub reverse: bool,
}

/// One item in a `/resources` page — a file or folder with a `resource_type` tag.
/// Re-uses [`ResourceContentDto`] so the shape is identical to `SharedWithMeItemDto.resource`.
#[derive(Debug, Serialize, ToSchema)]
pub struct FolderResourceItemDto {
    pub resource_type: ResourceTypeDto,
    /// Full resource details. Shape is determined by `resource_type`.
    pub resource: ResourceContentDto,
}

/// Response envelope for `GET /api/folders/{id}/resources`.
pub type FolderResourcesDto = CursorListResponse<FolderResourceItemDto>;
