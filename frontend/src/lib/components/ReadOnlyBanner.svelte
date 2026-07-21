<script lang="ts">
	/**
	 * Read-only drive banner.
	 *
	 * Rendered at the top of any page whose content lives in (or is scoped
	 * to) a drive whose `policies.read_only === true`. Members see the
	 * banner and understand why upload / rename / delete / share
	 * affordances elsewhere in the app fail with a generic error toast —
	 * the backend engine gate refuses every non-`Read` permission on
	 * resources in the drive.
	 *
	 * Only `Read` permissions pass; the banner does not need to gate any
	 * behavior itself. It's pure signage. Backed by
	 * `docs/plan/drive.md` §8 (`read_only`).
	 *
	 * Consumed by:
	 *   - `routes/config/drive/[uuid]/+page.svelte` — always shown when
	 *     the drive being configured is frozen.
	 *   - `routes/files/[...path]/+page.svelte` — shown when the current
	 *     folder's owning drive is frozen (parent looks up drive via
	 *     `drives.findByRootFolderId`/`findById`).
	 *   - Future: `/photos`, `/music`, and any other drive-scoped views.
	 */
	import { t } from '$lib/i18n/index.svelte';
	import Icon from '$lib/icons/Icon.svelte';

	interface Props {
		/** Drive-name shown in the body so members know which drive the
		 *  freeze applies to. Optional — omit on pages where the drive is
		 *  implicit from context (e.g. the drive's own config page). */
		driveName?: string;
	}

	let { driveName }: Props = $props();
</script>

<div
	class="read-only-banner"
	role="region"
	aria-label={t('drive.read_only_banner.aria', 'This drive is read-only')}
	data-testid="read-only-banner"
>
	<div class="read-only-banner__icon" aria-hidden="true">
		<Icon name="lock" />
	</div>
	<div class="read-only-banner__body">
		<strong>
			{#if driveName}
				{t(
					'drive.read_only_banner.title_named',
					{ name: driveName },
					'Drive "{{name}}" is read-only'
				)}
			{:else}
				{t('drive.read_only_banner.title', 'This drive is read-only')}
			{/if}
		</strong>
		<span>
			{t(
				'drive.read_only_banner.body',
				'Uploads, edits, deletes, renames, sharing and membership changes are refused. Reads and downloads keep working. Contact an administrator to un-freeze the drive.'
			)}
		</span>
	</div>
</div>

<style>
	/* Shape matches the sibling upgrade-banner in
	   `routes/shared-with-me/+page.svelte` so the two banners read as
	   the same family; only the accent shifts to communicate "info /
	   frozen" rather than "action / upgrade." */
	.read-only-banner {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		padding: var(--space-3) var(--space-4);
		margin-bottom: var(--space-4);
		background: var(--color-surface-raised);
		border: 1px solid var(--color-border);
		border-left: 4px solid var(--color-accent);
		border-radius: var(--radius-md);
	}

	.read-only-banner__icon {
		flex-shrink: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		width: 2rem;
		height: 2rem;
		border-radius: var(--radius-md);
		background: var(--color-surface);
		color: var(--color-accent);
		font-size: var(--text-lg);
	}

	.read-only-banner__body {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		min-width: 0;
	}

	.read-only-banner__body strong {
		font-weight: var(--weight-semibold);
		color: var(--color-text);
	}

	.read-only-banner__body span {
		color: var(--color-text-muted);
		font-size: var(--text-sm);
	}

	@media (width <= 600px) {
		.read-only-banner {
			align-items: flex-start;
		}
	}
</style>
