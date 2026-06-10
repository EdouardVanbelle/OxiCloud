// @ts-check

/**
 * OxiCloud – Files data model.
 *
 * Pure data layer: all API calls for file/folder listing and breadcrumb
 * resolution, with zero DOM dependency. Views import these functions and
 * call them without knowing the fetch details.
 */

import { app } from '../app/state.js';
import { uiNotifications } from '../app/uiNotifications.js';

/** @import {FileItem, FolderItem} from '../core/types.js' */

/** @type {RequestInit} */
const NO_CACHE = {
    headers: { 'Cache-Control': 'no-cache, no-store, must-revalidate', Pragma: 'no-cache' },
    credentials: 'same-origin',
    cache: 'no-store'
};

/**
 * Fetch metadata for a single folder.
 * Rejects with `null` when the server returns a non-OK response.
 * @param {string} id
 * @returns {Promise<FolderItem>}
 */
async function getFolder(id) {
    const response = await fetch(`/api/folders/${id}`, NO_CACHE);
    if (response.ok) return response.json();
    console.warn(`Error fetching folder ${id}`);
    return Promise.reject(null);
}

/**
 * Session cache of folder metadata for breadcrumb resolution (`id →
 * FolderItem`). Ancestors of the current folder have almost always been
 * visited already, so a warm navigation rebuilds the whole crumb trail
 * with zero fetches instead of one round-trip per depth level.
 * Invalidated on rename/move via {@link invalidateFolderMeta}.
 * @type {Map<string, FolderItem>}
 */
const _folderMetaCache = new Map();

/**
 * Resolve breadcrumb metadata for one folder, consulting the session
 * cache first.
 * @param {string} id
 * @returns {Promise<FolderItem>}
 */
async function _getFolderMeta(id) {
    const cached = _folderMetaCache.get(id);
    if (cached) return cached;
    const info = await getFolder(id);
    _folderMetaCache.set(id, info);
    return info;
}

/**
 * Drop the cached breadcrumb metadata for a folder. Call after any
 * operation that changes its name or parent (rename, move) so the next
 * breadcrumb rebuild re-fetches the fresh row.
 * @param {string} folderId
 */
function invalidateFolderMeta(folderId) {
    _folderMetaCache.delete(folderId);
}

/**
 * Monotonic token identifying the most recent {@link rebuildBreadCrumb}
 * call. Rebuilds now run concurrently with the listing fetch, so a rapid
 * second navigation can supersede one still in flight — the superseded
 * run must not commit its (stale) trail over the newer one.
 */
let _breadcrumbGeneration = 0;

/**
 * Walk up the folder hierarchy to rebuild `app.breadcrumbPath`.
 *
 * Stops gracefully at a permission boundary (shared subtrees) — the partial
 * breadcrumb built so far becomes the visual root, matching how Google Drive
 * handles shared folders the user cannot traverse beyond.
 *
 * An error on the target folder itself is treated as a real error and falls
 * back to the home folder (resetting `app.currentPath`).
 *
 * The trail is built locally and committed to `app` atomically at the end,
 * and only when this call is still the most recent one — callers run this
 * concurrently with the listing fetch.
 *
 * @returns {Promise<boolean>} `true` when the trail was committed; `false`
 *   when this rebuild was superseded by a newer navigation.
 */
async function rebuildBreadCrumb() {
    const generation = ++_breadcrumbGeneration;

    /** @type {FolderItem|null} */
    let currentFolderInfo = null;
    /** @type {Array<{id: string, name: string}>} */
    const crumbs = [];

    /** @type {string|null} */
    let id = app.currentPath;

    while (id !== null) {
        try {
            const folderInfo = await _getFolderMeta(id);
            if (generation !== _breadcrumbGeneration) return false;
            if (currentFolderInfo === null) currentFolderInfo = folderInfo;
            crumbs.unshift({ id: folderInfo.id, name: folderInfo.name });
            id = folderInfo.parent_id;
        } catch (_e) {
            if (generation !== _breadcrumbGeneration) return false;
            if (currentFolderInfo === null) {
                console.warn(`Cannot access target folder ${app.currentPath}, falling back to home`);
                uiNotifications.show('error: folder not found or permission denied', 'the given folder is not available or you do not have sufficient rights');
                crumbs.length = 0;
                id = app.userHomeFolderId;
                if (id) app.currentPath = id;
            } else {
                console.log(`Stopped breadcrumb traversal at permission boundary (parent of ${currentFolderInfo.id} is not accessible)`);
                break;
            }
        }
    }

    if (generation !== _breadcrumbGeneration) return false;
    app.breadcrumbPath = crumbs;
    app.currentFolderInfo = currentFolderInfo;
    return true;
}

/**
 * Fetch the folder listing for the given folder id.
 *
 * @param {string} folderId
 * @param {{ forceRefresh?: boolean }} [options]
 * @returns {Promise<{ folders: FolderItem[], files: FileItem[] }>}
 */
async function fetchListing(folderId, options = {}) {
    const timestamp = Math.floor(Date.now() / 1000);
    let url = `/api/folders/${folderId}/listing?t=${timestamp}`;

    /** @type {HeadersInit} */
    const headers = { .../** @type {Record<string,string>} */ (NO_CACHE.headers) };

    if (options.forceRefresh) {
        url += '&force_refresh=true';
        headers['X-Force-Refresh'] = 'true';
    }

    const response = await fetch(url, { ...NO_CACHE, headers });

    if (response.status === 403) throw Object.assign(new Error('Forbidden'), { status: 403 });
    if (!response.ok) throw new Error(`Server responded with status: ${response.status}`);

    const listing = await response.json();
    return {
        folders: Array.isArray(listing.folders) ? listing.folders : [],
        files: Array.isArray(listing.files) ? listing.files : []
    };
}

/**
 * Map one tagged resource item from `/api/folders/{id}/resources` into the
 * canonical `FileItem` / `FolderItem` shape used by `ResourceListComponent`.
 *
 * @param {{ resource_type: string, resource: Record<string, unknown> }} tagged
 * @returns {FileItem|FolderItem}
 */
function _mapResourceItem(tagged) {
    const r = tagged.resource;
    if (tagged.resource_type === 'folder') {
        return /** @type {FolderItem} */ ({
            id: String(r.id ?? ''),
            name: String(r.name ?? ''),
            path: String(r.path ?? ''),
            parent_id: r.parent_id != null ? String(r.parent_id) : '',
            owner_id: r.owner_id != null ? String(r.owner_id) : '',
            created_at: /** @type {number} */ (r.created_at),
            modified_at: /** @type {number} */ (r.modified_at),
            is_root: Boolean(r.is_root),
            icon_class: String(r.icon_class ?? 'fas fa-folder'),
            icon_special_class: String(r.icon_special_class ?? 'folder-icon'),
            category: String(r.category ?? 'Folder')
        });
    }
    return /** @type {FileItem} */ ({
        id: String(r.id ?? ''),
        name: String(r.name ?? ''),
        path: String(r.path ?? ''),
        folder_id: r.folder_id != null ? String(r.folder_id) : '',
        owner_id: r.owner_id != null ? String(r.owner_id) : '',
        mime_type: String(r.mime_type ?? ''),
        size: /** @type {number} */ (r.size),
        size_formatted: String(r.size_formatted ?? ''),
        created_at: /** @type {number} */ (r.created_at),
        modified_at: /** @type {number} */ (r.modified_at),
        icon_class: String(r.icon_class ?? ''),
        icon_special_class: String(r.icon_special_class ?? ''),
        category: String(r.category ?? '')
    });
}

/**
 * Fetch one cursor page from `GET /api/folders/{id}/resources`.
 *
 * @param {string} folderId
 * @param {{ cursor?: string|null, orderBy?: string, limit?: number, reverse?: boolean }} [opts]
 * @returns {Promise<{ items: Array<FileItem|FolderItem>, nextCursor: string|null }>}
 */
async function fetchResourcesPage(folderId, { cursor = null, orderBy = 'name', limit = 50, reverse = false } = {}) {
    const params = new URLSearchParams({ order_by: orderBy, limit: String(limit) });
    if (cursor) params.set('cursor', cursor);
    if (reverse) params.set('reverse', 'true');

    const res = await fetch(`/api/folders/${folderId}/resources?${params}`, NO_CACHE);
    if (!res.ok) {
        const err = /** @type {any} */ (new Error(`fetchResourcesPage: ${res.status}`));
        err.status = res.status;
        throw err;
    }

    const data = await res.json();
    const items = /** @type {Array<{ resource_type: string, resource: Record<string, unknown> }>} */ (Array.isArray(data.items) ? data.items : []).map(
        _mapResourceItem
    );

    return { items, nextCursor: data.next_cursor ?? null };
}

export { fetchListing, fetchResourcesPage, getFolder, invalidateFolderMeta, rebuildBreadCrumb };
