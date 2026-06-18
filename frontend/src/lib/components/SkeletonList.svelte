<script lang="ts">
	import { files as filesStore } from '$lib/stores/files.svelte';

	interface Props {
		/** Number of placeholder cards/rows to render (default 6). */
		count?: number;
	}

	let { count = 6 }: Props = $props();

	const placeholders = $derived(Array.from({ length: count }, (_, i) => i));
</script>

<div class="files-container">
	<div class={filesStore.viewMode === 'grid' ? 'files-grid-view files-skeleton' : 'files-skeleton'}>
		{#each placeholders as i (i)}
			{#if filesStore.viewMode === 'grid'}
				<div class="skeleton-card">
					<div class="skeleton skeleton-thumb"></div>
					<div class="skeleton skeleton-line skeleton-line--medium"></div>
					<div class="skeleton skeleton-line skeleton-line--short"></div>
				</div>
			{:else}
				<div class="skeleton-row">
					<div class="skeleton skeleton-icon"></div>
					<div class="skeleton skeleton-line skeleton-line--medium"></div>
					<div class="skeleton skeleton-line skeleton-line--short"></div>
				</div>
			{/if}
		{/each}
	</div>
</div>
