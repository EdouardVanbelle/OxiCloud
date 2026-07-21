/**
 * Local-storage prefs — key convention + user-scoped cleanup.
 *
 * # Key convention
 *
 * Every persistent client-side preference lives under the `oxi-` prefix.
 * Historical mix of `oxicloud_*`, `oxicloud-*`, and `oxi-*` normalised to
 * one form so `wipeAppKeys()` below can sweep the whole set with a single
 * `startsWith('oxi-')` predicate.
 *
 * # Nuke-on-mismatch at login
 *
 * `ensureActiveUser(userId)` compares the newly-authenticated user id
 * against the stored `oxi-active-user-id`. If they differ, EVERY `oxi-*`
 * key is removed (except the active-user marker itself). This runs on:
 *   * first login after page load,
 *   * "switch account" flows where the current tab silently changes user,
 *   * session expiry then re-login as someone else.
 *
 * The wipe is intentionally broad — one naming convention beats maintaining
 * a per-key whitelist that decays as new preferences get added.
 *
 * # Not stored here
 *
 * Auth tokens and CSRF cookies do NOT use `oxi-*` keys — they live in
 * HTTP-only cookies set by the backend and are outside localStorage.
 * Nothing to wipe there.
 */

/** Marker key: which user's prefs currently live in localStorage. */
const ACTIVE_USER_KEY = 'oxi-active-user-id';

/** Every persistent client pref key must start with this. */
const OXI_PREFIX = 'oxi-';

/**
 * Remove every `oxi-*` key from localStorage EXCEPT the active-user marker.
 * Idempotent; no-op when localStorage is unavailable (SSR, private mode).
 */
export function wipeAppKeys(): void {
	if (typeof localStorage === 'undefined') return;
	// Materialise the key list first — mutating localStorage while iterating
	// its live view skips half the entries.
	const keys = Object.keys(localStorage);
	for (const key of keys) {
		if (key === ACTIVE_USER_KEY) continue;
		if (key.startsWith(OXI_PREFIX)) {
			try {
				localStorage.removeItem(key);
			} catch {
				/* private mode / quota — best-effort cleanup */
			}
		}
	}
}

/**
 * Ensure any localStorage state belongs to `userId`. When the marker
 * doesn't match — first login of the page, re-login as someone else,
 * or a fossil from a previous release with no marker — the whole app
 * key namespace is nuked and the marker is set to the current user.
 *
 * Call once, right after the session store observes an authenticated
 * user. Cheap when no work is needed (single `getItem`).
 */
export function ensureActiveUser(userId: string): void {
	if (typeof localStorage === 'undefined') return;
	let stored: string | null = null;
	try {
		stored = localStorage.getItem(ACTIVE_USER_KEY);
	} catch {
		/* private mode — treat as "no marker" so we do the cleanup pass */
	}
	if (stored === userId) return;
	wipeAppKeys();
	try {
		localStorage.setItem(ACTIVE_USER_KEY, userId);
	} catch {
		/* private mode — cleanup still ran, marker will re-attempt next login */
	}
}
