/**
 * Global ARIA live region.
 *
 * Announces transient UI messages (toasts, notifications) to assistive
 * technology without any visual footprint. Screen-reader users otherwise
 * miss feedback that sighted users see in the notification bell / toasts.
 *
 * Dependency-free; the region is created lazily on first use so importing
 * this module has no DOM side effect until something is actually announced.
 *
 * @module core/liveRegion
 */

/** @type {HTMLElement | null} */
let politeRegion = null;
/** @type {HTMLElement | null} */
let assertiveRegion = null;

/**
 * Find or create the visually-hidden live region for the given urgency.
 * @param {boolean} assertive
 * @returns {HTMLElement | null}
 */
function ensureRegion(assertive) {
    if (!document.body) return null;
    if (assertive && assertiveRegion) return assertiveRegion;
    if (!assertive && politeRegion) return politeRegion;

    const el = document.createElement('div');
    el.className = 'sr-only';
    el.id = assertive ? 'a11y-live-assertive' : 'a11y-live-polite';
    el.setAttribute('aria-live', assertive ? 'assertive' : 'polite');
    el.setAttribute('aria-atomic', 'true');
    el.setAttribute('role', assertive ? 'alert' : 'status');
    document.body.appendChild(el);

    if (assertive) assertiveRegion = el;
    else politeRegion = el;
    return el;
}

/**
 * Announce a message to screen readers.
 *
 * @param {string} message - text to announce (empty/whitespace is ignored)
 * @param {{ assertive?: boolean }} [opts] - `assertive: true` interrupts the
 *        current speech (use for errors); default is polite.
 */
export function announce(message, opts = {}) {
    const msg = String(message ?? '').trim();
    if (!msg) return;
    const region = ensureRegion(Boolean(opts.assertive));
    if (!region) return;

    // Clear first, then set on the next frame so the DOM mutation is registered
    // as a change even when the same message is announced twice in a row.
    region.textContent = '';
    requestAnimationFrame(() => {
        region.textContent = msg;
    });
}
