import { getCsrfHeaders } from '../../core/csrf.js';
import { installFetchInterceptor } from '../../core/fetchWrapper.js';
import { i18n } from '../../core/i18n.js';
import { formatRelativeTime } from '../../core/formatters.js';
import { oxiIconsInit } from '../../core/icons.js';
import { resizeImageToDataUrl } from '../../utils/imageResize.js';

// Install the fetch interceptor so expired access tokens are refreshed
// automatically on this standalone page (it is not loaded by main.js here).
installFetchInterceptor();

const API = '/api';

// TOOD: reuse common library
/**
 * @returns {Record<string, string>}
 */
function headers() {
    return { 'Content-Type': 'application/json', ...getCsrfHeaders() };
}

// TOOD: move to common library
/** @param {number} bytes */
function formatBytes(bytes) {
    if (bytes === 0) return '0 B';
    const k = 1024,
        sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return `${parseFloat((bytes / k ** i).toFixed(1))} ${sizes[i]}`;
}

// TOOD: move to common library
/** @param {string | null | undefined} dateStr */
function timeAgo(dateStr) {
    if (!dateStr) return i18n.t('profile.never');
    return formatRelativeTime(dateStr);
}

// ── Avatar helpers ─────────────────────────────────────────────────────────────

/**
 * Render the large profile avatar (#p-avatar) — photo or initials.
 * @param {string | null | undefined} photo
 * @param {string} initials
 */
function _renderAvatar(photo, initials) {
    const avatarEl = document.getElementById('p-avatar');
    if (!avatarEl) return;
    if (photo) {
        const img = document.createElement('img');
        img.alt = initials;
        img.src = photo;
        img.onerror = () => {
            avatarEl.replaceChildren();
            avatarEl.textContent = initials;
        };
        avatarEl.replaceChildren(img);
    } else {
        avatarEl.replaceChildren();
        avatarEl.textContent = initials;
    }
}

/**
 * Persist user data to localStorage and refresh the top-right avatar.
 * Calls GET /api/auth/me to get the fresh user object.
 * @returns {Promise<void>}
 */
async function _refreshUserCache() {
    try {
        const resp = await fetch(`${API}/auth/me`, {
            headers: headers(),
            credentials: 'same-origin'
        });
        if (!resp.ok) return;
        const user = await resp.json();
        localStorage.setItem('oxicloud_user', JSON.stringify(user));

        // Refresh top-right avatars if userMenu module is loaded on this page
        // (profile.html is a standalone page, userMenu is only in index.html)
        // — so we update #user-avatar / #user-menu-avatar directly if present
        const initials = (user.username || user.email || '?').substring(0, 2).toUpperCase();
        const topEl = /** @type {HTMLElement|null} */ (document.getElementById('user-avatar'));
        const dropEl = /** @type {HTMLElement|null} */ (document.getElementById('user-menu-avatar'));
        if (topEl || dropEl) {
            /** @param {HTMLElement|null} el */
            function applyPhoto(el) {
                if (!el) return;
                if (user.image) {
                    const img = document.createElement('img');
                    img.alt = initials;
                    img.src = user.image;
                    img.onerror = () => {
                        el.replaceChildren();
                        el.textContent = initials;
                    };
                    el.replaceChildren(img);
                } else {
                    el.replaceChildren();
                    el.textContent = initials;
                }
            }
            applyPhoto(topEl);
            applyPhoto(dropEl);
        }
    } catch (_) {
        // Best-effort
    }
}

// ── Photo edit panel ────────────────────────────────────────────────────────────

/** @type {string|null} Pending data URI from file upload (upload mode) */
let _uploadedDataUri = null;

/**
 * Switch the visible edit tab.
 * @param {'url'|'upload'} tab
 */
function _switchTab(tab) {
    const urlPane = document.getElementById('p-pane-url');
    const uploadPane = document.getElementById('p-pane-upload');
    const urlBtn = document.getElementById('p-tab-url');
    const uploadBtn = document.getElementById('p-tab-upload');
    if (tab === 'url') {
        urlPane?.classList.remove('hidden');
        uploadPane?.classList.add('hidden');
        urlBtn?.classList.add('active');
        uploadBtn?.classList.remove('active');
    } else {
        urlPane?.classList.add('hidden');
        uploadPane?.classList.remove('hidden');
        urlBtn?.classList.remove('active');
        uploadBtn?.classList.add('active');
    }
}

function _openEditPanel() {
    document.getElementById('p-avatar-edit-panel')?.classList.remove('hidden');
    _switchTab('url');
    _uploadedDataUri = null;
    const preview = /** @type {HTMLImageElement|null} */ (document.getElementById('p-image-preview'));
    if (preview) {
        preview.src = '';
        preview.classList.add('hidden');
    }
    const urlInput = /** @type {HTMLInputElement|null} */ (document.getElementById('p-image-url'));
    if (urlInput) urlInput.value = '';
    const status = document.getElementById('p-avatar-status');
    if (status) status.innerHTML = '';
}

function _closeEditPanel() {
    document.getElementById('p-avatar-edit-panel')?.classList.add('hidden');
    _uploadedDataUri = null;
}

/**
 * Send PUT /api/auth/me/image and update UI on success.
 * @param {string | null} image
 */
async function _saveImage(image) {
    const statusEl = document.getElementById('p-avatar-status');
    const saveBtn = /** @type {HTMLButtonElement|null} */ (document.getElementById('p-avatar-save'));
    if (saveBtn) {
        saveBtn.disabled = true;
        saveBtn.innerHTML = `<i class="fas fa-spinner fa-spin"></i>`;
    }
    if (statusEl) statusEl.innerHTML = '';

    try {
        const resp = await fetch(`${API}/auth/me/image`, {
            method: 'PUT',
            headers: headers(),
            credentials: 'same-origin',
            body: JSON.stringify({ image })
        });

        if (resp.ok) {
            await _refreshUserCache();
            // Update large avatar immediately
            const raw = localStorage.getItem('oxicloud_user');
            const user = raw ? JSON.parse(raw) : null;
            const initials = (user?.username || user?.email || '?').substring(0, 2).toUpperCase();
            _renderAvatar(user?.image, initials);
            _closeEditPanel();
        } else {
            const err = await resp.json().catch(() => ({}));
            if (statusEl) {
                statusEl.innerHTML =
                    '<div class="alert alert-error"><i class="fas fa-exclamation-circle"></i> ' +
                    escapeHtml(err.message || err.error || i18n.t('profile.photo_save_failed')) +
                    '</div>';
            }
        }
    } catch (err) {
        if (statusEl) {
            statusEl.innerHTML =
                '<div class="alert alert-error"><i class="fas fa-exclamation-circle"></i> ' +
                escapeHtml(i18n.t('profile.error_network', { message: /** @type {Error} */ (err).message })) +
                '</div>';
        }
    } finally {
        if (saveBtn) {
            saveBtn.disabled = false;
            saveBtn.innerHTML = `<i class="fas fa-save"></i> ${escapeHtml(i18n.t('profile.photo_save'))}`;
        }
    }
}

function _setupPhotoEdit() {
    const editBtn = document.getElementById('p-avatar-edit-btn');
    const cancelBtn = document.getElementById('p-avatar-cancel');
    const saveBtn = document.getElementById('p-avatar-save');
    const removeBtn = document.getElementById('p-avatar-remove');
    const tabUrl = document.getElementById('p-tab-url');
    const tabUpload = document.getElementById('p-tab-upload');
    const fileInput = /** @type {HTMLInputElement|null} */ (document.getElementById('p-image-file'));

    editBtn?.addEventListener('click', _openEditPanel);
    cancelBtn?.addEventListener('click', _closeEditPanel);

    tabUrl?.addEventListener('click', () => {
        _switchTab('url');
    });
    tabUpload?.addEventListener('click', () => {
        _switchTab('upload');
    });

    saveBtn?.addEventListener('click', async () => {
        const activePane = document.getElementById('p-pane-url')?.classList.contains('hidden') ? 'upload' : 'url';
        if (activePane === 'url') {
            const urlInput = /** @type {HTMLInputElement|null} */ (document.getElementById('p-image-url'));
            const val = urlInput?.value.trim() || null;
            await _saveImage(val || null);
        } else {
            if (!_uploadedDataUri) {
                const status = document.getElementById('p-avatar-status');
                if (status)
                    status.innerHTML = `<div class="alert alert-error"><i class="fas fa-exclamation-circle"></i> ${escapeHtml(i18n.t('profile.photo_no_file'))}</div>`;
                return;
            }
            await _saveImage(_uploadedDataUri);
        }
    });

    removeBtn?.addEventListener('click', async () => {
        await _saveImage(null);
    });

    fileInput?.addEventListener('change', async () => {
        const file = fileInput.files?.[0];
        if (!file) return;
        const status = document.getElementById('p-avatar-status');
        if (status) status.innerHTML = '';
        try {
            const dataUri = await resizeImageToDataUrl(file, 104);
            _uploadedDataUri = dataUri;
            const preview = /** @type {HTMLImageElement|null} */ (document.getElementById('p-image-preview'));
            if (preview) {
                preview.src = dataUri;
                preview.classList.remove('hidden');
            }
        } catch (err) {
            _uploadedDataUri = null;
            if (status) {
                status.innerHTML = `<div class="alert alert-error"><i class="fas fa-exclamation-circle"></i> ${escapeHtml(/** @type {Error} */ (err).message)}</div>`;
            }
        }
    });
}

async function init() {
    try {
        oxiIconsInit();
        const resp = await fetch(`${API}/auth/me`, {
            headers: headers(),
            credentials: 'same-origin'
        });
        if (!resp.ok) {
            showError();
            return;
        }
        const user = await resp.json();

        const initials = (user.username || user.email || '?').substring(0, 2).toUpperCase();
        _renderAvatar(user.image, initials);
        document.getElementById('p-username').textContent = user.username || user.email || '—';
        document.getElementById('p-email').textContent = user.email || '';

        const badge = document.getElementById('p-role-badge');
        if (user.role === 'admin') {
            badge.className = 'role-badge role-badge-admin';
            badge.innerHTML = `<i class="fas fa-shield-alt"></i> ${i18n.t('profile.role_admin')}`;
        } else {
            badge.className = 'role-badge role-badge-user';
            badge.innerHTML = `<i class="fas fa-user"></i> ${i18n.t('profile.role_user')}`;
        }

        // Photo edit controls
        const isLocal = !user.auth_provider || user.auth_provider === 'local';
        const editBtn = document.getElementById('p-avatar-edit-btn');
        const oidcNote = document.getElementById('p-avatar-oidc-note');
        if (user.can_edit_image && isLocal) {
            editBtn?.classList.remove('hidden');
        } else if (!isLocal && user.image) {
            // OIDC user with a photo: show note, no edit button
            oidcNote?.classList.remove('hidden');
        }

        document.getElementById('p-detail-username').textContent = user.username || user.email || '—';
        document.getElementById('p-detail-email').textContent = user.email || '—';
        document.getElementById('p-detail-role').textContent = user.role === 'admin' ? i18n.t('profile.role_admin') : i18n.t('profile.role_user');
        document.getElementById('p-detail-login').textContent = timeAgo(user.last_login_at);

        const used = user.storage_used_bytes || 0;
        const quota = user.storage_quota_bytes || 0;
        const pct = quota > 0 ? Math.min(Math.round((used / quota) * 100), 100) : 0;

        document.getElementById('p-storage-used').textContent = formatBytes(used);
        document.getElementById('p-storage-quota').textContent = quota > 0 ? formatBytes(quota) : '∞';
        document.getElementById('p-storage-pct').textContent = quota > 0 ? `${pct}%` : '—';

        const bar = document.getElementById('p-storage-bar');
        bar.style.width = `${pct}%`;
        bar.className = `storage-fill ${pct > 90 ? 'red' : pct > 70 ? 'orange' : 'green'}`;
        document.getElementById('p-storage-text').textContent = `${formatBytes(used)} / ${quota > 0 ? formatBytes(quota) : i18n.t('profile.unlimited')}`;

        if (user.auth_provider && user.auth_provider !== 'local') {
            document.getElementById('password-section').classList.add('hidden');
        }

        _renderProfileEdit(user);

        loadAppPasswords();

        try {
            const oidcResp = await fetch(`${API}/auth/oidc/providers`, {
                credentials: 'same-origin'
            });
            if (oidcResp.ok) {
                const oidcInfo = await oidcResp.json();
                if (!oidcInfo.password_login_enabled) {
                    document.getElementById('password-section').classList.add('hidden');
                }
            }
        } catch (_oidcErr) {}

        document.getElementById('loading').classList.add('hidden');
        document.getElementById('main-content').classList.remove('hidden');
    } catch (e) {
        console.error(e);
        showError();
    }
}

function showError() {
    document.getElementById('loading').classList.add('hidden');
    document.getElementById('auth-error').classList.remove('hidden');
}

/** @param {Event} e */
async function changePassword(e) {
    e.preventDefault();
    const currentPw = /** @type {HTMLInputElement} */ (document.getElementById('current-password')).value;
    const newPw = /** @type {HTMLInputElement} */ (document.getElementById('new-password')).value;
    const confirmPw = /** @type {HTMLInputElement} */ (document.getElementById('confirm-password')).value;
    const statusEl = document.getElementById('pw-status');

    if (newPw !== confirmPw) {
        statusEl.innerHTML = `<div class="alert alert-error"><i class="fas fa-exclamation-circle"></i> ${escapeHtml(i18n.t('profile.passwords_no_match'))}</div>`;
        return false;
    }

    if (newPw.length < 8) {
        statusEl.innerHTML = `<div class="alert alert-error"><i class="fas fa-exclamation-circle"></i> ${escapeHtml(i18n.t('profile.password_too_short'))}</div>`;
        return false;
    }

    const btn = /** @type {HTMLButtonElement} */ (document.getElementById('pw-submit'));
    btn.disabled = true;
    btn.innerHTML = `<i class="fas fa-spinner fa-spin"></i> ${escapeHtml(i18n.t('profile.updating'))}`;

    try {
        const resp = await fetch(`${API}/auth/change-password`, {
            method: 'PUT',
            headers: headers(),
            credentials: 'same-origin',
            body: JSON.stringify({
                current_password: currentPw,
                new_password: newPw
            })
        });

        if (resp.ok) {
            statusEl.innerHTML = `<div class="alert alert-success"><i class="fas fa-check-circle"></i> ${escapeHtml(i18n.t('profile.password_updated'))}</div>`;
            /** @type {HTMLFormElement} */ (document.getElementById('password-form')).reset();
        } else {
            const err = await resp.json().catch(() => ({}));
            statusEl.innerHTML =
                '<div class="alert alert-error"><i class="fas fa-exclamation-circle"></i> ' +
                escapeHtml(err.message || i18n.t('profile.password_change_failed')) +
                '</div>';
        }
    } catch (err) {
        statusEl.innerHTML =
            '<div class="alert alert-error"><i class="fas fa-exclamation-circle"></i> ' +
            escapeHtml(i18n.t('profile.error_network', { message: /** @type {Error} */ (err).message })) +
            '</div>';
    }

    btn.disabled = false;
    btn.innerHTML = `<i class="fas fa-save"></i> ${escapeHtml(i18n.t('profile.update_password'))}`;
    return false;
}

// ── App Passwords ──

const AUTO_LABELS = ['Nextcloud', 'Nextcloud (OIDC)'];

/** @param {{label: string, active?: boolean, id: string}} pw */
function isAutoPassword(pw) {
    return AUTO_LABELS.includes(pw.label);
}

/** @param {{label: string, active?: boolean, id: string, created_at: string, last_used_at?: string}} pw */
function renderPwRow(pw) {
    const tr = document.createElement('tr');
    const label = document.createElement('td');
    label.textContent = pw.label;
    const created = document.createElement('td');
    created.textContent = new Date(pw.created_at).toLocaleDateString();
    const lastUsed = document.createElement('td');
    lastUsed.textContent = pw.last_used_at ? timeAgo(pw.last_used_at) : i18n.t('profile.never');
    const status = document.createElement('td');
    const badge = document.createElement('span');
    if (pw.active !== false) {
        badge.className = 'badge badge-active';
        badge.textContent = i18n.t('profile.active');
    } else {
        badge.className = 'badge badge-expired';
        badge.textContent = i18n.t('profile.revoked');
    }
    status.appendChild(badge);
    const actions = document.createElement('td');
    if (pw.active !== false) {
        const btn = document.createElement('button');
        btn.className = 'btn btn-danger-sm';
        btn.innerHTML = '<i class="fas fa-trash"></i>';
        btn.title = i18n.t('profile.revoke_title');
        btn.addEventListener('click', () => {
            revokeAppPassword(pw.id, pw.label);
        });
        actions.appendChild(btn);
    }
    tr.append(label, created, lastUsed, status, actions);
    return tr;
}

async function loadAppPasswords() {
    try {
        const resp = await fetch(`${API}/auth/app-passwords`, {
            headers: headers(),
            credentials: 'same-origin'
        });
        if (!resp.ok) {
            document.getElementById('app-passwords-section').classList.add('hidden');
            return;
        }
        const data = await resp.json();
        const passwords = /** @type {Array<{label: string, active?: boolean, id: string, created_at: string, last_used_at?: string}>} */ (
            data.app_passwords || data
        );
        const userPws = passwords.filter((pw) => {
            return !isAutoPassword(pw);
        });
        const autoPws = passwords.filter(isAutoPassword);

        // User-created passwords
        const tbody = document.getElementById('app-pw-tbody');
        const table = document.getElementById('app-pw-table');
        const empty = document.getElementById('app-pw-empty');
        tbody.innerHTML = '';
        if (userPws.length === 0) {
            table.classList.add('hidden');
            empty.classList.remove('hidden');
        } else {
            table.classList.remove('hidden');
            empty.classList.add('hidden');
            for (const pw of userPws) tbody.appendChild(renderPwRow(pw));
        }

        // Auto-generated (client session) passwords
        const autoSection = document.getElementById('app-pw-auto-section');
        if (autoPws.length === 0) {
            autoSection.classList.add('hidden');
        } else {
            autoSection.classList.remove('hidden');
            document.getElementById('app-pw-auto-count').textContent = String(autoPws.length);
            const autoTbody = document.getElementById('app-pw-auto-tbody');
            autoTbody.innerHTML = '';
            for (const pw of autoPws) autoTbody.appendChild(renderPwRow(pw));
        }
    } catch (e) {
        console.error('Failed to load app passwords', e);
    }
}

function toggleAutoPasswords() {
    const body = document.getElementById('app-pw-auto-body');
    const chevron = document.getElementById('app-pw-auto-chevron');
    const isHidden = body.classList.contains('hidden');
    body.classList.toggle('hidden', !isHidden);
    chevron.className = isHidden ? 'fas fa-chevron-down' : 'fas fa-chevron-right';
}

async function createAppPassword() {
    const labelInput = /** @type {HTMLInputElement} */ (document.getElementById('app-pw-label'));
    const label = labelInput.value.trim();
    const statusEl = document.getElementById('app-pw-status');
    const btn = /** @type {HTMLButtonElement} */ (document.getElementById('app-pw-generate'));

    if (!label) {
        statusEl.innerHTML = `<div class="alert alert-error"><i class="fas fa-exclamation-circle"></i> ${escapeHtml(i18n.t('profile.error_label_required'))}</div>`;
        return;
    }

    btn.disabled = true;
    btn.innerHTML = `<i class="fas fa-spinner fa-spin"></i> ${escapeHtml(i18n.t('profile.generating'))}`;
    statusEl.innerHTML = '';

    try {
        const resp = await fetch(`${API}/auth/app-passwords`, {
            method: 'POST',
            headers: headers(),
            credentials: 'same-origin',
            body: JSON.stringify({ label: label })
        });
        if (!resp.ok) {
            const err = await resp.json().catch(() => ({}));
            statusEl.innerHTML =
                '<div class="alert alert-error"><i class="fas fa-exclamation-circle"></i> ' +
                escapeHtml(err.message || i18n.t('profile.error_create_pw')) +
                '</div>';
            return;
        }
        const result = await resp.json();
        document.getElementById('app-pw-created-label').textContent = result.label;
        document.getElementById('app-pw-created-password').textContent = result.password;
        document.getElementById('app-pw-created').classList.remove('hidden');
        labelInput.value = '';
        loadAppPasswords();
    } catch (err) {
        statusEl.innerHTML = `<div class="alert alert-error"><i class="fas fa-exclamation-circle"></i> ${/** @type {Error} */ (err).message}</div>`;
    } finally {
        btn.disabled = false;
        btn.innerHTML = `<i class="fas fa-plus"></i> ${escapeHtml(i18n.t('profile.generate'))}`;
    }
}

function copyAppPassword() {
    const pw = document.getElementById('app-pw-created-password').textContent;
    navigator.clipboard.writeText(pw).then(() => {
        const btn = document.getElementById('app-pw-copy-btn');
        btn.innerHTML = '<i class="fas fa-check"></i>';
        setTimeout(() => {
            btn.innerHTML = '<i class="fas fa-copy"></i>';
        }, 1500);
    });
}

/**
 * @param {string} id
 * @param {string} label
 */
async function revokeAppPassword(id, label) {
    if (!confirm(i18n.t('profile.confirm_revoke', { label: label }))) return;
    try {
        const resp = await fetch(`${API}/auth/app-passwords/${encodeURIComponent(id)}`, {
            method: 'DELETE',
            headers: headers(),
            credentials: 'same-origin'
        });
        if (resp.ok || resp.status === 204) {
            document.getElementById('app-pw-created').classList.add('hidden');
            loadAppPasswords();
        } else {
            const err = await resp.json().catch(() => ({}));
            alert(err.message || i18n.t('profile.error_revoke'));
        }
    } catch (err) {
        alert(i18n.t('profile.error_network', { message: /** @type {Error} */ (err).message }));
    }
}

/**
 * Render the Edit Profile card based on the current user.
 *
 * For OIDC users: the entire form is hidden and a single alert tells
 * them their profile is managed at the IdP. For local users: the form
 * is populated from the current values, and the username input is
 * disabled when a handle is already claimed (PR 24's claim-once
 * policy).
 *
 * @param {import('../../core/types.js').User} user
 */
function _renderProfileEdit(user) {
    const oidcNote = document.getElementById('profile-edit-oidc-note');
    const form = document.getElementById('profile-edit-form');
    if (!oidcNote || !form) return;

    const isOidc = user.auth_provider && user.auth_provider !== 'local';
    if (isOidc) {
        oidcNote.classList.remove('hidden');
        form.classList.add('hidden');
        return;
    }

    oidcNote.classList.add('hidden');
    form.classList.remove('hidden');

    const usernameInput = /** @type {HTMLInputElement} */ (document.getElementById('profile-edit-username'));
    const usernameHint = document.getElementById('profile-edit-username-hint');
    const givenInput = /** @type {HTMLInputElement} */ (document.getElementById('profile-edit-given-name'));
    const familyInput = /** @type {HTMLInputElement} */ (document.getElementById('profile-edit-family-name'));

    if (user.username) {
        usernameInput.value = user.username;
        usernameInput.disabled = true;
        if (usernameHint) {
            usernameHint.textContent = i18n.t('profile.username_already_claimed');
        }
    } else {
        usernameInput.value = '';
        usernameInput.disabled = false;
        if (usernameHint) {
            usernameHint.textContent = i18n.t('profile.username_claim_hint');
        }
    }
    givenInput.value = user.given_name || '';
    familyInput.value = user.family_name || '';

    const notifyInput = /** @type {HTMLInputElement | null} */ (document.getElementById('profile-edit-notify-on-share'));
    if (notifyInput) {
        // notify_on_share is a boolean on the server; default TRUE for
        // pre-existing rows via the column default, so the checkbox is
        // ticked unless the user has explicitly opted out.
        notifyInput.checked = user.notify_on_share !== false;
    }
}

/**
 * Submit the profile edit form. Only sends fields the user can change:
 *   - Username only if not already claimed (input wasn't disabled).
 *   - Given/family names only when their value differs from the
 *     current (avoids 400-rejecting an empty string the user never
 *     touched).
 *
 * @param {Event} e
 */
async function submitProfile(e) {
    e.preventDefault();

    const statusEl = document.getElementById('profile-edit-status');
    const btn = /** @type {HTMLButtonElement} */ (document.getElementById('profile-edit-submit'));
    const usernameInput = /** @type {HTMLInputElement} */ (document.getElementById('profile-edit-username'));
    const givenInput = /** @type {HTMLInputElement} */ (document.getElementById('profile-edit-given-name'));
    const familyInput = /** @type {HTMLInputElement} */ (document.getElementById('profile-edit-family-name'));

    /** @type {{ username?: string, given_name?: string, family_name?: string, notify_on_share?: boolean }} */
    const body = {};
    if (!usernameInput.disabled && usernameInput.value.trim()) {
        body.username = usernameInput.value.trim();
    }
    const given = givenInput.value.trim();
    if (given) body.given_name = given;
    const family = familyInput.value.trim();
    if (family) body.family_name = family;

    // Always send the share-notification preference. The backend
    // compares against the current value and skips the write if
    // unchanged, so this is idempotent — sending it on every save
    // simplifies the frontend rather than tracking a dirty bit.
    const notifyInput = /** @type {HTMLInputElement | null} */ (document.getElementById('profile-edit-notify-on-share'));
    if (notifyInput) {
        body.notify_on_share = notifyInput.checked;
    }

    if (Object.keys(body).length === 0) {
        statusEl.innerHTML = `<div class="alert alert-info"><i class="fas fa-info-circle"></i> ${escapeHtml(i18n.t('profile.profile_no_changes'))}</div>`;
        return false;
    }

    btn.disabled = true;
    btn.innerHTML = `<i class="fas fa-spinner fa-spin"></i> ${escapeHtml(i18n.t('profile.updating'))}`;

    try {
        const resp = await fetch(`${API}/auth/me/profile`, {
            method: 'PATCH',
            headers: headers(),
            credentials: 'same-origin',
            body: JSON.stringify(body)
        });

        if (resp.ok) {
            /** @type {import('../../core/types.js').User} */
            const updated = await resp.json();
            _renderProfileEdit(updated);
            // Also refresh the read-only "Account Details" username field.
            const detailUsername = document.getElementById('p-detail-username');
            if (detailUsername) detailUsername.textContent = updated.username || '—';
            const topUsername = document.getElementById('p-username');
            if (topUsername && updated.username) topUsername.textContent = updated.username;
            statusEl.innerHTML = `<div class="alert alert-success"><i class="fas fa-check-circle"></i> ${escapeHtml(i18n.t('profile.profile_saved'))}</div>`;
        } else if (resp.status === 409) {
            const err = await resp.json().catch(() => ({}));
            // Distinguish "username taken" from "username immutable" using the
            // human-readable message — both are 409. Server audit has the
            // structured `reason` field; the JSON body just carries `message`.
            const msg = (err.message || '').toLowerCase();
            const key = msg.includes('already claimed') ? 'profile.username_immutable_error' : 'profile.username_taken_error';
            statusEl.innerHTML = `<div class="alert alert-error"><i class="fas fa-exclamation-circle"></i> ${escapeHtml(i18n.t(key))}</div>`;
        } else if (resp.status === 403) {
            statusEl.innerHTML = `<div class="alert alert-error"><i class="fas fa-exclamation-circle"></i> ${escapeHtml(i18n.t('profile.edit_oidc_managed'))}</div>`;
        } else {
            const err = await resp.json().catch(() => ({}));
            statusEl.innerHTML =
                '<div class="alert alert-error"><i class="fas fa-exclamation-circle"></i> ' +
                escapeHtml(err.message || i18n.t('profile.profile_save_failed')) +
                '</div>';
        }
    } catch (err) {
        statusEl.innerHTML =
            '<div class="alert alert-error"><i class="fas fa-exclamation-circle"></i> ' +
            escapeHtml(i18n.t('profile.error_network', { message: /** @type {Error} */ (err).message })) +
            '</div>';
    }

    btn.disabled = false;
    btn.innerHTML = `<i class="fas fa-save"></i> ${escapeHtml(i18n.t('profile.save_profile'))}`;
    return false;
}

/** @param {string} str */
function escapeHtml(str) {
    var div = document.createElement('div');
    div.textContent = str || '';
    return div.innerHTML;
}

init();

/* Wire up event handlers (replaces inline onclick/onsubmit) */
document.getElementById('password-form').addEventListener('submit', changePassword);
document.getElementById('profile-edit-form')?.addEventListener('submit', submitProfile);
document.getElementById('app-pw-generate').addEventListener('click', createAppPassword);
document.getElementById('app-pw-copy-btn').addEventListener('click', copyAppPassword);
document.getElementById('app-pw-auto-toggle').addEventListener('click', toggleAutoPasswords);

/* Photo-edit panel — wired once at module load, not per init() call */
_setupPhotoEdit();

/* Re-render when language changes */
window.addEventListener('translationsLoaded', () => {
    init();
});
window.addEventListener('localeChanged', () => {
    init();
});
