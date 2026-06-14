/**
 * User menu, profile modal and logout logic
 */

import { createUserVignette, disposeVignette } from '../components/userVignette.js';
import { getCsrfHeaders } from '../core/csrf.js';
import { formatFileSize, formatQuotaSize } from '../core/formatters.js';
import { i18n } from '../core/i18n.js';
import { groupsView } from '../views/groups/groupsView.js';

function setupUserMenu() {
    const wrapper = document.getElementById('user-menu-wrapper');
    const avatarBtn = document.getElementById('user-avatar-btn');
    const menu = document.getElementById('user-menu');
    const logoutBtn = document.getElementById('user-menu-logout');
    const themeSegmented = document.getElementById('user-menu-theme-segmented');
    const aboutBtn = document.getElementById('user-menu-about');
    const adminBtn = document.getElementById('user-menu-admin');
    const groupsBtn = document.getElementById('user-menu-groups');
    const adminDivider = document.getElementById('user-menu-admin-divider');
    const profileBtn = document.getElementById('user-menu-profile');
    const roleBadge = document.getElementById('user-menu-role-badge');

    if (!wrapper || !avatarBtn || !menu) return;

    // Populate avatar and name immediately from localStorage on every page load.
    updateUserMenuData();

    avatarBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        const isOpen = wrapper.classList.contains('open');
        wrapper.classList.toggle('open');

        const notifWrapper = document.getElementById('notif-wrapper');
        const notifBtn = document.getElementById('notif-bell-btn');
        if (notifWrapper) notifWrapper.classList.remove('open');
        if (notifBtn) notifBtn.classList.remove('active');

        if (!isOpen) {
            updateUserMenuData();
            const USER_DATA_KEY = 'oxicloud_user';
            const userData = JSON.parse(localStorage.getItem(USER_DATA_KEY) || '{}');
            const isAdmin = userData.role === 'admin';
            if (adminBtn) {
                isAdmin ? adminBtn.classList.remove('hidden') : adminBtn.classList.add('hidden');
            }
            if (groupsBtn) {
                // v1: admin-only. v2 will broaden to "has any manageable
                // group" — change the right-hand side here without touching
                // anything else.
                isAdmin ? groupsBtn.classList.remove('hidden') : groupsBtn.classList.add('hidden');
            }
            if (adminDivider) {
                isAdmin ? adminDivider.classList.remove('hidden') : adminDivider.classList.add('hidden');
            }
            if (roleBadge) {
                isAdmin ? roleBadge.classList.remove('hidden') : roleBadge.classList.add('hidden');
            }
        }
    });

    document.addEventListener('click', (e) => {
        if (wrapper.classList.contains('open') && !wrapper.contains(/** @type {Node|null} */ (e.target))) {
            wrapper.classList.remove('open');
        }
    });

    if (logoutBtn) {
        logoutBtn.addEventListener('click', () => {
            wrapper.classList.remove('open');
            logout();
        });
    }

    if (themeSegmented) {
        /**
         * Read the current selection. Returns `'auto'` when no explicit choice
         * is stored — that's the default and means "follow OS preference".
         * @returns {'light' | 'dark' | 'auto'}
         */
        function currentMode() {
            const saved = localStorage.getItem('oxicloud_theme');
            return saved === 'light' || saved === 'dark' ? saved : 'auto';
        }

        /**
         * Apply a mode: persist it, stamp the html attribute (or remove it
         * for 'auto' so CSS `color-scheme: light dark` resolves via OS), and
         * highlight the matching button.
         * @param {'light' | 'dark' | 'auto'} mode
         */
        function applyMode(mode) {
            if (mode === 'auto') {
                localStorage.setItem('oxicloud_theme', 'auto');
                document.documentElement.removeAttribute('data-color-scheme');
            } else {
                localStorage.setItem('oxicloud_theme', mode);
                document.documentElement.setAttribute('data-color-scheme', mode);
            }
            themeSegmented.querySelectorAll('.theme-segmented__opt').forEach((b) => {
                const el = /** @type {HTMLElement} */ (b);
                const isActive = el.dataset.mode === mode;
                el.classList.toggle('theme-segmented__opt--active', isActive);
                el.setAttribute('aria-checked', String(isActive));
            });
        }

        // Initial highlight (theme-init.js already stamped the attribute).
        applyMode(currentMode());

        themeSegmented.addEventListener('click', (e) => {
            const btn = /** @type {HTMLElement | null} */ (/** @type {HTMLElement} */ (e.target).closest('.theme-segmented__opt'));
            if (!btn) return;
            e.stopPropagation();
            const mode = /** @type {'light' | 'dark' | 'auto'} */ (btn.dataset.mode);
            if (mode !== 'light' && mode !== 'dark' && mode !== 'auto') return;
            applyMode(mode);
        });
    }

    if (adminBtn) {
        adminBtn.addEventListener('click', () => {
            wrapper.classList.remove('open');
            window.location.href = '/admin';
        });
    }

    if (groupsBtn) {
        groupsBtn.addEventListener('click', () => {
            wrapper.classList.remove('open');
            groupsView.open();
        });
    }

    if (profileBtn) {
        profileBtn.addEventListener('click', () => {
            wrapper.classList.remove('open');
            window.location.href = '/profile';
        });
    }

    if (aboutBtn) {
        aboutBtn.addEventListener('click', () => {
            wrapper.classList.remove('open');
            const overlay = document.getElementById('about-modal-overlay');
            if (overlay) overlay.classList.remove('hidden');
        });
    }

    const aboutCloseBtn = document.getElementById('about-close-btn');
    const aboutOverlay = document.getElementById('about-modal-overlay');
    if (aboutCloseBtn) {
        aboutCloseBtn.addEventListener('click', () => {
            aboutOverlay?.classList.add('hidden');
        });
    }
    if (aboutOverlay) {
        aboutOverlay.addEventListener('click', (e) => {
            if (e.target === aboutOverlay) {
                aboutOverlay.classList.add('hidden');
            }
        });
        document.addEventListener('keydown', (e) => {
            if (e.key === 'Escape' && !aboutOverlay.classList.contains('hidden')) {
                aboutOverlay.classList.add('hidden');
            }
        });
    }

    fetchAppVersion();
}

/**
 * Mount avatar-only vignettes for the toolbar button and the dropdown header.
 * Called whenever user data in localStorage changes (login, photo save, etc.).
 *
 * The toolbar button (#user-avatar-btn) and the menu header (.user-menu-header)
 * are the stable mount points.  Both receive a fresh vignette each call so
 * the photo / initials are always in sync with the current localStorage state.
 *
 * @param {string} userId
 */
function _mountAvatarVignettes(userId) {
    const avatarBtn = document.getElementById('user-avatar-btn');
    if (avatarBtn) {
        // Dispose the outgoing vignette's tooltip before replacing it, or its
        // body-portalled popover orphans (the "email tooltip stays stuck" bug).
        disposeVignette(/** @type {HTMLElement | null} */ (avatarBtn.firstElementChild));
        // No hover-tooltip on your own avatar — the email is already in the open
        // menu header, so it'd be redundant and overlap the notification bell.
        avatarBtn.replaceChildren(createUserVignette(userId, 'menu', { showName: false, noTooltip: true }));
    }

    const menuHeader = document.querySelector('.user-menu-header');
    if (menuHeader) {
        disposeVignette(/** @type {HTMLElement | null} */ (menuHeader.firstElementChild));
        menuHeader.replaceChildren(createUserVignette(userId, 'xl', { showName: true, showEmail: true }));
    }
}

/**
 * @returns {void}
 */
function updateUserMenuData() {
    const USER_DATA_KEY = 'oxicloud_user';
    /** @type {import('../core/types.js').User} */
    const userData = JSON.parse(localStorage.getItem(USER_DATA_KEY) || '{}');

    const storageFill = document.getElementById('user-menu-storage-fill');
    const storageText = document.getElementById('user-menu-storage-text');

    // Username is optional (PR 16) — gate on `id` only; the avatar
    // mount needs the UUID, not the handle.
    if (userData.id) {
        _mountAvatarVignettes(userData.id);
    }

    const usedBytes = userData.storage_used_bytes || 0;
    const quotaBytes = userData.storage_quota_bytes == null ? 10 * 1024 * 1024 * 1024 : userData.storage_quota_bytes;
    const percentage = quotaBytes > 0 ? Math.min(Math.round((usedBytes / quotaBytes) * 100), 100) : 0;

    if (storageFill) storageFill.style.width = `${percentage}%`;
    if (storageText) {
        const used = formatFileSize(usedBytes);
        const total = formatQuotaSize(quotaBytes);
        storageText.textContent = `${quotaBytes > 0 ? `${percentage}% · ` : ''}${used} / ${total}`;
    }
}

async function fetchAppVersion() {
    try {
        const response = await fetch('/api/version');
        if (response.ok) {
            const data = await response.json();
            const versionEl = document.getElementById('about-version');
            if (versionEl && data.version) {
                versionEl.textContent = `v${data.version}`;
            }
        }
    } catch (err) {
        console.warn('Could not fetch app version:', err);
    }
}

function showUserProfileModal() {
    const USER_DATA_KEY = 'oxicloud_user';
    const userData = JSON.parse(localStorage.getItem(USER_DATA_KEY) || '{}');
    // Username is optional (PR 16); fall back to email so usernameless
    // recipients (magic-link sign-ins) still get a recognizable label.
    const email = userData.email || '';
    const username = userData.username || email || 'User';
    const role = userData.role || 'user';
    const initials = username.substring(0, 2).toUpperCase();
    const usedBytes = userData.storage_used_bytes || 0;
    const quotaBytes = userData.storage_quota_bytes == null ? 10 * 1024 * 1024 * 1024 : userData.storage_quota_bytes;
    const percentage = quotaBytes > 0 ? Math.min(Math.round((usedBytes / quotaBytes) * 100), 100) : 0;
    // FIXME: use classes
    const barColor = percentage > 90 ? '#ef4444' : percentage > 70 ? '#f59e0b' : '#22c55e';

    const existing = document.getElementById('profile-modal-overlay');
    if (existing) existing.remove();

    const overlay = document.createElement('div');
    overlay.id = 'profile-modal-overlay';
    overlay.classList.add('about-modal-overlay', 'hidden');
    overlay.innerHTML = `
        <div class="about-modal about-modal-body">
            <div class="about-modal-header">
                <div class="about-modal-avatar">${initials}</div>
                <h3 class="about-modal-username">${username}</h3>
                <p class="about-modal-email">${email}</p>
                <span class="about-modal-role ${role === 'admin' ? 'about-modal-role-admin' : 'about-modal-role-user'}">${role === 'admin' ? '🛡️ Admin' : `👤 ${i18n.t('user_menu.role_user')}`}</span>
            </div>
            <div class="about-modal-storage">
                <div class="about-modal-storage-label">
                    <i class="fas fa-database"></i>${i18n.t('storage.title')}
                </div>
                <div class="about-modal-bar-bg">
                    <div class="about-modal-bar-fill" id="about-bar-fill"></div>
                </div>
                <div class="about-modal-bar-text">${percentage}% · ${formatFileSize(usedBytes)} / ${formatQuotaSize(quotaBytes)}</div>
            </div>
            <div class="about-modal-footer">
                <button id="profile-modal-close" class="about-modal-close-btn">${i18n.t('actions.close')}</button>
            </div>
        </div>
    `;

    // Set dynamic bar width and color via JS property (CSP-safe)
    const barFill = /** @type {HTMLDivElement} */ (overlay.querySelector('#about-bar-fill'));
    if (barFill) {
        barFill.style.width = `${percentage}%`;
        barFill.style.background = barColor;
    }

    document.body.appendChild(overlay);
    requestAnimationFrame(() => overlay.classList.add('show'));

    overlay.querySelector('#profile-modal-close')?.addEventListener('click', () => {
        overlay.classList.remove('show');
        setTimeout(() => overlay.remove(), 200);
    });
    overlay.addEventListener('click', (e) => {
        if (e.target === overlay) {
            overlay.classList.remove('show');
            setTimeout(() => overlay.remove(), 200);
        }
    });
}

async function logout() {
    const USER_DATA_KEY = 'oxicloud_user';

    // Clear local state first to prevent login page from auto-refreshing
    localStorage.removeItem(USER_DATA_KEY);
    localStorage.removeItem('refresh_attempts');
    sessionStorage.removeItem('redirect_count');

    // Tell the server to clear HttpOnly cookies (await to ensure cookies are
    // cleared before redirecting, otherwise the login page's session probe
    // will refresh the token and redirect back to the app).
    try {
        await fetch('/api/auth/logout', {
            method: 'POST',
            credentials: 'same-origin',
            headers: getCsrfHeaders()
        });
    } catch (_) {
        // Best-effort
    }

    window.location.href = '/login';
}

export { logout, setupUserMenu, showUserProfileModal, updateUserMenuData };
