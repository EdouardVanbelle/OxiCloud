// @ts-check

/**
 * Inline element representing a ReBAC subject group: user-group icon +
 * the group's name. Used by the share dialog (to display groups as share
 * recipients) and by the group-management view (to display nested-group
 * members).
 *
 * Visually mirrors `createUserVignette` from `./userVignette.js` so a row
 * built from one can swap in the other without layout shift. Picks
 * `fa-user-group` (a *people* icon) rather than `fa-layer-group`, which is
 * reserved across the app for the *grouping operator* on group-by menu pills
 * — keeping the two concepts visually distinct.
 *
 *   subject group         (this file)        fa-user-group
 *   grouping operator     (group-by pills)   fa-layer-group
 */

import { escapeHtml } from '../core/formatters.js';
import { i18n } from '../core/i18n.js';
import { groups, INTERNAL_GROUP_ID } from '../model/groups.js';
import { systemUsers } from '../model/systemUsers.js';
import { attachRichTooltip, OxiTooltipClass } from '../utils/tooltip.js';

/** Max member names displayed in the on-hover tooltip; any extra count
 *  surfaces as a "+N" badge on the last line. Kept small enough to fit
 *  inside the 280 px tooltip without scrolling, large enough to be
 *  informative for the typical share-with-a-team case. */
const MAX_MEMBERS_IN_TOOLTIP = 8;

/**
 * Session-scoped cache of resolved member lists, keyed by group UUID.
 * One entry per group ever hovered; the promise is reused across
 * subsequent vignettes for the same group so a list of 50 rows that
 * all reference the same group hits `/api/groups/{id}/members` once.
 *
 * @type {Map<string, Promise<{ names: string[], total: number }>>}
 */
const _membersCache = new Map();

/**
 * Resolve the first N direct members of a group to display names.
 * Idempotent + memoised across vignettes that share a group id.
 *
 * Failures (403 on a group the caller can't list, 404 on a stale id,
 * network errors) resolve to an empty-names + zero-total shape so the
 * UI shows a graceful "no members" placeholder instead of throwing.
 *
 * @param {string} groupId
 * @returns {Promise<{ names: string[], total: number }>}
 */
async function _resolveGroupMembers(groupId) {
    const cached = _membersCache.get(groupId);
    if (cached) return cached;

    const promise = (async () => {
        /** @type {import('../core/types.js').GroupMemberItem[]} */
        let members;
        try {
            members = await groups.listMembers(groupId);
        } catch {
            return { names: [], total: 0 };
        }
        const total = members.length;
        // Only resolve the names we'll actually display — the "+N more"
        // badge counts the rest from `total - MAX`. Cuts down on the
        // number of /api/users/{id} backfills for big groups.
        const slice = members.slice(0, MAX_MEMBERS_IN_TOOLTIP);

        // Parallelise the per-member name lookups so a 8-member group
        // resolves in one round-trip's worth of latency, not eight.
        const names = await Promise.all(
            slice.map(async (m) => {
                if (m.kind === 'user') {
                    try {
                        return await systemUsers.getDisplayName(m.id);
                    } catch {
                        return `${m.id.slice(0, 8)}…`;
                    }
                }
                // Nested group — resolve via groups model.
                try {
                    const resolved = await groups.resolveGroups([m.id]);
                    const g = resolved?.[m.id];
                    if (g?.name) return `👥 ${g.name}`;
                } catch {
                    // fall through
                }
                return `👥 ${m.id.slice(0, 8)}…`;
            })
        );
        return { names, total };
    })();

    _membersCache.set(groupId, promise);
    return promise;
}

/**
 * Attach the on-hover member popover to a group vignette. Delegates
 * positioning + show/hide + portal placement to the generic
 * `attachRichTooltip` helper (see `utils/tooltip.js`); this function
 * just owns the per-row content — the placeholder, the member lines,
 * and the "+N" overflow badge.
 *
 * @param {HTMLElement} vignetteEl  the wrapper returned by `createGroupVignette`
 * @param {string}      groupId     UUID of the group whose members to show
 */
function _attachMembersTooltip(vignetteEl, groupId) {
    attachRichTooltip(vignetteEl, async (pop) => {
        // Placeholder shown until the network resolves. Slow connections
        // see "Loading members…" instead of an empty box.
        const placeholder = document.createElement('div');
        placeholder.className = OxiTooltipClass.PLACEHOLDER;
        placeholder.textContent = i18n.t('groups.members_loading', 'Loading members…');
        pop.appendChild(placeholder);

        const { names, total } = await _resolveGroupMembers(groupId);
        pop.replaceChildren();

        if (total === 0) {
            const empty = document.createElement('div');
            empty.className = OxiTooltipClass.PLACEHOLDER;
            // Special case: the built-in "Internal" virtual group has
            // implicit membership — it represents every internal user
            // on this server. `listMembers` returns an empty array
            // because no explicit rows exist in `auth.subject_group_members`,
            // but "No members" would mislead. Surface the real meaning
            // instead. Future virtual groups with implicit membership
            // (e.g. "Everyone") would extend this branch.
            empty.textContent =
                groupId === INTERNAL_GROUP_ID
                    ? i18n.t('groups.virtual_internal_explanation', 'Every internal user on this server')
                    : i18n.t('groups.members_empty', 'No members');
            pop.appendChild(empty);
            return;
        }
        for (const name of names) {
            const line = document.createElement('div');
            line.className = OxiTooltipClass.LINE;
            line.textContent = name;
            pop.appendChild(line);
        }
        if (total > MAX_MEMBERS_IN_TOOLTIP) {
            const overflow = document.createElement('div');
            overflow.className = OxiTooltipClass.LINE;
            // Inner badge so the "+N" reads as a count, not another name.
            const badge = document.createElement('span');
            badge.className = OxiTooltipClass.OVERFLOW;
            badge.textContent = `+${total - MAX_MEMBERS_IN_TOOLTIP}`;
            overflow.append('… ', badge);
            pop.appendChild(overflow);
        }
    });
}

/**
 * Build the inline vignette.
 *
 * @param {string} name
 *   Display name of the group (escaped before injection).
 * @param {'xs'|'sm'|'md'|'list'} [size='sm']
 *   Matches the size scale of `createUserVignette`. The size class is
 *   `user-vignette--${size}`; see `static/css/components/userVignette.css`.
 * @param {{ icon?: string, groupId?: string }} [opts]
 *   `icon`: FA class string without the `fa-` prefix (defaults to
 *   `'fa-user-group'`). Used to signal virtual groups visually — see
 *   `groupIconClass()` / `groupIconClassByVirtual()` in `./groupDisplay.js`
 *   return a distinct icon for system-wide virtual groups (Internal,
 *   future Everyone, …).
 *
 *   `groupId`: UUID of the group. When supplied, hovering the vignette
 *   reveals a tooltip listing up to {@link MAX_MEMBERS_IN_TOOLTIP}
 *   member names with a `+N` badge for overflow. The member list is
 *   fetched lazily on first hover, memoised across all vignettes that
 *   share the same id, so a row of 50 grants pointing at the same
 *   team only hits `/api/groups/{id}/members` once.
 * @returns {HTMLElement}
 */
export function createGroupVignette(name, size = 'sm', { icon = 'fa-user-group', groupId } = {}) {
    const el = document.createElement('div');
    el.className = `user-vignette user-vignette-group user-vignette--${size}`;
    el.innerHTML = `<span class="user-vignette__avatar"><i class="fas ${escapeHtml(icon)}"></i></span><span class="user-vignette__name">${escapeHtml(name)}</span>`;
    if (groupId) _attachMembersTooltip(el, groupId);
    return el;
}
