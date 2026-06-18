<script lang="ts">
	import EmptyState from '$lib/components/EmptyState.svelte';
	import { errorMessage, errorToast } from '$lib/utils/errors';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import {
		expiryToIso,
		fetchMyShares,
		notifyGrantRecipient,
		revokeGrant,
		updateGrantRole,
		type NotifyOutcome,
		type OutgoingGrantItem,
		type OutgoingResourceGrant,
		type ShareRole
	} from '$lib/api/endpoints/grants';
	import { copyShareLink, deleteShare, getShareById, updateShare } from '$lib/api/endpoints/shares';
	import { ensureResolvers, resolveLabel } from '$lib/api/endpoints/recipients';
	import { fileInlineUrl } from '$lib/api/endpoints/files';
	import type { FileItem, FolderItem, ItemType } from '$lib/api/types';
	import Icon from '$lib/icons/Icon.svelte';
	import ListToolbar from '$lib/components/ListToolbar.svelte';
	import ShareDialog from '$lib/components/ShareDialog.svelte';
	import { t } from '$lib/i18n/index.svelte';
	import { ui } from '$lib/stores/ui.svelte';
	import { iconNameFromClass } from '$lib/utils/display';

	type GroupBy = 'items' | 'sharedWith';

	const GROUP_BYS: { key: GroupBy; label: string; orderBy: string }[] = [
		{ key: 'items', label: t('groupby.byFiles', 'By files'), orderBy: 'type' },
		{ key: 'sharedWith', label: t('groupby.sharedWith', 'Shared with'), orderBy: 'subject' }
	];

	const ROLES: { v: ShareRole; l: string; icon: string }[] = [
		{ v: 'admin', l: t('share.role.canManage', 'Can manage'), icon: 'crown' },
		{ v: 'editor', l: t('share.role.canEdit', 'Can edit'), icon: 'pencil-alt' },
		{ v: 'viewer', l: t('share.role.canView', 'Can view'), icon: 'eye' }
	];
	function roleMeta(r: string) {
		return ROLES.find((x) => x.v === r) ?? ROLES[2];
	}

	let raw = $state<OutgoingGrantItem[]>([]);
	let cursor = $state<string | undefined>(undefined);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let groupBy = $state<GroupBy>('items');
	let reversed = $state(false);

	// Edit-sharing dialog
	let dialogOpen = $state(false);
	let dialogItem = $state<{ id: string; name: string; kind: ItemType } | null>(null);

	// Open kebab menu, keyed by grant id.
	let menuFor = $state<string | null>(null);

	function expiryTier(iso: string | null | undefined): 'never' | 'active' | 'soon' | 'expired' {
		if (!iso) return 'never';
		const ms = new Date(iso).getTime() - Date.now();
		if (ms < 0) return 'expired';
		if (ms <= 30 * 86_400_000) return 'soon';
		return 'active';
	}
	function expiryLabel(iso: string | null | undefined): string {
		if (!iso) return t('share.noExpiry', 'No expiry');
		const d = new Date(iso);
		if (Number.isNaN(d.getTime())) return '';
		return d.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
	}
	function isoToDate(iso: string | null | undefined): string {
		return iso ? String(iso).slice(0, 10) : '';
	}

	// ── Swimlane assembly ───────────────────────────────────────────────────
	interface Lane {
		key: string;
		header:
			| { kind: 'resource'; item: OutgoingGrantItem }
			| { kind: 'user'; id: string }
			| { kind: 'group'; id: string }
			| { kind: 'linkPublic' }
			| { kind: 'linkPassword' };
		rows: { grant: OutgoingResourceGrant; item: OutgoingGrantItem }[];
	}

	const lanes = $derived.by((): Lane[] => {
		const out: Lane[] = [];
		const byKey = new Map<string, Lane>();
		const ensure = (key: string, header: Lane['header']): Lane => {
			let lane = byKey.get(key);
			if (!lane) {
				lane = { key, header, rows: [] };
				byKey.set(key, lane);
				out.push(lane);
			}
			return lane;
		};
		for (const item of raw) {
			if (groupBy === 'items') {
				const lane = ensure(`resource:${item.resource.id}`, { kind: 'resource', item });
				for (const grant of item.grants) lane.rows.push({ grant, item });
			} else {
				for (const grant of item.grants) {
					let key: string;
					let header: Lane['header'];
					if (grant.subject_type === 'user') {
						key = `user:${grant.subject_id}`;
						header = { kind: 'user', id: grant.subject_id };
					} else if (grant.subject_type === 'group') {
						key = `group:${grant.subject_id}`;
						header = { kind: 'group', id: grant.subject_id };
					} else if (grant.has_password) {
						key = 'links:password';
						header = { kind: 'linkPassword' };
					} else {
						key = 'links:public';
						header = { kind: 'linkPublic' };
					}
					ensure(key, header).rows.push({ grant, item });
				}
			}
		}
		return out;
	});

	function laneTitle(header: Lane['header']): string {
		switch (header.kind) {
			case 'user':
				return resolveLabel('user', header.id);
			case 'group':
				return resolveLabel('group', header.id);
			case 'linkPublic':
				return t('myshares.publicLinks', 'Public links');
			case 'linkPassword':
				return t('myshares.passwordLinks', 'Password-protected links');
			case 'resource':
				return header.item.resource.name;
		}
	}

	// ── Data loading ────────────────────────────────────────────────────────
	async function load(reset = false) {
		loading = true;
		error = null;
		try {
			await ensureResolvers();
			const order = GROUP_BYS.find((g) => g.key === groupBy)?.orderBy;
			const page = await fetchMyShares({
				cursor: reset ? undefined : cursor,
				orderBy: order,
				reverse: reversed
			});
			raw = reset ? page.items : [...raw, ...page.items];
			cursor = page.next_cursor;
		} catch (e) {
			error = errorMessage(e);
		} finally {
			loading = false;
		}
	}

	function reload() {
		cursor = undefined;
		raw = [];
		void load(true);
	}

	function setGroupBy(key: GroupBy) {
		if (groupBy === key) return;
		groupBy = key;
		reload();
	}
	function toggleDirection() {
		reversed = !reversed;
		reload();
	}

	function openResource(item: OutgoingGrantItem) {
		if (item.resource_type === 'folder') goto(`/files/${item.resource.id}`);
		else window.open(fileInlineUrl(item.resource.id), '_blank', 'noopener');
	}

	function editSharing(item: OutgoingGrantItem) {
		dialogItem = { id: item.resource.id, name: item.resource.name, kind: item.resource_type };
		dialogOpen = true;
	}

	function toggleMenu(grantId: string) {
		menuFor = menuFor === grantId ? null : grantId;
	}
	function closeMenu() {
		menuFor = null;
	}

	function summarize(outcomes: NotifyOutcome[]) {
		if (!outcomes || outcomes.length === 0) {
			ui.notify(t('myshares.notifySent', 'Notification sent.'), 'success');
			return;
		}
		const sent = outcomes.filter((o) => o.kind === 'sent').length;
		const coalesced = outcomes.filter((o) => o.kind === 'coalesced').length;
		const rate = outcomes.filter((o) => o.kind === 'rate_limited').length;
		const skipped = outcomes.filter((o) => o.kind === 'not_applicable').length;
		const lines: string[] = [];
		if (sent > 0) lines.push(t('share.notify.sent', { n: sent }, '{{n}} notified by email.'));
		if (coalesced > 0)
			lines.push(t('share.notify.coalesced', { n: coalesced }, '{{n}} already notified recently.'));
		if (rate > 0)
			lines.push(
				t('share.notify.rateLimited', { n: rate }, '{{n}} hit the rate limit — try later.')
			);
		if (skipped > 0)
			lines.push(
				t('share.notify.skipped', { n: skipped }, '{{n}} skipped (no email / opted out).')
			);
		ui.notify(
			lines.join(' ') || t('myshares.notifySent', 'Notification sent.'),
			rate || skipped ? 'info' : 'success'
		);
	}

	// ── Per-grant actions ───────────────────────────────────────────────────
	async function changeRole(g: OutgoingResourceGrant, item: OutgoingGrantItem, role: ShareRole) {
		closeMenu();
		if (g.role === role) return;
		try {
			await updateGrantRole(
				{ type: g.subject_type, id: g.subject_id },
				{ type: item.resource_type, id: item.resource.id },
				role,
				expiryToIso(isoToDate(g.expires_at) || null)
			);
			g.role = role;
			raw = [...raw];
		} catch (e) {
			errorToast(e);
		}
	}

	async function changeExpiry(g: OutgoingResourceGrant, item: OutgoingGrantItem, date: string) {
		try {
			const iso = expiryToIso(date || null);
			await updateGrantRole(
				{ type: g.subject_type, id: g.subject_id },
				{ type: item.resource_type, id: item.resource.id },
				g.role,
				iso
			);
			g.expires_at = iso;
			raw = [...raw];
		} catch (e) {
			errorToast(e);
		}
	}

	async function notify(g: OutgoingResourceGrant) {
		closeMenu();
		try {
			summarize((await notifyGrantRecipient(g.grant_id)).outcomes);
		} catch (e) {
			errorToast(e);
		}
	}

	async function removeAccess(g: OutgoingResourceGrant) {
		closeMenu();
		try {
			await revokeGrant(g.grant_id);
			dropGrant(g.grant_id);
		} catch (e) {
			errorToast(e);
		}
	}

	async function copyLink(g: OutgoingResourceGrant) {
		closeMenu();
		try {
			const share = await getShareById(g.subject_id);
			if (await copyShareLink(share.url)) ui.notify(t('share.copied', 'Link copied'), 'success');
			else ui.notify(t('share.copy_failed', 'Could not copy link'), 'error');
		} catch (e) {
			errorToast(e);
		}
	}

	async function changeLinkExpiry(g: OutgoingResourceGrant, date: string) {
		try {
			await updateShare(g.subject_id, { expiresAt: date || null });
			g.expires_at = expiryToIso(date || null);
			raw = [...raw];
		} catch (e) {
			errorToast(e);
		}
	}

	async function editLinkPassword(g: OutgoingResourceGrant) {
		closeMenu();
		const pw = window.prompt(
			g.has_password
				? t('share.passwordPrompt_clear', 'New password (blank to remove):')
				: t('share.passwordPrompt', 'Set a password:')
		);
		if (pw === null) return;
		try {
			await updateShare(g.subject_id, { password: pw || null });
			g.has_password = !!pw;
			raw = [...raw];
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

	async function deleteLink(g: OutgoingResourceGrant) {
		closeMenu();
		try {
			await deleteShare(g.subject_id);
			dropGrant(g.grant_id);
		} catch (e) {
			errorToast(e);
		}
	}

	/** Remove a grant locally, pruning now-empty resources. */
	function dropGrant(grantId: string) {
		raw = raw
			.map((item) => ({ ...item, grants: item.grants.filter((g) => g.grant_id !== grantId) }))
			.filter((item) => item.grants.length > 0);
	}

	function linkLabel(g: OutgoingResourceGrant): string {
		return `${t('share.link', 'Link')} · …${g.subject_id.slice(-4)} · ${g.subject_display}`;
	}

	function resourceIcon(item: OutgoingGrantItem): string {
		return item.resource_type === 'folder'
			? 'folder'
			: iconNameFromClass((item.resource as FileItem | FolderItem).icon_class);
	}

	const isEmpty = $derived(!loading && raw.length === 0 && !error);

	onMount(() => load(true));
</script>

<svelte:head><title>{t('nav.shared', 'Shared')} · OxiCloud</title></svelte:head>
<svelte:window onclick={() => menuFor && closeMenu()} />

<div class="page-sticky-header">
	<h1 class="page-title">{t('nav.shared', 'Shared')}</h1>
	<ListToolbar
		groups={GROUP_BYS}
		{groupBy}
		{reversed}
		ongroup={(key) => setGroupBy(key as GroupBy)}
		ondirection={toggleDirection}
		showViewToggle={false}
	/>
</div>

{#if error}
	<EmptyState icon="exclamation-circle" title={error} error />
{:else if isEmpty}
	<EmptyState
		icon="share-alt"
		title={t('myshares.emptyStateTitle', "You haven't shared anything yet")}
		hint={t('myshares.emptyStateDesc', 'Items you share with others will appear here')}
	/>
{:else}
	<div class="ms-lanes">
		{#each lanes as lane (lane.key)}
			<section class="ms-lane">
				<header class="ms-lane__header">
					{#if lane.header.kind === 'resource'}
						{@const laneItem = lane.header.item}
						<button class="ms-lane__resource" onclick={() => openResource(laneItem)}>
							<Icon name={resourceIcon(laneItem)} />
							<span class="ms-lane__name">{laneItem.resource.name}</span>
						</button>
						<button class="btn btn-secondary ms-lane__edit" onclick={() => editSharing(laneItem)}>
							<Icon name="pencil-alt" />
							{t('myshares.editSharing', 'Edit sharing')}
						</button>
					{:else}
						<span class="ms-lane__subject">
							<Icon
								name={lane.header.kind === 'user'
									? 'user'
									: lane.header.kind === 'group'
										? 'user-group'
										: lane.header.kind === 'linkPassword'
											? 'lock'
											: 'link'}
							/>
							<span class="ms-lane__name">{laneTitle(lane.header)}</span>
						</span>
					{/if}
				</header>

				<ul class="ms-rows">
					{#each lane.rows as { grant, item } (grant.grant_id)}
						{@const tier = expiryTier(grant.expires_at)}
						<li class="ms-row" class:ms-row--expired={tier === 'expired'}>
							<!-- Identity -->
							<span class="ms-row__identity">
								{#if (grant.subject_type === 'user' || grant.subject_type === 'group') && groupBy === 'sharedWith'}
									<button class="ms-link-btn" onclick={() => openResource(item)}>
										<Icon name={resourceIcon(item)} />
										<span class="ms-row__name">{item.resource.name}</span>
									</button>
								{:else if grant.subject_type === 'user'}
									<Icon name="user" />
									<span class="ms-row__name">{resolveLabel('user', grant.subject_id)}</span>
								{:else if grant.subject_type === 'group'}
									<Icon name="user-group" />
									<span class="ms-row__name">{resolveLabel('group', grant.subject_id)}</span>
								{:else}
									<button
										class="ms-chip ms-chip--link"
										class:ms-chip--locked={grant.has_password}
										onclick={() => copyLink(grant)}
										title={t('share.copyLink', 'Copy link')}
									>
										<Icon name={grant.has_password ? 'lock' : 'link'} />
										<span class="ms-row__name">{linkLabel(grant)}</span>
									</button>
									{#if groupBy === 'sharedWith'}
										<span class="ms-arrow">→</span>
										<button class="ms-link-btn" onclick={() => openResource(item)}>
											<Icon name={resourceIcon(item)} />
											<span>{item.resource.name}</span>
										</button>
									{/if}
								{/if}
							</span>

							<!-- Role pill (not shown for token subjects) -->
							{#if grant.subject_type !== 'token'}
								<span class="ms-role ms-role--{roleMeta(grant.role).v}">
									<Icon name={roleMeta(grant.role).icon} />
									{roleMeta(grant.role).l}
								</span>
							{/if}

							<!-- Expiry chip -->
							<span class="ms-expiry ms-expiry--{tier}" title={expiryLabel(grant.expires_at)}>
								<Icon name={grant.expires_at ? 'clock' : 'infinity'} />
								{expiryLabel(grant.expires_at)}
							</span>

							<!-- Kebab -->
							<div class="ms-kebab">
								<button
									class="btn-icon"
									aria-label={t('myshares.manageAccess', 'Manage access')}
									aria-haspopup="menu"
									aria-expanded={menuFor === grant.grant_id}
									onclick={(e) => {
										e.stopPropagation();
										toggleMenu(grant.grant_id);
									}}><Icon name="ellipsis-v" /></button
								>
								{#if menuFor === grant.grant_id}
									<div
										class="ms-menu"
										role="menu"
										tabindex="-1"
										onclick={(e) => e.stopPropagation()}
										onkeydown={(e) => e.key === 'Escape' && closeMenu()}
									>
										{#if grant.subject_type === 'user' || grant.subject_type === 'group'}
											<button class="ms-menu__item" role="menuitem" onclick={() => notify(grant)}>
												<Icon name="paper-plane" />
												{grant.subject_type === 'group'
													? t('myshares.notifyGroupMembers', 'Notify group members')
													: grant.is_external
														? t('myshares.resendInvitation', 'Resend invitation email')
														: t('myshares.notifyByEmail', 'Notify by email')}
											</button>
											<div class="ms-menu__sep"></div>
											{#each ROLES as r (r.v)}
												<button
													class="ms-menu__item"
													class:ms-menu__item--current={grant.role === r.v}
													role="menuitem"
													onclick={() => changeRole(grant, item, r.v)}
												>
													<Icon name={grant.role === r.v ? 'check' : r.icon} />
													{r.l}
												</button>
											{/each}
											<div class="ms-menu__sep"></div>
											<div class="ms-menu__field">
												<span class="ms-menu__label">{t('share.expiry', 'Expiry')}</span>
												<input
													type="date"
													class="ms-menu__date"
													value={isoToDate(grant.expires_at)}
													onchange={(e) =>
														changeExpiry(grant, item, (e.currentTarget as HTMLInputElement).value)}
												/>
											</div>
											<div class="ms-menu__sep"></div>
											<button
												class="ms-menu__item ms-menu__item--danger"
												role="menuitem"
												onclick={() => removeAccess(grant)}
											>
												<Icon name="user-xmark" />
												{t('myshares.removeAccess', 'Remove access')}
											</button>
										{:else}
											<button class="ms-menu__item" role="menuitem" onclick={() => copyLink(grant)}>
												<Icon name="copy" />
												{t('myshares.copyLink', 'Copy link')}
											</button>
											<div class="ms-menu__sep"></div>
											<div class="ms-menu__field">
												<span class="ms-menu__label">{t('share.expiry', 'Expiry')}</span>
												<input
													type="date"
													class="ms-menu__date"
													value={isoToDate(grant.expires_at)}
													onchange={(e) =>
														changeLinkExpiry(grant, (e.currentTarget as HTMLInputElement).value)}
												/>
											</div>
											<button
												class="ms-menu__item"
												role="menuitem"
												onclick={() => editLinkPassword(grant)}
											>
												<Icon name={grant.has_password ? 'lock' : 'lock-open'} />
												{grant.has_password
													? t('share.changePassword', 'Change password')
													: t('share.addPassword', 'Add password')}
											</button>
											<div class="ms-menu__sep"></div>
											<button
												class="ms-menu__item ms-menu__item--danger"
												role="menuitem"
												onclick={() => deleteLink(grant)}
											>
												<Icon name="trash" />
												{t('myshares.deleteLink', 'Delete link')}
											</button>
										{/if}
									</div>
								{/if}
							</div>
						</li>
					{/each}
				</ul>
			</section>
		{/each}

		{#if cursor}
			<button class="btn btn-secondary ms-more" onclick={() => load(false)} disabled={loading}>
				{loading ? t('common.loading', 'Loading…') : t('common.load_more', 'Load more')}
			</button>
		{/if}
	</div>
{/if}

<ShareDialog bind:open={dialogOpen} item={dialogItem} />

<style>
	.ms-lanes {
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
		padding-top: var(--space-3);
	}

	.ms-lane {
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		overflow: visible;
	}

	.ms-lane__header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-2);
		padding: var(--space-2) var(--space-3);
		border-bottom: 1px solid var(--color-border-faint, var(--color-border));
		background: var(--color-bg-muted);
	}

	.ms-lane__resource,
	.ms-lane__subject {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		font-weight: var(--weight-semibold, 600);
		color: var(--color-text);
		background: none;
		border: none;
		cursor: pointer;
		min-width: 0;
	}

	.ms-lane__subject {
		cursor: default;
	}

	.ms-lane__name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.ms-lane__edit {
		display: inline-flex;
		align-items: center;
		gap: var(--space-1);
		flex: none;
	}

	.ms-rows {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
	}

	.ms-row {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		padding: var(--space-2) var(--space-3);
		border-top: 1px solid var(--color-border-faint, var(--color-border));
	}

	.ms-row:first-child {
		border-top: none;
	}

	.ms-row--expired {
		opacity: 0.6;
	}

	.ms-row__identity {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		flex: 1;
		min-width: 0;
	}

	.ms-row__name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.ms-link-btn,
	.ms-chip {
		display: inline-flex;
		align-items: center;
		gap: var(--space-1);
		background: none;
		border: none;
		color: var(--color-text);
		cursor: pointer;
		min-width: 0;
		padding: 0;
	}

	.ms-chip--link {
		padding: var(--space-1) var(--space-2);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-pill, 999px);
	}

	.ms-chip--locked {
		color: var(--color-accent);
	}

	.ms-arrow {
		color: var(--color-text-muted);
	}

	/* Role pill */
	.ms-role {
		display: inline-flex;
		align-items: center;
		gap: var(--space-1);
		padding: var(--space-1) var(--space-2);
		border-radius: var(--radius-pill, 999px);
		font-size: var(--text-sm);
		background: var(--color-bg-muted);
		color: var(--color-text-secondary);
		flex: none;
	}

	.ms-role--admin {
		background: var(--color-warning-bg, var(--color-bg-muted));
		color: var(--color-warning-text-amber, var(--color-text-secondary));
	}

	.ms-role--editor {
		background: var(--color-accent-bg, var(--color-bg-muted));
		color: var(--color-accent-text, var(--color-text-secondary));
	}

	/* Expiry chip tiers */
	.ms-expiry {
		display: inline-flex;
		align-items: center;
		gap: var(--space-1);
		padding: var(--space-1) var(--space-2);
		border-radius: var(--radius-pill, 999px);
		font-size: var(--text-sm);
		color: var(--color-text-muted);
		background: var(--color-bg-muted);
		flex: none;
	}

	.ms-expiry--soon {
		color: var(--color-warning-text-amber, var(--color-text-secondary));
		background: var(--color-warning-bg, var(--color-bg-muted));
	}

	.ms-expiry--expired {
		color: var(--color-danger-text);
		background: var(--color-danger-bg, var(--color-bg-muted));
	}

	/* Kebab + menu */
	.ms-kebab {
		position: relative;
		flex: none;
	}

	.btn-icon {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 32px;
		height: 32px;
		border: none;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--color-text-secondary);
		cursor: pointer;
	}

	.btn-icon:hover {
		background: var(--color-bg-hover);
	}

	.ms-menu {
		position: absolute;
		right: 0;
		top: 100%;
		z-index: 50;
		min-width: 14rem;
		margin-top: var(--space-1);
		padding: var(--space-1);
		background: var(--color-bg-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		box-shadow: var(--shadow-lg);
	}

	.ms-menu__item {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		width: 100%;
		padding: var(--space-2) var(--space-3);
		border: none;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--color-text);
		text-align: left;
		cursor: pointer;
	}

	.ms-menu__item:hover {
		background: var(--color-bg-hover);
	}

	.ms-menu__item--current {
		font-weight: var(--weight-semibold, 600);
	}

	.ms-menu__item--danger {
		color: var(--color-danger-text);
	}

	.ms-menu__sep {
		height: 1px;
		margin: var(--space-1) 0;
		background: var(--color-border);
	}

	.ms-menu__field {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-2);
		padding: var(--space-2) var(--space-3);
	}

	.ms-menu__label {
		color: var(--color-text-muted);
		font-size: var(--text-sm);
	}

	.ms-menu__date {
		padding: var(--space-1) var(--space-2);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		background: var(--color-bg-input);
		color: var(--color-text);
		font-size: var(--text-sm);
	}

	.ms-more {
		margin: var(--space-3) auto 0;
	}
</style>
