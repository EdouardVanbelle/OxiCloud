/**
 * Typed API client with transparent 401 → token-refresh → retry.
 *
 * Ported from static/js/core/fetchWrapper.js. Unlike that wrapper, this does
 * NOT monkeypatch `window.fetch`; every endpoint module calls `apiFetch`
 * explicitly. The behavioural invariants are preserved exactly:
 *
 *  - A captured raw `fetch` is used for the real network calls so the refresh
 *    request and the retry never re-enter the interceptor (no recursion).
 *  - Concurrent 401s collapse into a single in-flight `/api/auth/refresh`.
 *  - Cross-origin responses are passed through untouched.
 *  - Auth primitives (login/logout/refresh/register/setup/oidc/device) and
 *    public-share endpoints (/api/s/) bypass the refresh-and-retry path:
 *    a 401 there is genuine ("bad credentials" / "password required"), not an
 *    expired access token.
 *  - When refresh fails, the session-expired handler fires (clear + redirect)
 *    and the call rejects.
 */

import { getCsrfHeaders } from './csrf';

const REFRESH_ENDPOINT = '/api/auth/refresh';

/** Auth primitives — a 401 here is genuine, never an expired access token. */
const AUTH_PRIMITIVES = [
	'/api/auth/login',
	'/api/auth/logout',
	'/api/auth/refresh',
	'/api/auth/register',
	'/api/auth/setup',
	'/api/auth/oidc/',
	'/api/auth/device/'
];

export type FetchFn = typeof fetch;

export interface ApiClientDeps {
	/** Underlying fetch used for the real network call (bypasses the interceptor). */
	rawFetch: FetchFn;
	/** Invoked once when a refresh definitively fails (clear session + redirect). */
	onSessionExpired: () => void;
	/** Test seam for `window.location.origin`. */
	origin?: string;
}

function urlString(input: RequestInfo | URL): string {
	if (typeof input === 'string') return input;
	if (input instanceof URL) return input.href;
	return input.url ?? '';
}

function isCrossOrigin(urlStr: string, origin: string): boolean {
	try {
		return new URL(urlStr, origin).origin !== origin;
	} catch {
		// Unparseable URL — treat as cross-origin so we pass it through untouched.
		return true;
	}
}

function bypassesRetry(urlStr: string): boolean {
	return AUTH_PRIMITIVES.some((p) => urlStr.includes(p)) || urlStr.includes('/api/s/');
}

/**
 * Build an isolated apiFetch with its own refresh-dedup state. Used directly in
 * tests; the app uses the default singleton below.
 */
export function createApiFetch(deps: ApiClientDeps): FetchFn {
	const { rawFetch, onSessionExpired } = deps;
	let refreshInFlight: Promise<boolean> | null = null;

	async function refresh(): Promise<boolean> {
		if (refreshInFlight) return refreshInFlight;
		refreshInFlight = (async () => {
			try {
				const r = await rawFetch(REFRESH_ENDPOINT, {
					method: 'POST',
					credentials: 'same-origin',
					headers: { 'Content-Type': 'application/json', ...getCsrfHeaders() },
					body: '{}'
				});
				return r.ok;
			} catch {
				return false;
			} finally {
				refreshInFlight = null;
			}
		})();
		return refreshInFlight;
	}

	const apiFetch: FetchFn = async (input, init) => {
		const origin = deps.origin ?? globalThis.location?.origin ?? 'http://localhost';
		const response = await rawFetch(input, init);
		if (response.status !== 401) return response;

		const urlStr = urlString(input as RequestInfo | URL);
		if (isCrossOrigin(urlStr, origin)) return response;
		if (bypassesRetry(urlStr)) return response;

		const refreshed = await refresh();
		if (!refreshed) {
			onSessionExpired();
			throw new Error('Session expired');
		}
		return rawFetch(input, init);
	};

	return apiFetch;
}

// ── Default singleton ──────────────────────────────────────────────────────

let sessionExpiredHandler: () => void = () => {
	if (typeof window !== 'undefined') {
		window.location.href = '/login?source=session_expired';
	}
};

/** Wire the real session-expired behaviour (clear store + redirect) at startup. */
export function setSessionExpiredHandler(fn: () => void): void {
	sessionExpiredHandler = fn;
}

const rawFetch: FetchFn =
	typeof globalThis.fetch === 'function' ? globalThis.fetch.bind(globalThis) : (undefined as never);

/** App-wide fetch — route every API call through this. */
export const apiFetch: FetchFn = createApiFetch({
	rawFetch,
	onSessionExpired: () => sessionExpiredHandler()
});

/** Convenience: fetch JSON, throwing on non-2xx. */
export async function apiJson<T>(input: RequestInfo | URL, init?: RequestInit): Promise<T> {
	const res = await apiFetch(input, init);
	if (!res.ok) {
		throw new ApiError(res.status, res.statusText, input);
	}
	return (await res.json()) as T;
}

export class ApiError extends Error {
	/**
	 * `error_type` field from the backend's `ErrorResponse` body, when
	 * present. Callers switch on this to render specific UX for
	 * distinguished failures (e.g. `EmailNotVerified` → "resend
	 * verification link" prompt). Falls back to `undefined` when the
	 * response body isn't parseable or the endpoint doesn't emit one.
	 */
	readonly errorType?: string;

	constructor(
		readonly status: number,
		readonly statusText: string,
		readonly resource: RequestInfo | URL,
		errorType?: string,
		serverMessage?: string
	) {
		super(
			serverMessage ?? `API ${status} ${statusText} for ${urlString(resource as RequestInfo | URL)}`
		);
		this.name = 'ApiError';
		this.errorType = errorType;
	}
}
