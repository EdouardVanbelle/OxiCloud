# Plan — Auth simplification (PR 16–21)

## Context

The magic-link work (PRs 6–12) shipped external users as a second principal kind with `username = email`. PR 13 closed the route-level lockouts. Across a design conversation on 2026-06-02 we agreed the resulting model is needlessly two-tier and can be simplified by making **email the identity**, **username an optional handle**, and **credentials (password / OIDC) truly optional and orthogonal**. The proximate motivation: avoid the username-vs-email cross-collision class of bugs we just spent time guarding against, and reduce password-hash density in the DB by letting users sign up email-only with magic-link as their bootstrap path.

The end state: every user is identified by email; `username`, `password_hash`, and `oidc_subject` are all `Option<…>` columns whose presence is observable but never *required*. Login is a one-line dispatch on `@`-in-input. Magic-link eligibility is a three-branch rule (OIDC always rejected, password rejected by default, no-credential allowed) with one env knob to flip the middle branch. No new auth methods land here — TOTP / WebAuthn / passkey enrolment stays out of scope. The whole work is reorganisation of existing primitives, not new ones.

## Design decisions (locked in by conversation)

### Identity model

| Slot | Type | Meaning |
|---|---|---|
| `email` | `String` (NOT NULL UNIQUE) | The identity. Every login path ultimately resolves here. |
| `username` | `Option<String>` (UNIQUE, NULL allowed) | Optional handle. 2-64 chars inclusive, `[A-Za-z0-9._-]+` (no `@`). Claimable post-creation. Multiple NULLs coexist under the existing UNIQUE index. |
| `password_hash` | `Option<String>` | An Argon2 hash if the user chose one. NULL otherwise. NO placeholder strings. |
| `oidc_subject` | `Option<String>` | The IdP subject claim if the user linked one. NULL otherwise. |
| `is_external` | `bool` | Provisioning origin marker. `true` when created via email-invite from a sharer. Future "promote to internal" flow (separate TODO) flips this. |

Eligibility predicates derive from the slots:

```rust
fn has_password(&self) -> bool      { self.password_hash.is_some() }
fn has_oidc(&self) -> bool          { self.oidc_subject.is_some() }
fn has_login_credential(&self) -> bool {
    self.has_password() || self.has_oidc()
}
```

No more sentinel strings (`__EXTERNAL_NO_PASSWORD__`, `__OIDC_NO_PASSWORD__`). The schema migration NULLs them out as part of PR 16.

### Login dispatch

```
input contains '@'   → lookup by email,    verify password
input does not       → lookup by username, verify password
```

Single DB hit. Unambiguous because `@` is forbidden in `username`. The same dispatcher serves the magic-link send endpoint, but that endpoint only takes email — input without `@` is a 400.

### Magic-link eligibility ladder

```rust
pub fn magic_link_eligibility(user: &User, open_to_password_users: bool) -> Eligibility {
    if user.has_oidc()        { return Reject("oidc_user"); }      // unconditional; IdP is the security boundary
    if user.has_password()    {
        return if open_to_password_users { Allow } else { Reject("has_password") };
    }
    Allow                                                            // no credentials at all
}
```

| User state | Magic-link eligible? |
|---|---|
| No password, no OIDC | Yes — always |
| Has password, no OIDC | Default no; `OXICLOUD_MAGIC_LINK_OPEN_TO_PASSWORD_USERS=true` opens it |
| Has OIDC (with or without password) | **No — always.** Flag has no effect. |

The `oidc_user` reject is unconditional because OIDC is the only path to MFA today (delegated to the IdP), and magic-link must never bypass it. Future native 2FA (TOTP, WebAuthn) lands a third reject branch behind the existing patterns.

### Registration paths

| Path | Pre-condition | Effect |
|---|---|---|
| `POST /api/auth/register` with `email + password` | Public registration enabled | User row with password_hash set. JWT issued. |
| `POST /api/auth/register` with `email` only | Public registration enabled | User row with `password_hash = None`. Magic-link mailed to the address for first-session bootstrap. JWT NOT issued by the register call. |
| `POST /api/grants { subject.type: "email" }` | Sharer has Share permission | External user lazily provisioned. Magic-link invitation mailed. Existing PR 9 flow. |
| OIDC JIT | First IdP-mediated login | User row with `oidc_subject` set, `password_hash = None`. JWT issued. |

All four return the **same** uniform shape regardless of email existence (PR 20 closes the register oracle that survives from the original schema). Real reason recorded in audit.

### Anti-enumeration is preserved everywhere

- `POST /api/auth/register` → 200 uniform, audit reasons `created` / `email_taken` / `username_taken` / `disabled`
- `POST /api/auth/magic-link/send` → 200 uniform, audit reasons `sent` / `no_account` / `has_password` / `oidc_user` / `account_deactivated` / `malformed_email` / `rate_limited_email` / `rate_limited_ip`
- `POST /api/auth/login` → 403 uniform `Invalid credentials`, audit reasons `unknown_user` / `bad_password` / `account_deactivated`

### Migration discipline

Forward-only migrations. The previous `…000003_users_username_email_login.sql` widening to 254 chars has already been applied to dev / CI environments — squashing with the new shrink-and-NULL migration would break `_sqlx_migrations` checksum tracking. Add a fresh migration file; history records the two-step story honestly.

### What stays the same

- JWT structure, session lifetimes, refresh-token rotation
- `is_external` flag and its DB CHECK constraints (`users_external_not_admin`, `users_external_no_storage`)
- All PR 13 route-level lockouts (external users still can't reach CalDAV/CardDAV/WebDAV/NC, can't mint app passwords, can't enumerate groups)
- ReBAC `access_grants` table and all permission semantics
- SMTP-unconfigured ⟹ magic-link unavailable ⟹ external invitations unavailable (existing 503 paths)
- The three rate limiters from PR 12 — caps and keys unchanged

## PR sequence

| PR | Subject | Why land separately |
|---|---|---|
| **16** | Schema + entity: nullable `username` / `password_hash`, format CHECK, sentinel cleanup, `User::new` collapse, audit-log Option handling | Foundational — every later PR consumes `Option<String>` columns. Verifiable in isolation by re-running the existing Hurl suite. |
| **17** | Login dispatcher: input `contains('@')` decides email vs. username path. Pure refactor of `AuthApplicationService::login`. | One behavioural axis; isolated test surface. |
| **18** | Optional password at registration: `RegisterDto.password: Option<String>`. Password-less path mints a magic-link to the supplied email. | The "email-only signup" UX. Depends on 16. |
| **19** | `OXICLOUD_MAGIC_LINK_OPEN_TO_PASSWORD_USERS` env knob + `magic_link_eligibility()` refactor with three audit reasons. | Single env-driven policy switch with observability. |
| **20** | Anti-enumeration `register`: uniform 200 regardless of outcome, real reasons in audit channel. | Closes the username/email enumeration oracle that survives from the original schema. |
| **21** | `docs/architecture/auth-model.md` (~250 lines) + sidebar + cross-references. Acceptance gate. | Big-picture documentation of the final identity / credential / login surface. |
| **22** | Device-bound magic-link redemption (challenge cookie) + asymmetric TTLs: login-via-email tokens live 10 minutes, invitation tokens live 24 hours. | Closes the mailbox-as-bearer-token attack class on the login flow. Invitations stay cross-device (recipient has no prior browser context with the server). |
| **23** | `auth.users.email_verified_at` column + flip-to-verified on magic-link redemption / OIDC-with-verified-claim. Read-only API surface for now (future PRs gate features on it). | Establishes a verified-control-of-inbox signal so later policy can require it (e.g. block uploads / shares for unverified users). |

## Critical files

### New

- `migrations/20260603000000_username_optional_no_email_shape.sql` — username + password_hash schema cleanup
- `tests/api/auth_login.hurl` — dispatcher coverage (PR 17)
- `tests/api/registration.hurl` — email-only signup + anti-enumeration (PR 18 + PR 20)
- `docs/architecture/auth-model.md` — final architecture page (PR 21)

### Modified — by PR

**PR 16:**
- `src/domain/entities/user.rs` — `username: Option<String>`, `password_hash: Option<String>`, `oidc_subject` confirmed `Option<String>`. Collapse `new_password` / `new_oidc` / `new_external` into one `User::new` (or keep three named factories if readability outweighs the duplication). Drop sentinel string comparisons; `has_login_credential` becomes a two-line `Option::is_some` check.
- `src/infrastructure/repositories/pg/user_pg_repository.rs` — Option binding in all 8 SELECT/INSERT/UPDATE sites
- `src/application/dtos/user_dto.rs` — `username: Option<String>`
- `src/application/services/magic_link_invite_service.rs::resolve_or_create_recipient` — pass `None` for username when minting an external
- `src/application/services/auth_application_service.rs` — every audit line emitting `username = %user.username()` switches to a `display_for_audit(&user)` helper that falls back to user_id when `None`
- `src/interfaces/api/handlers/contacts_handler.rs::user_to_contact` — already handles None fallback (from yesterday's given/family work), but verify after the type change
- `src/interfaces/nextcloud/routes.rs::verify_url_user` — when `auth_user.username` is None, return 403 instead of comparing to the URL segment (externals are already PR-13-blocked but the type change forces an explicit branch)
- Every `audit` log line in `auth_handler.rs`, `auth_application_service.rs`, `magic_link_invite_service.rs` — review for `username =` interpolations

**PR 17:**
- `src/application/services/auth_application_service.rs::login` — dispatch on `@`
- `src/application/dtos/user_dto.rs::LoginDto` — rename `username` field to `username_or_email` with a serde alias for backward compat, or leave the name and document the new semantics
- Frontend `static/login.html` — change the input placeholder from "Username" to "Username or email"

**PR 18:**
- `src/application/dtos/user_dto.rs::RegisterDto.password: Option<String>` + validation: when present, enforce length minimum
- `src/application/services/auth_application_service.rs::register` — branch on `dto.password.as_ref()`. When None: create user with `password_hash = None`, then call `MagicLinkInviteService::send_login_link(&dto.email)` as a best-effort post-action
- `src/interfaces/api/handlers/auth_handler.rs::register` — uniform 201 either way

**PR 19:**
- `src/common/config.rs::MagicLinkConfig.open_to_password_users: bool` (default false) + env loader
- `src/application/services/magic_link_invite_service.rs` — new `magic_link_eligibility()` function; replace the single `if user.has_login_credential()` check in `send_login_link` and `issue_invitation`
- `example.env` — new entry with explanation

**PR 20:**
- `src/application/services/auth_application_service.rs::register` — silence the descriptive error strings; return Ok with the same uniform shape on collision; audit-log the truth
- `src/interfaces/api/handlers/auth_handler.rs::register` — response body becomes the uniform "If the email is available, a confirmation link has been sent."
- Hurl regression: the existing register test in `tests/api/setup.hurl` etc. needs to assert the new uniform shape

**PR 21:**
- `docs/architecture/auth-model.md` (new file, ~250 lines)
- `docs/.vitepress/config.mts` — sidebar entry after `magic-link-auth`
- `docs/architecture/magic-link-auth.md` — § "Identity model" links to `auth-model.md`
- `docs/architecture/share-integration.md` — already references `magic-link-auth`; chain stays one hop deep

## Existing patterns to reuse (with paths)

- **MockEmailSender** at `src/infrastructure/services/mock_email_sender.rs` — every PR 18 / 20 Hurl test uses it via `GET /api/admin/smtp/test/captured?to=…`. Already configured in `tests/common/server.env` (`OXICLOUD_SMTP_MOCK=true`).
- **Audit-log convention** from `CLAUDE.md` § Authorization — every reject emits `tracing::info!(target: "audit", event = "<domain>.<verb>", reason = "<key>", …)` with structured fields. New reasons in this work:
  - `auth.register_rejected` with `email_taken`, `username_taken`, `disabled`
  - `auth.magic_link_send` gains `has_password`, `oidc_user` (replacing single `has_credential`)
  - `auth.login_rejected` reasons unchanged
- **Migration mirroring** — `migrations/20260612000003_users_username_email_login.sql` (or the actual existing file) is the precedent for username-column changes. New migration sits beside it.
- **Three rate limiters from PR 12** — caps and keys unchanged. No new limiters in this work.
- **Eligibility split pattern** — `Eligibility::Allow / Reject(reason)` enum is new; canonical home is `application/services/magic_link_invite_service.rs` next to the existing `MagicLinkResourceKind`.

## Verification

Per-PR (mandatory, every PR):
```bash
cargo fmt --all
cargo clippy --all-features --all-targets -- -D warnings
cargo test --workspace --lib
bash tests/api/run.sh
```

Frontend checks when touching `static/`:
```bash
biome check --fix static/
stylelint static/css/
tsc -p jsconfig.json --noEmit
```

### End-to-end gates

**After PR 16** (smoke): the existing 14 Hurl files still pass. Bob's flow in `external_users.hurl` works with bob's username now NULL (assertion updates in `Step 11c` and `Step 12`).

**After PR 17:**
1. Hurl: alice (internal, picked username "alice") logs in via `username = "alice"` → 200
2. Hurl: alice logs in via `username = "alice@oxicloud.local"` (her email) → 200
3. Hurl: bob (external, NULL username) logs in via `username = "bob@externalcompany.com"` → 403 (no password); via magic-link path → still works as before
4. Hurl: wrong password on either path → uniform `403 Invalid credentials`

**After PR 18:**
1. Hurl: `POST /api/auth/register` with `{email, password}` → 201, JWT returned (existing behaviour)
2. Hurl: `POST /api/auth/register` with `{email}` only → 200 with uniform body; `GET /api/admin/smtp/test/captured?to=…` returns the welcome magic-link
3. Hurl: follow the captured URL → session cookies set, redirect to `/#/`
4. Hurl: user now has `password_hash = NULL` in DB; subsequent magic-link send is allowed (eligible)
5. Hurl: user sets a password via `PUT /api/auth/change-password` → next magic-link send to same email returns 200 but NO new mail captured (strict mode + `has_password` audit)

**After PR 19:**
1. Hurl in default mode: password user gets no mail on magic-link send (existing strict-mode test)
2. Hurl in lenient mode (`OXICLOUD_MAGIC_LINK_OPEN_TO_PASSWORD_USERS=true` in `tests/common/server.env`, or a separate run): mail IS captured for the same scenario
3. Hurl: OIDC user → no mail regardless of flag value (the `oidc_user` audit reason fires unconditionally)
4. Unit test: 16-row matrix of `(has_password, has_oidc, is_external, flag)` × eligibility outcome

**After PR 20:**
1. Hurl: `POST /api/auth/register` with an already-used email → 200 uniform; no new user row in DB; audit log line `auth.register_rejected reason=email_taken`
2. Hurl: `POST /api/auth/register` with an already-used username → 200 uniform; same shape
3. Hurl: timing comparison (informal) — collision path returns within ±10ms of the success path

**After PR 21** (acceptance):
- `auth-model.md` renders correctly in VitePress dev (`cd docs && npm run dev`)
- Sidebar shows the new entry under Architecture
- Every audit reason listed in the doc is grep-able to a real `tracing::info!` call in `src/`

## Out of scope (do NOT bundle)

Items the conversation explicitly deferred. Each has a clear future trigger.

- **Native 2FA (TOTP, WebAuthn enrolment) for password users.** Today OIDC delegation is the only MFA path; native enrolment requires UI, recovery codes, and a third reject branch in `magic_link_eligibility()` (`mfa_enrolled`). Listed in `auth-model.md` § "What is deliberately out of scope".
- **`login_strategy` per-user policy enum.** The conversation surfaced this as a future architectural direction. Captured in `auth-model.md` § "Future direction — per-user login strategy" with the matrix (`passwordless`, `password`, `password_or_magic_link`, `password_and_magic_link`, `oidc`, `password_and_totp`, `password_and_webauthn`). No implementation in this work.
- **External-user promotes to internal.** Triggered when an external sets a credential. Today `is_external` stays TRUE post-credential-set; the upgrade flips it to FALSE and provisions a home folder + Internal-group membership + DAV access. Depends on registration UX direction (Design A "claim username when needed" vs. Design B "auto-generate handle") being decided. Separate work.
- **`session_kind` on sessions emitted from magic-link.** A magic-link session today is indistinguishable from a password session. Enables scoped sessions later (Option-B style "magic-link session can only access granted resources"). Not load-bearing for v1.
- **Differentiated session TTL for externals.** Uniform refresh-token expiry today. Future env `OXICLOUD_EXTERNAL_REFRESH_TOKEN_EXPIRY_DAYS`.
- **Open Cloud Mesh (OCM) federation.** Third source for external provisioning. The `ExternalIdentityLifecycleHook::on_user_created` design accommodates the `source` discriminator (`magic_link` / `oidc` / `ocm`).
- **Recovery codes / passkey enrolment.** Tied to native 2FA above.
- **Per-user opt-out of magic-link when lenient mode is on.** Today the env flag is instance-wide. A future per-account toggle (e.g. high-privilege admins disabling magic-link for themselves) would need an `auth.users.magic_link_disabled BOOLEAN` column and one extra branch in eligibility. Listed in `auth-model.md`.

### PR 23 — Email-verified signal (design recap)

**Goal**: track whether the user has demonstrated control of their email address. The signal is data-only in PR 23 — future PRs will gate features (uploads, shares, sensitive operations) on it via an env switch.

**Schema migration**:

```sql
ALTER TABLE auth.users
    ADD COLUMN email_verified_at TIMESTAMPTZ NULL;

-- Backfill: anyone who has successfully been through a flow that
-- proves email control gets stamped retroactively. OIDC implies the
-- IdP confirmed the email; external users who've logged in at least
-- once must have clicked an invitation link.
UPDATE auth.users
   SET email_verified_at = COALESCE(last_login_at, created_at)
 WHERE oidc_subject IS NOT NULL
    OR (is_external = TRUE AND last_login_at IS NOT NULL);

COMMENT ON COLUMN auth.users.email_verified_at IS
    'When the user demonstrated control of their email. NULL = unverified
     (password-only signup whose user never clicked a verification link, or
     admin-created user who hasn''t logged in via magic-link). Set on
     successful magic-link redemption OR OIDC JIT with email_verified=true claim.';
```

**Entity additions** (`domain/entities/user.rs`):

```rust
pub email_verified_at: Option<DateTime<Utc>>,

impl User {
    pub fn is_email_verified(&self) -> bool {
        self.email_verified_at.is_some()
    }
    /// Stamp the verification time. Idempotent — keeps the first
    /// verification timestamp on re-verification to preserve the
    /// "first proof of control" semantics.
    pub fn mark_email_verified(&mut self) {
        if self.email_verified_at.is_none() {
            self.email_verified_at = Some(Utc::now());
            self.updated_at = Utc::now();
        }
    }
}
```

**Trigger points** (one call per flow):

1. `magic_link_invite_service::redeem` — after successful token consumption (clicking the link IS the proof). Set on **every** magic-link redemption regardless of whether it's an invitation or a login-via-email.
2. `auth_application_service::oidc_callback` JIT-create branch — when claims include `email_verified=true`. The existing `email_verified` check (around line 1666) already enforces this for OIDC; we just persist the timestamp.
3. `auth_application_service::oidc_callback` existing-user branch — if the IdP STILL says verified AND our column is NULL (e.g. user predates this PR), upgrade them retroactively.

**Trigger points that do NOT set it** (worth being explicit):

- Classic password registration via `/api/auth/register` with both email + password — the user gave us an email but hasn't proven it works.
- Admin-create-user via the admin panel — admin asserts the email; user hasn't.
- The email-only signup welcome path: the FIRST magic-link redemption stamps the flag (PR 23 hook). So between "user signed up email-only" and "user clicked welcome link", they're unverified.

**API surface**:

- `UserDto.email_verified_at: Option<DateTime<Utc>>` (with `#[serde(skip_serializing_if = "Option::is_none")]` to match the existing optional fields).
- `GET /api/auth/me` and `GET /api/users/{id}` carry it through transparently. No new endpoints in PR 23.

**Hurl coverage**:

- After bob redeems his invitation: `GET /api/users/{bob_user_id}` returns `email_verified_at` set.
- After charlie (classic password registration): `GET /api/auth/me` returns no `email_verified_at` (omitted from JSON).
- After charlie later runs through magic-link (lenient mode): flag flips to non-NULL.

**What PR 23 does NOT do** (deferred):

- The env switch `OXICLOUD_REQUIRE_EMAIL_VERIFICATION` that gates uploads/shares/etc. — that's a future feature PR. PR 23 just establishes the signal.
- UI affordances ("verify your email" banner, resend button) — future frontend PR.
- Per-feature thresholds (e.g. "verified users can share publicly, unverified can only share with internal users") — future policy PRs.

### PR 22 — Device-bound magic-link redemption (design recap)

**Threat closed**: today a magic-link URL is a bearer token — anyone who reads the recipient's email can redeem it. PR 22 binds the login-via-email path to the originating browser so mailbox compromise alone no longer grants a session.

**Asymmetric scope** — binding applies to **login-via-email only**, not invitations:

| Flow | Initiator | Bound to browser? | TTL |
|---|---|---|---|
| `POST /api/auth/magic-link/send` (login-via-email) | The user themselves, in a browser | Yes (challenge cookie) | **10 minutes** |
| `POST /api/grants` with `subject.type=email` (invitation) | A sharer, recipient has no prior browser context | No (inherently cross-device) | **24 hours** (existing) |

**Mechanism** (cookie-only, no UX overhaul):

1. `POST /api/auth/magic-link/send` mints the token AND sets `oxicloud_magic_request=<random>` cookie on the requesting browser (HttpOnly, SameSite=Strict, TTL matches token TTL). The cookie value is mirrored onto a new `auth.magic_link_tokens.request_challenge` column.
2. `GET /magic/v1/{token}` for login-bound tokens checks the cookie:
   - **Cookie present and matches** → redeem instantly (common case; zero UX change).
   - **Cookie absent or mismatched** → show a small confirmation page: *"You opened this link in a different browser than you requested it from. If you trust this device, click Continue to sign in."* On click → redeem and audit-log `auth.magic_link_redeem reason="cross_browser_confirmed"`.
3. Invitation tokens (`resource_id IS NOT NULL`) bypass the check — they have no `request_challenge` to compare against and are cross-device by design.

**Config additions** (replacing single `OXICLOUD_MAGIC_LINK_TTL_HOURS`):

- `OXICLOUD_MAGIC_LINK_LOGIN_TTL_MINUTES` — default 10. Applies to tokens minted by `send_login_link` (no resource target, browser-bound).
- `OXICLOUD_MAGIC_LINK_INVITE_TTL_HOURS` — default 24. Applies to tokens minted by `issue_invitation` (resource target, recipient is the audience).
- The old `OXICLOUD_MAGIC_LINK_TTL_HOURS` becomes a deprecated alias for `_INVITE_TTL_HOURS` to avoid breaking existing deployments.

**Schema migration**:

```sql
ALTER TABLE auth.magic_link_tokens
    ADD COLUMN request_challenge TEXT NULL;
COMMENT ON COLUMN auth.magic_link_tokens.request_challenge IS
    'Random per-request value mirrored into the oxicloud_magic_request cookie. NULL for invitation tokens (cross-device by design); set for login-via-email tokens (browser-bound).';
```

**Hurl coverage**:
- Login-via-email: send → capture cookie + mail → redeem with cookie → 302 + session. Same flow without cookie → confirmation HTML page; submit the confirmation form → 302 + session + `cross_browser_confirmed` audit entry.
- Invitations: send via grants → recipient (different browser, no cookie) → 302 + session (no challenge requested).
- Login-bound token at 11 minutes → expired; invitation token at 11 minutes → still valid.

**Why this slots in here**: hardens the most attack-prone surface in the auth model. PR 21's `auth-model.md` should describe the bound + asymmetric-TTL model from the start — so PR 22 either lands before PR 21, or PR 21's doc explicitly flags the upcoming work.

## Locked-in design decisions (confirmed before PR 16)

1. **Single `User::new` constructor.** Signature: `User::new(email, password_hash: Option<String>, oidc_subject: Option<String>, is_external: bool, …)`. The three named factories (`new_password` / `new_oidc` / `new_external`) collapse into call-site helpers if needed, but the canonical API is one constructor.
2. **`OXICLOUD_MAGIC_LINK_OPEN_TO_PASSWORD_USERS` is symmetric.** When `true`, both `/api/auth/magic-link/send` AND `/api/grants` email-invitations mail through to password users. Single `magic_link_eligibility()` predicate, same audit reason `has_password` in both code paths.
3. **Email-only signup welcome redemption lands on `/#/files`.** Redemption logic adjustment: NULL `resource_id` + `is_external = false` → `/#/files`; NULL + `is_external = true` → `/#/sharedwithme` (existing rule). One extra branch in `magic_link_handler::redeem`.
4. **`LoginDto.username` field name kept** with docstring updated to "accepts email or username". Frontend already sends the input as `username` regardless of what the user typed. Mentioned in `auth-model.md` as an intentional ambiguity in the API shape.

## Recommended future event triggers (DON'T ship in this work)

Same convention as previous plans — a future event ships only when there's a concrete consumer.

| Future event | What would force it |
|---|---|
| `on_password_set` | When a user first sets a password — useful for audit ("alice is no longer magic-link-eligible" or "alice is now lenient-mode-eligible") and for invalidating any outstanding magic-link tokens she has. Today the new tokens are simply unused; reaping them via this event would be cleaner. |
| `on_oidc_linked` | When a user first links OIDC — useful for the same audit purpose, and especially load-bearing because OIDC linkage permanently disables magic-link. |
| `on_username_claimed` | When a NULL-username user picks one. Audit visibility for the share-modal autocomplete suddenly showing a new entry. |
| `on_credential_revoked` | When a user removes their password or unlinks OIDC. Reverses the eligibility decision. |

These are doc-only; their absence doesn't block anything.

## Doc skeleton — `auth-model.md`

The PR 21 deliverable. Headings only:

1. Why this page
2. Identity model (email, username, credentials)
3. Credential slots and the eligibility derivation
4. Login paths
   - Username + password
   - Email + password
   - Email + magic-link (with the 3-branch table)
   - OIDC redirect
5. Login dispatcher — how the input is interpreted (the `@`-in-input rule)
6. Registration paths
   - Email + password
   - Email-only
   - OIDC JIT
   - Email invitation
7. Anti-enumeration — what each endpoint returns
8. Security trade-offs
   - Mailbox-as-bypass when `OPEN_TO_PASSWORD_USERS=true`
   - Why OIDC is unconditionally excluded
   - No native 2FA today; how OIDC delegation provides MFA via IdPs
   - Rate-limit caps
9. Audit events — table
10. Migration path for existing instances
11. Future direction — per-user `login_strategy` (sketch with the 7-row matrix)
12. What is deliberately out of scope
13. Related documents (cross-refs)

Target: ~250 lines, big-picture, no code walkthroughs (in line with `magic-link-auth.md`).

---

**Status**: ready to start at PR 16 on confirmation. No code written yet. No tasks created in the TaskCreate system — those happen per-PR when implementation begins.
