<script lang="ts">
	import Button from '$lib/components/Button.svelte';
	import { useOwnerCache } from '$lib/composables/useOwnerCache.svelte';
	import { errorToast } from '$lib/utils/errors';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { onMount } from 'svelte';
	import { SvelteSet } from 'svelte/reactivity';
	import { clearRecent, fetchRecentPage, type RecentResourceItem } from '$lib/api/endpoints/recent';
	import {
		addFavorite,
		dateBucket,
		fetchFavoritesPage,
		removeFavorite,
		resolveOwnerName,
		sizeBucket,
		typeLabel
	} from '$lib/api/endpoints/favorites';
	import { fileDownloadUrl, renameFile, deleteFile } from '$lib/api/endpoints/files';
	import { renameFolder, deleteFolder } from '$lib/api/endpoints/folders';
	import type { FileItem, ItemType } from '$lib/api/types';
	import { lazyComponent } from '$lib/composables/lazyComponent.svelte';
	import ResourceList, {
		type ContextAction,
		type GroupByDef,
		type ResourceEntry
	} from '$lib/components/ResourceList.svelte';
	import { confirmDialog, promptDialog } from '$lib/stores/dialogs.svelte';
	import { preferences } from '$lib/stores/preferences.svelte';
	import { filterDotfiles } from '$lib/utils/dotfileFilter';
	import { t } from '$lib/i18n/index.svelte';

	let raw = $state<RecentResourceItem[]>([]);
	let cursor = $state<string | undefined>(undefined);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let groupBy = $state('');
	let reversed = $state(false);
	const owners = useOwnerCache(resolveOwnerName);
	let favoriteIds = $state<Set<string>>(new Set());

	const byId = $derived(new Map(raw.map((it) => [it.resource.id, it])));

	const allEntries = $derived(
		raw.map((it): ResourceEntry => {
			const isFile = it.resource_type === 'file';
			// §14 provenance: Recent's mental model is "who touched this
			// recently", so `updated_by` (the last mutator) is the right
			// signal — distinct from Favorites/Files which use `created_by`.
			const ownerId = it.resource.updated_by ?? null;
			return {
				id: it.resource.id,
				name: it.resource.name,
				kind: it.resource_type,
				iconClass: it.resource.icon_class,
				path: it.resource.path,
				size: isFile ? (it.resource as FileItem).size : null,
				date: it.accessed_at,
				ownerId,
				ownerName: owners.name(ownerId),
				isFavorite: favoriteIds.has(it.resource.id),
				category: isFile ? it.resource.category : 'Folder',
				modifiedAt: it.resource.modified_at
			};
		})
	);
	const entries = $derived(filterDotfiles(allEntries, preferences.hideDotfiles));
	// Count of items suppressed by the dotfile filter — surfaced in
	// the empty-state hint below so a `.eslintrc`-only Recent doesn't
	// read as "nothing here yet".
	const hiddenCount = $derived(preferences.hideDotfiles ? allEntries.length - entries.length : 0);

	const groupBys: GroupByDef[] = [
		{ key: '', label: t('files.name', 'Name'), orderBy: 'name', icon: 'arrow-up-a-z' },
		{
			key: 'owner',
			label: t('groupby.owner', 'Owner'),
			orderBy: 'owner',
			bucketOf: (e) => e.ownerId ?? null,
			labelOf: (id) => owners.label(id)
		},
		{
			key: 'type',
			label: t('groupby.type', 'Type'),
			orderBy: 'type',
			bucketOf: (e) => e.category ?? 'other',
			labelOf: (k) => typeLabel(k)
		},
		{
			key: 'size',
			label: t('groupby.size', 'Size'),
			orderBy: 'size',
			bucketOf: (e) => sizeBucket(e.kind === 'folder' ? null : e.size)
		},
		{
			key: 'accessedAt',
			label: t('groupby.accessedAt', 'Accessed date'),
			orderBy: 'accessed_at',
			bucketOf: (e) => dateBucket(e.date)
		},
		{
			key: 'modifiedAt',
			label: t('groupby.modifiedAt', 'Modified date'),
			orderBy: 'modified_at',
			bucketOf: (e) => dateBucket(e.modifiedAt)
		}
	];

	async function loadFavoriteIds() {
		try {
			const favs = await fetchFavoritesPage({ resourceTypes: ['file', 'folder'] });
			favoriteIds = new Set(favs.items.map((f) => f.resource.id));
		} catch {
			// non-fatal — stars just default to off
		}
	}

	// Recent defaults to most-recently-accessed first (accessed_at DESC).
	async function load(reset = false, orderBy = 'accessed_at', rev = reversed) {
		loading = true;
		error = null;
		try {
			const page = await fetchRecentPage({
				cursor: reset ? undefined : cursor,
				orderBy,
				reverse: rev,
				resourceTypes: ['file', 'folder']
			});
			raw = reset ? page.items : [...raw, ...page.items];
			cursor = page.next_cursor;
			void owners.resolve(page.items.map((i) => i.resource.updated_by));
		} catch (e) {
			console.error('recent: load error', e);
			error = t('errors_loadFailed', 'Failed to load items');
		} finally {
			loading = false;
		}
	}

	function orderByForGroup(): string {
		return groupBys.find((g) => g.key === groupBy)?.orderBy ?? 'accessed_at';
	}

	let viewerOpen = $state(false);
	let viewerFile = $state<FileItem | null>(null);

	// The file preview is loaded the first time a file is opened, keeping its
	// module out of this route's initial chunk.
	const fileViewer = lazyComponent(() => import('$lib/components/FileViewer.svelte'));
	const moveDialog = lazyComponent(() => import('$lib/components/MoveDialog.svelte'));
	const shareDialog = lazyComponent(() => import('$lib/components/ShareDialog.svelte'));
	$effect(() => {
		if (viewerOpen) void fileViewer.load();
		if (moveOpen) void moveDialog.load();
		if (shareOpen) void shareDialog.load();
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

	async function toggleFavorite(entry: ResourceEntry) {
		const isFav = favoriteIds.has(entry.id);
		const next = new SvelteSet(favoriteIds);
		if (isFav) next.delete(entry.id);
		else next.add(entry.id);
		favoriteIds = next;
		try {
			if (isFav) await removeFavorite(entry.kind, entry.id);
			else await addFavorite(entry.kind, entry.id);
		} catch (e) {
			// revert on failure
			favoriteIds = isFav
				? new Set([...favoriteIds, entry.id])
				: new Set([...favoriteIds].filter((id) => id !== entry.id));
			errorToast(e);
		}
	}

	async function clearAll() {
		const ok = await confirmDialog({
			title: t('recent.clear', 'Clear recent'),
			message: t('recent.confirm_clear', 'Clear your recent items?'),
			confirmText: t('recent.clear', 'Clear recent')
		});
		if (!ok) return;
		try {
			await clearRecent();
			raw = [];
			cursor = undefined;
		} catch (e) {
			errorToast(e);
		}
	}

	// ── Context-menu actions ──────────────────────────────────────────────────
	let moveOpen = $state(false);
	let moveTarget = $state<{ id: string; name: string; kind: ItemType } | null>(null);
	let moveItems = $state<{ id: string; name: string; kind: ItemType }[] | null>(null);
	let shareOpen = $state(false);
	let shareTarget = $state<{ id: string; name: string; kind: ItemType } | null>(null);

	async function rename(entry: ResourceEntry) {
		const name = await promptDialog({
			title: t('common.rename', 'Rename'),
			defaultValue: entry.name,
			confirmText: t('common.rename', 'Rename')
		});
		if (!name || name === entry.name) return;
		try {
			if (entry.kind === 'file') await renameFile(entry.id, name);
			else await renameFolder(entry.id, name);
			await load(true, orderByForGroup());
		} catch (e) {
			errorToast(e);
		}
	}

	async function remove(entry: ResourceEntry) {
		const ok = await confirmDialog({
			title: t('common.delete', 'Delete'),
			message: t('files.confirm_delete', { name: entry.name }, 'Delete "{{name}}"?'),
			confirmText: t('common.delete', 'Delete'),
			danger: true
		});
		if (!ok) return;
		try {
			if (entry.kind === 'file') await deleteFile(entry.id);
			else await deleteFolder(entry.id);
			raw = raw.filter((i) => i.resource.id !== entry.id);
		} catch (e) {
			errorToast(e);
		}
	}

	function downloadEntry(entry: ResourceEntry) {
		if (entry.kind !== 'file') return;
		const a = document.createElement('a');
		a.href = fileDownloadUrl(entry.id);
		a.download = entry.name;
		document.body.appendChild(a);
		a.click();
		a.remove();
	}

	const contextActions: ContextAction[] = [
		{
			key: 'download',
			label: t('common.download', 'Download'),
			icon: 'download',
			run: downloadEntry
		},
		{
			key: 'share',
			label: t('files.share', 'Share'),
			icon: 'share-alt',
			run: (e) => {
				shareTarget = { id: e.id, name: e.name, kind: e.kind };
				shareOpen = true;
			}
		},
		{
			key: 'move',
			label: t('files.move', 'Move'),
			icon: 'arrows-alt',
			run: (e) => {
				moveItems = null;
				moveTarget = { id: e.id, name: e.name, kind: e.kind };
				moveOpen = true;
			}
		},
		{ key: 'rename', label: t('common.rename', 'Rename'), icon: 'pen', run: rename },
		{ key: 'delete', label: t('common.delete', 'Delete'), icon: 'trash', danger: true, run: remove }
	];

	// ── Selection + batch ─────────────────────────────────────────────────────
	let selectedIds = $state<Set<string>>(new Set());
	const selectedEntries = $derived(entries.filter((e) => selectedIds.has(e.id)));

	function batchTargets() {
		return selectedEntries.map((e) => ({ id: e.id, name: e.name, kind: e.kind }));
	}

	function batchDownload() {
		for (const e of selectedEntries) downloadEntry(e);
	}

	async function batchDelete() {
		const ok = await confirmDialog({
			title: t('common.delete', 'Delete'),
			message: t(
				'files.confirm_delete_n',
				{ count: selectedEntries.length },
				'Delete {{count}} item(s)?'
			),
			confirmText: t('common.delete', 'Delete'),
			danger: true
		});
		if (!ok) return;
		try {
			await Promise.all(
				selectedEntries.map((e) => (e.kind === 'file' ? deleteFile(e.id) : deleteFolder(e.id)))
			);
			const removed = new Set(selectedEntries.map((e) => e.id));
			raw = raw.filter((i) => !removed.has(i.resource.id));
			selectedIds = new Set();
		} catch (e) {
			errorToast(e);
		}
	}

	onMount(() => {
		void loadFavoriteIds();
		void load(true);
	});
</script>

<svelte:head><title>{t('nav.recent', 'Recent')} · OxiCloud</title></svelte:head>

<ResourceList
	title={t('nav.recent', 'Recent')}
	items={entries}
	{loading}
	{error}
	emptyIcon={hiddenCount > 0 ? 'eye-slash' : 'clock'}
	emptyText={hiddenCount > 0
		? t(
				'recent.empty_hidden_state',
				{ n: hiddenCount },
				'{{n}} recent item(s) hidden by your dotfile preference'
			)
		: t('recent.empty_state', 'No recent files')}
	emptyHint={hiddenCount > 0
		? t('recent.empty_hidden_hint', 'Turn off "Hide dotfiles" in your profile to see them.')
		: t('recent.empty_hint', 'Files you open will appear here')}
	hasMore={!!cursor}
	onloadmore={() => load(false, orderByForGroup())}
	onopen={open}
	onfavorite={toggleFavorite}
	showOwner
	showDotfileToggle
	selectable
	{contextActions}
	{groupBys}
	bind:groupBy
	bind:reversed
	onreload={(orderBy, rev) => {
		cursor = undefined;
		load(true, orderBy, rev);
	}}
	onselectionchange={(ids) => (selectedIds = ids)}
>
	{#snippet toolbar()}
		{#if entries.length > 0}
			<Button icon="broom" data-testid="recent-clear-btn" onclick={clearAll}
				>{t('recent.clear', 'Clear recent')}</Button
			>
		{/if}
	{/snippet}
	{#snippet batchToolbar()}
		<Button icon="download" data-testid="recent-batch-download-btn" onclick={batchDownload}
			>{t('common.download', 'Download')}</Button
		>
		<Button
			icon="arrows-alt"
			data-testid="recent-batch-move-btn"
			onclick={() => {
				moveTarget = null;
				moveItems = batchTargets();
				moveOpen = true;
			}}>{t('files.move', 'Move')}</Button
		>
		<Button
			variant="danger"
			icon="trash"
			data-testid="recent-batch-delete-btn"
			onclick={batchDelete}>{t('common.delete', 'Delete')}</Button
		>
	{/snippet}
</ResourceList>

{#if fileViewer.component}
	{@const FileViewer = fileViewer.component}
	<FileViewer bind:open={viewerOpen} file={viewerFile} />
{/if}
{#if moveDialog.component}
	{@const MoveDialog = moveDialog.component}
	<MoveDialog
		bind:open={moveOpen}
		item={moveTarget}
		items={moveItems}
		onmoved={() => {
			selectedIds = new Set();
			load(true, orderByForGroup());
		}}
	/>
{/if}
{#if shareDialog.component}
	{@const ShareDialog = shareDialog.component}
	<ShareDialog bind:open={shareOpen} item={shareTarget} />
{/if}
