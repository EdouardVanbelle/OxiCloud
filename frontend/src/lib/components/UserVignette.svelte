<script lang="ts">
	/**
	 * Identity chip for a user in share/recipient lists: avatar (uploaded photo
	 * or coloured initials), display name, email, and an internal-vs-external
	 * badge. Resolves the profile lazily via `/api/users/{id}` (cached) so
	 * external users show real details instead of a bare UUID. Falls back to a
	 * caller-supplied label/sublabel while resolving or when not visible.
	 */
	import Icon from '$lib/icons/Icon.svelte';
	import { t } from '$lib/i18n/index.svelte';
	import { resolveUser, type ResolvedUser } from '$lib/api/endpoints/users';
	import { userInitials, avatarColorIndex } from '$lib/utils/avatar';

	interface Props {
		userId: string;
		fallbackLabel?: string;
		fallbackSublabel?: string;
	}
	let { userId, fallbackLabel, fallbackSublabel }: Props = $props();

	let resolved = $state<ResolvedUser | null>(null);
	$effect(() => {
		let alive = true;
		resolved = null;
		void resolveUser(userId).then((u) => {
			if (alive) resolved = u;
		});
		return () => {
			alive = false;
		};
	});

	const label = $derived(resolved?.name ?? fallbackLabel ?? userId);
	const email = $derived(resolved?.email || fallbackSublabel || '');
	const isExternal = $derived(resolved?.isExternal ?? false);
	const image = $derived(resolved?.image ?? null);
	const colorIndex = $derived(avatarColorIndex(userId));
	const initials = $derived(userInitials(label));
</script>

<span class="uv">
	<span class="uv__avatar">
		{#if image}
			<img class="uv__photo" src={image} alt="" />
		{:else}
			<span class="uv__initials uv__initials--c{colorIndex}">{initials}</span>
		{/if}
		{#if isExternal}
			<span class="uv__badge" title={t('share.externalUser', 'External user')}>
				<Icon name="building-circle-xmark" />
			</span>
		{/if}
	</span>
	<span class="uv__text">
		<span class="uv__name">{label}</span>
		{#if email}<span class="uv__email">{email}</span>{/if}
	</span>
</span>

<style>
	.uv {
		display: flex;
		flex: 1;
		min-width: 0;
		align-items: center;
		gap: var(--space-2);
	}

	.uv__avatar {
		position: relative;
		flex-shrink: 0;
	}
	/* Colour buckets mirror AppShell's .avatar--c* (shared userVignette palette). */
	.uv__photo,
	.uv__initials {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 32px;
		height: 32px;
		border-radius: 50%;
		overflow: hidden;
		font-size: var(--text-xs);
		font-weight: var(--weight-bold);
	}

	.uv__photo {
		object-fit: cover;
	}

	.uv__initials--c0 {
		background: var(--color-badge-indigo-bg);
		color: var(--color-badge-indigo-text);
	}

	.uv__initials--c1 {
		background: var(--color-badge-green-bg);
		color: var(--color-badge-green-text);
	}

	.uv__initials--c2 {
		background: var(--color-badge-orange-bg);
		color: var(--color-badge-orange-text);
	}

	.uv__initials--c3 {
		background: var(--color-badge-blue-bg);
		color: var(--color-badge-blue-text);
	}

	.uv__initials--c4 {
		background: var(--color-badge-amber-bg);
		color: var(--color-badge-amber-text);
	}

	.uv__badge {
		position: absolute;
		right: -3px;
		bottom: -3px;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 16px;
		height: 16px;
		border-radius: 50%;
		background: var(--color-bg-surface);
		color: var(--color-text-muted);
		font-size: 9px;
	}

	.uv__text {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}

	.uv__name {
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.uv__email {
		font-size: var(--text-xs);
		color: var(--color-text-muted);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
</style>
