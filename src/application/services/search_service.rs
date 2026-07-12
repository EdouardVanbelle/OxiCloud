use std::cmp::Reverse;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::application::dtos::display_helpers::{
    category_for, icon_class_for, icon_special_class_for,
};
use crate::application::dtos::file_dto::FileDto;
use crate::application::dtos::folder_dto::FolderDto;
use crate::application::dtos::search_dto::{
    SearchCriteriaDto, SearchFileResultDto, SearchFolderResultDto, SearchResultsDto,
    SearchSuggestionItem, SearchSuggestionsDto,
};
use crate::application::ports::content_index_ports::{ContentHitDto, ContentIndexPort};
use crate::application::ports::inbound::SearchUseCase;
use crate::application::ports::storage_ports::FileReadPort;
use crate::common::errors::Result;
use crate::domain::entities::folder::Folder;
use crate::domain::repositories::folder_repository::FolderRepository;
use crate::infrastructure::repositories::pg::file_blob_read_repository::FileBlobReadRepository;
use crate::infrastructure::repositories::pg::folder_db_repository::FolderDbRepository;
use std::hash::{Hash, Hasher};
use uuid::Uuid;

/**
 * High-performance search service implementation for files and folders.
 *
 * All search processing (filtering, scoring, sorting, categorization,
 * formatting) is performed server-side in Rust for maximum efficiency.
 * The frontend acts as a thin rendering client only.
 *
 * Features:
 * - Single-query recursive subtree search via PostgreSQL ltree
 * - Relevance scoring (exact match > starts-with > contains)
 * - Content categorization and icon mapping
 * - Multiple sort options (relevance, name, date, size)
 * - Server-side formatted file sizes
 * - Quick suggestions endpoint for autocomplete
 * - TTL-based result caching
 */
pub struct SearchService {
    /// Repository for file operations
    file_repository: Arc<FileBlobReadRepository>,

    /// Repository for folder operations
    folder_repository: Arc<FolderDbRepository>,

    /// Optional full-text content index (embedded Tantivy). When present,
    /// query-bearing searches additionally surface files whose CONTENT
    /// matches; hits are hydrated and re-filtered through SQL before use.
    content_index: Option<Arc<dyn ContentIndexPort>>,

    /// Optional authorization engine — needed to resolve the caller's
    /// accessible drive set before querying the content index, and to
    /// re-verify each Tantivy hit against `engine.check(Read, File(id))`
    /// as a defense-in-depth measure (catches index staleness and
    /// per-file grants that the drive-only Tantivy filter misses; see
    /// `docs/plan/drive.md` §11). `None` short-circuits the content
    /// index (the cheapest safe degradation).
    authorization: Option<Arc<crate::infrastructure::services::pg_acl_engine::PgAclEngine>>,

    /// Optional drive repository — used in tandem with the authorization
    /// engine to resolve the caller's accessible drives for the Tantivy
    /// filter. `None` short-circuits the content index.
    drive_repo: Option<Arc<dyn crate::domain::repositories::drive_repository::DriveRepository>>,

    /// Lock-free concurrent cache with automatic TTL and LRU eviction (moka).
    /// Values are `Arc<SearchResultsDto>` so cache insert/hit is a single
    /// atomic ref-count increment (~1 ns) instead of cloning thousands of Strings.
    search_cache: moka::future::Cache<u64, Arc<SearchResultsDto>>,
}

// ─── Utility functions (pure, no self — computed on the server) ─────────

/// Compute relevance score (0–100) for a name against a query.
/// Exact match = 100, starts-with = 80, contains = 50, no match = 0.
///
/// `query_lower` **must** already be lowercased by the caller so that the
/// allocation happens once per search, not once per result.
fn compute_relevance(name: &str, query_lower: &str) -> u32 {
    let name_lower = name.to_lowercase();

    if name_lower == query_lower {
        100
    } else if name_lower.starts_with(query_lower) {
        80
    } else if name_lower.contains(query_lower) {
        // Bonus for shorter names (more specific match)
        let ratio = query_lower.len() as f64 / name_lower.len() as f64;
        50 + (ratio * 20.0) as u32
    } else {
        0
    }
}

/// Max content-index candidates fetched per search. Hydration re-filters
/// them in ONE SQL round-trip, so this bounds both index and DB work.
const CONTENT_HITS_LIMIT: usize = 200;

/// Map a BM25 score into the 10–45 relevance band, normalized against the
/// best score of the result set. Deliberately below the weakest name match
/// (contains = 50): a filename hit is more specific than a body mention.
fn content_relevance(score: f32, max_score: f32) -> u32 {
    if !score.is_finite() || max_score <= 0.0 {
        return 10;
    }
    let ratio = (score / max_score).clamp(0.0, 1.0);
    10 + (ratio * 35.0).round() as u32
}

/// Re-sort the merged file list with the same semantics the folder list
/// uses. Only invoked when content hits were merged into a SQL-ordered page.
fn sort_enriched_files(files: &mut [SearchFileResultDto], sort_by: &str) {
    match sort_by {
        "name" => files.sort_by_cached_key(|f| f.name.to_lowercase()),
        "name_desc" => files.sort_by_cached_key(|f| Reverse(f.name.to_lowercase())),
        "date" => files.sort_by_key(|f| f.modified_at),
        "date_desc" => files.sort_by_key(|f| Reverse(f.modified_at)),
        "size" => files.sort_by_key(|f| f.size),
        "size_desc" => files.sort_by_key(|f| Reverse(f.size)),
        _ => files.sort_by_key(|f| Reverse(f.relevance_score)),
    }
}

/// Format bytes into a human-readable string (e.g. "2.5 MB").
fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let exp = (bytes as f64).log(1024.0).floor() as usize;
    let exp = exp.min(UNITS.len() - 1);
    let value = bytes as f64 / 1024_f64.powi(exp as i32);
    if exp == 0 {
        format!("{} B", bytes)
    } else {
        format!("{:.1} {}", value, UNITS[exp])
    }
}

/// Get Font Awesome icon class for a file based on extension and MIME type.
/// Delegates to the centralised `display_helpers` so every API surface is
/// consistent.
fn get_icon_class(name: &str, mime: &str) -> String {
    icon_class_for(name, mime).to_string()
}

/// Get CSS special class for icon styling.
fn get_icon_special_class(name: &str, mime: &str) -> String {
    icon_special_class_for(name, mime).to_string()
}

/// Get category label from centralised helpers.
fn get_category(name: &str, mime: &str) -> String {
    category_for(name, mime).to_string()
}

// ─── SearchService implementation ───────────────────────────────────────

impl SearchService {
    /**
     * Creates a new instance of the search service.
     */
    pub fn new(
        file_repository: Arc<FileBlobReadRepository>,
        folder_repository: Arc<FolderDbRepository>,
        content_index: Option<Arc<dyn ContentIndexPort>>,
        authorization: Option<Arc<crate::infrastructure::services::pg_acl_engine::PgAclEngine>>,
        drive_repo: Option<Arc<dyn crate::domain::repositories::drive_repository::DriveRepository>>,
        cache_ttl: u64,
        max_cache_size: usize,
    ) -> Self {
        let search_cache = moka::future::Cache::builder()
            .max_capacity(max_cache_size as u64)
            .time_to_live(Duration::from_secs(cache_ttl))
            .build();

        Self {
            file_repository,
            folder_repository,
            content_index,
            authorization,
            drive_repo,
            search_cache,
        }
    }

    /// Creates a cache key from the search criteria using zero-allocation hashing.
    fn create_cache_key(criteria: &SearchCriteriaDto, user_id: &str) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        criteria.hash(&mut hasher);
        user_id.hash(&mut hasher);
        hasher.finish()
    }

    /// Enrich a FileDto → SearchFileResultDto with server-computed metadata.
    ///
    /// `query_lower` must already be lowercased (empty string when no query).
    fn enrich_file(file: &FileDto, query_lower: &str) -> SearchFileResultDto {
        let relevance = if query_lower.is_empty() {
            50
        } else {
            compute_relevance(&file.name, query_lower)
        };

        SearchFileResultDto {
            id: file.id.clone(),
            name: file.name.clone(),
            path: file.path.clone(),
            size: file.size,
            mime_type: file.mime_type.to_string(),
            folder_id: file.folder_id.clone(),
            created_at: file.created_at,
            modified_at: file.modified_at,
            relevance_score: relevance,
            size_formatted: format_bytes(file.size),
            icon_class: get_icon_class(&file.name, &file.mime_type),
            icon_special_class: get_icon_special_class(&file.name, &file.mime_type),
            category: get_category(&file.name, &file.mime_type),
            // Carry the content hash through so REPORT/SEARCH
            // responses on the NC surface can emit the same ETag
            // (`File::compute_etag`) as PROPFIND/GET would.
            blob_hash: file.content_hash.clone(),
            snippet: None,
            match_source: (!query_lower.is_empty() && relevance > 0).then(|| "name".to_string()),
        }
    }

    /// Enrich a FolderDto → SearchFolderResultDto with server-computed metadata.
    ///
    /// `query_lower` must already be lowercased (empty string when no query).
    fn enrich_folder(folder: &FolderDto, query_lower: &str) -> SearchFolderResultDto {
        let relevance = if query_lower.is_empty() {
            50
        } else {
            compute_relevance(&folder.name, query_lower)
        };

        SearchFolderResultDto {
            id: folder.id.clone(),
            name: folder.name.clone(),
            path: folder.path.clone(),
            parent_id: folder.parent_id.clone(),
            drive_id: folder.drive_id,
            created_at: folder.created_at,
            modified_at: folder.modified_at,
            is_root: folder.is_root,
            relevance_score: relevance,
        }
    }

    /// Query the content index for files matching by CONTENT (when the index
    /// is enabled). First page only — content hits have no stable
    /// interleaving with SQL pagination beyond it, and page one is where
    /// search UX lives. Index failures degrade to name-only results, never
    /// to a failed search.
    async fn lookup_content_hits(
        &self,
        criteria: &SearchCriteriaDto,
        user_id: Uuid,
    ) -> Vec<ContentHitDto> {
        use crate::application::ports::authorization_ports::AuthorizationEngine;
        use crate::domain::services::authorization::{Permission, Resource, Subject};

        let Some(index) = &self.content_index else {
            return Vec::new();
        };
        let Some(authz) = &self.authorization else {
            return Vec::new();
        };
        let Some(drive_repo) = &self.drive_repo else {
            return Vec::new();
        };
        if criteria.offset != 0 {
            return Vec::new();
        }
        let Some(query) = criteria
            .name_contains
            .as_deref()
            .map(str::trim)
            .filter(|q| q.len() >= 2)
        else {
            return Vec::new();
        };

        // Resolve the caller's accessible drive set. Group-mediated
        // grants are honoured inline by `storage.caller_group_ids` on
        // the SQL side, so no Rust-side subject expansion here.
        let accessible_drives: Vec<Uuid> = match drive_repo.list_readable_by(user_id).await {
            Ok(drives) => drives.into_iter().map(|d| d.drive.id).collect(),
            Err(e) => {
                tracing::warn!("Content-index: drive lookup failed — degrading to empty: {e}");
                return Vec::new();
            }
        };

        // Tantivy filter (Must drive_id ∈ accessible_drives) handles
        // the cross-drive isolation. Empty drive list short-circuits
        // inside `search_content`.
        let hits = match index
            .search_content(&accessible_drives, query, CONTENT_HITS_LIMIT)
            .await
        {
            Ok(hits) => hits,
            Err(e) => {
                tracing::warn!("Content-index lookup failed — returning name-only results: {e}");
                return Vec::new();
            }
        };

        // Defense in depth: re-verify each hit through the engine.
        // Catches two cases the drive_id filter can't:
        //   * Index staleness — the file just moved drives and the
        //     worker hasn't caught up.
        //   * Per-file grants — ReBAC can grant a single file inside a
        //     drive the caller doesn't otherwise have. The Tantivy
        //     filter is drive-only; this re-check restores per-file
        //     resolution.
        // Failures degrade conservatively (drop the hit, log it) —
        // never leak.
        let mut verified = Vec::with_capacity(hits.len());
        for hit in hits {
            let file_uuid = match Uuid::parse_str(&hit.file_id) {
                Ok(u) => u,
                Err(_) => {
                    tracing::warn!("Content-index hit had non-UUID file_id: {}", hit.file_id);
                    continue;
                }
            };
            match authz
                .check(
                    Subject::User(user_id),
                    Permission::Read,
                    Resource::File(file_uuid),
                )
                .await
            {
                Ok(true) => verified.push(hit),
                Ok(false) => {
                    tracing::debug!(
                        target: "oxicloud::search",
                        file_id = %file_uuid,
                        "dropping content-index hit: ReBAC denies Read after Tantivy filter",
                    );
                }
                Err(e) => {
                    tracing::warn!("ReBAC re-check failed for {file_uuid}: {e}");
                }
            }
        }
        verified
    }

    /// Merge content-index hits into the name-search result page:
    /// * files the name search already found just gain their `snippet`;
    /// * content-only candidates are hydrated through SQL in one round-trip
    ///   (re-applying user scope, trash state and every active filter — a
    ///   stale index id silently drops out), enriched, scored into the
    ///   content relevance band and appended;
    /// * the merged page is re-sorted with the caller's `sort_by`.
    ///
    /// Returns how many files were added (callers bump their totals by it).
    async fn merge_content_hits(
        &self,
        hits: Vec<ContentHitDto>,
        enriched_files: &mut Vec<SearchFileResultDto>,
        criteria: &SearchCriteriaDto,
        user_id: Uuid,
    ) -> Result<usize> {
        if hits.is_empty() {
            return Ok(0);
        }

        let mut by_id: std::collections::HashMap<&str, &ContentHitDto> =
            hits.iter().map(|h| (h.file_id.as_str(), h)).collect();
        for file in enriched_files.iter_mut() {
            if let Some(hit) = by_id.remove(file.id.as_str()) {
                file.snippet = hit.snippet.clone();
            }
        }
        if by_id.is_empty() {
            return Ok(0);
        }

        // Preserve the index's score order when collecting the leftovers.
        let candidate_ids: Vec<String> = hits
            .iter()
            .filter(|h| by_id.contains_key(h.file_id.as_str()))
            .map(|h| h.file_id.clone())
            .collect();
        let files = self
            .file_repository
            .fetch_files_by_ids_filtered(&candidate_ids, criteria, user_id)
            .await?;
        if files.is_empty() {
            return Ok(0);
        }

        let max_score = hits.iter().map(|h| h.score).fold(0.0_f32, f32::max);
        let mut added = 0usize;
        for file in files {
            let dto = FileDto::from(file);
            let Some(hit) = by_id.get(dto.id.as_str()) else {
                continue;
            };
            let mut enriched = Self::enrich_file(&dto, "");
            enriched.relevance_score = content_relevance(hit.score, max_score);
            enriched.snippet = hit.snippet.clone();
            enriched.match_source = Some("content".to_string());
            enriched_files.push(enriched);
            added += 1;
        }
        if added > 0 {
            sort_enriched_files(enriched_files, &criteria.sort_by);
        }
        Ok(added)
    }

    /// Quick suggestions search — returns up to `limit` name suggestions
    /// matching the query. Pushes filtering, relevance sort and LIMIT to SQL
    /// so only a handful of rows cross the DB→app boundary.
    pub async fn suggest(
        &self,
        query: &str,
        folder_id: Option<&str>,
        limit: usize,
    ) -> Result<SearchSuggestionsDto> {
        let start = Instant::now();

        // Ask SQL for at most `limit` best-matching files and folders
        let (files, folders) = tokio::join!(
            self.file_repository
                .suggest_files_by_name(folder_id, query, limit),
            self.folder_repository
                .suggest_folders_by_name(folder_id, query, limit),
        );
        let files = files?;
        let folders = folders?;

        let mut suggestions: Vec<SearchSuggestionItem> =
            Vec::with_capacity(files.len() + folders.len());

        // Pre-compute once — avoids N heap allocations inside the loops.
        let query_lower = query.to_lowercase();

        for file in &files {
            let file_dto = FileDto::from(file.clone());
            let score = compute_relevance(&file_dto.name, &query_lower);
            suggestions.push(SearchSuggestionItem {
                name: file_dto.name.clone(),
                item_type: "file".to_string(),
                id: file_dto.id.clone(),
                path: file_dto.path.clone(),
                icon_class: get_icon_class(&file_dto.name, &file_dto.mime_type),
                icon_special_class: get_icon_special_class(&file_dto.name, &file_dto.mime_type),
                relevance_score: score,
            });
        }

        for folder in &folders {
            let folder_dto = FolderDto::from(folder.clone());
            let score = compute_relevance(&folder_dto.name, &query_lower);
            suggestions.push(SearchSuggestionItem {
                name: folder_dto.name.clone(),
                item_type: "folder".to_string(),
                id: folder_dto.id.clone(),
                path: folder_dto.path.clone(),
                icon_class: "fas fa-folder".to_string(),
                icon_special_class: "folder-icon".to_string(),
                relevance_score: score,
            });
        }

        // Merge files + folders by relevance and truncate to the final limit
        suggestions.sort_by_key(|f| Reverse(f.relevance_score));
        suggestions.truncate(limit);

        let elapsed = start.elapsed().as_millis() as u64;
        Ok(SearchSuggestionsDto {
            suggestions,
            query_time_ms: elapsed,
        })
    }
}

// ─── SearchUseCase trait implementation ──────────────────────────────────

impl SearchUseCase for SearchService {
    /**
     * Performs a search based on the specified criteria.
     *
     * Optimization: For non-recursive searches, uses database-level pagination
     * for better performance. For recursive searches, uses the parallel approach.
     *
     * All processing happens server-side:
     * - Database-level pagination for non-recursive searches
     * - Parallel recursive traversal for recursive searches
     * - Filtering by name, type, dates, size
     * - Relevance scoring
     * - Sorting (relevance, name, date, size)
     * - Content categorization & icon mapping
     * - Human-readable size formatting
     * - Pagination
     */
    async fn search(
        &self,
        criteria: SearchCriteriaDto,
        user_id: Uuid,
    ) -> Result<Arc<SearchResultsDto>> {
        let user_id_str = user_id.to_string();
        let cache_key = Self::create_cache_key(&criteria, &user_id_str);

        // Single-flight: collapse N identical concurrent searches into ONE
        // execution. `try_get_with` serves the cached result on a hit and, on a
        // miss, runs the closure exactly once while the other callers await it
        // — so a burst of identical queries no longer floods Postgres or drains
        // the connection pool (the old get-from-cache fast path is subsumed).
        self.search_cache
            .try_get_with(cache_key, async move {
                let start = Instant::now();
                let query = criteria.name_contains.as_deref().unwrap_or("");
                // Pre-compute once — avoids N heap allocations inside enrich_file/enrich_folder.
                let query_lower = query.to_lowercase();

                // Content-index candidates (first page only). Feature-off or an
                // index failure yields an empty set — the search stays name-only.
                let content_hits = self.lookup_content_hits(&criteria, user_id).await;

                // For non-recursive searches, use efficient database-level pagination
                // This avoids loading all files into memory
                if !criteria.recursive {
                    // Use database-level pagination
                    let (files, total_file_count) = self
                        .file_repository
                        .search_files_paginated(criteria.folder_id.as_deref(), &criteria, user_id)
                        .await?;

                    // Convert to DTOs and enrich with metadata
                    let file_dtos: Vec<FileDto> = files.into_iter().map(FileDto::from).collect();
                    let mut enriched_files: Vec<SearchFileResultDto> = file_dtos
                        .iter()
                        .map(|f| Self::enrich_file(f, &query_lower))
                        .collect();

                    // Get folders for this folder (non-recursive, filtered in SQL)
                    let folders = self
                        .folder_repository
                        .search_folders(
                            criteria.folder_id.as_deref(),
                            criteria.name_contains.as_deref(),
                            user_id,
                            false,
                        )
                        .await?;

                    let filtered_folders: Vec<FolderDto> =
                        folders.into_iter().map(FolderDto::from).collect();

                    // For folders, apply sorting and pagination in memory (usually fewer folders)
                    let mut enriched_folders: Vec<SearchFolderResultDto> = filtered_folders
                        .iter()
                        .map(|f| Self::enrich_folder(f, &query_lower))
                        .collect();

                    // Sort folders (cached_key avoids O(N log N) temporary String allocations)
                    match criteria.sort_by.as_str() {
                        "name" => {
                            enriched_folders.sort_by_cached_key(|f| f.name.to_lowercase());
                        }
                        "name_desc" => {
                            enriched_folders.sort_by_cached_key(|f| Reverse(f.name.to_lowercase()));
                        }
                        "date" => {
                            enriched_folders.sort_by_key(|f| f.modified_at);
                        }
                        "date_desc" => {
                            enriched_folders.sort_by_key(|f| Reverse(f.modified_at));
                        }
                        _ => {
                            enriched_folders.sort_by_key(|f| Reverse(f.relevance_score));
                        }
                    }

                    // Blend in content-discovered files before the pagination math.
                    let added = self
                        .merge_content_hits(content_hits, &mut enriched_files, &criteria, user_id)
                        .await?;
                    let total_file_count = total_file_count + added;

                    let folder_count = enriched_folders.len();
                    let total_count = total_file_count + folder_count;

                    // Combine and paginate (folders first, then files)
                    let start_idx = criteria.offset.min(total_count);
                    let end_idx = (criteria.offset + criteria.limit).min(total_count);

                    let folder_start = start_idx.min(folder_count);
                    let folder_end = end_idx.min(folder_count);
                    let paginated_folders = enriched_folders[folder_start..folder_end].to_vec();

                    let file_start = start_idx.saturating_sub(folder_count);
                    let file_end = end_idx
                        .saturating_sub(folder_count)
                        .min(enriched_files.len());
                    let paginated_files = enriched_files[file_start..file_end].to_vec();

                    let elapsed_ms = start.elapsed().as_millis() as u64;

                    let search_results = Arc::new(SearchResultsDto::new(
                        paginated_files,
                        paginated_folders,
                        criteria.limit,
                        criteria.offset,
                        Some(total_count),
                        elapsed_ms,
                        criteria.sort_by.clone(),
                    ));

                    return Ok(search_results);
                }

                // ── Recursive search via ltree (single SQL query per entity type) ──
                // Uses PostgreSQL ltree GiST index to find all files and folders
                // in the subtree in O(1) queries, replacing the O(N) spawn-per-folder
                // approach that could saturate the connection pool.
                let (found_files, total_file_count) = self
                    .file_repository
                    .search_files_in_subtree(criteria.folder_id.as_deref(), &criteria, user_id)
                    .await?;

                // Get folders (SQL-filtered, user-scoped, recursive when applicable)
                let found_folders: Vec<Folder> = self
                    .folder_repository
                    .search_folders(
                        criteria.folder_id.as_deref(),
                        criteria.name_contains.as_deref(),
                        user_id,
                        true,
                    )
                    .await?;

                // ── Convert to DTOs and enrich with server-computed metadata ──
                let file_dtos: Vec<FileDto> = found_files.into_iter().map(FileDto::from).collect();
                let mut enriched_files: Vec<SearchFileResultDto> = file_dtos
                    .iter()
                    .map(|f| Self::enrich_file(f, &query_lower))
                    .collect();

                let folder_dtos: Vec<FolderDto> =
                    found_folders.into_iter().map(FolderDto::from).collect();
                let mut enriched_folders: Vec<SearchFolderResultDto> = folder_dtos
                    .iter()
                    .map(|f| Self::enrich_folder(f, &query_lower))
                    .collect();

                // ── Sort folders (cached_key avoids O(N log N) temporary String allocations) ──
                match criteria.sort_by.as_str() {
                    "name" => {
                        enriched_folders.sort_by_cached_key(|f| f.name.to_lowercase());
                    }
                    "name_desc" => {
                        enriched_folders.sort_by_cached_key(|f| Reverse(f.name.to_lowercase()));
                    }
                    "date" => {
                        enriched_folders.sort_by_key(|f| f.modified_at);
                    }
                    "date_desc" => {
                        enriched_folders.sort_by_key(|f| Reverse(f.modified_at));
                    }
                    _ => {
                        enriched_folders.sort_by_key(|f| Reverse(f.relevance_score));
                    }
                }

                // Blend in content-discovered files before the pagination math.
                let added = self
                    .merge_content_hits(content_hits, &mut enriched_files, &criteria, user_id)
                    .await?;
                let total_file_count = total_file_count + added;

                // ── Pagination (folders first, then files) ──
                let folder_count = enriched_folders.len();
                let total_count = total_file_count + folder_count;
                let start_idx = criteria.offset.min(total_count);
                let end_idx = (criteria.offset + criteria.limit).min(total_count);

                let folder_start = start_idx.min(folder_count);
                let folder_end = end_idx.min(folder_count);
                let paginated_folders = enriched_folders[folder_start..folder_end].to_vec();

                let file_start = start_idx.saturating_sub(folder_count);
                let file_end = end_idx
                    .saturating_sub(folder_count)
                    .min(enriched_files.len());
                let paginated_files = enriched_files[file_start..file_end].to_vec();

                let elapsed_ms = start.elapsed().as_millis() as u64;

                let search_results = Arc::new(SearchResultsDto::new(
                    paginated_files,
                    paginated_folders,
                    criteria.limit,
                    criteria.offset,
                    Some(total_count),
                    elapsed_ms,
                    criteria.sort_by.clone(),
                ));

                Ok(search_results)
            })
            .await
            .map_err(|shared: Arc<crate::common::errors::DomainError>| {
                crate::common::errors::DomainError::new(
                    shared.kind,
                    shared.entity_type,
                    shared.message.clone(),
                )
            })
    }

    /// Returns quick suggestions for autocomplete.
    async fn suggest(
        &self,
        query: &str,
        folder_id: Option<&str>,
        limit: usize,
    ) -> Result<SearchSuggestionsDto> {
        self.suggest(query, folder_id, limit).await
    }

    /// Clears the search results cache.
    async fn clear_search_cache(&self) -> Result<()> {
        self.search_cache.invalidate_all();
        self.search_cache.run_pending_tasks().await;
        Ok(())
    }
}

// ─── Stub for testing ────────────────────────────────────────────────────

impl SearchService {
    /// Creates a stub version of the service for testing
    pub fn new_stub() -> impl SearchUseCase {
        struct SearchServiceStub;

        impl SearchUseCase for SearchServiceStub {
            async fn search(
                &self,
                _criteria: SearchCriteriaDto,
                _user_id: Uuid,
            ) -> Result<Arc<SearchResultsDto>> {
                Ok(Arc::new(SearchResultsDto::empty()))
            }

            async fn suggest(
                &self,
                _query: &str,
                _folder_id: Option<&str>,
                _limit: usize,
            ) -> Result<SearchSuggestionsDto> {
                Ok(SearchSuggestionsDto {
                    suggestions: Vec::new(),
                    query_time_ms: 0,
                })
            }

            async fn clear_search_cache(&self) -> Result<()> {
                Ok(())
            }
        }

        SearchServiceStub
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_relevance_stays_below_name_contains_band() {
        // Best hit of the set caps at 45 — always under contains (50).
        assert_eq!(content_relevance(8.0, 8.0), 45);
        assert_eq!(content_relevance(4.0, 8.0), 28);
        // Degenerate inputs fall to the floor instead of panicking.
        assert_eq!(content_relevance(1.0, 0.0), 10);
        assert_eq!(content_relevance(f32::NAN, 8.0), 10);
        assert!(content_relevance(0.0, 8.0) >= 10);
    }

    fn dto(name: &str, relevance: u32, size: u64, modified_at: u64) -> SearchFileResultDto {
        SearchFileResultDto {
            id: name.to_string(),
            name: name.to_string(),
            path: format!("/{name}"),
            size,
            mime_type: "text/plain".to_string(),
            folder_id: None,
            created_at: 0,
            modified_at,
            relevance_score: relevance,
            size_formatted: String::new(),
            icon_class: String::new(),
            icon_special_class: String::new(),
            category: String::new(),
            blob_hash: String::new(),
            snippet: None,
            match_source: None,
        }
    }

    #[test]
    fn merged_files_resort_by_relevance_and_by_column() {
        let mut files = vec![
            dto("b-content.txt", 30, 10, 200),
            dto("a-name.txt", 80, 99, 100),
        ];
        sort_enriched_files(&mut files, "relevance");
        assert_eq!(
            files[0].name, "a-name.txt",
            "name match must outrank content match"
        );

        sort_enriched_files(&mut files, "size_desc");
        assert_eq!(files[0].name, "a-name.txt");
        sort_enriched_files(&mut files, "date");
        assert_eq!(files[0].name, "a-name.txt");
        sort_enriched_files(&mut files, "name_desc");
        assert_eq!(files[0].name, "b-content.txt");
    }
}
