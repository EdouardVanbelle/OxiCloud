<script lang="ts">
	import Button from '$lib/components/Button.svelte';
	import { useOwnerCache } from '$lib/composables/useOwnerCache.svelte';
	import { errorToast } from '$lib/utils/errors';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { onMount } from 'svelte';
	import {
		dateBucket,
		fetchFavoritesPage,
		removeFavorite,
		resolveOwnerName,
		sizeBucket,
		typeLabel,
		type FavoritesResourceItem
	} from '$lib/api/endpoints/favorites';
	import { fileDownloadUrl } from '$lib/api/endpoints/files';
	import { renameFile, deleteFile } from '$lib/api/endpoints/files';
	import { renameFolder, deleteFolder } from '$lib/api/endpoints/folders';
	import type { FileItem } from '$lib/api/types';
	import { lazyComponent } from '$lib/composables/lazyComponent.svelte';
	import ResourceList, {
		type ContextAction,
		type GroupByDef,
		type ResourceEntry
	} from '$lib/components/ResourceList.svelte';
	import { confirmDialog, promptDialog } from '$lib/stores/dialogs.svelte';
	import { t } from '$lib/i18n/index.svelte';

	let raw = $state<FavoritesResourceItem[]>([]);
	let cursor = $state<string | undefined>(undefined);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let groupBy = $state('');
	let reversed = $state(false);
	const owners = useOwnerCache(resolveOwnerName);

	const byId = $derived(new Map(raw.map((it) => [it.resource.id, it])));

	// Favorites view DELIBERATELY ignores `preferences.hideDotfiles`.
	// Rationale: favoriting is an explicit "I want to keep an eye on
	// this" action by the user — hiding a starred item on a different
	// listing page because it starts with `.` contradicts that intent.
	// The hide preference is for reducing incidental clutter in
	// algorithmic listings (files/recent/photos), not for overriding
	// user-intentional pins. Trash follows the same principle for a
	// safety-net reason; the general rule shaping up: explicit-action
	// surfaces don't filter, algorithmic surfaces do.
	const entries = $derived(
		raw.map((it): ResourceEntry => {
			const isFile = it.resource_type === 'file';
			// §14 provenance: `created_by` names who put the item into
			// the system (Files browser / Favorites / Shared semantic).
			const ownerId = it.resource.created_by ?? null;
			return {
				id: it.resource.id,
				name: it.resource.name,
				kind: it.resource_type,
				iconClass: it.resource.icon_class,
				path: it.resource.path,
				size: isFile ? (it.resource as FileItem).size : null,
				date: it.favorited_at,
				ownerId,
				ownerName: owners.name(ownerId),
				isFavorite: true,
				category: isFile ? it.resource.category : 'Folder',
				modifiedAt: it.resource.modified_at
			};
		})
	);

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
			key: 'favoriteDate',
			label: t('groupby.favoriteDate', 'Favorite date'),
			orderBy: 'favorited_at',
			bucketOf: (e) => dateBucket(e.date)
		},
		{
			key: 'modifiedAt',
			label: t('groupby.modifiedAt', 'Modified date'),
			orderBy: 'modified_at',
			bucketOf: (e) => dateBucket(e.modifiedAt)
		}
	];

	async function load(reset = false, orderBy = 'name', rev = reversed) {
		loading = true;
		error = null;
		try {
			const page = await fetchFavoritesPage({
				cursor: reset ? undefined : cursor,
				orderBy,
				reverse: rev,
				resourceTypes: ['file', 'folder']
			});
			raw = reset ? page.items : [...raw, ...page.items];
			cursor = page.next_cursor;
			void owners.resolve(page.items.map((i) => i.resource.created_by));
		} catch (e) {
			console.error('favorites: load error', e);
			error = t('errors_loadFailed', 'Failed to load items');
		} finally {
			loading = false;
		}
	}

	function orderByForGroup(): string {
		return groupBys.find((g) => g.key === groupBy)?.orderBy ?? 'name';
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

	async function unfavorite(entry: ResourceEntry) {
		try {
			await removeFavorite(entry.kind, entry.id);
			raw = raw.filter((i) => i.resource.id !== entry.id);
		} catch (e) {
			errorToast(e);
		}
	}

	// ── Context-menu actions ──────────────────────────────────────────────────
	let moveOpen = $state(false);
	let moveTarget = $state<{ id: string; name: string; kind: 'file' | 'folder' } | null>(null);
	let moveItems = $state<{ id: string; name: string; kind: 'file' | 'folder' }[] | null>(null);
	let shareOpen = $state(false);
	let shareTarget = $state<{ id: string; name: string; kind: 'file' | 'folder' } | null>(null);

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

	onMount(() => load(true));
</script>

<svelte:head><title>{t('nav.favorites', 'Favorites')} · OxiCloud</title></svelte:head>

<ResourceList
	title={t('nav.favorites', 'Favorites')}
	items={entries}
	{loading}
	{error}
	emptyIcon="star"
	emptyText={t('favorites.empty_state', 'No favorites yet')}
	emptyHint={t('favorites.empty_hint', 'Star files and folders to find them here quickly')}
	hasMore={!!cursor}
	onloadmore={() => load(false, orderByForGroup())}
	onopen={open}
	onfavorite={unfavorite}
	showOwner
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
	{#snippet batchToolbar()}
		<Button icon="download" data-testid="favorites-batch-download-btn" onclick={batchDownload}
			>{t('common.download', 'Download')}</Button
		>
		<Button
			icon="arrows-alt"
			data-testid="favorites-batch-move-btn"
			onclick={() => {
				moveTarget = null;
				moveItems = batchTargets();
				moveOpen = true;
			}}>{t('files.move', 'Move')}</Button
		>
		<Button
			variant="danger"
			icon="trash"
			data-testid="favorites-batch-delete-btn"
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
