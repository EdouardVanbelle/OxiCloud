<script lang="ts">
	import { errorToast } from '$lib/utils/errors';
	import { getEditorUrlWithFallback } from '$lib/api/endpoints/wopi';
	import Icon from '$lib/icons/Icon.svelte';
	import { t } from '$lib/i18n/index.svelte';

	interface Props {
		open: boolean;
		fileId: string | null;
		fileName: string;
		action?: 'edit' | 'view';
		onclose?: () => void;
	}

	let { open = $bindable(false), fileId, fileName, action = 'edit', onclose }: Props = $props();

	let form = $state<HTMLFormElement | null>(null);
	let editorUrl = $state('');
	let token = $state('');
	let tokenTtl = $state('');
	let loading = $state(false);

	function close() {
		open = false;
		editorUrl = '';
		onclose?.();
	}

	function onKeydown(e: KeyboardEvent) {
		if (open && e.key === 'Escape') close();
	}

	// The editor iframe posts status messages (Collabora / OnlyOffice WOPI
	// protocol). We drop the spinner once it loads, and close the host modal
	// when the editor's own close button fires UI_Close / Document close.
	function onMessage(e: MessageEvent) {
		if (!open) return;
		let data: Record<string, unknown>;
		try {
			data = JSON.parse(typeof e.data === 'string' ? e.data : '') as Record<string, unknown>;
		} catch {
			return; // not a JSON message — ignore
		}
		const msgId = String(data.MessageId ?? data.messageId ?? '');
		if (msgId === 'UI_Close' || msgId === 'close') {
			close();
		} else if (msgId === 'App_LoadingStatus') {
			const values = data.Values as { Status?: string } | undefined;
			const status = values?.Status;
			if (status === 'Document_Loaded' || status === 'Frame_Ready') {
				loading = false;
			}
		}
	}

	// When opened, fetch the editor URL + token, then submit the (hidden) form
	// into the iframe — this is the WOPI host-page POST handshake.
	$effect(() => {
		if (!open || !fileId) return;
		loading = true;
		editorUrl = '';
		getEditorUrlWithFallback(fileId, fileName, action)
			.then((data) => {
				editorUrl = data.editor_url;
				token = data.access_token;
				tokenTtl = String(data.access_token_ttl);
				// Submit on the next microtask once the form has the values bound.
				queueMicrotask(() => form?.submit());
			})
			.catch((e) => {
				errorToast(e);
				close();
			})
			.finally(() => (loading = false));
	});
</script>

<svelte:window onkeydown={onKeydown} onmessage={onMessage} />

{#if open}
	<div class="wopi" role="dialog" aria-modal="true" aria-label={fileName}>
		<header class="wopi__bar">
			<span class="wopi__title">{fileName}</span>
			<button class="wopi__close" aria-label={t('common.close', 'Close')} onclick={close}>
				<Icon name="times" />
			</button>
		</header>
		<div class="wopi__frame-wrap">
			{#if loading}
				<p class="wopi__status">{t('common.loading', 'Loading…')}</p>
			{/if}
			{#if editorUrl}
				<form
					bind:this={form}
					action={editorUrl}
					method="post"
					target="wopi_frame"
					class="wopi__form"
				>
					<input type="hidden" name="access_token" value={token} />
					<input type="hidden" name="access_token_ttl" value={tokenTtl} />
				</form>
			{/if}
			<iframe
				name="wopi_frame"
				title={t('files.editor', 'Document editor')}
				class="wopi__frame"
				allow="clipboard-read; clipboard-write"
				allowfullscreen
				sandbox="allow-scripts allow-same-origin allow-forms allow-popups allow-top-navigation allow-popups-to-escape-sandbox"
			></iframe>
		</div>
	</div>
{/if}

<style>
	.wopi {
		position: fixed;
		inset: 0;
		z-index: 1100;
		display: flex;
		flex-direction: column;
		background: var(--color-bg-base, var(--color-bg-surface));
	}

	.wopi__bar {
		display: flex;
		align-items: center;
		justify-content: space-between;
		height: 40px;
		padding: 0 1rem;
		background: var(--color-bg-elevated, var(--color-bg-surface));
		border-bottom: 1px solid var(--color-border);
		color: var(--color-text-heading);
	}

	.wopi__title {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.wopi__close {
		background: none;
		border: none;
		color: var(--color-text);
		cursor: pointer;
		font-size: 1.1rem;
	}

	.wopi__frame-wrap {
		position: relative;
		flex: 1;
	}

	.wopi__form {
		display: none;
	}

	.wopi__frame {
		width: 100%;
		height: 100%;
		border: none;
	}

	.wopi__status {
		position: absolute;
		inset: 0;
		display: grid;
		place-items: center;
		color: var(--color-text-muted);
	}
</style>
