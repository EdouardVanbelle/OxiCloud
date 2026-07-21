<script lang="ts">
	import { page } from '$app/state';
	import Icon from '$lib/icons/Icon.svelte';
	import { t } from '$lib/i18n/index.svelte';

	type ErrorAction = 'retry' | 'close';
	interface ErrorView {
		title: string;
		message: string;
		actionLabel: string;
		action: ErrorAction;
	}

	// Legacy used `?type=`; the rewrite briefly renamed it to `?reason=`. Read
	// `type` first and fall back to `reason` so older links keep working.
	const errorType = $derived(
		page.url.searchParams.get('type') ?? page.url.searchParams.get('reason') ?? 'generic'
	);

	const view = $derived<ErrorView>(buildView(errorType));

	function buildView(type: string): ErrorView {
		switch (type) {
			case 'invalid-credentials':
				return {
					title: t('nextcloud.error_invalid_title', 'Login Failed'),
					message: t(
						'nextcloud.error_invalid_body',
						'Invalid username or password. Please check your credentials and try again.'
					),
					actionLabel: t('common.retry', 'Try Again'),
					action: 'retry'
				};
			case 'session-expired':
				return {
					title: t('nextcloud.error_expired_title', 'Session Expired'),
					message: t('nextcloud.error_expired_body', 'Your session has expired. Please try again.'),
					actionLabel: t('nextcloud.close_window', 'Close Window'),
					action: 'close'
				};
			case 'not-found':
				return {
					title: t('nextcloud.error_notfound_title', 'Not Found'),
					message: t('nextcloud.error_notfound_body', 'The requested page was not found.'),
					actionLabel: t('nextcloud.close_window', 'Close Window'),
					action: 'close'
				};
			default:
				return {
					title: t('nextcloud.error_title', 'Error'),
					message: t(
						'nextcloud.error_generic_body',
						'An unexpected error occurred. Please try again.'
					),
					actionLabel: t('nextcloud.close_window', 'Close Window'),
					action: 'close'
				};
		}
	}

	function onAction() {
		if (view.action === 'retry') history.back();
		else window.close();
	}
</script>

<svelte:head><title>{view.title} · OxiCloud</title></svelte:head>

<main class="nc-status">
	<Icon name="exclamation-circle" class="nc-status__icon nc-status__icon--err" />
	<h1>{view.title}</h1>
	<p>{view.message}</p>
	<button
		type="button"
		class="nc-status__action"
		data-testid="nextcloud-error-action-btn"
		onclick={onAction}>{view.actionLabel}</button
	>
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

	:global(.nc-status__icon--err) {
		/* `--color-danger-text` is white — it's the foreground for text
		   sitting on a red button bg, not the standalone red glyph
		   colour. `--color-error-text` is the light-dark(...) red pair
		   designed for standalone use on the page background: darker
		   red in light mode, softer coral in dark mode. */
		color: var(--color-error-text);
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
