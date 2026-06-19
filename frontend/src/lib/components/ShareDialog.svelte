<script lang="ts">
	import { errorToast } from '$lib/utils/errors';
	import {
		copyShareLink,
		createShare,
		deleteShare,
		listSharesForItem,
		updateShare
	} from '$lib/api/endpoints/shares';
	import {
		createGrant,
		expiryToIso,
		displayRole,
		fetchGrantsForResource,
		notifyGrantRecipient,
		revokeGrant,
		updateGrantRole,
		type Grant,
		type GrantSubject,
		type GrantSubjectInput,
		type NotifyOutcome,
		type ShareRole
	} from '$lib/api/endpoints/grants';
	import {
		ensureResolvers,
		isDirectoryAvailable,
		resolveRecipient,
		searchRecipients,
		type Recipient
	} from '$lib/api/endpoints/recipients';
	import type { ItemType, ShareItem } from '$lib/api/types';
	import Icon from '$lib/icons/Icon.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import UserVignette from '$lib/components/UserVignette.svelte';
	import { t } from '$lib/i18n/index.svelte';
	import { ui } from '$lib/stores/ui.svelte';

	interface Target {
		id: string;
		name: string;
		kind: ItemType;
	}

	interface Props {
		open: boolean;
		item: Target | null;
		/** Fired with the item id when an outgoing share (grant or link) is created. */
		onshared?: (id: string) => void;
	}

	let { open = $bindable(false), item, onshared }: Props = $props();

	let tab = $state<'people' | 'link'>('people');
	let directoryAvailable = $state(true);

	const ROLES: { v: ShareRole; l: string; icon: string }[] = [
		{ v: 'owner', l: t('share.role.canManage', 'Can manage'), icon: 'crown' },
		{ v: 'editor', l: t('share.role.canEdit', 'Can edit'), icon: 'pencil-alt' },
		{ v: 'viewer', l: t('share.role.canView', 'Can view'), icon: 'eye' }
	];
	const ROLE_ORDER: ShareRole[] = ['owner', 'editor', 'viewer'];
	function roleLabel(r: ShareRole): string {
		return ROLES.find((x) => x.v === r)?.l ?? r;
	}
	function roleIcon(r: ShareRole): string {
		return ROLES.find((x) => x.v === r)?.icon ?? 'eye';
	}

	// ── People / grants ──────────────────────────────────────────────────────
	interface Member {
		subject: GrantSubject;
		recipient: Recipient;
		role: ShareRole;
		grantIds: string[];
		/** Representative grant id for notify (any grant on this subject). */
		notifyGrantId?: string;
		expiry: string | null; // YYYY-MM-DD or null
	}
	let members = $state<Member[]>([]);
	let grantsLoading = $state(false);
	let query = $state('');
	let results = $state<Recipient[]>([]);
	let newRole = $state<ShareRole>('viewer');
	let newExpiry = $state<string | null>(null);
	let searchTimer: ReturnType<typeof setTimeout> | null = null;

	function isoToDate(iso: string | null | undefined): string | null {
		return iso ? String(iso).slice(0, 10) : null;
	}

	function groupGrants(grants: Grant[]): Member[] {
		const bySubject = new Map<
			string,
			{ subject: GrantSubject; role: ShareRole; ids: string[]; expiry: string | null }
		>();
		for (const g of grants) {
			if (g.subject.type === 'token') continue;
			const key = `${g.subject.type}:${g.subject.id}`;
			const entry = bySubject.get(key) ?? {
				subject: g.subject,
				role: 'viewer' as ShareRole,
				ids: [],
				expiry: null
			};
			// Role-grants emit one row per (subject, resource), so the row's role
			// is the subject's role directly.
			entry.role = displayRole(g.role);
			entry.ids.push(g.id);
			if (g.expires_at && !entry.expiry) entry.expiry = isoToDate(g.expires_at);
			bySubject.set(key, entry);
		}
		return [...bySubject.values()].map((e) => ({
			subject: e.subject,
			recipient: resolveRecipient(e.subject.type as 'user' | 'group', e.subject.id),
			role: e.role,
			grantIds: e.ids,
			notifyGrantId: e.ids[0],
			expiry: e.expiry
		}));
	}

	async function loadGrants() {
		if (!item) return;
		grantsLoading = true;
		try {
			await ensureResolvers();
			directoryAvailable = isDirectoryAvailable();
			members = groupGrants(await fetchGrantsForResource(item.kind, item.id));
		} catch (e) {
			errorToast(e);
		} finally {
			grantsLoading = false;
		}
	}

	function onQueryInput() {
		if (searchTimer) clearTimeout(searchTimer);
		searchTimer = setTimeout(async () => {
			const existing = new Set(members.map((m) => `${m.subject.type}:${m.subject.id}`));
			results = (await searchRecipients(query)).filter(
				(r) => !existing.has(`${r.type === 'email' ? 'user' : r.type}:${r.id}`)
			);
		}, 200);
	}

	function subjectInput(r: Recipient): GrantSubjectInput {
		if (r.type === 'email') return { type: 'email', email: r.id };
		return { type: r.type, id: r.id };
	}

	async function addRecipient(r: Recipient) {
		if (!item) return;
		try {
			const res = await createGrant(
				subjectInput(r),
				{ type: item.kind, id: item.id },
				newRole,
				expiryToIso(newExpiry)
			);
			query = '';
			results = [];
			summarizeNotifications(res.notification.outcomes);
			onshared?.(item.id);
			await loadGrants();
		} catch (e) {
			errorToast(e);
		}
	}

	async function changeRole(m: Member, role: ShareRole) {
		if (!item || role === m.role) return;
		try {
			await updateGrantRole(
				m.subject,
				{ type: item.kind, id: item.id },
				role,
				expiryToIso(m.expiry)
			);
			await loadGrants();
		} catch (e) {
			errorToast(e);
		}
	}

	async function changeMemberExpiry(m: Member, expiry: string | null) {
		if (!item) return;
		try {
			await updateGrantRole(
				m.subject,
				{ type: item.kind, id: item.id },
				m.role,
				expiryToIso(expiry)
			);
			await loadGrants();
		} catch (e) {
			errorToast(e);
		}
	}

	async function removeMember(m: Member) {
		try {
			for (const id of m.grantIds) await revokeGrant(id);
			await loadGrants();
		} catch (e) {
			errorToast(e);
		}
	}

	async function notifyMember(m: Member) {
		if (!m.notifyGrantId) return;
		try {
			const set = await notifyGrantRecipient(m.notifyGrantId);
			summarizeNotifications(set.outcomes);
		} catch (e) {
			errorToast(e);
		}
	}

	/** Aggregate notification outcomes into a single toast (mirrors OLD _surfaceNotifySummary). */
	function summarizeNotifications(outcomes: NotifyOutcome[]) {
		if (!outcomes || outcomes.length === 0) return;
		const sent = outcomes.filter((o) => o.kind === 'sent').length;
		const coalesced = outcomes.filter((o) => o.kind === 'coalesced').length;
		const rateLimited = outcomes.filter((o) => o.kind === 'rate_limited').length;
		const skipped = outcomes.filter((o) => o.kind === 'not_applicable').length;
		const lines: string[] = [];
		if (sent > 0) lines.push(t('share.notify.sent', { n: sent }, '{{n}} notified by email.'));
		if (coalesced > 0)
			lines.push(t('share.notify.coalesced', { n: coalesced }, '{{n}} already notified recently.'));
		if (rateLimited > 0)
			lines.push(
				t('share.notify.rateLimited', { n: rateLimited }, '{{n}} hit the rate limit — try later.')
			);
		if (skipped > 0)
			lines.push(
				t('share.notify.skipped', { n: skipped }, '{{n}} skipped (no email / opted out).')
			);
		if (lines.length === 0) return;
		const onlySent = coalesced === 0 && rateLimited === 0 && skipped === 0;
		ui.notify(lines.join(' '), onlySent ? 'success' : 'info');
	}

	// Members grouped by role, highest privilege first.
	const memberGroups = $derived(
		ROLE_ORDER.map((role) => ({
			role,
			members: members.filter((m) => m.role === role)
		})).filter((g) => g.members.length > 0)
	);

	// ── Public link ──────────────────────────────────────────────────────────
	let shares = $state<ShareItem[]>([]);
	let linkLoading = $state(false);
	let creating = $state(false);
	let newLinkName = $state('');
	let password = $state('');
	let expiresAt = $state<string | null>(null);

	async function loadShares() {
		if (!item) return;
		linkLoading = true;
		try {
			shares = await listSharesForItem(item.id, item.kind);
		} catch (e) {
			errorToast(e);
		} finally {
			linkLoading = false;
		}
	}

	async function createLink() {
		if (!item) return;
		creating = true;
		try {
			await createShare({
				itemId: item.id,
				itemName: newLinkName.trim() || item.name,
				itemType: item.kind,
				password: password || null,
				expiresAt: expiresAt || null
			});
			newLinkName = '';
			password = '';
			expiresAt = null;
			onshared?.(item.id);
			await loadShares();
			ui.notify(t('share.created', 'Public link created'), 'success');
		} catch (e) {
			errorToast(e);
		} finally {
			creating = false;
		}
	}

	async function editLinkExpiry(share: ShareItem, expiry: string | null) {
		try {
			await updateShare(share.id, { expiresAt: expiry });
			await loadShares();
		} catch (e) {
			errorToast(e);
		}
	}

	async function editLinkPassword(share: ShareItem, pw: string | null) {
		try {
			await updateShare(share.id, { password: pw });
			await loadShares();
			ui.notify(
				pw
					? t('share.password_set', 'Password updated')
					: t('share.password_cleared', 'Password removed'),
				'success'
			);
		} catch (e) {
			errorToast(e);
		}
	}

	async function removeLink(share: ShareItem) {
		try {
			await deleteShare(share.id);
			shares = shares.filter((s) => s.id !== share.id);
		} catch (e) {
			errorToast(e);
		}
	}

	async function copy(url: string) {
		if (await copyShareLink(url)) ui.notify(t('share.copied', 'Link copied'), 'success');
		else ui.notify(t('share.copy_failed', 'Could not copy link'), 'error');
	}

	function shareExpiryIso(s: ShareItem): string | null {
		return s.expires_at ? new Date(s.expires_at * 1000).toISOString().slice(0, 10) : null;
	}

	$effect(() => {
		if (open && item) {
			void loadGrants();
			void loadShares();
		}
	});
</script>

<!-- ── Reusable expiry chip ─────────────────────────────────────────────── -->
{#snippet expiryChip(value: string | null, onchange: (v: string | null) => void)}
	<span class="chip-edit">
		{#if value}
			<input
				class="chip-edit__date"
				type="date"
				value={value ?? ''}
				onchange={(e) => onchange((e.currentTarget as HTMLInputElement).value || null)}
				aria-label={t('share.expiry', 'Expiry')}
			/>
			<button
				class="chip-edit__clear"
				title={t('actions.clear', 'Clear')}
				onclick={() => onchange(null)}
				aria-label={t('actions.clear', 'Clear')}>×</button
			>
		{:else}
			<label class="chip chip--ghost">
				<Icon name="infinity" />
				<span>{t('share.noExpiry', 'No expiry')}</span>
				<input
					class="chip-edit__date chip-edit__date--hidden"
					type="date"
					onchange={(e) => onchange((e.currentTarget as HTMLInputElement).value || null)}
					aria-label={t('share.set_expiry', 'Set expiry')}
				/>
			</label>
		{/if}
	</span>
{/snippet}

<Modal bind:open title={t('share.dialog_title', { name: item?.name ?? '' }, 'Share “{{name}}”')}>
	<div class="tabs" role="tablist">
		<button role="tab" aria-selected={tab === 'people'} onclick={() => (tab = 'people')}>
			{t('share.people', 'People')}
		</button>
		<button role="tab" aria-selected={tab === 'link'} onclick={() => (tab = 'link')}>
			{t('share.public_link', 'Public link')}
		</button>
	</div>

	{#if tab === 'people'}
		{#if !directoryAvailable && !grantsLoading}
			<p class="status status--note">
				{t('share.directoryUnavailable', 'User directory unavailable')}
			</p>
		{:else}
			<div class="add-row">
				<div class="search">
					<input
						placeholder={t('share.add_people', 'Add people, groups, or email…')}
						bind:value={query}
						oninput={onQueryInput}
						autocomplete="off"
					/>
					{#if results.length > 0}
						<ul class="results">
							{#each results as r (r.type + r.id)}
								<li>
									<button class="result" onclick={() => addRecipient(r)}>
										<Icon
											name={r.type === 'group'
												? 'user-group'
												: r.type === 'email'
													? 'envelope'
													: 'user'}
										/>
										<span class="result__label">{r.label}</span>
										{#if r.type === 'email'}
											<span class="result__sub">{t('share.inviteByEmail', 'Invite by email')}</span>
										{:else if r.sublabel}
											<span class="result__sub">{r.sublabel}</span>
										{/if}
									</button>
								</li>
							{/each}
						</ul>
					{/if}
				</div>
				<select class="role-select" bind:value={newRole} aria-label={t('share.role_label', 'Role')}>
					{#each ROLES as r (r.v)}<option value={r.v}>{r.l}</option>{/each}
				</select>
				{@render expiryChip(newExpiry, (v) => (newExpiry = v))}
			</div>
		{/if}

		{#if grantsLoading}
			<div class="skeleton" aria-hidden="true">
				<div class="skeleton__line skeleton__line--short"></div>
				<div class="skeleton__line skeleton__line--medium"></div>
				<div class="skeleton__line"></div>
			</div>
		{:else if members.length === 0}
			<p class="status">{t('share.no_people', 'Not shared with anyone yet.')}</p>
		{:else}
			{#each memberGroups as group (group.role)}
				<div class="member-group">
					<div class="member-group__header">
						<Icon name={roleIcon(group.role)} />
						<span>{roleLabel(group.role)}</span>
						<span class="member-group__badge">{group.members.length}</span>
					</div>
					<ul class="members">
						{#each group.members as m (m.subject.type + m.subject.id)}
							<li
								class="member"
								class:member--expired={m.expiry && new Date(m.expiry) < new Date()}
							>
								{#if m.subject.type === 'user'}
									<UserVignette
										userId={m.subject.id}
										fallbackLabel={m.recipient.label}
										fallbackSublabel={m.recipient.sublabel}
									/>
								{:else}
									<Icon name="user-group" />
									<span class="member__label">
										{m.recipient.label}
										{#if m.recipient.sublabel}<span class="member__sub">{m.recipient.sublabel}</span
											>{/if}
									</span>
								{/if}
								{@render expiryChip(m.expiry, (v) => changeMemberExpiry(m, v))}
								<select
									class="role-select"
									value={m.role}
									onchange={(e) => changeRole(m, e.currentTarget.value as ShareRole)}
								>
									{#each ROLES as r (r.v)}<option value={r.v}>{r.l}</option>{/each}
								</select>
								<button
									class="btn-action"
									title={t('share.notifyByEmail', 'Notify by email')}
									onclick={() => notifyMember(m)}><Icon name="paper-plane" /></button
								>
								<button
									class="btn-action btn-action--delete"
									title={t('share.revoke', 'Remove')}
									onclick={() => removeMember(m)}><Icon name="user-xmark" /></button
								>
							</li>
						{/each}
					</ul>
				</div>
			{/each}
		{/if}
	{:else}
		<section class="sh-create">
			<div class="sh-fields">
				<label>
					<span>{t('share.link_name', 'Link name (optional)')}</span>
					<input type="text" bind:value={newLinkName} autocomplete="off" />
				</label>
				<label>
					<span>{t('share.password_optional', 'Password (optional)')}</span>
					<input type="text" bind:value={password} autocomplete="off" />
				</label>
				<label>
					<span>{t('share.expires_optional', 'Expires (optional)')}</span>
					<input
						type="date"
						value={expiresAt ?? ''}
						onchange={(e) => (expiresAt = e.currentTarget.value || null)}
					/>
				</label>
			</div>
			<button class="btn btn-primary" disabled={creating} onclick={createLink}>
				{t('share.create_link', 'Create link')}
			</button>
		</section>

		{#if linkLoading}
			<div class="skeleton" aria-hidden="true">
				<div class="skeleton__line skeleton__line--medium"></div>
				<div class="skeleton__line"></div>
			</div>
		{:else if shares.length === 0}
			<p class="status">{t('share.none', 'No public links yet.')}</p>
		{:else}
			<ul class="links">
				{#each shares as s (s.id)}
					<li class="link-row">
						<span class="link-row__title">
							<Icon name={s.has_password ? 'lock' : 'link'} />
							<span class="link-row__name"
								>{s.item_name || t('share.sharedLink', 'Shared link')}</span
							>
						</span>
						{@render expiryChip(shareExpiryIso(s), (v) => editLinkExpiry(s, v))}
						<button
							class="btn-action"
							class:btn-action--on={s.has_password}
							title={s.has_password
								? t('share.changePassword', 'Change password')
								: t('share.addPassword', 'Add password')}
							onclick={() => {
								const pw = window.prompt(
									s.has_password
										? t('share.passwordPrompt_clear', 'New password (blank to remove):')
										: t('share.passwordPrompt', 'Set a password:')
								);
								if (pw !== null) editLinkPassword(s, pw || null);
							}}><Icon name={s.has_password ? 'lock' : 'lock-open'} /></button
						>
						<button class="btn-action" title={t('share.copy', 'Copy')} onclick={() => copy(s.url)}>
							<Icon name="copy" />
						</button>
						<button
							class="btn-action btn-action--delete"
							title={t('common.delete', 'Delete')}
							onclick={() => removeLink(s)}><Icon name="trash" /></button
						>
					</li>
				{/each}
			</ul>
		{/if}
	{/if}

	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>
			{t('common.close', 'Close')}
		</button>
	{/snippet}
</Modal>

<style>
	.tabs {
		display: flex;
		gap: var(--space-1);
		border-bottom: 1px solid var(--color-border);
		margin-bottom: var(--space-4);
	}

	.tabs button {
		padding: var(--space-2) var(--space-3);
		border: none;
		background: none;
		color: var(--color-text-muted);
		cursor: pointer;
		border-bottom: 2px solid transparent;
	}

	.tabs button[aria-selected='true'] {
		color: var(--color-text);
		border-bottom-color: var(--color-accent);
	}

	.add-row {
		display: flex;
		gap: var(--space-2);
		margin-bottom: var(--space-3);
		align-items: center;
		flex-wrap: wrap;
	}

	.search {
		position: relative;
		flex: 1;
		min-width: 12rem;
	}

	.search input,
	.role-select,
	.sh-fields input {
		padding: var(--space-2) var(--space-3);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		background: var(--color-bg-input);
		color: var(--color-text);
	}

	.search input {
		width: 100%;
	}

	.results {
		position: absolute;
		left: 0;
		right: 0;
		top: 100%;
		z-index: 10;
		list-style: none;
		margin: var(--space-1) 0 0;
		padding: var(--space-1);
		background: var(--color-bg-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		box-shadow: var(--shadow-lg);
		max-height: 14rem;
		overflow: auto;
	}

	.result {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		width: 100%;
		padding: var(--space-2);
		border: none;
		background: none;
		color: var(--color-text);
		cursor: pointer;
		border-radius: var(--radius-sm);
		text-align: left;
	}

	.result:hover {
		background: var(--color-bg-hover);
	}

	.result__label {
		flex: 1;
	}

	.result__sub {
		color: var(--color-text-muted);
		font-size: var(--text-sm);
	}

	.member-group {
		margin-bottom: var(--space-3);
	}

	.member-group__header {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		font-size: var(--text-sm);
		font-weight: var(--weight-semibold, 600);
		color: var(--color-text-muted);
		margin-bottom: var(--space-2);
	}

	.member-group__badge {
		min-width: 1.25rem;
		text-align: center;
		padding: 0 var(--space-1);
		border-radius: var(--radius-pill, 999px);
		background: var(--color-bg-muted);
		color: var(--color-text-muted);
		font-size: var(--text-xs, 0.75rem);
	}

	.members,
	.links {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
	}

	.member {
		display: flex;
		align-items: center;
		gap: var(--space-2);
	}

	.member--expired {
		opacity: 0.6;
	}

	.member__label {
		flex: 1;
		display: flex;
		flex-direction: column;
		overflow: hidden;
	}

	.member__sub {
		color: var(--color-text-muted);
		font-size: var(--text-sm);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.sh-fields {
		display: flex;
		gap: var(--space-3);
		margin-bottom: var(--space-3);
		flex-wrap: wrap;
	}

	.sh-fields label {
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
		flex: 1;
		min-width: 8rem;
		font-size: var(--text-sm);
	}

	.link-row {
		display: flex;
		align-items: center;
		gap: var(--space-2);
	}

	.link-row__title {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		flex: 1;
		overflow: hidden;
	}

	.link-row__name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.status {
		color: var(--color-text-muted);
		padding: var(--space-3) 0;
	}

	.status--note {
		font-style: italic;
	}

	.btn-action--delete:hover {
		color: var(--color-danger-text);
	}

	.btn-action--on {
		color: var(--color-accent);
	}

	/* ── Expiry chip ─────────────────────────────────────────────────────── */
	.chip-edit {
		display: inline-flex;
		align-items: center;
		gap: var(--space-1);
	}

	.chip {
		display: inline-flex;
		align-items: center;
		gap: var(--space-1);
		padding: var(--space-1) var(--space-2);
		border-radius: var(--radius-pill, 999px);
		border: 1px solid var(--color-border);
		font-size: var(--text-sm);
		color: var(--color-text);
		cursor: pointer;
		position: relative;
	}

	.chip--ghost {
		border-style: dashed;
		color: var(--color-text-muted);
	}

	.chip-edit__date {
		padding: var(--space-1) var(--space-2);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		background: var(--color-bg-input);
		color: var(--color-text);
		font-size: var(--text-sm);
	}

	.chip-edit__date--hidden {
		position: absolute;
		inset: 0;
		opacity: 0;
		cursor: pointer;
	}

	.chip-edit__clear {
		border: none;
		background: none;
		color: var(--color-text-muted);
		cursor: pointer;
		font-size: var(--text-md, 1rem);
		line-height: 1;
	}

	/* ── Loading skeleton ────────────────────────────────────────────────── */
	.skeleton {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		padding: var(--space-3) 0;
	}

	.skeleton__line {
		height: 1rem;
		border-radius: var(--radius-sm);
		background: linear-gradient(
			90deg,
			var(--color-bg-muted) 25%,
			var(--color-bg-hover) 37%,
			var(--color-bg-muted) 63%
		);
		background-size: 400% 100%;
		animation: shimmer 1.4s ease infinite;
	}

	.skeleton__line--short {
		width: 40%;
	}

	.skeleton__line--medium {
		width: 65%;
	}

	@keyframes shimmer {
		0% {
			background-position: 100% 0;
		}

		100% {
			background-position: 0 0;
		}
	}
</style>
