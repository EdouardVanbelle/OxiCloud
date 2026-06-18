<script lang="ts">
	import Button from '$lib/components/Button.svelte';
	import EmptyState from '$lib/components/EmptyState.svelte';
	import { useSelection } from '$lib/composables/useSelection.svelte';
	import { errorMessage, errorToast } from '$lib/utils/errors';
	import { onMount } from 'svelte';
	import {
		batchTrash,
		fetchFileMetadata,
		fetchPhotos,
		uploadThumbnail,
		type FileMetadata
	} from '$lib/api/endpoints/photos';
	import { addFavorite } from '$lib/api/endpoints/favorites';
	import { deleteFile, fileDownloadUrl, fileInlineUrl } from '$lib/api/endpoints/files';
	import type { FileItem } from '$lib/api/types';
	import Icon from '$lib/icons/Icon.svelte';
	import { confirmDialog } from '$lib/stores/dialogs.svelte';
	import { t } from '$lib/i18n/index.svelte';
	import { ui } from '$lib/stores/ui.svelte';

	let items = $state<FileItem[]>([]);
	let cursor = $state<string | null>(null);
	let exhausted = $state(false);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let sentinel = $state<HTMLElement | null>(null);

	type GroupMode = 'day' | 'month' | 'year';
	const GROUP_KEY = 'oxicloud-photos-group';
	let groupMode = $state<GroupMode>('month');
	const selected = useSelection();
	let lightbox = $state(-1); // index into `items`, -1 = closed

	/** Client-generated video frame thumbnails (file id → data/URL). */
	let videoThumbs = $state<Record<string, string>>({});

	function isVideo(p: FileItem): boolean {
		return (p.mime_type ?? '').startsWith('video/');
	}

	/** EXIF-aware timestamp (seconds → ms), matching the OLD grouping logic. */
	function ts(p: FileItem): number {
		const v = p.sort_date || p.created_at || 0;
		return v < 1e12 ? v * 1000 : v;
	}

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
		const out: Array<{ key: string; label: string; photos: FileItem[] }> = [];
		const index = new Map<string, number>();
		for (const p of items) {
			const d = new Date(ts(p));
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

	function iconUrl(id: string): string {
		return `/api/files/${id}/thumbnail/icon`;
	}
	function previewUrl(id: string): string {
		return `/api/files/${id}/thumbnail/preview`;
	}
	function largeUrl(id: string): string {
		return `/api/files/${id}/thumbnail/large`;
	}

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
			error = errorMessage(e);
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

	/** A plain tile click toggles selection once anything is selected, else opens the lightbox. */
	function onTileClick(p: FileItem) {
		if (selected.size > 0) selected.toggle(p.id);
		else openLightbox(p);
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

	// ── Client-side video thumbnail generation ──────────────────────────────
	// When the server has no thumbnail for a video tile the <img> errors; we
	// then extract a frame with the browser's native decoder and upload it.

	async function generateVideoThumb(file: FileItem) {
		if (videoThumbs[file.id]) return;
		try {
			const bitmap = await frameFromVideo(fileInlineUrl(file.id));
			const SIZES: Array<['icon' | 'preview' | 'large', number, number]> = [
				['icon', 150, 150],
				['preview', 400, 400],
				['large', 800, 800]
			];
			let previewData = '';
			for (const [size, w, h] of SIZES) {
				const blob = await bitmapToBlob(bitmap, w, h);
				if (size === 'preview') previewData = await blobToDataUrl(blob);
				await uploadThumbnail(file.id, size, blob).catch(() => {});
			}
			if (previewData) videoThumbs = { ...videoThumbs, [file.id]: previewData };
		} catch {
			// Keep the generic play badge on failure.
		}
	}

	function frameFromVideo(src: string): Promise<ImageBitmap> {
		return new Promise((resolve, reject) => {
			const video = document.createElement('video');
			video.src = src;
			video.muted = true;
			video.preload = 'metadata';
			video.onloadedmetadata = () => {
				video.currentTime = (video.duration || 3) / 3;
			};
			video.onseeked = async () => {
				try {
					const bitmap = await createImageBitmap(video);
					video.removeAttribute('src');
					video.load();
					resolve(bitmap);
				} catch (e) {
					reject(e instanceof Error ? e : new Error(String(e)));
				}
			};
			video.onerror = () => reject(new Error('video frame extraction failed'));
		});
	}

	async function bitmapToBlob(bitmap: ImageBitmap, tw: number, th: number): Promise<Blob> {
		const ratio = bitmap.width / bitmap.height;
		const target = tw / th;
		const w = ratio > target ? tw : Math.round(th * ratio);
		const h = ratio > target ? Math.round(tw / ratio) : th;
		const canvas = document.createElement('canvas');
		canvas.width = w;
		canvas.height = h;
		canvas.getContext('2d')?.drawImage(bitmap, 0, 0, w, h);
		return new Promise<Blob>((resolve, reject) => {
			canvas.toBlob(
				(b) => (b ? resolve(b) : reject(new Error('canvas toBlob failed'))),
				'image/jpeg',
				0.8
			);
		});
	}

	function blobToDataUrl(blob: Blob): Promise<string> {
		return new Promise((resolve, reject) => {
			const reader = new FileReader();
			reader.onload = () => resolve(String(reader.result));
			reader.onerror = () => reject(new Error('blob read failed'));
			reader.readAsDataURL(blob);
		});
	}

	// ── Lightbox ─────────────────────────────────────────────────────────────
	let lbShowingOriginal = $state(false);
	let lbFullResBusy = $state(false);
	let lbMeta = $state('');
	let lbFavorited = $state(false);
	/** Token guarding against stale async loads during rapid prev/next. */
	let lbGeneration = 0;

	const lbItem = $derived(lightbox >= 0 ? (items[lightbox] ?? null) : null);

	function baseMeta(p: FileItem): string {
		const dateStr = new Date(ts(p)).toLocaleDateString(undefined, {
			year: 'numeric',
			month: 'short',
			day: 'numeric',
			hour: '2-digit',
			minute: '2-digit'
		});
		return p.size_formatted ? `${dateStr} · ${p.size_formatted}` : dateStr;
	}

	function applyMetadata(p: FileItem, md: FileMetadata) {
		const parts = [baseMeta(p)];
		if (md.camera_make || md.camera_model) {
			parts.push([md.camera_make, md.camera_model].filter(Boolean).join(' '));
		}
		if (md.width && md.height) parts.push(`${md.width}×${md.height}`);
		lbMeta = parts.join(' · ');
	}

	function openLightbox(p: FileItem) {
		lightbox = items.findIndex((x) => x.id === p.id);
	}

	/** Reset per-item lightbox state and kick off metadata + neighbour preload. */
	function showLightboxItem(p: FileItem) {
		const generation = ++lbGeneration;
		lbShowingOriginal = p.mime_type === 'image/gif';
		lbFullResBusy = false;
		lbFavorited = false;
		lbMeta = baseMeta(p);
		preloadNeighbors();
		void fetchFileMetadata(p.id).then((md) => {
			if (md && generation === lbGeneration) applyMetadata(p, md);
		});
	}

	// Re-run per-item setup whenever the visible lightbox item changes.
	$effect(() => {
		if (lbItem) showLightboxItem(lbItem);
	});

	function preloadNeighbors() {
		for (const i of [lightbox - 1, lightbox + 1]) {
			const it = items[i];
			if (it && !isVideo(it)) {
				const pre = new Image();
				pre.src = largeUrl(it.id);
			}
		}
	}

	/** The image src to display: large thumbnail first, original on expand/GIF. */
	const lbImgSrc = $derived(
		lbItem ? (lbShowingOriginal ? fileInlineUrl(lbItem.id) : largeUrl(lbItem.id)) : ''
	);

	function onLbImgError() {
		if (!lbItem) return;
		// Thumbnail missing → fall back to the original; original failing is terminal.
		if (!lbShowingOriginal) {
			lbShowingOriginal = true;
		}
	}

	function onLbImgLoad() {
		lbFullResBusy = false;
	}

	function expandFullRes() {
		if (!lbItem || lbShowingOriginal) return;
		lbShowingOriginal = true;
		lbFullResBusy = true;
	}

	function lbDownload() {
		if (!lbItem) return;
		const a = document.createElement('a');
		a.href = fileDownloadUrl(lbItem.id);
		a.download = lbItem.name;
		document.body.appendChild(a);
		a.click();
		a.remove();
	}

	async function lbToggleFavorite() {
		if (!lbItem) return;
		try {
			await addFavorite('file', lbItem.id);
			lbFavorited = !lbFavorited;
		} catch (e) {
			errorToast(e);
		}
	}

	async function lbDelete() {
		if (!lbItem) return;
		const target = lbItem;
		const ok = await confirmDialog({
			title: t('photos.delete', 'Delete photo'),
			message: t('photos.confirm_delete_one', { name: target.name }, 'Delete {{name}}?'),
			confirmText: t('common.delete', 'Delete'),
			danger: true
		});
		if (!ok) return;
		try {
			await deleteFile(target.id);
			const at = items.findIndex((x) => x.id === target.id);
			items = items.filter((x) => x.id !== target.id);
			if (items.length === 0) {
				lightbox = -1;
			} else {
				lightbox = Math.min(at, items.length - 1);
			}
		} catch (e) {
			errorToast(e);
		}
	}

	function lbPrev() {
		if (lightbox > 0) lightbox -= 1;
	}
	function lbNext() {
		if (lightbox >= 0 && lightbox < items.length - 1) lightbox += 1;
	}
	function onKeydown(e: KeyboardEvent) {
		if (lightbox < 0) return;
		if (e.key === 'Escape') lightbox = -1;
		else if (e.key === 'ArrowLeft') lbPrev();
		else if (e.key === 'ArrowRight') lbNext();
	}

	onMount(() => {
		const saved = typeof localStorage !== 'undefined' ? localStorage.getItem(GROUP_KEY) : null;
		if (saved === 'day' || saved === 'month' || saved === 'year') groupMode = saved;
		void loadMore();
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
<svelte:window onkeydown={onKeydown} />

<div class="page-sticky-header photos-head">
	<h1 class="page-title">{t('nav.photos', 'Photos')}</h1>
	<div class="seg" role="group" aria-label={t('photos.group_by', 'Group by')}>
		{#each MODES as m (m)}
			<button class="seg__btn" class:active={groupMode === m} onclick={() => setGroupMode(m)}>
				{t(`photos.${m}`, m)}
			</button>
		{/each}
	</div>
</div>

{#if selected.size > 0}
	<div class="batch-bar">
		<span>{t('files.selected_count', { n: selected.size }, '{{n}} selected')}</span>
		<div class="batch-bar__actions">
			<Button onclick={downloadSelected}>{t('common.download', 'Download')}</Button>
			<Button onclick={() => selected.clear()}>{t('common.clear', 'Clear')}</Button>
			<Button variant="danger" onclick={trashSelected}>{t('common.delete', 'Delete')}</Button>
		</div>
	</div>
{/if}

{#if error}
	<p class="status status--error" role="alert">{error}</p>
{:else if items.length === 0 && exhausted}
	<EmptyState
		icon="images"
		title={t('photos.empty', 'No photos yet.')}
		hint={t('photos.empty_hint', 'Photos and videos you upload will appear here, grouped by date.')}
	/>
{:else}
	{#each groups as group (group.key)}
		<h2 class="photos-group">
			{group.label} <span class="photos-group__count">{group.photos.length}</span>
		</h2>
		<ul class="photos">
			{#each group.photos as photo (photo.id)}
				<li class="photos__cell" class:selected={selected.has(photo.id)}>
					<button class="photos__open" onclick={() => onTileClick(photo)}>
						{#if videoThumbs[photo.id]}
							<img src={videoThumbs[photo.id]} alt={photo.name} loading="lazy" decoding="async" />
						{:else}
							<img
								src={previewUrl(photo.id)}
								srcset={`${iconUrl(photo.id)} 150w, ${previewUrl(photo.id)} 400w, ${largeUrl(photo.id)} 800w`}
								sizes="(max-width: 768px) 33vw, 200px"
								alt={photo.name}
								loading="lazy"
								decoding="async"
								onerror={isVideo(photo) ? () => generateVideoThumb(photo) : undefined}
							/>
						{/if}
						{#if isVideo(photo)}
							<span class="photos__video-badge" aria-hidden="true"><Icon name="play" /></span>
						{/if}
					</button>
					<button
						class="photos__check"
						class:on={selected.has(photo.id)}
						aria-label={t('common.select', 'Select')}
						onclick={() => selected.toggle(photo.id)}
					>
						<Icon name="check" />
					</button>
				</li>
			{/each}
		</ul>
	{/each}
{/if}

<div bind:this={sentinel} class="sentinel" aria-hidden="true"></div>
{#if loading}<p class="status">{t('common.loading', 'Loading…')}</p>{/if}

{#if lbItem}
	<!-- svelte-ignore a11y_click_events_have_key_events -->
	<div
		class="lb"
		role="dialog"
		aria-modal="true"
		aria-label={lbItem.name}
		tabindex="-1"
		onclick={(e) => e.target === e.currentTarget && (lightbox = -1)}
	>
		<div class="lb__info">
			<div class="lb__filename">{lbItem.name}</div>
			<div class="lb__meta">{lbMeta}</div>
		</div>

		<button
			class="lb__close"
			aria-label={t('common.close', 'Close')}
			onclick={() => (lightbox = -1)}>×</button
		>

		<button
			class="lb__nav lb__nav--prev"
			aria-label={t('common.previous', 'Previous')}
			disabled={lightbox === 0}
			onclick={(e) => {
				e.stopPropagation();
				lbPrev();
			}}><Icon name="chevron-left" /></button
		>

		<div class="lb__content">
			{#if isVideo(lbItem)}
				{#key lbItem.id}
					<video class="lb__media" controls autoplay poster={largeUrl(lbItem.id)}>
						<source src={fileInlineUrl(lbItem.id)} type={lbItem.mime_type} />
					</video>
				{/key}
			{:else}
				<img
					class="lb__media"
					src={lbImgSrc}
					alt={lbItem.name}
					onload={onLbImgLoad}
					onerror={onLbImgError}
				/>
			{/if}
		</div>

		<button
			class="lb__nav lb__nav--next"
			aria-label={t('common.next', 'Next')}
			disabled={lightbox === items.length - 1}
			onclick={(e) => {
				e.stopPropagation();
				lbNext();
			}}><Icon name="chevron-right" /></button
		>

		<div class="lb__toolbar">
			{#if !isVideo(lbItem) && lbItem.mime_type !== 'image/gif' && !lbShowingOriginal}
				<button
					class="lb__tool"
					title={t('photos.full_resolution', 'Full resolution')}
					disabled={lbFullResBusy}
					onclick={expandFullRes}><Icon name={lbFullResBusy ? 'spinner' : 'expand'} /></button
				>
			{/if}
			<button class="lb__tool" title={t('common.download', 'Download')} onclick={lbDownload}
				><Icon name="download" /></button
			>
			<button
				class="lb__tool"
				class:active={lbFavorited}
				title={t('common.favorite', 'Favorite')}
				onclick={lbToggleFavorite}><Icon name={lbFavorited ? 'star' : 'star-outline'} /></button
			>
			<button class="lb__tool" title={t('common.delete', 'Delete')} onclick={lbDelete}
				><Icon name="trash" /></button
			>
		</div>

		<div class="lb__counter">{lightbox + 1} / {items.length}</div>
	</div>
{/if}

<style>
	.photos-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		padding: 1rem 1rem 0;
	}

	.page-title {
		margin: 0;
		font-size: 1.5rem;
		color: var(--color-text-heading);
	}

	.seg {
		display: flex;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		overflow: hidden;
	}

	.seg__btn {
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

	.photos-group {
		margin: var(--space-4) 0 var(--space-2);
		padding: 0 1rem;
		font-size: 1rem;
		color: var(--color-text-heading);
	}

	.photos-group__count {
		color: var(--color-text-muted);
		font-size: var(--text-sm);
		font-weight: var(--weight-normal);
	}

	.photos {
		list-style: none;
		margin: 0;
		padding: 0 1rem;
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(9rem, 1fr));
		gap: 0.25rem;
	}

	.photos__cell {
		position: relative;
		aspect-ratio: 1;
		overflow: hidden;
		border-radius: var(--radius-sm);
		background: var(--color-bg-muted);
	}

	.photos__cell.selected {
		outline: 3px solid var(--color-accent);
		outline-offset: -3px;
	}

	.photos__open {
		display: block;
		width: 100%;
		height: 100%;
		border: none;
		padding: 0;
		cursor: pointer;
		background: none;
	}

	.photos__open img {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}

	.photos__video-badge {
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

	.photos__check {
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

	.photos__cell:hover .photos__check,
	.photos__check.on {
		opacity: 1;
	}

	.photos__check.on {
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

	.lb {
		position: fixed;
		inset: 0;
		z-index: 1000;
		background: var(--color-lightbox-overlay);
		display: flex;
		align-items: center;
		justify-content: center;
	}

	.lb__content {
		max-width: 92vw;
		max-height: 88vh;
		display: flex;
		align-items: center;
		justify-content: center;
	}

	.lb__media {
		max-width: 92vw;
		max-height: 88vh;
		object-fit: contain;
	}

	.lb__info {
		position: absolute;
		top: 1rem;
		left: 1rem;
		color: var(--color-on-accent);
		max-width: 60vw;
	}

	.lb__filename {
		font-weight: var(--weight-medium);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.lb__meta {
		font-size: var(--text-sm);
		opacity: 0.8;
	}

	.lb__close {
		position: absolute;
		top: 1rem;
		right: 1rem;
		font-size: 2rem;
		line-height: 1;
		background: none;
		border: none;
		color: var(--color-on-accent);
		cursor: pointer;
	}

	.lb__nav {
		position: absolute;
		top: 50%;
		transform: translateY(-50%);
		font-size: 2rem;
		background: none;
		border: none;
		color: var(--color-on-accent);
		cursor: pointer;
		padding: 1rem;
	}

	.lb__nav:disabled {
		opacity: 0.3;
		cursor: default;
	}

	.lb__nav--prev {
		left: 0.5rem;
	}

	.lb__nav--next {
		right: 0.5rem;
	}

	.lb__toolbar {
		position: absolute;
		bottom: 1rem;
		left: 50%;
		transform: translateX(-50%);
		display: flex;
		gap: var(--space-2);
	}

	.lb__tool {
		width: 40px;
		height: 40px;
		border-radius: 50%;
		border: none;
		background: var(--color-scrim-control);
		color: var(--color-on-accent);
		cursor: pointer;
		display: grid;
		place-items: center;
	}

	.lb__tool:disabled {
		opacity: 0.5;
		cursor: default;
	}

	.lb__tool.active {
		color: var(--color-accent);
	}

	.lb__counter {
		position: absolute;
		bottom: 1rem;
		right: 1rem;
		color: var(--color-on-accent);
		font-size: var(--text-sm);
		opacity: 0.8;
	}
</style>
