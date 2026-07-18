<script lang="ts">
	import { errorMessage } from '$lib/utils/errors';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { onMount } from 'svelte';
	import { dateBucket, resolveOwnerName, typeLabel } from '$lib/api/endpoints/favorites';
	import { fetchSharedWithMe, type IncomingGrantItem } from '$lib/api/endpoints/grants';
	import type { FileItem, FolderItem } from '$lib/api/types';
	import { lazyComponent } from '$lib/composables/lazyComponent.svelte';
	import { useOwnerCache } from '$lib/composables/useOwnerCache.svelte';
	import ResourceList, {
		isFile,
		type GroupByDef,
		type ItemContext
	} from '$lib/components/ResourceList.svelte';
	import { t } from '$lib/i18n/index.svelte';
	import { session } from '$lib/stores/session.svelte';

	// External users landing here are the natural audience for the
	// "upgrade to a full account" prompt — they don't own a drive of
	// their own, this view IS their entry point. Internal users don't
	// see the banner even when their /shared-with-me happens to be
	// non-empty (they already have a drive; nothing to upgrade).
	const showUpgradeBanner = $derived(session.isExternalUser);

	let raw = $state<IncomingGrantItem[]>([]);
	let cursor = $state<string | undefined>(undefined);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let groupBy = $state<string>('');
	let reversed = $state(false);

	const sharers = useOwnerCache(resolveOwnerName);

	// Drive resources also surface in `/api/grants/incoming/resources`
	// since the role_grants rewrite, but they don't belong in the
	// file/folder ResourceList — they're reached through the drive
	// picker / breadcrumb. Filter them out so the row UI keeps its
	// file|folder type contract.
	const fileFolderGrants = $derived(raw.filter((it) => it.resource_type !== 'drive'));

	// `granted_at` → `ctx.date`; `granted_by` overrides the owner column
	// so the sharer shows up in the vignette (rather than the resource's
	// intrinsic `created_by`, which is a stranger for grantees).
	const items = $derived(fileFolderGrants.map((it) => it.resource as FileItem | FolderItem));
	const contextMap = $derived(
		new Map<string, ItemContext>(
			fileFolderGrants.map((it) => [
				it.resource.id,
				{ date: it.granted_at, ownerId: it.granted_by ?? null } satisfies ItemContext
			])
		)
	);

	// Server-supported sort_by values (see grant_handler.rs:615):
	//   granted_at, granted_by, name, type
	// The first entry (no `bucketOf`) renders a flat list sorted by name —
	// the A-Z icon flags it as "sort, not group" so users don't read it as
	// a real bucket dimension. The remaining three are honest groupings and
	// get the default layer-group icon.
	const groupBys: GroupByDef[] = [
		{ key: '', label: t('files.name', 'Name'), orderBy: 'name', icon: 'arrow-up-a-z' },
		{
			key: 'sharedBy',
			label: t('groupby.sharedBy', 'Shared by'),
			orderBy: 'granted_by',
			bucketOf: (_item, ctx) => ctx?.ownerId ?? null,
			labelOf: (id) => sharers.label(id)
		},
		{
			key: 'type',
			label: t('groupby.type', 'Type'),
			orderBy: 'type',
			bucketOf: (item) => item.category ?? 'other',
			labelOf: (k) => typeLabel(k)
		},
		{
			key: 'sharedAt',
			label: t('groupby.sharedAt', 'Shared date'),
			orderBy: 'granted_at',
			bucketOf: (_item, ctx) => dateBucket(ctx?.date)
		}
	];

	function orderByForGroup(): string {
		return groupBys.find((g) => g.key === groupBy)?.orderBy ?? 'granted_at';
	}

	async function load(reset = false, orderBy = 'granted_at', rev = reversed) {
		loading = true;
		error = null;
		try {
			const page = await fetchSharedWithMe({
				cursor: reset ? undefined : cursor,
				orderBy,
				reverse: rev
			});
			raw = reset ? page.items : [...raw, ...page.items];
			cursor = page.next_cursor;
			// Warm the sharer-name cache so the "Shared by" group headers
			// show real names instead of UUIDs.
			void sharers.resolve(page.items.map((i) => i.granted_by).filter((id): id is string => !!id));
		} catch (e) {
			error = errorMessage(e);
		} finally {
			loading = false;
		}
	}

	let viewerOpen = $state(false);
	let viewerFile = $state<FileItem | null>(null);

	// The file preview is loaded the first time a file is opened, keeping its
	// module out of this route's initial chunk.
	const fileViewer = lazyComponent(() => import('$lib/components/FileViewer.svelte'));
	$effect(() => {
		if (viewerOpen) void fileViewer.load();
	});

	function open(item: FileItem | FolderItem) {
		if (!isFile(item)) {
			goto(resolve(`/files/${item.id}`));
			return;
		}
		viewerFile = item;
		viewerOpen = true;
	}

	onMount(() => load(true));
</script>

<svelte:head><title>{t('nav.shared_with_me', 'Shared with me')} · OxiCloud</title></svelte:head>

{#if showUpgradeBanner}
	<div class="upgrade-banner" role="region" aria-label={t('upgrade.banner_aria', 'Upgrade prompt')}>
		<div class="upgrade-banner__body">
			<strong>{t('upgrade.banner_title', 'Get your own storage')}</strong>
			<span
				>{t(
					'upgrade.banner_body',
					"You're using a guest account. Upgrade to get a personal drive and start uploading files."
				)}</span
			>
		</div>
		<a
			class="upgrade-banner__cta"
			data-testid="shared-with-me-upgrade-btn"
			href={resolve('/upgrade')}
		>
			{t('upgrade.banner_cta', 'Upgrade')}
		</a>
	</div>
{/if}

<ResourceList
	title={t('nav.shared_with_me', 'Shared with me')}
	{items}
	{contextMap}
	resolveOwnerName={(id) => sharers.name(id)}
	{loading}
	{error}
	emptyText={t('shared_with_me.empty', 'Nothing has been shared with you yet.')}
	hasMore={!!cursor}
	showOwner={true}
	{groupBys}
	bind:groupBy
	bind:reversed
	onloadmore={() => load(false, orderByForGroup())}
	onopen={open}
	onreload={(orderBy, rev) => {
		cursor = undefined;
		load(true, orderBy, rev);
	}}
/>

{#if fileViewer.component}
	{@const FileViewer = fileViewer.component}
	<FileViewer bind:open={viewerOpen} file={viewerFile} />
{/if}

<style>
	.upgrade-banner {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-4);
		padding: var(--space-3) var(--space-4);
		margin-bottom: var(--space-4);
		background: var(--color-surface-raised);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
	}

	.upgrade-banner__body {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		color: var(--color-text);
	}

	.upgrade-banner__body strong {
		font-weight: var(--weight-semibold);
	}

	.upgrade-banner__body span {
		color: var(--color-text-muted);
		font-size: var(--text-sm);
	}

	.upgrade-banner__cta {
		flex-shrink: 0;
		padding: var(--space-2) var(--space-4);
		background: var(--color-accent);
		color: var(--color-accent-contrast);
		text-decoration: none;
		font-weight: var(--weight-medium);
		border-radius: var(--radius-md);
	}

	.upgrade-banner__cta:hover {
		filter: brightness(0.95);
	}

	@media (width <= 600px) {
		.upgrade-banner {
			flex-direction: column;
			align-items: stretch;
		}

		.upgrade-banner__cta {
			text-align: center;
		}
	}
</style>
