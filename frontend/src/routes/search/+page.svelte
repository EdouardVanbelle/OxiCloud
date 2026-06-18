<script lang="ts">
	import EmptyState from '$lib/components/EmptyState.svelte';
	import { errorMessage } from '$lib/utils/errors';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { searchFiles } from '$lib/api/endpoints/search';
	import { fileInlineUrl } from '$lib/api/endpoints/files';
	import type { FileItem, FolderItem, SearchResults, SortBy } from '$lib/api/types';
	import Icon from '$lib/icons/Icon.svelte';
	import { t } from '$lib/i18n/index.svelte';
	import { files as filesStore } from '$lib/stores/files.svelte';
	import { formatBytes } from '$lib/utils/format';
	import { formatDate, iconNameFromClass } from '$lib/utils/display';

	const query = $derived(page.url.searchParams.get('q') ?? '');

	let results = $state<SearchResults | null>(null);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let sortBy = $state<SortBy>('relevance');
	// Scope: search everywhere, or within the folder last open in the files view.
	// Default to the current folder when one is set (and we're not in the trash
	// section), mirroring the legacy searchView behaviour.
	let scope = $state<'all' | 'folder'>(
		filesStore.currentFolder && filesStore.section !== 'trash' ? 'folder' : 'all'
	);

	// Filters
	type TypeKey = 'all' | 'image' | 'video' | 'document' | 'audio' | 'archive';
	type SizeKey = 'all' | 'small' | 'medium' | 'large';
	type DateKey = 'all' | 'day' | 'week' | 'month' | 'year';
	let typeFilter = $state<TypeKey>('all');
	let sizeFilter = $state<SizeKey>('all');
	let dateFilter = $state<DateKey>('all');

	const TYPE_EXT: Record<Exclude<TypeKey, 'all'>, string[]> = {
		image: ['jpg', 'jpeg', 'png', 'gif', 'webp', 'svg', 'bmp', 'heic', 'avif', 'tiff'],
		video: ['mp4', 'mov', 'mkv', 'avi', 'webm', 'm4v', 'wmv', 'flv'],
		document: [
			'pdf',
			'doc',
			'docx',
			'xls',
			'xlsx',
			'ppt',
			'pptx',
			'txt',
			'md',
			'odt',
			'rtf',
			'csv'
		],
		audio: ['mp3', 'wav', 'flac', 'aac', 'ogg', 'm4a', 'opus'],
		archive: ['zip', 'rar', '7z', 'tar', 'gz', 'bz2', 'xz']
	};
	const TYPES: { v: TypeKey; l: string }[] = [
		{ v: 'all', l: t('search.type.all', 'All types') },
		{ v: 'image', l: t('search.type.image', 'Images') },
		{ v: 'video', l: t('search.type.video', 'Videos') },
		{ v: 'document', l: t('search.type.document', 'Documents') },
		{ v: 'audio', l: t('search.type.audio', 'Audio') },
		{ v: 'archive', l: t('search.type.archive', 'Archives') }
	];
	const SIZES: { v: SizeKey; l: string }[] = [
		{ v: 'all', l: t('search.size.all', 'Any size') },
		{ v: 'small', l: t('search.size.small', '< 1 MB') },
		{ v: 'medium', l: t('search.size.medium', '1–100 MB') },
		{ v: 'large', l: t('search.size.large', '> 100 MB') }
	];
	const DATES: { v: DateKey; l: string }[] = [
		{ v: 'all', l: t('search.date.all', 'Any time') },
		{ v: 'day', l: t('search.date.day', 'Past 24 hours') },
		{ v: 'week', l: t('search.date.week', 'Past week') },
		{ v: 'month', l: t('search.date.month', 'Past month') },
		{ v: 'year', l: t('search.date.year', 'Past year') }
	];

	const MB = 1024 * 1024;
	function sizeBounds(k: SizeKey): { minSize?: number; maxSize?: number } {
		switch (k) {
			case 'small':
				return { maxSize: MB };
			case 'medium':
				return { minSize: MB, maxSize: 100 * MB };
			case 'large':
				return { minSize: 100 * MB };
			default:
				return {};
		}
	}
	function dateBound(k: DateKey): number | undefined {
		const day = 86400;
		const now = Math.floor(Date.now() / 1000);
		switch (k) {
			case 'day':
				return now - day;
			case 'week':
				return now - 7 * day;
			case 'month':
				return now - 30 * day;
			case 'year':
				return now - 365 * day;
			default:
				return undefined;
		}
	}

	const hasFilters = $derived(typeFilter !== 'all' || sizeFilter !== 'all' || dateFilter !== 'all');
	function clearFilters() {
		typeFilter = 'all';
		sizeFilter = 'all';
		dateFilter = 'all';
	}

	const SORTS: { v: SortBy; l: string }[] = [
		{ v: 'relevance', l: t('search.sort.relevance', 'Relevance') },
		{ v: 'name', l: t('search.sort.name_asc', 'Name A-Z') },
		{ v: 'name_desc', l: t('search.sort.name_desc', 'Name Z-A') },
		{ v: 'date_desc', l: t('search.sort.newest', 'Newest') },
		{ v: 'date', l: t('search.sort.oldest', 'Oldest') },
		{ v: 'size_desc', l: t('search.sort.largest', 'Largest') },
		{ v: 'size', l: t('search.sort.smallest', 'Smallest') }
	];

	async function run(q: string) {
		if (!q) {
			results = null;
			return;
		}
		loading = true;
		error = null;
		try {
			// Trash section searches are always global — there is no folder to scope to.
			const folderId =
				scope === 'folder' && filesStore.section !== 'trash'
					? (filesStore.currentFolder ?? undefined)
					: undefined;
			results = await searchFiles(q, {
				recursive: true,
				sortBy,
				folderId,
				fileTypes: typeFilter === 'all' ? undefined : TYPE_EXT[typeFilter],
				...sizeBounds(sizeFilter),
				modifiedAfter: dateBound(dateFilter)
			});
		} catch (e) {
			error = errorMessage(e);
		} finally {
			loading = false;
		}
	}

	function openFolder(folder: FolderItem) {
		goto(`/files/${folder.id}`);
	}

	function openFile(file: FileItem) {
		window.open(fileInlineUrl(file.id), '_blank', 'noopener');
	}

	const isEmpty = $derived(!!results && results.files.length === 0 && results.folders.length === 0);

	$effect(() => {
		// re-run when query, sort, scope, or any filter changes
		void sortBy;
		void scope;
		void typeFilter;
		void sizeFilter;
		void dateFilter;
		void run(query);
	});
</script>

<svelte:head><title>{t('search.title', 'Search')} · OxiCloud</title></svelte:head>

<div class="page-sticky-header search-head">
	<h1 class="page-title">
		{#if query}{t('search.results_for', { q: query }, 'Results for “{{q}}”')}{:else}{t(
				'search.title',
				'Search'
			)}{/if}
		{#if results?.query_time_ms != null}
			<span class="search-time">({results.query_time_ms} ms)</span>
		{/if}
	</h1>
	{#if query}
		<div class="search-controls">
			{#if filesStore.currentFolder}
				<div class="seg" role="group" aria-label={t('search.scope', 'Scope')}>
					<button class="seg__btn" class:active={scope === 'all'} onclick={() => (scope = 'all')}>
						{t('search.everywhere', 'Everywhere')}
					</button>
					<button
						class="seg__btn"
						class:active={scope === 'folder'}
						onclick={() => (scope = 'folder')}
					>
						{t('search.this_folder', 'This folder')}
					</button>
				</div>
			{/if}
			<select
				class="sort-select"
				bind:value={typeFilter}
				aria-label={t('search.type_label', 'Type')}
			>
				{#each TYPES as o (o.v)}<option value={o.v}>{o.l}</option>{/each}
			</select>
			<select
				class="sort-select"
				bind:value={sizeFilter}
				aria-label={t('search.size_label', 'Size')}
			>
				{#each SIZES as o (o.v)}<option value={o.v}>{o.l}</option>{/each}
			</select>
			<select
				class="sort-select"
				bind:value={dateFilter}
				aria-label={t('search.date_label', 'Date')}
			>
				{#each DATES as o (o.v)}<option value={o.v}>{o.l}</option>{/each}
			</select>
			<select class="sort-select" bind:value={sortBy} aria-label={t('search.sort_by', 'Sort by')}>
				{#each SORTS as s (s.v)}<option value={s.v}>{s.l}</option>{/each}
			</select>
			{#if hasFilters}
				<button class="clear-filters" onclick={clearFilters}>
					<Icon name="times" />
					{t('search.clear_filters', 'Clear filters')}
				</button>
			{/if}
		</div>
	{/if}
</div>

{#if loading}
	<div class="search-loading">
		<Icon name="spinner" class="search-loading__spinner" />
		<h2 class="search-loading__text">
			{t('search.searching_for', { q: query }, 'Searching for “{{q}}”…')}
		</h2>
	</div>
{:else if error}
	<EmptyState title={error} error />
{:else if !query}
	<EmptyState title={t('search.prompt', 'Type a query in the search bar above.')} />
{:else if isEmpty}
	<EmptyState icon="search" title={t('search.no_results', 'No results found for this search')} />
{:else if results}
	<div class="files-container">
		<div class="files-list-view" style="--files-list-columns: minmax(200px, 2fr) 1fr 110px 140px">
			<div class="list-header">
				<div>{t('files.col_name', 'Name')}</div>
				<div>{t('files.col_path', 'Path')}</div>
				<div>{t('files.col_size', 'Size')}</div>
				<div>{t('files.col_modified', 'Modified')}</div>
			</div>

			{#each results.folders as folder (folder.id)}
				<div
					class="file-item"
					role="button"
					tabindex="0"
					onclick={() => openFolder(folder)}
					onkeydown={(e) => e.key === 'Enter' && openFolder(folder)}
				>
					<div class="name-cell">
						<span class="file-icon"><Icon name="folder" /></span>
						<span>{folder.name}</span>
					</div>
					<div class="path-cell">{folder.path}</div>
					<div class="size-cell">—</div>
					<div class="date-cell">{formatDate(folder.modified_at)}</div>
				</div>
			{/each}

			{#each results.files as file (file.id)}
				<div
					class="file-item"
					role="button"
					tabindex="0"
					onclick={() => openFile(file)}
					onkeydown={(e) => e.key === 'Enter' && openFile(file)}
				>
					<div class="name-cell">
						<span class="file-icon"><Icon name={iconNameFromClass(file.icon_class)} /></span>
						<span>{file.name}</span>
					</div>
					<div class="path-cell">{file.path}</div>
					<div class="size-cell">{file.size != null ? formatBytes(file.size) : ''}</div>
					<div class="date-cell">{formatDate(file.modified_at)}</div>
				</div>
			{/each}
		</div>
	</div>
{/if}

<style>
	.search-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		flex-wrap: wrap;
	}

	.search-controls {
		display: flex;
		align-items: center;
		gap: var(--space-2);
	}

	.sort-select {
		padding: var(--space-2) var(--space-2-5);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		background: var(--color-bg-input);
		color: var(--color-text);
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
	}

	.seg__btn.active {
		background: var(--color-accent);
		color: var(--color-on-accent);
	}

	.search-time {
		font-size: var(--text-sm);
		font-weight: var(--weight-normal);
		color: var(--color-text-muted);
	}

	.clear-filters {
		display: inline-flex;
		align-items: center;
		gap: 0.35rem;
		padding: var(--space-2) var(--space-3);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		background: var(--color-bg-surface);
		color: var(--color-text-muted);
		cursor: pointer;
	}

	.clear-filters:hover {
		background: var(--color-bg-hover);
	}

	.search-loading {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		padding: var(--space-4) 0;
		color: var(--color-text-muted);
	}

	.search-loading :global(.search-loading__spinner) {
		font-size: var(--text-xl);
		color: var(--color-accent);
		animation: spin var(--spin-duration) linear infinite;
	}

	.search-loading__text {
		margin: 0;
		font-size: var(--text-lg);
		font-weight: var(--weight-medium);
		color: var(--color-text);
	}
</style>
