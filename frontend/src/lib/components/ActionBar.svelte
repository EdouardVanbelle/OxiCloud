<script lang="ts">
	/**
	 * Two-slot action-bar layout used above every ResourceList surface.
	 *
	 *   [ start-slot ]   ← page actions / batch actions
	 *   [ end-slot   ]   ← <DisplayModeControls /> — group-by, sort, view, dotfile
	 *
	 * This is pure layout — no state, no visual variation per section.
	 * It reuses the existing `.actions-bar` / `.action-buttons` classes
	 * defined globally in `styles/ported/content.css` so it renders
	 * identically to `<ListToolbar>` (the component it's replacing).
	 *
	 * Consumers pass whatever they want on either side; `<ResourceList>`
	 * uses this internally to wire its `actions` / `batchActions` /
	 * display-mode-controls snippets, and pages can also use it directly
	 * when they need a bespoke layout that doesn't fit ResourceList's
	 * default (e.g. `/files` upload split-button).
	 */
	import type { Snippet } from 'svelte';

	interface Props {
		/** Left cluster — page action buttons (or batch actions on
		 *  selection). Omit to render an empty placeholder that still
		 *  reserves the space, so the end cluster stays right-aligned. */
		start?: Snippet;
		/** Right cluster — usually a `<DisplayModeControls />` instance,
		 *  but any content works. */
		end?: Snippet;
	}

	let { start, end }: Props = $props();
</script>

<div class="actions-bar">
	{#if start}{@render start()}{:else}<div class="action-buttons"></div>{/if}
	{#if end}{@render end()}{/if}
</div>
