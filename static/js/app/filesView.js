// @ts-check

/**
 * OxiCloud – Files section view.
 *
 * Orchestrates the main Files section:
 *   - Data fetching via `filesModel` (cursor-paginated `/api/folders/{id}/resources`)
 *   - Rendering via a `ResourceListComponent` instance with optional swimlane grouping
 *   - Drag-and-drop initialisation (delegated to `ui.initDragDrop`)
 *
 * Exports:
 *   - `loadFiles`   – navigation & deep-link entry-point
 *   - `addItem`     – post-upload / post-create optimistic UI updates
 *   - `filesView`   – group-by controller consumed by `navigation.js` / `main.js`
 */

import { ResourceListComponent } from '../components/resourceList.js';
import { shareModal } from '../components/shareModal.js';
import { normalizeDateBucket, sizeBucket } from '../core/formatters.js';
import { i18n } from '../core/i18n.js';
import * as viewPrefs from '../core/viewPrefs.js';
import { batchToolbar } from '../features/files/batchToolbar.js';
import { inlineViewer } from '../features/files/inlineViewer.js';
import { favorites } from '../features/library/favorites.js';
import { fetchResourcesPage, rebuildBreadCrumb } from '../model/filesModel.js';
import { grants } from '../model/grants.js';
import { attachInfiniteScroll } from '../utils/infiniteScroll.js';
import { resolveHomeFolder } from './authSession.js';
import { updateHistory } from './main.js';
import { app } from './state.js';
import { ui } from './ui.js';
import { uiNotifications } from './uiNotifications.js';

/** @import {FileItem, FolderItem} from '../core/types.js' */

/**
 * @typedef {{ key: string, label: string, icon?: string, orderBy: string,
 *             keyFn?: (item: FileItem|FolderItem) => string|null,
 *             labelFn?: (key: string) => string }} GroupByDef
 */

// ── Group-by dimension definitions ───────────────────────────────────────────

/**
 * Group-by dimension definitions for the Files section.
 * Mirrors the same shape used by `sharedWithMeView.groupByDefs` so `main.js`
 * can drive the group-by dropdown generically.
 *
 * @type {GroupByDef[]}
 */
const GROUP_BY_DEFS = [
    {
        key: '',
        get label() {
            return i18n.t('files.name', 'Name');
        },
        icon: 'fas fa-arrow-up-a-z',
        orderBy: 'name'
        // no keyFn → flat list.
    },
    {
        key: 'type',
        get label() {
            return i18n.t('groupby.type', 'Type');
        },
        icon: 'fas fa-layer-group',
        orderBy: 'type',
        // Folders → 'Folder'; files → their pre-computed category string.
        keyFn: (item) => ('mime_type' in item ? /** @type {Record<string,string>} */ (/** @type {unknown} */ (item)).category || 'other' : 'Folder'),
        labelFn: (key) => {
            // biome-ignore format: keep indentation
            /** @type {Record<string, string>} */
            const labels = {
                Folder:       i18n.t('groupby.type.folders',     'Folders'),
                Image:        i18n.t('category.images',          'Images'),
                Video:        i18n.t('category.videos',          'Videos'),
                Audio:        i18n.t('category.audio',           'Audio'),
                PDF:          'PDF',
                Document:     i18n.t('category.documents',       'Documents'),
                Spreadsheet:  i18n.t('category.spreadsheets',    'Spreadsheets'),
                Presentation: i18n.t('category.presentations',   'Presentations'),
                Archive:      i18n.t('category.archives',        'Archives'),
                Code:         i18n.t('category.code',            'Code'),
                Markdown:     i18n.t('category.markdown',        'Markdown'),
                Text:         i18n.t('category.text',            'Text'),
                Installer:    i18n.t('category.installers',      'Installers')
            };
            return labels[key] ?? key;
        }
    },
    {
        key: 'size',
        get label() {
            return i18n.t('groupby.size', 'Size');
        },
        icon: 'fas fa-layer-group',
        orderBy: 'size',
        // sizeBucket(-1) → "Folders" sentinel; no labelFn needed.
        keyFn: (item) => {
            if (!('mime_type' in item)) return sizeBucket(-1);
            const r = /** @type {Record<string, number>} */ (/** @type {unknown} */ (item));
            return sizeBucket(r.size ?? 0);
        }
    },
    {
        key: 'modifiedAt',
        get label() {
            return i18n.t('groupby.modifiedAt', 'Modified date');
        },
        icon: 'fas fa-layer-group',
        orderBy: 'modified_at',
        // keyFn returns the human-readable bucket; the bucket IS the key.
        keyFn: (item) => {
            const r = /** @type {Record<string, number>} */ (/** @type {unknown} */ (item));
            return r.modified_at ? normalizeDateBucket(r.modified_at) : null;
        }
    },
    {
        key: 'createdAt',
        get label() {
            return i18n.t('groupby.createdAt', 'Created date');
        },
        icon: 'fas fa-layer-group',
        orderBy: 'created_at',
        keyFn: (item) => {
            const r = /** @type {Record<string, number>} */ (/** @type {unknown} */ (item));
            return r.created_at ? normalizeDateBucket(r.created_at) : null;
        }
    }
];

// ── Module-level state ────────────────────────────────────────────────────────

/** ID of the "Load more" wrapper injected below `.files-container`. */
const LOAD_MORE_ID = 'files-load-more-wrapper';

/** @type {ResourceListComponent|null} */
let _component = null;

/** Guard against concurrent `_loadPage` calls. */
let _loading = false;

/** Opaque cursor for the next page; `null` on first page or when exhausted. */
let _nextCursor = /** @type {string|null} */ (null);

/**
 * Active group-by key: '' = no grouping (name order), or one of the keys
 * from GROUP_BY_DEFS.
 * @type {string}
 */
let _groupBy = '';

/** Whether the current sort order is reversed. */
let _reversed = false;

// ── Group-by controller (public API, consumed by navigation.js / main.js) ───

/**
 * Controller object registered with `setGroupByView()` by navigation.js when
 * the Files section is active.  Exposes the same interface as
 * `sharedWithMeView` so the generic group-by infrastructure in `main.js`
 * drives both sections identically.
 */
const filesView = {
    /**
     * The group-by dimension definitions for this section.
     * `main.js` reads this to populate the Group-by dropdown dynamically.
     * @returns {GroupByDef[]}
     */
    get groupByDefs() {
        return GROUP_BY_DEFS;
    },

    /**
     * Change the active group-by dimension and reload from page 1.
     * Calling with the current key is a no-op.
     * @param {string} key  '' | 'type' | 'modifiedAt' | 'createdAt' | 'size'
     */
    setGroupBy(key) {
        if (_groupBy === key) return;
        _groupBy = key;
        viewPrefs.save('files', _groupBy, _reversed, viewPrefs.load('files').view);
        _nextCursor = null;
        _component?.clear();
        _loadPage({ isFirstPage: true });
    },

    /**
     * Flip the sort direction and reload from page 1.
     * Calling with the current value is a no-op.
     * @param {boolean} reversed
     */
    setDirection(reversed) {
        if (_reversed === reversed) return;
        _reversed = reversed;
        viewPrefs.save('files', _groupBy, _reversed, viewPrefs.load('files').view);
        _nextCursor = null;
        _component?.clear();
        _loadPage({ isFirstPage: true });
    }
};

// ── Component factory ─────────────────────────────────────────────────────────

/**
 * Return (creating on first call) the `ResourceListComponent` bound to
 * `#files-list`. The element must already be in the DOM.
 * @returns {ResourceListComponent|null}
 */
function _ensureComponent() {
    const filesList = document.getElementById('files-list');
    if (!filesList) return null;

    if (!_component) {
        _component = new ResourceListComponent(/** @type {HTMLElement} */ (filesList), {
            selectable: true,
            showFavorite: true,
            showOwner: true,
            showShareBadge: true,
            draggable: true,
            showContextMenu: true,
            isFavorite: (id, type) => favorites.isFavorite(id, type),
            isShared: (id, type) => grants.getOutgoingGrantsFor(type, id).length > 0,
            onOpen: (item) => ui.openItem(item),
            onFavoriteToggle: async (item) => {
                const isFile = 'mime_type' in item;
                const type = isFile ? 'file' : 'folder';
                if (favorites.isFavorite(item.id, type)) {
                    await favorites.removeFromFavorites(item.id, type, item.name);
                    _component?.setFavoriteVisualState(item.id, type, false);
                } else {
                    await favorites.addToFavorites(item.id, item.name, type, null);
                    _component?.setFavoriteVisualState(item.id, type, true);
                }
            },
            onShareBadgeClick: (item) => {
                const isFile = 'mime_type' in item;
                shareModal.open(item, isFile ? 'file' : 'folder', () => {
                    grants.fetchOutgoingGrants().then(() => refreshSharedBadges());
                });
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
                        parentId: isFile ? /** @type {FileItem} */ (sel).folder_id || '' : /** @type {FolderItem} */ (sel).parent_id || ''
                    });
                }
                batchToolbar._syncUI();
            }
        });

        // Wire drag-and-drop on the container once the component is created.
        ui.initDragDrop(/** @type {HTMLElement} */ (filesList));
    }

    _ensureLoadMoreButton();

    return _component;
}

// ── "Load more" button ────────────────────────────────────────────────────────

/**
 * Create the "Load more" wrapper once and attach it below `.files-container`.
 * Subsequent calls are no-ops.
 */
function _ensureLoadMoreButton() {
    if (document.getElementById(LOAD_MORE_ID)) return;

    const filesContainer = document.querySelector('.files-container');
    if (!filesContainer) return;

    const wrapper = document.createElement('div');
    wrapper.id = LOAD_MORE_ID;
    wrapper.className = 'swm-load-more-wrapper hidden';

    const btn = document.createElement('button');
    btn.id = 'files-load-more';
    btn.className = 'button secondary';
    btn.textContent = i18n.t('files.loadMore', 'Load more');
    btn.addEventListener('click', () => {
        _loadPage({ isFirstPage: false });
    });

    wrapper.appendChild(btn);
    filesContainer.after(wrapper);

    attachInfiniteScroll(wrapper, () => _loadPage({ isFirstPage: false }));
}

/**
 * @param {boolean} visible
 */
function _setLoadMoreVisible(visible) {
    const w = document.getElementById(LOAD_MORE_ID);
    if (w) w.classList.toggle('hidden', !visible);
}

// ── Page loader ───────────────────────────────────────────────────────────────

/**
 * Fetch one cursor page and render it.
 * @param {{ isFirstPage?: boolean }} [opts]
 * @returns {Promise<void>}
 */
async function _loadPage({ isFirstPage = false } = {}) {
    if (_loading) return;
    _loading = true;

    try {
        const def = GROUP_BY_DEFS.find((d) => d.key === _groupBy);
        const orderBy = def?.orderBy ?? 'name';

        const { items, nextCursor } = await fetchResourcesPage(app.currentPath, {
            cursor: _nextCursor,
            orderBy,
            limit: 50,
            reverse: _reversed
        });

        _nextCursor = nextCursor;

        if (items.length === 0 && isFirstPage) {
            ui.showEmptyList();
            _setLoadMoreVisible(false);
            return;
        }

        if (isFirstPage) {
            _component?.render(items, def?.keyFn, def?.labelFn);
        } else {
            _component?.append(items, def?.keyFn, def?.labelFn);
        }

        await _component?.resolveOwnerCells();
        _setLoadMoreVisible(!!nextCursor);
    } catch (/** @type {any} */ err) {
        if (err?.status === 403) {
            ui.showError(`<p>${i18n.t('errors.forbidden', 'Could not load files')}</p>`);
        } else {
            console.error('filesView: load error', err);
            uiNotifications.show('Error', 'Could not load files and folders');
        }
    } finally {
        _loading = false;
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/**
 * Append a single item to the current view (post-upload / post-create
 * optimistic update). No-op when the Files section is not active or the
 * item is already in the list.
 *
 * Called by `fileOperations.js` and `search.js`.
 *
 * @param {FileItem|FolderItem} item
 */
function addItem(item) {
    const component = _ensureComponent();
    if (!component) return;
    // Hide the empty-state placeholder if it's currently showing —
    // creating a folder in an empty directory should reveal the new
    // row, not display both states side-by-side. (The component will
    // un-hide `#files-list` itself when the item is inserted.)
    document.getElementById('files-container-error')?.classList.add('hidden');
    // Hand the item to the component so it can place it in the right
    // swimlane (when the current view is grouped) and pulse-highlight
    // + smooth-scroll it into view. We deliberately do NOT call
    // `ui.resetFilesList()` here — that wipes the rendered DOM,
    // defeating the whole point of an optimistic single-item insert.
    component.addItem(item, { scroll: true, highlight: true });
}

/**
 * Load and render the contents of `app.currentPath`, rebuilding the
 * breadcrumb and updating browser history.
 *
 * @param {Object}  [options]
 * @param {boolean} [options.insertHistory=true]
 * @param {boolean} [options.forceRefresh=false]  (legacy — kept for callers; ignored internally)
 */
async function loadFiles(options = { insertHistory: true }) {
    if (_loading) {
        console.log('A file load is already in progress, ignoring request');
        return;
    }

    // Reset cursor on navigation; restore saved group-by/direction preferences.
    _nextCursor = null;
    const _savedPrefs = viewPrefs.load('files');
    _groupBy = _savedPrefs.groupBy;
    _reversed = _savedPrefs.reversed;

    // Delay spinner so fast loads avoid the flash
    const spinnerTimeout = setTimeout(() => {
        ui.showError(`
            <div class="files-loading-spinner">
                <div class="spinner"></div>
                <span>${i18n.t('files.loading')}</span>
            </div>
        `);
    }, 100);

    // A temporary guard: _loadPage sets _loading itself, but we need to
    // block re-entrant loadFiles() calls during the setup below.
    _loading = true;

    try {
        if (!app.userHomeFolderId) await resolveHomeFolder();

        // External users have no home folder. If they land on /files
        // without a specific folder id in the URL, redirect them to
        // /#/sharedwithme — their actual landing page. This guards
        // against `fetchResourcesPage('')` building `/api/folders//resources`.
        if (app.isExternalUser && (!app.currentPath || app.currentPath === '')) {
            clearTimeout(spinnerTimeout);
            _loading = false;
            window.location.hash = '#/sharedwithme';
            return;
        }

        // Resolve path to home folder when none is set
        if (!app.currentPath || app.currentPath === '') {
            if (app.userHomeFolderId) {
                app.currentPath = app.userHomeFolderId;
                app.breadcrumbPath = [];
                console.log(`Loading user folder: ${app.userHomeFolderName} (${app.userHomeFolderId})`);
            } else {
                console.warn('No home folder id — this should not normally happen');
            }
        }

        // Rebuild the breadcrumb concurrently with the first page fetch —
        // the listing does not depend on it, and awaiting the ancestor
        // chain first added one round-trip per depth level before the
        // content even started loading. Crumbs, history and title update
        // when it resolves (skipped when superseded by a newer navigation).
        const requestedPath = app.currentPath;
        const breadcrumbReady = rebuildBreadCrumb().then((committed) => {
            if (!committed) return;
            ui.updateBreadcrumb();
            updateHistory(options.insertHistory ?? true);
        });

        clearTimeout(spinnerTimeout);

        // Prepare the container (shows #files-list, hides error panel)
        ui.resetFilesList();

        const component = _ensureComponent();
        if (!component) return;

        batchToolbar.clear();
        batchToolbar.init();
        batchToolbar.setActiveComponent(component);

        // Hand off to _loadPage (re-use cursor/groupBy state just reset above).
        _loading = false; // _loadPage sets its own guard
        await _loadPage({ isFirstPage: true });
        await breadcrumbReady;

        // rebuildBreadCrumb falls back to the home folder when the
        // requested folder is inaccessible (deleted / permission revoked).
        // The old sequential flow got the home listing for free; reload to
        // match it.
        if (app.currentPath !== requestedPath) {
            ui.resetFilesList();
            _nextCursor = null;
            await _loadPage({ isFirstPage: true });
        }

        // Deep-link: open a specific file if requested via app.viewFile.
        // We don't have a flat file list anymore (cursor pages), so only try
        // to open it if it was already rendered (first page).
        if (app.viewFile) {
            // Find the item among all rendered cards via the DOM attribute.
            const rendered = document.querySelector(`[data-id="${app.viewFile}"][data-type="file"]`);
            if (rendered) {
                // The component's item list may be sparse; ask for a fresh fetch.
                const fileRes = await fetch(`/api/files/${app.viewFile}`, {
                    credentials: 'same-origin',
                    cache: 'no-store'
                });
                if (fileRes.ok) {
                    const fileFound = /** @type {FileItem} */ (await fileRes.json());
                    await inlineViewer.openFile(fileFound);
                } else {
                    app.viewFile = null;
                    updateHistory(false);
                }
            } else {
                console.log(`file ${app.viewFile} not in first page — skipping auto-open`);
                app.viewFile = null;
                updateHistory(false);
            }
        }
    } catch (/** @type {any} */ err) {
        clearTimeout(spinnerTimeout);
        if (err?.status === 403) {
            ui.showError(`<p>${i18n.t('errors.forbidden', 'Could not load files')}</p>`);
        } else {
            console.error('Error loading folders:', err);
            uiNotifications.show('Error', 'Could not load files and folders');
        }
    } finally {
        _loading = false;
    }
}

/**
 * Replace the list contents with a whole result set in one batched render
 * (single DocumentFragment pass — no per-item DOM scans, smooth-scrolls or
 * highlight pulses). Used by search to display results; optimistic
 * single-item inserts should keep using `addItem`.
 *
 * @param {Array<FileItem|FolderItem>} items
 */
function renderItems(items) {
    const component = _ensureComponent();
    if (!component) return;
    document.getElementById('files-container-error')?.classList.add('hidden');
    component.render(items);
}

/**
 * Re-evaluate the shared badge for every item currently rendered in the Files list.
 * Call this after the outgoing grants cache has been refreshed.
 */
function refreshSharedBadges() {
    _component?.refreshSharedBadges();
}

export { addItem, filesView, loadFiles, refreshSharedBadges, renderItems };
