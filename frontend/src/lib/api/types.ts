/**
 * API wire types — ported from static/js/core/types.js.
 *
 * This is a focused, hand-ported subset covering the core resources. The plan
 * is to regenerate the full set from the backend OpenAPI (`just openapi` +
 * `openapi-typescript`) so these track the Rust DTOs; until then, extend here.
 */

export type ItemType = 'file' | 'folder';

export interface LightItem {
	id: string;
	name: string;
	type: ItemType;
	parentId: string;
}

export interface FolderItem {
	category: string;
	created_at: number;
	icon_class: string;
	icon_special_class: string;
	id: string;
	is_root: boolean;
	modified_at: number;
	name: string;
	// §14 provenance — who originally created the folder. `null` when
	// the creating user has since been deleted (backend FK is
	// `ON DELETE SET NULL`), or when the folder is returned to a
	// share recipient that lost provenance via
	// `FolderDto::without_hierarchy_info`. The canonical "owner"
	// signal on the Files browser / Favorites / Shared surfaces
	// (replaced the retired `owner_id` field in D7).
	created_by: string | null;
	// §14 provenance — who last touched the folder (rename / move /
	// metadata change). The canonical "who touched this recently"
	// signal on the Recent surface.
	updated_by: string | null;
	parent_id: string | null;
	path: string;
	etag: string;
}

export interface FileItem {
	category: string;
	created_at: number;
	icon_class: string;
	icon_special_class: string;
	id: string;
	mime_type: string;
	modified_at: number;
	name: string;
	// §14 provenance — see FolderItem for semantics. Replaced the
	// retired `owner_id` field in D7.
	created_by: string | null;
	updated_by: string | null;
	folder_id: string;
	path: string;
	size: number;
	size_formatted: string;
	sort_date: number;
	etag: string;
	content_hash: string;
	/** Search-only: plain-text fragment around a content match. */
	snippet?: string;
	/** Search-only: "name" or "content". */
	match_source?: string;
}

export interface ShareItem {
	access_count: number;
	created_at: number;
	created_by: string;
	expires_at: number;
	has_password: boolean;
	id: string;
	item_id: string;
	item_name: string;
	item_type: ItemType;
	token: string | null;
	url: string;
}

export interface CreateShare {
	item_id: string;
	item_name?: string | null;
	item_type: ItemType;
	password: string | null;
	expires_at: number | null;
}

export interface UpdateShare {
	password?: string | null;
	expires_at?: number | null;
}

export interface FavoriteItem {
	id: string;
	user_id: string;
	item_id: string;
	item_type: ItemType;
	created_at: number;
	item_name: string | null;
	item_size: number | null;
	item_mime_type: string | null;
	parent_id: string | null;
	modified_at: number | null;
	item_path: string;
	icon_class: string;
	icon_special_class: string;
	category: string;
	size_formatted: string;
}

export interface RecentItem {
	id: string;
	user_id: string;
	item_id: string;
	item_type: ItemType;
	accessed_at: number;
	item_name: string | null;
	item_size: number | null;
	item_mime_type: string | null;
	parent_id: string | null;
	item_path: string;
	icon_class: string;
	icon_special_class: string;
	category: string;
	size_formatted: string;
}

export interface TrashResourceItem {
	resource_type: ItemType;
	trashed_at: string;
	deletion_date: string;
	/**
	 * Drive the trashed item belongs to (D2b). Enables client-side
	 * group-by-drive in the `/trash` UI without resolving the drive from
	 * `resource.drive_id` per row. The drive's display name resolves
	 * against `drives.svelte` (the in-memory store already populated by
	 * the sidebar picker / config pages — no extra round-trip).
	 */
	drive_id: string;
	resource: FileItem | FolderItem;
}

export interface TrashResourcesResponse {
	items: TrashResourceItem[];
	next_cursor?: string;
}

export type Role = 'user' | 'admin';

/** Wire shape of `UserDto` (backend: src/application/dtos/user_dto.rs). */
export interface User {
	id: string;
	username?: string;
	email: string;
	role: string;
	storage_quota_bytes: number;
	storage_used_bytes: number;
	created_at: string;
	updated_at: string;
	last_login_at?: string | null;
	active: boolean;
	auth_provider: string;
	image?: string | null;
	can_edit_image: boolean;
	is_external: boolean;
	given_name?: string;
	family_name?: string;
	email_verified_at?: string;
	preferred_locale?: string;
	notify_on_share: boolean;
	/**
	 * Opaque UI preferences bag. Server-side JSONB column that persists
	 * pure UI toggles (hide-dotfiles, view mode, sidebar collapse, …)
	 * across devices. The server never inspects the contents — the SPA
	 * defines the keys (see `lib/stores/preferences.svelte.ts` for the
	 * typed view). Always an object on the wire (empty bag is `{}`,
	 * never `null` or missing).
	 *
	 * When PATCHing back to the server via
	 * `PATCH /api/auth/me/profile { ui_preferences: {...} }`, the
	 * server SHALLOW-merges — only the keys present in the patch are
	 * touched, so partial writes from one device don't clobber
	 * preferences set on another. Set a key to `null` in the patch to
	 * delete it from the bag.
	 */
	ui_preferences: Record<string, unknown>;
}

export interface AuthResponse {
	user: User;
	access_token: string;
	refresh_token: string;
	token_type: string;
	expires_in: number;
}

export type SortBy =
	| 'relevance'
	| 'name'
	| 'name_desc'
	| 'date'
	| 'date_desc'
	| 'size'
	| 'size_desc';

export interface SearchCriteria {
	sort_by: SortBy;
	recursive: boolean;
	limit: number;
	offset: number;
	name_contains?: string;
	file_types?: string[];
	folder_id?: string;
	min_size?: number;
	max_size?: number;
	created_before?: number;
	created_after?: number;
	modified_before?: number;
	modified_after?: number;
}

export interface SearchResults {
	files: FileItem[];
	folders: FolderItem[];
	total_count: number | null;
	limit: number;
	offset: number;
	has_more: boolean;
	query_time_ms: number;
	sort_by: string;
}

export type DriveKind = 'personal' | 'shared';

/** Role-keyed share strength. Matches `Role` in the backend authz model. */
export type DriveRole = 'owner' | 'editor' | 'contributor' | 'commenter' | 'viewer';

/** Subject of a grant. Mirrors `SubjectDto`. */
export type SubjectKind = 'user' | 'group' | 'token';
export interface DriveMemberSubject {
	type: SubjectKind;
	id: string;
}

/**
 * One row from `GET /api/drives`. Mirrors `DriveDto` in
 * `src/application/dtos/drive_dto.rs`. `default_for_user` is the caller's
 * id when present, `null`/undefined otherwise — used to pick the default
 * personal drive without hard-coding name conventions.
 *
 * `caller_role` is the strongest role the calling user holds on this drive
 * (direct + group-mediated, collapsed). Drives the permission-aware UI
 * gating on `/config/drive/<id>` and similar pages. `undefined` in
 * contexts where the caller is the granter rather than a member (e.g.
 * outgoing-grants listing).
 */
export interface Drive {
	id: string;
	name: string;
	kind: DriveKind;
	default_for_user?: string | null;
	root_folder_id: string;
	quota_bytes?: number | null;
	used_bytes: number;
	/**
	 * Drive policies — raw JSONB bag from the backend. Unknown keys are
	 * preserved verbatim. For the typed view used by the admin policy
	 * editor, see [`DrivePolicies`].
	 */
	policies: Record<string, unknown>;
	created_at: string;
	updated_at: string;
	caller_role?: DriveRole | null;
}

/**
 * Typed mirror of the known drive policy keys. Every field defaults to
 * `false` (= "opted out" for the `include_in_*` keys, "allowed" for the
 * `forbid_*` keys). The wire shape returned by
 * `PATCH /api/drives/{id}/policies` carries every known key; the request
 * body uses [`DrivePoliciesPartial`] so unsupplied keys aren't disturbed
 * (the backend uses a JSONB `||` merge — see
 * `drive_pg_repository.rs::update_policies`).
 *
 * See `docs/plan/drive.md` §8 for the `forbid_*` gates and §15 for the
 * `include_in_*_index` scope flags.
 */
export interface DrivePolicies {
	forbid_sharing: boolean;
	forbid_external_sharing: boolean;
	forbid_public_links: boolean;
	forbid_cross_drive_move: boolean;
	forbid_owner_role_change: boolean;
	/**
	 * §15 opt-in for `/api/photos` timeline scope. Default personal drives
	 * are created with `true`; non-default drives (secondary personals,
	 * shared) start `false` and opt in via the admin policy modal.
	 */
	include_in_photo_index: boolean;
	/**
	 * §15 opt-in for the Music library surface (currently playlists;
	 * future `/api/music/tracks` library view will read this too).
	 * Symmetric shape to `include_in_photo_index`.
	 */
	include_in_music_index: boolean;
}

/**
 * Body shape for the admin policy editor — every key optional so omitting
 * a field leaves that policy untouched (the backend uses a JSONB merge).
 */
export type DrivePoliciesPartial = Partial<DrivePolicies>;

/**
 * Request body for `POST /api/drives` (D3a). Mirrors `CreateDriveDto` in
 * `src/interfaces/api/handlers/drive_handler.rs`. `kind: 'personal'` is a
 * recognised wire shape but returns 501 today (the authz model + quota
 * source for secondary personals are still open product questions).
 */
export interface CreateDriveBody {
	kind: DriveKind;
	name: string;
	owner: DriveMemberSubject;
	quota_bytes?: number | null;
}

/**
 * One row from `GET /api/drives/{id}/members`. Mirrors `GrantDto` in
 * `src/application/dtos/grant_dto.rs` — the shape is the same as any
 * other role-grant; drive membership just constrains `resource.type` to
 * `"drive"`.
 */
export interface DriveMember {
	id: string;
	subject: DriveMemberSubject;
	resource: { type: 'drive'; id: string };
	role: DriveRole;
	granted_by: string;
	granted_at: string;
	expires_at?: string | null;
}
