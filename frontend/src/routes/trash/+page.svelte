<script lang="ts">
	import { errorToast } from '$lib/utils/errors';
	import { onMount } from 'svelte';
	import {
		deleteTrashItem,
		emptyTrash,
		emptyTrashForDrive,
		expiryChip,
		fetchTrashPage,
		remainingDaysBucket,
		restoreTrashItem
	} from '$lib/api/endpoints/trash';
	import { dateBucket, sizeBucket, typeLabel } from '$lib/api/endpoints/favorites';
	import type { Drive, FileItem, FolderItem, TrashResourceItem } from '$lib/api/types';
	import Icon from '$lib/icons/Icon.svelte';
	import ResourceList, {
		isFile,
		type GroupByDef,
		type ItemContext
	} from '$lib/components/ResourceList.svelte';
	import { confirmDialog } from '$lib/stores/dialogs.svelte';
	import { t } from '$lib/i18n/index.svelte';
	import { drives as drivesStore } from '$lib/stores/drives.svelte';
	import { ui } from '$lib/stores/ui.svelte';

	let raw = $state<TrashResourceItem[]>([]);
	let cursor = $state<string | undefined>(undefined);
	let loading = $state(false);
	let error = $state<string | null>(null);
	// Default: items expiring soonest first, grouped by remaining days.
	let groupBy = $state('remainingDays');
	let reversed = $state(false);

	// Trash view DELIBERATELY doesn't set `showDotfileToggle` on the
	// ResourceList below. Trash is a safety net — hiding items here
	// would let an accidentally-trashed dotfile ride the retention
	// timer to permanent deletion without ever being visible for
	// recovery. The `preferences.hideDotfiles` toggle is UI cosmetics
	// elsewhere; here it would become a footgun. Same reasoning
	// applies to any future "review before destructive action" surface.
	//
	// Items go to ResourceList as raw `FileItem | FolderItem`; the trash
	// envelope's extra fields (`deletion_date`, `trashed_at`, `drive_id`)
	// travel through `contextMap`, which page-provided group-by / render
	// callbacks read via the `ctx` parameter.
	const items = $derived(raw.map((it) => it.resource as FileItem | FolderItem));
	const contextMap = $derived(
		new Map<string, ItemContext>(
			raw.map((it) => [
				it.resource.id,
				{
					date: it.deletion_date,
					extras: {
						driveId: it.drive_id,
						trashedAt: it.trashed_at
					}
				}
			])
		)
	);

	// "Drive" group rank: default-personal first, then secondary personal, then
	// shared — matches `DrivePicker.svelte::sortedDrives` so the sidebar and
	// trash sections agree on ordering. Used as the bucket sort key.
	function driveRank(d: Drive | null): number {
		if (!d) return 99;
		if (d.default_for_user) return 0;
		return d.kind === 'personal' ? 1 : 2;
	}
	function driveLabel(driveId: string): string {
		const d = drivesStore.findById(driveId);
		return d?.name ?? driveId;
	}
	function driveBucketKey(driveId: string): string {
		// Bucket key has to be a string but we want ordering; prefix with
		// rank so the natural lexical sort puts buckets in the picker's order.
		const d = drivesStore.findById(driveId);
		const rank = driveRank(d).toString().padStart(2, '0');
		return `${rank}:${driveId}`;
	}
	function driveBucketLabel(key: string): string {
		const driveId = key.split(':')[1] ?? key;
		return driveLabel(driveId);
	}

	const groupBys: GroupByDef[] = [
		{ key: '', label: t('files.name', 'Name'), orderBy: 'name', icon: 'arrow-up-a-z' },
		{
			key: 'drive',
			label: t('trash.groupby.drive', 'Drive'),
			orderBy: 'name',
			bucketOf: (_item, ctx) => {
				const driveId = ctx?.extras?.driveId;
				return typeof driveId === 'string' ? driveBucketKey(driveId) : null;
			},
			labelOf: driveBucketLabel
		},
		{
			key: 'remainingDays',
			label: t('trash.groupby.remaining_days', 'Remaining days'),
			orderBy: 'deletion_date',
			bucketOf: (_item, ctx) => remainingDaysBucket(ctx?.date)
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
			key: 'trashedTime',
			label: t('trash.groupby.trashed_time', 'Trashed time'),
			orderBy: 'trashed_at',
			bucketOf: (_item, ctx) => {
				const t = ctx?.extras?.trashedAt;
				return dateBucket(typeof t === 'number' ? t : null);
			}
		}
	];

	function orderByForGroup(): string {
		return groupBys.find((g) => g.key === groupBy)?.orderBy ?? 'deletion_date';
	}

	async function load(reset = false, orderBy = 'deletion_date', rev = reversed) {
		loading = true;
		error = null;
		try {
			const page = await fetchTrashPage({
				cursor: reset ? undefined : cursor,
				orderBy,
				reverse: rev,
				resourceTypes: ['file', 'folder']
			});
			raw = reset ? page.items : [...raw, ...page.items];
			cursor = page.next_cursor;
		} catch (e) {
			console.error('trash: load error', e);
			error = t('errors_loadFailed', 'Failed to load items');
		} finally {
			loading = false;
		}
	}

	/** Re-fetch from the top so pagination + grouping stay correct after a mutation. */
	async function reloadFromTop() {
		cursor = undefined;
		await load(true, orderByForGroup());
	}

	async function restore(item: FileItem | FolderItem) {
		try {
			await restoreTrashItem(item.id);
			ui.notify(t('trash.restored', 'Restored'), 'success');
			await reloadFromTop();
		} catch (e) {
			errorToast(e);
		}
	}

	async function purge(item: FileItem | FolderItem) {
		const ok = await confirmDialog({
			title: t('trash.delete', 'Delete permanently'),
			message: t('trash.confirm_delete', 'Permanently delete this item? This cannot be undone.'),
			confirmText: t('trash.delete', 'Delete'),
			danger: true
		});
		if (!ok) return;
		try {
			await deleteTrashItem(item.id);
			await reloadFromTop();
		} catch (e) {
			errorToast(e);
		}
	}

	async function purgeAll() {
		const ok = await confirmDialog({
			title: t('trash.empty_action', 'Empty trash'),
			message: t('trash.confirm_empty', 'Empty the trash? This cannot be undone.'),
			confirmText: t('trash.empty_action', 'Empty trash'),
			danger: true
		});
		if (!ok) return;
		try {
			await emptyTrash();
			raw = [];
			cursor = undefined;
		} catch (e) {
			errorToast(e);
		}
	}

	// Per-drive empty (D2b stage 4 follow-up). The bucket key on the
	// Drive group-by encodes "{rank}:{driveId}" so the natural lexical
	// sort puts default-personal first; we strip the rank prefix here
	// to recover the raw drive UUID. Only an Owner of the drive
	// (Delete-bearing role) reaches the per-drive Empty button because
	// the backend resolves a Delete-set first and refuses (404) any
	// other drive.
	function driveIdFromBucketKey(key: string): string {
		return key.includes(':') ? (key.split(':')[1] ?? key) : key;
	}

	async function purgeDrive(bucketKey: string) {
		const driveId = driveIdFromBucketKey(bucketKey);
		const drive = drivesStore.findById(driveId);
		// Owner-only check mirrors the backend gate so the UI doesn't
		// surface the action for non-Owners — keeps the affordance
		// honest. The bucket only appears on the page if the trash list
		// already contained items the caller could see, but caller_role
		// distinguishes Owner from Viewer/Editor on shared drives.
		const ok = await confirmDialog({
			title: t('trash.empty_drive_title', 'Empty drive trash'),
			message: t(
				'trash.confirm_empty_drive',
				{ name: drive?.name ?? driveId },
				'Empty the trash on drive "{{name}}"? This cannot be undone.'
			),
			confirmText: t('trash.empty_action', 'Empty trash'),
			danger: true
		});
		if (!ok) return;
		try {
			await emptyTrashForDrive(driveId);
			// Drop every entry that belonged to this drive; cheaper than a
			// full refetch and matches what the user just saw.
			raw = raw.filter((it) => it.drive_id !== driveId);
		} catch (e) {
			errorToast(e);
		}
	}

	// The Drive group-by is the only one where a per-bucket empty
	// affordance is meaningful — every other bucket key (remaining
	// days, type, size, trashed time) isn't a permission scope. Hide
	// the button on those group-bys.
	const showPerDriveEmpty = $derived(groupBy === 'drive');

	function driveCanPurge(driveId: string): boolean {
		const d = drivesStore.findById(driveId);
		// `caller_role === 'owner'` is the same gate the backend
		// applies via Permission::Delete in the role bundle. Hide the
		// button on Viewer/Editor drives so a click can't 404.
		return d?.caller_role === 'owner';
	}

	onMount(() => {
		// Drive names for the "Drive" group-by labels — `drivesStore.load()` is
		// idempotent (cached on the singleton) so this is essentially free.
		void drivesStore.load();
		void load(true);
	});
</script>

<svelte:head><title>{t('nav.trash', 'Trash')} · OxiCloud</title></svelte:head>

<ResourceList
	title={t('nav.trash', 'Trash')}
	{items}
	{contextMap}
	{loading}
	{error}
	emptyIcon="trash"
	emptyText={t('trash.empty_state', 'Trash is empty')}
	hasMore={!!cursor}
	onloadmore={() => load(false, orderByForGroup())}
	pathLabel={t('trash.original_location', 'Original location')}
	dateLabel={t('trash.remaining', 'Remaining')}
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
			<button class="btn btn-danger" data-testid="trash-empty-btn" onclick={purgeAll}>
				<Icon name="trash" />
				{t('trash.empty_action', 'Empty trash')}
			</button>
		{/if}
	{/snippet}
	{#snippet dateCell(_item, ctx)}
		{@const chip = expiryChip(ctx?.date)}
		<span class="expiry-chip expiry-chip--{chip.tier}">
			<Icon name={chip.icon} class="expiry-chip__icon" />
			{chip.label}
		</span>
	{/snippet}
	{#snippet bucketAction(bucketKey: string)}
		{#if showPerDriveEmpty}
			{@const driveId = driveIdFromBucketKey(bucketKey)}
			{#if driveCanPurge(driveId)}
				<button
					type="button"
					class="btn-action btn-action--delete"
					data-testid={`trash-empty-drive-btn-${driveId}`}
					title={t('trash.empty_drive_title', 'Empty drive trash')}
					aria-label={t('trash.empty_drive_title', 'Empty drive trash')}
					onclick={() => purgeDrive(bucketKey)}
				>
					<Icon name="trash" />
				</button>
			{/if}
		{/if}
	{/snippet}
	{#snippet actions(item)}
		<button
			class="btn-action"
			data-testid={`trash-restore-btn-${item.id}`}
			title={t('trash.restore', 'Restore')}
			onclick={() => restore(item)}
		>
			<Icon name="undo" />
		</button>
		<button
			class="btn-action btn-action--delete"
			data-testid={`trash-delete-btn-${item.id}`}
			title={t('trash.delete', 'Delete permanently')}
			onclick={() => purge(item)}
		>
			<Icon name="trash" />
		</button>
	{/snippet}
</ResourceList>

<style>
	/* Tiered expiry chip — ported from static/css expiryChip styles. */
	.expiry-chip {
		display: inline-flex;
		align-items: center;
		gap: var(--space-1);
		padding: var(--space-1) var(--space-2);
		border-radius: var(--radius-pill, var(--radius-md));
		font-size: var(--text-xs);
		font-weight: var(--weight-medium, 500);
		white-space: nowrap;
		background: var(--color-bg-muted);
		color: var(--color-text-secondary);
	}

	.expiry-chip :global(.expiry-chip__icon) {
		font-size: 0.85em;
	}

	.expiry-chip--never {
		background: var(--color-bg-muted);
		color: var(--color-text-faint);
	}

	.expiry-chip--caution {
		background: var(--color-warning-bg);
		color: var(--color-warning-text-amber);
	}

	.expiry-chip--soon {
		background: var(--color-warning-orange-bg);
		color: var(--color-warning-text-orange);
	}

	.expiry-chip--urgent {
		background: var(--color-danger-bg);
		color: var(--color-danger-text);
	}

	.expiry-chip--expired {
		background: var(--color-danger-bg);
		color: var(--color-danger-text);
		font-weight: var(--weight-semibold);
	}
</style>
