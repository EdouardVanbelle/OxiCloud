<script lang="ts">
	import type { Snippet } from 'svelte';
	import Icon from '$lib/icons/Icon.svelte';

	type Variant = 'primary' | 'secondary' | 'danger';

	interface Props {
		/** Visual style → `.btn-{variant}` (default `secondary`). */
		variant?: Variant;
		/** Optional leading icon-registry name. */
		icon?: string;
		/** Compact size → adds `.btn-sm`. */
		small?: boolean;
		type?: 'button' | 'submit' | 'reset';
		disabled?: boolean;
		title?: string;
		onclick?: (e: MouseEvent) => void;
		/** Extra classes appended after the base `.btn` classes. */
		class?: string;
		children?: Snippet;
	}

	let {
		variant = 'secondary',
		icon,
		small = false,
		type = 'button',
		disabled = false,
		title,
		onclick,
		class: cls = '',
		children
	}: Props = $props();

	const className = $derived(
		['btn', `btn-${variant}`, small ? 'btn-sm' : '', cls].filter(Boolean).join(' ')
	);
</script>

<button class={className} {type} {disabled} {title} {onclick}>
	{#if icon}<Icon name={icon} />{/if}
	{@render children?.()}
</button>
