<script lang="ts">
	import { errorToast } from '$lib/utils/errors';
	import { listFolder, moveFolder } from '$lib/api/endpoints/folders';
	import { moveFile } from '$lib/api/endpoints/files';
	import { copyFiles, copyFolders } from '$lib/api/endpoints/batch';
	import type { FolderItem } from '$lib/api/types';
	import Icon from '$lib/icons/Icon.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import { t } from '$lib/i18n/index.svelte';
	import { session } from '$lib/stores/session.svelte';
	import { ui } from '$lib/stores/ui.svelte';

	interface Target {
		id: string;
		name: string;
		kind: 'file' | 'folder';
	}

	interface Props {
		open: boolean;
		item: Target | null;
		/** Optional multi-item batch; takes precedence over `item`. */
		items?: Target[] | null;
		/** 'move' (default) relocates; 'copy' duplicates into the picked folder. */
		mode?: 'move' | 'copy';
		onmoved?: () => void;
	}

	let { open = $bindable(false), item, items = null, mode = 'move', onmoved }: Props = $props();

	const targets = $derived(items && items.length ? items : item ? [item] : []);
	const targetIds = $derived(new Set(targets.map((x) => x.id)));

	let crumbs = $state<Array<{ id: string; name: string }>>([]);
	let folders = $state<FolderItem[]>([]);
	let currentId = $state<string | null>(null);
	let loading = $state(false);
	let working = $state(false);

	async function loadInto(id: string) {
		loading = true;
		try {
			currentId = id;
			folders = (await listFolder(id)).folders;
		} catch (e) {
			errorToast(e);
		} finally {
			loading = false;
		}
	}

	async function init() {
		const home = await session.loadHomeFolder();
		if (!home) return;
		crumbs = [{ id: home, name: session.homeFolderName ?? t('nav.files', 'Files') }];
		await loadInto(home);
	}

	function enter(f: FolderItem) {
		crumbs = [...crumbs, { id: f.id, name: f.name }];
		void loadInto(f.id);
	}

	function gotoCrumb(index: number) {
		crumbs = crumbs.slice(0, index + 1);
		void loadInto(crumbs[index].id);
	}

	/** Jump to the home (root) folder — the first crumb. */
	function goHome() {
		if (crumbs.length) gotoCrumb(0);
	}

	/** Step up one level to the parent folder (no-op at home). */
	function goParent() {
		if (crumbs.length > 1) gotoCrumb(crumbs.length - 2);
	}

	const atHome = $derived(crumbs.length <= 1);

	async function confirmMove() {
		if (!targets.length || !currentId) return;
		working = true;
		try {
			if (mode === 'copy') {
				const fileIds = targets.filter((x) => x.kind === 'file').map((x) => x.id);
				const folderIds = targets.filter((x) => x.kind === 'folder').map((x) => x.id);
				await copyFiles(fileIds, currentId);
				await copyFolders(folderIds, currentId);
				ui.notify(t('files.copied', 'Copied'), 'success');
			} else {
				for (const tgt of targets) {
					if (tgt.id === currentId) continue;
					if (tgt.kind === 'file') await moveFile(tgt.id, currentId);
					else await moveFolder(tgt.id, currentId);
				}
				ui.notify(t('files.moved', 'Moved'), 'success');
			}
			open = false;
			onmoved?.();
		} catch (e) {
			errorToast(e);
		} finally {
			working = false;
		}
	}

	// (Re)initialise the picker each time it opens.
	$effect(() => {
		if (open && targets.length) void init();
	});

	const moveTitle = $derived.by(() => {
		if (mode === 'copy') {
			return targets.length > 1
				? t('files.copy_n', { n: targets.length }, 'Copy {{n}} items')
				: t('files.copy_title', { name: targets[0]?.name ?? '' }, 'Copy “{{name}}”');
		}
		return targets.length > 1
			? t('files.move_n', { n: targets.length }, 'Move {{n}} items')
			: t('files.move_title', { name: targets[0]?.name ?? '' }, 'Move “{{name}}”');
	});
</script>

<Modal bind:open title={moveTitle}>
	<div class="mv-nav">
		<button
			class="mv-nav-btn"
			title={t('breadcrumb.home', 'Home')}
			aria-label={t('breadcrumb.home', 'Home')}
			disabled={atHome}
			onclick={goHome}><Icon name="home" /></button
		>
		<button
			class="mv-nav-btn"
			title={t('dialogs.go_to_parent', 'Go to parent')}
			aria-label={t('dialogs.go_to_parent', 'Go to parent')}
			disabled={atHome}
			onclick={goParent}><Icon name="level-up-alt" /></button
		>
		<nav class="mv-crumbs" aria-label="Breadcrumb">
			{#each crumbs as c, i (c.id)}
				{#if i > 0}<span class="mv-sep">/</span>{/if}
				<button class="mv-crumb" onclick={() => gotoCrumb(i)}>{c.name}</button>
			{/each}
		</nav>
	</div>

	{#if loading}
		<p class="mv-status">{t('common.loading', 'Loading…')}</p>
	{:else if folders.length === 0}
		<p class="mv-status">{t('files.no_subfolders', 'No subfolders here.')}</p>
	{:else}
		<ul class="mv-list">
			{#each folders as f (f.id)}
				<li>
					<button class="mv-folder" disabled={targetIds.has(f.id)} onclick={() => enter(f)}>
						<Icon name="folder" /> <span>{f.name}</span>
						<Icon name="chevron-right" class="mv-enter" />
					</button>
				</li>
			{/each}
		</ul>
	{/if}

	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>
			{t('common.cancel', 'Cancel')}
		</button>
		<button class="btn btn-primary" disabled={working || !currentId} onclick={confirmMove}>
			{mode === 'copy' ? t('files.copy_here', 'Copy here') : t('files.move_here', 'Move here')}
		</button>
	{/snippet}
</Modal>

<style>
	.mv-nav {
		display: flex;
		align-items: center;
		gap: var(--space-1);
		margin-bottom: var(--space-3);
	}

	.mv-nav-btn {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 28px;
		height: 28px;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		background: var(--color-bg-input);
		color: var(--color-text);
		cursor: pointer;
		flex: none;
	}

	.mv-nav-btn:hover:not(:disabled) {
		background: var(--color-bg-hover);
	}

	.mv-nav-btn:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}

	.mv-crumbs {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0.25rem;
		min-width: 0;
	}

	.mv-crumb {
		background: none;
		border: none;
		color: var(--color-accent-text, var(--color-primary));
		cursor: pointer;
		padding: 0.125rem 0.25rem;
	}

	.mv-sep {
		color: var(--color-text-muted);
	}

	.mv-list {
		list-style: none;
		margin: 0;
		padding: 0;
		max-height: 50vh;
		overflow: auto;
	}

	.mv-folder {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		width: 100%;
		padding: 0.5rem 0.625rem;
		border: none;
		background: none;
		color: var(--color-text);
		cursor: pointer;
		border-radius: var(--radius-md);
		text-align: left;
	}

	.mv-folder:hover:not(:disabled) {
		background: var(--color-bg-hover);
	}

	.mv-folder:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}

	.mv-folder span {
		flex: 1;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	:global(.mv-enter) {
		color: var(--color-text-muted);
	}

	.mv-status {
		color: var(--color-text-muted);
		padding: 1rem 0;
		text-align: center;
	}
</style>
