/**
 * ResourceListComponent — generic grid / list renderer for files and folders.
 *
 * Each view that shows a list of resources (SharedWithMe, Favorites, Recent,
 * and the main file manager) creates its own component instance with a config
 * that enables only the features the view needs.
 *
 * The component is responsible for:
 *   - Creating .file-item DOM nodes (folders first, then files)
 *   - Injecting optional swimlane dividers via a `groupFn`
 *   - Scoped event delegation (one listener per instance, never global)
 *   - Reporting events back to the view through callbacks
 *
 * The component does NOT own: context menus, multi-select toolbar, navigation
 * state, or thumbnail generation queues — those remain in the calling module
 * and are reached through the config callbacks.
 */

// @ts-check

import { escapeHtml, formatDateTime, formatFileSize } from '../core/formatters.js';
import { i18n } from '../core/i18n.js';
import { thumbnail } from '../features/thumbnail.js';

/**
 * @import {FileItem, FolderItem} from '../core/types.js'
 */

/**
 * @typedef {Object} ResourceListConfig
 *
 * Feature flags
 * @property {boolean}  [selectable=true]      - Show per-item checkboxes and enable selection.
 * @property {boolean}  [showFavorite=true]    - Show the favorite-star button on each item.
 * @property {boolean}  [showOwner=false]      - Show the owner column initially.
 * @property {boolean}  [showShareBadge=true]  - Show the shared-resource badge on items.
 * @property {boolean}  [draggable=false]      - Mark items as draggable (HTML attribute).
 * @property {boolean}  [showContextMenu=true] - Enable the three-dots button and right-click menu.
 *
 * Appearance
 * @property {string}   [itemModifierClass]    - Extra CSS class applied to every .file-item
 *                                              (e.g. 'favorite-item', 'recent-item').
 * @property {string}   [dateField='modified_at'] - Which date field to display in the date column.
 * @property {string}   [dateLabel]            - Column header label for the date column (i18n key).
 *
 * State providers (called at item-creation time)
 * @property {(id: string, type: 'file'|'folder') => boolean} [isFavorite]
 * @property {(id: string, type: 'file'|'folder') => boolean} [isShared]
 *
 * Callbacks (all optional; the component silently skips missing ones)
 * @property {(item: FileItem|FolderItem, event: MouseEvent) => void} [onOpen]
 *   Called when the user clicks an item (not a button inside it).
 * @property {(item: FileItem|FolderItem) => Promise<void>} [onFavoriteToggle]
 *   Called when the user clicks the favorite-star button.
 * @property {(item: FileItem|FolderItem, event: MouseEvent) => void} [onContextMenu]
 *   Called for the three-dots button click, right-click, and shared-badge click.
 * @property {(selected: Array<FileItem|FolderItem>) => void} [onSelectionChange]
 *   Called whenever the selection set changes.
 */

export class ResourceListComponent {
    /**
     * @param {HTMLElement}        container - The element that will contain .file-item nodes.
     * @param {ResourceListConfig} config
     */
    constructor(container, config) {
        this._container = container;

        /** @type {Required<Pick<ResourceListConfig,'selectable'|'showFavorite'|'showOwner'|'showShareBadge'|'draggable'|'showContextMenu'|'dateField'>> & ResourceListConfig} */
        this._cfg = {
            selectable: true,
            showFavorite: true,
            showOwner: false,
            showShareBadge: true,
            draggable: false,
            showContextMenu: true,
            dateField: 'modified_at',
            ...config
        };

        /** Items registered with this instance, keyed by id. */
        /** @type {Map<string, FileItem|FolderItem>} */
        this._items = new Map();

        /** IDs of currently selected items. */
        /** @type {Set<string>} */
        this._selected = new Set();

        /** Index of the last clicked item — used for shift-click range selection. */
        this._lastClickedIndex = -1;

        this._ownerVisible = this._cfg.showOwner;

        this._initDelegation();
    }

    // ── Public API ──────────────────────────────────────────────────────────

    /**
     * Replace the current item list.  Preserves an existing `.list-header`
     * at the start of the container.
     *
     * @param {FolderItem[]} folders
     * @param {FileItem[]}   files
     * @param {((item: FileItem|FolderItem) => string|null)=} groupFn
     *   When provided, a swimlane divider is injected whenever the returned
     *   label changes.  Return `null` to suppress the divider for that item.
     */
    render(folders, files, groupFn) {
        const header = this._container.querySelector('.list-header');
        this._container.innerHTML = '';
        if (header) this._container.appendChild(header);

        this._selected.clear();
        this._items.clear();
        this._lastClickedIndex = -1;

        // Prevent ui.js global delegation from firing on this container
        this._container.dataset.managedBy = 'resource-list';

        this._appendItems(folders, files, groupFn);
        this._wireSelectAll();
    }

    /**
     * Append additional items without clearing the existing ones (load-more).
     *
     * @param {FolderItem[]} folders
     * @param {FileItem[]}   files
     * @param {((item: FileItem|FolderItem) => string|null)=} groupFn
     */
    append(folders, files, groupFn) {
        this._appendItems(folders, files, groupFn);
    }

    /** Remove all items (but keep `.list-header` if present). */
    clear() {
        const header = this._container.querySelector('.list-header');
        this._container.innerHTML = '';
        if (header) this._container.appendChild(header);
        this._selected.clear();
        this._items.clear();
        this._lastClickedIndex = -1;
        // Hand delegation back to ui.js
        delete this._container.dataset.managedBy;
    }

    /**
     * Deselect all items without removing them from the DOM.
     * Used by the batch toolbar after an operation completes.
     */
    clearSelection() {
        this._selected.clear();
        this._lastClickedIndex = -1;
        this._container.querySelectorAll('.file-item.selected').forEach((card) => {
            card.classList.remove('selected');
            const cb = /** @type {HTMLInputElement | null} */ (card.querySelector('.item-checkbox'));
            if (cb) cb.checked = false;
        });
        this._syncSelectAllCheckbox();
        this._cfg.onSelectionChange?.([]);
    }

    /**
     * Select all visible items in the container.
     */
    selectAll() {
        this._container.querySelectorAll('.file-item').forEach((card) => {
            const el = /** @type {HTMLElement} */ (card);
            const id = el.dataset.fileId || el.dataset.folderId || '';
            if (!id) return;
            el.classList.add('selected');
            const cb = /** @type {HTMLInputElement | null} */ (el.querySelector('.item-checkbox'));
            if (cb) cb.checked = true;
            this._selected.add(id);
        });
        this._syncSelectAllCheckbox();
        this._notifySelectionChange();
    }

    /**
     * Switch between grid and list rendering mode.
     * @param {'grid'|'list'} mode
     */
    setViewMode(mode) {
        this._container.classList.toggle('files-grid-view', mode === 'grid');
        this._container.classList.toggle('files-list-view', mode === 'list');
    }

    /**
     * Show or hide the owner column on all current and future items.
     * @param {boolean} visible
     */
    setOwnerVisible(visible) {
        this._ownerVisible = visible;
        this._container.querySelectorAll('.owner-cell').forEach((cell) => {
            cell.classList.toggle('hidden', !visible);
        });
    }

    /**
     * Update the favorite-star visual on a specific item without re-rendering.
     * @param {string}  id
     * @param {'file'|'folder'} type
     * @param {boolean} isFavorite
     */
    setFavoriteVisualState(id, type, isFavorite) {
        const selector = type === 'folder' ? `.file-item[data-folder-id="${id}"]` : `.file-item[data-file-id="${id}"]`;
        const item = this._container.querySelector(selector);
        if (!item) return;

        const star = item.querySelector('.favorite-star');
        if (star) {
            star.classList.toggle('active', isFavorite);
            const i = star.querySelector('i');
            if (i) {
                i.classList.toggle('fas', isFavorite);
                i.classList.toggle('far', !isFavorite);
            }
        }

        const badge = item.querySelector('.file-badge-favorite');
        badge?.classList.toggle('hidden', !isFavorite);
    }

    /**
     * Update the shared-badge visual on a specific item without re-rendering.
     * @param {string}  id
     * @param {'file'|'folder'} type
     * @param {boolean} isShared
     */
    setSharedVisualState(id, type, isShared) {
        const selector = type === 'folder' ? `.file-item[data-folder-id="${id}"]` : `.file-item[data-file-id="${id}"]`;
        const item = this._container.querySelector(selector);
        if (!item) return;
        item.querySelector('.file-badge-shared')?.classList.toggle('hidden', !isShared);
    }

    // ── Private helpers ─────────────────────────────────────────────────────

    /**
     * @param {FolderItem[]} folders
     * @param {FileItem[]}   files
     * @param {((item: FileItem|FolderItem) => string|null)=} groupFn
     */
    _appendItems(folders, files, groupFn) {
        const fragment = document.createDocumentFragment();
        let lastGroupKey = /** @type {string|null|undefined} */ (undefined);

        for (const folder of folders) {
            this._items.set(folder.id, folder);
            if (groupFn) {
                const key = groupFn(folder);
                if (key !== lastGroupKey) {
                    lastGroupKey = key;
                    if (key !== null) fragment.appendChild(this._createGroupHeader(key));
                }
            }
            fragment.appendChild(this._createFolderItem(folder));
        }

        for (const file of files) {
            this._items.set(file.id, file);
            if (groupFn) {
                const key = groupFn(file);
                if (key !== lastGroupKey) {
                    lastGroupKey = key;
                    if (key !== null) fragment.appendChild(this._createGroupHeader(key));
                }
            }
            fragment.appendChild(this._createFileItem(file));
        }

        this._container.appendChild(fragment);
    }

    /**
     * Create a swimlane divider element.
     * @param {string} label
     */
    _createGroupHeader(label) {
        const el = document.createElement('div');
        el.className = 'resource-list__swimlane-header';
        el.dataset.swimlaneHeader = 'true';
        el.textContent = label;
        return el;
    }

    /**
     * Build a .file-item DOM element for a folder.
     * @param {FolderItem} folder
     * @returns {HTMLElement}
     */
    _createFolderItem(folder) {
        const cfg = this._cfg;
        const el = document.createElement('div');
        const modClass = cfg.itemModifierClass ? ` ${cfg.itemModifierClass}` : '';
        el.className = `file-item${modClass}`;
        el.dataset.folderId = folder.id;
        el.dataset.folderName = folder.name;
        el.dataset.parentId = folder.parent_id || '';
        if (folder.path) el.dataset.path = folder.path;
        if (cfg.draggable) el.setAttribute('draggable', 'true');

        const isFav = cfg.isFavorite ? cfg.isFavorite(folder.id, 'folder') : false;
        const isShared = cfg.isShared ? cfg.isShared(folder.id, 'folder') : false;
        const dateVal = /** @type {Record<string,string>} */ (/** @type {unknown} */ (folder))[cfg.dateField] ?? folder.modified_at;
        const formattedDate = formatDateTime(new Date(dateVal));

        el.innerHTML = `
            ${cfg.selectable ? '<div class="checkbox-cell"><input type="checkbox" class="item-checkbox"></div>' : ''}
            <div class="name-cell">
                <div class="file-icon folder-icon">
                    <i class="fas fa-folder"></i>
                </div>
                <span>${escapeHtml(folder.name)}</span>
                ${cfg.showFavorite ? `<div class="file-badge file-badge-favorite${isFav ? '' : ' hidden'}"><i class="fas fa-star favorite-star-inline"></i></div>` : ''}
                ${cfg.showShareBadge ? `<div class="file-badge file-badge-shared${isShared ? '' : ' hidden'}"><i class="fas fa-oxiexport"></i></div>` : ''}
            </div>
            <div class="owner-cell${this._ownerVisible ? '' : ' hidden'}" data-owner-id="${escapeHtml(folder.owner_id || '')}"></div>
            <div class="type-cell">${i18n.t('files.file_types.folder')}</div>
            <div class="size-cell">--</div>
            <div class="date-cell">${formattedDate}</div>
            <div class="action-cell">
                ${cfg.showFavorite ? `<button class="favorite-star${isFav ? ' active' : ''}"><i class="${isFav ? 'fas' : 'far'} fa-star"></i></button>` : ''}
                ${cfg.showContextMenu ? '<button class="file-actions"><i class="fas fa-ellipsis-v"></i></button>' : ''}
            </div>
        `;

        this._bindItemEvents(el, folder);
        return el;
    }

    /**
     * Build a .file-item DOM element for a file.
     * @param {FileItem} file
     * @returns {HTMLElement}
     */
    _createFileItem(file) {
        const cfg = this._cfg;
        const iconClass = file.icon_class || 'fas fa-file';
        const iconSpecialClass = file.icon_special_class || '';
        const cat = file.category || '';
        const typeLabel = cat ? i18n.t(`files.file_types.${cat.toLowerCase()}`) || cat : i18n.t('files.file_types.document');
        const fileSize = file.size_formatted || formatFileSize(file.size);
        const dateVal = /** @type {Record<string,string>} */ (/** @type {unknown} */ (file))[cfg.dateField] ?? file.modified_at;
        const formattedDate = formatDateTime(new Date(dateVal));
        const isFav = cfg.isFavorite ? cfg.isFavorite(file.id, 'file') : false;
        const isShared = cfg.isShared ? cfg.isShared(file.id, 'file') : false;
        const canThumbnail = thumbnail?.canHandle(file) ?? false;

        const el = document.createElement('div');
        const modClass = cfg.itemModifierClass ? ` ${cfg.itemModifierClass}` : '';
        el.className = `file-item${modClass}`;
        el.dataset.fileId = file.id;
        el.dataset.fileName = file.name;
        el.dataset.folderId = file.folder_id || '';
        if (file.path) el.dataset.path = file.path;
        if (cfg.draggable) el.setAttribute('draggable', 'true');

        el.innerHTML = `
            ${cfg.selectable ? '<div class="checkbox-cell"><input type="checkbox" class="item-checkbox"></div>' : ''}
            <div class="name-cell">
                <div class="file-icon ${iconSpecialClass}">
                    ${canThumbnail ? `<img class="file-thumb" src="/api/files/${file.id}/thumbnail/icon" loading="lazy" alt="">` : ''}
                    <i class="${iconClass}"></i>
                </div>
                <span>${escapeHtml(file.name)}</span>
                ${cfg.showFavorite ? `<div class="file-badge file-badge-favorite${isFav ? '' : ' hidden'}"><i class="fas fa-star favorite-star-inline"></i></div>` : ''}
                ${cfg.showShareBadge ? `<div class="file-badge file-badge-shared${isShared ? '' : ' hidden'}"><i class="fas fa-oxiexport"></i></div>` : ''}
            </div>
            <div class="owner-cell${this._ownerVisible ? '' : ' hidden'}" data-owner-id="${escapeHtml(file.owner_id || '')}"></div>
            <div class="type-cell">${typeLabel}</div>
            <div class="size-cell">${fileSize}</div>
            <div class="date-cell">${formattedDate}</div>
            <div class="action-cell">
                ${cfg.showFavorite ? `<button class="favorite-star${isFav ? ' active' : ''}"><i class="${isFav ? 'fas' : 'far'} fa-star"></i></button>` : ''}
                ${cfg.showContextMenu ? '<button class="file-actions"><i class="fas fa-ellipsis-v"></i></button>' : ''}
            </div>
        `;

        const thumb = /** @type {HTMLImageElement | null} */ (el.querySelector('.file-thumb'));
        if (thumb) {
            thumb.addEventListener('error', () => {
                thumb.classList.add('hidden');
                thumbnail?.queueGenerate(file, (dataUrl) => {
                    thumb.src = dataUrl;
                    thumb.classList.remove('hidden');
                });
            });
        }

        this._bindItemEvents(el, file);
        return el;
    }

    /**
     * Attach direct event listeners to interactive elements inside a .file-item.
     * This covers buttons that must stop propagation before the delegated listener runs.
     * @param {HTMLElement}          el
     * @param {FileItem|FolderItem}  item
     */
    _bindItemEvents(el, item) {
        const cfg = this._cfg;

        // Favorite-star — direct click, stopPropagation so the card open doesn't fire
        if (cfg.showFavorite && cfg.onFavoriteToggle) {
            const star = el.querySelector('.favorite-star');
            star?.addEventListener('click', (e) => {
                e.stopPropagation();
                e.stopImmediatePropagation();
                e.preventDefault();
                cfg.onFavoriteToggle?.(item);
            });
        }

        // Shared-badge click → treat as context-menu trigger (e.g. open share modal)
        if (cfg.showShareBadge && cfg.onContextMenu) {
            const badge = el.querySelector('.file-badge-shared');
            badge?.addEventListener('click', (e) => {
                e.stopPropagation();
                e.stopImmediatePropagation();
                e.preventDefault();
                cfg.onContextMenu?.(item, /** @type {MouseEvent} */ (e));
            });
        }
    }

    /** Wire one delegated listener for all pointer events in this container. */
    _initDelegation() {
        const container = this._container;
        const cfg = this._cfg;

        // ── click ──────────────────────────────────────────────────────────
        container.addEventListener('click', (e) => {
            const target = /** @type {HTMLElement} */ (e.target);

            // Swimlane dividers are not interactive
            if (target.dataset.swimlaneHeader) return;

            const card = /** @type {HTMLElement | null} */ (target.closest('.file-item'));
            if (!card) return;

            // Three-dots button → context menu
            if (target.closest('.file-actions')) {
                e.stopPropagation();
                e.preventDefault();
                const item = this._itemFromCard(card);
                if (item && cfg.onContextMenu) cfg.onContextMenu(item, /** @type {MouseEvent} */ (e));
                return;
            }

            // Checkbox cell → selection (shift extends range)
            if (cfg.selectable && target.closest('.checkbox-cell')) {
                if (e.shiftKey) {
                    this._handleShiftSelect(card);
                } else {
                    this._toggleSelection(card);
                }
                return;
            }

            // Favorite star is handled by the direct listener in _bindItemEvents
            if (target.closest('.favorite-star')) return;

            // Modifier-key click → selection toggle
            if (e.metaKey || e.altKey || e.ctrlKey) {
                if (cfg.selectable) this._toggleSelection(card);
                return;
            }

            // Shift-click anywhere on the card → extend selection range
            if (e.shiftKey && cfg.selectable) {
                this._handleShiftSelect(card);
                return;
            }

            // Plain click → open or navigate
            const item = this._itemFromCard(card);
            if (item && cfg.onOpen) cfg.onOpen(item, /** @type {MouseEvent} */ (e));
        });

        // ── contextmenu ────────────────────────────────────────────────────
        if (cfg.showContextMenu) {
            container.addEventListener('contextmenu', (e) => {
                const target = /** @type {HTMLElement} */ (e.target);
                if (target.dataset.swimlaneHeader) return;
                const card = /** @type {HTMLElement | null} */ (target.closest('.file-item'));
                if (!card) return;
                e.preventDefault();
                const item = this._itemFromCard(card);
                if (item && cfg.onContextMenu) cfg.onContextMenu(item, /** @type {MouseEvent} */ (e));
            });
        }

        // ── dblclick — prevent double-fire of open on rapid clicks ─────────
        container.addEventListener('dblclick', (e) => e.preventDefault());
    }

    /**
     * Return the registered item object for a given card element.
     * @param {HTMLElement} card
     * @returns {FileItem|FolderItem|undefined}
     */
    _itemFromCard(card) {
        const id = card.dataset.fileId || card.dataset.folderId || '';
        return this._items.get(id);
    }

    /**
     * Toggle selection on a single card and notify.
     * Tracks `_lastClickedIndex` for subsequent shift-clicks.
     * @param {HTMLElement} card
     */
    _toggleSelection(card) {
        const id = card.dataset.fileId || card.dataset.folderId || '';
        if (!id) return;

        const nowSelected = !card.classList.contains('selected');
        card.classList.toggle('selected', nowSelected);

        const checkbox = /** @type {HTMLInputElement | null} */ (card.querySelector('.item-checkbox'));
        if (checkbox) checkbox.checked = nowSelected;

        if (nowSelected) {
            this._selected.add(id);
        } else {
            this._selected.delete(id);
        }

        // Record position for the next shift-click
        const items = [...this._container.querySelectorAll('.file-item')];
        this._lastClickedIndex = items.indexOf(card);

        this._syncSelectAllCheckbox();
        this._notifySelectionChange();
    }

    /**
     * Extend the selection from `_lastClickedIndex` to `card` (inclusive).
     * If no previous click exists, falls back to a plain toggle.
     * @param {HTMLElement} card
     */
    _handleShiftSelect(card) {
        const items = /** @type {HTMLElement[]} */ ([...this._container.querySelectorAll('.file-item')]);
        const index = items.indexOf(card);

        if (this._lastClickedIndex >= 0 && index >= 0) {
            const start = Math.min(this._lastClickedIndex, index);
            const end = Math.max(this._lastClickedIndex, index);
            for (let i = start; i <= end; i++) {
                const el = items[i];
                const id = el.dataset.fileId || el.dataset.folderId || '';
                if (!id) continue;
                el.classList.add('selected');
                const cb = /** @type {HTMLInputElement | null} */ (el.querySelector('.item-checkbox'));
                if (cb) cb.checked = true;
                this._selected.add(id);
            }
        } else {
            this._toggleSelection(card);
            return;
        }

        this._lastClickedIndex = index;
        this._syncSelectAllCheckbox();
        this._notifySelectionChange();
    }

    /**
     * Find the select-all checkbox in the container header and wire its
     * `change` event.  Called after every `render()`.
     */
    _wireSelectAll() {
        if (!this._cfg.selectable) return;
        const cb = /** @type {HTMLInputElement | null} */ (this._container.querySelector('#select-all-checkbox'));
        if (!cb) return;
        // Replace with a fresh listener to avoid duplicates across re-renders
        const fresh = /** @type {HTMLInputElement} */ (cb.cloneNode(true));
        cb.parentNode?.replaceChild(fresh, cb);
        fresh.addEventListener('change', () => {
            if (fresh.checked) {
                this.selectAll();
            } else {
                this.clearSelection();
            }
        });
    }

    /**
     * Sync the three-state select-all checkbox in the list header.
     * Checked = all selected, indeterminate = some selected, unchecked = none.
     */
    _syncSelectAllCheckbox() {
        const cb = /** @type {HTMLInputElement | null} */ (this._container.querySelector('#select-all-checkbox'));
        if (!cb) return;
        const total = this._container.querySelectorAll('.file-item').length;
        if (total === 0) {
            cb.checked = false;
            cb.indeterminate = false;
        } else if (this._selected.size >= total) {
            cb.checked = true;
            cb.indeterminate = false;
        } else if (this._selected.size > 0) {
            cb.checked = false;
            cb.indeterminate = true;
        } else {
            cb.checked = false;
            cb.indeterminate = false;
        }
    }

    /** Build the selected-items array and fire `onSelectionChange`. */
    _notifySelectionChange() {
        if (!this._cfg.onSelectionChange) return;
        /** @type {Array<FileItem|FolderItem>} */
        const selectedItems = [...this._selected].flatMap((id) => {
            const item = this._items.get(id);
            return item ? [item] : [];
        });
        this._cfg.onSelectionChange(selectedItems);
    }
}
