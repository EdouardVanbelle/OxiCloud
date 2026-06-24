<script lang="ts">
	import Icon from '$lib/icons/Icon.svelte';
	import { t } from '$lib/i18n/index.svelte';
	import { resolveRecipient } from '$lib/api/endpoints/recipients';
	import { resolveUser, type ResolvedUser } from '$lib/api/endpoints/users';
	import { userInitials, avatarColorIndex } from '$lib/utils/avatar';
	import type { DriveMember } from '$lib/api/types';

	interface Props {
		members: DriveMember[];
		max?: number;
	}
	let { members, max = 6 }: Props = $props();

	// Render only Owner-role grants and exclude token subjects (they can't
	// own a drive per the service contract; defensive filter so a stray
	// row from a future role doesn't render as an avatar).
	const owners = $derived(
		members.filter(
			(m) => m.role === 'owner' && (m.subject.type === 'user' || m.subject.type === 'group')
		)
	);

	const shown = $derived(owners.slice(0, max));
	const overflow = $derived(Math.max(0, owners.length - shown.length));

	// Per-user profile cache (image + email + real name). The cache itself
	// lives in `resolveUser`; this state just mirrors what we've fetched so
	// reactive `$derived` recomputes when a lookup lands.
	let resolved = $state<Record<string, ResolvedUser | null>>({});

	$effect(() => {
		// Refresh on every membership change. `resolveUser` dedupes
		// concurrent calls and caches per id, so this is cheap when the
		// same id reappears across rows.
		const users = owners.filter((m) => m.subject.type === 'user');
		for (const m of users) {
			if (m.subject.id in resolved) continue;
			void resolveUser(m.subject.id).then((u) => {
				resolved = { ...resolved, [m.subject.id]: u };
			});
		}
	});

	function nameFor(m: DriveMember): string {
		if (m.subject.type === 'group') return resolveRecipient('group', m.subject.id).label;
		return resolved[m.subject.id]?.name ?? resolveRecipient('user', m.subject.id).label;
	}

	function imageFor(m: DriveMember): string | null {
		if (m.subject.type !== 'user') return null;
		return resolved[m.subject.id]?.image ?? null;
	}

	function isExternalFor(m: DriveMember): boolean {
		if (m.subject.type !== 'user') return false;
		return resolved[m.subject.id]?.isExternal ?? false;
	}

	// "Name — email" tooltip; group falls back to label only.
	function titleFor(m: DriveMember): string {
		const name = nameFor(m);
		if (m.subject.type === 'group') return name;
		const email = resolved[m.subject.id]?.email ?? '';
		return email ? `${name} — ${email}` : name;
	}
</script>

{#if owners.length === 0}
	<span class="owner-stack__empty">{t('admin.drive_no_owners', 'No owners')}</span>
{:else}
	<ul class="owner-stack" aria-label={t('admin.drive_owners_aria', 'Drive owners')}>
		{#each shown as m (`${m.subject.type}-${m.subject.id}`)}
			{@const title = titleFor(m)}
			{@const image = imageFor(m)}
			<li class="owner-stack__chip" {title}>
				{#if m.subject.type === 'group'}
					<span class="owner-stack__avatar owner-stack__avatar--group">
						<Icon name="users" />
					</span>
				{:else if image}
					<img class="owner-stack__photo" src={image} alt="" />
				{:else}
					<span class="owner-stack__avatar owner-stack__avatar--c{avatarColorIndex(m.subject.id)}">
						{userInitials(nameFor(m))}
					</span>
				{/if}
				{#if isExternalFor(m)}
					<span class="owner-stack__ext-badge" title={t('share.externalUser', 'External user')}>
						<Icon name="building-circle-xmark" />
					</span>
				{/if}
			</li>
		{/each}
		{#if overflow > 0}
			<li class="owner-stack__chip" title={owners.slice(max).map(titleFor).join('\n')}>
				<span class="owner-stack__avatar owner-stack__avatar--more">
					+{overflow}
				</span>
			</li>
		{/if}
	</ul>
{/if}

<style>
	.owner-stack {
		display: inline-flex;
		flex-direction: row;
		list-style: none;
		margin: 0;
		padding: 0;
	}

	/* Negative margin = overlap. The chip after this one slides under,
	   producing the stacked look. First chip keeps full margin so the
	   leftmost avatar isn't clipped by the container. */
	.owner-stack__chip {
		position: relative;
		margin-left: -0.5rem;
	}

	.owner-stack__chip:first-child {
		margin-left: 0;
	}

	.owner-stack__avatar,
	.owner-stack__photo {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 1.75rem;
		height: 1.75rem;
		border-radius: 50%;
		font-size: 0.7rem;
		font-weight: var(--weight-semibold, 600);
		color: var(--color-text-light);
		border: 2px solid var(--color-bg-surface);
		box-sizing: border-box;
		user-select: none;
		object-fit: cover;
		overflow: hidden;
	}

	/* Group icon avatar — neutral background, distinct from the coloured
	   user buckets so a viewer can tell user-vs-group at a glance. */
	.owner-stack__avatar--group {
		background: var(--color-bg-muted);
		color: var(--color-text);
	}

	/* "+N" overflow chip mirrors the group neutral palette. */
	.owner-stack__avatar--more {
		background: var(--color-bg-muted);
		color: var(--color-text);
		font-weight: var(--weight-semibold, 600);
		font-size: 0.65rem;
	}

	/* Colour buckets mirror UserVignette so the same user gets the same
	   colour across the app. */
	.owner-stack__avatar--c0 {
		background: var(--color-badge-indigo-bg);
		color: var(--color-badge-indigo-text);
	}

	.owner-stack__avatar--c1 {
		background: var(--color-badge-green-bg);
		color: var(--color-badge-green-text);
	}

	.owner-stack__avatar--c2 {
		background: var(--color-badge-orange-bg);
		color: var(--color-badge-orange-text);
	}

	.owner-stack__avatar--c3 {
		background: var(--color-badge-blue-bg);
		color: var(--color-badge-blue-text);
	}

	.owner-stack__avatar--c4 {
		background: var(--color-badge-amber-bg);
		color: var(--color-badge-amber-text);
	}

	/* External-user marker — small badge in the bottom-right corner. */
	.owner-stack__ext-badge {
		position: absolute;
		right: -2px;
		bottom: -2px;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 0.85rem;
		height: 0.85rem;
		border-radius: 50%;
		background: var(--color-bg-surface);
		color: var(--color-text-muted);
		font-size: 0.55rem;
	}

	.owner-stack__empty {
		color: var(--color-text-muted);
		font-style: italic;
		font-size: 0.8125rem;
	}
</style>
