/**
 * Shared drive-policy definitions.
 *
 * Consumed by two surfaces:
 *   - Admin "Manage policies" modal (`routes/admin/+page.svelte`) — read+write.
 *   - Drive settings page (`routes/config/drive/[uuid]/+page.svelte`) — read-only,
 *     so drive members can see which policies an admin has set.
 *
 * Kept in a plain `.ts` module (not a component) so both consumers import the
 * same array and the definition of "one policy" lives in exactly one place.
 * Adding a sixth policy is a single push here + one migration + the
 * `DrivePolicies` interface extension in `types.ts`. See
 * `docs/plan/drive.md` §8 (forbid_* gates) + §15 (include_in_*_index scope).
 */
import { t } from '$lib/i18n/index.svelte';
import type { DrivePoliciesPartial } from '$lib/api/types';

/**
 * `impliedBy` captures the semantic dependency between policies: when the
 * named parent policy is on, this subordinate gate is moot (its enforcement
 * is already covered by the broader rule). The admin modal disables the
 * child toggle and shows `impliedHint` so the admin understands the
 * hierarchy without our having to mutate the stored value — their
 * preference is preserved for the moment they relax the parent. The
 * read-only config surface uses the same signal to dim implied rows.
 */
export interface PolicyDef {
	key: keyof Required<DrivePoliciesPartial>;
	label: () => string;
	help: () => string;
	impliedBy?: keyof Required<DrivePoliciesPartial>;
	impliedHint?: () => string;
}

/**
 * Mirrors the entity field order in `src/domain/entities/drive.rs` so a
 * future policy lands here as one literal-array push.
 */
export const policyDefs: PolicyDef[] = [
	{
		key: 'forbid_sharing',
		label: () => t('admin.drive_policy.forbid_sharing', 'Forbid per-resource sharing'),
		help: () =>
			t(
				'admin.drive_policy.forbid_sharing_help',
				'Block per-file / per-folder grants (covers public links and external sharing as well). Drive-level membership still works.'
			)
	},
	{
		key: 'forbid_public_links',
		label: () => t('admin.drive_policy.forbid_public_links', 'Forbid public links'),
		help: () =>
			t(
				'admin.drive_policy.forbid_public_links_help',
				'Block anonymous share links on resources in this drive.'
			),
		impliedBy: 'forbid_sharing',
		impliedHint: () =>
			t(
				'admin.drive_policy.implied_by_forbid_sharing',
				'Already enforced by Forbid per-resource sharing.'
			)
	},
	{
		key: 'forbid_external_sharing',
		label: () => t('admin.drive_policy.forbid_external_sharing', 'Forbid external sharing'),
		help: () =>
			t(
				'admin.drive_policy.forbid_external_sharing_help',
				'Block grants to external users (email invitations and pre-existing external accounts).'
			),
		impliedBy: 'forbid_sharing',
		impliedHint: () =>
			t(
				'admin.drive_policy.implied_by_forbid_sharing',
				'Already enforced by Forbid per-resource sharing.'
			)
	},
	{
		key: 'forbid_cross_drive_move',
		label: () => t('admin.drive_policy.forbid_cross_drive_move', 'Forbid cross-drive move'),
		help: () =>
			t(
				'admin.drive_policy.forbid_cross_drive_move_help',
				'Block moving files or folders out to another drive. Does not stop download + re-upload.'
			)
	},
	{
		key: 'forbid_owner_role_change',
		label: () => t('admin.drive_policy.forbid_owner_role_change', 'Lock Owner roster'),
		help: () =>
			t(
				'admin.drive_policy.forbid_owner_role_change_help',
				'Only admin can add, remove, or demote drive Owners while this is on.'
			)
	},
	{
		key: 'include_in_photo_index',
		label: () => t('admin.drive_policy.include_in_photo_index', 'Include in Photos'),
		help: () =>
			t(
				'admin.drive_policy.include_in_photo_index_help',
				'Show image and video files from this drive in the Photos timeline and on the Places map. Default personal drives are opted in automatically; turn on for shared drives that genuinely hold photos (e.g. "Family Photos").'
			)
	},
	{
		key: 'include_in_music_index',
		label: () => t('admin.drive_policy.include_in_music_index', 'Include in Music'),
		help: () =>
			t(
				'admin.drive_policy.include_in_music_index_help',
				'Include audio files from this drive in the Music library. Default personal drives are opted in automatically; turn on for shared drives that genuinely hold a music collection (e.g. "Family Music", "Band Collaboration").'
			)
	}
];

/**
 * True when `def` is subordinate to another policy whose value is currently
 * `true` in `values`. Both surfaces use this to gray out implied rows.
 */
export function isPolicyImplied(def: PolicyDef, values: Required<DrivePoliciesPartial>): boolean {
	return def.impliedBy != null && values[def.impliedBy];
}

/**
 * JSONB reader — the backend may hold a raw `Record<string, unknown>` bag
 * (unknown keys preserved verbatim), so any missing / non-bool key resolves
 * to `false`. Shared between the admin modal (initialising the edit draft)
 * and the config/drive page (reading the current state for display).
 */
export function readPolicyBool(p: Record<string, unknown>, key: string): boolean {
	const v = p[key];
	return typeof v === 'boolean' ? v : false;
}

/**
 * Populate a full `Required<DrivePoliciesPartial>` from the JSONB bag by
 * reading each known key with `readPolicyBool`. Both admin and config
 * surfaces call this on load; the admin edits the returned object in
 * place while the config surface renders it read-only.
 */
export function readAllPolicies(p: Record<string, unknown>): Required<DrivePoliciesPartial> {
	const out = {} as Required<DrivePoliciesPartial>;
	for (const def of policyDefs) {
		out[def.key] = readPolicyBool(p, def.key);
	}
	return out;
}
