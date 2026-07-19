<script lang="ts">
	/**
	 * Identity chip for a group subject in share/member lists: neutral group
	 * avatar, display name, "N member(s)" sublabel, and — on hover — the
	 * expanded list of user members surfaced as a native `title` tooltip.
	 *
	 * Mirrors <UserVignette> so the two chip shapes align across share and
	 * membership surfaces. Uses the preloaded group name cache
	 * (`resolveRecipient` / `ensureResolvers` in `endpoints/recipients.ts`)
	 * so the name lands synchronously when the parent has already primed
	 * the cache; falls back to the group id while resolving.
	 *
	 * Members are fetched lazily on mount via `listMembers` and cached
	 * per-id at the module level so multiple chips for the same group
	 * share one round-trip. Nested group members (kind === 'group') expand
	 * one level and surface as "+ group X" lines in the tooltip; deeper
	 * expansion isn't attempted here — the read-only summary would get
	 * unwieldy and the AuthZ engine expands transitively at check-time.
	 */
	import Icon from '$lib/icons/Icon.svelte';
	import { t } from '$lib/i18n/index.svelte';
	import { resolveRecipient } from '$lib/api/endpoints/recipients';
	import { resolveUser } from '$lib/api/endpoints/users';
	import { listMembers, type GroupMember } from '$lib/api/endpoints/groups';
	import { SvelteMap } from 'svelte/reactivity';

	interface Props {
		groupId: string;
	}
	let { groupId }: Props = $props();

	const label = $derived(resolveRecipient('group', groupId).label);

	// Module-scoped cache of resolved members per group. Chips render N
	// times on a busy /shared page; the shared cache avoids N × HTTP.
	const memberCache = new SvelteMap<string, Promise<GroupMember[]>>();
	function loadMembers(id: string): Promise<GroupMember[]> {
		let p = memberCache.get(id);
		if (!p) {
			p = listMembers(id).catch(() => [] as GroupMember[]);
			memberCache.set(id, p);
		}
		return p;
	}

	let members = $state<GroupMember[]>([]);
	let memberNames = $state<string[]>([]);
	let membersLoaded = $state(false);

	$effect(() => {
		let alive = true;
		members = [];
		memberNames = [];
		membersLoaded = false;
		void loadMembers(groupId).then(async (list) => {
			if (!alive) return;
			members = list;
			// Resolve user members to display names; group members show
			// as `+ <group name>` via the recipient cache. All resolutions
			// happen in parallel; each `resolveUser` is itself cached.
			const names = await Promise.all(
				list.map(async (m) => {
					if (m.kind === 'user') {
						const u = await resolveUser(m.id).catch(() => null);
						return u?.name ?? u?.email ?? m.id;
					}
					return `+ ${resolveRecipient('group', m.id).label}`;
				})
			);
			if (!alive) return;
			memberNames = names;
			membersLoaded = true;
		});
		return () => {
			alive = false;
		};
	});

	const memberCount = $derived(members.length);

	// Native `title` tooltip carries the member list. Cheap, works on
	// keyboard focus, no popover machinery needed for a read-only chip.
	const titleText = $derived(
		!membersLoaded
			? label
			: memberNames.length === 0
				? `${label} — ${t('group.members_empty', 'No members')}`
				: `${label}\n${memberNames.join('\n')}`
	);

	const sublabel = $derived(
		membersLoaded
			? t(
					'group.member_count',
					{ n: memberCount },
					memberCount === 1 ? '1 member' : `${memberCount} members`
				)
			: ''
	);
</script>

<span class="gv" title={titleText}>
	<span class="gv__avatar" aria-hidden="true">
		<Icon name="users" />
	</span>
	<span class="gv__text">
		<span class="gv__name">{label}</span>
		{#if sublabel}<span class="gv__sub">{sublabel}</span>{/if}
	</span>
</span>

<style>
	.gv {
		display: flex;
		flex: 1;
		min-width: 0;
		align-items: center;
		gap: var(--space-2);
	}

	.gv__avatar {
		flex-shrink: 0;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 32px;
		height: 32px;
		border-radius: 50%;
		background: var(--color-bg-muted);
		color: var(--color-text);
		font-size: 0.85rem;
		user-select: none;
	}

	.gv__text {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}

	.gv__name {
		font-weight: var(--weight-semibold, 600);
		color: var(--color-text);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.gv__sub {
		color: var(--color-text-muted);
		font-size: 0.8125rem;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
</style>
