/**
 * OxiCloud - View navigation actions
 * Extracted from main.js to keep navigation concerns isolated.
 */

import { applyGroupByMenuState } from '../core/groupBySync.js';
import { i18n } from '../core/i18n.js';
import * as viewPrefs from '../core/viewPrefs.js';
import { batchToolbar } from '../features/files/batchToolbar.js';
import { favorites } from '../features/library/favorites.js';
import { musicView } from '../features/library/music.js';
import { photosView } from '../features/library/photos.js';
import { grants } from '../model/grants.js';
import { favoritesView } from '../views/favorites/favoritesView.js';
import { mySharesView } from '../views/myShares/mySharesView.js';
import { recentView } from '../views/recent/recentView.js';
import { sharedWithMeView } from '../views/sharedWithMe/sharedWithMeView.js';
import { trashView } from '../views/trash/trashView.js';
import { filesView, loadFiles, refreshSharedBadges } from './filesView.js';
import { setActionsBarMode, setGroupByView, syncGroupByMenu } from './main.js';
import { app, appElements } from './state.js';
import { ui } from './ui.js';

/**
 * Sync the hidden class and inline display for the grid/list containers
 * based on the current view preference.
 */
/**
 * Restore the grid/list view preference for a section before rendering.
 * @param {string} section  Matches `app.currentSection` values.
 */
function restoreView(section) {
    app.currentView = viewPrefs.resolveView(section);
}

function syncViewContainers() {
    const filesList = document.getElementById('files-list');
    const gridViewBtn = document.getElementById('grid-view-btn');
    const listViewBtn = document.getElementById('list-view-btn');

    const isGrid = app.currentView === 'grid';
    if (isGrid) {
        filesList?.classList.remove('files-list-view');
        filesList?.classList.add('files-grid-view');

        gridViewBtn?.classList.add('active');
        listViewBtn?.classList.remove('active');
    } else {
        filesList?.classList.add('files-list-view');
        filesList?.classList.remove('files-grid-view');

        gridViewBtn?.classList.remove('active');
        listViewBtn?.classList.add('active');
    }
}

/**
 * Hide file containers (used when switching to non-file views).
 * @param {boolean} show false to hide, true to show
 */
function toggleFileContainer(show) {
    const filesList = document.getElementById('files-list');
    filesList?.classList.toggle('hidden', !show);
}

/**
 * Mobile sidebar toggle functionality
 */
function initSidebarToggle() {
    const sidebarToggle = document.getElementById('sidebar-toggle');
    const sidebar = document.getElementById('sidebar');
    const sidebarOverlay = document.getElementById('sidebar-overlay');

    if (!sidebarToggle || !sidebar || !sidebarOverlay) return;

    /** @type {HTMLElement | null} */
    let drawerPrevFocus = null;

    function openSidebar() {
        drawerPrevFocus = /** @type {HTMLElement | null} */ (document.activeElement);
        sidebar?.classList.add('open');
        sidebarOverlay?.classList.add('active');
        document.body.style.overflow = 'hidden';
        sidebarToggle?.setAttribute('aria-expanded', 'true');
        /** @type {HTMLElement | null} */ (sidebar?.querySelector('.nav-item'))?.focus();
    }

    function closeSidebar() {
        sidebar?.classList.remove('open');
        sidebarOverlay?.classList.remove('active');
        document.body.style.overflow = '';
        sidebarToggle?.setAttribute('aria-expanded', 'false');
        drawerPrevFocus?.focus?.();
        drawerPrevFocus = null;
    }

    function toggleSidebar() {
        if (sidebar?.classList.contains('open')) {
            closeSidebar();
        } else {
            openSidebar();
        }
    }

    // Toggle button click
    sidebarToggle.addEventListener('click', toggleSidebar);

    // Close sidebar when clicking overlay
    sidebarOverlay.addEventListener('click', closeSidebar);

    // Trap Tab focus inside the open drawer (mobile).
    sidebar.addEventListener('keydown', (e) => {
        if (e.key !== 'Tab' || !sidebar.classList.contains('open')) return;
        const f = /** @type {NodeListOf<HTMLElement>} */ (
            sidebar.querySelectorAll('a[href], button:not([disabled]), [tabindex]:not([tabindex="-1"])')
        );
        if (!f.length) return;
        const first = f[0];
        const last = f[f.length - 1];
        if (e.shiftKey && document.activeElement === first) {
            e.preventDefault();
            last.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
            e.preventDefault();
            first.focus();
        }
    });

    // Close sidebar on escape key
    document.addEventListener('keydown', (e) => {
        if (e.key === 'Escape' && sidebar.classList.contains('open')) {
            closeSidebar();
        }
    });

    // Close sidebar when navigating (nav item click on mobile)
    const navItems = sidebar.querySelectorAll('.nav-item');
    navItems.forEach((item) => {
        item.addEventListener('click', () => {
            if (window.innerWidth <= 768) {
                closeSidebar();
            }
        });
    });
}

// Initialize sidebar toggle when DOM is ready
document.addEventListener('DOMContentLoaded', initSidebarToggle);

/**
 * Derive section name from nav item's data-i18n attribute.
 * @param {HTMLElement} navItem - The nav item element
 * @returns {string|null} - Section name or null if not found
 */
function getSectionFromNavItem(navItem) {
    const i18nKey = navItem.querySelector('span[data-i18n]')?.getAttribute('data-i18n');
    return i18nKey ? i18nKey.replace('nav.', '') : null;
}

// Mapping section name to associated switch functions
/** @type {Record<String, Function>} */
export const SECTIONS_MAPPER = {
    files: switchToFilesSection,
    shared: switchToSharedSection,
    sharedwithme: switchToSharedWithMeSection,
    recent: switchToRecentFilesSection,
    favorites: switchToFavoritesSection,
    trash: switchToTrashSection,
    photos: switchToPhotosSection,
    music: switchToMusicSection
};

/**
 * Set the current active section, updating all view flags and nav UI.
 * @param {string} section - The section to activate ('files', 'shared', 'recent', 'favorites', 'trash')
 * @returns {boolean} true if the section changed
 */
function setCurrentSection(section) {
    if (app.currentSection === section) return false;
    app.currentSection = section;

    // Update nav item active classes by finding matching item from DOM
    appElements.navItems?.forEach((item) => {
        const itemSection = getSectionFromNavItem(item);
        const isActive = itemSection === section;
        item.classList.toggle('active', isActive);
        if (isActive) {
            item.setAttribute('aria-current', 'page');
        } else {
            item.removeAttribute('aria-current');
        }
    });

    // Update page title
    const titleKey = `nav.${section}`;
    // TODO check why no more used: const defaultTitle = section.charAt(0).toUpperCase() + section.slice(1);
    if (appElements.pageTitle) {
        appElements.pageTitle.textContent = i18n.t(titleKey);
        appElements.pageTitle.setAttribute('data-i18n', titleKey);
    }

    // Hide mySharesView when switching to any other section
    if (section !== 'shared') {
        mySharesView.hide();
    }

    // Hide "Load more" button when leaving the sharedwithme section
    if (section !== 'sharedwithme' && sharedWithMeView) {
        sharedWithMeView.hide();
    }

    // Hide favoritesView "Load more" button when leaving the favorites section
    if (section !== 'favorites' && favoritesView) {
        favoritesView.hide();
    }

    // Hide recentView "Load more" button when leaving the recent section
    if (section !== 'recent' && recentView) {
        recentView.hide();
    }

    // Hide trashView "Load more" button when leaving the trash section
    if (section !== 'trash' && trashView) {
        trashView.hide();
    }

    // Reset owner column — sections that need it re-enable it explicitly below.
    ui.setOwnerColumnVisible(false);

    // Hide photosView when switching to any other section
    if (section !== 'photos' && photosView) {
        photosView.hide();
    }

    // Hide musicView when switching to any other section
    if (section !== 'music' && musicView) {
        musicView.hide();
    }

    return true;
}

function switchToSharedSection() {
    if (!setCurrentSection('shared')) return;

    // Hide breadcrumb (only shown in Files view)
    const breadcrumb = document.querySelector('.breadcrumb');
    breadcrumb?.classList.add('hidden');

    // Show actions-bar with group-by controls only (no grid/list toggle —
    // MySharesList is always in list mode).
    setActionsBarMode('shared');

    // Populate the group-by dropdown with this section's dimensions.
    setGroupByView(mySharesView);
    syncGroupByMenu(mySharesView.groupByDefs);

    // Restore the saved group-by selection in the dropdown.
    const msPrefs = viewPrefs.load('shared');
    applyGroupByMenuState(msPrefs.groupBy, msPrefs.reversed);

    // Show the files container always in list view — grid is not applicable here.
    toggleFileContainer(true);
    app.currentView = 'list';
    syncViewContainers();

    if (batchToolbar) batchToolbar.clear();

    // Load and render items into the files container
    mySharesView.init();
}

function switchToSharedWithMeSection() {
    if (!setCurrentSection('sharedwithme')) return;

    // Hide breadcrumb (only shown in Files view)
    const breadcrumb = document.querySelector('.breadcrumb');
    breadcrumb?.classList.add('hidden');

    // Show actions-bar with view toggle (no upload / new-folder in this view)
    setActionsBarMode('sharedwithme');

    // Populate the group-by dropdown with this section's dimensions.
    // Must be called AFTER setActionsBarMode() so the selector elements exist.
    setGroupByView(sharedWithMeView);
    syncGroupByMenu(sharedWithMeView.groupByDefs);

    // Restore the saved group-by selection in the dropdown.
    const swmPrefs = viewPrefs.load('sharedwithme');
    applyGroupByMenuState(swmPrefs.groupBy, swmPrefs.reversed);

    // Show the Owner column — names are resolved async after render.
    ui.setOwnerColumnVisible(true);

    // Show the standard files container and respect grid/list preference
    toggleFileContainer(true);
    restoreView('sharedwithme');
    syncViewContainers();

    if (batchToolbar) batchToolbar.clear();

    // Load and render items into the files container
    sharedWithMeView.init();
}

/**
 * Switch the UI into the Files section.
 *
 * @param {Object}  [options]
 * @param {boolean} [options.preservePath=false]
 *   When true, keep the current `app.currentPath` instead of resetting
 *   to the home folder. Used on initial page load with a hash-driven
 *   path (e.g. `#/files/folder/<uuid>`) so the section switch doesn't
 *   clobber the path the caller has just set from the URL. Without
 *   this flag, the unconditional reset races with `loadFiles()`
 *   (which is also kicked off from here) and the home-folder URL
 *   wins, redirecting the user back to root on every other refresh.
 */
function switchToFilesSection({ preservePath = false } = {}) {
    if (!setCurrentSection('files')) return;

    // Set actions bar mode
    setActionsBarMode('files', true);
    setGroupByView(filesView);
    syncGroupByMenu(filesView.groupByDefs);

    // Restore the saved group-by selection in the dropdown.
    const filesPrefs = viewPrefs.load('files');
    applyGroupByMenuState(filesPrefs.groupBy, filesPrefs.reversed);

    // Show owner column in the Files section
    ui.setOwnerColumnVisible(true);

    // Show breadcrumb (only in Files view)
    const breadcrumb = document.querySelector('.breadcrumb');
    breadcrumb?.classList.remove('hidden');

    // show files container
    toggleFileContainer(true);

    // ensure correct view
    restoreView('files');
    syncViewContainers();

    //reset files view + remove any error
    ui.resetFilesList();

    // Reset to home folder unless the caller has pre-set a path (the
    // `preservePath` opt-in). External users have no home — leave
    // `currentPath` as the caller set it (e.g. the magic-link
    // landing's hash context) so loadFiles() doesn't fall through to
    // `/api/folders//resources`. If `currentPath` is still empty by
    // the time loadFiles() runs, it self-redirects to /#/sharedwithme.
    if (!app.isExternalUser && !preservePath) {
        app.currentPath = app.userHomeFolderId || '';
        app.breadcrumbPath = [];
    }
    ui.updateBreadcrumb();
    if (batchToolbar) batchToolbar.clear();

    loadFiles();

    // Refresh outgoing grants in the background and repaint badges once done.
    // Badges are rendered synchronously from the in-memory cache, so any staleness
    // from navigating away and back (or starting on a different section) is corrected
    // without blocking the file list render.
    grants.fetchOutgoingGrants().then(() => refreshSharedBadges());
}

function switchToFavoritesSection() {
    if (!setCurrentSection('favorites')) return;

    // Set actions bar mode
    setActionsBarMode('favorites');
    setGroupByView(favoritesView);
    syncGroupByMenu(favoritesView.groupByDefs);

    // Restore the saved group-by selection in the dropdown.
    const favPrefs = viewPrefs.load('favorites');
    applyGroupByMenuState(favPrefs.groupBy, favPrefs.reversed);

    // Show the Owner column — names are resolved async after render.
    ui.setOwnerColumnVisible(true);

    // Hide breadcrumb (only shown in Files view)
    const breadcrumb = document.querySelector('.breadcrumb');
    breadcrumb?.classList.add('hidden');

    // show files container
    toggleFileContainer(true);

    // ensure correct view
    restoreView('favorites');
    syncViewContainers();

    if (batchToolbar) batchToolbar.clear();

    // Prefetch isFavorite cache in background (non-blocking)
    favorites.init();

    // Load and render via the cursor-paginated view
    favoritesView.init();
}

function switchToRecentFilesSection() {
    if (!setCurrentSection('recent')) return;

    // Set actions bar mode with group-by support
    setActionsBarMode('recent');
    setGroupByView(recentView);
    syncGroupByMenu(recentView.groupByDefs);

    // Restore the saved group-by selection in the dropdown.
    const recentPrefs = viewPrefs.load('recent');
    applyGroupByMenuState(recentPrefs.groupBy, recentPrefs.reversed);

    // Show the Owner column
    ui.setOwnerColumnVisible(true);

    // Hide breadcrumb (only shown in Files view)
    const breadcrumb = document.querySelector('.breadcrumb');
    breadcrumb?.classList.add('hidden');

    // show files container
    toggleFileContainer(true);

    // ensure correct view
    restoreView('recent');
    syncViewContainers();

    if (batchToolbar) batchToolbar.clear();

    recentView.init();
}

function switchToPhotosSection() {
    if (!setCurrentSection('photos')) return;

    // Hide breadcrumb
    const breadcrumb = document.querySelector('.breadcrumb');
    breadcrumb?.classList.add('hidden');

    // Hide actions-bar (photos has its own upload via selection bar)
    setActionsBarMode('hidden');

    //reset files view + remove any error
    ui.resetFilesList();

    // Hide file containers
    toggleFileContainer(false);

    // Show photos view
    if (photosView) {
        photosView.show();
    }
    if (batchToolbar) batchToolbar.clear();
}

function switchToTrashSection() {
    setCurrentSection('trash');

    // Hide breadcrumb (only shown in Files view)
    const breadcrumb = document.querySelector('.breadcrumb');
    breadcrumb?.classList.add('hidden');

    // Show files containers (to be filled with trash)
    // Hide file containers
    toggleFileContainer(true);

    setActionsBarMode('trash');
    setGroupByView(trashView);
    syncGroupByMenu(trashView.groupByDefs);
    const trashPrefs = viewPrefs.load('trash');
    applyGroupByMenuState(trashPrefs.groupBy || 'remainingDays', trashPrefs.reversed);

    //reset files view + remove any error
    ui.resetFilesList();

    //ensure buttons match the current view
    restoreView('trash');
    syncViewContainers();

    // Load trash items (cursor-based view)
    trashView.init();

    if (batchToolbar) batchToolbar.clear();
}

function switchToMusicSection() {
    if (!setCurrentSection('music')) return;

    // Hide breadcrumb
    const breadcrumb = document.querySelector('.breadcrumb');
    breadcrumb?.classList.add('hidden');

    // Hide file containers
    toggleFileContainer(false);

    // Hide actions-bar
    setActionsBarMode('hidden');

    // Reset files view + remove any error
    ui.resetFilesList();

    // Hide list header (created by resetFilesList)
    const listHeader = document.querySelector('.list-header');
    listHeader?.classList.add('hidden');

    // Show music view
    if (musicView) {
        musicView.show();
    }
    if (batchToolbar) batchToolbar.clear();
}

/**
 * Activate the Files section UI (nav state, breadcrumb, actions bar,
 * files container, grid/list sync) WITHOUT resetting `app.currentPath`
 * or `app.breadcrumbPath`.
 *
 * Used by `selectFolder` when the user clicks a folder from a
 * non-files section (e.g. "Shared with me") so the Files view is
 * fully set up before the folder content loads.
 */
function activateFilesUI() {
    setCurrentSection('files');
    setActionsBarMode('files', true);
    setGroupByView(filesView);
    syncGroupByMenu(filesView.groupByDefs);
    const breadcrumb = document.querySelector('.breadcrumb');
    breadcrumb?.classList.remove('hidden');
    toggleFileContainer(true);
    syncViewContainers();
    if (batchToolbar) batchToolbar.clear();
}

export {
    activateFilesUI,
    switchToFavoritesSection,
    switchToFilesSection,
    switchToMusicSection,
    switchToPhotosSection,
    switchToRecentFilesSection,
    switchToSharedSection,
    switchToSharedWithMeSection,
    switchToTrashSection,
    syncViewContainers
};
