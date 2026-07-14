# Authentication

OxiCloud ships with JWT-based authentication and Argon2id password hashing for local accounts. It also exposes status and OIDC-related auth endpoints under the same `/api/auth` namespace, plus a magic-link (email link) sign-in flow for accounts that don't use a password.

## Core Endpoints

| Method | Endpoint | Description |
| --- | --- | --- |
| `POST` | `/api/auth/register` | Create a local user account. `email` is required; `username` and `password` are both optional. |
| `POST` | `/api/auth/login` | Exchange an identifier (username **or** email — dispatches on `@`) and password for access and refresh tokens |
| `POST` | `/api/auth/magic-link/send` | Send a one-click sign-in link to the account's email. Accepts either a username or an email in the request body |
| `GET` | `/magic/v1/{token}` | Redeem a magic-link — creates a session and stamps `email_verified_at` on the account |
| `POST` | `/api/auth/refresh` | Refresh the session tokens |
| `GET` | `/api/auth/me` | Return the current authenticated user |
| `PUT` | `/api/auth/change-password` | Change the current user's password (requires the current password) |
| `POST` | `/api/auth/logout` | Invalidate the current session |
| `GET` | `/api/auth/status` | Return auth system state, including OIDC availability |

## OIDC Endpoints Under Auth

| Method | Endpoint | Description |
| --- | --- | --- |
| `GET` | `/api/auth/oidc/providers` | Report which self-service auth methods this deployment offers (see fields below) |
| `GET` | `/api/auth/oidc/authorize` | Build the authorization redirect URL |
| `GET` | `/api/auth/oidc/callback` | Handle provider redirect callback |
| `POST` | `/api/auth/oidc/exchange` | Exchange the auth code for OxiCloud session tokens |

`GET /api/auth/oidc/providers` fields:

| Field | Meaning |
| --- | --- |
| `enabled` | OIDC is configured on this deployment |
| `provider_name` | Display name for the IdP (shown on the SSO button) |
| `authorize_endpoint` | Where the SPA should start the OIDC round-trip |
| `password_login_enabled` | `POST /api/auth/login` will accept credentials |
| `magic_link_login_enabled` | `POST /api/auth/magic-link/send` will mint tokens (SMTP wired + allowlist + no OIDC — see rules below) |
| `require_verified_email` | `OXICLOUD_REQUIRE_VERIFIED_EMAIL` is set — the SPA uses this hint to explain `EmailNotVerified` responses |

## Configuring which methods are offered

Two environment variables control the self-service surface (OIDC is orthogonal — see `OXICLOUD_OIDC_ENABLED`).

### `OXICLOUD_AUTH_METHODS`

Comma-separated allowlist of `password` and/or `magic_link`. Default `password,magic_link`.

| Configuration | Effect |
| --- | --- |
| Unset or `password,magic_link` | Both methods allowed (default) |
| `password` | Password login OK. Magic-link send / redeem → 403 `MagicLinkLoginDisabled` |
| `magic_link` | Password login → 403 `PasswordLoginDisabled`. Password-based `register` → 403 `PasswordRegistrationDisabled`. Email-only signup still works |

**Startup gate.** If `magic_link` is the only method allowed AND no SMTP transport is configured (`OXICLOUD_SMTP_HOST` empty), the server refuses to start with a fatal message. A magic-link-only policy without a working mailer silently locks every user out.

**OIDC master rule.** When `OXICLOUD_OIDC_ENABLED=true`, magic-link login is **hard-disabled** regardless of this list. The IdP is the identity boundary; magic-link would bypass any 2FA / step-up policy the IdP enforces. The startup gate above does **not** trigger in this case — OIDC provides the login path.

Legacy alias: `OXICLOUD_OIDC_DISABLE_PASSWORD_LOGIN=true` still removes `password` from the effective allowlist.

### `OXICLOUD_REQUIRE_VERIFIED_EMAIL`

Default `false`. When `true`, `POST /api/auth/login` returns 403 `EmailNotVerified` for any account whose `email_verified_at IS NULL`.

**Order matters:** the verified-email check runs **after** password validation. An attacker without the password sees only the generic `Invalid credentials` shape — they can't probe whether an account's email is verified.

**Verification piggyback.** When the branch fires (password OK, email unverified), the server auto-sends a verification magic-link to the account's registered address using the same login request. The user sees `EmailNotVerified` in the response and a "check your inbox" hint on the login page; resubmitting the form re-sends the link. This is why there is no separate "resend verification" endpoint — offering an unauthenticated one would leak `has_password` state.

**Admin exemption.** Admin accounts (role `admin`) are exempt from this gate at login, regardless of `email_verified_at`. Rationale: an operator who flips the flag on an existing deployment must not lock the admin(s) out of their own instance. Fresh admin accounts created via `POST /api/setup` or `POST /api/admin/users` are stamped verified at creation; the exemption covers pre-existing accounts that predate the flag.

**Auto-verified on creation:** OIDC-JIT users, admin-created users (`POST /api/admin/users`), and the first-run setup admin (`POST /api/setup`). Verification is only ever missing on regular users who signed up before the flag was turned on.

## Login identifier dispatch

`POST /api/auth/login` accepts either a username (no `@`) or an email (contains `@`) in the `username` field. The two namespaces are provably disjoint — usernames forbid `@` — so the dispatch is unambiguous and both paths return the same session shape.

`POST /api/auth/magic-link/send` mirrors this convention. The `email` field can be either an email or a username; the server resolves username → registered email before rate-limiting so both shapes share one budget (no bypass).

## Registration flow

Since PR 18, both `username` and `password` are optional on `POST /api/auth/register`. The only required field is `email`.

| Combination | Result |
| --- | --- |
| `email + password` | Classic signup — account gets a password hash; user can log in immediately |
| `email + password + username` | Same, plus the username is claimed at creation |
| `email` only | Email-only signup — no password stored; server sends a welcome magic-link. Clicking it creates a session and stamps `email_verified_at`. The user can later claim a handle via `PATCH /api/auth/me/profile` and set a password via `PUT /api/auth/change-password` |

The response body is uniform across success, email collision, and username collision — the SPA does not learn whether an address is already taken. The real reason lands in the audit log.

### `OXICLOUD_DISABLE_REGISTRATION`

Turns the endpoint off entirely (returns 403 `RegistrationDisabled`).

### `OXICLOUD_REGISTRATION_ALLOWED_EMAIL_DOMAINS`

Comma-separated allowlist. Rejected registrations return 403 `RegistrationDomainNotAllowed`. Distinct from `OXICLOUD_EXTERNAL_EMAIL_DOMAINS`, which gates external-user **invitations**; self-registration and invitations have independent policies.

## Magic-link eligibility

`POST /api/auth/magic-link/send` looks up the resolved email → user, then applies the eligibility ladder:

1. **OIDC-linked user** → refused with `reason="oidc_user"`. Unconditional; the IdP is the security boundary and may enforce MFA that magic-link would sidestep.
2. **Has a password configured** → refused with `reason="has_password"` (default). Set `OXICLOUD_AUTH_POLICIES=permit_magic_link_for_password_users` to allow — this weakens the password to mailbox-strength for affected accounts; opt-in only.
3. **No credential** (typical external user or fresh email-only signup) → allow.

The verification-piggyback flow above deliberately **bypasses the `has_password` gate** — that path is only reachable after the user has already proven identity via password on the same login request, so mailbox-only trust is not being extended beyond what the password already established.

## Auth policy vector

`OXICLOUD_AUTH_POLICIES` is a comma-separated list of additive policy switches. Distinct from `OXICLOUD_AUTH_METHODS` (which enables/disables a method wholesale), each entry here grants a specific exception or restriction to default auth behaviour. Vector shape so future policies can be added by appending a token instead of introducing a new env var per behaviour. Variant names carry their own polarity (`Permit...`, future `Require...` / `Deny...`).

| Token | Effect |
| --- | --- |
| `permit_magic_link_for_password_users` | Allow magic-link login for accounts that also have a password. OIDC-linked users are still refused. |

Unknown tokens are logged-and-skipped at startup so a typo doesn't silently zero the vector.

## Example Flows

### Register — classic

```json
{ "username": "testuser", "email": "test@example.com", "password": "SecurePassword123" }
```

### Register — email-only

```json
{ "email": "test@example.com" }
```

### Login

```json
{ "username": "testuser", "password": "SecurePassword123" }
```

Or equivalently:

```json
{ "username": "test@example.com", "password": "SecurePassword123" }
```

Typical successful login response:

```json
{ "accessToken": "...", "refreshToken": "...", "expiresIn": 3600 }
```

### Send a sign-in link (magic-link)

```json
{ "email": "testuser" }
```

Uniform response regardless of whether the account exists / is eligible:

```json
{ "message": "If an account exists for that email, a sign-in link will be sent." }
```

### Current User

`GET /api/auth/me` returns the authenticated user's identity, role, `email_verified_at`, and storage information.

## Distinguished error codes

The `error_type` field on 4xx responses lets frontends render specific UX. Codes surfaced by this subsystem:

| `error_type` | HTTP | Meaning |
| --- | --- | --- |
| `PasswordLoginDisabled` | 403 | `OXICLOUD_AUTH_METHODS` doesn't include `password` |
| `PasswordRegistrationDisabled` | 403 | Same, on `register` with a password field |
| `MagicLinkLoginDisabled` | 403 | `OXICLOUD_AUTH_METHODS` doesn't include `magic_link`, OIDC is enabled, or email-only signup is attempted on a password-only deployment |
| `EmailNotVerified` | 403 | Password validated, but `email_verified_at IS NULL` and `OXICLOUD_REQUIRE_VERIFIED_EMAIL=true`. Server has already sent a verification link |
| `RegistrationDisabled` | 403 | Global registration off |
| `RegistrationDomainNotAllowed` | 403 | Email domain outside `OXICLOUD_REGISTRATION_ALLOWED_EMAIL_DOMAINS` |
| `AccountLocked` | 429 | Too many failed login attempts for (account, IP) — see rate-limit config |

## Security Model

- Local passwords hashed with Argon2id
- Access control is role-based (`admin` and `user`)
- Refresh tokens support session renewal without forcing frequent re-login
- Login endpoint uses anti-enumeration response shapes — bad-username and bad-password return the same 403
- Magic-link `send` returns a uniform 200 whether the account exists or not; the truth lands in the `audit` log target
- OIDC can coexist with local auth or disable password login entirely
- OIDC-enabled deployments have magic-link login hard-disabled to prevent IdP-MFA bypass

## Related Pages

- [OIDC / SSO](/config/oidc)
- [Admin Settings](/config/admin-settings)
- [Environment Variables](/config/env)
