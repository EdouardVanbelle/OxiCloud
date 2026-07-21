<script lang="ts">
	/**
	 * Reusable list-of-policy-toggles.
	 *
	 * Consumed by:
	 *   - Admin "Manage policies" modal — `readonly=false`, admin edits the
	 *     bound `values` in place.
	 *   - Drive settings page (`/config/drive/{uuid}`) — `readonly=true`,
	 *     drive members read the currently-in-effect state.
	 *
	 * The shared `policyDefs` in `$lib/utils/drivePolicies` is the single
	 * source of truth for label + help text + implied-by relations. Adding
	 * a policy is one push there + one row in `DrivePolicies` in
	 * `types.ts`; the two consuming surfaces update automatically.
	 */
	import type { DrivePoliciesPartial } from '$lib/api/types';
	import { isPolicyImplied, policyDefs, type PolicyDef } from '$lib/utils/drivePolicies';

	interface Props {
		/** Current values displayed on each row. */
		values: Required<DrivePoliciesPartial>;
		/** `true` = display only, disables the checkboxes so members can see the
		 *  live state without a mutation affordance. When `true`, `onchange`
		 *  is ignored — the component never emits. */
		readonly?: boolean;
		/** Additional disable signal (used by the admin modal during save). */
		busy?: boolean;
		/** Prefix for the `data-testid` on each checkbox
		 *  (e.g. `admin-policy-…` on the admin page, `drive-policy-…` on
		 *  the config page). Keeps test selectors stable per surface. */
		testIdPrefix?: string;
		/** Fired when the user toggles a checkbox (mutable surface only).
		 *  The parent owns the storage and applies the change. Not called
		 *  in `readonly` mode. */
		onchange?: (key: PolicyDef['key'], next: boolean) => void;
	}

	let {
		values,
		readonly = false,
		busy = false,
		testIdPrefix = 'policy',
		onchange
	}: Props = $props();
</script>

<ul class="policy-list">
	{#each policyDefs as def (def.key)}
		{@const implied = isPolicyImplied(def, values)}
		<li class="policy-row" class:policy-row--implied={implied}>
			<label class="policy-row__label">
				<span class="policy-row__head">
					<input
						type="checkbox"
						data-testid={`${testIdPrefix}-${def.key}`}
						checked={values[def.key]}
						disabled={readonly || busy || implied}
						onchange={(e) => onchange?.(def.key, (e.currentTarget as HTMLInputElement).checked)}
					/>
					<span class="policy-row__title">{def.label()}</span>
				</span>
				<span class="policy-row__help muted">
					{def.help()}
					{#if implied && def.impliedHint}
						<span class="policy-row__implied">{def.impliedHint()}</span>
					{/if}
				</span>
			</label>
		</li>
	{/each}
</ul>

<style>
	/* Ported from the admin modal's original block so the visual stays
	   identical when the modal switches to this component; the read-only
	   surface on `/config/drive/{uuid}` gets the same look for free. */
	.policy-list {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
	}

	.policy-row {
		padding: var(--space-2);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
	}

	.policy-row__label {
		/* Column layout: head (checkbox + title inline) on top, help
		   text underneath. The checkbox + title share a row via
		   `.policy-row__head` so the title sits beside the checkbox
		   instead of wrapping to its own line. */
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		cursor: pointer;
		margin: 0;
	}

	.policy-row__head {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		min-width: 0;
	}

	.policy-row__head input[type='checkbox'] {
		margin: 0;
		flex-shrink: 0;
	}

	.policy-row__title {
		font-weight: 600;
	}

	.policy-row__help {
		/* Indent the help text under the title so the relationship is
		   visually obvious. Width = checkbox width + the head's gap. */
		padding-left: calc(1rem + var(--space-2));
	}

	/* Implied state — the row's gate is already covered by a broader
	   policy (e.g. forbid_public_links when forbid_sharing is on).
	   Visually dimmed so the admin understands they don't need to
	   toggle it; the stored value is preserved for the moment they
	   relax the parent policy. Same treatment used on the read-only
	   surface so subordinate rules read as visually secondary. */
	.policy-row--implied {
		opacity: 0.55;
	}

	.policy-row--implied .policy-row__label {
		cursor: not-allowed;
	}

	.policy-row__implied {
		display: block;
		margin-top: var(--space-1);
		font-style: italic;
	}
</style>
