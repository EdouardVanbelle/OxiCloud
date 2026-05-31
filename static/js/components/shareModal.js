// @ts-check

/**
 * ShareModal — unified sharing dialog for files and folders.
 *
 * Covers two areas:
 *   • People (user-to-user grants via `/api/grants`)
 *   • Public links (via `/api/shares`)
 *
 * All mutations are staged locally and committed only when the user clicks
 * Apply.  The only immediate action is Copy Link (clipboard).
 *
 * The dialog shell (overlay, animation, header, footer, Escape/click-outside
 * handling) is delegated entirely to `Modal.openPanel()`.
 */

import { ui } from '../app/ui.js';
import { i18n } from '../core/i18n.js';
import { fileSharing } from '../features/sharing/fileSharing.js';
import { addressBook, SYSTEM_BOOK_ID } from '../model/addressBook.js';
import { grants } from '../model/grants.js';
import { groups } from '../model/groups.js';
import { systemUsers } from '../model/systemUsers.js';
import { buildExpiryChip } from '../utils/expiryChip.js';
import { buildPasswordChip } from '../utils/passwordChip.js';
import { groupDisplayName, groupIconClass, groupIconClassByVirtual } from './groupDisplay.js';
import { createGroupVignette } from './groupVignette.js';
import { Modal } from './modal.js';
import { createUserVignette } from './userVignette.js';

/** @import {FileItem, FolderItem, Grant, ContactItem, MemberEntry, LinkEntry, DraftLink, ShareRoleEnum} from '../core/types.js' */

/**
 * A ReBAC subject group surfaced by `/api/groups/search`. Shape is a
 * deliberate superset of `ContactItem` so the staging / chip / commit code
 * paths can treat both uniformly, discriminating on the `_kind` field.
 *
 * @typedef {Object} GroupSuggestion
 * @property {string} id
 * @property {string} name
 * @property {boolean} is_virtual
 * @property {'group'} _kind
 */

/** Permissions that belong to each role (must mirror the Rust DTO). */
const ROLE_PERMISSIONS = {
    viewer: ['read'],
    editor: ['read', 'comment', 'create', 'update'],
    admin: ['read', 'comment', 'create', 'update', 'share', 'delete']
};

/**
 * Fetch up to ~8 ReBAC subject groups whose name matches `q`. Authenticated
 * endpoint; returns `[]` on any failure so the autocomplete degrades to
 * contacts-only rather than breaking the dialog.
 * @param {string} q
 * @returns {Promise<GroupSuggestion[]>}
 */
async function _searchGroups(q) {
    try {
        const res = await fetch(`/api/groups/search?q=${encodeURIComponent(q)}&limit=8`, {
            credentials: 'include'
        });
        if (!res.ok) return [];
        /** @type {Array<{id:string,name:string,is_virtual:boolean}>} */
        const items = await res.json();
        return items.map((g) => ({
            id: g.id,
            name: g.name,
            is_virtual: !!g.is_virtual,
            _kind: /** @type {'group'} */ ('group')
        }));
    } catch {
        return [];
    }
}

/**
 * Derive the highest role a set of grants represents for one subject.
 * @param {Grant[]} subjectGrants
 * @returns {ShareRoleEnum}
 */
function _roleFromGrants(subjectGrants) {
    const perms = new Set(subjectGrants.map((g) => g.permission));
    if (perms.has('delete') || perms.has('share')) return 'admin';
    if (perms.has('create') || perms.has('update')) return 'editor';
    return 'viewer';
}

/**
 * Group grants by subject id and return one MemberEntry per unique subject.
 * @param {Grant[]} grantList
 * @returns {MemberEntry[]}
 */
function _buildMembers(grantList) {
    /** @type {Map<string, Grant[]>} */
    const bySubject = new Map();
    for (const g of grantList) {
        // Token grants represent public-link access — they belong in the Links
        // section, not the People section.
        if (g.subject.type === 'token') continue;
        const key = g.subject.id;
        if (!bySubject.has(key)) bySubject.set(key, []);
        bySubject.get(key).push(g);
    }
    /** @type {MemberEntry[]} */
    const members = [];
    for (const subjectGrants of bySubject.values()) {
        members.push({
            grant: subjectGrants[0], // representative grant (used for subject/resource info)
            _grants: subjectGrants, // all grants — needed to revoke every permission on remove
            role: _roleFromGrants(subjectGrants),
            _op: 'keep'
        });
    }
    return members;
}

// ── Component ──────────────────────────────────────────────────────────────────

const shareModal = {
    // ── State ──────────────────────────────────────────────────────────────────

    /** @type {FileItem|FolderItem|null} */
    _item: null,

    /** @type {'file'|'folder'} */
    _itemType: 'file',

    /** @type {MemberEntry[]} */
    _localMembers: [],

    /** @type {LinkEntry[]} */
    _localLinks: [],

    /** @type {DraftLink[]} */
    _newLinks: [],

    /** @type {Array<ContactItem | GroupSuggestion>} */
    _stagedUsers: [],

    /** @type {ShareRoleEnum} */
    _stagedRole: 'viewer',

    /** @type {string|null} — YYYY-MM-DD expiry for the next staged users batch */
    _stagedExpiry: null,

    /** @type {HTMLElement|null} — body node injected into Modal */
    _bodyEl: null,

    /** @type {(() => void)|null} — called after changes are successfully committed */
    _onApplied: null,

    // ── Public API ─────────────────────────────────────────────────────────────

    /**
     * Open the share modal for a file or folder.
     * @param {FileItem|FolderItem} item
     * @param {'file'|'folder'}     itemType
     * @param {(() => void)=}       onApplied - called after changes are successfully committed
     */
    async open(item, itemType, onApplied) {
        this._item = item;
        this._itemType = itemType;
        this._onApplied = onApplied ?? null;
        this._localMembers = [];
        this._localLinks = [];
        this._newLinks = [];
        this._stagedUsers = [];
        this._stagedRole = 'viewer';
        this._stagedExpiry = null;

        const title = `${i18n.t('share.shareOf', 'Share of:')} ${item.name}`;

        // Build body with loading skeleton
        this._bodyEl = this._buildSkeleton();

        Modal.openPanel({
            title,
            icon: 'fa-share-alt',
            content: this._bodyEl,
            confirmText: i18n.t('actions.apply', 'Apply'),
            confirmDisabled: true,
            onConfirm: () => {
                this._applyAll();
            } // intentionally discard Promise
        });

        // Prefetch system users in background so tooltips resolve instantly.
        systemUsers.prefetch();

        // Load data
        try {
            const [grantList, linkList] = await Promise.all([
                grants.fetchGrantsForResource(itemType, item.id),
                fileSharing.getSharedLinksForItem(item.id, itemType)
            ]);

            this._localMembers = _buildMembers(grantList);
            this._localLinks = linkList.map((share) => /** @type {LinkEntry} */ ({ share, _op: 'keep', _draft: null }));

            // Group subjects in grants only carry their UUID — resolve full
            // GroupItem records so member rows render the localised name and
            // pick the correct icon (virtual groups get people-roof via
            // `groupIconClass`).
            const groupIds = new Set(this._localMembers.filter((m) => m.grant.subject.type === 'group').map((m) => m.grant.subject.id));
            if (groupIds.size > 0) {
                const resolved = await groups.resolveGroups(groupIds);
                for (const m of this._localMembers) {
                    if (m.grant.subject.type === 'group') {
                        const g = resolved[m.grant.subject.id];
                        if (g) {
                            m._displayName = groupDisplayName(g);
                            m._isVirtual = g.is_virtual;
                        } else {
                            m._displayName = m.grant.subject.id;
                        }
                    }
                }
            }
        } catch (err) {
            console.error('shareModal: load error', err);
        }

        // Swap skeleton → real content
        if (this._bodyEl) {
            this._bodyEl.replaceChildren(...this._buildContent());
        }
    },

    /**
     * Close the modal (delegates to Modal.close).
     */
    close() {
        Modal.close(false);
    },

    // ── Apply-button state ─────────────────────────────────────────────────────

    /** @returns {boolean} */
    _hasPendingChanges() {
        return this._localMembers.some((m) => m._op !== 'keep') || this._localLinks.some((e) => e._op !== 'keep') || this._newLinks.length > 0;
    },

    _syncApplyBtn() {
        if (Modal.confirmBtn) Modal.confirmBtn.disabled = !this._hasPendingChanges();
    },

    // ── Skeleton ───────────────────────────────────────────────────────────────

    /**
     * @returns {HTMLElement}
     */
    _buildSkeleton() {
        const body = document.createElement('div');
        body.className = 'smd-body';
        const skel = document.createElement('div');
        skel.className = 'smd-skeleton';
        for (const cls of ['smd-skeleton-line smd-skeleton-line--short', 'smd-skeleton-line smd-skeleton-line--medium', 'smd-skeleton-line']) {
            const line = document.createElement('div');
            line.className = cls;
            skel.appendChild(line);
        }
        body.appendChild(skel);
        return body;
    },

    // ── Content builder ────────────────────────────────────────────────────────

    /**
     * Build the two sections (People + Links) as an array of elements.
     * @returns {HTMLElement[]}
     */
    _buildContent() {
        return [this._buildPeopleSection(), this._buildLinksSection()];
    },

    // ── People section ─────────────────────────────────────────────────────────

    /**
     * @returns {HTMLElement}
     */
    _buildPeopleSection() {
        const section = document.createElement('div');
        section.className = 'smd-section';

        const title = document.createElement('div');
        title.className = 'smd-section-title';
        title.textContent = i18n.t('share.people', 'People');
        section.appendChild(title);

        if (addressBook.isSystemAvailable()) {
            section.appendChild(this._buildSearchRow());
            section.appendChild(this._buildChipsRow());
        } else {
            const note = document.createElement('p');
            note.className = 'smd-directory-unavailable';
            note.textContent = i18n.t('share.directoryUnavailable', 'User directory unavailable');
            section.appendChild(note);
        }

        section.appendChild(this._buildMemberGroups());
        return section;
    },

    /**
     * @returns {HTMLElement}
     */
    _buildSearchRow() {
        const row = document.createElement('div');
        row.className = 'smd-search-row';

        // ── Search input + dropdown ──────────────────────────────────────────
        const wrap = document.createElement('div');
        wrap.className = 'smd-search-wrap';

        const input = document.createElement('input');
        input.type = 'text';
        input.className = 'smd-search-input';
        input.placeholder = i18n.t('share.searchPlaceholder', 'Search people…');
        input.autocomplete = 'off';

        const dropdown = document.createElement('div');
        dropdown.className = 'smd-suggestions hidden';

        wrap.appendChild(input);
        wrap.appendChild(dropdown);

        // ── Role select ──────────────────────────────────────────────────────
        const roleSelect = document.createElement('select');
        roleSelect.className = 'smd-role-select';
        for (const [val, label] of [
            ['viewer', i18n.t('share.role.canView', 'Can view')],
            ['editor', i18n.t('share.role.canEdit', 'Can edit')],
            ['admin', i18n.t('share.role.canManage', 'Can manage')]
        ]) {
            const opt = document.createElement('option');
            opt.value = val;
            opt.textContent = label;
            if (val === this._stagedRole) opt.selected = true;
            roleSelect.appendChild(opt);
        }
        roleSelect.addEventListener('change', () => {
            this._stagedRole = /** @type {ShareRoleEnum} */ (roleSelect.value);
        });

        // ── Expiry chip ──────────────────────────────────────────────────────
        const expiryChip = this._buildExpiryChip(null, (v) => {
            this._stagedExpiry = v;
        });

        // ── Add button ───────────────────────────────────────────────────────
        const addBtn = document.createElement('button');
        addBtn.className = 'smd-add-btn btn btn-secondary';
        addBtn.textContent = i18n.t('actions.add', 'Add');
        addBtn.disabled = true;

        // Search debounce
        /** @type {ReturnType<typeof setTimeout>|null} */
        let debounce = null;

        input.addEventListener('input', () => {
            if (debounce) clearTimeout(debounce);
            const q = input.value.trim();
            if (!q) {
                dropdown.classList.add('hidden');
                dropdown.replaceChildren();
                return;
            }
            debounce = setTimeout(async () => {
                // Search contacts (users) and ReBAC subject groups in parallel.
                // Group results are tagged with `_kind='group'` so the rest of
                // the dialog can render and commit them as group subjects.
                const [contacts, groupItems] = await Promise.all([addressBook.searchContacts(q, [SYSTEM_BOOK_ID]), _searchGroups(q)]);
                // Filter out the currently logged-in user — they cannot share with themselves
                const currentUserId = (() => {
                    try {
                        return /** @type {{id?:string}} */ (JSON.parse(localStorage.getItem('oxicloud_user') ?? '{}'))?.id ?? null;
                    } catch {
                        return null;
                    }
                })();
                const filtered = currentUserId ? contacts.filter((c) => c.id !== currentUserId) : contacts;
                // Groups first (they're a smaller, distinctively-iconed set),
                // then contacts. Cap at 8 combined.
                const combined = [...groupItems, ...filtered].slice(0, 8);
                this._renderSuggestions(dropdown, combined, (item) => {
                    this._stageUser(item, input, dropdown, addBtn);
                });
            }, 200);
        });

        // Close dropdown on click outside
        document.addEventListener(
            'click',
            (e) => {
                if (!wrap.contains(/** @type {Node} */ (e.target))) {
                    dropdown.classList.add('hidden');
                }
            },
            { once: false }
        );

        addBtn.addEventListener('click', () => {
            if (this._stagedUsers.length === 0) return;
            this._commitStagedUsers();
            addBtn.disabled = true;
        });

        row.appendChild(wrap);
        row.appendChild(roleSelect);
        row.appendChild(expiryChip);
        row.appendChild(addBtn);

        return row;
    },

    /**
     * @param {HTMLElement}                                          container
     * @param {Array<ContactItem | GroupSuggestion>}                 results
     * @param {(c: ContactItem | GroupSuggestion) => void}           onSelect
     */
    _renderSuggestions(container, results, onSelect) {
        container.replaceChildren();
        if (results.length === 0) {
            container.classList.add('hidden');
            return;
        }
        results.forEach((c) => {
            const item = document.createElement('div');
            item.className = 'smd-suggestion-item';
            item.tabIndex = 0;

            if (c._kind === 'group') {
                const g = /** @type {GroupSuggestion} */ (c);
                item.appendChild(createGroupVignette(groupDisplayName(g), 'sm', { icon: groupIconClass(g) }));
            } else {
                item.appendChild(createUserVignette(c.id, 'sm', { showEmail: true }));
            }

            const select = () => onSelect(c);
            item.addEventListener('click', select);
            item.addEventListener('keydown', (e) => {
                if (e.key === 'Enter') select();
            });
            container.appendChild(item);
        });
        container.classList.remove('hidden');
    },

    /**
     * @param {ContactItem | GroupSuggestion} contact
     * @param {HTMLInputElement}              inputEl
     * @param {HTMLElement}                   dropdown
     * @param {HTMLButtonElement}             addBtn
     */
    _stageUser(contact, inputEl, dropdown, addBtn) {
        // Idempotent: skip duplicates and already-existing members. Match on
        // id *and* kind so a user and a group sharing a UUID collision (in
        // theory impossible; in practice harmless) wouldn't shadow each other.
        const kind = contact._kind === 'group' ? 'group' : 'user';
        const alreadyMember = this._localMembers.some((m) => m.grant.subject.id === contact.id && m.grant.subject.type === kind && m._op !== 'remove');
        const alreadyStaged = this._stagedUsers.some((u) => u.id === contact.id && (u._kind ?? 'user') === kind);
        if (alreadyMember || alreadyStaged) return;

        this._stagedUsers.push(contact);
        this._refreshChips();
        addBtn.disabled = false;

        inputEl.value = '';
        dropdown.classList.add('hidden');
        dropdown.replaceChildren();
    },

    /**
     * @returns {HTMLElement}
     */
    _buildChipsRow() {
        const row = document.createElement('div');
        row.id = 'smd-chips-row';
        row.className = 'smd-chips';
        this._renderChipsInto(row);
        return row;
    },

    _refreshChips() {
        const row = /** @type {HTMLElement|null} */ (document.getElementById('smd-chips-row'));
        if (row) this._renderChipsInto(row);
    },

    /**
     * @param {HTMLElement} container
     */
    _renderChipsInto(container) {
        container.replaceChildren();
        this._stagedUsers.forEach((c) => {
            const chip = document.createElement('div');
            chip.className = 'smd-chip';

            const visual =
                c._kind === 'group'
                    ? (() => {
                          const g = /** @type {GroupSuggestion} */ (c);
                          return createGroupVignette(groupDisplayName(g), 'xs', { icon: groupIconClass(g) });
                      })()
                    : createUserVignette(c.id, 'xs');

            const rm = document.createElement('button');
            rm.className = 'smd-chip-remove';
            rm.innerHTML = '&times;';
            rm.title = i18n.t('actions.remove', 'Remove');
            const kind = c._kind === 'group' ? 'group' : 'user';
            rm.addEventListener('click', () => {
                this._stagedUsers = this._stagedUsers.filter((u) => !(u.id === c.id && (u._kind ?? 'user') === kind));
                this._refreshChips();
                const addBtn = /** @type {HTMLButtonElement|null} */ (document.querySelector('.smd-add-btn'));
                if (addBtn) addBtn.disabled = this._stagedUsers.length === 0;
            });

            chip.appendChild(visual);
            chip.appendChild(rm);
            container.appendChild(chip);
        });
    },

    _commitStagedUsers() {
        for (const contact of this._stagedUsers) {
            const subjectType = contact._kind === 'group' ? 'group' : 'user';
            /** @type {Grant} */
            const placeholderGrant = {
                id: '', // not yet persisted
                granted_at: '',
                granted_by: '',
                subject: { type: subjectType, id: contact.id },
                permission: /** @type {import('../core/types.js').PermissionTypeEnum} */ (ROLE_PERMISSIONS[this._stagedRole][0]),
                resource: { type: this._itemType, id: this._item?.id ?? '' }
            };
            this._localMembers.push({
                grant: placeholderGrant,
                _grants: [], // no server grants yet — nothing to revoke on remove
                role: this._stagedRole,
                _op: 'new',
                expires_at: this._stagedExpiry,
                _displayName: contact._kind === 'group' ? /** @type {GroupSuggestion} */ (contact).name : undefined
            });
        }
        this._stagedUsers = [];
        this._refreshChips();
        this._refreshMemberGroups();
    },

    /**
     * @returns {HTMLElement}
     */
    _buildMemberGroups() {
        const container = document.createElement('div');
        container.id = 'smd-member-groups';
        this._renderMemberGroupsInto(container);
        return container;
    },

    _refreshMemberGroups() {
        const container = /** @type {HTMLElement|null} */ (document.getElementById('smd-member-groups'));
        if (container) this._renderMemberGroupsInto(container);
        this._syncApplyBtn();
    },

    /**
     * @param {HTMLElement} container
     */
    _renderMemberGroupsInto(container) {
        container.replaceChildren();
        // Highest-privilege role first ("Can manage" → "Can edit" → "Can view"),
        // matching the UX contract and the kebab-menu / role-select dropdown
        // order. Renaming the labels from "Manager"/"Editor"/"Viewer" to
        // "Can manage"/"Can edit"/"Can view" left this iteration order stale.
        const groups = /** @type {ShareRoleEnum[]} */ (['admin', 'editor', 'viewer']);
        let memberIndex = 0;

        for (const role of groups) {
            const visible = this._localMembers.filter((m) => m.role === role && m._op !== 'remove');
            if (visible.length === 0) continue;

            const group = document.createElement('div');
            group.className = 'smd-group';

            const header = document.createElement('div');
            header.className = 'smd-group-header';

            const labelMap = {
                admin: i18n.t('share.role.canManage', 'Can manage'),
                editor: i18n.t('share.role.canEdit', 'Can edit'),
                viewer: i18n.t('share.role.canView', 'Can view')
            };
            const badge = document.createElement('span');
            badge.className = 'smd-group-badge';
            badge.textContent = String(visible.length);
            header.textContent = labelMap[role];
            header.appendChild(badge);
            group.appendChild(header);

            for (const entry of visible) {
                group.appendChild(this._buildMemberRow(entry, memberIndex));
                memberIndex++;
            }
            container.appendChild(group);
        }
    },

    /**
     * @param {MemberEntry} entry
     * @param {number}      _idx  (unused — color is now derived deterministically from userId)
     * @returns {HTMLElement}
     */
    _buildMemberRow(entry, _idx) {
        const row = document.createElement('div');
        row.className = 'smd-member-row';

        const vignette =
            entry.grant.subject.type === 'group'
                ? createGroupVignette(entry._displayName ?? entry.grant.subject.id, 'md', {
                      icon: groupIconClassByVirtual(entry._isVirtual)
                  })
                : createUserVignette(entry.grant.subject.id, 'md');

        const roleSelect = document.createElement('select');
        roleSelect.className = 'smd-member-role-select';
        for (const [val, label] of [
            ['viewer', i18n.t('share.role.canView', 'Can view')],
            ['editor', i18n.t('share.role.canEdit', 'Can edit')],
            ['admin', i18n.t('share.role.canManage', 'Can manage')]
        ]) {
            const opt = document.createElement('option');
            opt.value = val;
            opt.textContent = label;
            if (val === entry.role) opt.selected = true;
            roleSelect.appendChild(opt);
        }
        roleSelect.addEventListener('change', () => {
            const newRole = /** @type {ShareRoleEnum} */ (roleSelect.value);
            entry.role = newRole;
            entry._op = entry._op === 'new' ? 'new' : 'change';
            this._refreshMemberGroups();
        });

        // ── Expiry chip ──────────────────────────────────────────────────────
        // Initialise entry.expires_at once from the representative grant so that
        // role-only changes preserve the current expiry across row rebuilds.
        if (!Object.hasOwn(entry, 'expires_at')) {
            const raw = entry.grant.expires_at ?? null;
            entry.expires_at = raw ? String(raw).slice(0, 10) : null;
        }
        const expiryChip = this._buildExpiryChip(entry.expires_at, (v) => {
            entry.expires_at = v;
            if (entry._op !== 'new') entry._op = 'change';
            this._syncApplyBtn();
        });

        const removeBtn = document.createElement('button');
        removeBtn.className = 'smd-row-action';
        removeBtn.title = i18n.t('actions.remove', 'Remove');
        removeBtn.innerHTML = '<i class="fas fa-times"></i>';
        removeBtn.addEventListener('click', () => {
            entry._op = 'remove';
            this._refreshMemberGroups();
        });

        row.appendChild(vignette);
        row.appendChild(roleSelect);
        row.appendChild(expiryChip);
        row.appendChild(removeBtn);
        return row;
    },

    // ── Expiry chip toggle ─────────────────────────────────────────────────────

    /**
     * @param {string|null} initialValue  - YYYY-MM-DD or null
     * @param {(v: string|null) => void}  onChange
     * @returns {HTMLElement}
     */
    _buildExpiryChip(initialValue, onChange) {
        return buildExpiryChip(initialValue, onChange);
    },

    /**
     * @param {boolean}              initialHasPassword
     * @param {(v: string) => void}  onChange  '' = remove / clear, non-empty = set new password
     * @returns {HTMLElement}
     */
    _buildPasswordChip(initialHasPassword, onChange) {
        return buildPasswordChip(initialHasPassword, onChange);
    },

    // ── Links section ──────────────────────────────────────────────────────────

    /**
     * @returns {HTMLElement}
     */
    _buildLinksSection() {
        const section = document.createElement('div');
        section.className = 'smd-section';

        const title = document.createElement('div');
        title.className = 'smd-section-title';
        title.textContent = i18n.t('share.publicLinks', 'Public links');
        section.appendChild(title);

        section.appendChild(this._buildAddLinkRow());

        const listEl = document.createElement('div');
        listEl.id = 'smd-links-list';
        this._renderLinksInto(listEl);
        section.appendChild(listEl);

        return section;
    },

    /**
     * Always-visible add-link row — mirrors the People search row layout.
     * Rebuilds itself after each Add to reset chip state.
     * @returns {HTMLElement}
     */
    _buildAddLinkRow() {
        const row = document.createElement('div');
        row.className = 'smd-search-row';
        row.id = 'smd-add-link-row';

        // Name input — wrapped in smd-search-wrap so it inherits flex:1
        const wrap = document.createElement('div');
        wrap.className = 'smd-search-wrap';
        const nameInput = document.createElement('input');
        nameInput.type = 'text';
        nameInput.className = 'smd-search-input';
        nameInput.placeholder = i18n.t('share.linkNamePlaceholder', 'Link name (optional)');
        wrap.appendChild(nameInput);

        /** @type {string|null} */
        let stagedPassword = null;
        /** @type {string|null} */
        let stagedExpiry = null;

        const pwChip = this._buildPasswordChip(false, (v) => {
            stagedPassword = v || null;
        });

        const expChip = this._buildExpiryChip(null, (v) => {
            stagedExpiry = v;
        });

        const addBtn = document.createElement('button');
        addBtn.className = 'smd-add-btn btn btn-secondary';
        addBtn.textContent = i18n.t('actions.add', 'Add');

        addBtn.addEventListener('click', () => {
            /** @type {DraftLink} */
            const draft = {
                name: nameInput.value.trim(),
                password: stagedPassword,
                expires_at: stagedExpiry
            };
            this._newLinks.push(draft);
            this._refreshLinks();
            // Reset row (also resets chips via closure state)
            const fresh = this._buildAddLinkRow();
            row.replaceWith(fresh);
        });

        row.appendChild(wrap);
        row.appendChild(pwChip);
        row.appendChild(expChip);
        row.appendChild(addBtn);

        return row;
    },

    /**
     * @param {HTMLElement} container
     */
    _renderLinksInto(container) {
        container.replaceChildren();

        // Existing links
        for (const entry of this._localLinks.filter((e) => e._op !== 'remove')) {
            container.appendChild(this._buildLinkRow(entry));
        }

        // Draft (new) links
        for (const draft of this._newLinks) {
            container.appendChild(this._buildDraftLinkRow(draft));
        }
    },

    _refreshLinks() {
        const container = /** @type {HTMLElement|null} */ (document.getElementById('smd-links-list'));
        if (container) this._renderLinksInto(container);
        this._syncApplyBtn();
    },

    /**
     * @param {LinkEntry} entry
     * @returns {HTMLElement}
     */
    _buildLinkRow(entry) {
        const share = entry.share;

        const ensureDraft = () => {
            if (!entry._draft) {
                entry._draft = {
                    name: share.item_name || '',
                    password: null,
                    expires_at: share.expires_at ? new Date(share.expires_at * 1000).toISOString().slice(0, 10) : null
                };
                entry._op = 'edit';
                this._syncApplyBtn();
            }
            return entry._draft;
        };

        // Derive current display values from draft if present, otherwise from share
        const currentHasPassword = entry._draft
            ? entry._draft.password === ''
                ? false
                : entry._draft.password
                  ? true
                  : share.has_password
            : share.has_password;
        const currentExpiry = entry._draft ? entry._draft.expires_at : share.expires_at ? new Date(share.expires_at * 1000).toISOString().slice(0, 10) : null;

        const row = document.createElement('div');
        row.className = 'smd-link-row';

        const name = document.createElement('div');
        name.className = 'smd-link-name';
        name.textContent = entry._draft?.name || share.item_name || i18n.t('share.sharedLink', 'Shared link');

        const copyBtn = document.createElement('button');
        copyBtn.className = 'smd-row-action';
        copyBtn.title = i18n.t('actions.copy', 'Copy link');
        copyBtn.innerHTML = '<i class="fas fa-copy"></i>';
        copyBtn.addEventListener('click', () => fileSharing.copyLinkToClipboard(share.url));

        const pwChip = this._buildPasswordChip(currentHasPassword, (v) => {
            ensureDraft().password = v;
        });

        const expChip = this._buildExpiryChip(currentExpiry, (v) => {
            ensureDraft().expires_at = v;
        });

        const delBtn = document.createElement('button');
        delBtn.className = 'smd-row-action';
        delBtn.title = i18n.t('actions.delete', 'Delete');
        delBtn.innerHTML = '<i class="fas fa-times"></i>';
        delBtn.addEventListener('click', () => {
            entry._op = 'remove';
            this._refreshLinks();
        });

        row.appendChild(name);
        row.appendChild(copyBtn);
        row.appendChild(pwChip);
        row.appendChild(expChip);
        row.appendChild(delBtn);
        return row;
    },

    /**
     * @param {DraftLink} draft
     * @returns {HTMLElement}
     */
    _buildDraftLinkRow(draft) {
        const row = document.createElement('div');
        row.className = 'smd-link-row';

        const name = document.createElement('div');
        name.className = 'smd-link-name';
        name.textContent = draft.name || i18n.t('share.newLink', 'New link');

        const pending = document.createElement('span');
        pending.className = 'smd-link-tag';
        pending.textContent = i18n.t('share.pending', 'Pending');

        const pwChip = this._buildPasswordChip(!!draft.password, (v) => {
            draft.password = v || null;
        });

        const expChip = this._buildExpiryChip(draft.expires_at, (v) => {
            draft.expires_at = v;
        });

        const delBtn = document.createElement('button');
        delBtn.className = 'smd-row-action';
        delBtn.title = i18n.t('actions.remove', 'Remove');
        delBtn.innerHTML = '<i class="fas fa-times"></i>';
        delBtn.addEventListener('click', () => {
            this._newLinks = this._newLinks.filter((d) => d !== draft);
            this._refreshLinks();
        });

        row.appendChild(name);
        row.appendChild(pending);
        row.appendChild(pwChip);
        row.appendChild(expChip);
        row.appendChild(delBtn);
        return row;
    },

    // ── Apply ──────────────────────────────────────────────────────────────────

    /**
     * Commit all pending local operations to the server, then close.
     * @returns {Promise<void>}
     */
    async _applyAll() {
        if (!this._item) return;

        // Disable the Apply button while working
        if (Modal.confirmBtn) Modal.confirmBtn.disabled = true;

        const item = this._item;
        const itemType = this._itemType;

        try {
            // ── Grants ─────────────────────────────────────────────────────────
            for (const m of this._localMembers) {
                // Convert YYYY-MM-DD from date input to ISO-8601 datetime (midnight UTC).
                const expiresIso = m.expires_at ? new Date(`${m.expires_at}T00:00:00Z`).toISOString() : null;
                if (m._op === 'remove') {
                    // Revoke every individual grant for this subject (one per permission).
                    for (const g of m._grants) {
                        if (g.id) await grants.revokeGrant(g.id);
                    }
                } else if (m._op === 'change' && m.grant.id) {
                    await grants.updateRole({
                        subject: { type: m.grant.subject.type, id: m.grant.subject.id },
                        resource: { type: itemType, id: item.id },
                        role: m.role,
                        expires_at: expiresIso
                    });
                } else if (m._op === 'new') {
                    await grants.createGrant({
                        subject: { type: m.grant.subject.type, id: m.grant.subject.id },
                        resource: { type: itemType, id: item.id },
                        role: m.role,
                        expires_at: expiresIso
                    });
                }
            }

            // ── Links ──────────────────────────────────────────────────────────
            for (const e of this._localLinks) {
                if (e._op === 'remove') {
                    await fileSharing.removeSharedLink(e.share.id);
                } else if (e._op === 'edit' && e._draft) {
                    const expiresTs = e._draft.expires_at ? Math.floor(new Date(e._draft.expires_at).getTime() / 1000) : null;
                    await fileSharing.updateSharedLink(e.share.id, {
                        password: e._draft.password,
                        expires_at: expiresTs
                    });
                }
            }

            for (const draft of this._newLinks) {
                await fileSharing.createSharedLink(
                    item.id,
                    itemType,
                    /** @type {import('../core/types.js').CreateShare} */ ({
                        item_id: item.id,
                        item_name: item.name ?? null,
                        item_type: itemType,
                        password: draft.password,
                        // Pass as ms timestamp so fileSharing's new Date(expires_at) works correctly
                        expires_at: draft.expires_at ? new Date(draft.expires_at).getTime() : null,
                        permissions: { read: true, write: false, reshare: false }
                    })
                );
            }

            // ── Refresh badge cache ────────────────────────────────────────────
            await grants.fetchOutgoingGrants();

            const hasAnyShare =
                this._localMembers.some((m) => m._op !== 'remove') || this._localLinks.some((e) => e._op !== 'remove') || this._newLinks.length > 0;

            ui.setSharedVisualState(item.id, itemType, hasAnyShare);

            Modal.close(true);
            this._onApplied?.();
        } catch (err) {
            console.error('shareModal._applyAll error:', err);
            if (Modal.confirmBtn) Modal.confirmBtn.disabled = false;
        }
    }
};

export { shareModal };
