# External-user upgrade — design proposal

> **Status**: design proposal, locked but not implemented. Reviewed Jun 2026
> with Ed; all blocking decisions answered. Open follow-ups listed at the end.

## Context

External users in OxiCloud today are grant-only recipients. They're
created via the magic-link invitation flow:

1. Internal user shares a resource with `subject.type = "email"`.
2. `MagicLinkInviteService::resolve_or_create_recipient` lazy-provisions
   an `auth.users` row with `is_external = true`, `storage_quota_bytes
   = 0` (CHECK constraint `users_external_no_storage`), `password_hash
   = NULL`, no home folder.
3. The recipient gets an email with a magic link; clicking it logs them
   in. From then on they can request a fresh login link (PR 22 cookie
   challenge + 5/h rate limit per recipient).

The model works well for "I just want to share one folder with my
accountant", but it has a sharp edge: if the operator later enables
self-registration on the instance, the external user can't sign up
with their existing email — `POST /api/auth/register`
(`auth_application_service.rs:329-344`) does an email lookup before
anything else and returns `EmailTaken`. The user is locked out of
becoming a full participant on the instance, despite being already
trusted (by whoever invited them).

The intended outcome: a one-click path to promote an existing
external row to a full internal user, preserving every grant they
already have, without forcing them to set a password and without
losing the email-magic-link sign-in path they already know how to
use.

## Locked design (decisions ratified in discussion)

### 1. Authentication and credentials

- **No password required** at upgrade time. The user is already
  authenticated via magic-link session when the upgrade endpoint is
  called; that authentication is sufficient. Post-upgrade they
  continue to log in via login-via-email (the existing
  `send_login_link` path remains available because they still
  satisfy `magic_link_eligibility(user) == Allow` when no password
  is set).
- A password can be set later via the existing `PATCH /api/auth/me`
  change-password endpoint, but it is **not** required during
  upgrade.
- For OIDC instances: the user is authenticated via OIDC. No password
  needed either. See §3 below for OIDC-specific linking.

### 2. Operator gate

- The upgrade endpoint is **enabled iff `OXICLOUD_REGISTRATION_ENABLED
  = true`**.
- No new env var. Operators who keep self-registration closed also
  keep the external-upgrade path closed — symmetric and predictable.
  An instance that doesn't accept new public signups also doesn't
  want existing external rows being promoted into internal accounts
  via a side channel.
- If `false`: endpoint returns `403 Forbidden` with `error_code =
  "upgrade_disabled"`. The UI hides the upgrade affordances when the
  config flag is reflected in `/api/auth/config` (or equivalent
  bootstrap payload).

### 3. OIDC interaction

OIDC-enabled instances have two complications: the user may sign in
via OIDC, and on first OIDC sign-in their OIDC subject claim needs to
land on the existing external row instead of creating a duplicate.

**OIDC callback behaviour**:

- On every OIDC callback, if `claims.email` matches an existing
  `auth.users` row with `oidc_subject IS NULL` and `is_external =
  true`:
  - **Auto-link** — write `oidc_provider = <issuer>`, `oidc_subject =
    <claims.sub>` to the existing row. The user can now log in via
    OIDC.
  - **Do NOT auto-upgrade** — leave `is_external = true` intact. The
    user must still explicitly trigger upgrade via the upgrade
    endpoint.
  - Audit: `auth.oidc_linked_to_external` with `user_id`,
    `oidc_subject`, `issuer`.
- This avoids the surprise where "I logged in via OIDC and suddenly
  I'm a full account I didn't ask for". The two steps are kept
  explicit: log in (auto-link), then opt-in to upgrade.

**Effect**: an OIDC-only instance with an external user has the same
two-step UX as a password instance — log in (via OIDC or magic-link,
both available), then click "Upgrade". The endpoint backing the
upgrade is the same in both cases.

### 4. Username

- Optional, same as today's first-time registration via
  `POST /api/auth/register`. A user can claim a handle now or later
  via `PATCH /api/auth/me/profile`.
- Username is **decoupled from the home-folder name** in this PR
  (see §5 below). Leaving the username blank no longer produces an
  ugly UUID-suffixed home folder.

### 5. Home folder name — `"Personal"` for new and upgraded accounts

Bundled into this PR (promoted from the original open-follow-up
list).

- `folder_service.rs::ensure_home_folder` is updated to create new
  home folders with the hard-coded name **`"Personal"`**.
- The previous logic — `format!("My Folder - {}", username)` with
  fallback to `format!("My Folder - {}", user_id)` — is removed.
  The `username: Option<&str>` parameter on the helper either goes
  away or is kept for compatibility but no longer consulted.
- Affects two callsites:
  - The existing `register` flow (today's registration ships with
    nicer folder names too — small win for new internal users).
  - The new `upgrade_external_user` flow added in this PR.

**Existing users keep their current home-folder name** (e.g.
`My Folder - admin`) until the drives plan's D0 migration runs.
That migration (see `docs/plan/drive.md` → "Migration strategy →
Phase A") will rename every existing user's root folder to
`Personal` as part of the same transaction that creates personal
drives + backfills `drive_id`. The drive layer is then a clean
wrapper named `Personal` whose root folder is also named
`Personal` — no naming mismatch between the drive and its only
child.

**Cosmetic inconsistency window**: between this PR landing and D0
landing, new accounts will have a `Personal` home folder while
existing accounts will have their pre-existing `My Folder - …`
folder. The window is intentional and obviously transient — D0
closes it for everyone.

**Storage representation**: the string `"Personal"` is stored
literally in the DB (`storage.folders.name = 'Personal'`), not
localised. Mirrors how Google's "My Drive" and Microsoft's "My
Files" handle the same naming problem — fixed label across all
users so collaborators see the same name when paths get shared in
chat / docs / search.

**WebDAV implications**:
- Native `/webdav/<path>` keeps working transparently because the
  auto-prepend logic in `resolve_webdav_path` looks up the user's
  home folder dynamically (it reads `home.name`, not a hard-coded
  string).
- NextCloud-compat `/remote.php/dav/files/<username>/<path>` keeps
  working because the prepend in `nc_to_internal_path` (`webdav_handler.rs:51`)
  currently uses `format!("My Folder - {}", username)` — that
  function gets updated **in the same PR** to look up the home
  folder's actual name dynamically (the user-id → drive lookup
  isn't available yet pre-drives, so the simplest implementation
  asks the folder service for the user's root folder name and uses
  it).
- An eventual side effect of this dynamic lookup: when D0 runs and
  every user's home folder gets renamed to `Personal`,
  `nc_to_internal_path` automatically follows. No second code
  change needed.

### 6. Quota

- Set `storage_quota_bytes = OXICLOUD_DEFAULT_STORAGE_QUOTA_BYTES`
  (read from `AppConfig`, same default as registration).
- The CHECK constraint `users_external_no_storage` is no longer
  violated because we simultaneously flip `is_external = false`.
- After the drives plan (`docs/plan/drive.md`) lands, this quota
  becomes the quota on the upgraded user's newly-created personal
  drive. Pre-drives, it lives on the user row as today.

### 7. Domain allowlist re-check

- `OXICLOUD_EXTERNAL_EMAIL_DOMAINS` (the existing allowlist for new
  external-user provisioning via `subject.type = "email"`) is
  **re-checked at upgrade time**.
- If the user's email domain is no longer on the allowlist (e.g.
  the operator pulled the domain since the user was originally
  invited), the upgrade endpoint refuses with `403` and `error_code
  = "domain_not_allowed"`.
- Mirrors today's share-by-email behaviour where the allowlist gates
  *every* new external provisioning, not just the first one. The
  upgrade is conceptually "external → internal account creation",
  so re-checking is consistent.

### 8. Lifecycle and side effects

When the upgrade endpoint runs:

1. Verify caller is `is_external = true` and `active = true`. If not
   external → `409` (`"already_internal"`); if not active → `403`
   (`"account_inactive"`).
2. Verify `OXICLOUD_REGISTRATION_ENABLED = true`. If not → `403`
   (`"upgrade_disabled"`).
3. Verify the caller's email domain is on
   `OXICLOUD_EXTERNAL_EMAIL_DOMAINS` (when set). If not → `403`
   (`"domain_not_allowed"`).
4. If a username was supplied in the body, validate format
   (`validate_username`) and uniqueness. If taken → `409`
   (`"username_taken"`).
5. Flip user row in a single transaction:
   - `is_external = false`
   - `username = <claimed>` (or `NULL`)
   - `storage_quota_bytes = OXICLOUD_DEFAULT_STORAGE_QUOTA_BYTES`
   - `email_verified_at = COALESCE(email_verified_at, NOW())`
     (the upgrade itself is a proof-of-email-control event — the
     user authenticated via magic-link, which already verified
     their email, so this is mostly a no-op for users whose
     `email_verified_at` was stamped at their first magic-link
     redemption)
6. Run the existing **user-lifecycle hook chain** that registration
   already invokes. This creates:
   - Home folder via `folder_service::create_home_folder`
   - Default calendar (when CalDAV is enabled)
   - Default address book (when CardDAV is enabled)
   - Any other registered `UserLifecycleHook` impls
7. Audit: `auth.upgraded_from_external` with `caller_id`, claimed
   username (or `null`), source auth mechanism (`"magic_link"` /
   `"oidc"` — taken from the session).
8. Reissue access + refresh tokens (the old session's claims — most
   notably `is_external` — are stale). Return the new tokens in the
   response body. The frontend swaps them into local storage in the
   same handler.
9. Return refreshed `UserDto`.

**All existing grants stay intact** because every grant references
`user_id`, which doesn't change. Shares of resources *to* the
upgraded user (`subject_id = user.id`) also continue to resolve
correctly.

### 9. Notification-pipeline knock-on

After upgrade:

- `magic_link_eligibility(user)` now returns either:
  - `Allow` (when `has_password` is still NULL and no OIDC link — user
    can still receive login-via-email)
  - `Reject("has_password")` (if they later set a password)
  - `Reject("oidc_user")` (if they later linked OIDC, or it was
    auto-linked per §3)
- `RecipientNotificationService::send_share_notification` (the new
  unified pipeline shipped in PR N1) routes future shares to the
  upgraded user based on this eligibility. No code change needed —
  the pipeline already handles all three branches correctly.
- Concretely: a future internal user sharing a folder with an
  upgraded user gets the **plain-notification email** path (since
  `magic_link_eligibility` now returns Reject for OIDC-linked users
  or has-password users) — the same path as any other internal
  recipient.

### 10. UI cache invalidation

After upgrade, several frontend caches need to refresh:

- `systemUsers` cache keys this user with `is_external = true`.
  Implementation already supports invalidation — call
  `systemUsers.refreshUser(userId)` (or equivalent) after upgrade.
- `OutgoingResourceGrantDto.is_external` field exposed in the My
  Shares listing (added in PR N2) — next API call returns the new
  flag; no migration needed.
- The role-chip / vignette badges that today display the
  "external" marker should automatically dim once the underlying
  `is_external` flag flips.

## UX touchpoints — v1 ships all three

| # | Surface | When it appears | Tone | Conversion strength |
|---|---|---|---|---|
| 1 | Profile page "Account" section | Always present for `is_external = true` users | Informational | Low — for users who go looking |
| 2 | Soft banner above file views | First visit of session, dismissible | Soft nudge | Medium |
| 3 | On-action modal | User clicks Upload / New folder | Strong | High — clearest intent |

### Touchpoint 1 — profile section

Sits above the existing profile-edit form:

```
┌──────────────────────────────────────────────────┐
│ Account                                          │
│ ─────────────────────                            │
│ You're using a guest account. You can view       │
│ files shared with you, but you can't upload      │
│ your own files or create folders.                │
│                                                  │
│ [ Upgrade to a full account → ]                  │
└──────────────────────────────────────────────────┘
```

Click → inline form: optional username field (placeholder shows
what the home folder will be named based on input), [Upgrade]
button. No password field, no extra prompts. Confirmation on success
shows the new home folder location and a "go to my files" link.

### Touchpoint 2 — soft banner

Above the file-list area on every file view:

```
👋 You're a guest. Upgrade to upload files and create folders. [Upgrade] [Dismiss for now]
```

Dismissal is **per-session** (sessionStorage), not permanent — comes
back on next login. Click Upgrade → opens the same form as in
touchpoint 1, modal style.

### Touchpoint 3 — on-action modal

When an external user clicks Upload / New folder / Drag-drop:

```
┌──────────────────────────────────────────────────┐
│ Guest accounts can't upload                      │
│                                                  │
│ To upload your own files, you need a full        │
│ OxiCloud account. It takes 30 seconds.           │
│                                                  │
│ [ Cancel ]    [ Upgrade my account → ]           │
└──────────────────────────────────────────────────┘
```

Click Upgrade → same form, inline in the modal.

### Affordances NOT in v1

- One-time prompt after every magic-link redemption — too
  aggressive; some users genuinely want magic-link-only flow.
- Email reminders / nag campaigns — spam vector, no.
- Blocking *view* features behind upgrade — guests must keep their
  current viewing capabilities. Upgrade unlocks ownership + uploads
  + folder creation + sharing as a granter, nothing else changes
  for what they could already see.

## Endpoint shape

```
POST /api/auth/me/upgrade
Headers: Authorization: Bearer <session>   ← magic-link OR OIDC OK
Body:
{
    "username": "alice"   // optional
}

Success (200):
{
    "user": UserDto,           // refreshed, is_external = false
    "access_token": "...",
    "refresh_token": "...",
    "expires_in": 3600
}

Errors:
    403 upgrade_disabled       // OXICLOUD_REGISTRATION_ENABLED = false
    403 domain_not_allowed     // email domain off the allowlist
    403 account_inactive       // user.active = false
    409 already_internal       // user.is_external = false
    409 username_taken         // claimed handle is in use
    400 invalid_username       // doesn't pass validate_username
```

## OIDC callback change (separate, lands with this PR)

In `auth_application_service::oidc_callback` (or wherever the JIT
provisioning branch lives):

```text
existing logic:
    by_subject = find_user_by_oidc_subject(provider, sub)
    if by_subject:
        update last_login, return session
    else:
        check email, if email taken by non-external row → conflict
        else: create new user row with is_external = false, hook chain runs
        return session

new branch BEFORE creation:
    if email matches an existing row AND row.is_external = true AND row.oidc_subject IS NULL:
        link OIDC subject to existing row (UPDATE only, no row creation,
            no is_external change, no hook chain)
        audit: auth.oidc_linked_to_external
        return session for the existing row
```

The link does NOT trigger the user-lifecycle hooks (no home folder
created yet). Those run when the upgrade endpoint is explicitly
called.

## Migration — no schema migration needed

The upgrade is a logical state change on existing columns:

- `auth.users.is_external` — already exists, just flipped from
  `true` → `false`.
- `auth.users.username` — already exists, may be set or left NULL.
- `auth.users.storage_quota_bytes` — already exists, value adjusted.
- `auth.users.password_hash` — left NULL (no password set during
  upgrade).
- `auth.users.email_verified_at` — already stamped at the first
  magic-link redemption for most users.

The existing CHECK constraints (`users_external_no_storage`,
`users_external_not_admin`) already permit the new state because
they're conditional on `is_external = true`. Flipping the flag
satisfies them by definition.

**No new env var.** Reuses `OXICLOUD_REGISTRATION_ENABLED`,
`OXICLOUD_DEFAULT_STORAGE_QUOTA_BYTES`, `OXICLOUD_EXTERNAL_EMAIL_DOMAINS`
exactly as they exist today.

## PR sequencing

Small enough to land in one PR:

| Phase | Scope |
|---|---|
| **Backend** | New endpoint `POST /api/auth/me/upgrade` in `auth_handler.rs`. New service method `AuthApplicationService::upgrade_external_user`. OIDC callback patch for the "link-on-first-OIDC-login" branch. Audit events. Unit + Hurl tests covering the eight error branches above. |
| **Frontend** | Three new affordances: profile section, soft banner, on-action modal. All call the same upgrade form. Form is shared component (inline in profile, modal elsewhere). Post-success token swap + user-data refresh + `systemUsers.refreshUser(id)`. |
| **i18n** | New keys: `upgrade.title`, `upgrade.intro`, `upgrade.username_optional_hint`, `upgrade.button`, `upgrade.banner_intro`, `upgrade.modal_blocked_title`, `upgrade.modal_blocked_body`, `upgrade.success_toast`, plus error messages for the six failure modes. Translate into all 16 locales (the existing translation sweep pattern). |
| **Docs** | `docs/config/env.md` — add a paragraph noting that `OXICLOUD_REGISTRATION_ENABLED` now also gates external-user upgrades. `example.env` mirrored. |

Single PR is realistic — ~400 lines backend, ~250 lines frontend,
~30 i18n keys × 16 locales, plus tests.

## Verification

- `cargo fmt && cargo clippy --all-features --all-targets -- -D
  warnings` clean.
- `cargo test --workspace` — new unit tests cover: happy path (no
  username, with username), all eight error branches, OIDC auto-link
  on first sign-in does not flip `is_external`, home folder is
  created during upgrade, quota is set, audit event fires with
  correct source auth.
- New Hurl test `tests/api/external_upgrade.hurl` — admin invites
  `upgrade-test@example.com` via share, recipient redeems magic
  link, recipient hits `POST /api/auth/me/upgrade` → 200, recipient
  uploads a file (proves the new quota + home folder work).
- Hurl negative tests: `OXICLOUD_REGISTRATION_ENABLED=false` → 403;
  email domain not in allowlist → 403; user.is_external = false →
  409; username collision → 409.
- Hurl OIDC test (using the existing OIDC stub mode): stub claims
  email matches existing external user → external user gets OIDC
  subject linked on next OIDC sign-in, `is_external` stays true →
  next call to `/api/auth/me/upgrade` succeeds and home folder is
  created.
- Frontend manual: log in as an invited external user via magic
  link, see the soft banner. Click [Dismiss for now] — banner gone
  for the session. Navigate to /profile → "Account" section
  visible. Click Upload → modal appears. Click Upgrade → form
  appears, submit blank → success, banner gone, can now upload,
  files appear in newly-created home folder.

## Out of scope (deferred to future)

- **Downgrade path** (internal → external). Not needed; if a user's
  storage should be revoked, the existing user-delete + share-revoke
  flows handle it.
- **Bulk upgrade** for admins ("upgrade all external users in
  domain X"). Operationally unusual; a one-off SQL update is the
  escape hatch.
- **Email confirmation** before upgrade. The user is already
  authenticated via magic-link (which verified email) or OIDC
  (which the IdP verified). No extra confirmation needed.
- **Email-local-part fallback for home folder name** when no
  username is claimed — see open follow-up §1.
- **Tracking upgrade-conversion analytics** (banner shown vs
  upgrade clicked). Useful product metric but separate from
  shipping the feature.
- **Re-running the audit trail across upgraded users** to label
  past actions retrospectively. The new audit event marks the
  transition point; backfill is unnecessary.

## Open follow-ups (non-blocking, worth noting)

1. **Soft banner persistence across sessions**. v1 dismisses
   per-session (sessionStorage). Some users might want
   "dismiss permanently". Could add a server-side
   `auth.users.dismissed_upgrade_banner` boolean, but that
   pollutes the user table for a UI affordance. Possibly defer
   to a future "user UI preferences" subsystem.

2. **Granting the upgrade implicitly when an external user
   self-OIDC-signs-in on an OIDC-only instance with auto-create
   policy**. We explicitly rejected this design decision
   (auto-link, no auto-upgrade), but it's worth keeping in the
   future-considerations notebook in case operator feedback
   pushes the other way for closed-team instances.

3. **Per-domain admin policy** — "upgrade is automatic for
   `@my-company.com`, manual for everyone else". Adds complexity;
   skip unless real demand surfaces.

## Existing code to reuse

- **`AuthApplicationService::register`**
  (`auth_application_service.rs:306`) — the user-lifecycle hook
  chain it invokes is the same one the upgrade endpoint reuses.
  Refactor to extract the hook-invocation helper into a private
  `_run_user_create_hooks(user_id)` method that both `register`
  and `upgrade` call.
- **`folder_service::create_home_folder`**
  (`folder_service.rs:644`) — already creates the home folder
  with the username-or-uuid suffix logic. Called by the hook
  chain.
- **`validate_username`** in `user.rs` — reused unchanged for the
  upgrade endpoint.
- **`User::set_username`** in `user.rs:435` — already handles
  the "claim once" semantic. The upgrade flow uses this when a
  username is supplied.
- **`magic_link_eligibility`**
  (`magic_link_invite_service.rs:79`) — already produces the
  correct branch for upgraded users; no change needed in the
  notification pipeline.
- **`RecipientNotificationService`** — automatically transitions
  upgraded users from the magic-link arm to the
  plain-notification arm as their eligibility changes.
  No code change.
- **Token issuance**
  (`auth_application_service.rs::issue_tokens` or wherever) — the
  upgrade endpoint reuses this to mint fresh tokens with the
  updated `is_external` claim.
- **`OidcCallback` handler** — the new "auto-link to existing
  external row" branch lives next to the existing JIT-provision
  branch.

## File map (anticipated)

```
src/application/services/auth_application_service.rs (modify)
  add: upgrade_external_user method
  add: auto-link branch in oidc_callback
  refactor: extract _run_user_create_hooks helper

src/interfaces/api/handlers/auth_handler.rs (modify)
  add: upgrade_external_user handler

src/interfaces/api/routes.rs (modify)
  add: POST /api/auth/me/upgrade route

src/application/dtos/user_dto.rs (modify)
  add: UpgradeExternalUserDto (just optional username for now)

static/js/views/profile/profile.js (modify)
  add: "Account" section render path for is_external users

static/js/components/                          (new)
  upgradeForm.js                               (shared form, used by all three touchpoints)
  upgradeBanner.js                             (top-of-files-view banner)
  upgradeModal.js                              (on-action blocked modal)

static/css/components/upgradeForm.css          (new)
static/css/components/upgradeBanner.css        (new)

static/locales/*.json (modify)
  add upgrade.* keys; translate into all 16 locales

docs/config/env.md (modify)
  paragraph on OXICLOUD_REGISTRATION_ENABLED gating both signup
  and external-upgrade

example.env (modify)
  mirror the docs paragraph in the inline comments
```

## Audit events (canonical names — stable, log aggregators key off these)

| event | reason | When |
|---|---|---|
| `auth.upgraded_from_external` | (none — success) | Upgrade endpoint succeeded |
| `auth.upgrade_rejected` | `disabled` | `OXICLOUD_REGISTRATION_ENABLED=false` |
| `auth.upgrade_rejected` | `domain_not_allowed` | Email domain off the allowlist |
| `auth.upgrade_rejected` | `account_inactive` | `user.active = false` |
| `auth.upgrade_rejected` | `already_internal` | `user.is_external = false` |
| `auth.upgrade_rejected` | `username_taken` | Claimed handle in use |
| `auth.upgrade_rejected` | `invalid_username` | Doesn't pass `validate_username` |
| `auth.oidc_linked_to_external` | (none — success) | OIDC subject auto-linked to existing external row on first OIDC sign-in |

The `auth.upgrade_rejected` reasons are stable enum-style keys; if a
new failure mode lands later it gets a new reason value, never
repurposing an existing one (per CLAUDE.md audit conventions).

## Glossary

- **External user** — `auth.users.is_external = true`. Created via
  magic-link invitation. No storage, no home folder, no password.
- **Internal user** — `auth.users.is_external = false`. Has a home
  folder, a quota, and at least one authentication method
  (password, OIDC, or magic-link login-via-email).
- **Upgrade** — flipping a user row from external to internal,
  running the user-lifecycle hooks that create the home folder + any
  feature-default resources (calendar, address book).
- **Auto-link** — the OIDC callback step that attaches an OIDC
  subject claim to an existing external row when the email matches.
  Does NOT upgrade by itself.
