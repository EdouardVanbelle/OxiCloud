<script lang="ts" module>
	import type { ItemType } from '$lib/api/types';

	/** Normalised row passed to ResourceList; views map their items to this. */
	export interface ResourceEntry {
		id: string;
		name: string;
		kind: ItemType;
		iconClass?: string;
		path?: string | null;
		size?: number | null;
		date?: number | string | null;
		typeLabel?: string;
		/** Owner user id — enables the owner column + vignette when `showOwner`. */
		ownerId?: string | null;
		/** Owner display name (resolved by the page). */
		ownerName?: string | null;
		/** Per-entry favorite state for the star-toggle widget. */
		isFavorite?: boolean;
		/** Stable category key (Folder / Image / …) used by the `type` group-by. */
		category?: string;
		/** Modified timestamp (epoch seconds/ms or ISO) for the `modifiedAt` group-by. */
		modifiedAt?: number | string | null;
	}

	/**
	 * A group-by ("swimlane") dimension a page can offer. `orderBy` is sent to the
	 * API; the optional `bucketOf` maps an entry to a section key, and `labelOf`
	 * maps that key to a header label. Omitting `bucketOf` means a flat list.
	 */
	export interface GroupByDef {
		key: string;
		label: string;
		orderBy: string;
		/** Optional icon for the dropdown option (defaults to the group glyph). */
		icon?: string;
		bucketOf?: (entry: ResourceEntry) => string | null;
		labelOf?: (bucketKey: string) => string;
	}

	/** A right-click / overflow context-menu action. */
	export interface ContextAction {
		key: string;
		label: string;
		icon: string;
		danger?: boolean;
		run: (entry: ResourceEntry) => void;
	}
</script>

<script lang="ts">
	import type { Snippet } from 'svelte';
	import Icon from '$lib/icons/Icon.svelte';
	import EmptyState from '$lib/components/EmptyState.svelte';
	import SkeletonList from '$lib/components/SkeletonList.svelte';
	import ListToolbar from '$lib/components/ListToolbar.svelte';
	import { t } from '$lib/i18n/index.svelte';
	import { files as filesStore } from '$lib/stores/files.svelte';
	import { formatBytes } from '$lib/utils/format';
	import { formatDate, iconNameFromClass } from '$lib/utils/display';

	interface Props {
		title: string;
		items: ResourceEntry[];
		loading?: boolean;
		error?: string | null;
		/** Empty-state primary line. */
		emptyText?: string;
		/** Empty-state secondary hint line. */
		emptyHint?: string;
		/** Empty-state icon-registry name (e.g. "star", "clock", "trash"). */
		emptyIcon?: string;
		hasMore?: boolean;
		onloadmore?: () => void;
		/** Show the path/location column (list view only). */
		showPath?: boolean;
		/** Override the path column header label (e.g. trash → "Original location"). */
		pathLabel?: string;
		showSize?: boolean;
		showType?: boolean;
		showDate?: boolean;
		/** Override the date column header label (e.g. trash → "Remaining"). */
		dateLabel?: string;
		/** Custom renderer for the date cell (e.g. trash expiry chip). */
		dateCell?: Snippet<[ResourceEntry]>;
		/** Show the owner column + vignette (list view) and hover tooltip. */
		showOwner?: boolean;
		/** Allow grid/list toggle (shares the app-wide view mode). */
		showViewToggle?: boolean;
		/** Multi-select checkboxes + selection model. */
		selectable?: boolean;
		/** Right-click / overflow context-menu actions. */
		contextActions?: ContextAction[];
		/** Group-by dimensions; when provided, a swimlane selector is shown. */
		groupBys?: GroupByDef[];
		/** Active group-by key (bind:groupBy from the page). */
		groupBy?: string;
		/** Reverse sort toggle state (bind:reversed from the page). */
		reversed?: boolean;
		/** Called when group-by or direction changes; page should reload page 1. */
		onreload?: (orderBy: string, reversed: boolean) => void;
		onopen?: (entry: ResourceEntry) => void;
		/** Per-entry favorite star toggle. */
		onfavorite?: (entry: ResourceEntry) => void;
		/** Selection changed (set of selected entry ids). */
		onselectionchange?: (ids: Set<string>) => void;
		actions?: Snippet<[ResourceEntry]>;
		toolbar?: Snippet;
		/** Batch toolbar shown when items are selected; receives selected entries. */
		batchToolbar?: Snippet<[ResourceEntry[]]>;
	}

	let {
		title,
		items,
		loading = false,
		error = null,
		emptyText,
		emptyHint,
		emptyIcon,
		hasMore = false,
		onloadmore,
		showPath = true,
		pathLabel,
		showSize = true,
		showType = false,
		showDate = true,
		dateLabel,
		dateCell,
		showOwner = false,
		showViewToggle = true,
		selectable = false,
		contextActions,
		groupBys,
		groupBy = $bindable(''),
		reversed = $bindable(false),
		onreload,
		onopen,
		onfavorite,
		onselectionchange,
		actions,
		toolbar,
		batchToolbar
	}: Props = $props();

	const isEmpty = $derived(items.length === 0);
	const viewClass = $derived(
		filesStore.viewMode === 'grid' ? 'files-grid-view' : 'files-list-view'
	);

	// Build the list-view column track from the enabled cells.
	const columns = $derived(
		[
			selectable ? '36px' : '',
			'minmax(200px, 2fr)',
			showOwner ? 'minmax(120px, 1fr)' : '',
			showPath ? 'minmax(140px, 1.5fr)' : '',
			showType ? '120px' : '',
			showSize ? '110px' : '',
			showDate ? '160px' : '',
			actions ? '120px' : ''
		]
			.filter(Boolean)
			.join(' ')
	);

	const SKELETON = [0, 1, 2, 3, 4, 5];

	// ── Group-by / direction ──────────────────────────────────────────────────
	const activeGroup = $derived(groupBys?.find((g) => g.key === groupBy));

	function selectGroup(key: string) {
		if (groupBy === key) return;
		groupBy = key;
		const def = groupBys?.find((g) => g.key === key);
		onreload?.(def?.orderBy ?? 'name', reversed);
	}

	function toggleDirection() {
		reversed = !reversed;
		onreload?.(activeGroup?.orderBy ?? 'name', reversed);
	}

	/**
	 * Partition the visible items into grouped sections when a `bucketOf` is
	 * active. Server order is preserved within and across buckets (first-seen).
	 */
	const sections = $derived.by((): Array<{ key: string; label: string; rows: ResourceEntry[] }> => {
		const bucketOf = activeGroup?.bucketOf;
		if (!bucketOf) return [{ key: '', label: '', rows: items }];
		const order: string[] = [];
		const map = new Map<string, ResourceEntry[]>();
		for (const entry of items) {
			const k = bucketOf(entry) ?? '∅';
			if (!map.has(k)) {
				map.set(k, []);
				order.push(k);
			}
			map.get(k)!.push(entry);
		}
		return order.map((k) => ({
			key: k,
			label: activeGroup?.labelOf?.(k) ?? k,
			rows: map.get(k)!
		}));
	});
	const grouped = $derived(!!activeGroup?.bucketOf);

	// ── Selection ─────────────────────────────────────────────────────────────
	let selected = $state<Set<string>>(new Set());

	function toggleSelected(id: string) {
		const next = new Set(selected);
		if (next.has(id)) next.delete(id);
		else next.add(id);
		selected = next;
		onselectionchange?.(next);
	}
	function clearSelection() {
		selected = new Set();
		onselectionchange?.(selected);
	}
	const allSelected = $derived(items.length > 0 && selected.size === items.length);
	function toggleSelectAll() {
		if (allSelected) clearSelection();
		else {
			selected = new Set(items.map((i) => i.id));
			onselectionchange?.(selected);
		}
	}
	const selectedEntries = $derived(items.filter((i) => selected.has(i.id)));

	// Drop selection ids that are no longer present after a reload.
	$effect(() => {
		const ids = new Set(items.map((i) => i.id));
		let changed = false;
		const next = new Set<string>();
		for (const id of selected) {
			if (ids.has(id)) next.add(id);
			else changed = true;
		}
		if (changed) {
			selected = next;
			onselectionchange?.(next);
		}
	});

	// ── Right-click context menu ──────────────────────────────────────────────
	let ctxOpen = $state(false);
	let ctxX = $state(0);
	let ctxY = $state(0);
	let ctxEntry = $state<ResourceEntry | null>(null);

	function openContext(e: MouseEvent, entry: ResourceEntry) {
		if (!contextActions?.length) return;
		e.preventDefault();
		e.stopPropagation();
		ctxEntry = entry;
		ctxX = Math.min(e.clientX, window.innerWidth - 220);
		ctxY = Math.min(e.clientY, window.innerHeight - (contextActions.length * 44 + 24));
		ctxOpen = true;
	}
	function closeContext() {
		ctxOpen = false;
		ctxEntry = null;
	}

	// ── Infinite scroll (IntersectionObserver) ────────────────────────────────
	let sentinel = $state<HTMLElement | null>(null);
	$effect(() => {
		const el = sentinel;
		if (!el || typeof IntersectionObserver === 'undefined') return;
		const obs = new IntersectionObserver(
			(entries) => {
				for (const en of entries) {
					if (en.isIntersecting && hasMore && !loading) onloadmore?.();
				}
			},
			{ rootMargin: '200px' }
		);
		obs.observe(el);
		return () => obs.disconnect();
	});

	function ownerTitle(entry: ResourceEntry): string {
		const owner = entry.ownerName ?? entry.ownerId ?? '';
		const path = entry.path ?? '';
		return [
			owner && `${t('files.col_owner', 'Owner')}: ${owner}`,
			path && `${t('files.col_path', 'Location')}: ${path}`
		]
			.filter(Boolean)
			.join('\n');
	}
</script>

{#snippet row(entry: ResourceEntry)}
	<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
	<div
		class="file-item"
		class:file-item--selected={selectable && selected.has(entry.id)}
		role={onopen ? 'button' : undefined}
		tabindex={onopen ? 0 : undefined}
		title={showOwner ? ownerTitle(entry) : undefined}
		onclick={onopen ? () => onopen(entry) : undefined}
		onkeydown={onopen ? (e) => e.key === 'Enter' && onopen(entry) : undefined}
		oncontextmenu={contextActions?.length ? (e) => openContext(e, entry) : undefined}
	>
		{#if selectable}
			<div class="select-cell" role="presentation" onclick={(e) => e.stopPropagation()}>
				<input
					type="checkbox"
					aria-label={t('common.select', 'Select')}
					checked={selected.has(entry.id)}
					onchange={() => toggleSelected(entry.id)}
				/>
			</div>
		{/if}
		<div class="name-cell">
			<span class="file-icon">
				<Icon name={entry.kind === 'folder' ? 'folder' : iconNameFromClass(entry.iconClass)} />
			</span>
			<span class="name-cell__text">{entry.name}</span>
		</div>
		{#if showOwner}
			<div class="owner-cell">
				<span class="rl-vignette">
					<span class="rl-vignette__avatar" aria-hidden="true"
						>{(entry.ownerName ?? '?').slice(0, 1).toUpperCase()}</span
					>
					<span class="rl-vignette__name">{entry.ownerName ?? entry.ownerId ?? ''}</span>
				</span>
			</div>
		{/if}
		{#if showPath}<div class="path-cell">{entry.path ?? ''}</div>{/if}
		{#if showType}<div class="type-cell">{entry.typeLabel ?? ''}</div>{/if}
		{#if showSize}
			<div class="size-cell">{entry.size != null ? formatBytes(entry.size) : '—'}</div>
		{/if}
		{#if showDate}
			<div class="date-cell">
				{#if dateCell}{@render dateCell(entry)}{:else}{formatDate(entry.date)}{/if}
			</div>
		{/if}
		<div class="grid-meta">
			{#if showDate && dateCell}<span class="grid-meta__chip">{@render dateCell(entry)}</span>{/if}
			<span class="grid-meta__line">
				{#if entry.size != null}<span class="grid-meta__size">{formatBytes(entry.size)}</span>{/if}
				{#if entry.date != null}<span class="grid-meta__date">{formatDate(entry.date)}</span>{/if}
			</span>
		</div>
		{#if onfavorite}
			<button
				class="rl-star"
				class:rl-star--on={entry.isFavorite}
				title={entry.isFavorite
					? t('files.unfavorite', 'Remove favorite')
					: t('files.favorite', 'Add favorite')}
				aria-pressed={!!entry.isFavorite}
				onclick={(e) => {
					e.stopPropagation();
					onfavorite(entry);
				}}><Icon name={entry.isFavorite ? 'star' : 'star-outline'} /></button
			>
		{/if}
		{#if actions}
			<div class="action-cell">{@render actions(entry)}</div>
		{/if}
	</div>
{/snippet}

<div class="page-sticky-header">
	<h1 class="page-title">{title}</h1>
	<ListToolbar
		groups={groupBys}
		{groupBy}
		{reversed}
		ongroup={selectGroup}
		ondirection={toggleDirection}
		{showViewToggle}
	>
		{#snippet start()}
			<div class="action-buttons">{@render toolbar?.()}</div>
		{/snippet}
	</ListToolbar>
</div>

{#if selectable && selected.size > 0 && batchToolbar}
	<div class="rl-batch" role="region" aria-label={t('files.selection', 'Selection')}>
		<button class="rl-batch__close" title={t('common.clear', 'Clear')} onclick={clearSelection}>
			<Icon name="times" />
		</button>
		<span class="rl-batch__count"
			>{t('files.selected_count', { count: selected.size }, '{{count}} selected')}</span
		>
		<div class="rl-batch__actions">{@render batchToolbar(selectedEntries)}</div>
	</div>
{/if}

{#if error}
	<EmptyState icon="exclamation-circle" title={error} error />
{:else if loading && isEmpty}
	<SkeletonList count={SKELETON.length} />
{:else if isEmpty}
	<EmptyState
		icon={emptyIcon}
		title={emptyText ?? t('common.empty', 'Nothing here yet.')}
		hint={emptyHint}
	/>
{:else}
	<div class="files-container">
		<div class={viewClass} style="--files-list-columns: {columns}">
			<div class="list-header">
				{#if selectable}
					<div class="select-cell">
						<input
							type="checkbox"
							aria-label={t('common.select_all', 'Select all')}
							checked={allSelected}
							onchange={toggleSelectAll}
						/>
					</div>
				{/if}
				<div>{t('files.col_name', 'Name')}</div>
				{#if showOwner}<div>{t('files.col_owner', 'Owner')}</div>{/if}
				{#if showPath}<div>{pathLabel ?? t('files.col_path', 'Location')}</div>{/if}
				{#if showType}<div>{t('files.col_type', 'Type')}</div>{/if}
				{#if showSize}<div>{t('files.col_size', 'Size')}</div>{/if}
				{#if showDate}<div>{dateLabel ?? t('files.col_modified', 'Date')}</div>{/if}
				{#if onfavorite || actions}<div></div>{/if}
			</div>

			{#if grouped}
				{#each sections as section (section.key)}
					<div class="rl-swimlane-header" role="rowheader">{section.label}</div>
					{#each section.rows as entry (entry.id)}
						{@render row(entry)}
					{/each}
				{/each}
			{:else}
				{#each items as entry (entry.id)}
					{@render row(entry)}
				{/each}
			{/if}
		</div>

		{#if hasMore}
			<button class="btn btn-secondary rl-more" onclick={onloadmore} disabled={loading}>
				{loading ? t('common.loading', 'Loading…') : t('common.load_more', 'Load more')}
			</button>
		{/if}
		<!-- Infinite-scroll sentinel: auto-loads the next page as it nears the viewport. -->
		<div bind:this={sentinel} class="rl-sentinel" aria-hidden="true"></div>
	</div>
{/if}

{#if ctxOpen && ctxEntry && contextActions}
	<div
		class="rl-ctx-scrim"
		role="presentation"
		onclick={closeContext}
		oncontextmenu={(e) => e.preventDefault()}
	></div>
	<div class="rl-ctx-menu" style:left="{ctxX}px" style:top="{ctxY}px" role="menu">
		{#each contextActions as action (action.key)}
			<button
				class="rl-ctx-item"
				class:rl-ctx-item--danger={action.danger}
				role="menuitem"
				onclick={() => {
					const e = ctxEntry!;
					closeContext();
					action.run(e);
				}}
			>
				<Icon name={action.icon} />
				{action.label}
			</button>
		{/each}
	</div>
{/if}

<style>
	.rl-more {
		margin: var(--space-4) auto 0;
	}

	.rl-sentinel {
		height: 1px;
		width: 100%;
	}

	/* ── Batch toolbar ── */
	.rl-batch {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		padding: var(--space-2) var(--space-4);
		margin-bottom: var(--space-3);
		background: var(--color-accent-bg);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
	}

	.rl-batch__close {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 28px;
		height: 28px;
		border: none;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--color-text-secondary);
		cursor: pointer;
	}

	.rl-batch__close:hover {
		background: var(--color-bg-hover);
	}

	.rl-batch__count {
		font-weight: var(--weight-semibold);
		color: var(--color-text);
	}

	.rl-batch__actions {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		margin-left: auto;
	}

	/* ── Selection column ── */
	.select-cell {
		display: flex;
		align-items: center;
		justify-content: center;
	}

	.file-item--selected {
		background: var(--color-accent-bg);
	}

	/* ── Owner vignette ── */
	.owner-cell {
		display: flex;
		align-items: center;
		min-width: 0;
	}

	.rl-vignette {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		min-width: 0;
	}

	.rl-vignette__avatar {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 24px;
		height: 24px;
		border-radius: 50%;
		background: var(--color-accent-bg-sm);
		color: var(--color-accent-text);
		font-size: var(--text-xs);
		font-weight: var(--weight-semibold);
		flex: none;
	}

	.rl-vignette__name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--color-text-secondary);
	}

	.name-cell__text {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	/* ── Favorite star ── */
	.rl-star {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 32px;
		height: 32px;
		border: none;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--color-text-faint);
		cursor: pointer;
	}

	.rl-star:hover {
		background: var(--color-bg-hover);
		color: var(--color-text-secondary);
	}

	.rl-star--on {
		color: var(--color-warning-text-amber);
	}

	/* ── Swimlane section header ── */
	.rl-swimlane-header {
		grid-column: 1 / -1;
		padding: var(--space-3) var(--space-1) var(--space-1);
		font-size: var(--text-sm);
		font-weight: var(--weight-semibold);
		color: var(--color-text-secondary);
		border-bottom: 1px solid var(--color-border-faint);
	}

	/* Grid view date meta line. */
	.grid-meta__line {
		display: flex;
		align-items: center;
		gap: var(--space-2);
	}

	/* Grid view: overlay a custom date chip (e.g. trash expiry) on the card corner. */
	:global(.files-grid-view) .grid-meta__chip {
		position: absolute;
		top: var(--space-2);
		right: var(--space-2);
		z-index: 1;
	}

	/* ── Context menu ── */
	.rl-ctx-scrim {
		position: fixed;
		inset: 0;
		z-index: 1000;
	}

	.rl-ctx-menu {
		position: fixed;
		z-index: 1001;
		min-width: 200px;
		padding: var(--space-1);
		background: var(--color-bg-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		box-shadow: var(--shadow-lg);
	}

	.rl-ctx-item {
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

	.rl-ctx-item:hover {
		background: var(--color-bg-hover);
	}

	.rl-ctx-item--danger {
		color: var(--color-danger-text);
	}
</style>
