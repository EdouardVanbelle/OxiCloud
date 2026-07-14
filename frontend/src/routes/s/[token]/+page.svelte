<script lang="ts">
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import Icon from '$lib/icons/Icon.svelte';
	import {
		getShareContents,
		getShareMeta,
		shareDownloadUrl,
		shareFileUrl,
		shareZipUrl,
		verifySharePassword,
		type ShareListing,
		type ShareMeta
	} from '$lib/api/endpoints/share';
	import { preferences } from '$lib/stores/preferences.svelte';
	import { session } from '$lib/stores/session.svelte';
	import { filterDotfiles } from '$lib/utils/dotfileFilter';
	import { t } from '$lib/i18n/index.svelte';

	type State = 'loading' | 'password' | 'expired' | 'invalid' | 'file' | 'folder';
	type Crumb = { id?: string; name: string };
	type ViewMode = 'grid' | 'list';

	const VIEW_KEY = 'oxi-share-view';
	const token = $derived(page.params.token ?? '');

	let view = $state<State>('loading');
	let meta = $state<ShareMeta | null>(null);
	let listing = $state<ShareListing | null>(null);
	let folderId = $state<string | undefined>(undefined);
	let crumbs = $state<Crumb[]>([]);
	let pwInput = $state('');
	let pwError = $state('');
	let busy = $state(false);
	let viewMode = $state<ViewMode>('grid');

	// Lightbox over the media files in the current folder
	let lightbox = $state(-1);

	function mediaKind(mime: string | undefined): 'image' | 'video' | null {
		const m = (mime ?? '').toLowerCase();
		if (m.startsWith('image/')) return 'image';
		if (m.startsWith('video/')) return 'video';
		return null;
	}

	// Dotfile hide only applies to LOGGED-IN viewers of a public
	// share. Anonymous viewers see exactly what the sharer put in
	// the link — hiding items behind a UI toggle they don't
	// control would be surprising ("the owner said this was in
	// there but I don't see it"). Logged-in viewers get their own
	// preference respected, matching every other list surface in
	// the app.
	const applyDotfileFilter = $derived(session.isAuthenticated && preferences.hideDotfiles);
	const visibleFolders = $derived(filterDotfiles(listing?.folders ?? [], applyDotfileFilter));
	const visibleFiles = $derived(filterDotfiles(listing?.files ?? [], applyDotfileFilter));

	const mediaFiles = $derived(visibleFiles.filter((f) => mediaKind(f.mime_type) !== null));

	function setViewMode(mode: ViewMode) {
		viewMode = mode;
		try {
			localStorage.setItem(VIEW_KEY, mode);
		} catch {
			/* storage unavailable — keep in-memory only */
		}
	}

	async function loadMeta() {
		view = 'loading';
		// Guard a missing/blank token before hitting the API.
		if (!token) {
			view = 'invalid';
			return;
		}
		try {
			const r = await getShareMeta(token);
			if (r.status === 'password') {
				view = 'password';
			} else if (r.status === 'expired') {
				view = 'expired';
			} else if (r.status === 'invalid') {
				view = 'invalid';
			} else {
				meta = r.data;
				if (r.data.item_type === 'folder') {
					crumbs = [{ name: r.data.item_name }];
					// Deep-link support: honour an initial #folder=<id> hash.
					await openFolder(hashFolderId(), undefined, false);
				} else view = 'file';
			}
		} catch {
			view = 'expired';
		}
	}

	/** Parse the `#folder=<id>` fragment from the URL, if present. */
	function hashFolderId(): string | undefined {
		if (typeof location === 'undefined') return undefined;
		const m = location.hash.match(/[#&]folder=([A-Za-z0-9-]{1,64})/);
		return m ? m[1] : undefined;
	}

	/**
	 * Load a folder's contents. When `crumb` is given, push it onto the trail.
	 * `pushHistory` controls whether we sync the URL hash + push a history entry
	 * (true for user navigation, false when restoring from popstate / deep link).
	 */
	async function openFolder(id: string | undefined, crumb?: Crumb, pushHistory = false) {
		const r = await getShareContents(token, id);
		if (r.status === 'password') {
			view = 'password';
			return;
		}
		if (r.status === 'expired') {
			view = 'expired';
			return;
		}
		listing = r.data;
		folderId = id;
		if (crumb) crumbs = [...crumbs, crumb];
		lightbox = -1;
		view = 'folder';
		if (pushHistory && typeof history !== 'undefined') {
			const hash = id ? `#folder=${encodeURIComponent(id)}` : '';
			history.pushState({ folderId: id }, '', location.pathname + location.search + hash);
		}
	}

	/** Navigate to a breadcrumb at depth `index` (0 = share root). */
	async function gotoCrumb(index: number) {
		const target = crumbs[index];
		crumbs = crumbs.slice(0, index + 1);
		await openFolder(target.id, undefined, true);
	}

	/** Browser back/forward — re-resolve the folder from the popped state/hash. */
	async function onPopState() {
		if (view !== 'folder') return;
		await openFolder(hashFolderId(), undefined, false);
	}

	/** Append a cache-busting query param to retry a failed media load once. */
	function retrySrc(original: string): string {
		const sep = original.indexOf('?') === -1 ? '?' : '&';
		return `${original}${sep}_r=${Date.now()}`;
	}

	/**
	 * Lazy video poster: defer loading until near the viewport, then seek a few
	 * frames in to render a thumbnail. Retries once with cache-busting on error.
	 * Ported from publicShare.js wireLazyVideos().
	 */
	function lazyVideo(node: HTMLVideoElement, src: string) {
		let retried = false;
		const start = () => {
			node.addEventListener(
				'loadedmetadata',
				() => {
					const at = Math.min(0.1, (node.duration || 1) * 0.1);
					try {
						node.currentTime = at;
					} catch {
						/* seeking unsupported */
					}
				},
				{ once: true }
			);
			node.addEventListener(
				'error',
				() => {
					if (retried) return;
					retried = true;
					setTimeout(() => (node.src = retrySrc(src)), 250);
				},
				{ once: true }
			);
			node.src = src;
		};
		let obs: IntersectionObserver | null = null;
		if (typeof IntersectionObserver !== 'undefined') {
			obs = new IntersectionObserver(
				(entries) => {
					for (const e of entries) {
						if (e.isIntersecting) {
							start();
							obs?.unobserve(node);
						}
					}
				},
				{ rootMargin: '300px' }
			);
			obs.observe(node);
		} else {
			start();
		}
		return { destroy: () => obs?.disconnect() };
	}

	/** Retry a failed image load once with cache-busting. Ported from wireImageRetry(). */
	function imageRetry(node: HTMLImageElement) {
		let retried = false;
		const onError = () => {
			if (retried) return;
			retried = true;
			const original = node.src;
			setTimeout(() => (node.src = retrySrc(original)), 250);
		};
		node.addEventListener('error', onError);
		return { destroy: () => node.removeEventListener('error', onError) };
	}

	function lbPrev() {
		if (lightbox > 0) lightbox -= 1;
	}
	function lbNext() {
		if (lightbox >= 0 && lightbox < mediaFiles.length - 1) lightbox += 1;
	}
	function onKeydown(e: KeyboardEvent) {
		if (lightbox < 0) return;
		if (e.key === 'Escape') lightbox = -1;
		else if (e.key === 'ArrowLeft') lbPrev();
		else if (e.key === 'ArrowRight') lbNext();
	}

	async function submitPassword(e: SubmitEvent) {
		e.preventDefault();
		if (!pwInput) return;
		busy = true;
		pwError = '';
		try {
			const ok = await verifySharePassword(token, pwInput);
			if (!ok) {
				pwError = t('share.bad_password', 'Incorrect password. Please try again.');
				return;
			}
			await loadMeta();
		} catch {
			pwError = t('share.error', 'Something went wrong. Please try again.');
		} finally {
			busy = false;
		}
	}

	onMount(() => {
		try {
			const saved = localStorage.getItem(VIEW_KEY);
			if (saved === 'list' || saved === 'grid') viewMode = saved;
		} catch {
			/* ignore */
		}
		void loadMeta();
	});
</script>

<svelte:head><title>{meta?.item_name ?? t('share.title', 'Shared')} · OxiCloud</title></svelte:head>
<svelte:window onkeydown={onKeydown} onpopstate={onPopState} />

<main class="share">
	{#if view === 'loading'}
		<p class="share__status">{t('common.loading', 'Loading…')}</p>
	{:else if view === 'invalid'}
		<div class="share__center">
			<Icon name="ban" class="share__big-icon" />
			<p>{t('share.invalid', 'This share link is invalid.')}</p>
		</div>
	{:else if view === 'expired'}
		<div class="share__center">
			<Icon name="ban" class="share__big-icon" />
			<p>{t('share.expired', 'This share link is no longer available.')}</p>
		</div>
	{:else if view === 'password'}
		<form class="share__pw" data-testid="public-share-password-form" onsubmit={submitPassword}>
			<h1>{t('share.password_title', 'Password required')}</h1>
			<input
				type="password"
				data-testid="public-share-password-input"
				bind:value={pwInput}
				placeholder={t('share.password', 'Password')}
				disabled={busy}
				autocomplete="off"
			/>
			{#if pwError}<p class="share__error" role="alert">{pwError}</p>{/if}
			<button type="submit" data-testid="public-share-unlock-btn" disabled={busy}
				>{t('share.unlock', 'Unlock')}</button
			>
		</form>
	{:else if view === 'file'}
		<div class="share__center">
			<Icon name="file" class="share__big-icon" />
			<h1>{meta?.item_name}</h1>
			<a
				class="share__btn"
				data-testid="public-share-download-btn"
				href={shareDownloadUrl(token)}
				download
				rel="external"
			>
				{t('share.download', 'Download')}
			</a>
		</div>
	{:else if view === 'folder' && listing}
		<header class="share__header">
			<nav class="breadcrumb" aria-label={t('files.breadcrumb', 'Breadcrumb')}>
				{#each crumbs as c, i (i)}
					{#if i > 0}<Icon name="chevron-right" class="breadcrumb__sep" />{/if}
					{#if i === crumbs.length - 1}
						<span class="breadcrumb__current">{c.name}</span>
					{:else}
						<button
							class="breadcrumb__link"
							data-testid={`public-share-breadcrumb-${i}`}
							onclick={() => gotoCrumb(i)}>{c.name}</button
						>
					{/if}
				{/each}
			</nav>
			<div class="share__header-actions">
				<div class="view-toggle" role="group" aria-label={t('files.view', 'View')}>
					<button
						type="button"
						aria-pressed={viewMode === 'grid'}
						class:active={viewMode === 'grid'}
						data-testid="public-share-view-grid-btn"
						title={t('files.grid', 'Grid')}
						onclick={() => setViewMode('grid')}><Icon name="th" /></button
					>
					<button
						type="button"
						aria-pressed={viewMode === 'list'}
						class:active={viewMode === 'list'}
						data-testid="public-share-view-list-btn"
						title={t('files.list', 'List')}
						onclick={() => setViewMode('list')}><Icon name="bars" /></button
					>
				</div>
				<a
					class="share__btn"
					data-testid="public-share-download-zip-btn"
					href={shareZipUrl(token, folderId)}
					download
					rel="external"
				>
					<Icon name="file-archive" />
					{t('share.download_zip', 'Download ZIP')}
				</a>
			</div>
		</header>

		{#if visibleFolders.length === 0 && visibleFiles.length === 0}
			<p class="share__status">{t('share.empty_folder', 'This folder is empty.')}</p>
		{/if}

		{#if visibleFolders.length > 0}
			<h2 class="share__section">{t('share.folders', 'Folders')}</h2>
			<ul class="share__grid" class:share__grid--list={viewMode === 'list'}>
				{#each visibleFolders as f (f.id)}
					<li>
						<button
							class="card"
							data-testid={f.name}
							onclick={() => openFolder(f.id, { id: f.id, name: f.name }, true)}
						>
							<span class="card__thumb"><Icon name="folder" class="card__icon" /></span>
							<span class="card__name">{f.name}</span>
						</button>
					</li>
				{/each}
			</ul>
		{/if}

		{#if visibleFiles.length > 0}
			<h2 class="share__section">{t('share.files', 'Files')}</h2>
			<ul class="share__grid" class:share__grid--list={viewMode === 'list'}>
				{#each visibleFiles as f (f.id)}
					{@const kind = mediaKind(f.mime_type)}
					{#if kind}
						<li>
							<button
								class="card"
								data-testid={f.name}
								onclick={() => (lightbox = mediaFiles.findIndex((m) => m.id === f.id))}
							>
								<span class="card__thumb">
									{#if kind === 'image'}
										<img
											src={shareFileUrl(token, f.id)}
											alt={f.name}
											loading="lazy"
											decoding="async"
											use:imageRetry
										/>
									{:else}
										<video
											use:lazyVideo={shareFileUrl(token, f.id)}
											preload="metadata"
											muted
											playsinline
										></video>
										<span class="card__play"><Icon name="play" /></span>
									{/if}
								</span>
								<span class="card__name">{f.name}</span>
							</button>
						</li>
					{:else}
						<li>
							<a
								class="card"
								data-testid={f.name}
								href={shareFileUrl(token, f.id)}
								target="_blank"
								rel="external noreferrer"
							>
								<span class="card__thumb"><Icon name="file" class="card__icon" /></span>
								<span class="card__name">{f.name}</span>
							</a>
						</li>
					{/if}
				{/each}
			</ul>
		{/if}
	{/if}
</main>

{#if lightbox >= 0 && mediaFiles[lightbox]}
	{@const m = mediaFiles[lightbox]}
	<!-- svelte-ignore a11y_click_events_have_key_events -->
	<div
		class="lb"
		role="dialog"
		data-testid="public-share-lightbox-dialog"
		aria-modal="true"
		aria-label={m.name}
		tabindex="-1"
		onclick={(e) => e.target === e.currentTarget && (lightbox = -1)}
	>
		<button
			class="lb__close"
			data-testid="public-share-lightbox-close-btn"
			aria-label={t('common.close', 'Close')}
			onclick={() => (lightbox = -1)}>×</button
		>
		<button
			class="lb__nav lb__nav--prev"
			data-testid="public-share-lightbox-prev-btn"
			aria-label={t('common.previous', 'Previous')}
			disabled={lightbox === 0}
			onclick={(e) => {
				e.stopPropagation();
				lbPrev();
			}}><Icon name="chevron-left" /></button
		>
		{#if mediaKind(m.mime_type) === 'image'}
			<img class="lb__media" src={shareFileUrl(token, m.id)} alt={m.name} />
		{:else}
			<!-- svelte-ignore a11y_media_has_caption -->
			<video class="lb__media" src={shareFileUrl(token, m.id)} controls autoplay></video>
		{/if}
		<button
			class="lb__nav lb__nav--next"
			data-testid="public-share-lightbox-next-btn"
			aria-label={t('common.next', 'Next')}
			disabled={lightbox === mediaFiles.length - 1}
			onclick={(e) => {
				e.stopPropagation();
				lbNext();
			}}><Icon name="chevron-right" /></button
		>
	</div>
{/if}

<style>
	.share {
		max-width: 60rem;
		margin: 0 auto;
		padding: 2rem 1rem;
	}

	.share__center {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 1rem;
		padding: 4rem 0;
		text-align: center;
	}

	:global(.share__big-icon) {
		font-size: 3rem;
		color: var(--color-text-muted);
	}

	.share__status {
		text-align: center;
		color: var(--color-text-muted);
		padding: 3rem 0;
	}

	.share__pw {
		max-width: 22rem;
		margin: 4rem auto;
		display: flex;
		flex-direction: column;
		gap: 0.75rem;
	}

	.share__pw input {
		padding: 0.625rem 0.75rem;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		background: var(--color-bg-input);
		color: var(--color-text);
	}

	.share__error {
		color: var(--color-danger-text);
		font-size: 0.875rem;
		margin: 0;
	}

	.share__header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 1rem;
		flex-wrap: wrap;
		margin-bottom: 1rem;
	}

	.share__header-actions {
		display: flex;
		align-items: center;
		gap: 0.75rem;
	}

	.share__section {
		font-size: 1rem;
		color: var(--color-text-muted);
		margin: 1.5rem 0 0.5rem;
	}

	.share__grid {
		list-style: none;
		margin: 0;
		padding: 0;
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(8rem, 1fr));
		gap: 0.75rem;
	}

	.card {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 0.5rem;
		width: 100%;
		padding: 0.5rem;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		background: var(--color-bg-surface);
		color: var(--color-text);
		cursor: pointer;
		text-decoration: none;
	}

	.card:hover {
		background: var(--color-bg-hover);
	}

	.card__thumb {
		position: relative;
		display: grid;
		place-items: center;
		width: 100%;
		aspect-ratio: 1;
		overflow: hidden;
		border-radius: var(--radius-sm);
		background: var(--color-bg-muted);
	}

	.card__thumb img,
	.card__thumb video {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}

	.card__play {
		position: absolute;
		inset: 0;
		display: grid;
		place-items: center;
		color: var(--color-on-accent);
		font-size: 1.5rem;
		background: var(--color-scrim-control, transparent);
		opacity: 0.85;
	}

	:global(.card__icon) {
		font-size: 2rem;
		color: var(--color-text-muted);
	}

	.card__name {
		font-size: 0.8125rem;
		text-align: center;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		max-width: 100%;
	}

	/* List mode: cards become single-row rows with a small leading thumb. */
	.share__grid--list {
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
	}

	.share__grid--list .card {
		flex-direction: row;
		align-items: center;
		gap: 0.75rem;
		padding: 0.4rem 0.6rem;
	}

	.share__grid--list .card__thumb {
		width: 2.5rem;
		height: 2.5rem;
		aspect-ratio: auto;
		flex-shrink: 0;
	}

	.share__grid--list .card__name {
		text-align: left;
		flex: 1;
	}

	.share__btn {
		display: inline-flex;
		align-items: center;
		gap: 0.4rem;
		padding: 0.5rem 1rem;
		border: none;
		border-radius: var(--radius-md);
		background: var(--color-primary);
		color: var(--color-text-light);
		text-decoration: none;
		cursor: pointer;
	}

	.breadcrumb {
		display: flex;
		align-items: center;
		gap: 0.35rem;
		flex-wrap: wrap;
	}

	.breadcrumb__link {
		background: none;
		border: none;
		color: var(--color-primary);
		cursor: pointer;
		font-size: 1rem;
		padding: 0;
	}

	.breadcrumb__current {
		font-weight: var(--weight-semibold, 600);
	}

	:global(.breadcrumb__sep) {
		color: var(--color-text-muted);
		font-size: 0.75rem;
	}

	.view-toggle {
		display: flex;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		overflow: hidden;
	}

	.view-toggle button {
		padding: 0.4rem 0.6rem;
		border: none;
		background: var(--color-bg-surface);
		color: var(--color-text-muted);
		cursor: pointer;
	}

	.view-toggle button.active {
		background: var(--color-accent);
		color: var(--color-on-accent);
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

	.lb__media {
		max-width: 92vw;
		max-height: 88vh;
		object-fit: contain;
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
</style>
