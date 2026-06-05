/**
 * MySharesList — row-per-grant list for the My Shares view.
 *
 * Both view modes emit one row per grant. Lane headers are emitted on
 * grouping-key change — the server guarantees ORDER BY group key first.
 *
 * Modes:
 *   'items'      — lane = resource; row identity = subject
 *   'sharedWith' — lane = user | 'links:public' | 'links:password'; row identity = resource
 */

import { getCsrfHeaders } from '../core/csrf.js';
import { formatExpiryChip } from '../core/formatters.js';
import { i18n } from '../core/i18n.js';
import { fileSharing } from '../features/sharing/fileSharing.js';
import { grants } from '../model/grants.js';
import { buildExpiryChip } from '../utils/expiryChip.js';
import { positionMenu } from '../utils/menuPosition.js';
import { buildPasswordChip } from '../utils/passwordChip.js';
import { groupDisplayName, groupIconClass } from './groupDisplay.js';
import { createGroupVignette } from './groupVignette.js';
import { buildLinkChip } from './linkChip.js';
import { buildResourceIcon } from './resourceIcon.js';
import { buildRoleChip, roleLabel } from './roleChip.js';
import { createUserVignette } from './userVignette.js';

/**
 * @import {OutgoingResourceItem, OutgoingResourceGrant, FileItem, FolderItem} from '../core/types.js'
 * @typedef {'items'|'sharedWith'} ViewMode
 * @typedef {'never'|'active'|'soon'|'expired'} ExpiryState
 */

const SOON_DAYS = 30;

/**
 * @param {string|null|undefined} expiresAt
 * @returns {ExpiryState}
 */
function _expiryState(expiresAt) {
    if (!expiresAt) return 'never';
    const ms = new Date(expiresAt).getTime() - Date.now();
    if (ms < 0) return 'expired';
    if (ms <= SOON_DAYS * 86_400_000) return 'soon';
    return 'active';
}

/**
 * Extract the unique group subject IDs across all grants in a page.
 * Callers feed the result to `groups.resolveGroups(...)` so rows can render
 * the group's display name instead of its UUID.
 *
 * @param {OutgoingResourceItem[]} items
 * @returns {Set<string>}
 */
function collectGroupSubjectIds(items) {
    const out = new Set();
    for (const item of items) {
        for (const g of item.grants) {
            if (g.subject_type === 'group') out.add(g.subject_id);
        }
    }
    return out;
}

export { collectGroupSubjectIds };

class MySharesList {
    /**
     * @param {HTMLElement} container
     * @param {{
     *   onResourceOpen: (resource: FileItem|FolderItem, resourceType: string) => void,
     *   onShareEdit:    (resource: FileItem|FolderItem, resourceType: string) => void,
     * }} config
     */
    constructor(container, config) {
        this._container = container;
        this._config = config;
        /** @type {string|null} */
        this._lastSwimKey = null;
        /** @type {HTMLElement|null} */
        this._lastSwimEl = null;
        /**
         * Cached map of group subject UUID → full GroupItem. Populated by
         * the view via `setGroupMeta()` before each `render()` / `append()`
         * so group lane headers and identity rows render with the localised
         * name + virtual-aware icon.
         * @type {Record<string, import('../core/types.js').GroupItem>}
         */
        this._groupMeta = {};
    }

    /**
     * Provide a resolved id→GroupItem map for group subjects expected in
     * the next render / append call. Replaces (does not merge) any previous
     * map.
     * @param {Record<string, import('../core/types.js').GroupItem>} map
     */
    setGroupMeta(map) {
        this._groupMeta = map;
    }

    /**
     * Best-effort display name for a group subject. Falls back to the UUID
     * when no entry has been resolved yet — better than nothing while the
     * resolve query is in flight.
     * @param {string} groupId
     * @returns {string}
     */
    _groupName(groupId) {
        const g = this._groupMeta[groupId];
        return g ? groupDisplayName(g) : groupId;
    }

    /**
     * Icon class for a group subject. Falls back to the regular group icon
     * if the entry hasn't been resolved yet.
     * @param {string} groupId
     * @returns {string}
     */
    _groupIcon(groupId) {
        const g = this._groupMeta[groupId];
        return g ? groupIconClass(g) : 'fa-user-group';
    }

    clear() {
        this._container.innerHTML = '';
        this._lastSwimKey = null;
        this._lastSwimEl = null;
    }

    /**
     * Full re-render (page 1).
     * @param {OutgoingResourceItem[]} items
     * @param {ViewMode} viewMode
     */
    render(items, viewMode) {
        this.clear();
        this._ingest(items, viewMode);
    }

    /**
     * Cursor append (page 2+).
     * @param {OutgoingResourceItem[]} items
     * @param {ViewMode} viewMode
     */
    append(items, viewMode) {
        this._ingest(items, viewMode);
    }

    // ── Core ingest ───────────────────────────────────────────────────────────

    /**
     * @param {OutgoingResourceItem[]} items
     * @param {ViewMode} viewMode
     */
    _ingest(items, viewMode) {
        for (const item of items) {
            if (viewMode === 'items') {
                this._ingestItemsMode(item);
            } else {
                this._ingestSharedWithMode(item);
            }
        }
    }

    /**
     * Items mode — one lane per resource, one grant row per grant.
     * @param {OutgoingResourceItem} item
     */
    _ingestItemsMode(item) {
        const swimKey = `resource:${item.resource.id}`;
        const laneBody = this._ensureLane(swimKey, () => this._buildResourceLaneHeader(item));
        for (const grant of item.grants) {
            laneBody.appendChild(this._buildGrantRow(grant, item, 'items'));
        }
    }

    /**
     * SharedWith mode — one lane per user or per link bucket, one row per grant.
     * @param {OutgoingResourceItem} item
     */
    _ingestSharedWithMode(item) {
        for (const grant of item.grants) {
            let swimKey;
            if (grant.subject_type === 'user') {
                swimKey = `user:${grant.subject_id}`;
            } else if (grant.subject_type === 'group') {
                swimKey = `group:${grant.subject_id}`;
            } else if (grant.has_password) {
                swimKey = 'links:password';
            } else {
                swimKey = 'links:public';
            }
            const laneBody = this._ensureLane(swimKey, () => this._buildSubjectLaneHeader(swimKey, grant));
            laneBody.appendChild(this._buildGrantRow(grant, item, 'sharedWith'));
        }
    }

    // ── Lane management ───────────────────────────────────────────────────────

    /**
     * Return the existing lane body when swimKey matches, else create a new lane.
     * @param {string} swimKey
     * @param {() => HTMLElement} buildHeader
     * @returns {HTMLElement}
     */
    _ensureLane(swimKey, buildHeader) {
        if (swimKey === this._lastSwimKey && this._lastSwimEl) return this._lastSwimEl;

        const lane = document.createElement('div');
        lane.className = 'ms-lane';
        lane.dataset.swimKey = swimKey;

        const header = document.createElement('div');
        header.className = 'ms-lane__header';
        header.appendChild(buildHeader());
        lane.appendChild(header);

        const body = document.createElement('div');
        body.className = 'ms-lane__body';
        lane.appendChild(body);

        this._container.appendChild(lane);
        this._lastSwimKey = swimKey;
        this._lastSwimEl = body;
        return body;
    }

    /**
     * Lane header for items mode: resource icon + name link + Edit sharing button.
     * @param {OutgoingResourceItem} item
     * @returns {HTMLElement}
     */
    _buildResourceLaneHeader(item) {
        const row = document.createElement('div');
        row.className = 'ms-resource-row';
        if (item.resource.path) row.dataset.path = item.resource.path;
        if (item.resource.owner_id) row.dataset.ownerId = item.resource.owner_id;

        row.appendChild(buildResourceIcon(item.resource, item.resource_type));

        const nameLink = document.createElement('a');
        nameLink.className = 'ms-resource-row__name';
        nameLink.href = '#';
        nameLink.textContent = item.resource.name;
        nameLink.addEventListener('click', (e) => {
            e.preventDefault();
            this._config.onResourceOpen(item.resource, item.resource_type);
        });
        row.appendChild(nameLink);

        const editBtn = document.createElement('button');
        editBtn.className = 'ms-resource-row__edit button ghost';
        editBtn.innerHTML = `<i class="fas fa-pencil-alt"></i> ${i18n.t('myshares.editSharing', 'Edit sharing')}`;
        editBtn.addEventListener('click', () => this._config.onShareEdit(item.resource, item.resource_type));
        row.appendChild(editBtn);

        return row;
    }

    /**
     * Lane header for sharedWith mode: user vignette or link bucket label.
     * @param {string} swimKey
     * @param {OutgoingResourceGrant} grant
     * @returns {HTMLElement}
     */
    _buildSubjectLaneHeader(swimKey, grant) {
        if (swimKey.startsWith('user:')) {
            return createUserVignette(grant.subject_id, 'list');
        }
        if (swimKey.startsWith('group:')) {
            return createGroupVignette(this._groupName(grant.subject_id), 'list', {
                icon: this._groupIcon(grant.subject_id),
                groupId: grant.subject_id
            });
        }
        const el = document.createElement('div');
        el.className = 'ms-link-lane-label';
        const icon = document.createElement('i');
        if (swimKey === 'links:password') {
            icon.className = 'fas fa-lock ms-link-lane-label__icon';
            el.appendChild(icon);
            el.appendChild(document.createTextNode(` ${i18n.t('myshares.passwordLinks', 'Password-protected links')}`));
        } else {
            icon.className = 'fas fa-link ms-link-lane-label__icon';
            el.appendChild(icon);
            el.appendChild(document.createTextNode(` ${i18n.t('myshares.publicLinks', 'Public links')}`));
        }
        return el;
    }

    // ── Grant row ─────────────────────────────────────────────────────────────

    /**
     * One grant row: identity + role pill + expiry chip + ⋯ button.
     * @param {OutgoingResourceGrant} grant
     * @param {OutgoingResourceItem} item
     * @param {ViewMode} viewMode
     * @returns {HTMLElement}
     */
    _buildGrantRow(grant, item, viewMode) {
        const row = document.createElement('div');
        row.className = 'ms-grant-row';
        if (_expiryState(grant.expires_at ?? null) === 'expired') {
            row.classList.add('ms-grant-row--expired');
        }
        // In sharedWith mode each grant row represents a (resource → subject)
        // pair, so stamp the resource hierarchy info for the hover tooltip.
        // In items mode the row represents a subject — no resource attrs.
        if (viewMode === 'sharedWith') {
            if (item.resource.path) row.dataset.path = item.resource.path;
            if (item.resource.owner_id) row.dataset.ownerId = item.resource.owner_id;
        }

        row.appendChild(this._buildIdentity(grant, item, viewMode));
        row.appendChild(this._buildRolePill(grant.role));
        row.appendChild(this._buildExpiryChip(grant.expires_at ?? null));
        row.appendChild(this._buildKebabBtn(grant, item, row));

        return row;
    }

    /**
     * Identity: user vignette or link icon + name; tokens in sharedWith mode add → resource.
     * @param {OutgoingResourceGrant} grant
     * @param {OutgoingResourceItem} item
     * @param {ViewMode} viewMode
     * @returns {HTMLElement}
     */
    _buildIdentity(grant, item, viewMode) {
        const el = document.createElement('div');
        el.className = 'ms-grant-row__identity';

        if ((grant.subject_type === 'user' || grant.subject_type === 'group') && viewMode === 'sharedWith') {
            // Lane header is already the subject — show the resource instead.
            el.appendChild(buildResourceIcon(item.resource, item.resource_type));
            const nameLink = document.createElement('a');
            nameLink.className = 'ms-identity__resource-name';
            nameLink.href = '#';
            nameLink.textContent = item.resource.name;
            nameLink.addEventListener('click', (e) => {
                e.preventDefault();
                this._config.onResourceOpen(item.resource, item.resource_type);
            });
            el.appendChild(nameLink);
        } else if (grant.subject_type === 'user') {
            el.appendChild(createUserVignette(grant.subject_id, 'xs'));
        } else if (grant.subject_type === 'group') {
            el.appendChild(
                createGroupVignette(this._groupName(grant.subject_id), 'xs', {
                    icon: this._groupIcon(grant.subject_id),
                    groupId: grant.subject_id
                })
            );
        } else {
            // Token — link chip handles icon + label + copy-on-click
            el.appendChild(buildLinkChip(grant));

            if (viewMode === 'sharedWith') {
                const arrow = document.createElement('span');
                arrow.className = 'ms-link-identity__arrow';
                arrow.textContent = '→';
                el.appendChild(arrow);

                const resLink = document.createElement('a');
                resLink.className = 'ms-link-identity__resource';
                resLink.href = '#';
                resLink.appendChild(buildResourceIcon(item.resource, item.resource_type));
                resLink.appendChild(document.createTextNode(` ${item.resource.name}`));
                resLink.addEventListener('click', (e) => {
                    e.preventDefault();
                    this._config.onResourceOpen(item.resource, item.resource_type);
                });
                el.appendChild(resLink);
            }
        }

        return el;
    }

    /** @param {string} role @returns {HTMLElement} */
    _buildRolePill(role) {
        return buildRoleChip(role);
    }

    /**
     * Build the expiry chip as a DOM element.
     *
     * Delegates label/tier/icon decisions to the shared `formatExpiryChip`
     * helper (used by Trash too) so all expiration chips look identical
     * across the app and stay in sync as the design evolves.
     *
     * @param {string|null} expiresAt
     * @returns {HTMLElement}
     */
    _buildExpiryChip(expiresAt) {
        const tpl = document.createElement('template');
        tpl.innerHTML = formatExpiryChip(expiresAt);
        return /** @type {HTMLElement} */ (tpl.content.firstElementChild);
    }

    // ── Kebab menu ────────────────────────────────────────────────────────────

    /**
     * @param {OutgoingResourceGrant} grant
     * @param {OutgoingResourceItem} item
     * @param {HTMLElement} rowEl
     * @returns {HTMLButtonElement}
     */
    _buildKebabBtn(grant, item, rowEl) {
        const btn = /** @type {HTMLButtonElement} */ (document.createElement('button'));
        btn.className = 'ms-kebab-btn ms-btn-icon';
        btn.setAttribute('aria-label', i18n.t('myshares.manageAccess', 'Manage access'));
        btn.innerHTML = '<i class="fas fa-ellipsis-v"></i>';
        btn.addEventListener('click', (e) => {
            e.stopPropagation();
            this._openGrantMenu(btn, grant, item, rowEl);
        });
        return btn;
    }

    /**
     * Build and show a dynamic context menu positioned below the trigger button.
     * @param {HTMLButtonElement} btn
     * @param {OutgoingResourceGrant} grant
     * @param {OutgoingResourceItem} item
     * @param {HTMLElement} rowEl
     */
    _openGrantMenu(btn, grant, item, rowEl) {
        document.querySelector('.ms-grant-menu')?.remove();

        const menu = document.createElement('div');
        menu.className = 'context-menu ms-grant-menu';

        // Current expiry as YYYY-MM-DD (or null)
        const initialExpiry = grant.expires_at ? String(grant.expires_at).slice(0, 10) : null;

        if (grant.subject_type === 'user' || grant.subject_type === 'group') {
            // PR N2 — "Resend invitation email" / "Notify by email" /
            // "Notify group members". First item in the menu; only
            // present for user and group subjects (token shares have
            // no email channel; the server returns 409 anyway).
            const notifyLabel =
                grant.subject_type === 'group'
                    ? i18n.t('myshares.notifyGroupMembers', 'Notify group members')
                    : grant.is_external
                      ? i18n.t('myshares.resendInvitation', 'Resend invitation email')
                      : i18n.t('myshares.notifyByEmail', 'Notify by email');
            menu.appendChild(
                this._menuItem('fas fa-paper-plane', notifyLabel, false, async () => {
                    menu.remove();
                    await this._notifyRecipient(grant);
                })
            );
            menu.appendChild(this._menuSeparator());

            for (const role of /** @type {('admin'|'editor'|'viewer')[]} */ (['admin', 'editor', 'viewer'])) {
                const isCurrent = grant.role === role;
                const mi = this._menuItem(isCurrent ? 'fas fa-check' : '', roleLabel(role), false, async () => {
                    menu.remove();
                    if (isCurrent) return;
                    await grants.updateRole({
                        subject: { type: grant.subject_type, id: grant.subject_id },
                        resource: { type: item.resource_type, id: item.resource.id },
                        role
                    });
                    const pill = rowEl.querySelector('.role-chip');
                    if (pill) pill.replaceWith(buildRoleChip(role));
                    grant.role = role;
                });
                if (isCurrent) mi.classList.add('ms-menu-item--current');
                menu.appendChild(mi);
            }
            menu.appendChild(this._menuSeparator());
            menu.appendChild(this._menuExpiryRow(grant, item, rowEl, initialExpiry));
            menu.appendChild(this._menuSeparator());
            const removeIcon = grant.subject_type === 'group' ? 'fas fa-user-group' : 'fas fa-user-xmark';
            menu.appendChild(
                this._menuItem(removeIcon, i18n.t('myshares.removeAccess', 'Remove access'), true, async () => {
                    menu.remove();
                    await grants.revokeGrant(grant.grant_id);
                    this._removeRowAndCleanLane(rowEl);
                })
            );
        } else {
            menu.appendChild(
                this._menuItem('fas fa-copy', i18n.t('myshares.copyLink', 'Copy link'), false, async () => {
                    menu.remove();
                    const share = await fileSharing.getShareById(grant.subject_id);
                    await fileSharing.copyLinkToClipboard(share.url);
                })
            );
            menu.appendChild(this._menuSeparator());
            menu.appendChild(this._menuExpiryRow(grant, item, rowEl, initialExpiry));
            menu.appendChild(this._menuPasswordRow(grant, rowEl));
            menu.appendChild(this._menuSeparator());
            menu.appendChild(
                this._menuItem('fas fa-trash', i18n.t('myshares.deleteLink', 'Delete link'), true, async () => {
                    menu.remove();
                    await fileSharing.removeSharedLink(grant.subject_id);
                    this._removeRowAndCleanLane(rowEl);
                })
            );
        }

        document.body.appendChild(menu);

        // Position below the trigger, flipping above (or clamping up)
        // when the trigger is too close to the bottom of the viewport.
        // Single source of truth for menu positioning — see
        // `static/js/utils/menuPosition.js`.
        positionMenu(menu, { anchor: btn });

        const close = (/** @type {Event} */ e) => {
            if (e.type === 'keydown' && /** @type {KeyboardEvent} */ (e).key !== 'Escape') return;
            // Keep menu open when interacting with elements inside it (e.g. the date input)
            if (e.type === 'click' && menu.contains(/** @type {Node} */ (e.target))) return;
            menu.remove();
            document.removeEventListener('click', close, true);
            document.removeEventListener('keydown', close, true);
        };
        setTimeout(() => {
            document.addEventListener('click', close, true);
            document.addEventListener('keydown', close, true);
        }, 0);
    }

    /**
     * Non-closing expiry row embedded in the context menu.
     * Uses the shared smd-expiry-chip; saves on blur/Enter.
     * @param {OutgoingResourceGrant} grant
     * @param {OutgoingResourceItem} item
     * @param {HTMLElement} rowEl
     * @param {string|null} initialExpiry  YYYY-MM-DD or null
     * @returns {HTMLElement}
     */
    _menuExpiryRow(grant, item, rowEl, initialExpiry) {
        const row = document.createElement('div');
        row.className = 'ms-menu-expiry-row';

        const label = document.createElement('span');
        label.className = 'ms-menu-expiry-label';
        label.textContent = i18n.t('share.expiry', 'Expiry');
        row.appendChild(label);

        const chip = buildExpiryChip(initialExpiry, async (dateStr) => {
            const expiresIso = dateStr ? new Date(`${dateStr}T00:00:00Z`).toISOString() : null;
            try {
                await grants.updateRole({
                    subject: { type: grant.subject_type, id: grant.subject_id },
                    resource: { type: item.resource_type, id: item.resource.id },
                    role: grant.role,
                    expires_at: expiresIso
                });
                grant.expires_at = expiresIso;
                // Replace the display chip in the grant row. The chip class
                // is `expiry-chip` (emitted by `formatExpiryChip` in
                // core/formatters.js) — NOT `ms-expiry-chip`. The earlier
                // selector silently no-op'd, which is why the row never
                // refreshed after an expiry change.
                const displayChip = rowEl.querySelector('.expiry-chip');
                if (displayChip) {
                    const newChip = this._buildExpiryChip(expiresIso);
                    displayChip.replaceWith(newChip);
                }
                // Keep the row's expired styling in sync (0.6 opacity).
                rowEl.classList.toggle('ms-grant-row--expired', _expiryState(expiresIso) === 'expired');
            } catch (err) {
                console.error('mySharesList: setExpiry failed', err);
            }
        });
        row.appendChild(chip);

        return row;
    }

    /**
     * PR N2 — manual share-notification resend. Calls
     * `POST /api/grants/{grant_id}/notify` and surfaces the aggregated
     * outcome to the granter. The endpoint returns:
     *   - 204 No Content      — all recipients sent
     *   - 200 + NotifyOutcomeSetDto — mixed outcomes (coalesced /
     *     not-applicable / partial sent)
     *   - 429 Too Many Requests — per-recipient rate limit hit on every
     *     recipient
     *   - 404 Not Found       — caller is not the granter, or grant
     *     doesn't exist (anti-enumeration; the audit log carries the
     *     truth)
     *   - 409 Conflict        — token subject (UI shouldn't reach this)
     *
     * @param {OutgoingResourceGrant} grant
     */
    async _notifyRecipient(grant) {
        try {
            const resp = await fetch(`/api/grants/${encodeURIComponent(grant.grant_id)}/notify`, {
                method: 'POST',
                credentials: 'same-origin',
                headers: { ...getCsrfHeaders() }
            });
            if (resp.status === 204) {
                // All sent — silent success.
                console.log('[myshares] notify: all recipients sent', grant.grant_id);
                return;
            }
            if (resp.status === 429) {
                // eslint-disable-next-line no-alert -- minimal v1 surface
                alert(i18n.t('myshares.notifyRateLimited', 'Too many notifications for this recipient — try again later.'));
                return;
            }
            if (resp.ok) {
                /** @type {{ total_recipients: number, outcomes: Array<{kind: string, detail?: string, reason?: string}> }} */
                const body = await resp.json();
                console.log('[myshares] notify outcomes:', body);
                const sent = body.outcomes.filter((o) => o.kind === 'sent').length;
                const coalesced = body.outcomes.filter((o) => o.kind === 'coalesced').length;
                const notApplicable = body.outcomes.filter((o) => o.kind === 'not_applicable').length;
                /** @type {string[]} */
                const lines = [];
                if (sent > 0) lines.push(`${sent} recipient(s) notified by email.`);
                if (coalesced > 0) lines.push(`${coalesced} recipient(s) already notified recently — they'll see the share at next login.`);
                if (notApplicable > 0) lines.push(`${notApplicable} recipient(s) skipped (opted out, no email, or operator-disabled).`);
                if (lines.length > 0) {
                    // eslint-disable-next-line no-alert -- minimal v1 surface
                    alert(lines.join('\n'));
                }
                return;
            }
            // 404 / 409 / unexpected
            console.error('[myshares] notify failed:', resp.status);
            // eslint-disable-next-line no-alert -- minimal v1 surface
            alert(i18n.t('myshares.notifyFailed', 'Could not send notification.'));
        } catch (err) {
            console.error('[myshares] notify error:', err);
            // eslint-disable-next-line no-alert -- minimal v1 surface
            alert(i18n.t('myshares.notifyFailed', 'Could not send notification.'));
        }
    }

    /**
     * Non-closing password row embedded in the link context menu.
     * Saves immediately on confirm (blur / Enter).
     * @param {OutgoingResourceGrant} grant
     * @param {HTMLElement} rowEl
     * @returns {HTMLElement}
     */
    _menuPasswordRow(grant, rowEl) {
        const row = document.createElement('div');
        row.className = 'ms-menu-expiry-row';

        const label = document.createElement('span');
        label.className = 'ms-menu-expiry-label';
        label.textContent = i18n.t('share.password', 'Password');
        row.appendChild(label);

        const chip = buildPasswordChip(grant.has_password, async (newPassword) => {
            try {
                await fileSharing.updateSharedLink(grant.subject_id, {
                    password: newPassword || null
                });
                grant.has_password = !!newPassword;
                // Update the lock icon on the link chip in the row
                const linkChipEl = rowEl.querySelector('.link-chip');
                if (linkChipEl) {
                    linkChipEl.classList.toggle('link-chip--locked', grant.has_password);
                    const iconEl = linkChipEl.querySelector('.link-chip__icon');
                    if (iconEl) {
                        iconEl.className = grant.has_password ? 'fas fa-lock link-chip__icon' : 'fas fa-link link-chip__icon';
                    }
                }
            } catch (err) {
                console.error('mySharesList: setPassword failed', err);
            }
        });
        row.appendChild(chip);

        return row;
    }

    /**
     * @param {string} iconClass
     * @param {string} label
     * @param {boolean} danger
     * @param {() => void} onClick
     * @returns {HTMLElement}
     */
    _menuItem(iconClass, label, danger, onClick) {
        const el = document.createElement('div');
        el.className = danger ? 'context-menu-item context-menu-item-danger' : 'context-menu-item';
        el.setAttribute('role', 'menuitem');
        if (iconClass) {
            el.innerHTML = `<i class="${iconClass}"></i> `;
        }
        el.appendChild(document.createTextNode(label));
        el.addEventListener('click', /** @type {EventListener} */ (onClick));
        return el;
    }

    /** @returns {HTMLElement} */
    _menuSeparator() {
        const el = document.createElement('div');
        el.className = 'context-menu-separator';
        return el;
    }

    /**
     * Remove the row; if the lane body is now empty, remove the whole lane.
     * @param {HTMLElement} rowEl
     */
    _removeRowAndCleanLane(rowEl) {
        const laneBody = rowEl.closest('.ms-lane__body');
        rowEl.remove();
        if (laneBody instanceof HTMLElement && laneBody.children.length === 0) {
            const lane = laneBody.closest('.ms-lane');
            if (lane instanceof HTMLElement) {
                if (lane.dataset.swimKey === this._lastSwimKey) {
                    this._lastSwimKey = null;
                    this._lastSwimEl = null;
                }
                lane.remove();
            }
        }
    }
}

export { MySharesList };
