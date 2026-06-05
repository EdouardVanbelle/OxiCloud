// @ts-check

/**
 * UserVignette — reusable user avatar component, two display modes.
 *
 * Mode 1 — avatar + name (default):
 *   A coloured circle with initials (or photo) alongside an async-resolved
 *   display name.  Used in the owner column, ShareModal rows / chips / items.
 *
 * Mode 2 — avatar only ({ showName: false }):
 *   The circle alone, no name span.  Used in the user-menu toolbar button
 *   and the dropdown header where the name is rendered separately.
 *
 * Usage:
 *   import { createUserVignette } from './userVignette.js';
 *   // with name
 *   cell.replaceChildren(createUserVignette(userId, 'sm'));
 *   // avatar only
 *   btn.replaceChildren(createUserVignette(userId, 'menu', { showName: false }));
 */

import { systemUsers } from '../model/systemUsers.js';
import { attachTooltip } from '../utils/tooltip.js';

// ── Helpers ────────────────────────────────────────────────────────────────────

/**
 * Get initials for an avatar (1-2 characters).
 * @param {string} name
 * @returns {string}
 */
export function _initials(name) {
    const parts = name.trim().split(/\s+/);
    if (parts.length >= 2) return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
    return name.slice(0, 2).toUpperCase();
}

/**
 * Deterministic color index 0-4 derived from a userId string.
 * Same userId always maps to the same color across all components.
 * @param {string} userId
 * @returns {number}
 */
export function _colorIndex(userId) {
    let hash = 0;
    for (let i = 0; i < userId.length; i++) {
        hash = (hash * 31 + userId.charCodeAt(i)) | 0;
    }
    return Math.abs(hash) % 5;
}

/**
 * Render a photo inside an avatar element, falling back to initials on error.
 * @param {HTMLElement} avatar     The `.user-vignette__avatar` element.
 * @param {string}      photoUrl   Non-empty photo URL or data URI.
 * @param {string}      name       Display name for the alt attribute / fallback.
 */
function _applyPhoto(avatar, photoUrl, name) {
    const img = document.createElement('img');
    img.alt = name;
    img.src = photoUrl;
    img.onerror = () => {
        // Photo failed to load — fall back to initials
        avatar.replaceChildren();
        avatar.textContent = _initials(name);
    };
    avatar.replaceChildren(img);
}

// ── Component ──────────────────────────────────────────────────────────────────

/**
 * Available sizes.  Each maps to a `.user-vignette--{size}` CSS modifier:
 *   xs   → 20 px   (chip avatar, small inline contexts)
 *   sm   → 24 px   (default; ShareModal suggestions, compact rows)
 *   list → 36 px   (owner column in list view)
 *   md   → 32 px   (ShareModal member rows)
 *   lg   → 40 px   (profile page, larger lists)
 *   menu → 38 px   (user-menu toolbar button)
 *   xl   → 48 px   (user-menu dropdown header)
 *
 * @typedef {'xs'|'sm'|'list'|'md'|'lg'|'menu'|'xl'} VignetteSize
 */

/**
 * @typedef {Object} VignetteOptions
 * @property {boolean} [showName=true]
 *   When false, only the avatar circle is rendered — no name span.
 *   Use this when the name is displayed separately (e.g. the user-menu header).
 * @property {boolean} [showEmail=false]
 *   When true (and showName is true), the primary email address is shown below
 *   the name in a lighter style.  Name and email are wrapped in a
 *   `.user-vignette__info` column.  Has no effect when showName is false.
 * @property {boolean} [showOrigin=true]
 *   When true (the default), an `is_external` badge overlays the
 *   bottom-right of the avatar for external users only — internal
 *   users render unchanged. Set false to suppress the badge in
 *   contexts where the distinction would be noise (e.g. the
 *   logged-in-user menu, where the caller is implicitly internal).
 */

/**
 * Create a user vignette element.  Returns immediately with a placeholder;
 * the display name, email, and photo resolve asynchronously via `systemUsers`.
 *
 * @param {string}          userId   UUID of the user
 * @param {VignetteSize}    [size='sm']
 * @param {VignetteOptions} [options]
 * @returns {HTMLElement}
 */
export function createUserVignette(userId, size = 'sm', { showName = true, showEmail = false, showOrigin = true } = {}) {
    const colorIdx = _colorIndex(userId);

    const wrapper = /** @type {HTMLElement} */ (document.createElement('span'));
    wrapper.className = `user-vignette user-vignette--${size}`;

    const avatar = document.createElement('span');
    avatar.className = `user-vignette__avatar uv-color-${colorIdx}`;
    // Temporary placeholder: first two chars of UUID
    avatar.textContent = userId.slice(0, 2).toUpperCase();
    wrapper.appendChild(avatar);

    /** @type {HTMLElement | null} */
    const nameEl = showName ? document.createElement('span') : null;

    /** @type {HTMLElement | null} */
    const emailEl = showName && showEmail ? document.createElement('span') : null;

    if (nameEl) {
        nameEl.className = 'user-vignette__name';
        nameEl.textContent = `${userId.slice(0, 8)}…`;

        if (emailEl) {
            // Wrap name + email in a column so they stack vertically.
            emailEl.className = 'user-vignette__email';
            const info = document.createElement('span');
            info.className = 'user-vignette__info';
            info.appendChild(nameEl);
            info.appendChild(emailEl);
            wrapper.appendChild(info);
        } else {
            wrapper.appendChild(nameEl);
        }
    }

    // Resolve name, photo, email, and (when requested) is_external
    // asynchronously. All four go through the systemUsers cache so a
    // single fetch back-fills every facet.
    //
    // The origin badge (external-user marker) is created here only
    // when `isExternal` is true — NOT pre-created hidden — because the
    // global icon-replacement `MutationObserver` (core/icons.js) swaps
    // every `<i class="fa-…">` for an `<svg>`, invalidating any
    // reference we'd otherwise hold across the await. Late-resolve
    // calls used to toggle `.hidden` on the original `<i>` that no
    // longer existed in the DOM, leaving the badge invisible until
    // the next render. Creating-then-appending keeps the icon system
    // and our reveal step in agreement.
    //
    // We always fetch the email — when `showEmail` is false (the common
    // case) it's still used as the hover-tooltip on the vignette so the
    // recipient identifier stays discoverable without visual clutter.
    Promise.all([
        systemUsers.getDisplayName(userId),
        systemUsers.getPhoto(userId),
        systemUsers.getEmail(userId),
        showOrigin ? systemUsers.getIsExternal(userId) : Promise.resolve(false)
    ]).then(([name, photo, email, isExternal]) => {
        if (nameEl) nameEl.textContent = name;
        if (emailEl) emailEl.textContent = email ?? '';
        // Tooltip: surface the email on hover for every vignette that
        // has one — including external users whose visible label IS
        // the email already. The redundant "alice@x.com → alice@x.com"
        // hover is a small price for keeping the interaction uniform:
        // every user row in a list reacts to hover the same way, so
        // the user doesn't learn "internal rows have tooltips, external
        // rows are silent". Suppressed only in `showEmail` mode, where
        // the email is already a permanent line below the name.
        //
        // `attachTooltip` portals the popover to `document.body` and
        // applies the shared 250 ms hover-intent delay (much faster
        // than the native `title` attribute's ~500–1500 ms wait).
        // `aria-label` is set in parallel so screen readers still get
        // the email — popover content is mouse/keyboard-hover only.
        if (email && !showEmail) {
            wrapper.setAttribute('aria-label', email);
            attachTooltip(wrapper, email);
        }
        if (photo) {
            _applyPhoto(avatar, photo, name);
        } else {
            avatar.textContent = _initials(name);
        }
        if (showOrigin && isExternal) {
            const badge = document.createElement('i');
            // In avatar-only mode (no name span), overlay the badge on
            // the bottom-right corner of the picture — the right-hand
            // sibling spot doesn't exist there and a row-end position
            // would visually float in nothing. With a name, keep the
            // badge as a sibling on the right of the row.
            const overlay = !showName;
            badge.className = overlay
                ? 'user-vignette__origin user-vignette__origin--external user-vignette__origin--overlay fa-solid fa-building-circle-xmark'
                : 'user-vignette__origin user-vignette__origin--external fa-solid fa-building-circle-xmark';
            badge.title = 'External user';
            badge.setAttribute('aria-hidden', 'true');
            if (overlay) {
                avatar.appendChild(badge);
            } else {
                wrapper.appendChild(badge);
            }
        }
    });

    return wrapper;
}
