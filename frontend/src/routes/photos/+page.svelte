<script lang="ts">
	import Button from '$lib/components/Button.svelte';
	import EmptyState from '$lib/components/EmptyState.svelte';
	import VirtualRows from '$lib/components/VirtualRows.svelte';
	import { lazyComponent } from '$lib/composables/lazyComponent.svelte';
	import { useSelection } from '$lib/composables/useSelection.svelte';
	import { errorToast } from '$lib/utils/errors';
	import { onMount } from 'svelte';
	import { batchTrash, fetchPhotos, type PhotoItem } from '$lib/api/endpoints/photos';
	import { peopleEnabled } from '$lib/api/endpoints/people';
	import { fileDownloadUrl, fileThumbnailUrl } from '$lib/api/endpoints/files';
	import Icon from '$lib/icons/Icon.svelte';
	import { confirmDialog } from '$lib/stores/dialogs.svelte';
	import { t } from '$lib/i18n/index.svelte';
	import { ui } from '$lib/stores/ui.svelte';
	import { isVideo, photoTimestamp } from '$lib/utils/media';

	type Tab = 'moments' | 'places' | 'people';
	let tab = $state<Tab>('moments');

	// The lightbox, the (maplibre-backed) places map and the people view are all
	// heavy and off the initial path, so each loads on first use: the lightbox
	// when a photo is opened, the map/people views when their tab is selected.
	const photoLightbox = lazyComponent(() => import('$lib/components/PhotoLightbox.svelte'));
	const placesMap = lazyComponent(() => import('$lib/components/PlacesMap.svelte'));
	const peopleView = lazyComponent(() => import('$lib/components/PeopleView.svelte'));
	let peopleAvailable = $state(false);

	let items = $state<PhotoItem[]>([]);
	let cursor = $state<string | null>(null);
	let exhausted = $state(false);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let sentinel = $state<HTMLElement | null>(null);
	/** Usable content width of the grid, for the justified layout. */
	let gridWidth = $state(0);

	type GroupMode = 'day' | 'month' | 'year';
	type LayoutMode = 'square' | 'justified';
	const GROUP_KEY = 'oxi-photos-group';
	const LAYOUT_KEY = 'oxi-photos-layout';
	let groupMode = $state<GroupMode>('month');
	let layoutMode = $state<LayoutMode>('square');
	const selected = useSelection();
	let lightbox = $state(-1); // index into `items`, -1 = closed

	$effect(() => {
		if (lightbox >= 0) void photoLightbox.load();
		if (tab === 'places') void placesMap.load();
		else if (tab === 'people') void peopleView.load();
	});

	/** EXIF-aware timestamp (seconds → ms), matching the OLD grouping logic. */
	function bucketKey(d: Date): string {
		const y = d.getFullYear();
		if (groupMode === 'year') return `${y}`;
		const m = `${d.getMonth() + 1}`.padStart(2, '0');
		if (groupMode === 'month') return `${y}-${m}`;
		return `${y}-${m}-${`${d.getDate()}`.padStart(2, '0')}`;
	}

	function bucketLabel(d: Date): string {
		if (groupMode === 'year') return `${d.getFullYear()}`;
		if (groupMode === 'month')
			return d.toLocaleDateString(undefined, { year: 'numeric', month: 'long' });
		return d.toLocaleDateString(undefined, {
			weekday: 'long',
			year: 'numeric',
			month: 'long',
			day: 'numeric'
		});
	}

	const groups = $derived.by(() => {
		const out: Array<{ key: string; label: string; photos: PhotoItem[] }> = [];
		// Transient scratch map built inside $derived.by and discarded — not reactive state.
		// eslint-disable-next-line svelte/prefer-svelte-reactivity
		const index = new Map<string, number>();
		for (const p of items) {
			const d = new Date(photoTimestamp(p));
			const key = bucketKey(d);
			let i = index.get(key);
			if (i === undefined) {
				i = out.length;
				index.set(key, i);
				out.push({ key, label: bucketLabel(d), photos: [] });
			}
			out[i].photos.push(p);
		}
		return out;
	});

	interface JustifiedTile {
		file: PhotoItem;
		w: number;
		h: number;
	}

	/**
	 * Pack files into justified rows (Flickr-style): each full row is scaled to
	 * fill `width` while preserving every tile's aspect ratio. Missing dimensions
	 * fall back to 1:1.
	 */
	function justifiedRows(
		files: PhotoItem[],
		width: number
	): Array<{ height: number; tiles: JustifiedTile[] }> {
		const gap = 8;
		const target = window.matchMedia('(max-width: 768px)').matches ? 150 : 200;
		const rows: Array<{ height: number; tiles: JustifiedTile[] }> = [];
		let cur: Array<{ file: PhotoItem; aspect: number }> = [];
		let aspectSum = 0;
		for (const file of files) {
			let aspect = file.width && file.height ? file.width / file.height : 1;
			if (!Number.isFinite(aspect) || aspect <= 0) aspect = 1;
			aspect = Math.min(Math.max(aspect, 0.4), 3);
			cur.push({ file, aspect });
			aspectSum += aspect;
			const rowWidth = aspectSum * target + (cur.length - 1) * gap;
			if (rowWidth >= width) {
				const h = (width - (cur.length - 1) * gap) / aspectSum;
				rows.push({
					height: Math.round(h),
					tiles: cur.map((tt) => ({
						file: tt.file,
						w: Math.max(1, Math.round(tt.aspect * h)),
						h: Math.round(h)
					}))
				});
				cur = [];
				aspectSum = 0;
			}
		}
		if (cur.length) {
			rows.push({
				height: target,
				tiles: cur.map((tt) => ({
					file: tt.file,
					w: Math.max(1, Math.round(tt.aspect * target)),
					h: target
				}))
			});
		}
		return rows;
	}

	// ── Virtualized row model ────────────────────────────────────────────────
	// Flatten the groups into a single list of fixed-height rows (a date header
	// or a strip of sized tiles), so VirtualRows can window the whole timeline —
	// only the rows near the viewport are mounted, regardless of library size.
	const SQUARE_GAP = 4; // .25rem, matches the old grid gap
	const SQUARE_MIN = 144; // 9rem minmax floor
	const JUSTIFIED_GAP = 8; // .photos-jrow margin-bottom
	const HEADER_H = 44;

	type PhotoRow =
		| { kind: 'header'; key: string; height: number; label: string; count: number }
		| { kind: 'tiles'; key: string; height: number; gap: number; tiles: JustifiedTile[] };

	const photoRows = $derived.by<PhotoRow[]>(() => {
		const W = gridWidth;
		if (W <= 0) return [];
		const rows: PhotoRow[] = [];
		const cols = Math.max(1, Math.floor((W + SQUARE_GAP) / (SQUARE_MIN + SQUARE_GAP)));
		const cell = (W - (cols - 1) * SQUARE_GAP) / cols;
		for (const g of groups) {
			rows.push({
				kind: 'header',
				key: `h:${g.key}`,
				height: HEADER_H,
				label: g.label,
				count: g.photos.length
			});
			if (layoutMode === 'justified') {
				const jrows = justifiedRows(g.photos, W);
				for (let ri = 0; ri < jrows.length; ri++) {
					rows.push({
						kind: 'tiles',
						key: `${g.key}:j${ri}`,
						height: jrows[ri].height + JUSTIFIED_GAP,
						gap: JUSTIFIED_GAP,
						tiles: jrows[ri].tiles
					});
				}
			} else {
				for (let i = 0; i < g.photos.length; i += cols) {
					const tiles = g.photos.slice(i, i + cols).map((file) => ({ file, w: cell, h: cell }));
					rows.push({
						kind: 'tiles',
						key: `${g.key}:s${i}`,
						height: cell + SQUARE_GAP,
						gap: SQUARE_GAP,
						tiles
					});
				}
			}
		}
		return rows;
	});

	async function loadMore() {
		if (loading || exhausted) return;
		loading = true;
		error = null;
		try {
			const page = await fetchPhotos(60, cursor);
			items = [...items, ...page.items];
			cursor = page.nextCursor;
			if (!page.nextCursor) exhausted = true;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
			exhausted = true;
		} finally {
			loading = false;
		}
	}

	function setGroupMode(m: GroupMode) {
		if (groupMode === m) return;
		groupMode = m;
		if (typeof localStorage !== 'undefined') localStorage.setItem(GROUP_KEY, m);
	}

	function setLayoutMode(m: LayoutMode) {
		if (layoutMode === m) return;
		layoutMode = m;
		if (typeof localStorage !== 'undefined') localStorage.setItem(LAYOUT_KEY, m);
	}

	/** A plain tile click toggles selection once anything is selected, else opens the lightbox. */
	function onTileClick(p: PhotoItem) {
		if (selected.size > 0) selected.toggle(p.id);
		else lightbox = items.findIndex((x) => x.id === p.id);
	}

	function onDeletePhoto(id: string) {
		items = items.filter((p) => p.id !== id);
		selected.delete(id);
	}

	function downloadSelected() {
		for (const id of selected.ids) {
			const a = document.createElement('a');
			a.href = fileDownloadUrl(id);
			a.download = '';
			document.body.appendChild(a);
			a.click();
			a.remove();
		}
	}

	async function trashSelected() {
		const ids = selected.values();
		const ok = await confirmDialog({
			title: t('photos.delete', 'Delete photos'),
			message: t('photos.confirm_delete', { n: ids.length }, 'Move {{n}} photos to trash?'),
			confirmText: t('common.delete', 'Delete'),
			danger: true
		});
		if (!ok) return;
		try {
			const trashed = await batchTrash(ids);
			if (trashed.size > 0) {
				items = items.filter((p) => !trashed.has(p.id));
				for (const id of trashed) selected.delete(id);
			}
			if (trashed.size < ids.length) {
				ui.notify(
					t(
						'photos.trash_partial',
						{ ok: trashed.size, total: ids.length },
						'{{ok}} of {{total}} moved to trash.'
					),
					'warning'
				);
			} else {
				ui.notify(t('photos.trashed', { n: trashed.size }, '{{n}} moved to trash.'), 'success');
			}
		} catch (e) {
			errorToast(e);
		}
	}

	/**
	 * A tile thumbnail failed to load — no server thumbnail yet (SVGs the backend
	 * can't rasterise; a video whose server-side frame extraction is still running
	 * or unavailable) or a transient error. Hide the broken <img> so the
	 * always-present placeholder (and, for videos, the play badge) shows through.
	 *
	 * Video thumbnails are now produced server-side (ffmpeg) on upload through the
	 * same WebP pipeline as photos — the browser no longer re-downloads the video
	 * to extract a frame.
	 */
	function onThumbError(e: Event) {
		(e.currentTarget as HTMLImageElement).style.display = 'none';
	}

	onMount(() => {
		const savedGroup = typeof localStorage !== 'undefined' ? localStorage.getItem(GROUP_KEY) : null;
		if (savedGroup === 'day' || savedGroup === 'month' || savedGroup === 'year')
			groupMode = savedGroup;
		const savedLayout =
			typeof localStorage !== 'undefined' ? localStorage.getItem(LAYOUT_KEY) : null;
		if (savedLayout === 'square' || savedLayout === 'justified') layoutMode = savedLayout;
		void loadMore();
		void peopleEnabled().then((ok) => (peopleAvailable = ok));
		if (!sentinel) return;
		const obs = new IntersectionObserver(
			(entries) => {
				if (entries.some((e) => e.isIntersecting)) void loadMore();
			},
			{ rootMargin: '600px' }
		);
		obs.observe(sentinel);
		return () => obs.disconnect();
	});

	const MODES: GroupMode[] = ['day', 'month', 'year'];
</script>

<svelte:head><title>{t('nav.photos', 'Photos')} · OxiCloud</title></svelte:head>

<div class="page-sticky-header photos-head">
	<h1 class="page-title">{t('nav.photos', 'Photos')}</h1>
	<div class="photos-subnav" role="tablist" aria-label={t('nav.photos', 'Photos')}>
		<button
			class="subnav__tab"
			class:active={tab === 'moments'}
			role="tab"
			aria-selected={tab === 'moments'}
			data-testid="photos-tab-moments"
			onclick={() => (tab = 'moments')}
		>
			{t('photos.tab_moments', 'Moments')}
		</button>
		<button
			class="subnav__tab"
			class:active={tab === 'places'}
			role="tab"
			aria-selected={tab === 'places'}
			data-testid="photos-tab-places"
			onclick={() => (tab = 'places')}
		>
			{t('photos.tab_places', 'Places')}
		</button>
		{#if peopleAvailable}
			<button
				class="subnav__tab"
				class:active={tab === 'people'}
				role="tab"
				aria-selected={tab === 'people'}
				data-testid="photos-tab-people"
				onclick={() => (tab = 'people')}
			>
				{t('photos.tab_people', 'People')}
			</button>
		{/if}
	</div>
</div>

{#if tab === 'moments'}
	<div class="photos-toolbar">
		<div class="seg" role="group" aria-label={t('photos.group_by', 'Group by')}>
			{#each MODES as m (m)}
				<button class="seg__btn" class:active={groupMode === m} onclick={() => setGroupMode(m)}>
					{t(`photos.${m}`, m)}
				</button>
			{/each}
		</div>
		<div class="seg" role="group" aria-label={t('photos.layout_square', 'Layout')}>
			<button
				class="seg__btn"
				class:active={layoutMode === 'square'}
				title={t('photos.layout_square', 'Grid')}
				aria-label={t('photos.layout_square', 'Grid')}
				data-testid="photos-layout-square-btn"
				onclick={() => setLayoutMode('square')}><Icon name="th" /></button
			>
			<button
				class="seg__btn"
				class:active={layoutMode === 'justified'}
				title={t('photos.layout_justified', 'Justified')}
				aria-label={t('photos.layout_justified', 'Justified')}
				data-testid="photos-layout-justified-btn"
				onclick={() => setLayoutMode('justified')}><Icon name="layer-group" /></button
			>
		</div>
	</div>

	{#if selected.size > 0}
		<div class="batch-bar" data-testid="photos-batch-bar">
			<span>{t('files.selected_count', { count: selected.size }, '{{count}} selected')}</span>
			<div class="batch-bar__actions">
				<Button data-testid="photos-batch-download-btn" onclick={downloadSelected}
					>{t('common.download', 'Download')}</Button
				>
				<Button data-testid="photos-batch-clear-btn" onclick={() => selected.clear()}
					>{t('common.clear', 'Clear')}</Button
				>
				<Button data-testid="photos-batch-delete-btn" variant="danger" onclick={trashSelected}
					>{t('common.delete', 'Delete')}</Button
				>
			</div>
		</div>
	{/if}

	{#if error}
		<p class="status status--error" role="alert">{error}</p>
	{:else if items.length === 0 && exhausted}
		<EmptyState
			icon="images"
			title={t('photos.empty', 'No photos yet.')}
			hint={t(
				'photos.empty_hint',
				'Photos and videos you upload will appear here, grouped by date.'
			)}
		/>
	{:else}
		<div class="photos-area">
			<div class="photos-measure" bind:clientWidth={gridWidth}>
				{#if photoRows.length}
					<VirtualRows rows={photoRows} overscan={1000}>
						{#snippet row(r)}
							{#if r.kind === 'header'}
								<div class="photos-group" style:height="{r.height}px">
									{r.label} <span class="photos-group__count">{r.count}</span>
								</div>
							{:else}
								<div class="photos-strip" style:height="{r.height}px" style:gap="{r.gap}px">
									{#each r.tiles as cell (cell.file.id)}
										{@render tile(cell.file, `width:${cell.w}px;height:${cell.h}px`)}
									{/each}
								</div>
							{/if}
						{/snippet}
					</VirtualRows>
				{/if}
			</div>
		</div>
	{/if}

	<div bind:this={sentinel} class="sentinel" aria-hidden="true"></div>
	{#if loading}<p class="status">{t('common.loading', 'Loading…')}</p>{/if}

	{#if photoLightbox.component}
		{@const PhotoLightbox = photoLightbox.component}
		<PhotoLightbox {items} bind:index={lightbox} onDelete={onDeletePhoto} />
	{/if}
{:else if tab === 'places'}
	{#if placesMap.component}
		{@const PlacesMap = placesMap.component}
		<PlacesMap />
	{/if}
{:else if tab === 'people'}
	{#if peopleView.component}
		{@const PeopleView = peopleView.component}
		<PeopleView />
	{/if}
{/if}

{#snippet tile(photo: PhotoItem, sizeStyle?: string)}
	<div class="photo-tile" class:selected={selected.has(photo.id)} style={sizeStyle}>
		<button
			class="photo-tile__open"
			data-testid={`photo-tile-${photo.id}`}
			onclick={() => onTileClick(photo)}
		>
			<!-- Always-present placeholder: the thumbnail <img> overlays it and, when
			     it can't load (no server thumbnail, e.g. SVG), hides itself to reveal
			     this default rather than the browser's broken-image glyph. -->
			<span class="photo-tile__placeholder" aria-hidden="true"><Icon name="file-image" /></span>
			<img
				src={fileThumbnailUrl(photo.id, 'preview')}
				srcset={`${fileThumbnailUrl(photo.id, 'icon')} 150w, ${fileThumbnailUrl(photo.id, 'preview')} 400w, ${fileThumbnailUrl(photo.id, 'large')} 800w`}
				sizes="(max-width: 768px) 33vw, 200px"
				alt={photo.name}
				loading="lazy"
				decoding="async"
				onerror={onThumbError}
			/>
			{#if isVideo(photo)}
				<span class="photo-tile__video-badge" aria-hidden="true"><Icon name="play" /></span>
			{/if}
		</button>
		<button
			class="photo-tile__check"
			class:on={selected.has(photo.id)}
			aria-label={t('common.select', 'Select')}
			data-testid={`photo-tile-check-${photo.id}`}
			onclick={() => selected.toggle(photo.id)}
		>
			<Icon name="check" />
		</button>
	</div>
{/snippet}

<style>
	.photos-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		flex-wrap: wrap;
		padding: 1rem 1rem 0;
	}

	.page-title {
		margin: 0;
		font-size: 1.5rem;
		color: var(--color-text-heading);
	}

	.photos-subnav {
		display: flex;
		gap: var(--space-1);
	}

	.subnav__tab {
		padding: var(--space-2) var(--space-3);
		border: none;
		border-bottom: 2px solid transparent;
		background: none;
		color: var(--color-text-muted);
		cursor: pointer;
		font-size: var(--text-base);
	}

	.subnav__tab.active {
		color: var(--color-accent);
		border-bottom-color: var(--color-accent);
	}

	.photos-toolbar {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		padding: var(--space-3) 1rem 0;
	}

	.seg {
		display: flex;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		overflow: hidden;
	}

	.seg__btn {
		display: grid;
		place-items: center;
		padding: var(--space-2) var(--space-3);
		border: none;
		background: var(--color-bg-surface);
		color: var(--color-text-muted);
		cursor: pointer;
		text-transform: capitalize;
	}

	.seg__btn.active {
		background: var(--color-accent);
		color: var(--color-on-accent);
	}

	.batch-bar {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		margin: var(--space-3) 1rem 0;
		padding: var(--space-2) var(--space-3);
		background: var(--color-accent-tint, var(--color-bg-hover));
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
	}

	.batch-bar__actions {
		display: flex;
		gap: var(--space-2);
	}

	.photos-area {
		padding: 0 1rem;
	}

	/* Date header — fixed height (set inline) so the virtualizer's offset table
	   matches the rendered layout exactly. */
	.photos-group {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		margin: 0;
		font-size: 1rem;
		color: var(--color-text-heading);
	}

	.photos-group__count {
		color: var(--color-text-muted);
		font-size: var(--text-sm);
		font-weight: var(--weight-normal);
	}

	/* A horizontal strip of explicitly-sized tiles — one virtualized row, used by
	   both the square and justified layouts (the bottom gap is baked into the
	   row's declared height). */
	.photos-strip {
		display: flex;
	}

	.photo-tile {
		position: relative;
		overflow: hidden;
		border-radius: var(--radius-sm);
		background: var(--color-bg-muted);
	}

	.photo-tile.selected {
		outline: 3px solid var(--color-accent);
		outline-offset: -3px;
	}

	.photo-tile__open {
		position: relative;
		display: block;
		width: 100%;
		height: 100%;
		border: none;
		padding: 0;
		cursor: pointer;
		background: none;
	}

	/* Default placeholder shown until the thumbnail paints over it (or when the
	   thumbnail can't load). Both this and the <img> are absolutely positioned so
	   DOM order — placeholder first, image second — keeps the image on top. */
	.photo-tile__placeholder {
		position: absolute;
		inset: 0;
		display: grid;
		place-items: center;
		font-size: 2rem;
		color: var(--color-text-faint);
		pointer-events: none;
	}

	.photo-tile__open img {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}

	.photo-tile__video-badge {
		position: absolute;
		right: 6px;
		bottom: 6px;
		width: 26px;
		height: 26px;
		border-radius: 50%;
		background: var(--color-scrim-control);
		color: var(--color-on-accent);
		display: grid;
		place-items: center;
		font-size: 0.7rem;
		pointer-events: none;
	}

	.photo-tile__check {
		position: absolute;
		top: 6px;
		left: 6px;
		width: 24px;
		height: 24px;
		border-radius: 50%;
		border: 2px solid var(--color-on-accent);
		background: var(--color-scrim-control);
		color: transparent;
		display: grid;
		place-items: center;
		cursor: pointer;
		opacity: 0;
		transition: opacity 0.15s;
	}

	.photo-tile:hover .photo-tile__check,
	.photo-tile__check.on {
		opacity: 1;
	}

	.photo-tile__check.on {
		background: var(--color-accent);
		color: var(--color-on-accent);
		border-color: var(--color-accent);
	}

	.status {
		text-align: center;
		color: var(--color-text-muted);
		padding: 2rem 0;
	}

	.status--error {
		color: var(--color-danger-text);
	}

	.sentinel {
		height: 1px;
	}
</style>
