/**
 * @typedef {'file' | 'folder'} ItemTypeEnum
 */

// FIXME to simplify
/**
 * @typedef {Object} LightItem
 * @property {string} id
 * @property {string} name
 * @property {ItemTypeEnum} type
 * @property {string} parentId
 */

//FIXME: rename into FolderItem
/**
 * @typedef {Object} FolderItem
 * @property {string} category (folder)
 * @property {number} created_at - timestamp
 * @property {string} icon_class
 * @property {string} icon_special_class
 * @property {string} id the uniq id of the folder
 * @property {boolean} is_root
 * @property {number} modified_at
 * @property {string} name
 * @property {string} owner_id
 * @property {string|null} parent_id the folder parent (null if is_root)
 * @property {string} path the full path
 * @property {string} etag opaque HTTP ETag, for If-Match / If-None-Match
 */

//FIXME: rename into FileItem
/**
 * @typedef {Object} FileItem
 * @property {string} category
 * @property {number} created_at - timestamp
 * @property {string} icon_class
 * @property {string} icon_special_class
 * @property {string} id the uniq id of the folder
 * @property {string} mime_type
 * @property {number} modified_at - timestamp
 * @property {string} name
 * @property {string} owner_id
 * @property {string} folder_id the folder parent
 * @property {string} path the full path
 * @property {number} size
 * @property {string} size_formatted
 * @property {number} sort_date
 * @property {string} etag opaque HTTP ETag, for If-Match / If-None-Match
 * @property {string} content_hash raw BLAKE3 content hash, for dedup checks
 * @property {string} [snippet] plain-text fragment around a content match (search results only)
 * @property {string} [match_source] "name" or "content" — how the search found this file
 */

/**
 * @typedef {Object} ShareItem
 * @property {number} access_count
 * @property {number} created_at - timestamp
 * @property {String} created_by
 * @property {number} expires_at - timestamp
 * @property {boolean} has_password
 * @property {string} id
 * @property {string} item_id
 * @property {string} item_name
 * @property {ItemTypeEnum} item_type
 * @property {string | null} token
 * @property {string} url
 */

/**
 * @typedef {Object} CreateShare
 * @property {string} item_id
 * @property {string|null} [item_name]
 * @property {ItemTypeEnum} item_type
 * @property {string|null} password
 * @property {number|null} expires_at - timestamp
 */

/**
 * @typedef {Object} UpdateShare
 * @property {string|null}  [password]
 * @property {number|null}  [expires_at]
 */

/**
 * @typedef {Object} FavoriteItem
 * @property {string} id
 * @property {string} user_id
 * @property {string} item_id /// ID of the favorited item (file or folder)
 * @property {ItemTypeEnum} item_type
 * @property {number} created_at
 * @property {string|null} item_name: null if folder
 * @property {number|null} item_size null if folder
 * @property {string|null} item_mime_type if file
 * @property {string|null} parent_id
 * @property {number|null} modified_at: Option<DateTime<Utc>>,
 * @property {String} item_path Full human-readable path (e.g. "Documents/Work" for a folder, "Documents/Work/report.pdf" for a file)
 * @property {String} icon_class
 * @property {String} icon_special_class
 * @property {String} category
 * @property {String} size_formatted
 * @property {string|null} owner_id UUID of the file/folder's actual owner
 */

/**
 * @typedef {Object} RecentItem
 * @property {string} id
 * @property {string} user_id
 * @property {string} item_id /// ID of the favorited item (file or folder)
 * @property {ItemTypeEnum} item_type
 * @property {number} accessed_at
 * @property {string|null} item_name: null if folder
 * @property {number|null} item_size null if folder
 * @property {string|null} item_mime_type if file
 * @property {string|null} parent_id
 * @property {String} item_path Full human-readable path (e.g. "Documents/Work" for a folder, "Documents/Work/report.pdf" for a file)
 * @property {String} icon_class
 * @property {String} icon_special_class
 * @property {String} category
 * @property {String} size_formatted
 */

/**
 * One item returned by `GET /api/trash/resources`.
 * `resource_type` discriminates the shape of `resource`.
 *
 * `deletion_date` is the real timestamp at which the retention sweeper will
 * permanently delete the item (= trashed_at + retention_days). Days remaining
 * is derived client-side from `deletion_date` and the current clock — it is
 * not duplicated in the wire format.
 *
 * `resource.path` carries the item's original location (soft-delete preserves
 * the row's `path` column).
 *
 * @typedef {Object} TrashResourceItem
 * @property {ResourceTypeEnum}    resource_type  - 'file' | 'folder'
 * @property {string}              trashed_at     - ISO-8601: when the user sent it to trash.
 * @property {string}              deletion_date  - ISO-8601: when retention will purge it.
 * @property {FileItem|FolderItem} resource       - Full resource details; shape follows resource_type.
 */

/**
 * Response for `GET /api/trash/resources`.
 * @typedef {Object} TrashResourcesResponse
 * @property {TrashResourceItem[]}  items
 * @property {string|undefined}     [next_cursor]  - Absent when the last page is reached.
 */

/**
 * Wire shape of `UserDto` (backend: `src/application/dtos/user_dto.rs`).
 * Returned by `/api/auth/me`, `/api/users/{id}`, the login response, and
 * the admin user-management endpoints. Optional fields (`image`,
 * `given_name`, `family_name`) are omitted when null on the wire — the
 * `?` markers below reflect that.
 *
 * @typedef {Object} User
 * @property {string} id
 * @property {string} [username]   Optional handle (PR 16); claim-once via /api/auth/me/profile (PR 24). Omitted from JSON when null.
 * @property {string} email
 * @property {string} role
 * @property {number} storage_quota_bytes
 * @property {number} storage_used_bytes
 * @property {string} created_at  ISO 8601 timestamp
 * @property {string} updated_at  ISO 8601 timestamp
 * @property {string|null} [last_login_at]  ISO 8601 timestamp; null until first login
 * @property {boolean} active
 * @property {string}  auth_provider  "local" or OIDC provider id
 * @property {string|null} [image]    Avatar URL or data URI
 * @property {boolean} can_edit_image  False for OIDC-only users
 * @property {boolean} is_external    True for magic-link / OIDC-only / OCM recipients
 * @property {string} [given_name]    First/given name; set at OIDC JIT or via PATCH /api/auth/me/profile (PR 24)
 * @property {string} [family_name]   Last/family name; set at OIDC JIT or via PATCH /api/auth/me/profile (PR 24)
 * @property {string} [email_verified_at]  ISO 8601 timestamp of the first proof-of-email-control (PR 23). Omitted when unverified.
 * @property {string} [preferred_locale]    User-chosen locale code (e.g. `"fr"`, `"zh-TW"`); omitted when unset. Round-trips via PATCH /api/auth/me/profile.
 * @property {boolean} notify_on_share       Whether the user wants share-notification emails ("Alice shared X with you"). Default TRUE. Toggled via the profile checkbox; round-trips via PATCH /api/auth/me/profile.
 */

/**
 * @typedef {Object} AuthResponse
 * @property {User} user
 * @property {String} access_token
 * @property {String} refresh_token
 * @property {String} token_type
 * @property {number} expires_in
 */

/**
 * @typedef {'user' | 'admin'} RoleEnum
 */

/**
 * @typedef {"relevance" | "name" | "name_desc" | "date" | "date_desc" | "size" | "size_desc"} SortByEnnum
 */

/**
 * @typedef {Object} SearchCriteria
 * @property {SortByEnnum} sort_by
 * @property {boolean} recursive
 * @property {number} limit
 * @property {number} offset
 *
 * @property {String} [name_contains]
 * @property {String[]} [file_types] pdf, jpg, ...
 * @property {String} [folder_id]
 *
 *
 * @property {number} [min_size]
 * @property {number} [max_size]
 *
 * @property {number} [created_before]
 * @property {number} [created_after]
 *
 * @property {number} [modified_before]
 * @property {number} [modified_after]
 */

/**
 * @typedef {Object} SearchResults
 * FIXME: is in fact Vec<SearchFileResultDto>,
 * @property {FileItem[]} files
 * FIXME: is infact Vec<SearchFolderResultDto>,
 * @property {FolderItem[]} folders:
 * @property {number | null} total_count
 * @property {number} limit
 * @property {number} offset
 * @property {boolean} has_more
 * @property {number} query_time_ms
 * @property {string} sort_by
 */

/**
 * @typedef {Object} Playlist
 * @property {String} id
 * @property {String} name
 * @property {String | null} description
 * @property {String} owner_id
 * @property {boolean} is_public
 * @property {String | null} cover_file_id
 * @property {number} track_count
 * @property {number} total_duration_secs
 * @property {number} created_at
 * @property {number} updated_at
 */

/**
 * @typedef {Object} PlaylistItem
 * @property {String} id
 * @property {String} playlist_id
 * @property {String} file_id
 * @property {number} position
 * @property {number} added_at
 * @property {String|null} file_name
 * @property {number|null} file_size
 * @property {String|null} mime_type
 * @property {String|null} title
 * @property {String|null} artist
 * @property {String|null} album
 * @property {number|null} duration_secs
 */

/**
 * @typedef {Object} Musicshare
 * @property {String} user_id
 * @property {boolean|null} can_write
 */

/**
 * @typedef {Object} FileMetadata
 * @property {String} file_id
 * @property {number} captured_at
 * @property {number|null} latitude
 * @property {number|null} longitude
 * @property {String|null} camera_make
 * @property {String|null} camera_model
 * @property {number|null} orientation
 * @property {number|null} width
 * @property {number|null} height
 */

// ------------------- grants

/**
 * @typedef {'read'|'create'|'share'|'comment'|'delete'|'update'} PermissionTypeEnum
 */

/**
 * @typedef {'folder'|'file'} ResourceTypeEnum
 */

/**
 * @typedef {Object} Resource
 * @property {ResourceTypeEnum} type
 * @property {String} id
 */

/**
 * @typedef {'user'|'group'|'token'|'external'} SubjectTypeEnum
 */

/**
 * @typedef {Object} Subject
 * @property {SubjectTypeEnum} type
 * @property {String} id
 */

/**
 * @typedef {Object} Grant
 * @property {string} id
 * @property {string} granted_at  - ISO-8601 datetime string.
 * @property {string} granted_by
 * @property {Subject} subject
 * @property {PermissionTypeEnum} permission
 * @property {Resource} resource
 * @property {string|null} [expires_at]  - ISO-8601 datetime string, or absent/null for no expiry.
 */

/**
 * Roles: `viewer`, `commenter`, `editor`, `manager`, `admin`
 */

/**
 * Configuration for `ResourceListComponent`.
 * @typedef {Object} ResourceListConfig
 * @property {boolean}  [selectable=true]      - Show per-item checkboxes and enable selection.
 * @property {boolean}  [showFavorite=true]    - Show the favorite-star button on each item.
 * @property {boolean}  [showOwner=false]      - Show the owner column initially.
 * @property {boolean}  [showShareBadge=true]  - Show the shared-resource badge on items.
 * @property {boolean}  [draggable=false]      - Mark items as draggable.
 * @property {boolean}  [showContextMenu=true] - Enable the three-dots button and right-click menu.
 * @property {string}   [itemModifierClass]    - Extra CSS class on every .file-item (e.g. 'favorite-item').
 * @property {string}   [dateField='modified_at'] - Which date field to display in the date column.
 * @property {string}   [dateLabel]            - Column header label for the date column.
 * @property {(id: string, type: 'file'|'folder') => boolean} [isFavorite]  - State provider for favorite badge.
 * @property {(id: string, type: 'file'|'folder') => boolean} [isShared]    - State provider for share badge.
 * @property {(item: FileItem|FolderItem, event: MouseEvent) => void} [onOpen]           - Item open/navigate callback.
 * @property {(item: FileItem|FolderItem) => Promise<void>}           [onFavoriteToggle] - Favorite-star click callback.
 * @property {(item: FileItem|FolderItem, event: MouseEvent) => void} [onContextMenu]    - Context menu callback.
 * @property {(selected: Array<FileItem|FolderItem>) => void}         [onSelectionChange] - Selection change callback.
 */

/**
 * One item returned by `GET /api/grants/incoming/resources`.
 * `resource_type` discriminates the shape of `resource`.
 * @typedef {Object} SharedWithMeItem
 * @property {ResourceTypeEnum}        resource_type
 * @property {PermissionTypeEnum[]}    permissions   - All permissions the caller holds on this resource.
 * @property {string}                  granted_at    - ISO-8601 timestamp of the earliest grant.
 * @property {string}                  granted_by    - UUID of the user who created the grant.
 * @property {FileItem|FolderItem}     resource      - Full resource details; shape follows resource_type.
 */

/**
 * Response for `GET /api/grants/incoming/resources`.
 * @typedef {Object} SharedWithMeResponse
 * @property {SharedWithMeItem[]}  items
 * @property {string|undefined}    [next_cursor]  - Absent when the last page is reached.
 */

/**
 * One (subject, permissions) entry within an outgoing resource item.
 * Mirrors the server's `OutgoingResourceGrantDto`. `subject_type` is the
 * full set the backend may emit; the UI for My Shares filters out `'group'`
 * before rendering (see `_excludeGroupGrants` in mySharesList.js) so only
 * `'user'` and `'token'` rows actually reach the view layer there.
 *
 * @typedef {Object} OutgoingResourceGrant
 * @property {string}                              grant_id
 * @property {'user'|'group'|'token'|'external'}   subject_type
 * @property {string}                              subject_id
 * @property {string}                              subject_display - Username (users) or share name (tokens).
 * @property {'viewer'|'editor'|'admin'}           role
 * @property {string}                              granted_at   - ISO-8601
 * @property {string|null}                         [expires_at] - ISO-8601 or absent.
 * @property {boolean}                             has_password - True when a token subject has a password set.
 * @property {boolean}                             [is_external] - True when a user subject is a magic-link-only external user (PR N2). Drives the My Shares menu label ("Resend invitation email" vs "Notify by email"). Always false for token and group subjects.
 */

/**
 * One item returned by `GET /api/grants/outgoing/resources`.
 * @typedef {Object} OutgoingResourceItem
 * @property {ResourceTypeEnum}          resource_type
 * @property {string}                    first_shared_at  - ISO-8601 earliest grant date.
 * @property {FileItem|FolderItem}       resource         - Full resource details.
 * @property {OutgoingResourceGrant[]}   grants           - One entry per (subject, permissions).
 */

/**
 * Response for `GET /api/grants/outgoing/resources`.
 * @typedef {Object} OutgoingResourcesResponse
 * @property {OutgoingResourceItem[]}  items
 * @property {string|undefined}        [next_cursor]  - Absent when the last page is reached.
 */

/**
 * One item returned by `GET /api/favorites/resources`.
 * `resource_type` discriminates the shape of `resource`.
 * @typedef {Object} FavoritesResourceItem
 * @property {ResourceTypeEnum}    resource_type  - 'file' | 'folder'
 * @property {string}              favorited_at   - ISO-8601 timestamp when the item was starred.
 * @property {FileItem|FolderItem} resource       - Full resource details; shape follows resource_type.
 */

/**
 * Response for `GET /api/favorites/resources`.
 * @typedef {Object} FavoritesResourcesResponse
 * @property {FavoritesResourceItem[]}  items
 * @property {string|undefined}         [next_cursor]  - Absent when the last page is reached.
 */

/**
 * @typedef {Object} ContactEmail
 * @property {string}  email
 * @property {string}  type        - e.g. "work", "home"
 * @property {boolean} is_primary
 */

/**
 * Mirrors the backend `ContactDto`.
 * `id` equals the OxiCloud user UUID for contacts from the system address book.
 * @typedef {Object} ContactItem
 * @property {string}          id
 * @property {string}          address_book_id
 * @property {string}          uid             - vCard UID
 * @property {string|null}     [full_name]
 * @property {string|null}     [first_name]
 * @property {string|null}     [last_name]
 * @property {string|null}     [nickname]
 * @property {ContactEmail[]}  email
 * @property {string|null}     [organization]
 * @property {string|null}     [title]
 * @property {string|null}     [photo_url]
 * @property {string}          created_at      - ISO-8601
 * @property {string}          updated_at      - ISO-8601
 * @property {string}          etag
 * @property {'user'|'group'}  [_kind]         - Discriminator added by the share-modal autocomplete when merging contacts with ReBAC subject groups. Absent (or 'user') for plain contacts; 'group' indicates the row is a subject-group suggestion with a `name` field instead of contact details.
 * @property {string}          [name]          - Present only when `_kind === 'group'` — the subject-group's display name.
 */

/**
 * Mirrors the backend `AddressBookResponse`.
 * @typedef {Object} AddressBookItem
 * @property {string}       id
 * @property {string}       name
 * @property {string}       owner_id
 * @property {string|null}  [description]
 * @property {string|null}  [color]
 * @property {boolean}      is_public
 * @property {boolean}      is_readonly
 * @property {boolean}      is_system
 * @property {string}       created_at   - ISO-8601
 * @property {string}       updated_at   - ISO-8601
 */

// ------------------- share modal

/**
 * Share roles (DTO-layer sugar for the ReBAC permission sets).
 * @typedef {'viewer'|'editor'|'admin'} ShareRoleEnum
 */

/**
 * One collaborator row in the share modal's People section.
 * @typedef {Object} MemberEntry
 * @property {Grant}         grant   - Representative grant (used for subject/resource info).
 * @property {Grant[]}       _grants - All grants for this subject on the resource (may be > 1).
 * @property {ShareRoleEnum} role    - Derived role label shown in the UI.
 * @property {'keep'|'remove'|'change'|'new'} _op - Pending local operation.
 * @property {string|null}  [expires_at]  - YYYY-MM-DD expiry date string, or null for no expiry.
 * @property {string}       [_displayName] - Optional human-readable label (set for group subjects so the row can show the group name; user subjects resolve their name via `createUserVignette`).
 * @property {boolean}      [_isVirtual]  - True when this row's subject is a virtual (system-managed) group, so the vignette renders with the virtual-group icon.
 * @property {string}       [_invitedEmail] - Transient marker for an
 *   email-invite that hasn't been committed yet. When set, the row
 *   renders with `pendingEmailVignette` (no UUID known) and
 *   `_applyAll` POSTs `subject.type=email` to `/api/grants`. Cleared
 *   on the next `fetchOutgoingGrants` refresh once the server has
 *   resolved the recipient to a real user UUID.
 */

/**
 * Existing public link with a pending local operation.
 * @typedef {Object} LinkEntry
 * @property {ShareItem}  share   - The existing share object.
 * @property {'keep'|'remove'|'edit'} _op - Pending local operation.
 * @property {DraftLink|null} _draft - Updated fields when _op === 'edit'.
 */

/**
 * A public link staged for creation (not yet committed).
 * @typedef {Object} DraftLink
 * @property {string}      name
 * @property {string|null} password
 * @property {string|null} expires_at  - ISO-8601 date string or null.
 */

// ------------------- ReBAC subject groups

/**
 * Mirrors `GroupDto` on the server (`subject_group_handler.rs::GroupDto`).
 * @typedef {Object} GroupItem
 * @property {string}        id
 * @property {string}        name
 * @property {string|null}   [description]
 * @property {boolean}       is_virtual
 * @property {string}        created_at   - ISO-8601
 * @property {string}        updated_at   - ISO-8601
 * @property {boolean}       can_manage   - True if the current caller may rename / delete / curate the membership.
 * @property {number}        member_count - Direct-member count (users + nested groups, one level). The
 *                                          `/groups/search` endpoint emits 0 to skip a per-row COUNT(*);
 *                                          list/get/create/update return the real value.
 */

/**
 * Response from `GET /api/groups` — paginated list of groups.
 * @typedef {Object} GroupListResponse
 * @property {GroupItem[]} items
 * @property {number}      total
 */

/**
 * One direct member of a group (tagged union: user or nested group).
 * @typedef {{kind: 'user', id: string} | {kind: 'group', id: string}} GroupMemberItem
 */

// ------------------- Instant upload (dedup)

/**
 * Response from `GET /api/dedup/check/{hash}` — user-scoped: `exists` only
 * reflects content the CALLER already owns, never global existence.
 * Mirrors `HashCheckResponse` on the server (`dedup_handler.rs`).
 * @typedef {Object} HashCheckAnswer
 * @property {boolean} exists
 * @property {string} hash BLAKE3 echoed back (64 hex chars)
 * @property {number} [existing_size] size in bytes, present when `exists`
 */

/**
 * Request body for `POST /api/files/by-hash` (instant upload — registers a
 * file from an already-owned blob, zero content bytes on the wire).
 * Mirrors `CreateFileByHashRequest` on the server (`file_handler.rs`).
 * The 201 response body is a {@link FileItem}.
 * @typedef {Object} CreateFileByHash
 * @property {string} name file name to create (basename only)
 * @property {string} folder_id target folder (caller needs Create on it)
 * @property {string} hash BLAKE3 of the owned content (64 hex chars)
 */

// ------------------- Delta sync (chunk negotiation)

/**
 * One chunk reference on the delta wire: terse on purpose (a 10 GB file
 * is ~40 000 of these). Mirrors `ChunkRef` on the server
 * (`delta_upload_service.rs`).
 * @typedef {Object} DeltaChunkRef
 * @property {string} h BLAKE3 of the chunk (64 hex chars)
 * @property {number} s chunk size in bytes (1 ..= 1 MiB)
 */

/**
 * Response of `POST /api/files/delta/negotiate` — the distinct chunk
 * hashes the caller must upload (user-scoped, advisory).
 * @typedef {Object} DeltaNegotiateAnswer
 * @property {string[]} missing
 */

/**
 * Request body of `POST /api/files/delta/commit`. Exactly one of
 * (`name` + `folder_id`) or `file_id` selects create vs update mode.
 * 201/200 responses carry a {@link FileItem}; 409 carries
 * `{still_missing: string[]}` (upload those chunks and retry).
 * @typedef {Object} DeltaCommitRequest
 * @property {string} file_hash BLAKE3 of the whole file (verified server-side)
 * @property {DeltaChunkRef[]} chunks full sequence, in file order
 * @property {string} [name] create mode: file name
 * @property {string} [folder_id] create mode: target folder
 * @property {string} [file_id] update mode: file whose content is replaced
 */

/**
 * Response of `GET /api/files/{id}/manifest` — the recipe to rebuild a
 * file from chunks (delta download, step 1). Immutable per `file_hash`;
 * the endpoint serves it with `ETag: file_hash` so polling sync clients
 * revalidate with a 304 for free.
 * @typedef {Object} DeltaManifestAnswer
 * @property {string} file_hash BLAKE3 of the whole file
 * @property {number} total_size bytes
 * @property {DeltaChunkRef[]} chunks full sequence, in file order
 */

/**
 * Request body of `POST /api/files/delta/download` (delta download,
 * step 2). Responds with `[u32 BE length][bytes]` frames in request
 * order, or 404 `{not_available: string[]}` for chunks outside the
 * caller's files.
 * @typedef {Object} DeltaDownloadRequest
 * @property {string[]} hashes distinct chunk hashes to fetch
 */

