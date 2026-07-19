<script lang="ts" module>
	import type { FileItem, FolderItem } from '$lib/api/types';

	/**
	 * Per-item envelope info that isn't on `FileItem` / `FolderItem` itself
	 * — supplied by the page in a `contextMap` keyed by item id.
	 *
	 * Example use-cases:
	 * - `/trash`: `date = deletion_date`, `extras = { driveId, trashedAt }`.
	 * - `/recent`: `date = accessed_at`, `ownerId = updated_by`.
	 * - `/favorites`: `ownerId = created_by`.
	 *
	 * All fields are optional; ResourceList falls back to the equivalent
	 * intrinsic on the item (`modified_at`, `created_by`) when absent.
	 */
	export interface ItemContext {
		/**
		 * Overrides `item.modified_at` for the date column + `modifiedAt`
		 * group-by dimension. Accepts epoch (ms or s), an ISO string, or
		 * `null` — same shape `formatDate` and the date-bucket helpers
		 * already tolerate. `/trash` uses ISO strings; `/recent` uses
		 * epoch ms.
		 */
		date?: number | string | null;
		/** Overrides `item.created_by` for the owner column + vignette. */
		ownerId?: string | null;
		/** Free-form extras that page-provided `bucketOf` / `contextActions` read. */
		extras?: Record<string, string | number | null>;
	}

	/**
	 * A group-by ("swimlane") dimension a page can offer. `orderBy` is sent to the
	 * API; the optional `bucketOf` maps an item + its context to a section key,
	 * and `labelOf` maps that key to a header label. Omitting `bucketOf` means a
	 * flat list.
	 */
	export interface GroupByDef {
		key: string;
		label: string;
		orderBy: string;
		/** Optional icon for the dropdown option (defaults to the group glyph). */
		icon?: string;
		bucketOf?: (item: FileItem | FolderItem, ctx?: ItemContext) => string | null;
		labelOf?: (bucketKey: string) => string;
	}

	/** A right-click / overflow context-menu action. */
	export interface ContextAction {
		key: string;
		label: string;
		icon: string;
		danger?: boolean;
		run: (item: FileItem | FolderItem, ctx?: ItemContext) => void;
	}

	/**
	 * True when the item is a file. Uses structural narrowing on
	 * `mime_type` (only present on `FileItem`) so callers can pattern
	 * match without importing the discriminator manually.
	 */
	export function isFile(item: FileItem | FolderItem): item is FileItem {
		return 'mime_type' in item;
	}
</script>

<script lang="ts">
	import type { Snippet } from 'svelte';
	import { SvelteSet } from 'svelte/reactivity';
	import Icon from '$lib/icons/Icon.svelte';
	import EmptyState from '$lib/components/EmptyState.svelte';
	import SkeletonList from '$lib/components/SkeletonList.svelte';
	import ListToolbar from '$lib/components/ListToolbar.svelte';
	import UserVignette from '$lib/components/UserVignette.svelte';
	import VirtualList from '$lib/components/VirtualList.svelte';
	import { t } from '$lib/i18n/index.svelte';
	import { files as filesStore } from '$lib/stores/files.svelte';
	import { preferences } from '$lib/stores/preferences.svelte';
	import { formatBytes } from '$lib/utils/format';
	import { formatDate, iconNameFromClass, fileIconKindClass } from '$lib/utils/display';
	import { gridColumns } from '$lib/utils/grid';
	import { fileThumbnailUrl, thumbSizeForView } from '$lib/api/endpoints/files';
	import {
		canThumbnailClientSide,
		preloadPdf,
		queueGenerate as queueThumbnailGenerate
	} from '$lib/utils/thumbnail';

	interface Props {
		title: string;
		items: Array<FileItem | FolderItem>;
		/**
		 * Per-item envelope info keyed by `item.id`. See `ItemContext`
		 * above. When absent, ResourceList uses the intrinsic item
		 * fields (`modified_at`, `created_by`).
		 */
		contextMap?: Map<string, ItemContext>;
		/**
		 * Set of item ids the caller considers "favorite". When
		 * provided, the star widget renders next to each row and
		 * `onfavorite` is invoked on click. Kept as an external Set so
		 * the page owns the source of truth (e.g. the favorites store).
		 */
		favoriteIds?: Set<string>;
		/**
		 * Resolve `userId → display name`. Optional; when absent
		 * `UserVignette` falls back to its own internal resolution.
		 * Accepts `null` for consistency with the useOwnerCache API
		 * (returns `null` for a not-yet-resolved id).
		 */
		resolveOwnerName?: (userId: string) => string | null | undefined;
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
		dateCell?: Snippet<[FileItem | FolderItem, ItemContext | undefined]>;
		/**
		 * Optional per-bucket action button rendered alongside the swimlane
		 * header label. Receives the bucket key (the value `bucketOf`
		 * returned for the active group-by). Used by the trash page to expose
		 * a per-drive "Empty" affordance — the page decides which group-bys
		 * the action is meaningful for and returns nothing otherwise.
		 */
		bucketAction?: Snippet<[string]>;
		/** Show the owner column + vignette (list view) and hover tooltip. */
		showOwner?: boolean;
		/** Allow grid/list toggle (shares the app-wide view mode). */
		showViewToggle?: boolean;
		/** Show the dotfile-visibility eye toggle in the toolbar AND
		 * apply the corresponding filter to `items` when
		 * `preferences.hideDotfiles` is true. Opt-in per host page —
		 * surfaces that never filter dotfiles (favorites, trash) leave
		 * this false so the button doesn't appear AND the filter never
		 * kicks in. Single flag governs both concerns so a page can't
		 * accidentally expose the button without wiring the filter or
		 * vice-versa.
		 *
		 * A host page that needs to surface "N items hidden" in its
		 * empty state derives that count independently via the shared
		 * `isDotfile` predicate in `$lib/utils/dotfileFilter` — no
		 * count-out prop here (avoids a bindable whose $bindable
		 * default is always shadowed by the effect that would sync it,
		 * and keeps the component's API one-way-inbound). */
		showDotfileToggle?: boolean;
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
		onopen?: (item: FileItem | FolderItem) => void;
		/** Per-item favorite star toggle. */
		onfavorite?: (item: FileItem | FolderItem) => void;
		/** Selection changed (set of selected item ids). */
		onselectionchange?: (ids: Set<string>) => void;
		actions?: Snippet<[FileItem | FolderItem]>;
		toolbar?: Snippet;
		/** Batch toolbar shown when items are selected; receives selected items. */
		batchToolbar?: Snippet<[Array<FileItem | FolderItem>]>;
		/**
		 * Render `<img>` thumbnails on file rows and fall back to
		 * client-side generation when the server doesn't have one
		 * (image / PDF / video via `$lib/utils/thumbnail`). Default on
		 * — every view that lists real files gets the same behaviour.
		 * Set false for views that never benefit (empty states,
		 * synthetic rows).
		 */
		enableThumbnails?: boolean;
		/**
		 * Enable per-row drag/drop hooks. Used by the files browser so
		 * a folder row is a drop target and any row is draggable to
		 * another folder or the breadcrumb. Pages that don't wire these
		 * (trash, favorites, recent, shared-with-me) opt out of the
		 * drag-drop UX entirely by leaving the callbacks unset.
		 */
		isDraggable?: (item: FileItem | FolderItem) => boolean;
		isDropTarget?: (item: FileItem | FolderItem) => boolean;
		/**
		 * Which item id currently shows the drop-target highlight (page
		 * owns the state so it can share it with breadcrumb / other drop
		 * zones). Only meaningful when `isDropTarget` is provided.
		 */
		dropTargetId?: string | null;
		onitemdragstart?: (e: DragEvent, item: FileItem | FolderItem) => void;
		onitemdragover?: (e: DragEvent, item: FileItem | FolderItem) => void;
		onitemdragleave?: (e: DragEvent, item: FileItem | FolderItem) => void;
		onitemdrop?: (e: DragEvent, item: FileItem | FolderItem) => void;
		/**
		 * Override the list-view column header. When provided,
		 * ResourceList renders this instead of its default header —
		 * used by the files browser to expose clickable column-sort
		 * buttons (name / size / type / modified). Pages that override
		 * this typically also handle sorting on their side (pass
		 * pre-sorted `items`) rather than relying on `onreload`.
		 */
		listHeader?: Snippet;
		/**
		 * Open the row on single click (default) vs. double click.
		 * Files browser prefers double-click so single-click can drive
		 * the shift-range selection model without accidentally
		 * navigating.
		 */
		openOnDoubleClick?: boolean;
		/**
		 * Enable shift-click range selection. The row that was clicked
		 * without shift becomes the anchor; the next shift-click
		 * selects the range between anchor and target in visible order.
		 * Requires `selectable`.
		 */
		shiftRangeSelect?: boolean;
	}

	let {
		title,
		items,
		contextMap,
		favoriteIds,
		resolveOwnerName,
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
		bucketAction,
		showOwner = false,
		showViewToggle = true,
		showDotfileToggle = false,
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
		batchToolbar,
		enableThumbnails = true,
		isDraggable,
		isDropTarget,
		dropTargetId = null,
		onitemdragstart,
		onitemdragover,
		onitemdragleave,
		onitemdrop,
		listHeader: listHeaderOverride,
		openOnDoubleClick = false,
		shiftRangeSelect = false
	}: Props = $props();

	// ── Per-item accessors ────────────────────────────────────────────────────
	// Every read of an item field goes through these helpers so the
	// contextMap override for date + owner is centralised. Kept as
	// module-level fns (not $derived) — they run on each row render;
	// caching a Map on every items/contextMap change would be wasteful.
	function ctxOf(id: string): ItemContext | undefined {
		return contextMap?.get(id);
	}
	function dateOf(item: FileItem | FolderItem): number | string | null {
		return ctxOf(item.id)?.date ?? item.modified_at;
	}
	function ownerIdOf(item: FileItem | FolderItem): string | null {
		const ctx = ctxOf(item.id);
		return ctx && 'ownerId' in ctx ? (ctx.ownerId ?? null) : (item.created_by ?? null);
	}
	function sizeOf(item: FileItem | FolderItem): number | null {
		return isFile(item) ? item.size : null;
	}
	function mimeOf(item: FileItem | FolderItem): string | null {
		return isFile(item) ? item.mime_type : null;
	}
	function iconClassOf(item: FileItem | FolderItem): string {
		return item.icon_class;
	}

	// ── Dotfile filter ────────────────────────────────────────────────────────
	// Two conditions gate the filter (both must be true):
	//   1. Host page opted in via `showDotfileToggle` — so pages where
	//      dotfiles are always visible (favorites, trash) never hide them
	//      even if the user's global preference is on.
	//   2. User preference is set to hide — read from the reactive
	//      `preferences.hideDotfiles` getter, so a toolbar click flips
	//      this list in real time without a reload.
	// The `visibleItems` derived is what every downstream reader
	// (bucketing, rendering, "all-selected", range-select) uses, so
	// hidden rows disappear consistently across grid, list, and every
	// group-by dimension. `selectedItems` and the reap-stale-selection
	// effect stay on the raw `items` — selection persists across a
	// display filter toggle, matching how file managers treat a
	// filter-hide as "hidden, not gone".
	const filterDotfiles = $derived(showDotfileToggle && preferences.hideDotfiles);
	const visibleItems = $derived(
		filterDotfiles ? items.filter((i) => !i.name.startsWith('.')) : items
	);

	// isEmpty tracks the VISIBLE list — an all-dotfile page with the
	// filter on shows the empty state (the host page's `emptyHint` can
	// reference `hiddenCount` to say "3 items hidden by the filter").
	const isEmpty = $derived(visibleItems.length === 0);
	/** Content width, for computing the grid's column count to match auto-fill. */
	let gridWidth = $state(0);
	const gridCols = $derived(gridColumns(gridWidth));

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
	const sections = $derived.by(
		(): Array<{ key: string; label: string; rows: Array<FileItem | FolderItem> }> => {
			const bucketOf = activeGroup?.bucketOf;
			if (!bucketOf) return [{ key: '', label: '', rows: visibleItems }];
			const order: string[] = [];
			// Transient bucketing map computed inside $derived.by — not reactive state.
			// eslint-disable-next-line svelte/prefer-svelte-reactivity
			const map = new Map<string, Array<FileItem | FolderItem>>();
			for (const item of visibleItems) {
				const k = bucketOf(item, ctxOf(item.id)) ?? '∅';
				if (!map.has(k)) {
					map.set(k, []);
					order.push(k);
				}
				map.get(k)!.push(item);
			}
			return order.map((k) => ({
				key: k,
				label: activeGroup?.labelOf?.(k) ?? k,
				rows: map.get(k)!
			}));
		}
	);
	const grouped = $derived(!!activeGroup?.bucketOf);

	// ── Selection ─────────────────────────────────────────────────────────────
	// SvelteSet is reactive on its own; mutate in place rather than reassigning.
	const selected = new SvelteSet<string>();

	function toggleSelected(id: string) {
		if (selected.has(id)) selected.delete(id);
		else selected.add(id);
		onselectionchange?.(selected);
	}

	/**
	 * Anchor id for shift-range selection. The row clicked without
	 * shift becomes the anchor; the next shift-click selects every
	 * row between anchor and target in visible order. Kept in module
	 * state so it survives re-renders that don't drop the component.
	 */
	let selectionAnchor = $state<string | null>(null);
	function selectRange(anchorId: string, targetId: string) {
		// Range-select over the VISIBLE order — a shift-click can't reach
		// a row the user can't see.
		const order = visibleItems.map((i) => i.id);
		const a = order.indexOf(anchorId);
		const b = order.indexOf(targetId);
		if (a < 0 || b < 0) return;
		const [lo, hi] = a < b ? [a, b] : [b, a];
		for (let i = lo; i <= hi; i++) selected.add(order[i]);
		onselectionchange?.(selected);
	}
	/**
	 * Left-click handler that either navigates (`onopen`) or manages
	 * selection depending on modifiers + config. Returns `true` when
	 * the click was consumed by selection, so callers can suppress the
	 * open. Enabled only for `selectable + shiftRangeSelect` callers.
	 */
	function handleRowClick(e: MouseEvent, id: string): boolean {
		if (!selectable || !shiftRangeSelect) return false;
		if (e.shiftKey && selectionAnchor) {
			e.preventDefault();
			selectRange(selectionAnchor, id);
			return true;
		}
		if (e.metaKey || e.ctrlKey) {
			e.preventDefault();
			toggleSelected(id);
			selectionAnchor = id;
			return true;
		}
		// Plain click: only sets the anchor; open (if any) still fires.
		selectionAnchor = id;
		return false;
	}
	function clearSelection() {
		selected.clear();
		onselectionchange?.(selected);
	}
	// "All-selected" means every VISIBLE row is selected — hiding
	// dotfiles by preference shouldn't be confused with "not selected".
	const allSelected = $derived(
		visibleItems.length > 0 && visibleItems.every((i) => selected.has(i.id))
	);
	function toggleSelectAll() {
		if (allSelected) clearSelection();
		else {
			selected.clear();
			// Select all VISIBLE rows only. A user hiding dotfiles then
			// pressing select-all shouldn't sweep in the hidden files
			// they can't see — that would be a footgun for destructive
			// batch actions.
			for (const i of visibleItems) selected.add(i.id);
			onselectionchange?.(selected);
		}
	}
	// `selectedItems` and the reap-stale effect below stay on the RAW
	// items — selection persists across a display-filter toggle, and
	// stale-selection cleanup only fires when items truly leave the
	// dataset (reload, delete, etc.), not when the filter hides them.
	//
	// Index rebuilt only when `items` changes; the projection is then
	// O(k · log k) in the selection size k instead of a full O(N) re-scan
	// of the list on every toggle (O(N²)-ish across a shift-range gesture
	// once the batch toolbar is mounted — benches/ROUND11.md §S1). The
	// index sort preserves item order, so the toolbar sees the same array
	// the old filter produced.
	const itemIndexById = $derived(new Map(items.map((i, idx) => [i.id, idx])));
	const selectedItems = $derived.by(() => {
		const picked: { idx: number; item: FileItem | FolderItem }[] = [];
		for (const id of selected) {
			const idx = itemIndexById.get(id);
			if (idx !== undefined) picked.push({ idx, item: items[idx] });
		}
		picked.sort((a, b) => a.idx - b.idx);
		return picked.map((p) => p.item);
	});

	// Drop selection ids that are no longer present after a reload.
	$effect(() => {
		// With nothing selected (the common case) every infinite-scroll page
		// re-fired this effect and built a throwaway O(N) id Set for a loop
		// that never runs — skip straight out. `selected.size` is reactive,
		// so the effect re-fires when a selection appears.
		if (selected.size === 0) return;
		const ids = new Set(items.map((i) => i.id));
		let changed = false;
		for (const id of selected) {
			if (!ids.has(id)) {
				selected.delete(id);
				changed = true;
			}
		}
		if (changed) onselectionchange?.(selected);
	});

	// ── Right-click context menu ──────────────────────────────────────────────
	let ctxOpen = $state(false);
	let ctxX = $state(0);
	let ctxY = $state(0);
	let ctxItem = $state<FileItem | FolderItem | null>(null);

	function openContext(e: MouseEvent, item: FileItem | FolderItem) {
		if (!contextActions?.length) return;
		e.preventDefault();
		e.stopPropagation();
		ctxItem = item;
		ctxX = Math.min(e.clientX, window.innerWidth - 220);
		ctxY = Math.min(e.clientY, window.innerHeight - (contextActions.length * 44 + 24));
		ctxOpen = true;
	}
	function closeContext() {
		ctxOpen = false;
		ctxItem = null;
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

	function ownerTitle(item: FileItem | FolderItem): string {
		const ownerId = ownerIdOf(item);
		const owner = ownerId ? (resolveOwnerName?.(ownerId) ?? ownerId) : '';
		const path = item.path ?? '';
		return [
			owner && `${t('files.col_owner', 'Owner')}: ${owner}`,
			path && `${t('files.col_path', 'Location')}: ${path}`
		]
			.filter(Boolean)
			.join('\n');
	}
</script>

{#snippet row(item: FileItem | FolderItem)}
	{@const kind = isFile(item) ? 'file' : 'folder'}
	{@const iconName = kind === 'folder' ? 'folder' : iconNameFromClass(iconClassOf(item))}
	{@const isFav = favoriteIds?.has(item.id) ?? false}
	{@const ctx = ctxOf(item.id)}
	{@const ownerId = ownerIdOf(item)}
	{@const dateVal = dateOf(item)}
	{@const sizeVal = sizeOf(item)}
	{@const mimeVal = mimeOf(item)}
	{@const draggable = isDraggable?.(item) ?? false}
	{@const dropTarget = isDropTarget?.(item) ?? false}
	<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
	<div
		class="file-item"
		class:file-item--selected={selectable && selected.has(item.id)}
		class:file-item--drop-target={dropTarget && dropTargetId === item.id}
		role={onopen ? 'button' : undefined}
		tabindex={onopen ? 0 : undefined}
		aria-label={onopen ? item.name : undefined}
		data-testid={item.name}
		title={showOwner ? ownerTitle(item) : undefined}
		{draggable}
		ondragstart={draggable && onitemdragstart ? (e) => onitemdragstart(e, item) : undefined}
		ondragover={dropTarget && onitemdragover ? (e) => onitemdragover(e, item) : undefined}
		ondragleave={dropTarget && onitemdragleave ? (e) => onitemdragleave(e, item) : undefined}
		ondrop={dropTarget && onitemdrop ? (e) => onitemdrop(e, item) : undefined}
		onclick={onopen
			? (e) => {
					// Selection-first for shift/meta clicks; only "open" fires on a
					// plain click when the click wasn't consumed by selection.
					if (handleRowClick(e, item.id)) return;
					if (!openOnDoubleClick) onopen(item);
				}
			: undefined}
		ondblclick={onopen && openOnDoubleClick ? () => onopen(item) : undefined}
		onkeydown={onopen ? (e) => e.key === 'Enter' && onopen(item) : undefined}
		oncontextmenu={contextActions?.length ? (e) => openContext(e, item) : undefined}
	>
		{#if selectable}
			<div class="select-cell" role="presentation" onclick={(e) => e.stopPropagation()}>
				<input
					type="checkbox"
					aria-label={t('common.select', 'Select')}
					data-testid={`resource-list-select-${item.id}-checkbox`}
					checked={selected.has(item.id)}
					onchange={() => toggleSelected(item.id)}
				/>
			</div>
		{/if}
		<div class="name-cell">
			<span class="file-icon {fileIconKindClass(iconName)}">
				<!-- Type icon always renders. When `enableThumbnails` is on
				     and the item is a file with a supported mime, an
				     `<img>` overlays the icon on success; onerror hides it
				     (revealing the icon) and kicks off client-side
				     generation for image / PDF / video so the next viewer
				     hits the server-side thumbnail. -->
				<Icon name={iconName} />
				{#if enableThumbnails && kind === 'file' && mimeVal && canThumbnailClientSide( { id: item.id, name: item.name, mime_type: mimeVal } )}
					<img
						class="file-thumb"
						src={fileThumbnailUrl(item.id, thumbSizeForView(filesStore.viewMode))}
						alt=""
						loading="lazy"
						onerror={(e) => {
							const img = e.currentTarget as HTMLImageElement;
							img.style.display = 'none';
							if (mimeVal === 'application/pdf') preloadPdf();
							void queueThumbnailGenerate(
								{ id: item.id, name: item.name, mime_type: mimeVal },
								(dataUrl) => {
									img.src = dataUrl;
									img.style.display = '';
								}
							);
						}}
					/>
				{/if}
			</span>
			<span class="name-cell__text">{item.name}</span>
		</div>
		{#if showOwner}
			<div class="owner-cell">
				{#if ownerId}
					<UserVignette userId={ownerId} fallbackLabel={resolveOwnerName?.(ownerId) ?? undefined} />
				{:else}
					<span class="owner-cell__placeholder">—</span>
				{/if}
			</div>
		{/if}
		{#if showPath}<div class="path-cell">{item.path ?? ''}</div>{/if}
		{#if showType}<div class="type-cell">{item.category ?? ''}</div>{/if}
		{#if showSize}
			<div class="size-cell">{sizeVal != null ? formatBytes(sizeVal) : '—'}</div>
		{/if}
		{#if showDate}
			<div class="date-cell">
				{#if dateCell}{@render dateCell(item, ctx)}{:else}{formatDate(dateVal)}{/if}
			</div>
		{/if}
		<div class="grid-meta">
			{#if showDate && dateCell}<span class="grid-meta__chip">{@render dateCell(item, ctx)}</span
				>{/if}
			<span class="grid-meta__line">
				{#if sizeVal != null}<span class="grid-meta__size">{formatBytes(sizeVal)}</span>{/if}
				{#if dateVal != null}<span class="grid-meta__date">{formatDate(dateVal)}</span>{/if}
			</span>
		</div>
		{#if onfavorite}
			<button
				class="rl-star"
				class:rl-star--on={isFav}
				data-testid={`resource-list-favorite-${item.id}-btn`}
				title={isFav
					? t('files.unfavorite', 'Remove favorite')
					: t('files.favorite', 'Add favorite')}
				aria-pressed={isFav}
				onclick={(e) => {
					e.stopPropagation();
					onfavorite(item);
				}}><Icon name={isFav ? 'star' : 'star-outline'} /></button
			>
		{/if}
		{#if actions}
			<div class="action-cell">{@render actions(item)}</div>
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
		{showDotfileToggle}
	>
		{#snippet start()}
			<div class="action-buttons">{@render toolbar?.()}</div>
		{/snippet}
	</ListToolbar>
</div>

{#if selectable && selected.size > 0 && batchToolbar}
	<div
		class="rl-batch"
		role="region"
		aria-label={t('files.selection', 'Selection')}
		data-testid="resource-list-batch-toolbar"
	>
		<button
			class="rl-batch__close"
			title={t('common.clear', 'Clear')}
			data-testid="resource-list-batch-close-btn"
			onclick={clearSelection}
		>
			<Icon name="times" />
		</button>
		<span class="rl-batch__count"
			>{t('files.selected_count', { count: selected.size }, '{{count}} selected')}</span
		>
		<div class="rl-batch__actions">{@render batchToolbar(selectedItems)}</div>
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
	<div class="files-container" bind:clientWidth={gridWidth}>
		{#if grouped && filesStore.viewMode === 'list'}
			<div class="files-list-view" style="--files-list-columns: {columns}">
				{#if listHeaderOverride}{@render listHeaderOverride()}{:else}{@render listHeader()}{/if}
				{#each sections as section (section.key)}
					<div class="rl-swimlane-header" role="rowheader">
						<span class="rl-swimlane-header__label">{section.label}</span>
						{#if bucketAction}
							<span class="rl-swimlane-header__action">
								{@render bucketAction(section.key)}
							</span>
						{/if}
					</div>
					<!-- Window each section's rows so a large grouped list (e.g. a big
					     trash, grouped by remaining days) doesn't mount every row. -->
					<VirtualList items={section.rows} rowHeight={56} key={(e) => e.id} {row} />
				{/each}
			</div>
		{:else if grouped}
			<!-- Grouped GRID: a vertical stack of (header + its own windowed card
			     grid) per section. The outer is a flex column, NOT `.files-grid-view`
			     (which is itself a grid and would place each header/VirtualList into a
			     cell) — the grid lives on each VirtualList's inner window via
			     `windowClass`, exactly like the flat-grid arm. This was the last
			     unwindowed path: a grouped-by-default grid (trash) mounted every card
			     (benches/ROUND13.md §V1). -->
			<div class="rl-grouped-grid">
				{#each sections as section (section.key)}
					<div class="rl-swimlane-header rl-swimlane-header--grid" role="rowheader">
						<span class="rl-swimlane-header__label">{section.label}</span>
						{#if bucketAction}
							<span class="rl-swimlane-header__action">
								{@render bucketAction(section.key)}
							</span>
						{/if}
					</div>
					<VirtualList
						items={section.rows}
						columns={gridCols}
						rowHeight={240}
						windowClass="files-grid-view"
						key={(e) => e.id}
						{row}
					/>
				{/each}
			</div>
		{:else if filesStore.viewMode === 'list'}
			<!-- Flat list view: only the visible rows are mounted. The spacer keeps the
			     full scroll height so the end-of-list sentinel still fires. -->
			<div class="files-list-view" style="--files-list-columns: {columns}">
				{#if listHeaderOverride}{@render listHeaderOverride()}{:else}{@render listHeader()}{/if}
				<VirtualList items={visibleItems} rowHeight={56} key={(e) => e.id} {row} />
			</div>
		{:else}
			<!-- Grid view: the windowed list's inner element IS the card grid. -->
			<VirtualList
				items={visibleItems}
				columns={gridCols}
				rowHeight={240}
				windowClass="files-grid-view"
				key={(e) => e.id}
				{row}
			/>
		{/if}

		{#if hasMore}
			<button
				class="btn btn-secondary rl-more"
				data-testid="resource-list-load-more-btn"
				onclick={onloadmore}
				disabled={loading}
			>
				{loading ? t('common.loading', 'Loading…') : t('common.load_more', 'Load more')}
			</button>
		{/if}
		<!-- Infinite-scroll sentinel: auto-loads the next page as it nears the viewport. -->
		<div bind:this={sentinel} class="rl-sentinel" aria-hidden="true"></div>
	</div>
{/if}

{#snippet listHeader()}
	<div class="list-header">
		{#if selectable}
			<div class="select-cell">
				<input
					type="checkbox"
					aria-label={t('common.select_all', 'Select all')}
					data-testid="resource-list-select-all-checkbox"
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
{/snippet}

{#if ctxOpen && ctxItem && contextActions}
	<div
		class="rl-ctx-scrim"
		role="presentation"
		onclick={closeContext}
		oncontextmenu={(e) => e.preventDefault()}
	></div>
	<div
		class="rl-ctx-menu"
		style:left="{ctxX}px"
		style:top="{ctxY}px"
		role="menu"
		data-testid="resource-list-context-menu"
	>
		{#each contextActions as action (action.key)}
			<button
				class="rl-ctx-item"
				class:rl-ctx-item--danger={action.danger}
				role="menuitem"
				data-testid={`resource-list-context-${action.key}-item`}
				onclick={() => {
					const target = ctxItem!;
					closeContext();
					action.run(target, ctxOf(target.id));
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

	/* Drop-target highlight — mirrors the legacy files browser's cue when
	   dragging a row over a folder row. */
	.file-item--drop-target {
		outline: 2px dashed var(--color-accent);
		outline-offset: -2px;
	}

	/* ── Owner vignette ── */
	.owner-cell {
		display: flex;
		align-items: center;
		min-width: 0;
	}

	.owner-cell__placeholder {
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
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-2);
	}

	/* Optional per-bucket action (e.g. trash page's per-drive Empty). */
	.rl-swimlane-header__action {
		display: inline-flex;
		align-items: center;
	}

	/* Grouped-grid container: a vertical stack of (header + its own windowed
	   card grid) per section. Not `.files-grid-view` — the grid is on each
	   VirtualList's inner window, so this outer element just stacks. */
	.rl-grouped-grid {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
	}

	/* In the flex stack the `grid-column: 1 / -1` span (meant for the grid
	   context) is inert; the header spans naturally as a block-level flex
	   child. */
	.rl-swimlane-header--grid {
		grid-column: auto;
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
		/* Danger *foreground* on the light menu surface — the red accent, not
		   --color-danger-text (white, for text ON a red fill, invisible here). */
		color: var(--color-danger-alt);
	}
</style>
