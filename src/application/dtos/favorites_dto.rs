use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::cursor::{CursorListResponse, CursorQuery, PageCursor};
use super::display_helpers::{classify_display, format_file_size};
use super::grant_dto::{ResourceContentDto, ResourceTypeDto};
use crate::domain::services::authorization::ResourceKind;

/// DTO for favorites item, enriched with item metadata via SQL JOIN
/// so the frontend does not need N+1 requests to resolve names/sizes.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FavoriteItemDto {
    /// Unique identifier for the favorite entry
    pub id: String,

    /// User ID who owns this favorite
    pub user_id: String,

    /// ID of the favorited item (file or folder)
    pub item_id: String,

    /// Type of the item ('file' or 'folder')
    pub item_type: String,

    /// When the item was added to favorites
    pub created_at: DateTime<Utc>,

    // ── Enriched metadata (resolved via JOIN) ──
    /// Display name of the file or folder
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_name: Option<String>,

    /// Size in bytes (files only; folders → None)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_size: Option<i64>,

    /// MIME type (files only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_mime_type: Option<String>,

    /// Parent folder ID (folder_id for files, parent_id for folders)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,

    /// Last modification timestamp of the item
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<DateTime<Utc>>,

    /// Full human-readable path (e.g. "Documents/Work" for a folder,
    /// "Documents/Work/report.pdf" for a file)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_path: Option<String>,

    // ── Pre-computed display fields ──
    /// FontAwesome icon CSS class (e.g. "fas fa-file-image", "fas fa-folder")
    pub icon_class: String,

    /// Extra CSS class for icon styling (e.g. "image-icon", "folder-icon")
    pub icon_special_class: String,

    /// Human-readable category (e.g. "Image", "Folder")
    pub category: String,

    /// Formatted file size (e.g. "3.27 MB"); "--" for folders
    pub size_formatted: String,
}

impl FavoriteItemDto {
    /// Populate display fields from the enriched metadata.
    /// Call this after constructing from the SQL row.
    pub fn with_display_fields(mut self) -> Self {
        if self.item_type == "folder" {
            self.icon_class = "fas fa-folder".to_string();
            self.icon_special_class = "folder-icon".to_string();
            self.category = "Folder".to_string();
            self.size_formatted = "--".to_string();
        } else {
            let name = self.item_name.as_deref().unwrap_or("");
            let mime = self
                .item_mime_type
                .as_deref()
                .unwrap_or("application/octet-stream");
            let classes = classify_display(name, mime);
            self.icon_class = classes.icon_class.to_string();
            self.icon_special_class = classes.icon_special_class.to_string();
            self.category = classes.category.to_string();
            self.size_formatted = format_file_size(self.item_size.unwrap_or(0) as u64);
        }
        self
    }
}

/// Result DTO for batch add-to-favorites.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BatchFavoritesResult {
    /// Statistics about the batch operation
    pub stats: BatchFavoritesStats,
    /// Full list of the user's favourites (enriched), so the client can
    /// replace its local cache in a single round-trip.
    pub favorites: Vec<FavoriteItemDto>,
}

// ════════════════════════════════════════════════════════════════════════════
// Cursor-paginated favorites resources  (GET /api/favorites/resources)
// ════════════════════════════════════════════════════════════════════════════

/// Raw row returned by the UNION ALL query that joins `auth.user_favorites`
/// with `storage.files` / `storage.folders`.  Never serialised directly.
pub struct FavoriteResourceRow {
    pub resource_type: String, // "file" | "folder"
    pub resource_id: Uuid,
    pub name: String,
    pub parent_id: Option<Uuid>,
    pub mime_type: Option<String>,
    /// `-1` for folders, actual byte-count for files.
    pub size: i64,
    pub resource_created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
    /// Drive that owns this row. Surfaced on the favorites listing
    /// so a UI can tell when a favorited item lives in a different
    /// drive than the user's home (post-D6 cross-drive moves +
    /// copies make this reachable).
    pub drive_id: Uuid,
    /// Raw BLAKE3 content hash. `Some(_)` for file rows, `None` for
    /// folder rows. Routes into `FileDto::content_hash` and feeds
    /// `File::compute_etag` to populate `FileDto::etag`.
    pub blob_hash: Option<String>,
    /// `true` when `owner_id == requesting user_id`.
    pub is_owner: bool,
    pub favorited_at: DateTime<Utc>,
    /// Human-readable path (e.g. `Documents/Work` for a folder,
    /// `Documents/Work/report.pdf` for a file).  Always populated; the
    /// handler clears it to `""` when `is_owner` is false.
    pub path: Option<String>,
    // Pre-computed sort fields for cursor construction.
    pub sort_str: Option<String>,
    pub sort_int: Option<i64>,
    pub sort_ts: Option<DateTime<Utc>>,
}

/// Opaque keyset-pagination cursor for `GET /api/favorites/resources`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FavoritesCursor {
    /// Sort dimension active when this cursor was produced.
    /// Values: `"name"` (default), `"type"`, `"favorited_at"`, `"modified_at"`,
    /// `"size"`, `"owner"`.
    #[serde(default = "FavoritesCursor::default_order")]
    pub order_by: String,
    /// UUID of the last item on the previous page (tie-breaker).
    pub resource_id: Uuid,
    /// `LOWER(name)` for `name`/`type` sorts; `LOWER(username)` for `owner`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_str: Option<String>,
    /// Multipurpose integer: `folder_first` for `name`, `type_order` for `type`,
    /// size in bytes for `size`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_int: Option<i64>,
    /// Timestamp for `favorited_at` and `modified_at` sorts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_ts: Option<DateTime<Utc>>,
    /// Whether the result set was reversed — must match on every page.
    #[serde(default)]
    pub reverse: bool,
}

impl FavoritesCursor {
    fn default_order() -> String {
        "name".to_owned()
    }
}

impl PageCursor for FavoritesCursor {}

/// Query parameters for `GET /api/favorites/resources`.
#[derive(Debug, Deserialize, IntoParams)]
pub struct FavoritesResourcesQuery {
    /// Maximum items per page (1–200, default 50).
    #[serde(default = "CursorQuery::default_limit")]
    pub limit: u32,
    /// Opaque cursor from a previous response. Omit to start from the first page.
    pub cursor: Option<String>,
    /// Sort / group-by dimension. Supported: `"name"` (default), `"type"`,
    /// `"favorited_at"`, `"modified_at"`, `"size"`, `"owner"`.
    pub order_by: Option<String>,
    /// Comma-separated resource types to include, e.g. `"file,folder"`.
    /// Omit to include both.
    pub resource_types: Option<String>,
    /// Reverse the sort order. Default `false`.
    #[serde(default)]
    pub reverse: bool,
}

impl FavoritesResourcesQuery {
    pub fn limit_clamped(&self) -> usize {
        self.limit.clamp(1, 200) as usize
    }

    pub fn decode_cursor(&self) -> Option<FavoritesCursor> {
        self.cursor.as_deref().and_then(FavoritesCursor::decode)
    }

    /// Returns `None` when `resource_types` is absent (= include all).
    pub fn resource_kinds(&self) -> Option<Vec<ResourceKind>> {
        self.resource_types.as_deref().map(|s| {
            s.split(',')
                .filter_map(|t| ResourceKind::parse(t.trim()))
                .collect()
        })
    }
}

/// One item in a `GET /api/favorites/resources` page.
#[derive(Debug, Serialize, ToSchema)]
pub struct FavoritesResourceItemDto {
    pub resource_type: ResourceTypeDto,
    /// When the resource was added to the user's favorites.
    pub favorited_at: DateTime<Utc>,
    /// Full resource details — shape determined by `resource_type`.
    pub resource: ResourceContentDto,
}

/// Response envelope for `GET /api/favorites/resources`.
pub type FavoritesResourcesDto = CursorListResponse<FavoritesResourceItemDto>;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BatchFavoritesStats {
    /// How many items were requested
    pub requested: usize,
    /// How many were actually inserted (new)
    pub inserted: u64,
    /// How many were already favourites (skipped)
    pub already_existed: u64,
}
