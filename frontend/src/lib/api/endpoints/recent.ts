/** Recent endpoints — ported from recentModel.js. */
import { apiFetch } from '$lib/api/client';
import { getCsrfHeaders } from '$lib/api/csrf';
import {
	fetchResourcePage,
	type ResourceBody,
	type ResourcePage,
	type ResourcePageOpts
} from './resources';
import type { ItemType } from '$lib/api/types';

export interface RecentResourceItem {
	resource_type: ItemType;
	accessed_at: string;
	resource: ResourceBody;
}

export function fetchRecentPage(
	opts?: ResourcePageOpts
): Promise<ResourcePage<RecentResourceItem>> {
	return fetchResourcePage<RecentResourceItem>('/api/recent/resources', 'accessed_at', opts);
}

export async function clearRecent(): Promise<void> {
	const res = await apiFetch('/api/recent/clear', {
		method: 'POST',
		credentials: 'same-origin',
		headers: { 'Content-Type': 'application/json', ...getCsrfHeaders() },
		body: '{}'
	});
	if (!res.ok) throw new Error(`clear recent failed: ${res.status}`);
}

/**
 * Remove a single item from the caller's recent history — the "broom"
 * per-row affordance in the recent view. Distinct from `clearRecent`
 * (which wipes every entry). 404 means the item wasn't in recents to
 * begin with — treated as a no-op success by the caller.
 */
export async function removeFromRecent(kind: ItemType, id: string): Promise<void> {
	const res = await apiFetch(`/api/recent/${encodeURIComponent(kind)}/${encodeURIComponent(id)}`, {
		method: 'DELETE',
		credentials: 'same-origin',
		headers: getCsrfHeaders()
	});
	if (!res.ok && res.status !== 404) {
		throw new Error(`remove from recent failed: ${res.status}`);
	}
}
