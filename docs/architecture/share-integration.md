# Share Integration

OxiCloud supports public file and folder sharing through signed share links. A share can be public, password-protected, or time-limited.

> **Where the role and expiration live now.** Both the granted role and the expiration timestamp are stored on the `storage.role_grants` row that represents the share, not on the share row itself. They are evaluated by the same `AuthorizationEngine` that handles user and group grants — see [ReBAC Authorization](/architecture/rebac-authorization). The `storage.shares` row keeps only the token-side metadata (public token, password hash, item name, access count).

> **Sharing with people who do not yet have an account.** Token-based shares are anonymous; anyone with the URL can use them. To share with a specific person who isn't on the instance yet, the share modal accepts a raw email address and provisions the recipient as an *external user* on the fly. That flow is described in [Magic-link external authentication](/architecture/magic-link-auth), and the resulting grant is a regular per-user `role_grants` row — identical in evaluation to a grant on an internal recipient.

## What a Share Contains

A share record (`storage.shares`) tracks:

- The shared item ID and whether it is a file or folder
- A public token used in the share URL
- Optional password protection (hash only — never plaintext)
- The creator and access count

What used to live on the share row but is now resolved through ReBAC:

- **Expiration** → `role_grants.expires_at`. The cascade query filters expired grants inline (`expires_at IS NULL OR expires_at > NOW()`), so an expired share fails the same path a revoked user grant fails. No separate "is this share expired" check.
- **Role scope** → `role_grants.role` (Postgres ENUM `storage.grant_role`). **For security, public share-link grants are always `viewer`** and cannot be raised. Anyone holding the token can view but not modify, comment, share, or delete. To grant write or share access to a specific recipient, create a per-user or per-group grant with a higher role (`editor`, `contributor`, `owner`) instead of a share link.

## Public and Private Routes

### Authenticated management routes

| Method | Path | Description |
| --- | --- | --- |
| `POST` | `/api/shares/` | Create a new share |
| `GET` | `/api/shares/` | List current user's shares |
| `GET` | `/api/shares/{id}` | Fetch one share |
| `PUT` | `/api/shares/{id}` | Update permissions, password, or expiration |
| `DELETE` | `/api/shares/{id}` | Delete a share |

### Public access routes

| Method | Path | Description |
| --- | --- | --- |
| `GET` | `/api/s/{token}` | Access a shared item |
| `POST` | `/api/s/{token}/verify` | Verify a password-protected share |

## Service Responsibilities

The share service is responsible for:

- Validating that the underlying file or folder exists
- Generating unique share IDs and public tokens
- Enforcing password checks and expiration rules
- Mapping domain permissions into API DTOs
- Recording access counts

Share metadata is persisted separately from the file content itself. The shared resource still uses the normal storage model for files and folders.

## Example Workflow

### Creating a share link

1. A user selects a file or folder in the UI
2. The frontend submits a request to `/api/shares/`
3. OxiCloud validates the target and requested permissions
4. The backend generates a token and public URL
5. The share metadata is saved and returned to the caller

### Opening a share link

1. A guest opens `/api/s/{token}`
2. OxiCloud verifies the token and checks expiration
3. If the share is password protected, the client verifies the password first
4. Access is counted and the shared resource is returned according to the granted permissions

## Lifecycle & cleanup

Because the role and expiry live on `role_grants`, every share is represented by two correlated rows: one in `storage.shares` (token metadata) and one in `storage.role_grants` with `subject_type='token'` and `subject_id=share.id` carrying the `viewer` role. Two triggers keep them in sync — one per direction — so neither side can outlive the other.

### Share deletion → grant cleanup

Deleting a share row (`DELETE FROM storage.shares` via `DELETE /api/shares/{id}`) fires the token-side cleanup trigger. It removes the matching `role_grants` row whose `subject_type='token'` and `subject_id=share.id`, in the same transaction. The token becomes unreachable immediately — no stale grant left behind.

The same pattern runs when the underlying resource is deleted: the per-resource-type triggers on `role_grants` (`trg_cleanup_role_grants_folder`, `trg_cleanup_role_grants_file`, `trg_cleanup_role_grants_drive`, `_calendar`, `_address_book`, `_playlist`) clean up the grants, and any share row referencing a deleted resource is then garbage-collected by the reverse trigger described below.

### Grant revocation → share row cleanup

`DELETE /api/grants/{grant_id}` on a token row removes the matching `storage.shares` row, atomically and in the same transaction. The `trg_cleanup_share_on_grant_delete` trigger (originally introduced in `migrations/20260612000001_share_grant_reverse_cascade.sql`, carried forward through the `role_grants` migration by `migrations/20260801000001_role_grants_cascade_triggers.sql`) watches `role_grants` for `DELETE` events with `subject_type='token'` and deletes the paired share row **iff no other grants for the same `subject_id` still exist**:

```sql
AFTER DELETE ON storage.role_grants:
    IF OLD.subject_type = 'token' THEN
        DELETE FROM storage.shares
         WHERE id = OLD.subject_id
           AND NOT EXISTS (SELECT 1 FROM storage.role_grants
                            WHERE subject_type = 'token'
                              AND subject_id   = OLD.subject_id);
```

The `NOT EXISTS` guard makes it safe in two important cases:

- **Multi-role tokens** — the schema doesn't currently allow more than one role on a token (public share-links are always `viewer`), but the guard is still correct for the general case. Reserved for a future extension where a token might carry multiple assignments.
- **Forward-cascade re-entry** — when the original DELETE comes from `storage.shares`, the forward trigger is already deleting the corresponding `role_grants` row. The reverse trigger then tries to delete a share row that's already gone, finds no row, and the statement is a no-op. No recursion.

Net effect: revoking the grant on a token via the grants API and deleting the share via `DELETE /api/shares/{id}` are equivalent — both end in a clean state with zero rows on either side.

### Resource deletion

Both triggers compose cleanly with resource lifecycle:

- A folder/file/drive delete → per-resource-type `trg_cleanup_role_grants_*` removes the grants → `trg_cleanup_share_on_grant_delete` removes the share rows that just lost their last grant. One delete on the resource cleans up everything downstream in a single transaction.

### Expired shares — background purge

Public shares with an expiration date follow the general expired-grant
lifecycle: the AuthZ engine treats them as unusable the moment `expires_at`
passes (inline filter, no separate expiry check), and the `GrantCleanupService`
daemon physically deletes the underlying `role_grants` row after a grace
window (default 15 days, `OXICLOUD_GRANT_CLEANUP_GRACE_DAYS`). When it does,
the reverse trigger described above fires and reaps the paired `storage.shares`
row in the same transaction. Expired public shares vanish end-to-end without
operator intervention. See
[ReBAC Authorization → Post-expiry cleanup](/architecture/rebac-authorization#post-expiry-cleanup)
for the daemon and its env vars.

### Pre-existing orphans

The `20260612000001` migration ran a one-shot `DELETE FROM storage.shares WHERE NOT EXISTS (… token grants)` to garbage-collect any orphans that accumulated before the reverse trigger existed. The `role_grants` migration path preserved that cleanup — no fresh orphan class was introduced.

## Security Notes

- Passwords are stored as hashes, never as plaintext
- Expired shares are rejected before content access by the engine's inline `expires_at` check — no separate code path
- Permissions are checked per action by the `AuthorizationEngine`, not just when the share is created. Revoking a grant takes effect immediately (subject to the 30 s group-expansion cache for user-grant checks; token-grant checks have no cache layer)
- Public share grants are server-side restricted to `read` regardless of what the request asked for — see the rebac-authorization doc for the role-to-permissions expansion

## Related Pages

- [ReBAC Authorization](/architecture/rebac-authorization) — how grants, permissions, expiry, and cascades work
- [OIDC / SSO](/config/oidc)
- [Admin Settings](/config/admin-settings)
- [Internal Architecture](/architecture/)