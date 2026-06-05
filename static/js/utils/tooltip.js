// @ts-check

/**
 * Generic tooltip helper used app-wide.
 *
 * Two flavours:
 *
 *  - {@link attachTooltip}      — short text label, optionally populated
 *                                 from a `data-tooltip` attribute. Use
 *                                 for one-liners (the email-on-hover on
 *                                 a user vignette, the title-text on a
 *                                 chip / button, etc.).
 *  - {@link attachRichTooltip}  — structured DOM populated lazily on
 *                                 first hover. Use when the content is
 *                                 multi-line, async-fetched, or needs
 *                                 inner styling (e.g. the member list
 *                                 in a group vignette).
 *
 * Both portal the popover to `document.body` and position it with
 * `position: fixed` so it escapes every ancestor `overflow: hidden`
 * clip in the page. This is the only reliable cross-browser way to
 * keep tooltips fully visible from triggers buried inside list rows,
 * scroll containers, or modal panels.
 *
 * The class toggle (`oxi-tooltip-popover--visible`) is JS-driven on
 * `mouseenter` / `mouseleave` / `focusin` / `focusout`. The hover-intent
 * delay (250 ms before fade-in, 0 ms before fade-out) lives entirely
 * in the CSS transition rules — never in a JS `setTimeout`. See
 * `static/css/components/tooltip.css` for the timing source of truth.
 *
 * Layout helpers exported for the rich variant:
 *  - `OxiTooltipClass.LINE`      — apply to each row inside the popover
 *  - `OxiTooltipClass.OVERFLOW`  — small "+N" badge for truncated lists
 *  - `OxiTooltipClass.PLACEHOLDER` — italic dimmed text for loading /
 *                                    empty states
 */

const POPOVER_CLASS = 'oxi-tooltip-popover';
const VISIBLE_CLASS = 'oxi-tooltip-popover--visible';
const SIMPLE_CLASS = 'oxi-tooltip-popover--simple';

/** Class names exported so callers can build the popover body with the
 *  layout helpers without dragging in private CSS module conventions. */
export const OxiTooltipClass = Object.freeze({
    LINE: 'oxi-tooltip-popover__line',
    OVERFLOW: 'oxi-tooltip-popover__overflow',
    PLACEHOLDER: 'oxi-tooltip-popover__placeholder'
});

/** Distance between the tooltip and the trigger edge, in pixels. */
const GAP = 6;

/** Inset from the viewport edges when clamping the tooltip position. */
const MARGIN = 8;

/**
 * Position `popover` above (or below, when there isn't room above) the
 * given trigger element. Uses `position: fixed` so it escapes any
 * ancestor `overflow: hidden`. Clamps horizontally and vertically into
 * the viewport so tooltips near the edges still read cleanly.
 *
 * @param {HTMLElement} popover
 * @param {HTMLElement} triggerEl
 */
function _positionPopover(popover, triggerEl) {
    const triggerRect = triggerEl.getBoundingClientRect();
    // Measure after content has been added so we know the final size.
    const popRect = popover.getBoundingClientRect();
    const vw = window.innerWidth;
    const vh = window.innerHeight;

    // Vertical: prefer above the trigger. Flip below when there's not
    // enough room above.
    let top = triggerRect.top - popRect.height - GAP;
    if (top < MARGIN) {
        top = triggerRect.bottom + GAP;
    }

    // Horizontal: center on the trigger, clamp into the viewport.
    let left = triggerRect.left + triggerRect.width / 2 - popRect.width / 2;
    if (left < MARGIN) left = MARGIN;
    if (left + popRect.width > vw - MARGIN) left = vw - popRect.width - MARGIN;

    // Final vertical clamp — covers the (very rare) case where the
    // tooltip is taller than the visible viewport.
    if (top + popRect.height > vh - MARGIN) top = vh - popRect.height - MARGIN;
    if (top < MARGIN) top = MARGIN;

    popover.style.top = `${top}px`;
    popover.style.left = `${left}px`;
}

/**
 * Internal: wire mouseenter/leave + focusin/out listeners on `triggerEl`,
 * lazily create the popover element on first hover, and call `populate`
 * once to fill it. Returns a cleanup function that removes the
 * listeners and the popover element.
 *
 * @param {HTMLElement} triggerEl
 * @param {(popover: HTMLElement) => void | Promise<void>} populate
 *   Called exactly once when the popover is first shown. Synchronous
 *   populates take effect immediately; async populates show the
 *   placeholder span (if you created one) until the promise resolves,
 *   after which the popover is re-positioned to account for size
 *   changes.
 * @param {{ simple?: boolean }} [opts]
 *   `simple`: add the `--simple` modifier so the popover uses the
 *   single-line label style (white-space: nowrap, no min-width).
 * @returns {() => void}  Cleanup; idempotent.
 */
function _attach(triggerEl, populate, opts = {}) {
    /** @type {HTMLElement | null} */
    let popover = null;
    let populated = false;
    let detached = false;

    const ensurePopover = () => {
        if (popover) return popover;
        popover = document.createElement('div');
        popover.className = POPOVER_CLASS + (opts.simple ? ` ${SIMPLE_CLASS}` : '');
        // ARIA: behave like a tooltip for screen readers — though we
        // also rely on `aria-label` / surrounding text since hover
        // isn't reachable via keyboard-only assistive tech.
        popover.setAttribute('role', 'tooltip');
        document.body.appendChild(popover);
        return popover;
    };

    const show = () => {
        if (detached) return;
        const pop = ensurePopover();

        if (!populated) {
            populated = true;
            // Synchronous populate paths render immediately. Async
            // populates (those returning a Promise) re-position after
            // resolve so the tooltip catches up to its final size —
            // important when the placeholder text is much narrower
            // than the eventual content.
            const result = populate(pop);
            if (result && typeof (/** @type {Promise<void>} */ (result).then) === 'function') {
                /** @type {Promise<void>} */ (result).then(() => {
                    if (popover?.classList.contains(VISIBLE_CLASS)) {
                        _positionPopover(popover, triggerEl);
                    }
                });
            }
        }

        _positionPopover(pop, triggerEl);
        pop.classList.add(VISIBLE_CLASS);
    };

    const hide = () => {
        if (popover) popover.classList.remove(VISIBLE_CLASS);
    };

    triggerEl.addEventListener('mouseenter', show);
    triggerEl.addEventListener('mouseleave', hide);
    triggerEl.addEventListener('focusin', show);
    triggerEl.addEventListener('focusout', hide);

    return () => {
        if (detached) return;
        detached = true;
        triggerEl.removeEventListener('mouseenter', show);
        triggerEl.removeEventListener('mouseleave', hide);
        triggerEl.removeEventListener('focusin', show);
        triggerEl.removeEventListener('focusout', hide);
        popover?.remove();
        popover = null;
    };
}

/**
 * Attach a simple single-line tooltip to `triggerEl`.
 *
 * @param {HTMLElement} triggerEl
 * @param {string}      text   The label to display.
 * @returns {() => void}        Cleanup function; idempotent.
 *
 * @example
 *   attachTooltip(emailBadgeEl, 'alice@example.com');
 */
export function attachTooltip(triggerEl, text) {
    return _attach(
        triggerEl,
        (pop) => {
            pop.textContent = text;
        },
        { simple: true }
    );
}

/**
 * Attach a rich tooltip with structured DOM populated lazily on first
 * hover. The `populate` callback receives the popover element and can
 * append whatever children it wants. Return a Promise to populate
 * async — the popover re-positions on resolve.
 *
 * @param {HTMLElement} triggerEl
 * @param {(popover: HTMLElement) => void | Promise<void>} populate
 * @returns {() => void}  Cleanup function; idempotent.
 *
 * @example
 *   attachRichTooltip(groupEl, async (pop) => {
 *       const placeholder = document.createElement('div');
 *       placeholder.className = OxiTooltipClass.PLACEHOLDER;
 *       placeholder.textContent = i18n.t('groups.members_loading');
 *       pop.appendChild(placeholder);
 *       const members = await fetchMembers(groupId);
 *       pop.replaceChildren(); // drop the placeholder
 *       for (const name of members.slice(0, 8)) {
 *           const line = document.createElement('div');
 *           line.className = OxiTooltipClass.LINE;
 *           line.textContent = name;
 *           pop.appendChild(line);
 *       }
 *       if (members.length > 8) {
 *           const overflow = document.createElement('div');
 *           overflow.className = OxiTooltipClass.LINE;
 *           const badge = document.createElement('span');
 *           badge.className = OxiTooltipClass.OVERFLOW;
 *           badge.textContent = `+${members.length - 8}`;
 *           overflow.append('… ', badge);
 *           pop.appendChild(overflow);
 *       }
 *   });
 */
export function attachRichTooltip(triggerEl, populate) {
    return _attach(triggerEl, populate);
}
