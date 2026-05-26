/**
 * OxiCloud - Favorites Module (server-authoritative)
 *
 * Source of truth: GET /api/favorites (enriched with name/size/mime via SQL JOIN).
 * Local in-memory cache (`_cache`) keeps `isFavorite()` synchronous for the
 * rendering path so star icons can be painted without a round-trip.
 */

import { ui } from '../../app/ui.js';
import { getCsrfHeaders } from '../../core/csrf.js';
import { i18n } from '../../core/i18n.js';
import { ResourceListComponent } from '../../components/resourceList.js';
import { batchToolbar } from '../files/batchToolbar.js';
import * as pathTooltip from '../pathTooltip.js';

/** @import {FavoriteItem, FileItem, FolderItem} from '../../core/types.js' */

const favorites = {
    /** @type {Map<string, FavoriteItem>} key = "file:<id>" | "folder:<id>" */
    _cache: new Map(),

    /** Whether the initial fetch from the server has completed */
    _ready: false,

    /** @type {ResourceListComponent|null} */
    _component: null,

    // ───────────────────── helpers ─────────────────────

    _authHeaders() {
        return { ...getCsrfHeaders() };
    },

    /**
     * @param {string} id
     * @param {string} type
     */
    _cacheKey(id, type) {
        return `${type}:${id}`;
    },

    /**
     * Replace the entire in-memory cache from an array of FavoriteItemDto
     * objects (as returned by the batch endpoint). Avoids an extra
     * GET /api/favorites round-trip.
     * @param {any[]} items
     */
    _replaceCacheFromResponse(items) {
        this._cache.clear();
        for (const item of items) {
            this._cache.set(this._cacheKey(item.item_id, item.item_type), item);
        }
        this._ready = true;
        console.log(`Favorites cache replaced from response: ${this._cache.size} items`);
    },

    // ───────────────────── lifecycle ─────────────────────

    /**
     * Initialise the module: fetch the full list from the server and populate
     * the in-memory cache.  Called once from app.js on startup.
     */
    async init() {
        console.log('Initializing favorites module (server-authoritative)');
        await this._fetchFromServer();
    },

    /**
     * Fetch favourites from the backend and rebuild the cache.
     */
    async _fetchFromServer() {
        try {
            const response = await fetch('/api/favorites', {
                headers: this._authHeaders()
            });

            if (!response.ok) {
                console.warn(`Favorites API returned ${response.status}`);
                return;
            }

            /** @type {FavoriteItem[]} */
            const items = await response.json();
            this._cache.clear();
            for (const item of items) {
                this._cache.set(this._cacheKey(item.item_id, item.item_type), item);
            }

            this._ready = true;
            console.log(`Favorites cache loaded: ${this._cache.size} items`);
        } catch (err) {
            console.error('Error fetching favorites:', err);
        }
    },

    // ───────────────────── public API ─────────────────────

    /**
     * Synchronous check used by ui.js to paint star icons.
     * @param {string} id
     * @param {string} type
     */
    isFavorite(id, type) {
        return this._cache.has(this._cacheKey(id, type));
    },

    /**
     * Add an item to favourites (server-first).
     * @param {string} id
     * @param {string} name
     * @param {string} type
     * @param {string | null} _parentId
     */
    async addToFavorites(id, name, type, _parentId) {
        try {
            const response = await fetch(`/api/favorites/${type}/${id}`, {
                method: 'POST',
                headers: this._authHeaders()
            });

            if (!response.ok) {
                throw new Error(`Server returned ${response.status}`);
            }

            // Refresh cache from server to get enriched data
            await this._fetchFromServer();

            // Notify user
            if (ui?.showNotification) {
                ui.showNotification(i18n.t('favorites.added_title'), `"${name}" ${i18n.t('favorites.added_msg')}`);
            }

            return true;
        } catch (error) {
            console.error('Error adding to favorites:', error);
            return false;
        }
    },

    /**
     * Remove an item from favourites (server-first).
     * @param {string} id
     * @param {string} type
     */
    async removeFromFavorites(id, type) {
        try {
            // Remember name for notification before removing from cache
            const cached = this._cache.get(this._cacheKey(id, type));
            const itemName = cached?.item_name || id;

            const response = await fetch(`/api/favorites/${type}/${id}`, {
                method: 'DELETE',
                headers: this._authHeaders()
            });

            if (!response.ok) {
                throw new Error(`Server returned ${response.status}`);
            }

            // Remove from local cache
            this._cache.delete(this._cacheKey(id, type));

            if (ui?.showNotification) {
                ui.showNotification(i18n.t('favorites.removed_title'), `"${itemName}" ${i18n.t('favorites.removed_msg')}`);
            }

            return true;
        } catch (error) {
            console.error('Error removing from favorites:', error);
            return false;
        }
    },

    // ───────────────────── display ─────────────────────

    /**
     * Render the favourites view.  All data comes from the in-memory cache
     * (which was populated from the enriched backend response — zero extra
     * fetches).
     */
    async displayFavorites() {
        try {
            await this._fetchFromServer();

            ui.resetFilesList();
            batchToolbar.init();
            ui.updateBreadcrumb();

            if (this._cache.size === 0) {
                ui.showError(`
                    <i class="fas fa-star empty-state-icon"></i>
                    <p>${i18n.t('favorites.empty_state')}</p>
                    <p>${i18n.t('favorites.empty_hint')}</p>
                `);
                return;
            }

            /** @type {FolderItem[]} */
            const folders = [];

            /** @type {FileItem[]} */
            const files = [];

            for (const item of this._cache.values()) {
                // owner_id comes from the backend JOIN (actual file/folder owner)
                if (item.item_type === 'folder') {
                    folders.push(
                        /** @type {FolderItem} */ ({
                            id: item.item_id,
                            name: item.item_name || item.item_id,
                            parent_id: item.parent_id || '',
                            modified_at: item.modified_at || item.created_at,
                            path: item.item_path || '',
                            category: 'folder',
                            created_at: item.created_at,
                            icon_class: item.icon_class,
                            icon_special_class: item.icon_special_class,
                            owner_id: item.owner_id ?? '',
                            is_root: false
                        })
                    );
                } else {
                    files.push(
                        /** @type {FileItem} */ ({
                            id: item.item_id,
                            name: item.item_name || item.item_id,
                            folder_id: item.parent_id || '',
                            mime_type: item.item_mime_type,
                            icon_class: item.icon_class,
                            icon_special_class: item.icon_special_class,
                            category: item.category,
                            size: item.item_size || 0,
                            size_formatted: item.size_formatted,
                            modified_at: item.modified_at || item.created_at,
                            path: item.item_path || '',
                            owner_id: item.owner_id ?? '',
                            created_at: item.created_at,
                            sort_date: item.created_at
                        })
                    );
                }
            }

            const filesList = document.getElementById('files-list');
            if (filesList) {
                if (!this._component) {
                    this._component = new ResourceListComponent(
                        /** @type {HTMLElement} */ (filesList),
                        {
                            selectable: true,
                            showFavorite: true,
                            showOwner: true,
                            showShareBadge: true,
                            draggable: false,
                            showContextMenu: true,
                            itemModifierClass: 'favorite-item',
                            isFavorite: (id, type) => this.isFavorite(id, type),
                            onOpen: (item) => ui.openItem(item),
                            onFavoriteToggle: async (item) => {
                                const isFile = 'mime_type' in item;
                                const type = isFile ? 'file' : 'folder';
                                if (this.isFavorite(item.id, type)) {
                                    await this.removeFromFavorites(item.id, type);
                                    this._component?.setFavoriteVisualState(item.id, type, false);
                                } else {
                                    await this.addToFavorites(item.id, item.name, type, null);
                                    this._component?.setFavoriteVisualState(item.id, type, true);
                                }
                            },
                            onContextMenu: (item, e) => ui.showContextMenuForItem(item, e),
                            onSelectionChange: (selectedItems) => {
                                batchToolbar._selected.clear();
                                for (const sel of selectedItems) {
                                    const isFile = 'mime_type' in sel;
                                    batchToolbar._selected.set(sel.id, {
                                        id: sel.id,
                                        name: sel.name,
                                        type: isFile ? 'file' : 'folder',
                                        parentId: isFile
                                            ? (/** @type {FileItem} */ (sel)).folder_id || ''
                                            : (/** @type {FolderItem} */ (sel)).parent_id || ''
                                    });
                                }
                                batchToolbar._syncUI();
                            }
                        }
                    );
                }
                batchToolbar.setActiveComponent(this._component);
                this._component.render(folders, files);
                pathTooltip.init(filesList);
            }

            await ui.resolveOwnerCells();
        } catch (error) {
            console.error('Error displaying favorites:', error);
            if (ui?.showNotification) {
                ui.showNotification('Error', 'Error loading favorite items');
            }
        }
    }
};

export { favorites };
