<script lang="ts">
	import { errorMessage } from '$lib/utils/errors';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { onMount } from 'svelte';
	import { dateBucket, resolveOwnerName, typeLabel } from '$lib/api/endpoints/favorites';
	import { fetchSharedWithMe, type IncomingGrantItem } from '$lib/api/endpoints/grants';
	import type { FileItem } from '$lib/api/types';
	import { lazyComponent } from '$lib/composables/lazyComponent.svelte';
	import { useOwnerCache } from '$lib/composables/useOwnerCache.svelte';
	import ResourceList, {
		type GroupByDef,
		type ResourceEntry
	} from '$lib/components/ResourceList.svelte';
	import { t } from '$lib/i18n/index.svelte';

	let raw = $state<IncomingGrantItem[]>([]);
	let cursor = $state<string | undefined>(undefined);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let groupBy = $state<string>('');
	let reversed = $state(false);

	const sharers = useOwnerCache(resolveOwnerName);

	const byId = $derived(new Map(raw.map((it) => [it.resource.id, it])));

	const entries = $derived(
		// Drive resources also surface in `/api/grants/incoming/resources`
		// since the role_grants rewrite, but they don't belong in the
		// file/folder ResourceList — they're reached through the drive
		// picker / breadcrumb. Filter them out here so the row UI keeps
		// its file|folder type contract.
		raw
			.filter((it) => it.resource_type !== 'drive')
			.map((it): ResourceEntry => {
				const isFile = it.resource_type === 'file';
				return {
					id: it.resource.id,
					name: it.resource.name,
					kind: it.resource_type as 'file' | 'folder',
					iconClass: it.resource.icon_class,
					// The sharer becomes the "owner" surface — ResourceList renders
					// `<UserVignette userId>` (avatar / name / external badge),
					// resolved lazily via `/api/users/{id}`. `path` keeps the
					// resource's real location so the row still shows where it
					// lives, not a translated string.
					ownerId: it.granted_by ?? null,
					ownerName: sharers.name(it.granted_by),
					path: it.resource.path,
					size: isFile ? (it.resource as FileItem).size : null,
					date: it.granted_at,
					category: isFile ? it.resource.category : 'Folder'
				};
			})
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
			bucketOf: (e) => e.ownerId ?? null,
			labelOf: (id) => sharers.label(id)
		},
		{
			key: 'type',
			label: t('groupby.type', 'Type'),
			orderBy: 'type',
			bucketOf: (e) => e.category ?? 'other',
			labelOf: (k) => typeLabel(k)
		},
		{
			key: 'sharedAt',
			label: t('groupby.sharedAt', 'Shared date'),
			orderBy: 'granted_at',
			bucketOf: (e) => dateBucket(e.date)
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

	function open(entry: ResourceEntry) {
		if (entry.kind === 'folder') {
			goto(resolve(`/files/${entry.id}`));
			return;
		}
		const item = byId.get(entry.id);
		if (item) {
			viewerFile = item.resource as FileItem;
			viewerOpen = true;
		}
	}

	onMount(() => load(true));
</script>

<svelte:head><title>{t('nav.shared_with_me', 'Shared with me')} · OxiCloud</title></svelte:head>

<ResourceList
	title={t('nav.shared_with_me', 'Shared with me')}
	items={entries}
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
