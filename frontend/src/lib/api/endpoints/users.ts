/**
 * Per-user profile resolution via `GET /api/users/{id}`, cached per id.
 *
 * Used to render external (and any non-directory) users in share/recipient UIs
 * with their real name, email, avatar and an internal/external flag — the
 * system address book only lists internal users, so external grant subjects
 * would otherwise show as a bare UUID. Mirrors the original `systemUsers`
 * resolver. The endpoint enforces its own visibility rules; a non-visible
 * profile resolves to `null` so callers fall back to whatever label they have.
 */
import { apiFetch } from '$lib/api/client';

export interface ResolvedUser {
	id: string;
	name: string;
	email: string;
	image: string | null;
	isExternal: boolean;
}

/** Subset of the backend `UserDto` we consume here. */
interface UserDtoShape {
	id: string;
	username?: string | null;
	email?: string | null;
	image?: string | null;
	is_external: boolean;
}

// id → in-flight/resolved lookup (the Promise is cached so concurrent callers
// for the same id share one request, and a `null` result isn't re-fetched).
const cache = new Map<string, Promise<ResolvedUser | null>>();

export function resolveUser(id: string): Promise<ResolvedUser | null> {
	const hit = cache.get(id);
	if (hit) return hit;

	const pending = (async (): Promise<ResolvedUser | null> => {
		try {
			const res = await apiFetch(`/api/users/${encodeURIComponent(id)}`, {
				credentials: 'same-origin'
			});
			if (!res.ok) return null;
			const u = (await res.json()) as UserDtoShape;
			return {
				id: u.id,
				name: u.username?.trim() || u.email || u.id,
				email: u.email ?? '',
				image: u.image ?? null,
				isExternal: u.is_external
			};
		} catch {
			return null;
		}
	})();

	cache.set(id, pending);
	return pending;
}
