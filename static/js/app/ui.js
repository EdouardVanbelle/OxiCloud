/**
 * OxiCloud - UI Module
 * This file handles UI-related functions, view toggling, and interface interactions
 */

// @ts-check

import { i18n } from '../core/i18n.js';
import { OxiIcons } from '../core/icons.js';
import * as viewPrefs from '../core/viewPrefs.js';
import { batchToolbar } from '../features/files/batchToolbar.js';
import { contextMenus } from '../features/files/contextMenus.js';
import { fileOps } from '../features/files/fileOperations.js';
import { inlineViewer } from '../features/files/inlineViewer.js';
import { wopiEditor } from '../features/files/wopiEditor.js';
import { recent } from '../features/library/recent.js';
import { buildBatchDownloadUrl } from '../utils/download.js';
import { positionMenu } from '../utils/menuPosition.js';
import { loadFiles } from './filesView.js';
import { updateHistory } from './main.js';
import { activateFilesUI, switchToFilesSection, syncViewContainers } from './navigation.js';
import { app } from './state.js';
import { uiFileTypes } from './uiFileTypes.js';
import { uiNotifications } from './uiNotifications.js';

/**
 *  @import {FileItem, FolderItem} from '../core/types.js'
 *  @import {BatchResult} from '../features/files/fileOperations.js'
 */

// UI Module
const ui = {
    /** @type {HTMLDivElement | null} */
    dragPreview: null,
    /** @type {HTMLDivElement | null} */
    draggedItems: null,

    /**
     * Whether the Owner column is currently visible.
     * Tracked so that newly rendered items can stamp the correct initial class.
     */
    _ownerVisible: false,

    /**
     * Initialize context menus and dialogs
     */
    initializeContextMenus() {
        // Folder context menu
        if (!document.getElementById('folder-context-menu')) {
            const folderMenu = document.createElement('div');
            folderMenu.classList.add('context-menu', 'hidden');
            folderMenu.id = 'folder-context-menu';
            folderMenu.innerHTML = `
                <div class="context-menu-item" id="download-folder-option">
                    <i class="fas fa-download"></i> <span data-i18n="actions.download">Download</span>
                </div>
                <div class="context-menu-item" id="favorite-folder-option">
                    <i class="fas fa-star"></i> <span data-i18n="actions.favorite">Add to favorites</span>
                </div>
                <div class="context-menu-item" id="share-folder-option">
                    <i class="fas fa-oxiexport"></i> <span data-i18n="actions.share">Share</span>
                </div>
                <div class="context-menu-separator"></div>
                <div class="context-menu-item" id="rename-folder-option">
                    <i class="fas fa-pen"></i> <span data-i18n="actions.rename">Rename</span>
                </div>
                <div class="context-menu-item" id="move-folder-option">
                    <i class="fas fa-arrows-alt"></i> <span data-i18n="actions.move">Move to...</span>
                </div>
                <div class="context-menu-separator"></div>
                <div class="context-menu-item context-menu-item-danger" id="delete-folder-option">
                    <i class="fas fa-trash-alt"></i> <span data-i18n="actions.delete">Delete</span>
                </div>
            `;
            document.body.appendChild(folderMenu);
            i18n.translateElement(folderMenu);
        }

        // File context menu
        if (!document.getElementById('file-context-menu')) {
            const fileMenu = document.createElement('div');
            fileMenu.classList.add('context-menu', 'hidden');
            fileMenu.id = 'file-context-menu';
            fileMenu.innerHTML = `
                <div class="context-menu-item" id="view-file-option">
                    <i class="fas fa-eye"></i> <span data-i18n="actions.view">View</span>
                </div>
                <div class="context-menu-item hidden" id="wopi-edit-file-option">
                    <i class="fas fa-file-word"></i> <span>Edit in Office</span>
                </div>
                <div class="context-menu-item hidden" id="wopi-edit-file-tab-option">
                    <i class="fas fa-external-link-alt"></i> <span>Edit in Office (new tab)</span>
                </div>
                <div class="context-menu-item" id="download-file-option">
                    <i class="fas fa-download"></i> <span data-i18n="actions.download">Download</span>
                </div>
                <div class="context-menu-item hidden" id="open-parent-folder-option">
                    <i class="fas fa-folder-open"></i> <span data-i18n="actions.open_parent_folder">Go to parent folder</span>
                </div>
                <div class="context-menu-separator"></div>
                <div class="context-menu-item" id="favorite-file-option">
                    <i class="fas fa-star"></i> <span data-i18n="actions.favorite">Add to favorites</span>
                </div>
                <div class="context-menu-item" id="share-file-option">
                    <i class="fas fa-oxiexport"></i> <span data-i18n="actions.share">Share</span>
                </div>
                <div class="context-menu-item hidden" id="add-to-playlist-option">
                    <i class="fas fa-compact-disc"></i> <span data-i18n="music.add_to_playlist">Add to Playlist</span>
                </div>
                <div class="context-menu-separator"></div>
                <div class="context-menu-item" id="rename-file-option">
                    <i class="fas fa-pen"></i> <span data-i18n="actions.rename">Rename</span>
                </div>
                <div class="context-menu-item" id="move-file-option">
                    <i class="fas fa-arrows-alt"></i> <span data-i18n="actions.move">Move to...</span>
                </div>
                <div class="context-menu-separator"></div>
                <div class="context-menu-item context-menu-item-danger" id="delete-file-option">
                    <i class="fas fa-trash-alt"></i> <span data-i18n="actions.delete">Delete</span>
                </div>
            `;
            document.body.appendChild(fileMenu);
            i18n.translateElement(fileMenu);
        }

        // Move dialog — modern with navigation
        if (!document.getElementById('move-file-dialog')) {
            const moveDialog = document.createElement('div');
            moveDialog.classList.add('rename-dialog', 'hidden');
            moveDialog.id = 'move-file-dialog';
            moveDialog.innerHTML = `
                <div class="rename-dialog-content">
                    <div class="rename-dialog-header">
                        <i class="fas fa-arrows-alt dialog-header-icon"></i>
                        <span data-i18n="dialogs.move_file">Move</span>
                    </div>
                    <div class="rename-dialog-body">
                        <p class="move-dialog-hint" data-i18n="dialogs.select_destination">Select destination folder:</p>
                        <div id="move-dialog-breadcrumb" class="move-dialog-breadcrumb"></div>
                        <div id="folder-select-container" class="folder-select-container">
                        </div>
                    </div>
                    <div class="rename-dialog-buttons">
                        <button class="btn btn-secondary" id="move-cancel-btn" data-i18n="actions.cancel">Cancel</button>
                        <button class="btn btn-outline" id="copy-confirm-btn" data-i18n="actions.copy">Copy</button>
                        <button class="btn btn-primary" id="move-confirm-btn" data-i18n="actions.move_to">Move</button>
                    </div>
                </div>
            `;
            document.body.appendChild(moveDialog);
        }

        // Share dialog is now handled by shareModal (components/shareModal.js)

        // Notification dialog
        if (!document.getElementById('notification-dialog')) {
            const notificationDialog = document.createElement('div');
            notificationDialog.classList.add('share-dialog', 'hidden');
            notificationDialog.id = 'notification-dialog';
            notificationDialog.innerHTML = `
                <div class="share-dialog-content">
                    <div class="share-dialog-header">
                        <i class="fas fa-envelope dialog-header-icon"></i>
                        <span data-i18n="dialogs.notify">Notify shared link</span>
                    </div>

                    <p><strong>URL:</strong> <span id="notification-share-url"></span></p>

                    <div class="form-group">
                        <label for="notification-email" data-i18n="dialogs.recipient">Recipient:</label>
                        <input type="email" id="notification-email" placeholder="Email address">
                    </div>

                    <div class="form-group">
                        <label for="notification-message" data-i18n="dialogs.message">Message (optional):</label>
                        <textarea id="notification-message" rows="3"></textarea>
                    </div>

                    <div class="share-dialog-buttons">
                        <button class="btn btn-secondary" id="notification-cancel-btn" data-i18n="actions.cancel">Cancel</button>
                        <button class="btn btn-primary" id="notification-send-btn" data-i18n="actions.send">Send</button>
                    </div>
                </div>
            `;
            document.body.appendChild(notificationDialog);

            // Add event listeners for notification dialog
            document.getElementById('notification-cancel-btn')?.addEventListener('click', () => {
                contextMenus.closeNotificationDialog();
            });

            document.getElementById('notification-send-btn')?.addEventListener('click', () => {
                contextMenus.sendShareNotification();
            });
        }

        // Playlist selection dialog
        if (!document.getElementById('playlist-dialog')) {
            const playlistDialog = document.createElement('div');
            playlistDialog.classList.add('share-dialog', 'hidden');
            playlistDialog.id = 'playlist-dialog';
            playlistDialog.innerHTML = `
                <div class="share-dialog-content">
                    <div class="share-dialog-header">
                        <i class="fas fa-music dialog-header-icon"></i>
                        <span data-i18n="music.add_to_playlist">Add to Playlist</span>
                    </div>
                    <div id="playlist-dialog-files-info" class="shared-item-info"></div>
                    <div id="playlist-select-container" class="folder-select-container">
                    </div>
                    <div class="rename-dialog-buttons">
                        <button class="btn btn-secondary" id="playlist-cancel-btn" data-i18n="actions.cancel">Cancel</button>
                        <button class="btn btn-primary" id="playlist-add-btn" data-i18n="music.add">Add</button>
                    </div>
                </div>
            `;
            document.body.appendChild(playlistDialog);

            document.getElementById('playlist-cancel-btn')?.addEventListener('click', () => {
                if (contextMenus) contextMenus.closePlaylistDialog();
            });
        }

        // Assign events to menu items
        if (contextMenus) {
            contextMenus.assignMenuEvents();
        } else {
            console.warn('contextMenus module not loaded');
        }
    },

    /**
     * Set up drag and drop functionality
     */
    setupDragAndDrop() {
        // prepare area to build dragged elements
        this.dragPreview = document.createElement('div');
        this.dragPreview.className = 'drag-preview';
        document.body.appendChild(this.dragPreview);
        this.draggedItems = null;

        const dropzone = document.getElementById('dropzone');

        /**
         *
         * @param {DataTransfer} dataTransfer
         * @returns {Promise<any[]|null>}
         */
        const collectDroppedEntries = async (dataTransfer) => {
            const items = Array.from(dataTransfer?.items || []);
            const rootEntries = items.map((it) => (typeof it.webkitGetAsEntry === 'function' ? it.webkitGetAsEntry() : null)).filter(Boolean);

            if (rootEntries.length === 0) return null;

            /** @type {Array<{file: File, relativePath: string}>} */
            const out = [];

            /**
             * @param {FileSystemEntry} entry
             * @param {string} prefix
             */
            const walkEntry = async (entry, prefix = '') => {
                if (!entry) return;

                if (entry.isFile) {
                    await new Promise((resolve) => {
                        /** @type {FileSystemFileEntry} */ (entry).file(
                            (/** @type {File} */ file) => {
                                out.push({ file, relativePath: `${prefix}${file.name}` });
                                resolve(undefined);
                            },
                            () => resolve(undefined)
                        );
                    });
                    return;
                }

                if (entry.isDirectory) {
                    const dirPrefix = `${prefix}${entry.name}/`;
                    const reader = /** @type {FileSystemDirectoryEntry} */ (entry).createReader();

                    while (true) {
                        const children = await new Promise((resolve) => {
                            reader.readEntries(resolve, () => resolve([]));
                        });
                        if (!children || children.length === 0) break;
                        for (const child of children) {
                            // eslint-disable-next-line no-await-in-loop
                            await walkEntry(child, dirPrefix);
                        }
                    }
                }
            };

            for (const root of rootEntries) {
                // eslint-disable-next-line no-await-in-loop
                await walkEntry(root, '');
            }

            return out;
        };

        // Dropzone events
        dropzone?.addEventListener('dragover', (e) => {
            e.preventDefault();
            dropzone.classList.add('active');
        });

        dropzone?.addEventListener('dragleave', () => {
            dropzone.classList.remove('active');
        });

        // remove previous hack e._oxiHandled
        // WeakSet will automatically garbage collect entry
        const handledDropEvents = new WeakSet();

        dropzone?.addEventListener('drop', async (e) => {
            e.preventDefault();
            e.stopPropagation(); // Prevent bubbling to document's drop handler (avoids double upload)
            handledDropEvents.add(e); // Mark as handled for document-level fallback
            dropzone.classList.remove('active');
            if (!e.dataTransfer) return;

            if (e.dataTransfer.files.length > 0) {
                // First try directory-aware extraction (Finder folder drag & drop)
                const droppedEntries = await collectDroppedEntries(e.dataTransfer);
                if (droppedEntries && droppedEntries.length > 0) {
                    const hasFolderStructure = droppedEntries.some((x) => x.relativePath?.includes('/'));
                    if (hasFolderStructure) {
                        fileOps.uploadFolderEntries(droppedEntries);
                    } else {
                        fileOps.uploadFiles(droppedEntries.map((x) => x.file));
                    }
                    setTimeout(() => {
                        dropzone?.classList.add('hidden');
                    }, 500);
                    return;
                }

                // Detect folder drops: files from folder drops have webkitRelativePath set
                const hasRelativePaths = Array.from(e.dataTransfer.files).some((f) => f.webkitRelativePath?.includes('/'));
                if (hasRelativePaths) {
                    fileOps.uploadFolderFiles(e.dataTransfer.files);
                } else {
                    fileOps.uploadFiles(e.dataTransfer.files);
                }
            }
            setTimeout(() => {
                dropzone?.classList.add('hidden');
            }, 500);
        });

        // Document-wide drag and drop — only active in the Files section
        document.addEventListener('dragover', (e) => {
            e.preventDefault();
            if (!e.dataTransfer) return;
            if (e.dataTransfer.types.includes('Files') && app.currentSection === 'files') {
                dropzone?.classList.remove('hidden');
                dropzone?.classList.add('active');
            }
        });

        document.addEventListener('dragleave', (e) => {
            if (e.clientX <= 0 || e.clientY <= 0 || e.clientX >= window.innerWidth || e.clientY >= window.innerHeight) {
                dropzone?.classList.remove('active');
                setTimeout(() => {
                    if (!dropzone?.classList.contains('active')) {
                        dropzone?.classList.add('hidden');
                    }
                }, 100);
            }
        });

        document.addEventListener('drop', async (e) => {
            e.preventDefault();
            dropzone?.classList.remove('active');

            // Skip if already handled by the dropzone handler (defensive against bubble leaks)
            if (handledDropEvents.has(e)) return;

            if (!e.dataTransfer) return;
            if (app.currentSection !== 'files') {
                if (e.dataTransfer.types.includes('Files')) {
                    this.showNotification(i18n.t('notifications.upload_files_section_title'), i18n.t('notifications.upload_files_section_body'));
                }
                return;
            }
            if (e.dataTransfer.files.length > 0) {
                // First try directory-aware extraction (Finder folder drag & drop)
                const droppedEntries = await collectDroppedEntries(e.dataTransfer);
                if (droppedEntries && droppedEntries.length > 0) {
                    const hasFolderStructure = droppedEntries.some((x) => x.relativePath?.includes('/'));
                    if (hasFolderStructure) {
                        fileOps.uploadFolderEntries(droppedEntries);
                    } else {
                        fileOps.uploadFiles(droppedEntries.map((x) => x.file));
                    }
                    setTimeout(() => {
                        dropzone?.classList.add('hidden');
                    }, 500);
                    return;
                }

                // Detect folder drops: files from folder drops have webkitRelativePath set
                const hasRelativePaths = Array.from(e.dataTransfer.files).some((f) => f.webkitRelativePath?.includes('/'));
                if (hasRelativePaths) {
                    fileOps.uploadFolderFiles(e.dataTransfer.files);
                } else {
                    fileOps.uploadFiles(e.dataTransfer.files);
                }
            }

            setTimeout(() => {
                dropzone?.classList.add('hidden');
            }, 500);
        });
    },

    /**
     * Switch to grid view
     */
    switchToGridView() {
        app.currentView = 'grid';
        localStorage.setItem('oxicloud-view', 'grid');
        if (app.currentSection) viewPrefs.saveView(app.currentSection, 'grid');

        syncViewContainers();
    },

    /**
     * Switch to list view
     */
    switchToListView() {
        app.currentView = 'list';
        localStorage.setItem('oxicloud-view', 'list');
        if (app.currentSection) viewPrefs.saveView(app.currentSection, 'list');

        syncViewContainers();
    },

    /**
     * Show or hide the Owner column. When hidden, no name-resolution calls are made.
     * Sections that show owner (SharedWithMe, Favorites) pass `true`; all others `false`.
     * @param {boolean} visible
     */
    setOwnerColumnVisible(visible) {
        this._ownerVisible = visible;
        document
            .getElementById('files-list')
            ?.querySelectorAll('.owner-cell')
            .forEach((cell) => {
                cell.classList.toggle('hidden', !visible);
            });
    },

    /**
     * Update breadcrumb navigation from the breadcrumbPath array.
     * Renders: Home > folder1 > folder2 > ...
     * Each segment is clickable to navigate back to that level.
     */
    updateBreadcrumb() {
        const breadcrumb = document.querySelector('.breadcrumb');
        if (breadcrumb) {
            breadcrumb.innerHTML = '';
        }
        const path = app.breadcrumbPath; // [{id, name}, ...]

        // -- Home icon (always present, clickable to go to root) --
        const homeIcon = document.createElement('span');
        homeIcon.className = 'breadcrumb-item breadcrumb-home';
        homeIcon.innerHTML = '<i class="fas fa-home"></i>';
        homeIcon.title = i18n.t('breadcrumb.home');

        // Home is always clickable if we have a home folder
        if (app.userHomeFolderId) {
            homeIcon.classList.add('breadcrumb-link');
            homeIcon.addEventListener('click', () => {
                app.breadcrumbPath = [];
                app.currentPath = app.userHomeFolderId;
                this.updateBreadcrumb();
                loadFiles();
            });
        }
        breadcrumb?.appendChild(homeIcon);

        // -- Root/Home + Intermediate + current segments --
        // NOTE: The home folder entry is added by rebuildBreadCrumb() (filesView.js) when it
        // reaches the root folder during traversal. updateBreadcrumb() just renders app.breadcrumbPath
        // as-is — no implicit mutation. This allows shared-folder navigation to show only the
        // reachable subtree without the home prefix leaking in.
        path.forEach((segment, index) => {
            const isLast = index === path.length - 1;

            // Separator
            const separator = document.createElement('span');
            separator.className = 'breadcrumb-separator';
            separator.textContent = '>';
            breadcrumb?.appendChild(separator);

            // Segment item
            const item = document.createElement('span');
            item.className = 'breadcrumb-item';
            item.textContent = segment.name;
            item.dataset.folderId = segment.id;

            if (!isLast) {
                // Intermediate segment: clickable – truncate path to this level
                item.classList.add('breadcrumb-link');
                item.addEventListener('click', () => {
                    app.breadcrumbPath = path.slice(0, index + 1);
                    app.currentPath = segment.id;
                    this.updateBreadcrumb();
                    loadFiles();
                });

                // can drag files on this folder
                // dragover – only folders are valid drop targets
                item.addEventListener('dragover', (e) => {
                    const card = /** @type {HTMLElement} */ (e.target).closest('span');
                    if (!card?.dataset.folderId) return;
                    e.preventDefault();
                    card.classList.add('drop-target');
                });

                // dragleave
                item.addEventListener('dragleave', (e) => {
                    console.log('dragleave ', e);
                    const card = /** @type {HTMLElement} */ (e.target).closest('span');
                    if (!card?.dataset.folderId) return;
                    card.classList.remove('drop-target');
                });

                // drop – only folders accept drops
                item.addEventListener('drop', async (e) => {
                    const card = /** @type {HTMLElement} */ (e.target).closest('span');
                    if (!card) return;
                    const targetFolderId = card.dataset.folderId;
                    if (!targetFolderId) return;

                    e.preventDefault();
                    card.classList.remove('drop-target');

                    const action = e.dataTransfer?.dropEffect;
                    if (action) {
                        await this._dropToFolder(action, targetFolderId, e.dataTransfer);
                    }
                });
            } else {
                // Last segment: current location, not clickable
                item.classList.add('breadcrumb-current');
            }
            breadcrumb?.appendChild(item);
        });
    },

    /**
     * Check if a file can be previewed in the viewer
     * @param {FileItem} file
     * @returns {boolean}
     */
    isViewableFile(file) {
        return uiFileTypes.isViewableFile(file);
    },

    /**
     * Get FontAwesome icon class for a filename based on its extension.
     * Used as fallback when the backend DTO doesn't include icon_class
     * (e.g. trash items).
     * @param {string} fileName
     */
    getIconClass(fileName) {
        return uiFileTypes.getIconClass(fileName);
    },

    /**
     * Get CSS special class for icon styling based on filename extension.
     * Used as fallback when the backend DTO doesn't include icon_special_class.
     * @param {string} fileName
     */
    getIconSpecialClass(fileName) {
        return uiFileTypes.getIconSpecialClass(fileName);
    },

    /**
     * Show notification
     * @param {string} title - Notification title
     * @param {string} message - Notification message
     */
    showNotification(title, message) {
        uiNotifications.show(title, message);
    },

    /**
     * Close folder context menu
     */
    closeContextMenu() {
        const menu = document.getElementById('folder-context-menu');
        if (menu) {
            menu.classList.add('hidden');
            app.contextMenuTargetFolder = null;
        }
    },

    /**
     * Close file context menu
     */
    closeFileContextMenu() {
        const menu = document.getElementById('file-context-menu');
        if (menu) {
            menu.classList.add('hidden');
            app.contextMenuTargetFile = null;
        }
    },

    _getActiveView() {
        if (app && app.currentView === 'list') return 'list';
        if (app && app.currentView === 'grid') return 'grid';

        const stored = localStorage.getItem('oxicloud-view');
        return stored === 'list' ? 'list' : 'grid';
    },

    /**
     * handle the drop
     * @param {string} action copy|move
     * @param {string} targetFolderId the target
     * @param {any} dataTransfer fallback if nothing is selected
     */
    async _dropToFolder(action, targetFolderId, dataTransfer) {
        const selection = batchToolbar.getSelection(targetFolderId);

        batchToolbar.clear();

        if (selection.fileIds.length === 0 && selection.folderIds.length === 0) {
            // try to use dataTransfer (direct move without selection)
            const id = dataTransfer.getData('text/plain');
            const isFolder = dataTransfer.getData('application/oxicloud-folder') === 'true';

            if (isFolder && id === targetFolderId) {
                console.log('nothing to do');
                return; //nothing to do
            }
            // append current item to selection
            if (isFolder) {
                console.log(`adding ${id} as folder`);
                selection.folderIds.push(id);
            } else {
                console.log(`adding ${id} as file`);
                selection.fileIds.push(id);
            }
        }

        console.log(`request ${action} of: `, selection);
        /*
        TODO do we prefer use atomic operation on 1 item ? like:
            await fileOps.moveFolder(sourceId, targetFolderId);
            await fileOps.moveFile(sourceId, targetFolderId);
        */

        /** @type {BatchResult} */
        let result;
        switch (action) {
            case 'copy':
                result = await fileOps.batchCopy(selection.fileIds, selection.folderIds, targetFolderId);
                break;

            case 'move':
                result = await fileOps.batchMove(selection.fileIds, selection.folderIds, targetFolderId);
                // redraw directory
                if (result.success > 0) loadFiles();
                break;

            default:
                console.error(`drag and drop: action ${action} unknown`);
                return;
        }
        batchToolbar.showBatchResult(action, result);
        console.log(result);
    },

    /**
     * Attach delegated drag-and-drop listeners to a files-list container.
     * Called once by `filesView.js` after the `ResourceListComponent` is created.
     * Idempotent — a second call on the same element is a no-op.
     *
     * Handles:
     *   - `dragstart` / `dragend` — visual preview + dataTransfer payload
     *   - `dragover` / `dragleave` / `drop` — folder drop targets (delegated)
     *
     * @param {HTMLElement} container  The `#files-list` element.
     */
    initDragDrop(container) {
        if (container.dataset.dragDropReady) return;
        container.dataset.dragDropReady = '1';

        // ── helpers ────────────────────────────────────────────────────────
        /** @param {HTMLElement} card */
        const itemInfo = (card) => {
            if (!card) return null;
            const fileId = card.dataset.fileId;
            if (fileId) return { type: 'file', id: fileId, name: card.dataset.fileName ?? '' };
            const folderId = card.dataset.folderId;
            if (folderId) return { type: 'folder', id: folderId, name: card.dataset.folderName ?? '' };
            return null;
        };

        // ── dragstart ──────────────────────────────────────────────────────
        container.addEventListener('dragstart', (e) => {
            const card = /** @type {HTMLDivElement | null} */ (/** @type {HTMLElement} */ (e.target).closest('.file-item'));
            if (!card) {
                e.preventDefault();
                return;
            }

            const info = itemInfo(card);
            if (!info) {
                e.preventDefault();
                return;
            }
            if (!e.dataTransfer) return;

            e.dataTransfer.setData('text/plain', info.id);
            if (info.type === 'folder') e.dataTransfer.setData('application/oxicloud-folder', 'true');
            e.dataTransfer.effectAllowed = 'copyMove';

            this.draggedItems = document.createElement('div');
            this.draggedItems.className = 'dragged-items';

            let selectedCards = container.querySelectorAll('div.selected > div.name-cell');
            if (selectedCards.length === 0) selectedCards = card.querySelectorAll('div.name-cell');

            const maxElements = 4;
            let lastItemDiv = null;
            let index = 0;

            while (index < selectedCards.length && index < maxElements) {
                const iconCell = document.createElement('div');
                const icon = selectedCards[index].getElementsByClassName('file-icon').item(0)?.cloneNode(true);
                if (icon) {
                    iconCell.appendChild(icon);
                    iconCell.querySelectorAll('img').forEach((img) => {
                        img.loading = 'eager';
                    });
                }
                const nameCell = document.createElement('div');
                const name = selectedCards[index].getElementsByTagName('span').item(0)?.cloneNode(true);
                if (name) nameCell.appendChild(name);

                const div = document.createElement('div');
                div.className = 'file-item';
                div.appendChild(iconCell);
                div.appendChild(nameCell);
                this.draggedItems.appendChild(div);
                lastItemDiv = div;
                index += 1;
            }

            let nameEncoded;
            let downloadUrl;
            if (selectedCards.length === 1) {
                if (info.type === 'file') {
                    nameEncoded = info.name.replaceAll(/:/g, '-');
                    downloadUrl = `${window.location.origin}/api/files/${info.id}`;
                } else {
                    nameEncoded = info.name.replaceAll(/:/g, '-').concat('.zip');
                    downloadUrl = `${window.location.origin}/api/folders/${info.id}/download?format=zip`;
                }
            } else {
                const now = new Date().toISOString().replace(/T/, ' ').replace(/\..*/, '').replaceAll(/:/g, '-');
                nameEncoded = `oxicloud ${now}.zip`;
                /** @type {string[]} */ const folderIds = [];
                /** @type {string[]} */ const fileIds = [];
                /** @type {NodeListOf<HTMLDivElement>} */ (container.querySelectorAll('div.selected')).forEach((el) => {
                    const item = itemInfo(/** @type {HTMLElement} */ (el));
                    if (item?.type === 'file') fileIds.push(item.id);
                    else if (item) folderIds.push(item.id);
                });
                downloadUrl = `${window.location.origin}${buildBatchDownloadUrl(fileIds, folderIds)}`;
            }

            e.dataTransfer.setData('DownloadURL', `application/octet-stream:${nameEncoded}:${downloadUrl}`);

            if (selectedCards.length > 1) {
                const badge = document.createElement('span');
                badge.className = 'dragged-items-badge';
                badge.innerText = `${selectedCards.length}`;
                this.draggedItems.appendChild(badge);
            }
            if (selectedCards.length > maxElements) lastItemDiv?.classList.add('fading');

            this.dragPreview?.appendChild(this.draggedItems);
            e.dataTransfer.setDragImage(this.draggedItems, 0, 0);
        });

        // ── dragend ────────────────────────────────────────────────────────
        container.addEventListener('dragend', () => {
            if (this.draggedItems && this.dragPreview?.contains(this.draggedItems)) {
                this.dragPreview.removeChild(this.draggedItems);
            }
            document.querySelectorAll('.drop-target').forEach((el) => {
                el.classList.remove('drop-target');
            });
        });

        // ── dragover — only folder cards are valid drop targets ────────────
        container.addEventListener('dragover', (e) => {
            const card = /** @type {HTMLElement} */ (/** @type {HTMLElement} */ (e.target).closest('.file-item'));
            if (!card || card.dataset.fileId || !card.dataset.folderId) return;
            e.preventDefault();
            card.classList.add('drop-target');
        });

        // ── dragleave ──────────────────────────────────────────────────────
        container.addEventListener('dragleave', (e) => {
            const card = /** @type {HTMLElement} */ (/** @type {HTMLElement} */ (e.target).closest('.file-item'));
            if (!card || card.dataset.fileId) return;
            card.classList.remove('drop-target');
        });

        // ── drop ───────────────────────────────────────────────────────────
        container.addEventListener('drop', async (e) => {
            const card = /** @type {HTMLElement} */ (/** @type {HTMLElement} */ (e.target).closest('.file-item'));
            if (!card || card.dataset.fileId || !card.dataset.folderId) return;
            e.preventDefault();
            card.classList.remove('drop-target');
            if (!e.dataTransfer) return;
            await this._dropToFolder(e.dataTransfer.dropEffect, card.dataset.folderId, e.dataTransfer);
        });
    },

    /* ================================================================
     *  Item open / navigate — shared by ui.js delegation and component
     *  callbacks so the same logic fires regardless of which view renders
     *  the items.
     * ================================================================ */

    /**
     * Open a file: dispatch a recent-access event, try WOPI, fall back to
     * inline viewer or download.
     * @param {FileItem} file
     */
    async _openFile(file) {
        if (!file) return;
        if (recent) {
            document.dispatchEvent(new CustomEvent('file-accessed', { detail: { file } }));
        }
        // WOPI editor intercept: open Office documents in the WOPI editor
        // But NOT image files - those should be previewed in the inline viewer
        const ext = (file.name || '').split('.').pop().toLowerCase();
        const imageExts = ['jpg', 'jpeg', 'png', 'gif', 'svg', 'webp', 'bmp', 'ico', 'heic', 'heif', 'avif', 'tiff'];
        const isImage = file.mime_type?.startsWith('image/') || imageExts.includes(ext);
        try {
            if (!isImage && wopiEditor && (await wopiEditor.canEdit(file.name))) {
                await wopiEditor.openInModal(file.id, file.name, 'edit');
                return;
            }
        } catch (e) {
            console.warn(`WOPI Editor failed, falling back to classic view`, e);
        }
        if (this.isViewableFile(file) || isImage) {
            if (inlineViewer) {
                inlineViewer.openFile(file);
                app.viewFile = file.id;
                updateHistory(false);
            } else {
                fileOps.downloadFile(file.id, file.name);
            }
        } else {
            fileOps.downloadFile(file.id, file.name);
        }
    },

    /**
     * Navigate into a folder, handling section transitions (SharedWithMe,
     * Favorites, Recent → Files).
     * @param {string|undefined} folderId
     * @param {string|undefined} folderName
     */
    _navigateToFolder(folderId, folderName) {
        if (!folderId) return;
        if (app.currentSection === 'favorites' || app.currentSection === 'recent') {
            switchToFilesSection();
            app.currentPath = folderId;
            loadFiles();
            return;
        }
        if (app.currentSection === 'sharedwithme' || app.currentSection === 'shared') {
            // Activate Files UI (nav, breadcrumb, actions bar) without
            // resetting the path — the shared folder becomes the entry point.
            activateFilesUI();
        }
        app.breadcrumbPath.push({ id: folderId, name: folderName || '' });
        app.currentPath = folderId;
        this.updateBreadcrumb();
        loadFiles();
    },

    /**
     * Open a file or navigate into a folder.
     * Used as the `onOpen` callback for `ResourceListComponent`.
     * @param {FileItem|FolderItem} item
     */
    async openItem(item) {
        if ('mime_type' in item) {
            await this._openFile(/** @type {FileItem} */ (item));
        } else {
            const folder = /** @type {FolderItem} */ (item);
            this._navigateToFolder(folder.id, folder.name);
        }
    },

    /**
     * Set the context-menu target and show the appropriate menu.
     * Used as the `onContextMenu` callback for `ResourceListComponent`.
     * @param {FileItem|FolderItem} item
     * @param {MouseEvent}          e
     */
    showContextMenuForItem(item, e) {
        const trigger = /** @type {HTMLElement | null} */ (/** @type {HTMLElement} */ (e.target).closest('.file-actions'));
        const menuId = 'mime_type' in item ? 'file-context-menu' : 'folder-context-menu';
        if ('mime_type' in item) {
            app.contextMenuTargetFile = /** @type {FileItem} */ (item);
        } else {
            app.contextMenuTargetFolder = /** @type {FolderItem} */ (item);
        }

        if (trigger) {
            showContextMenuAtElement(trigger, menuId);
            return;
        }
        // Right-click on the row body with no kebab in scope — open at
        // the cursor. positionMenu() clamps into the viewport, so menus
        // near the bottom of the screen no longer overflow off-screen.
        const menu = /** @type {HTMLElement | null} */ (document.getElementById(menuId));
        if (!menu) return;
        contextMenus.sync();
        positionMenu(menu, { x: e.pageX, y: e.pageY });
    },

    /**
     * Sync favorite visuals for a file/folder across grid and list views.
     * @param {string} itemId
     * @param {string} itemType
     * @param {boolean} isFavorite
     */
    setFavoriteVisualState(itemId, itemType, isFavorite) {
        const selector = itemType === 'folder' ? `#files-list .file-item[data-folder-id="${itemId}"]` : `#files-list .file-item[data-file-id="${itemId}"]`;

        const item = document.querySelector(selector);
        const starBtn = item?.querySelector('.favorite-star');

        // chzn
        if (starBtn) {
            starBtn.classList.toggle('active', !!isFavorite);

            // SVG icon path (after icons.js replacement)
            const svg = starBtn.querySelector('svg');
            const filledPath = OxiIcons?.star;
            const outlinePath = OxiIcons?.['star-outline'];
            const targetPath = isFavorite ? filledPath : outlinePath;
            if (svg && targetPath) {
                const p = svg.querySelector('path');
                if (p) p.setAttribute('d', String(targetPath[1]));
                svg.setAttribute('viewBox', `0 0 ${targetPath[0]} 512`);
            }

            // Fallback <i> icon (before icons.js replacement)
            const i = starBtn.querySelector('i');
            if (i) {
                i.classList.remove('fas', 'far');
                i.classList.add(isFavorite ? 'fas' : 'far');
            }
        }

        // toggle favorite's badge
        if (item) {
            const badgeFavorite = item.querySelector('.file-badge-favorite');
            badgeFavorite?.classList.toggle('hidden', !isFavorite);
        }
    },

    /**
     * @param {string} itemId
     * @param {string} itemType
     * @param {boolean} isShared
     */
    setSharedVisualState(itemId, itemType, isShared) {
        console.log(`setSharedVisual call for ${itemId} ${itemType} to ${isShared}`);
        const selector = itemType === 'folder' ? `#files-list .file-item[data-folder-id="${itemId}"]` : `#files-list .file-item[data-file-id="${itemId}"]`;
        // toggle favorite's badge
        const item = document.querySelector(selector);
        if (item) {
            const badgeShared = item.querySelector('.file-badge-shared');
            badgeShared?.classList.toggle('hidden', !isShared);
        }
    },

    /* ================================================================
     *  Element-creation helpers
     * ================================================================ */

    resetFilesList() {
        const filesList = document.getElementById('files-list');
        const filesContainerError = document.getElementById('files-container-error');

        if (!filesList) return;
        // Let ui.js delegation handle this container again
        delete filesList.dataset.managedBy;

        filesList.innerHTML = `
            <div class="list-header">
                <div class="list-header-checkbox"><input type="checkbox" id="select-all-checkbox" title="Select all"></div>
                <div data-i18n="files.name">Name</div>
                <div class="owner-cell${this._ownerVisible ? '' : ' hidden'}" data-i18n="files.owner">Owner</div>
                <div data-i18n="files.type">Type</div>
                <div data-i18n="files.size">Size</div>
                <div data-i18n="files.modified">Modified</div>
                <div></div><!-- actions -->
            </div>`;

        i18n.translateElement(filesList);

        filesList.classList.remove('hidden');
        filesContainerError?.classList.add('hidden');
    },

    showEmptyList() {
        this.showError(`
                <i class="fas fa-folder-open empty-state-icon"></i>
                <p data-i18n="files.no_files"></p>
                <p data-i18n="files.empty_hint"></p>
            `);
    },

    /**
     *
     * @param {string} content
     *
     */
    showError(content) {
        const filesContainerError = document.getElementById('files-container-error');
        const filesList = document.getElementById('files-list');
        if (filesContainerError) filesContainerError.innerHTML = content;

        i18n.translateElement(filesContainerError);

        filesContainerError?.classList.remove('hidden');
        filesList?.classList.add('hidden');
    }
};

// --- Global helper functions for card interactions ---

/**
 * Toggle selection state of a file/folder card.
 * Routes through the batchToolbar module so batch actions know about selected items.
 * @param {HTMLDivElement} card
 * @param {MouseEvent} event
 */
function toggleCardSelection(card, event) {
    if (batchToolbar) {
        batchToolbar.handleToggleItem(card, event);
    } else {
        card.classList.toggle('selected');
    }
}

/**
 * Show the context menu anchored next to a trigger element (the 3-dot button).
 * @param {HTMLElement} triggerElement
 * @param {string} menuId
 */
function showContextMenuAtElement(triggerElement, menuId) {
    // Hide any open menus first
    document.querySelectorAll('.context-menu').forEach((m) => {
        m.classList.add('hidden');
    });

    const menu = /** @type {HTMLElement | null} */ (document.getElementById(menuId));
    if (!menu) return;

    contextMenus.sync();
    positionMenu(menu, { anchor: triggerElement });
}

/**
 * Rubber band (lasso) selection — click + drag on empty grid area
 * to draw a rectangle and select all cards it touches.
 */
function initRubberBandSelection() {
    // Create the visual rectangle element once
    let selRect = document.getElementById('selection-rect');
    if (!selRect) {
        selRect = document.createElement('div');
        selRect.id = 'selection-rect';
        selRect.className = 'selection-rect';
        document.body.appendChild(selRect);
    }

    let active = false;
    let startX = 0,
        startY = 0;
    let curX = 0,
        curY = 0;
    let rafId = 0;

    /**
     * Card geometry snapshot taken once per drag (and rebuilt on scroll).
     * Comparing the lasso against these cached rects means the per-frame
     * pass performs zero DOM reads — no forced reflow per card.
     * @type {Array<{el: HTMLElement, left: number, top: number, right: number,
     *               bottom: number, info: ReturnType<typeof batchToolbar._extractInfo>,
     *               selected: boolean}> | null}
     */
    let cardCache = null;

    const buildCardCache = () => {
        cardCache = [];
        document.querySelectorAll('#files-list .file-item').forEach((card) => {
            const el = /** @type {HTMLElement} */ (card);
            const r = el.getBoundingClientRect();
            cardCache.push({
                el,
                left: r.left,
                top: r.top,
                right: r.right,
                bottom: r.bottom,
                info: batchToolbar ? batchToolbar._extractInfo(/** @type {HTMLDivElement} */ (el)) : null,
                selected: el.classList.contains('selected')
            });
        });
    };

    // Scrolling mid-drag shifts every viewport rect — drop the snapshot so
    // the next frame rebuilds it.
    const invalidateCardCache = () => {
        cardCache = null;
    };

    /** One classification pass per animation frame (cached rects only). */
    const classifyCards = () => {
        rafId = 0;
        if (!cardCache) buildCardCache();

        const left = Math.min(startX, curX);
        const top = Math.min(startY, curY);
        const right = Math.max(startX, curX);
        const bottom = Math.max(startY, curY);

        for (const entry of cardCache) {
            const intersects = entry.left < right && entry.right > left && entry.top < bottom && entry.bottom > top;
            if (intersects === entry.selected) continue;
            entry.selected = intersects;
            entry.el.classList.toggle('selected', intersects);

            // Sync with batchToolbar module (only on state change)
            if (batchToolbar && entry.info) {
                if (intersects) {
                    batchToolbar.select(entry.info.id, entry.info.name, entry.info.type, entry.info.parentId);
                } else {
                    batchToolbar.deselect(entry.info.id);
                }
            }
        }
    };

    // We listen on the whole files-container (covers grid + empty space)
    const container = document.querySelector('.files-container') || document.getElementById('files-list');
    if (!container) return;

    container.addEventListener('mousedown', (e) => {
        if (!(e instanceof MouseEvent)) return;
        // Only start if clicking empty area (not on a card, button, menu, input…)
        if (e.button !== 0) return; // left click only
        const target = /** @type {Element} */ (e.target);
        if (
            target.closest('.file-item') ||
            target.closest('.context-menu') ||
            target.closest('.upload-dropdown') ||
            target.closest('button') ||
            target.closest('input') ||
            target.closest('.breadcrumb') ||
            target.closest('.list-header')
        )
            return;

        active = true;
        startX = e.clientX;
        startY = e.clientY;
        curX = startX;
        curY = startY;
        cardCache = null; // built lazily on the first classification frame
        document.addEventListener('scroll', invalidateCardCache, { capture: true, passive: true });

        selRect.style.left = `${startX}px`;
        selRect.style.top = `${startY}px`;
        selRect.style.width = '0px';
        selRect.style.height = '0px';
        selRect.style.display = 'none'; // show only after a small movement

        e.preventDefault(); // prevent text selection
    });

    document.addEventListener('mousemove', (e) => {
        if (!active) return;

        curX = e.clientX;
        curY = e.clientY;

        const left = Math.min(startX, curX);
        const top = Math.min(startY, curY);
        const width = Math.abs(curX - startX);
        const height = Math.abs(curY - startY);

        // Only show the rect after a small threshold to avoid flicker on click
        if (width > 5 || height > 5) {
            selRect.style.display = 'block';
        }

        // Style writes only — no layout reads here. The card highlighting
        // runs at most once per frame against the cached geometry.
        selRect.style.left = `${left}px`;
        selRect.style.top = `${top}px`;
        selRect.style.width = `${width}px`;
        selRect.style.height = `${height}px`;

        if (!rafId) rafId = requestAnimationFrame(classifyCards);
    });

    document.addEventListener('mouseup', () => {
        if (!active) return;
        active = false;
        document.removeEventListener('scroll', invalidateCardCache, { capture: true });
        // Apply the still-pending classification so the final lasso
        // position is what determines the selection.
        if (rafId) {
            cancelAnimationFrame(rafId);
            classifyCards();
        }
        cardCache = null;
        const hadSelection = selRect.style.display === 'block';
        selRect.style.display = 'none';
        // Update the batch bar after rubber band selection completes
        if (batchToolbar) batchToolbar._syncUI();
        // Suppress the click event that follows mouseup so the global
        // deselect handler doesn't immediately clear the selection.
        if (hadSelection) {
            requestAnimationFrame(() => {});
        }
    });
}

// Initialize rubber band once DOM is ready
if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', initRubberBandSelection);
} else {
    initRubberBandSelection();
}

/**
 * Show a modern confirm dialog (replaces native confirm())
 * @param {Object} options
 * @param {string} options.title - Dialog title
 * @param {string} options.message - Dialog message/body
 * @param {string} [options.confirmText='Confirmar'] - Text for confirm button
 * @param {string} [options.cancelText='Cancelar'] - Text for cancel button
 * @param {boolean} [options.danger=false] - Use danger styling (red)
 * @returns {Promise<boolean>} true if confirmed, false if cancelled
 */
function showConfirmDialog({ title, message, confirmText, cancelText, danger = true }) {
    const ct = confirmText || i18n.t('actions.delete');
    const cc = cancelText || i18n.t('actions.cancel');
    const t = title || i18n.t('dialogs.confirm_title');

    return new Promise((resolve) => {
        // Remove any previous confirm dialog
        const prev = document.getElementById('confirm-dialog-overlay');
        if (prev) prev.remove();

        const overlay = document.createElement('div');
        overlay.id = 'confirm-dialog-overlay';
        overlay.className = 'confirm-dialog';
        overlay.innerHTML = `
            <div class="confirm-dialog-content">
                <div class="confirm-dialog-icon">
                    <i class="fas ${danger ? 'fa-exclamation-triangle' : 'fa-question-circle'}"></i>
                </div>
                <div class="confirm-dialog-title">${t}</div>
                <div class="confirm-dialog-message">${message || ''}</div>
                <div class="confirm-dialog-buttons">
                    <button class="btn btn-secondary confirm-dialog-cancel">${cc}</button>
                    <button class="btn ${danger ? 'btn-danger' : 'btn-primary'} confirm-dialog-ok">${ct}</button>
                </div>
            </div>
        `;
        document.body.appendChild(overlay);

        // Force layout then show
        requestAnimationFrame(() => {
            overlay.classList.add('active');
        });

        /** @param {boolean} result */
        const cleanup = (result) => {
            overlay.classList.remove('active');
            setTimeout(() => overlay.remove(), 200);
            resolve(result);
        };

        overlay.querySelector('.confirm-dialog-cancel')?.addEventListener('click', () => cleanup(false));
        overlay.querySelector('.confirm-dialog-ok')?.addEventListener('click', () => cleanup(true));
        overlay.addEventListener('click', (e) => {
            if (e.target === overlay) cleanup(false);
        });
    });
}

export { initRubberBandSelection, showConfirmDialog, showContextMenuAtElement, toggleCardSelection, ui };
