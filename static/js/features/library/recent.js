/**
 * OxiCloud - Recent Files Module (server-authoritative)
 *
 * Source of truth: GET /api/recent (enriched with name/size/mime via SQL JOIN).
 * File-access events are forwarded to the backend with POST /api/recent/{type}/{id}.
 * No localStorage usage — the server persists and prunes recent items.
 */

import { ui } from '../../app/ui.js';
import { getCsrfHeaders } from '../../core/csrf.js';
import { i18n } from '../../core/i18n.js';
import { ResourceListComponent } from '../../components/resourceList.js';
import { batchToolbar } from '../files/batchToolbar.js';
import * as pathTooltip from '../pathTooltip.js';

/** @import {FileItem, FolderItem, ItemTypeEnum, RecentItem} from '../../core/types.js' */

const recent = {
    /** Maximum items to request from the server */
    MAX_RECENT_FILES: 20,

    /** @type {ResourceListComponent|null} */
    _component: null,

    // ───────────────────── helpers ─────────────────────

    _authHeaders() {
        return { ...getCsrfHeaders() };
    },

    // ───────────────────── lifecycle ─────────────────────

    /**
     * Initialise the module.  Called once from app.js on startup.
     */
    init() {
        console.log('Initializing recent files module (server-authoritative)');
        this.setupEventListeners();
    },

    /**
     * Listen for file-accessed events dispatched by ui.js and forward
     * them to the backend.
     */
    setupEventListeners() {
        document.addEventListener('file-accessed', (event) => {
            const e = /** @type {CustomEvent} */ (event);
            if (e.detail?.file) {
                const file = e.detail.file;
                const itemType = file.item_type || 'file';
                this._recordAccess(file.id, itemType);
            }
        });
    },

    /**
     * Record an access event on the server.
     * @param {string} itemId
     * @param {ItemTypeEnum} itemType
     */
    async _recordAccess(itemId, itemType) {
        try {
            await fetch(`/api/recent/${itemType}/${itemId}`, {
                method: 'POST',
                headers: this._authHeaders()
            });
        } catch (err) {
            console.warn('Failed to record recent access:', err);
        }
    },

    // ───────────────────── public API ─────────────────────

    /**
     * Clear all recent items (delegates to the server).
     */
    async clearRecentFiles() {
        try {
            await fetch('/api/recent/clear', {
                method: 'DELETE',
                headers: this._authHeaders()
            });
        } catch (err) {
            console.error('Error clearing recent files:', err);
        }
    },

    /**
     * Fetch and display recent files.  Data comes directly from the
     * enriched backend response — zero extra per-item fetches.
     */
    async displayRecentFiles() {
        try {
            const response = await fetch(`/api/recent?limit=${this.MAX_RECENT_FILES}`, {
                headers: this._authHeaders()
            });

            if (!response.ok) {
                throw new Error(`Server returned ${response.status}`);
            }

            const recentItems = /** @type {RecentItem[]} */ (await response.json());

            // resetFilesList injects the standard list-header with the
            // Modified column label; we swap the last header cell to "Accessed".
            ui.resetFilesList();

            const filesList = document.getElementById('files-list');
            if (filesList) {
                // Relabel the date column header from "Modified" → "Accessed"
                const dateHeader = /** @type {HTMLElement|null} */ (
                    [...filesList.querySelectorAll('.list-header > div')].find(
                        (el) => el.getAttribute('data-i18n') === 'files.modified'
                    )
                );
                if (dateHeader) {
                    dateHeader.removeAttribute('data-i18n');
                    dateHeader.setAttribute('data-i18n', 'recent.accessed');
                    dateHeader.textContent = i18n.t('recent.accessed', 'Accessed');
                }
            }

            batchToolbar.clear();
            batchToolbar.init();
            ui.updateBreadcrumb();

            if (recentItems.length === 0) {
                ui.showError(`
                    <i class="fas fa-clock empty-state-icon"></i>
                    <p>${i18n.t('recent.empty_state')}</p>
                    <p>${i18n.t('recent.empty_hint')}</p>
                `);
                return;
            }

            /** @type {FolderItem[]} */
            const folders = [];

            /** @type {FileItem[]} */
            const files = [];

            for (const item of recentItems) {
                const isFolder = item.item_type === 'folder';
                if (isFolder) {
                    folders.push({
                        id: item.item_id,
                        name: item.item_name || item.item_id,
                        parent_id: item.parent_id || '',
                        modified_at: item.accessed_at,
                        path: item.item_path || '',
                        category: 'folder',
                        created_at: item.accessed_at, // Wrong information — server only stores accessed_at
                        icon_class: item.icon_class,
                        icon_special_class: item.icon_special_class,
                        owner_id: '',
                        is_root: false
                    });
                } else {
                    if (item.item_mime_type === undefined || item.item_mime_type === null) {
                        // FIXME: this case should not be possible, is it an information badly cleaned up on server ?
                        console.warn('Broken information for RecentItem: ', item);
                    }
                    files.push({
                        id: item.item_id,
                        name: item.item_name || item.item_id,
                        folder_id: item.parent_id || '',
                        mime_type: item.item_mime_type,
                        icon_class: item.icon_class,
                        icon_special_class: item.icon_special_class,
                        category: item.category,
                        size: item.item_size || 0,
                        size_formatted: item.size_formatted,
                        modified_at: item.accessed_at,
                        path: item.item_path || '',
                        owner_id: '',
                        created_at: item.accessed_at, // Wrong information — server only stores accessed_at
                        sort_date: item.accessed_at
                    });
                }
            }

            if (filesList) {
                if (!this._component) {
                    this._component = new ResourceListComponent(
                        /** @type {HTMLElement} */ (filesList),
                        {
                            selectable: true,
                            showFavorite: true,
                            showOwner: false,
                            showShareBadge: false,
                            draggable: false,
                            showContextMenu: true,
                            itemModifierClass: 'recent-item',
                            dateField: 'modified_at', // mapped from accessed_at above
                            onOpen: (item) => ui.openItem(item),
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
        } catch (error) {
            console.error('Error displaying recent files:', error);
            if (ui?.showNotification) {
                ui.showNotification('Error', 'Error loading recent files');
            }
        }
    }
};

export { recent };
