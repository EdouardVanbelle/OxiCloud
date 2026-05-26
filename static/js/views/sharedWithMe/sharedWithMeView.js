/**
 * OxiCloud – "Shared with me" view.
 *
 * Renders files and folders that other users have explicitly granted the
 * current user access to, using the cursor-paginated
 * `GET /api/grants/incoming/resources` endpoint.
 *
 * Uses `ResourceListComponent` so the grid ↔ list toggle and all card
 * components work out of the box. A "Load more" button is injected below
 * the files container for cursor-based pagination.
 */

import { ui } from '../../app/ui.js';
import { i18n } from '../../core/i18n.js';
import { ResourceListComponent } from '../../components/resourceList.js';
import { batchToolbar } from '../../features/files/batchToolbar.js';
import { favorites } from '../../features/library/favorites.js';
import { ownerTooltip } from '../../features/ownerTooltip.js';
import { grants } from '../../model/grants.js';
import { systemUsers } from '../../model/systemUsers.js';

/** @import {SharedWithMeItem, FileItem, FolderItem, ResourceTypeEnum} from '../../core/types.js' */

/** ID of the "Load more" wrapper injected below `.files-container`. */
const LOAD_MORE_ID = 'swm-load-more-wrapper';

const sharedWithMeView = {
    // ── State ─────────────────────────────────────────────────────────────────

    /** @type {string|null} */
    _nextCursor: null,

    _loading: false,

    /** @type {ResourceListComponent|null} */
    _component: null,

    // ── Public API ────────────────────────────────────────────────────────────

    /**
     * (Re-)load from page 1 and render into the existing files container.
     * Called every time the user switches to this section.
     */
    async init() {
        this._nextCursor = null;
        this._loading = false;

        this._ensureLoadMoreButton();

        // Start fetching system users in background so tooltips resolve instantly
        // by the time the user hovers over an item.
        systemUsers.prefetch();

        // Standard files-view setup: clear list, show container
        ui.resetFilesList();
        batchToolbar.init();
        ui.updateBreadcrumb();

        // Create (or re-use) the component bound to #files-list.
        const filesList = document.getElementById('files-list');
        if (filesList) {
            if (!this._component) {
                this._component = new ResourceListComponent(
                    /** @type {HTMLElement} */ (filesList),
                    {
                        selectable: true,
                        showFavorite: true,
                        showOwner: true,
                        showShareBadge: false,
                        draggable: false,
                        showContextMenu: true,
                        isFavorite: (id, type) => favorites.isFavorite(id, type),
                        isShared: () => false,
                        onOpen: (item) => ui.openItem(item),
                        onFavoriteToggle: async (item) => {
                            const isFile = 'mime_type' in item;
                            const type = isFile ? 'file' : 'folder';
                            if (favorites.isFavorite(item.id, type)) {
                                await favorites.removeFromFavorites(item.id, type);
                                this._component?.setFavoriteVisualState(item.id, type, false);
                            } else {
                                await favorites.addToFavorites(item.id, item.name, type, null);
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
        }

        await this._loadPage();
    },

    /**
     * Hide the "Load more" button when leaving this section.
     * The files container itself is managed by navigation.js.
     */
    hide() {
        const w = document.getElementById(LOAD_MORE_ID);
        if (w) w.classList.add('hidden');

        batchToolbar.setActiveComponent(null);

        const filesList = document.getElementById('files-list');
        if (filesList) ownerTooltip.destroy(filesList);
    },

    // ── Internal helpers ──────────────────────────────────────────────────────

    /**
     * Fetch one page, map items → FileItem / FolderItem, render them, then
     * wire the owner tooltip.
     * @returns {Promise<void>}
     */
    async _loadPage() {
        if (this._loading) return;
        this._loading = true;

        // Remember whether this is a fresh first-page load (cursor was null on
        // entry) so we know whether to replace or append items.
        const isFirstPage = this._nextCursor === null;

        try {
            const data = await grants.fetchSharedWithMe({
                resourceTypes: /** @type {ResourceTypeEnum[]} */ (['file', 'folder']),
                limit: 50,
                cursor: this._nextCursor ?? undefined
            });

            this._nextCursor = data.next_cursor ?? null;

            if (data.items.length === 0 && isFirstPage) {
                // First page came back empty
                ui.showError(`
                    <i class="fas fa-share-alt empty-state-icon"></i>
                    <p>${i18n.t('sharedwithme_emptyStateTitle', 'Nothing shared with you yet')}</p>
                    <p>${i18n.t('sharedwithme_emptyStateDesc', 'Items shared with you by other users will appear here')}</p>
                `);
                this._setLoadMoreVisible(false);
                return;
            }

            const { folders, files } = this._mapItems(data.items);

            if (isFirstPage) {
                this._component?.render(folders, files);
            } else {
                this._component?.append(folders, files);
            }

            // Wire owner tooltips after items are in the DOM
            const filesList = document.getElementById('files-list');
            if (filesList) ownerTooltip.init(filesList);

            // Fill the Owner column cells (idempotent: skips already-resolved rows).
            await ui.resolveOwnerCells();

            this._setLoadMoreVisible(!!this._nextCursor);
        } catch (err) {
            ui.showError(`
                <i class="fas fa-exclamation-circle empty-state-icon error"></i>
                <p>${i18n.t('errors_loadFailed', 'Failed to load items')}</p>
            `);
            console.error('sharedWithMeView: load error', err);
        } finally {
            this._loading = false;
        }
    },

    /**
     * Map `SharedWithMeItem[]` to separate arrays for rendering.
     * Sets `owner_id` to `item.granted_by` so the component stamps
     * `data-owner-id` with the granter's user ID automatically.
     *
     * @param {SharedWithMeItem[]} items
     * @returns {{ folders: FolderItem[], files: FileItem[] }}
     */
    _mapItems(items) {
        /** @type {FolderItem[]} */
        const folders = [];

        /** @type {FileItem[]} */
        const files = [];

        for (const item of items) {
            if (item.resource_type === 'folder') {
                const f = /** @type {FolderItem} */ (item.resource);
                folders.push(
                    /** @type {FolderItem} */ ({
                        id: f.id,
                        name: f.name,
                        path: f.path ?? '',
                        parent_id: f.parent_id ?? '',
                        // Use granted_by as owner_id so the component populates
                        // data-owner-id with the sharing user's ID.
                        owner_id: item.granted_by,
                        is_root: f.is_root ?? false,
                        created_at: f.created_at,
                        modified_at: f.modified_at,
                        icon_class: f.icon_class,
                        icon_special_class: f.icon_special_class ?? '',
                        category: 'folder'
                    })
                );
            } else if (item.resource_type === 'file') {
                const f = /** @type {FileItem} */ (item.resource);
                files.push(
                    /** @type {FileItem} */ ({
                        id: f.id,
                        name: f.name,
                        path: f.path ?? '',
                        folder_id: f.folder_id ?? '',
                        // Use granted_by as owner_id so the component populates
                        // data-owner-id with the sharing user's ID.
                        owner_id: item.granted_by,
                        mime_type: f.mime_type,
                        size: f.size,
                        size_formatted: f.size_formatted,
                        created_at: f.created_at,
                        modified_at: f.modified_at,
                        sort_date: f.modified_at,
                        icon_class: f.icon_class,
                        icon_special_class: f.icon_special_class ?? '',
                        category: f.category
                    })
                );
            }
        }

        return { folders, files };
    },

    // ── "Load more" button ────────────────────────────────────────────────────

    /**
     * Create the "Load more" wrapper once and attach it below `.files-container`.
     * Subsequent calls are no-ops.
     */
    _ensureLoadMoreButton() {
        if (document.getElementById(LOAD_MORE_ID)) return;

        const filesContainer = document.querySelector('.files-container');
        if (!filesContainer) return;

        const wrapper = document.createElement('div');
        wrapper.id = LOAD_MORE_ID;
        wrapper.className = 'swm-load-more-wrapper hidden';

        const btn = document.createElement('button');
        btn.id = 'swm-load-more';
        btn.className = 'button secondary';
        btn.textContent = i18n.t('sharedwithme_loadMore', 'Load more');
        btn.addEventListener('click', () => this._loadPage());

        wrapper.appendChild(btn);
        filesContainer.after(wrapper);
    },

    /**
     * @param {boolean} visible
     */
    _setLoadMoreVisible(visible) {
        const w = document.getElementById(LOAD_MORE_ID);
        if (w) w.classList.toggle('hidden', !visible);
    }
};

export { sharedWithMeView };
