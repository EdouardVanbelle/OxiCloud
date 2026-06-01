# User Lifecycle Hooks

Observer pattern for per-service reactions to user state transitions: created, login, logout, deleted. Mirrors the [File and Blob lifecycle](/architecture/file-and-blob-lifecycle) pattern for files; deliberately diverges from it on two points (async + sync await for most events) because user-lifecycle work is rare and sometimes has hard synchronisation requirements.

## Why hooks

Before this work, four code paths in `AuthApplicationService` each called `create_personal_folder()` immediately after inserting an `auth.users` row (public registration, first-admin bootstrap, admin-creates-user, OIDC just-in-time provisioning), plus a fifth self-heal at `folder_service.rs` for users whose folder somehow went missing. **Five places, one concern, no shared abstraction.** Adding a future per-user resource — default calendar, address book, GPG keyring, external-identity provenance for the upcoming magic-link feature — would have meant touching all five.

Hooks fix this once. Each domain service implements `UserLifecycleHook` for the events it cares about; the dispatcher fires events; services that don't care declare explicit `Ok(())` no-ops. New services register a hook in DI and inherit all four events for free.

## The trait

```rust
// src/application/ports/user_lifecycle.rs
#[async_trait]
pub trait UserLifecycleHook: Send + Sync {
    fn name(&self) -> &'static str;

    async fn on_user_created(&self, user: &User)
        -> Result<(), DomainError>;

    async fn on_user_login(&self, user: &User)
        -> Result<(), DomainError>;

    async fn on_user_logout(&self, user: &User, reason: LogoutReason)
        -> Result<(), DomainError>;

    async fn on_user_deleted(&self, user: &User, mode: DeletionMode)
        -> Result<(), DomainError>;
}
```

Two enums frame the trait:

```rust
pub enum LogoutReason {
    UserInitiated,    // explicit logout
    SessionExpired,   // TTL hit
    AdminRevoked,     // admin-initiated single-session revoke
    AccountDisabled,  // user.active flipped to FALSE → sessions revoked
    PasswordChanged,  // sibling sessions invalidated by password change
    TokenReused,      // session-family reuse detection
}

pub enum DeletionMode {
    AdminDelete,      // admin deletes via UI; resources go to trash
    GdprPurge,        // GDPR right-to-erasure; hard-delete everything
}
```

**No default impls.** Every implementor must declare all four methods explicitly. Use `Ok(())` for events you don't care about. This forces conscious acknowledgement of every lifecycle event rather than silent inheritance — same convention as `FileLifecycleHook`.

## Dispatcher semantics

`UserLifecycleService` aggregates registered hooks and fans out events with **per-event failure semantics**. The trait itself is uniform; the dispatcher decides whether to await, whether to spawn, and whether `Err` aborts.

| Event              | Awaited?      | On `Err`                                |
|--------------------|---------------|-----------------------------------------|
| `on_user_created`  | yes (sync)    | log-and-continue (retry on next login)  |
| `on_user_login`    | yes (sync)    | log-and-continue (idempotent retry)     |
| `on_user_logout`   | no (spawned)  | logged, never propagated                |
| `on_user_deleted`  | yes (sync)    | log-and-continue today; PR 4 makes it abort-the-transaction |

The asymmetry is deliberate. `on_user_created` and `on_user_login` must complete before the session token is returned, so callers see consistent state. `on_user_logout` is bookkeeping; the HTTP response shouldn't wait for cache flushes — the dispatcher spawns. `on_user_deleted` will become atomic-with-the-DELETE in PR 4 when a transaction handle joins the trait signature.

```text
            ┌──────────────────────────────────────────────────┐
            │ AuthApplicationService                           │
            │   register / login / logout / delete             │
            └──────────────────────┬───────────────────────────┘
                                   │
                                   ▼
            ┌──────────────────────────────────────────────────┐
            │ UserLifecycleService::dispatch_*                 │
            │   created / login / logout / deleted             │
            └──────────────────────┬───────────────────────────┘
                                   │
                  ┌────────────────┼────────────────┐
                  ▼                ▼                ▼
         AuditLifecycleHook  HomeFolderHook  AuthzCacheHook   …
            (PR 1 only)        (PR 3)           (PR 4)
```

## Owner-located convention

Each concrete hook impl lives **next to the service that owns the work**, not in a centralised `lifecycle/` directory.

Examples (PR plan):

- `HomeFolderLifecycleHook` lives in `src/application/services/folder_service.rs` — same module as `FolderService`, owner of home-folder policy.
- `AuthzCacheLifecycleHook` lives in `src/infrastructure/services/pg_acl_engine.rs` — same module as the Moka cache it invalidates.
- `AuditLifecycleHook` lives in `src/application/services/user_lifecycle_service.rs` (with the dispatcher) — cross-cutting, no domain owner.

This mirrors how `FileLifecycleHook` impls are placed: `ThumbnailRefreshHook` lives in `thumbnail_service.rs`, the audio metadata impl lives in `audio_metadata_service.rs`. A future maintainer reading the folder service sees the lifecycle reactions next to the rest of the folder logic — no jumping between modules to understand why a folder gets created on login.

## Tips for implementors

These are codified in the module-level docstring of `application/ports/user_lifecycle.rs` so they show up in IDE hover.

1. **First-ever login detection.** `on_user_login` fires *before* `user.register_login()` is called, so `user.last_login_at().is_none()` is a reliable "this is the user's first login since account creation" signal. Use it for welcome emails, one-shot default-resource seeding, "complete your profile" prompts.

2. **External-user short-circuit.** Hooks that provision per-user resources (home folder, default calendar, address book, GPG keys, …) must start with `if user.is_external() { return Ok(()); }`. External users (`is_external = TRUE`) are grant-only recipients — they have no home folder and don't consume storage quota. The DB `CHECK (NOT is_external OR storage_used_bytes = 0)` constraint catches code paths that bypass this short-circuit.

   **Subtle but important rule**: external users can **never** be admins. The DB enforces this via `CHECK (NOT (is_external AND role = 'admin'))`. `User::new_external(...)` doesn't accept a role parameter — it always sets `UserRole::User`. To make an existing external user an admin, an admin must first convert them to internal (`UPDATE auth.users SET is_external = FALSE`) and *then* update the role. The two-step process is intentional friction: granting admin to a federated principal would let external identity providers indirectly manage the local instance.

3. **Idempotency is mandatory.** `on_user_login` fires on every successful authentication, not just the first. A hook that creates a resource must check whether the resource already exists before creating it. Cache invalidation, audit deduplication, etc., must all tolerate redundant calls.

4. **External → internal conversion needs no special event.** When an admin flips `is_external = FALSE`, the user's next login fires `on_user_login` with the new flag value. Idempotent hooks see `!is_external` and missing resources → provision. No `on_user_converted` method needed; the safety-net pattern carries the load.

3. **Failure swallowing on create/login.** If your hook returns `Err`, the user is still created/logged in; only your hook's effect is delayed. Log enough detail via `tracing::error!` that subsequent investigation can identify the user. The next successful login's `on_user_login` will retry idempotently.

4. **Per-session logout firing.** When a flow revokes multiple sessions in one call (e.g. `revoke_all_user_sessions` on password change), today the dispatcher fires `on_user_logout` ONCE per logical revoke-call. PR 4's `SessionRevocationLifecycleHook` will refine to once-per-session for proper audit granularity. Hooks must accept N redundant calls with the same reason — keep them idempotent.

5. **`on_user_deleted` is post-commit today.** The user row is already gone when the hook fires. Returning `Err` cannot roll back. PR 4 refactors `delete_user_admin` to expose a transaction handle, at which point the trait gains `tx: &mut Transaction` and `Err` will abort the delete.

6. **Hook order is registration order.** The DI factory determines firing sequence. If two hooks have an ordering dependency (e.g. home-folder must exist before default-calendar can be seeded inside it), the dependent hook registers AFTER the producer. Document the convention inline in the DI block.

## Concrete hooks shipped today

| Hook | Lives in | Responsibility |
|---|---|---|
| `AuditLifecycleHook` | `src/application/services/user_lifecycle_service.rs` (co-located with dispatcher) | All four events: emits one `tracing::info!(target: "audit", event = "user.*", ...)` per call. Co-located because audit is cross-cutting with no domain owner. |

Subsequent PRs add:

- `HomeFolderLifecycleHook` (PR 3) — owns home-folder provisioning; replaces the four eager `create_personal_folder` calls + the self-heal.
- `AuthzCacheLifecycleHook` (PR 4) — invalidates the `user_groups_cache` Moka entry on logout/delete.
- `SessionRevocationLifecycleHook` (PR 4) — refines per-session logout granularity; explicit session revocation inside the user-delete transaction.
- `ExternalIdentityLifecycleHook` (PR 5, stub for now) — populated by the upcoming magic-link external-user feature.

## Future events (NOT shipped — design door)

These events are reserved for situations that don't exist yet but probably will. Adding a method to the trait costs every hook impl a new no-op forever, so we don't add them speculatively. Each row lists what would force the addition.

| Future event | Why someone might want it | What would force adding it |
|---|---|---|
| `on_user_password_changed` | Notify the user via email; invalidate cached credentials; trigger TOTP re-enrolment | A per-user notification service. Today the existing `revoke_all_user_sessions` cascade fires `on_user_logout(PasswordChanged)` for each session — sufficient for current consumers. |
| `on_user_role_changed` | Audit promotion to admin; revoke admin-only sessions on demotion | A multi-role system. Today only `admin` / `user` exist and the one-liner audit log at the admin handler covers it. |
| `on_user_email_changed` | External users: re-verify the new email via magic-link; notify both old and new addresses | When external users start changing their email. Today email is immutable. |
| `on_user_avatar_changed` | Bust thumbnail caches; sync to federated servers (OCM) | When OCM federation ships. |
| `on_user_disabled` / `on_user_enabled` | Audit-distinguishable state changes; pause per-user scheduled jobs | When per-user scheduled jobs land. Today `on_user_logout(AccountDisabled)` covers the only consumer. |
| `on_user_external_to_internal_converted` | Welcome email; pre-provision internal-only resources at conversion time | If admins routinely promote external users and the next-login lag is unacceptable. Today idempotent `on_user_login` handles conversion fine. |
| `on_user_2fa_enabled` / `on_user_2fa_disabled` | Audit; force re-login of other sessions | When 2FA ships. |

**Rule of thumb for adding any of these**: pair the addition with a default `Ok(())` body (one-time exception to the "no defaults" rule) so existing hooks don't need to declare it. State in the docstring whether the event is await-or-spawn and whether `Err` aborts.

## File map

| Concern | Module |
|---|---|
| Trait + `LogoutReason` + `DeletionMode` enums + tips | `src/application/ports/user_lifecycle.rs` |
| Dispatcher + `AuditLifecycleHook` | `src/application/services/user_lifecycle_service.rs` |
| Wire-in: created / login / logout / deleted | `src/application/services/auth_application_service.rs` |
| DI registration | `src/common/di.rs` (constructs the dispatcher) + `src/infrastructure/auth_factory.rs` (threads it into `AuthApplicationService`) |
