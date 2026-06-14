// Global command palette (Cmd / Ctrl + K).
//
// Lists navigation targets and key actions; running one *clicks the existing
// control* (nav button, menu item, file input) so the palette can never drift
// from the real behaviour. Filtering is a simple case-insensitive substring
// match; keyboard: ↑/↓ to move, Enter to run, Esc to close.
import { i18n } from '../core/i18n.js';

/** @typedef {{ label: string, icon: string, run: () => void }} Command */

const state = {
    /** @type {HTMLElement | null} */ overlay: null,
    /** @type {HTMLInputElement | null} */ input: null,
    /** @type {HTMLElement | null} */ list: null,
    /** @type {Command[]} */ items: [],
    /** @type {Command[]} */ filtered: [],
    index: 0,
    /** @type {HTMLElement | null} */ prevFocus: null
};

/** Click an element by selector (no-op if absent). */
function click(selector) {
    /** @type {HTMLElement | null} */ (document.querySelector(selector))?.click();
}

/** Activate the sidebar nav button whose label key matches `nav.<section>`. */
function navTo(section) {
    document.querySelectorAll('.nav-item').forEach((it) => {
        const key = it.querySelector('span[data-i18n]')?.getAttribute('data-i18n');
        if (key === `nav.${section}`) /** @type {HTMLElement} */ (it).click();
    });
}

/** @returns {Command[]} */
function buildCommands() {
    /** @type {Command[]} */
    const cmds = [
        { label: i18n.t('nav.files'), icon: 'fa-folder', run: () => navTo('files') },
        { label: i18n.t('nav.shared'), icon: 'fa-oxiexport', run: () => navTo('shared') },
        { label: i18n.t('nav.sharedwithme'), icon: 'fa-oxiimport', run: () => navTo('sharedwithme') },
        { label: i18n.t('nav.recent'), icon: 'fa-clock', run: () => navTo('recent') },
        { label: i18n.t('nav.favorites'), icon: 'fa-star', run: () => navTo('favorites') },
        { label: i18n.t('nav.photos'), icon: 'fa-images', run: () => navTo('photos') },
        { label: i18n.t('nav.music'), icon: 'fa-music', run: () => navTo('music') },
        { label: i18n.t('nav.trash'), icon: 'fa-trash', run: () => navTo('trash') },
        { label: i18n.t('actions.upload_files'), icon: 'fa-cloud-upload-alt', run: () => click('#file-input') },
        { label: i18n.t('user_menu.profile'), icon: 'fa-user-circle', run: () => click('#user-menu-profile') },
        { label: i18n.t('user_menu.about'), icon: 'fa-info-circle', run: () => click('#user-menu-about') },
        { label: i18n.t('actions.logout'), icon: 'fa-sign-out-alt', run: () => click('#user-menu-logout') }
    ];
    const adminBtn = document.getElementById('user-menu-admin');
    if (adminBtn && !adminBtn.classList.contains('hidden')) {
        cmds.splice(8, 0, {
            label: i18n.t('user_menu.admin_panel'),
            icon: 'fa-cogs',
            run: () => click('#user-menu-admin')
        });
    }
    return cmds;
}

function render() {
    if (!state.list) return;
    if (!state.filtered.length) {
        state.list.innerHTML = `<li class="cmdk-empty">${i18n.t('cmdk.no_results', 'No matching commands')}</li>`;
        return;
    }
    state.list.innerHTML = state.filtered
        .map(
            (cmd, i) => `
        <li class="cmdk-item${i === state.index ? ' is-active' : ''}" role="option" aria-selected="${i === state.index}" data-i="${i}">
            <i class="fas ${cmd.icon}" aria-hidden="true"></i>
            <span>${cmd.label}</span>
        </li>`
        )
        .join('');
}

function filter(query) {
    const q = query.trim().toLowerCase();
    state.filtered = q ? state.items.filter((c) => c.label.toLowerCase().includes(q)) : state.items.slice();
    state.index = 0;
    render();
}

function move(delta) {
    if (!state.filtered.length) return;
    state.index = (state.index + delta + state.filtered.length) % state.filtered.length;
    render();
    state.list?.querySelector('.is-active')?.scrollIntoView({ block: 'nearest' });
}

function runActive() {
    const cmd = state.filtered[state.index];
    close();
    cmd?.run();
}

/** @param {KeyboardEvent} e */
function onKeydown(e) {
    if (e.key === 'Escape') {
        e.preventDefault();
        close();
    } else if (e.key === 'ArrowDown') {
        e.preventDefault();
        move(1);
    } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        move(-1);
    } else if (e.key === 'Enter') {
        e.preventDefault();
        runActive();
    }
}

function ensureOverlay() {
    if (state.overlay) return;
    const overlay = document.createElement('div');
    overlay.className = 'cmdk-overlay hidden';
    overlay.setAttribute('role', 'dialog');
    overlay.setAttribute('aria-modal', 'true');
    overlay.setAttribute('aria-label', i18n.t('cmdk.title', 'Command palette'));
    overlay.innerHTML = `
        <div class="cmdk-panel">
            <div class="cmdk-search">
                <i class="fas fa-search" aria-hidden="true"></i>
                <input type="text" class="cmdk-input" autocomplete="off" spellcheck="false" aria-label="${i18n.t('cmdk.title', 'Command palette')}">
            </div>
            <ul class="cmdk-list" role="listbox"></ul>
        </div>`;
    document.body.appendChild(overlay);
    state.overlay = overlay;
    state.input = /** @type {HTMLInputElement} */ (overlay.querySelector('.cmdk-input'));
    state.list = /** @type {HTMLElement} */ (overlay.querySelector('.cmdk-list'));
    state.input.placeholder = i18n.t('cmdk.placeholder', 'Type a command…');

    overlay.addEventListener('click', (e) => {
        if (e.target === overlay) close();
    });
    overlay.addEventListener('keydown', onKeydown);
    state.input.addEventListener('input', () => filter(state.input?.value ?? ''));
    state.list.addEventListener('click', (e) => {
        const li = /** @type {HTMLElement} */ (e.target).closest('.cmdk-item');
        if (li instanceof HTMLElement) {
            state.index = Number(li.dataset.i);
            runActive();
        }
    });
}

function open() {
    ensureOverlay();
    state.prevFocus = /** @type {HTMLElement | null} */ (document.activeElement);
    state.items = buildCommands();
    if (state.input) state.input.value = '';
    filter('');
    state.overlay?.classList.remove('hidden');
    requestAnimationFrame(() => state.overlay?.classList.add('active'));
    state.input?.focus();
}

function close() {
    if (!state.overlay) return;
    state.overlay.classList.remove('active');
    state.overlay.classList.add('hidden');
    state.prevFocus?.focus?.();
    state.prevFocus = null;
}

// Global shortcut: Cmd/Ctrl + K toggles the palette.
document.addEventListener('keydown', (e) => {
    if ((e.metaKey || e.ctrlKey) && (e.key === 'k' || e.key === 'K')) {
        e.preventDefault();
        if (state.overlay && !state.overlay.classList.contains('hidden')) close();
        else open();
    }
});
