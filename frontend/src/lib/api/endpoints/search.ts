/** Search endpoint — ported from features/files/search.js. */
import { apiFetch, apiJson } from '$lib/api/client';
import type { SearchResults, SortBy } from '$lib/api/types';

export interface SearchOptions {
	folderId?: string;
	recursive?: boolean;
	fileTypes?: string[];
	minSize?: number;
	maxSize?: number;
	/** Unix-seconds lower bound on created time. */
	createdAfter?: number;
	/** Unix-seconds upper bound on created time. */
	createdBefore?: number;
	/** Unix-seconds lower bound on modified time. */
	modifiedAfter?: number;
	/** Unix-seconds upper bound on modified time. */
	modifiedBefore?: number;
	limit?: number;
	offset?: number;
	sortBy?: SortBy;
}

export function searchFiles(query: string, opts: SearchOptions = {}): Promise<SearchResults> {
	const params = new URLSearchParams();
	params.append('query', query);
	if (opts.folderId) params.append('folder_id', opts.folderId);
	if (opts.recursive !== undefined) params.append('recursive', String(opts.recursive));
	// The backend expects a single comma-separated `type` param (it splits on
	// ','); appending one param per type yields a "duplicate field" 400.
	if (opts.fileTypes?.length) params.append('type', opts.fileTypes.join(','));
	if (opts.minSize != null) params.append('min_size', String(opts.minSize));
	if (opts.maxSize != null) params.append('max_size', String(opts.maxSize));
	if (opts.createdAfter != null) params.append('created_after', String(opts.createdAfter));
	if (opts.createdBefore != null) params.append('created_before', String(opts.createdBefore));
	if (opts.modifiedAfter != null) params.append('modified_after', String(opts.modifiedAfter));
	if (opts.modifiedBefore != null) params.append('modified_before', String(opts.modifiedBefore));
	params.append('limit', String(opts.limit ?? 100));
	params.append('offset', String(opts.offset ?? 0));
	params.append('sort_by', opts.sortBy ?? 'relevance');
	return apiJson<SearchResults>(`/api/search?${params.toString()}`, { credentials: 'same-origin' });
}

/** A single autocomplete suggestion returned by the lightweight suggest endpoint. */
export interface SearchSuggestions {
	suggestions: string[];
	query_time_ms: number;
}

export interface SuggestOptions {
	folderId?: string;
	limit?: number;
}

/**
 * Lightweight autocomplete suggestions from the backend `GET /api/search/suggest`
 * endpoint — name-only hints without the full search overhead.
 */
export function searchSuggest(
	query: string,
	opts: SuggestOptions = {}
): Promise<SearchSuggestions> {
	const params = new URLSearchParams();
	params.append('query', query);
	if (opts.folderId) params.append('folder_id', opts.folderId);
	if (opts.limit != null) params.append('limit', String(opts.limit));
	return apiJson<SearchSuggestions>(`/api/search/suggest?${params.toString()}`, {
		credentials: 'same-origin'
	});
}

/**
 * Clear the shared server-side search cache
 * (`DELETE /api/admin/search/cache`). Admin-only — moved from
 * `/api/search/cache` on 2026-07-17 because the underlying
 * `invalidate_all()` touches every tenant (see AuthZ audit #14).
 */
export async function clearSearchCache(): Promise<void> {
	const res = await apiFetch('/api/admin/search/cache', {
		method: 'DELETE',
		credentials: 'same-origin'
	});
	if (!res.ok) throw new Error(`Failed to clear search cache: ${res.status} ${res.statusText}`);
}
