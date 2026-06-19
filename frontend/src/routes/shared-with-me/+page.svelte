<script lang="ts">
	import { errorMessage } from '$lib/utils/errors';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { fetchSharedWithMe, type IncomingGrantItem } from '$lib/api/endpoints/grants';
	import type { FileItem } from '$lib/api/types';
	import { lazyComponent } from '$lib/composables/lazyComponent.svelte';
	import ResourceList, { type ResourceEntry } from '$lib/components/ResourceList.svelte';
	import { t } from '$lib/i18n/index.svelte';

	let raw = $state<IncomingGrantItem[]>([]);
	let cursor = $state<string | undefined>(undefined);
	let loading = $state(false);
	let error = $state<string | null>(null);

	const byId = $derived(new Map(raw.map((it) => [it.resource.id, it])));

	const entries = $derived(
		raw.map(
			(it): ResourceEntry => ({
				id: it.resource.id,
				name: it.resource.name,
				kind: it.resource_type,
				iconClass: it.resource.icon_class,
				path: it.granted_by
					? t('shared_with_me.from', { who: it.granted_by }, 'Shared by {{who}}')
					: it.resource.path,
				size: it.resource_type === 'file' ? (it.resource as FileItem).size : null,
				date: it.granted_at
			})
		)
	);

	async function load(reset = false) {
		loading = true;
		error = null;
		try {
			const page = await fetchSharedWithMe({ cursor: reset ? undefined : cursor });
			raw = reset ? page.items : [...raw, ...page.items];
			cursor = page.next_cursor;
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
			goto(`/files/${entry.id}`);
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
	onloadmore={() => load(false)}
	onopen={open}
/>

{#if fileViewer.component}
	{@const FileViewer = fileViewer.component}
	<FileViewer bind:open={viewerOpen} file={viewerFile} />
{/if}
