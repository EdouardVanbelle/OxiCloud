use axum::{
    extract::{Json, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use tracing::{error, info};

use crate::application::dtos::search_dto::{
    SearchCriteriaDto, SearchResultsDto, SearchSuggestionsDto,
};
use crate::application::ports::inbound::SearchUseCase;
use crate::common::di::AppState;
use crate::interfaces::errors::AppError;
use crate::interfaces::middleware::auth::AuthUser;
use std::sync::Arc;

/**
 * Handler for search operations through the API.
 *
 * All search processing (filtering, scoring, sorting, categorization,
 * formatting) is performed server-side. These handlers are thin HTTP
 * adapters that delegate to the SearchUseCase.
 */
/// Hard cap on the search page size. The default is 100; without a ceiling a
/// client could pass `?limit=<huge>`, which flows straight into the SQL `LIMIT`
/// and would pull that many rows into memory (and into the result cache). 500
/// is a generous page for a search UI — `total_count` still reflects the full
/// match set, so deeper results stay reachable via `offset`. Mirrors the
/// suggestions endpoint, which already clamps with `.min(20)`.
const MAX_SEARCH_LIMIT: usize = 500;

pub struct SearchHandler;

impl SearchHandler {
    // ── Why no #[utoipa::path] here? ─────────────────────────────────────────────
    // utoipa 5.4.0's proc macro generates helper structs / impls inside its expansion.
    // Rust allows struct definitions at module scope but forbids them inside impl blocks,
    // so `#[utoipa::path]` fails on every method in this impl block regardless of HTTP
    // verb or annotation content. All route handlers are free functions below.
    // TODO: collapse after utoipa upgrade.
    pub(super) async fn search_files_get_impl(
        State(state): State<Arc<AppState>>,
        auth_user: AuthUser,
        Query(params): Query<SearchParams>,
    ) -> impl IntoResponse {
        info!("API: File search with parameters: {:?}", params);

        let search_service = match &state.applications.search_service {
            Some(service) => service,
            None => {
                error!("Search service not available");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({ "error": "Search service is not available" })),
                )
                    .into_response();
            }
        };

        let search_criteria = SearchCriteriaDto {
            name_contains: params.query,
            file_types: params
                .type_filter
                .map(|t| t.split(',').map(|s| s.trim().to_string()).collect()),
            created_after: params.created_after,
            created_before: params.created_before,
            modified_after: params.modified_after,
            modified_before: params.modified_before,
            min_size: params.min_size,
            max_size: params.max_size,
            folder_id: params.folder_id,
            recursive: params.recursive.unwrap_or(true),
            limit: params.limit.unwrap_or(100).min(MAX_SEARCH_LIMIT),
            offset: params.offset.unwrap_or(0),
            sort_by: params.sort_by.unwrap_or_else(|| "relevance".to_string()),
        };

        match search_service.search(search_criteria, auth_user.id).await {
            Ok(results) => {
                info!(
                    "Search completed in {}ms — {} files, {} folders",
                    results.query_time_ms,
                    results.files.len(),
                    results.folders.len()
                );
                {
                    // Pre-sized serialization (benches/ROUND12.md §M1).
                    let rows = results.files.len() + results.folders.len();
                    crate::interfaces::api::sized_json::sized_json(
                        256 + rows * crate::interfaces::api::sized_json::EST_WRAPPED_ROW_BYTES,
                        &*results,
                    )
                }
            }
            Err(err) => {
                error!("Search error: {}", err);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "Search error" })),
                )
                    .into_response()
            }
        }
    }

    /// Advanced search with full criteria in the request body.
    pub(super) async fn search_files_post_impl(
        State(state): State<Arc<AppState>>,
        auth_user: AuthUser,
        Json(criteria): Json<SearchCriteriaDto>,
    ) -> impl IntoResponse {
        info!("API: Advanced file search");

        let search_service = match &state.applications.search_service {
            Some(service) => service,
            None => {
                error!("Search service not available");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({ "error": "Search service is not available" })),
                )
                    .into_response();
            }
        };

        match search_service.search(criteria, auth_user.id).await {
            Ok(results) => {
                info!(
                    "Advanced search completed in {}ms — {} files, {} folders",
                    results.query_time_ms,
                    results.files.len(),
                    results.folders.len()
                );
                {
                    // Pre-sized serialization (benches/ROUND12.md §M1).
                    let rows = results.files.len() + results.folders.len();
                    crate::interfaces::api::sized_json::sized_json(
                        256 + rows * crate::interfaces::api::sized_json::EST_WRAPPED_ROW_BYTES,
                        &*results,
                    )
                }
            }
            Err(err) => {
                error!("Search error: {}", err);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "Search error" })),
                )
                    .into_response()
            }
        }
    }

    /// Autocomplete suggestions for search.
    pub(super) async fn suggest_files_impl(
        State(state): State<Arc<AppState>>,
        auth_user: AuthUser,
        Query(params): Query<SuggestParams>,
    ) -> impl IntoResponse {
        info!("API: Search suggestions for {:?}", params.query);

        let search_service = match &state.applications.search_service {
            Some(service) => service,
            None => {
                error!("Search service not available");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({ "error": "Search service is not available" })),
                )
                    .into_response();
            }
        };

        let limit = params.limit.unwrap_or(10).min(20);

        match search_service
            .suggest_with_perms(
                &params.query,
                params.folder_id.as_deref(),
                limit,
                auth_user.id,
            )
            .await
        {
            Ok(suggestions) => {
                info!(
                    "Suggestions completed in {}ms — {} results",
                    suggestions.query_time_ms,
                    suggestions.suggestions.len()
                );
                (StatusCode::OK, Json(suggestions)).into_response()
            }
            Err(err) => {
                error!("Suggestions error: {}", err);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "Suggestions error" })),
                )
                    .into_response()
            }
        }
    }

    /// `DELETE /admin/search/cache` — flush the shared moka search
    /// results cache. Admin-only.
    ///
    /// AuthZ audit #14 (2026-07-12): pre-fix this endpoint lived at
    /// `/api/search/cache` and required only a valid JWT — any
    /// authenticated user (external / magic-link included) could
    /// DELETE it in a loop and keep the results cache cold indefinitely
    /// (sustained DoS on every subsequent `/api/search` query). Now
    /// mounted at `/api/admin/search/cache`, gated by the
    /// `require_admin` middleware layer on the `/api/admin` nest point.
    /// The handler no longer needs an inline authz call — reaching
    /// this code implies `AuthUser` is admin by construction. Audit
    /// line on success so operator-driven flushes are traceable in
    /// security reviews.
    pub(super) async fn clear_search_cache_impl(
        State(state): State<Arc<AppState>>,
        auth_user: AuthUser,
    ) -> Result<Response, AppError> {
        let caller_id = auth_user.id;
        info!("API: Clearing search cache");

        let Some(search_service) = &state.applications.search_service else {
            error!("Search service not available");
            return Ok((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "Search service is not available" })),
            )
                .into_response());
        };

        match search_service.clear_search_cache().await {
            Ok(_) => {
                tracing::info!(
                    target: "audit",
                    event = "search.cache_cleared",
                    caller_id = %caller_id,
                    "🧹 search results cache flushed by admin",
                );
                Ok((
                    StatusCode::OK,
                    Json(json!({ "message": "Search cache cleared successfully" })),
                )
                    .into_response())
            }
            Err(err) => {
                error!("Error clearing search cache: {}", err);
                Ok((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "Error clearing search cache" })),
                )
                    .into_response())
            }
        }
    }
}

/// Search parameters for the GET /search endpoint
#[derive(Debug, serde::Deserialize)]
pub struct SearchParams {
    /// Text to search in file and folder names
    pub query: Option<String>,

    /// Filter by file types (comma-separated extensions)
    #[serde(rename = "type")]
    pub type_filter: Option<String>,

    /// Created after this timestamp
    pub created_after: Option<u64>,

    /// Created before this timestamp
    pub created_before: Option<u64>,

    /// Modified after this timestamp
    pub modified_after: Option<u64>,

    /// Modified before this timestamp
    pub modified_before: Option<u64>,

    /// Minimum file size in bytes
    pub min_size: Option<u64>,

    /// Maximum file size in bytes
    pub max_size: Option<u64>,

    /// Folder ID to limit the search scope
    pub folder_id: Option<String>,

    /// Recursive search in subfolders (default: true)
    pub recursive: Option<bool>,

    /// Result limit for pagination
    pub limit: Option<usize>,

    /// Offset for pagination
    pub offset: Option<usize>,

    /// Sort order: relevance | name | name_desc | date | date_desc | size | size_desc
    pub sort_by: Option<String>,
}

/// Parameters for the GET /search/suggest endpoint
#[derive(Debug, serde::Deserialize)]
pub struct SuggestParams {
    /// Text to search for suggestions
    pub query: String,

    /// Folder ID to limit the suggestion scope
    pub folder_id: Option<String>,

    /// Maximum number of suggestions (default 10, max 20)
    pub limit: Option<usize>,
}

// ── Route handlers (free functions) ──────────────────────────────────────────
//
// All four route functions live here rather than as methods on SearchHandler
// because utoipa 5.4.0's #[utoipa::path] macro generates helper structs inside
// its expansion. Rust allows struct definitions at module scope but forbids them
// inside impl blocks — so every #[utoipa::path] annotation on a SearchHandler
// method fails to compile regardless of HTTP verb or annotation content.
//
// All logic lives in the SearchHandler::*_impl methods above; these thin wrappers
// exist solely to carry the OpenAPI annotation at a scope where utoipa can
// generate its helper types.
//
// routes.rs calls these free functions directly.
// TODO: collapse back into the impl block after a utoipa upgrade resolves the issue.

#[utoipa::path(
    get,
    path = "/api/search",
    params(
        ("query" = Option<String>, Query, description = "Text to search in names"),
        ("type" = Option<String>, Query, description = "Comma-separated MIME type filter"),
        ("folder_id" = Option<String>, Query, description = "Restrict search to this folder"),
        ("recursive" = Option<bool>, Query, description = "Include sub-folders"),
        ("limit" = Option<u32>, Query, description = "Max results"),
        ("offset" = Option<u32>, Query, description = "Pagination offset"),
    ),
    responses(
        (status = 200, description = "Search results", body = SearchResultsDto),
        (status = 503, description = "Search service unavailable"),
    ),
    security(("bearerAuth" = [])),
    tag = "search"
)]
pub async fn search_files_get(
    state: State<Arc<AppState>>,
    auth_user: AuthUser,
    query: Query<SearchParams>,
) -> impl IntoResponse {
    SearchHandler::search_files_get_impl(state, auth_user, query).await
}

#[utoipa::path(
    post,
    path = "/api/search/advanced",
    request_body(content = SearchCriteriaDto, content_type = "application/json", description = "Search criteria"),
    responses(
        (status = 200, description = "Search results", body = SearchResultsDto),
        (status = 503, description = "Search service unavailable"),
    ),
    security(("bearerAuth" = [])),
    tag = "search"
)]
pub async fn search_files_post(
    state: State<Arc<AppState>>,
    auth_user: AuthUser,
    json: Json<SearchCriteriaDto>,
) -> impl IntoResponse {
    SearchHandler::search_files_post_impl(state, auth_user, json).await
}

#[utoipa::path(
    get,
    path = "/api/search/suggest",
    params(
        ("query" = String, Query, description = "Partial name to complete"),
        ("folder_id" = Option<String>, Query, description = "Restrict to this folder"),
        ("limit" = Option<u32>, Query, description = "Max suggestions (default 10, max 20)"),
    ),
    responses(
        (status = 200, description = "Suggestions", body = SearchSuggestionsDto),
        (status = 503, description = "Search service unavailable"),
    ),
    security(("bearerAuth" = [])),
    tag = "search"
)]
pub async fn suggest_files(
    state: State<Arc<AppState>>,
    auth_user: AuthUser,
    query: Query<SuggestParams>,
) -> impl IntoResponse {
    SearchHandler::suggest_files_impl(state, auth_user, query).await
}

#[utoipa::path(
    delete,
    path = "/api/admin/search/cache",
    responses(
        (status = 200, description = "Cache cleared"),
        (status = 401, description = "Missing or invalid token"),
        (status = 403, description = "Caller is not an admin"),
        (status = 503, description = "Search service unavailable"),
    ),
    security(("bearerAuth" = [])),
    tag = "admin"
)]
pub async fn clear_search_cache(
    state: State<Arc<AppState>>,
    auth_user: AuthUser,
) -> Result<Response, AppError> {
    SearchHandler::clear_search_cache_impl(state, auth_user).await
}
