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
import { systemUsers } from '../model/systemUsers.js';
import { buildResourceIcon } from './resourceIcon.js';
import { createUserVignette } from './userVignette.js';

/**
 * @import {FileItem, FolderItem} from '../core/types.js'
 */

/**
 * Reusable swimlane key for the client-side "just added" lane that
 * `addItem()` opens at the top of the list in grouped views. Distinct
 * from any natural group key the server might produce so the lookup
 * can't collide with a real bucket whose label happens to read "New".
 * @type {string}
 */
const JUST_ADDED_KEY = '__oxicloud_just_added__';

/**
 * @typedef {Object} CustomAction
 * @property {string} iconHtml   - Inner HTML for the button icon (e.g. `<i class="fas fa-undo"></i>`).
 * @property {string} [labelKey] - i18n key used for the button's `title` / `aria-label`.
 * @property {string} [className] - Extra CSS class(es) appended to `btn-action`.
 * @property {(item: FileItem|FolderItem) => (void|Promise<void>)} onClick
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
 * @property {boolean}  [showType=true]        - Render the Type column.
 * @property {boolean}  [showPath=false]       - Render the Path column (CSS hides it in grid mode).
 *
 * Appearance
 * @property {string}   [itemModifierClass]    - Extra CSS class applied to every .file-item
 *                                              (e.g. 'favorite-item', 'recent-item').
 * @property {string}   [dateField='modified_at'] - Which date field to display in the date column.
 * @property {string}   [dateLabel]            - Column header label for the date column (i18n key).
 * @property {(value: string | number | Date | null | undefined) => string} [dateFormatter]
 *   Override the date-cell formatter. Defaults to `formatDateTime`. Pass
 *   `formatDaysRemaining` for the Trash view to surface remaining lifetime.
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
 *   Called for the three-dots button click and right-click.
 * @property {(item: FileItem|FolderItem) => void} [onShareBadgeClick]
 *   Called when the user clicks the shared badge. Falls back to onContextMenu if absent.
 * @property {(selected: Array<FileItem|FolderItem>) => void} [onSelectionChange]
 *   Called whenever the selection set changes.
 *
 * Per-section inline actions
 * @property {CustomAction[]} [customActions]
 *   Extra buttons rendered in the action cell (always visible, both grid and list view).
 *   Use this for section-specific verbs like restore / permanently-delete on trash.
 */

export class ResourceListComponent {
    /**
     * @param {HTMLElement}        container - The element that will contain .file-item nodes.
     * @param {ResourceListConfig} config
     */
    constructor(container, config) {
        this._container = container;

        /** @type {Required<Pick<ResourceListConfig,'selectable'|'showFavorite'|'showOwner'|'showShareBadge'|'draggable'|'showContextMenu'|'showType'|'showPath'|'dateField'>> & ResourceListConfig} */
        this._cfg = {
            selectable: true,
            showFavorite: true,
            showOwner: false,
            showShareBadge: true,
            draggable: false,
            showContextMenu: true,
            showType: true,
            showPath: false,
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

        /**
         * The grouping key of the last rendered item — persisted across
         * `append()` calls so load-more pages don't insert a redundant header
         * when the first item of the new page shares a group with the last
         * item of the previous page.
         * `undefined` means no items have been rendered yet (reset in `render()`).
         * @type {string|null|undefined}
         */
        this._lastGroupKey = undefined;

        /**
         * The live DOM group wrapper of the last rendered swimlane — persisted
         * across `append()` calls so load-more items that continue the same
         * group are appended into the existing card rather than starting a new one.
         * @type {HTMLElement|null}
         */
        this._lastGroupEl = null;

        /**
         * Optional grouping-key resolver stored between `render()` / `append()`
         * calls so `addItem()` can place a new row in the correct swimlane
         * without the caller having to re-supply it. `undefined` means the
         * current view is flat (no group-by); `null` is never stored — only
         * function or `undefined`.
         * @type {((item: FileItem|FolderItem) => string|null) | undefined}
         */
        this._groupFn = undefined;

        /**
         * Optional label-resolver stored between `render()` and `append()` calls.
         * @type {((key: string) => string) | undefined}
         */
        this._groupLabelFn = undefined;

        /**
         * Optional node-builder stored between `render()` and `append()` calls.
         * When set, the swimlane header renders a DOM node instead of plain text
         * (e.g. a user vignette for the "owner" group-by dimension).
         * @type {((key: string) => HTMLElement) | undefined}
         */
        this._headerNodeFn = undefined;

        this._ownerVisible = this._cfg.showOwner;

        this._initDelegation();
    }

    // ── Public API ──────────────────────────────────────────────────────────

    /**
     * Replace the current item list.  Preserves an existing `.list-header`
     * at the start of the container.
     *
     * Items are rendered **in the order supplied** — do not pre-sort or
     * split them into folders/files; the caller (or server) owns ordering.
     * Folders vs. files are distinguished at render time by whether the item
     * has a `mime_type` property.
     *
     * @param {Array<FileItem|FolderItem>} items
     * @param {((item: FileItem|FolderItem) => string|null)=} groupFn
     *   When provided, a swimlane divider is injected whenever the returned
     *   key changes.  Return `null` to suppress the divider for that item.
     * @param {((key: string) => string)=} groupLabelFn
     *   Optional: converts the raw grouping key to a human-readable header
     *   label.  When omitted the key itself is used.
     * @param {((key: string) => HTMLElement)=} headerNodeFn
     *   Optional: builds a rich DOM node for the swimlane header (e.g. a user
     *   vignette for the "owner" group-by).  When provided, `groupLabelFn` is
     *   ignored for the header and the returned node is appended instead.
     */
    render(items, groupFn, groupLabelFn, headerNodeFn) {
        const header = this._container.querySelector('.list-header');
        this._container.innerHTML = '';
        if (header) this._container.appendChild(header);

        this._selected.clear();
        this._items.clear();
        this._lastClickedIndex = -1;
        // Reset group tracking for the fresh render
        this._lastGroupKey = undefined;
        this._lastGroupEl = null;
        this._groupFn = groupFn;
        this._groupLabelFn = groupLabelFn;
        this._headerNodeFn = headerNodeFn;

        // Prevent ui.js global delegation from firing on this container
        this._container.dataset.managedBy = 'resource-list';

        this._appendItems(items, groupFn, groupLabelFn, headerNodeFn);
        this._wireSelectAll();
    }

    /**
     * Append additional items without clearing the existing ones (load-more).
     * Continues swimlane grouping from the last item of the previous page —
     * no redundant header is inserted when the key is unchanged.
     *
     * @param {Array<FileItem|FolderItem>} items
     * @param {((item: FileItem|FolderItem) => string|null)=} groupFn
     * @param {((key: string) => string)=} groupLabelFn
     * @param {((key: string) => HTMLElement)=} headerNodeFn
     */
    append(items, groupFn, groupLabelFn, headerNodeFn) {
        // Persist the latest non-undefined callbacks so `addItem()` can
        // reuse them without the caller having to re-supply them on every
        // optimistic insertion.
        if (groupFn !== undefined) this._groupFn = groupFn;
        if (groupLabelFn !== undefined) this._groupLabelFn = groupLabelFn;
        if (headerNodeFn !== undefined) this._headerNodeFn = headerNodeFn;
        this._appendItems(items, groupFn ?? this._groupFn, groupLabelFn ?? this._groupLabelFn, headerNodeFn ?? this._headerNodeFn);
    }

    /** Remove all items (but keep `.list-header` if present). */
    clear() {
        const header = this._container.querySelector('.list-header');
        this._container.innerHTML = '';
        if (header) this._container.appendChild(header);
        this._selected.clear();
        this._items.clear();
        this._lastClickedIndex = -1;
        this._lastGroupKey = undefined;
        this._lastGroupEl = null;
        this._groupFn = undefined;
        this._groupLabelFn = undefined;
        this._headerNodeFn = undefined;
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
     * Return the registered item for the given id, or `undefined` if absent.
     * @param {string} id
     * @returns {FileItem|FolderItem|undefined}
     */
    getItem(id) {
        return this._items.get(id);
    }

    /**
     * Append a single item, skipping silently if already present (duplicate guard).
     * Clears the empty-state placeholder when the first item is added.
     *
     * Group-by aware: if the current view is grouped, the row goes into
     * a dedicated **"New" swimlane pinned at the top of the list** that
     * is created on first call and reused across subsequent inserts in
     * the same session. This deliberately sidesteps re-computing the
     * item's natural bucket on the client:
     *
     * - Different group-by dimensions (date, type, size, …) would each
     *   need their own resolver, and date-bucket math is sensitive to
     *   clock skew between client and server.
     * - Cross-swimlane sort-position is impossible to mirror exactly
     *   without re-implementing the server's tiebreaker chain.
     *
     * The "New" lane is purely client-side and dissolves on the next
     * full reload (when the server's authoritative grouping reasserts).
     * Predictable and uniform across every group-by mode.
     *
     * @param {FileItem|FolderItem} item
     * @param {{ scroll?: boolean, highlight?: boolean }} [opts]
     *   - `scroll`: smooth-scroll the new row into view. The
     *     `.resource-row--just-added` class also sets `scroll-margin`
     *     so the sticky page header doesn't cover the row.
     *   - `highlight`: flash a brief CSS pulse on the new row so the
     *     user can spot it amid similar siblings.
     * @returns {HTMLElement | null} The inserted row, or `null` when the
     *   item was deduped.
     */
    addItem(item, opts = {}) {
        if (this._items.has(item.id)) return null;
        // Also guard against stale DOM remnants not tracked in _items
        const isFile = 'mime_type' in item;
        const attr = isFile ? `data-file-id="${item.id}"` : `data-folder-id="${item.id}"`;
        if (this._container.querySelector(`.file-item[${attr}]`)) return null;
        this._container.classList.remove('hidden');

        /** @type {HTMLElement | null} */
        let row = null;

        if (this._groupFn) {
            // Grouped view → drop the new row into the top-of-list
            // "New" swimlane, creating it on first call.
            const lane = this._ensureJustAddedLane();
            this._items.set(item.id, item);
            row = isFile ? this._createFileItem(/** @type {FileItem} */ (item)) : this._createFolderItem(/** @type {FolderItem} */ (item));
            lane.appendChild(row);
        } else {
            // Flat list (no grouping) — append at the end like before.
            this._appendItems([item]);
            row = /** @type {HTMLElement | null} */ (this._container.querySelector(`.file-item[${attr}]`));
        }

        if (!row) return null;

        if (opts.highlight) {
            row.classList.add('resource-row--just-added');
            // Self-cleaning: drop the class once the keyframe completes
            // so a future re-render starts from a neutral baseline.
            row.addEventListener('animationend', () => row?.classList.remove('resource-row--just-added'), { once: true });
        }
        if (opts.scroll) {
            // `block: 'nearest'` is a no-op when the row is already in
            // view. `.resource-row--just-added` sets `scroll-margin-top`
            // so the sticky page header doesn't clip the row when the
            // scroll lands.
            row.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
        }

        return row;
    }

    /**
     * Ensure the top-of-list "New" swimlane exists and return its
     * wrapper. Created on first call after a render; reused across
     * subsequent `addItem()` calls in the same session. The lane
     * dissolves on the next `render()` / `clear()`, at which point
     * the server's authoritative grouping reasserts.
     *
     * Header label uses i18n key `groupby.justAdded` with an English
     * fallback so views that haven't translated it still read sensibly.
     *
     * @returns {HTMLElement}
     */
    _ensureJustAddedLane() {
        const existing = this._findLaneByKey(JUST_ADDED_KEY);
        if (existing) return existing;

        const lane = document.createElement('div');
        lane.className = 'resource-list__swimlane-group resource-list__swimlane-group--just-added';
        lane.dataset.groupKey = JUST_ADDED_KEY;

        const header = document.createElement('div');
        header.className = 'resource-list__swimlane-header';
        header.dataset.swimlaneHeader = 'true';
        header.textContent = i18n.t('groupby.justAdded', 'New');
        lane.appendChild(header);

        // Insert at the very top of the container, immediately after
        // the optional `.list-header` row, so the affordance is
        // discoverable and the row's scroll-into-view brings the
        // swimlane header into view too.
        const listHeader = this._container.querySelector('.list-header');
        if (listHeader?.nextSibling) {
            this._container.insertBefore(lane, listHeader.nextSibling);
        } else if (listHeader) {
            this._container.appendChild(lane);
        } else {
            this._container.prepend(lane);
        }
        return lane;
    }

    /**
     * Locate an on-screen swimlane wrapper by its group key. Returns
     * `null` when no swimlane currently matches.
     *
     * @param {string} key
     * @returns {HTMLElement | null}
     */
    _findLaneByKey(key) {
        // CSS.escape covers arbitrary key shapes (dates with colons,
        // UUIDs with dashes, etc.) so the attribute selector is safe.
        return /** @type {HTMLElement | null} */ (this._container.querySelector(`.resource-list__swimlane-group[data-group-key="${CSS.escape(String(key))}"]`));
    }

    /**
     * Asynchronously fill every un-resolved `.owner-cell` in this component's
     * container with the display name for its `data-owner-id` attribute.
     * Idempotent — cells already stamped with `data-owner-resolved` are skipped.
     * @returns {Promise<void>}
     */
    async resolveOwnerCells() {
        const cells = /** @type {NodeListOf<HTMLElement>} */ (this._container.querySelectorAll('.owner-cell[data-owner-id]:not([data-owner-resolved])'));
        if (!cells.length) return;
        systemUsers.prefetch(); // warm cache once (idempotent)
        for (const cell of cells) {
            const id = cell.dataset.ownerId;
            cell.dataset.ownerResolved = '1';
            if (!id) continue;
            cell.replaceChildren(createUserVignette(id, 'list'));
        }
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

    /**
     * Re-evaluate the shared badge for every currently rendered item using the
     * `isShared` callback from config.  Call this after the grants cache is refreshed.
     */
    refreshSharedBadges() {
        if (!this._cfg.isShared) return;
        for (const item of this._items.values()) {
            const isFile = 'mime_type' in item;
            const type = /** @type {'file'|'folder'} */ (isFile ? 'file' : 'folder');
            this.setSharedVisualState(item.id, type, this._cfg.isShared(item.id, type));
        }
    }

    // ── Private helpers ─────────────────────────────────────────────────────

    /**
     * Internal: append items to the container in the order supplied.
     * Files vs. folders are distinguished by presence of `mime_type`.
     *
     * @param {Array<FileItem|FolderItem>} items
     * @param {((item: FileItem|FolderItem) => string|null)=} groupFn
     * @param {((key: string) => string)=} groupLabelFn
     * @param {((key: string) => HTMLElement)=} headerNodeFn
     */
    _appendItems(items, groupFn, groupLabelFn, headerNodeFn) {
        const fragment = document.createDocumentFragment();

        // Start from the persisted key so load-more pages continue seamlessly.
        let lastGroupKey = this._lastGroupKey;

        // liveGroup: existing DOM group element from the previous page; items
        // that continue its group are appended directly into it (not via fragment).
        // fragmentGroup: the group wrapper currently being built in the fragment.
        let liveGroup = groupFn ? this._lastGroupEl : null;
        let fragmentGroup = /** @type {HTMLElement|null} */ (null);

        for (const item of items) {
            this._items.set(item.id, item);

            if (groupFn) {
                const key = groupFn(item);
                if (key !== lastGroupKey) {
                    lastGroupKey = key;
                    liveGroup = null; // stop extending the previous page's group
                    fragmentGroup = null;

                    if (key !== null) {
                        fragmentGroup = document.createElement('div');
                        fragmentGroup.className = 'resource-list__swimlane-group';
                        // Stamp the group key on the wrapper so `addItem()`
                        // can locate this swimlane later via
                        // `_findLaneByKey()` and append into it without a
                        // full re-render.
                        fragmentGroup.dataset.groupKey = key;
                        fragmentGroup.appendChild(this._createGroupHeader(key, groupLabelFn, headerNodeFn));
                        fragment.appendChild(fragmentGroup);
                    }
                }
            }

            // Dispatch to the correct renderer: files have mime_type, folders do not.
            const isFile = 'mime_type' in item;
            const itemEl = isFile ? this._createFileItem(/** @type {FileItem} */ (item)) : this._createFolderItem(/** @type {FolderItem} */ (item));

            // Priority: live DOM group (load-more continuation) > current fragment group > bare container
            const target = liveGroup ?? fragmentGroup;
            if (target) {
                target.appendChild(itemEl);
            } else {
                fragment.appendChild(itemEl);
            }
        }

        this._container.appendChild(fragment);
        // Persist for the next append() call (load-more continuity).
        this._lastGroupKey = lastGroupKey;
        // Track the last group element (live or freshly added) for the next page.
        this._lastGroupEl = fragmentGroup ?? liveGroup;
    }

    /**
     * Create a swimlane divider element.
     *
     * When `headerNodeFn` is supplied the header renders a rich DOM node
     * (e.g. a user vignette) instead of plain text; the `--node` CSS modifier
     * is added to suppress the small-caps / uppercase text styles.
     *
     * @param {string} key - Raw grouping key (e.g. UUID or bucket name).
     * @param {((key: string) => string)=}      labelFn     - Optional plain-text resolver.
     * @param {((key: string) => HTMLElement)=} headerNodeFn - Optional rich-node builder.
     */
    _createGroupHeader(key, labelFn, headerNodeFn) {
        const el = document.createElement('div');
        el.className = 'resource-list__swimlane-header';
        el.dataset.swimlaneHeader = 'true';
        if (headerNodeFn) {
            el.classList.add('resource-list__swimlane-header--node');
            el.appendChild(headerNodeFn(key));
        } else {
            el.textContent = labelFn ? labelFn(key) : key;
        }
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
        if (folder.owner_id) el.dataset.ownerId = folder.owner_id;
        if (cfg.draggable) el.setAttribute('draggable', 'true');

        const isFav = cfg.isFavorite ? cfg.isFavorite(folder.id, 'folder') : false;
        const isShared = cfg.isShared ? cfg.isShared(folder.id, 'folder') : false;
        const dateVal = /** @type {Record<string,string>} */ (/** @type {unknown} */ (folder))[cfg.dateField] ?? folder.modified_at;
        const formattedDate = cfg.dateFormatter ? cfg.dateFormatter(dateVal) : formatDateTime(new Date(dateVal));

        el.innerHTML = `
            ${cfg.selectable ? '<div class="checkbox-cell"><input type="checkbox" class="item-checkbox"></div>' : ''}
            <div class="name-cell">
                <div class="resource-icon-slot"></div>
                <span>${escapeHtml(folder.name)}</span>
                ${cfg.showFavorite ? `<div class="file-badge file-badge-favorite${isFav ? '' : ' hidden'}"><i class="fas fa-star favorite-star-inline"></i></div>` : ''}
                ${cfg.showShareBadge ? `<div class="file-badge file-badge-shared${isShared ? '' : ' hidden'}"><i class="fas fa-oxiexport"></i></div>` : ''}
            </div>
            <div class="owner-cell${this._ownerVisible ? '' : ' hidden'}" data-owner-id="${escapeHtml(folder.owner_id || '')}"></div>
            ${cfg.showPath ? `<div class="path-cell" title="${escapeHtml(folder.path || '')}">${escapeHtml(folder.path || '')}</div>` : ''}
            ${cfg.showType ? `<div class="type-cell">${i18n.t('files.file_types.folder')}</div>` : ''}
            <div class="size-cell">--</div>
            <div class="date-cell">${formattedDate}</div>
            <div class="action-cell">
                ${this._renderCustomActions()}
                ${cfg.showFavorite ? `<button class="favorite-star${isFav ? ' active' : ''}"><i class="${isFav ? 'fas' : 'far'} fa-star"></i></button>` : ''}
                ${cfg.showContextMenu ? '<button class="file-actions"><i class="fas fa-ellipsis-v"></i></button>' : ''}
            </div>
        `;

        el.querySelector('.resource-icon-slot')?.replaceWith(buildResourceIcon(folder, 'folder'));
        return el;
    }

    /**
     * Build a .file-item DOM element for a file.
     * @param {FileItem} file
     * @returns {HTMLElement}
     */
    _createFileItem(file) {
        const cfg = this._cfg;
        const cat = file.category || '';
        const typeLabel = cat ? i18n.t(`files.file_types.${cat.toLowerCase()}`) || cat : i18n.t('files.file_types.document');
        const fileSize = file.size_formatted || formatFileSize(file.size);
        const dateVal = /** @type {Record<string,string>} */ (/** @type {unknown} */ (file))[cfg.dateField] ?? file.modified_at;
        const formattedDate = cfg.dateFormatter ? cfg.dateFormatter(dateVal) : formatDateTime(new Date(dateVal));
        const isFav = cfg.isFavorite ? cfg.isFavorite(file.id, 'file') : false;
        const isShared = cfg.isShared ? cfg.isShared(file.id, 'file') : false;

        const el = document.createElement('div');
        const modClass = cfg.itemModifierClass ? ` ${cfg.itemModifierClass}` : '';
        el.className = `file-item${modClass}`;
        el.dataset.fileId = file.id;
        el.dataset.fileName = file.name;
        el.dataset.folderId = file.folder_id || '';
        if (file.path) el.dataset.path = file.path;
        if (file.owner_id) el.dataset.ownerId = file.owner_id;
        if (cfg.draggable) el.setAttribute('draggable', 'true');

        el.innerHTML = `
            ${cfg.selectable ? '<div class="checkbox-cell"><input type="checkbox" class="item-checkbox"></div>' : ''}
            <div class="name-cell">
                <div class="resource-icon-slot"></div>
                <span>${escapeHtml(file.name)}</span>
                ${cfg.showFavorite ? `<div class="file-badge file-badge-favorite${isFav ? '' : ' hidden'}"><i class="fas fa-star favorite-star-inline"></i></div>` : ''}
                ${cfg.showShareBadge ? `<div class="file-badge file-badge-shared${isShared ? '' : ' hidden'}"><i class="fas fa-oxiexport"></i></div>` : ''}
            </div>
            <div class="owner-cell${this._ownerVisible ? '' : ' hidden'}" data-owner-id="${escapeHtml(file.owner_id || '')}"></div>
            ${cfg.showPath ? `<div class="path-cell" title="${escapeHtml(file.path || '')}">${escapeHtml(file.path || '')}</div>` : ''}
            ${cfg.showType ? `<div class="type-cell">${typeLabel}</div>` : ''}
            <div class="size-cell">${fileSize}</div>
            <div class="date-cell">${formattedDate}</div>
            <div class="action-cell">
                ${this._renderCustomActions()}
                ${cfg.showFavorite ? `<button class="favorite-star${isFav ? ' active' : ''}"><i class="${isFav ? 'fas' : 'far'} fa-star"></i></button>` : ''}
                ${cfg.showContextMenu ? '<button class="file-actions"><i class="fas fa-ellipsis-v"></i></button>' : ''}
            </div>
        `;

        el.querySelector('.resource-icon-slot')?.replaceWith(buildResourceIcon(file, 'file'));
        return el;
    }

    /**
     * Render the inline action buttons declared in `cfg.customActions`.
     * Each button gets `data-custom-action="<index>"` so the binder can
     * dispatch by position.  Returns an empty string when no actions are
     * configured.
     * @returns {string}
     */
    _renderCustomActions() {
        const actions = this._cfg.customActions;
        if (!actions?.length) return '';
        return actions
            .map((a, i) => {
                const cls = a.className ? ` ${a.className}` : '';
                const label = a.labelKey ? escapeHtml(i18n.t(a.labelKey)) : '';
                return `<button type="button" class="btn-action${cls}" data-custom-action="${i}" title="${label}" aria-label="${label}">${a.iconHtml}</button>`;
            })
            .join('');
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

            // Favorite-star button — never opens the card
            if (target.closest('.favorite-star')) {
                e.preventDefault();
                const item = this._itemFromCard(card);
                if (item) cfg.onFavoriteToggle?.(item);
                return;
            }

            // Custom inline actions (e.g. restore / delete-permanently on trash)
            const customBtn = /** @type {HTMLElement | null} */ (target.closest('button[data-custom-action]'));
            if (customBtn) {
                e.preventDefault();
                const idx = Number(customBtn.dataset.customAction);
                const action = cfg.customActions?.[idx];
                const item = this._itemFromCard(card);
                if (action && item) action.onClick(item);
                return;
            }

            // Shared-badge click → open share modal (or fall back to context menu)
            if (target.closest('.file-badge-shared')) {
                e.preventDefault();
                const item = this._itemFromCard(card);
                if (item) {
                    if (cfg.onShareBadgeClick) {
                        cfg.onShareBadgeClick(item);
                    } else {
                        cfg.onContextMenu?.(item, /** @type {MouseEvent} */ (e));
                    }
                }
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
