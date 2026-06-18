<script lang="ts">
	import { errorToast } from '$lib/utils/errors';
	import { onMount } from 'svelte';
	import {
		deleteTrashItem,
		emptyTrash,
		expiryChip,
		fetchTrashPage,
		remainingDaysBucket,
		restoreTrashItem
	} from '$lib/api/endpoints/trash';
	import { dateBucket, sizeBucket, typeLabel } from '$lib/api/endpoints/favorites';
	import type { FileItem, TrashResourceItem } from '$lib/api/types';
	import Icon from '$lib/icons/Icon.svelte';
	import ResourceList, {
		type GroupByDef,
		type ResourceEntry
	} from '$lib/components/ResourceList.svelte';
	import { confirmDialog } from '$lib/stores/dialogs.svelte';
	import { t } from '$lib/i18n/index.svelte';
	import { ui } from '$lib/stores/ui.svelte';

	let raw = $state<TrashResourceItem[]>([]);
	let cursor = $state<string | undefined>(undefined);
	let loading = $state(false);
	let error = $state<string | null>(null);
	// Default: items expiring soonest first, grouped by remaining days.
	let groupBy = $state('remainingDays');
	let reversed = $state(false);

	const entries = $derived(
		raw.map((it): ResourceEntry => {
			const isFile = it.resource_type === 'file';
			return {
				id: it.resource.id,
				name: it.resource.name,
				kind: it.resource_type,
				iconClass: it.resource.icon_class,
				path: it.resource.path,
				size: isFile ? (it.resource as FileItem).size : null,
				// `date` carries the deletion date — rendered as an expiry chip.
				date: it.deletion_date,
				category: isFile ? it.resource.category : 'Folder',
				modifiedAt: it.trashed_at
			};
		})
	);

	const groupBys: GroupByDef[] = [
		{ key: '', label: t('files.name', 'Name'), orderBy: 'name' },
		{
			key: 'remainingDays',
			label: t('trash.groupby.remaining_days', 'Remaining days'),
			orderBy: 'deletion_date',
			bucketOf: (e) => remainingDaysBucket(e.date)
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
			key: 'trashedTime',
			label: t('trash.groupby.trashed_time', 'Trashed time'),
			orderBy: 'trashed_at',
			bucketOf: (e) => dateBucket(e.modifiedAt)
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

	async function restore(entry: ResourceEntry) {
		try {
			await restoreTrashItem(entry.id);
			ui.notify(t('trash.restored', 'Restored'), 'success');
			await reloadFromTop();
		} catch (e) {
			errorToast(e);
		}
	}

	async function purge(entry: ResourceEntry) {
		const ok = await confirmDialog({
			title: t('trash.delete', 'Delete permanently'),
			message: t('trash.confirm_delete', 'Permanently delete this item? This cannot be undone.'),
			confirmText: t('trash.delete', 'Delete'),
			danger: true
		});
		if (!ok) return;
		try {
			await deleteTrashItem(entry.id);
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

	onMount(() => load(true));
</script>

<svelte:head><title>{t('nav.trash', 'Trash')} · OxiCloud</title></svelte:head>

<ResourceList
	title={t('nav.trash', 'Trash')}
	items={entries}
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
		{#if entries.length > 0}
			<button class="btn btn-danger" onclick={purgeAll}>
				<Icon name="trash" />
				{t('trash.empty_action', 'Empty trash')}
			</button>
		{/if}
	{/snippet}
	{#snippet dateCell(entry)}
		{@const chip = expiryChip(entry.date)}
		<span class="expiry-chip expiry-chip--{chip.tier}">
			<Icon name={chip.icon} class="expiry-chip__icon" />
			{chip.label}
		</span>
	{/snippet}
	{#snippet actions(entry)}
		<button class="btn-action" title={t('trash.restore', 'Restore')} onclick={() => restore(entry)}>
			<Icon name="undo" />
		</button>
		<button
			class="btn-action btn-action--delete"
			title={t('trash.delete', 'Delete permanently')}
			onclick={() => purge(entry)}
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
