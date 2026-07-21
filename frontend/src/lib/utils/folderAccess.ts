/**
 * Folder-access cache — memoises "can the caller read this folder?" so
 * UI decisions (e.g. showing / hiding the "Open parent folder" entry in
 * a context menu) don't fire an HTTP call at click-time.
 *
 * The backend answers the question via `GET /api/folders/{id}`:
 *   * 2xx → caller has Read on the folder (or it's their own).
 *   * 404 → anti-enumeration; treated as "no access" from the UI's
 *     perspective (the recipient can't navigate there whether the
 *     folder exists or not).
 *
 * The cache is a simple insertion-order-bumping LRU capped at
 * `MAX_ENTRIES`. `probeFolderAccess` is the async entry point; pages
 * kick a bulk `warmFolderAccess` when a list loads so the cache is
 * populated before the user right-clicks anything.
 */
import { getFolder } from '$lib/api/endpoints/folders';

const MAX_ENTRIES = 200;

// Cache: id → resolved answer. Presence means we know; `true`/`false`
// distinguishes the two outcomes. Insertion order preserved by Map;
// `bump` re-inserts on write so oldest sits at the front for eviction.
const cache = new Map<string, boolean>();

// In-flight dedup — if two callers ask about the same id before the
// first request settles, they share the same Promise. Cleared once the
// promise resolves.
const inflight = new Map<string, Promise<boolean>>();

function bump(id: string, value: boolean): void {
	cache.delete(id);
	cache.set(id, value);
	// Trim from the front (oldest insertion) until we're back under cap.
	while (cache.size > MAX_ENTRIES) {
		const oldest = cache.keys().next().value;
		if (oldest === undefined) break;
		cache.delete(oldest);
	}
}

/**
 * Sync lookup — `undefined` means "not yet probed"; callers gating UI
 * on this should call `warmFolderAccess` when items load so the
 * `true` / `false` answer is present by the time the user reaches for
 * the context menu.
 */
export function folderAccessCached(id: string): boolean | undefined {
	return cache.get(id);
}

/**
 * Async probe. Fires a `GET /api/folders/{id}` (deduplicated against
 * concurrent callers) and caches the boolean outcome. Never throws —
 * 404 and network failures both resolve to `false`.
 */
export async function probeFolderAccess(id: string): Promise<boolean> {
	const cached = cache.get(id);
	if (cached !== undefined) return cached;
	const running = inflight.get(id);
	if (running) return running;
	const p = (async () => {
		try {
			await getFolder(id);
			bump(id, true);
			return true;
		} catch {
			bump(id, false);
			return false;
		} finally {
			inflight.delete(id);
		}
	})();
	inflight.set(id, p);
	return p;
}
