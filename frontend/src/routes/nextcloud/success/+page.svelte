<script lang="ts">
	import { onMount } from 'svelte';
	import Icon from '$lib/icons/Icon.svelte';
	import { t } from '$lib/i18n/index.svelte';

	function closeWindow() {
		window.close();
	}

	onMount(() => {
		// Auto-close the tab a few seconds after landing. NC clients
		// receive their credentials through the LFv2 poll endpoint
		// (`/login/v2/poll`) in the backchannel — this browser tab is
		// only useful as a "flow succeeded" landing. Users who want
		// to keep it around click nothing; users who want it gone
		// get it gone automatically.
		const timer = setTimeout(closeWindow, 3000);
		return () => clearTimeout(timer);
	});
</script>

<svelte:head><title>{t('nextcloud.success_title', 'Access granted')} · OxiCloud</title></svelte:head
>

<main class="nc-status">
	<Icon name="check" class="nc-status__icon nc-status__icon--ok" />
	<h1>{t('nextcloud.success_title', 'Access granted')}</h1>
	<p>{t('nextcloud.success_body', 'You can now return to your application — it is connected.')}</p>
	<button
		type="button"
		class="nc-status__action"
		data-testid="nextcloud-success-close-btn"
		onclick={closeWindow}
	>
		{t('nextcloud.close_window', 'Close Window')}
	</button>
</main>

<style>
	.nc-status {
		/* `base/reset.css` sets `body { display: flex }`. Public
		   `/nextcloud/*` routes render children directly (bypassing
		   AppShell), so <main> is a flex item on the body's row axis
		   and needs to claim the full slot for its own centering to
		   land in the viewport middle. */
		flex: 1;
		min-height: 100dvh;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 1rem;
		text-align: center;
		padding: 2rem 1rem;
	}

	:global(.nc-status__icon) {
		font-size: 3rem;
	}

	:global(.nc-status__icon--ok) {
		color: var(--color-success-text);
	}

	.nc-status__action {
		margin-top: 0.5rem;
		padding: 0.5rem 1.25rem;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		background: var(--color-primary);
		color: var(--color-text-light);
		cursor: pointer;
	}
</style>
