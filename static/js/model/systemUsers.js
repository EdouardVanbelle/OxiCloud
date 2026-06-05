// @ts-check

/**
 * System-users convenience layer.
 *
 * Thin wrapper over `addressBook.listContacts(SYSTEM_BOOK_ID)` that
 * provides a userId → display-name index, a userId → photo-url index,
 * and a userId → primary-email index.
 * Used wherever a grant's `granted_by` UUID needs to be shown as a
 * human-readable name (owner tooltips, share dialogs, etc.), avatar
 * image (userVignette, user menu), or email (suggestion dropdowns).
 *
 * Falls back gracefully when the system address book is disabled
 * server-side (`OXICLOUD_EXPOSE_SYSTEM_USERS` not set): `isAvailable()`
 * returns false and `getDisplayName()` returns a shortened UUID.
 */

/** @import {ContactItem, User} from '../core/types.js' */

import { addressBook, SYSTEM_BOOK_ID } from './addressBook.js';

/** @type {Map<string, string> | null} userId → display name, built lazily */
let _index = null;

/** @type {Map<string, string | null> | null} userId → photo URL (or null), built lazily */
let _photoIndex = null;

/** @type {Map<string, string | null> | null} userId → primary email (or null), built lazily */
let _emailIndex = null;

/** @type {Map<string, boolean> | null} userId → is_external flag, built lazily.
 *  The system-book bulk load populates `false` for every entry (PR 6 filters
 *  externals out of the system book). Externals appear only when their UUID
 *  shows up in a grant — `_resolveMissing` then back-fills via `/api/users/{id}`.
 */
let _externalIndex = null;

/** @type {Map<string, Promise<void>>} userId → in-flight fetch (de-dupe). */
const _inflight = new Map();

/**
 * Derive the best human-readable name from a contact.
 * Priority: "First Last" → full_name → primary email → shortened id.
 * @param {ContactItem} c
 * @returns {string}
 */
function _nameFor(c) {
    const parts = /** @type {string[]} */ ([c.first_name, c.last_name].filter(Boolean));
    if (parts.length) return parts.join(' ');
    if (c.full_name) return c.full_name;
    const mail = c.email?.find((e) => e.is_primary)?.email ?? c.email?.[0]?.email;
    if (mail) return mail;
    return `${c.id.slice(0, 8)}…`;
}

/**
 * Derive the best display name from a `User` shape (i.e. the
 * `/api/users/{id}` payload OR the `oxicloud_user` localStorage blob).
 * Priority — matches the server-side `User::display_full()` rule sans
 * the email decoration; the `<email>` part is added in the vignette
 * layer as a tooltip when the email isn't already in the displayed
 * label:
 *
 *   1. `"Given Family"` — both names set
 *   2. `username`       — handle (the typical case for password / OIDC
 *                         users with no profile claims)
 *   3. `email`          — last resort but unambiguous
 *   4. shortened UUID   — failure mode (e.g. /api/users/{id} returned
 *                         nothing usable)
 *
 * @param {{id?: string, given_name?: string|null, family_name?: string|null, username?: string|null, email?: string|null}} u
 * @returns {string}
 */
function _displayNameFromUser(u) {
    if (u.given_name && u.family_name) return `${u.given_name} ${u.family_name}`;
    if (u.username) return u.username;
    if (u.email) return u.email;
    return u.id ? `${u.id.slice(0, 8)}…` : '?';
}

/**
 * Ensure both indexes are built (idempotent).
 * After loading contacts from the system address book, the current user
 * (from localStorage) is injected so owner cells resolve correctly even
 * when the server-side address book does not include the logged-in user.
 * @returns {Promise<void>}
 */
async function _ensureIndex() {
    if (_index !== null) return;

    // System address book load. Tolerates ANY error (most relevantly the
    // 403 external users receive on `/api/address-books/system/contacts`
    // since PR 11.1's defense-in-depth lockout): treat as "empty book"
    // and fall through to the per-user localStorage injection below so
    // at least the logged-in user resolves correctly. Without this
    // try/catch an external user's systemUsers cache stays unbuilt and
    // every userVignette (including their own avatar in the user menu)
    // falls back to the UUID-prefix placeholder.
    /** @type {ContactItem[]} */
    let contacts = [];
    try {
        contacts = await addressBook.listContacts(SYSTEM_BOOK_ID);
    } catch {
        // 403 / 5xx / network error — leave contacts empty.
    }
    _index = new Map(contacts.map((c) => [c.id, _nameFor(c)]));
    _photoIndex = new Map(contacts.map((c) => [c.id, c.photo_url ?? null]));
    _emailIndex = new Map(
        contacts.map((c) => {
            const primary = c.email?.find((e) => e.is_primary)?.email ?? c.email?.[0]?.email ?? null;
            return [c.id, primary];
        })
    );
    // System book is internal-only post-PR-6 → every entry here is is_external=false.
    _externalIndex = new Map(contacts.map((c) => [c.id, false]));

    // Inject the current user if they are not already in the index
    try {
        const raw = localStorage.getItem('oxicloud_user');
        if (raw) {
            const u =
                /** @type {{id?:string, given_name?:string|null, family_name?:string|null, username?:string|null, email?:string|null, image?:string|null, is_external?:boolean}} */ (
                    JSON.parse(raw)
                );
            if (u?.id) {
                if (!_index.has(u.id)) {
                    _index.set(u.id, _displayNameFromUser(u));
                }
                if (!_photoIndex.has(u.id)) {
                    _photoIndex.set(u.id, u.image ?? null);
                }
                if (!_emailIndex.has(u.id)) {
                    _emailIndex.set(u.id, u.email ?? null);
                }
                if (!_externalIndex.has(u.id)) {
                    _externalIndex.set(u.id, u.is_external ?? false);
                }
            }
        }
    } catch {
        // localStorage not available or JSON is invalid — silently skip
    }
}

/**
 * Fetch a single user profile from `/api/users/{id}` and back-fill every
 * cache map. Used when a userId surfaces (e.g. via a grant) that wasn't
 * part of the bulk system-book load — typically external users.
 *
 * In-flight requests are de-duplicated through `_inflight` so concurrent
 * vignette renders for the same external userId issue only one HTTP call.
 * Failures (404 / 403 / 429 / network) leave the caches in their
 * default-unknown state; callers fall back to UUID-prefix display.
 *
 * @param {string} userId
 * @returns {Promise<void>}
 */
async function _resolveMissing(userId) {
    if (_index?.has(userId)) return;
    const pending = _inflight.get(userId);
    if (pending) return pending;

    const promise = (async () => {
        try {
            const resp = await fetch(`/api/users/${encodeURIComponent(userId)}`, {
                credentials: 'same-origin'
            });
            if (!resp.ok) return;
            /** @type {User} */
            const u = await resp.json();
            _index?.set(u.id, _displayNameFromUser(u));
            _photoIndex?.set(u.id, u.image ?? null);
            _emailIndex?.set(u.id, u.email ?? null);
            _externalIndex?.set(u.id, !!u.is_external);
        } catch {
            // network error — caches stay unset; getters fall back to defaults
        } finally {
            _inflight.delete(userId);
        }
    })();
    _inflight.set(userId, promise);
    return promise;
}

// ── Public API ────────────────────────────────────────────────────────────────

/**
 * Start loading the system address book in the background.
 * Safe to call multiple times — subsequent calls are no-ops once loaded.
 */
function prefetch() {
    if (!addressBook.isSystemAvailable()) return;
    _ensureIndex(); // intentionally fire-and-forget
}

/**
 * Synchronous best-effort display-name lookup from the pre-fetched cache.
 * Returns a shortened UUID prefix when the cache is not yet loaded.
 * Call `prefetch()` at view init time so the cache is warm by the time
 * items are rendered.
 * @param {string} userId
 * @returns {string}
 */
function getDisplayNameSync(userId) {
    if (_index === null) return `${userId.slice(0, 8)}…`;
    return _index.get(userId) ?? `${userId.slice(0, 8)}…`;
}

/**
 * Resolve a user UUID to a display name.
 * Awaits the first load if not yet cached; subsequent calls resolve
 * instantly. On a system-book miss (e.g. external users, which are
 * filtered out of the system address book), back-fills via
 * `/api/users/{id}` once per session.
 *
 * @param {string} userId
 * @returns {Promise<string>}
 */
async function getDisplayName(userId) {
    await _ensureIndex();
    if (!_index?.has(userId)) await _resolveMissing(userId);
    return _index?.get(userId) ?? `${userId.slice(0, 8)}…`;
}

/**
 * Resolve a user UUID to a photo URL (or null if none set).
 * Awaits the first load if not yet cached; subsequent calls resolve instantly.
 *
 * @param {string} userId
 * @returns {Promise<string | null>}
 */
async function getPhoto(userId) {
    await _ensureIndex();
    if (!_index?.has(userId)) await _resolveMissing(userId);
    return _photoIndex?.get(userId) ?? null;
}

/**
 * Resolve a user UUID to their primary email address (or null if unknown).
 * Awaits the first load if not yet cached; subsequent calls resolve instantly.
 *
 * @param {string} userId
 * @returns {Promise<string | null>}
 */
async function getEmail(userId) {
    await _ensureIndex();
    if (!_index?.has(userId)) await _resolveMissing(userId);
    return _emailIndex?.get(userId) ?? null;
}

/**
 * Resolve a user UUID to whether they are an external (grant-only)
 * recipient. Defaults to `false` (internal-by-assumption) for unknown
 * UUIDs so callers can render without an extra null-check.
 * Awaits the first system-book load; falls back to `/api/users/{id}`
 * on miss — externals are excluded from the system book per PR 6.
 *
 * @param {string} userId
 * @returns {Promise<boolean>}
 */
async function getIsExternal(userId) {
    await _ensureIndex();
    if (!_externalIndex?.has(userId)) await _resolveMissing(userId);
    return _externalIndex?.get(userId) ?? false;
}

/**
 * Force-refresh the current user's photo entry in the index from localStorage.
 * Call this after saving a new avatar on the profile page so that existing
 * vignettes can re-render without a full page reload.
 */
function refreshCurrentUserPhoto() {
    try {
        const raw = localStorage.getItem('oxicloud_user');
        if (!raw || !_photoIndex) return;
        const u = /** @type {{id?:string, image?:string|null}} */ (JSON.parse(raw));
        if (u?.id) {
            _photoIndex.set(u.id, u.image ?? null);
        }
    } catch {
        // ignore
    }
}

/**
 * Returns `false` only after a confirmed 404 from the server (feature
 * disabled).  Returns `true` when status is unknown or the book loaded OK.
 * @returns {boolean}
 */
function isAvailable() {
    return addressBook.isSystemAvailable();
}

export const systemUsers = {
    prefetch,
    getDisplayName,
    getDisplayNameSync,
    getPhoto,
    getEmail,
    getIsExternal,
    refreshCurrentUserPhoto,
    isAvailable
};
