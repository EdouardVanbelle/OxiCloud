use std::sync::Arc;

use uuid::Uuid;

use crate::application::dtos::search_dto::{
    SearchCriteriaDto, SearchResultsDto, SearchSuggestionsDto,
};
use crate::common::errors::DomainError;

/**
 * Primary port for file and folder search.
 *
 * All search processing (filtering, scoring, sorting, categorization)
 * is handled server-side in Rust for maximum efficiency.
 */
pub trait SearchUseCase: Send + Sync + 'static {
    /// Performs a full search based on the specified criteria.
    ///
    /// Returns `Arc<SearchResultsDto>` so the cache and the caller share
    /// the same allocation — zero-copy on both insert and hit.
    /// `user_id` identifies the authenticated user so that SQL queries filter
    /// by owner and the result cache is isolated per tenant.
    async fn search(
        &self,
        criteria: SearchCriteriaDto,
        user_id: Uuid,
    ) -> Result<Arc<SearchResultsDto>, DomainError>;

    /// Returns quick suggestions for autocomplete (lightweight, fast).
    /// `caller_id` scopes results to drives the caller can Read — without
    /// it the endpoint leaks names + paths across every tenant on the
    /// instance (AuthZ audit finding #1, 2026-07-12).
    async fn suggest(
        &self,
        query: &str,
        folder_id: Option<&str>,
        limit: usize,
        caller_id: Uuid,
    ) -> Result<SearchSuggestionsDto, DomainError>;

    /// Clears the search results cache.
    async fn clear_search_cache(&self) -> Result<(), DomainError>;
}
