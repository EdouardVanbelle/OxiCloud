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
	import type { FileItem, FolderItem, ItemType } from '$lib/api/types';
	import { lazyComponent } from '$lib/composables/lazyComponent.svelte';
	import ResourceList, {
		isFile,
		type ContextAction,
		type GroupByDef,
		type ItemContext
	} from '$lib/components/ResourceList.svelte';
	import { confirmDialog, promptDialog } from '$lib/stores/dialogs.svelte';
	// `preferences.hideDotfiles` + `isDotfile` are read here only to
	// derive `hiddenCount` for the empty-state message — the actual
	// filter is inside ResourceList (gated on `showDotfileToggle`).
	// `replaceSet` is from perf-round-6: `loadFavoriteIds` mutates
	// the reactive SvelteSet in place instead of re-creating it.
	import { preferences } from '$lib/stores/preferences.svelte';
	import { isDotfile } from '$lib/utils/dotfileFilter';
	import { replaceSet } from '$lib/utils/sets';
	import { t } from '$lib/i18n/index.svelte';

	let raw = $state<RecentResourceItem[]>([]);
	let cursor = $state<string | undefined>(undefined);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let groupBy = $state('');
	let reversed = $state(false);
	const owners = useOwnerCache(resolveOwnerName);
	// In-place reactive set — a star toggle skips the full-set copy and
	// spares the other favorited rows' readers.
	const favoriteIds = new SvelteSet<string>();

	// Envelope shape: `accessed_at` → `ctx.date`, `updated_by` → `ctx.ownerId`
	// (Recent's provenance semantic — "who touched this recently" — differs
	// from Favorites'/Files' `created_by`).
	//
	// Dotfile hiding is delegated to ResourceList via `showDotfileToggle`
	// — the component reads `preferences.hideDotfiles` and drops matching
	// rows from every downstream reader (bucketing, rendering, select-
	// all). The `hiddenCount` here is derived independently via the
	// shared `isDotfile` predicate purely for the empty-state message
	// below (distinguishes "genuinely empty" from "everything filtered").
	const items = $derived(raw.map((it) => it.resource as FileItem | FolderItem));
	const contextMap = $derived(
		new Map<string, ItemContext>(
			raw.map((it) => [
				it.resource.id,
				{ date: it.accessed_at, ownerId: it.resource.updated_by ?? null } satisfies ItemContext
			])
		)
	);
	const hiddenCount = $derived(
		preferences.hideDotfiles ? items.filter((i) => isDotfile(i.name)).length : 0
	);

	const groupBys: GroupByDef[] = [
		{ key: '', label: t('files.name', 'Name'), orderBy: 'name', icon: 'arrow-up-a-z' },
		{
			key: 'owner',
			label: t('groupby.owner', 'Owner'),
			orderBy: 'owner',
			bucketOf: (_item, ctx) => ctx?.ownerId ?? null,
			labelOf: (id) => owners.label(id)
		},
		{
			key: 'type',
			label: t('groupby.type', 'Type'),
			orderBy: 'type',
			bucketOf: (item) => item.category ?? 'other',
			labelOf: (k) => typeLabel(k)
		},
		{
			key: 'size',
			label: t('groupby.size', 'Size'),
			orderBy: 'size',
			bucketOf: (item) => sizeBucket(isFile(item) ? item.size : null)
		},
		{
			key: 'accessedAt',
			label: t('groupby.accessedAt', 'Accessed date'),
			orderBy: 'accessed_at',
			bucketOf: (_item, ctx) => dateBucket(ctx?.date)
		},
		{
			key: 'modifiedAt',
			label: t('groupby.modifiedAt', 'Modified date'),
			orderBy: 'modified_at',
			bucketOf: (item) => dateBucket(item.modified_at)
		}
	];

	async function loadFavoriteIds() {
		try {
			const favs = await fetchFavoritesPage({ resourceTypes: ['file', 'folder'] });
			replaceSet(
				favoriteIds,
				favs.items.map((f) => f.resource.id)
			);
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

	function kindOf(item: FileItem | FolderItem): ItemType {
		return isFile(item) ? 'file' : 'folder';
	}

	function open(item: FileItem | FolderItem) {
		if (!isFile(item)) {
			goto(resolve(`/files/${item.id}`));
			return;
		}
		viewerFile = item;
		viewerOpen = true;
	}

	// Callback signature is `FileItem | FolderItem` (ResourceList
	// hands raw items to `onfavorite` — the pre-migration
	// `ResourceEntry` shape is gone). Set mutation is in-place per
	// perf-round-6: 1 000 toggles @ N=5 000 dropped from 771.9 ms
	// to 1.9 ms by skipping the full-set copy that every reader of
	// `favoriteIds` used to see.
	async function toggleFavorite(item: FileItem | FolderItem) {
		const isFav = favoriteIds.has(item.id);
		const kind = kindOf(item);
		// Optimistic in-place toggle, reverted on failure.
		if (isFav) favoriteIds.delete(item.id);
		else favoriteIds.add(item.id);
		try {
			if (isFav) await removeFavorite(kind, item.id);
			else await addFavorite(kind, item.id);
		} catch (e) {
			if (isFav) favoriteIds.add(item.id);
			else favoriteIds.delete(item.id);
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

	async function rename(item: FileItem | FolderItem) {
		const name = await promptDialog({
			title: t('common.rename', 'Rename'),
			defaultValue: item.name,
			confirmText: t('common.rename', 'Rename')
		});
		if (!name || name === item.name) return;
		try {
			if (isFile(item)) await renameFile(item.id, name);
			else await renameFolder(item.id, name);
			await load(true, orderByForGroup());
		} catch (e) {
			errorToast(e);
		}
	}

	async function remove(item: FileItem | FolderItem) {
		const ok = await confirmDialog({
			title: t('common.delete', 'Delete'),
			message: t('files.confirm_delete', { name: item.name }, 'Delete "{{name}}"?'),
			confirmText: t('common.delete', 'Delete'),
			danger: true
		});
		if (!ok) return;
		try {
			if (isFile(item)) await deleteFile(item.id);
			else await deleteFolder(item.id);
			raw = raw.filter((i) => i.resource.id !== item.id);
		} catch (e) {
			errorToast(e);
		}
	}

	function downloadItem(item: FileItem | FolderItem) {
		if (!isFile(item)) return;
		const a = document.createElement('a');
		a.href = fileDownloadUrl(item.id);
		a.download = item.name;
		document.body.appendChild(a);
		a.click();
		a.remove();
	}

	const contextActions: ContextAction[] = [
		{
			key: 'download',
			label: t('common.download', 'Download'),
			icon: 'download',
			run: downloadItem
		},
		{
			key: 'share',
			label: t('files.share', 'Share'),
			icon: 'share-alt',
			run: (item) => {
				shareTarget = { id: item.id, name: item.name, kind: kindOf(item) };
				shareOpen = true;
			}
		},
		{
			key: 'move',
			label: t('files.move', 'Move'),
			icon: 'arrows-alt',
			run: (item) => {
				moveItems = null;
				moveTarget = { id: item.id, name: item.name, kind: kindOf(item) };
				moveOpen = true;
			}
		},
		{ key: 'rename', label: t('common.rename', 'Rename'), icon: 'pen', run: rename },
		{ key: 'delete', label: t('common.delete', 'Delete'), icon: 'trash', danger: true, run: remove }
	];

	// ── Selection + batch ─────────────────────────────────────────────────────
	// Selected items arrive via the batchToolbar snippet param —
	// ResourceList already derives them (O(selection), not O(N)); a
	// host-side `items.filter(...)` shadow would re-run a second full scan
	// per selection toggle, and its id mirror is unnecessary (the component
	// prunes its own selection when items reload) — benches/ROUND11.md §S1.
	type Selectable = FileItem | FolderItem;

	function batchTargets(sel: Selectable[]) {
		return sel.map((i) => ({ id: i.id, name: i.name, kind: kindOf(i) }));
	}

	function batchDownload(sel: Selectable[]) {
		for (const i of sel) downloadItem(i);
	}

	async function batchDelete(sel: Selectable[]) {
		const ok = await confirmDialog({
			title: t('common.delete', 'Delete'),
			message: t('files.confirm_delete_n', { count: sel.length }, 'Delete {{count}} item(s)?'),
			confirmText: t('common.delete', 'Delete'),
			danger: true
		});
		if (!ok) return;
		try {
			await Promise.all(sel.map((i) => (isFile(i) ? deleteFile(i.id) : deleteFolder(i.id))));
			const removed = new Set(sel.map((i) => i.id));
			raw = raw.filter((i) => !removed.has(i.resource.id));
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
	{items}
	{contextMap}
	{favoriteIds}
	resolveOwnerName={(id) => owners.name(id)}
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
>
	{#snippet toolbar()}
		{#if items.length > 0}
			<Button icon="broom" data-testid="recent-clear-btn" onclick={clearAll}
				>{t('recent.clear', 'Clear recent')}</Button
			>
		{/if}
	{/snippet}
	{#snippet batchToolbar(sel)}
		<Button
			icon="download"
			data-testid="recent-batch-download-btn"
			onclick={() => batchDownload(sel)}>{t('common.download', 'Download')}</Button
		>
		<Button
			icon="arrows-alt"
			data-testid="recent-batch-move-btn"
			onclick={() => {
				moveTarget = null;
				moveItems = batchTargets(sel);
				moveOpen = true;
			}}>{t('files.move', 'Move')}</Button
		>
		<Button
			variant="danger"
			icon="trash"
			data-testid="recent-batch-delete-btn"
			onclick={() => batchDelete(sel)}>{t('common.delete', 'Delete')}</Button
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
		onmoved={() => load(true, orderByForGroup())}
	/>
{/if}
{#if shareDialog.component}
	{@const ShareDialog = shareDialog.component}
	<ShareDialog bind:open={shareOpen} item={shareTarget} />
{/if}
