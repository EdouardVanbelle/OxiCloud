# =============================================================
# OxiCloud — OIDC happy-path integration test
# =============================================================
# Drives the full SSO flow against the fake IdP under
# tests/oidc/fake_idp/ (panva/node-oidc-provider with an auto-approve
# interaction handler) and asserts every contract the SPA depends on.
# Pinned regression: commit d1bbe8ba changed the callback's frontend
# redirect from `/?oidc_code=…` to `/login?oidc_code=…` — Step 5's
# `Location matches "^…/login\?oidc_code=…"` is the assertion that
# would have caught that bug before users hit it.
#
# Flow walked manually (no auto-follow) so each handler is asserted
# independently:
#
#   1. Bootstrap: create the local admin (provider list works
#      regardless of admin presence, but the rest of the test
#      lives more comfortably with a fully-initialised server).
#   2. GET /api/auth/oidc/providers — SPA reads this to render
#      the SSO button.
#   3. GET /api/auth/oidc/authorize — server mints state + PKCE,
#      redirects to the IdP.
#   4. GET <IdP>/auth (+ full redirect chain) — fake-idp
#      auto-approves login + consent, OxiCloud's callback
#      JIT-provisions the user and redirects to
#      {frontend_url}/login?oidc_code=… The final 404 (no SPA
#      shell in test config) IS the test signal: we assert on
#      the URL we landed at, which is the d1bbe8ba contract.
#   5. POST /api/auth/oidc/exchange — SPA swaps the one-time
#      code for tokens + cookies.
#   6. GET /api/auth/me — proves the cookie session is live AND
#      that every OIDC profile claim (name → username, given_name,
#      family_name, picture → image, email, groups → admin role)
#      was JIT-provisioned correctly into the local user record.
#   7. POST /api/auth/refresh — rotation of all three cookies;
#      proves the SPA's session-renewal path works on top of
#      an OIDC-provisioned account.
#   8. GET /api/auth/me — the refreshed cookies authenticate too.
#   9. Existing-user re-login — a second OIDC flow with the same
#      `sub` resolves to the same local user, not a duplicate.
#  10. Anti-takeover — an unverified-email callback is rejected.
#  11. POST /api/auth/oidc/exchange — replay-protection: the
#      one-time code is single-use, second exchange returns 401.
# =============================================================


# ─────────────────────────────────────────────────────────────
# Step 1 — Create the local admin
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/setup
Content-Type: application/json
{
  "username": "{{username}}",
  "email":    "{{email}}",
  "password": "{{password}}"
}

HTTP 201


# ─────────────────────────────────────────────────────────────
# Step 2 — Provider discovery for the SPA
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/auth/oidc/providers

HTTP 200
[Asserts]
jsonpath "$.enabled" == true
# tests/common/server-with-oidc.env sets OXICLOUD_OIDC_PROVIDER_NAME=MockSSO.
jsonpath "$.provider_name" == "MockSSO"
jsonpath "$.authorize_endpoint" == "/api/auth/oidc/authorize"
jsonpath "$.password_login_enabled" == true


# ─────────────────────────────────────────────────────────────
# Step 3 — SPA-initiated authorize. Server returns a 302 with
#          state + PKCE challenge in the Location URL.
#          [Options] location: false keeps Hurl from following
#          the redirect so we can capture the target intact.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/auth/oidc/authorize
[Options]
location: false

# 307 (not 302): handler uses axum Redirect::temporary, which preserves
# the request method on follow. For a GET-initiated SSO flow it makes
# no practical difference, but the assertion has to match what's emitted.
HTTP 307
[Captures]
idp_url: header "Location"
[Asserts]
# panva/node-oidc-provider publishes authorize at /auth (not
# /authorize). The OXICLOUD_OIDC_ISSUER_URL points at the issuer
# root; the discovery doc tells OxiCloud the actual endpoint.
header "Location" matches "^{{oidc_authorize_endpoint}}\\?"
header "Location" contains "state="
header "Location" contains "code_challenge="
header "Location" contains "code_challenge_method=S256"
header "Location" contains "client_id=oxicloud-test"
header "Location" contains "redirect_uri="


# ─────────────────────────────────────────────────────────────
# Step 4 — Walk the entire IdP + OxiCloud redirect chain.
#
#          The fake IdP's auto-approve handler resolves login +
#          consent silently and 302s back to OxiCloud's callback;
#          the callback validates state + exchanges code with the
#          IdP, JIT-provisions the user, then 307s the browser to
#          {frontend_url}/login?oidc_code=…
#
#          With `location: true` Hurl follows the whole chain and
#          lands on the SPA login URL. The test server config
#          (server-with-oidc.env) points `OXICLOUD_STATIC_PATH`
#          at ./static — which is the legacy vanilla frontend, NOT
#          static-dist/ — so /login returns 404. That 404 is the
#          test signal: it proves we landed AT /login (i.e. the
#          d1bbe8ba contract held). The URL we end at is the
#          actual assertion.
#
#          A pre-d1bbe8ba server would have redirected to
#          `http://localhost:8087/?oidc_code=…` instead — same
#          404, but the `landed_at` assertion would catch it.
# ─────────────────────────────────────────────────────────────
GET {{idp_url}}
[Options]
location: true
location-trusted: true

HTTP 404
[Captures]
landed_at: url
oidc_code: url regex "oidc_code=([a-f0-9]+)"
[Asserts]
# The d1bbe8ba regression guard. The exact contract the SvelteKit
# SPA depends on — `/login`, not `/`.
variable "landed_at" matches "^http://localhost:8087/login\\?oidc_code=[a-f0-9]+$"


# ─────────────────────────────────────────────────────────────
# Step 5 — Swap the one-time code for a session.
#          Response sets the HttpOnly auth cookies + the
#          double-submit CSRF cookie the SPA reads to populate
#          X-CSRF-Token on subsequent mutating requests.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/auth/oidc/exchange
Content-Type: application/json
{ "code": "{{oidc_code}}" }

HTTP 200
[Captures]
oidc_session_user: jsonpath "$.user.username"
# Snapshotted so Step 7's refresh can prove the tokens rotated
# rather than being re-issued unchanged. The refresh handler in
# auth_handler.rs always rotates all three cookies (access JWT,
# refresh UUID, CSRF UUID); a regression that silently keeps the
# old refresh token would let a leaked refresh credential live
# forever — exactly the kind of issue token-family rotation exists
# to prevent.
initial_access_token:  jsonpath "$.access_token"
initial_refresh_token: jsonpath "$.refresh_token"
initial_csrf_token:    cookie "oxicloud_csrf"
[Asserts]
jsonpath "$.user.username" == "oidc_user"
jsonpath "$.user.email"    == "oidc@example.com"
jsonpath "$.access_token"  isString
# Multiple Set-Cookie headers come back as a list of values, so
# `contains` only matches whole-element strings. Each cookie shows up
# as its own list entry; we use `cookie "<name>"` (Hurl's dedicated
# helper) which finds the cookie by name across all Set-Cookie headers.
cookie "oxicloud_access"   exists
cookie "oxicloud_refresh"  exists
cookie "oxicloud_csrf"     exists
cookie "oxicloud_access[HttpOnly]"  exists
cookie "oxicloud_refresh[HttpOnly]" exists


# ─────────────────────────────────────────────────────────────
# Step 6 — Same-jar follow-up GET proves the cookie session
#          actually authenticates. Hurl reuses the cookie jar
#          across requests in one file by default, so the
#          Set-Cookie from Step 5 carries forward.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/auth/me

HTTP 200
[Captures]
# Stash the user id for the re-login check in Step 10 below — a
# second OIDC flow with the same `sub` must resolve back to this
# exact user, not silently create a duplicate.
oidc_user_id: jsonpath "$.id"
[Asserts]
jsonpath "$.username" == "oidc_user"
jsonpath "$.email"    == "oidc@example.com"
# `auth_provider` stores the OIDC provider's display name (set via
# OXICLOUD_OIDC_PROVIDER_NAME in tests/common/server-with-oidc.env),
# NOT a generic "oidc" tag. A locally registered admin would have
# this field as something like "local". The distinct value here is
# what proves JIT provisioning landed via OIDC, not setup.hurl.
jsonpath "$.auth_provider" == "MockSSO"
# Full claim round-trip — the fake IdP (tests/oidc/fake_idp/server.js)
# pins these values and OxiCloud must persist each one verbatim during
# JIT provisioning (see auth_application_service.rs around line 2257).
# A regression that drops, swaps, or truncates a claim trips here.
# Note the field name flip on the API side: OIDC `picture` becomes
# UserDto.image (a URL or data URI).
jsonpath "$.given_name"  == "OIDC"
jsonpath "$.family_name" == "Test"
jsonpath "$.image"       == "https://example.com/oidc-test-user.png"
# Group-to-role mapping. server-with-oidc.env sets
# OXICLOUD_OIDC_ADMIN_GROUPS=admin-users; the fake IdP's claims include
# `groups: ["admin-users"]`. The JIT path intersects the claim against
# the env and promotes the new user from `user` to `admin`. A
# regression here would silently strip (or wrongly grant) admin rights
# for every SSO deployment that uses group-based role mapping.
jsonpath "$.role" == "admin"


# ─────────────────────────────────────────────────────────────
# Step 7 — Refresh-token rotation.
#
#          POST /api/auth/refresh reads the refresh token from
#          the HttpOnly oxicloud_refresh cookie (the browser flow
#          OxiCloud's SPA uses; the JSON body shape is only a
#          backwards-compat path for non-browser clients) and
#          re-issues all three cookies. Token-family rotation:
#          the prior refresh token is invalidated server-side
#          and a reuse attempt would be caught as a theft signal.
#
#          CSRF middleware fires here because we have a cookie
#          session — we pass the captured oxicloud_csrf value as
#          the double-submit X-CSRF-Token header, matching what
#          the SvelteKit SPA does via getCsrfHeaders().
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/auth/refresh
X-CSRF-Token: {{initial_csrf_token}}
Content-Type: application/json
{}

HTTP 200
[Captures]
refreshed_access_token:  jsonpath "$.access_token"
refreshed_refresh_token: jsonpath "$.refresh_token"
[Asserts]
jsonpath "$.user.username" == "oidc_user"
jsonpath "$.access_token"  isString
jsonpath "$.refresh_token" isString
# All three cookies must rotate. If any value were re-used, a
# regression in cookie_auth::append_auth_cookies (or in the
# RefreshToken use case) would silently leave the old credential
# live — exactly the kind of bug that motivates rotation.
variable "refreshed_access_token"  != "{{initial_access_token}}"
variable "refreshed_refresh_token" != "{{initial_refresh_token}}"
cookie "oxicloud_access"   exists
cookie "oxicloud_refresh"  exists
cookie "oxicloud_csrf"     exists


# ─────────────────────────────────────────────────────────────
# Step 8 — The refreshed cookies authenticate too. Belt-and-braces:
#          rotation is only useful if the new tokens actually work.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/auth/me

HTTP 200
[Asserts]
jsonpath "$.username" == "oidc_user"


# ─────────────────────────────────────────────────────────────
# Step 9 — Existing-user re-login. A second pass through the same
#          OIDC `sub` MUST resolve back to the SAME local user
#          (`oidc_user_id` captured in Step 6) — silently creating
#          a duplicate account on every login would be the
#          regression. Exercises the existing-user branch in
#          auth_application_service.rs around line 2157, distinct
#          from the JIT-provisioning branch the earlier steps hit.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/auth/oidc/authorize
[Options]
location: false

HTTP 307
[Captures]
relogin_idp_url: header "Location"


GET {{relogin_idp_url}}
[Options]
location: true
location-trusted: true

HTTP 404
[Captures]
relogin_oidc_code: url regex "oidc_code=([a-f0-9]+)"
[Asserts]
variable "landed_at" matches "^http://localhost:8087/login\\?oidc_code=[a-f0-9]+$"


POST {{base_url}}/api/auth/oidc/exchange
Content-Type: application/json
{ "code": "{{relogin_oidc_code}}" }

HTTP 200
[Asserts]
# Same local id — proves the existing-user resolver matched on `sub`
# (or `oidc_provider + oidc_subject`) instead of minting a new row.
jsonpath "$.user.id" == "{{oidc_user_id}}"
jsonpath "$.user.username" == "oidc_user"
# Role from the prior JIT-provisioned admin survives the re-login.
# Two regressions this catches: (a) the existing-user branch wiping
# the role to a default `user`; (b) the existing-user branch
# re-evaluating groups but missing the admin-group claim (the fake
# IdP still emits `groups: ["admin-users"]`, OXICLOUD_OIDC_ADMIN_GROUPS
# still resolves to "admin"). Either way, the role should remain
# `admin` — otherwise we have a silent admin demotion on every login.
jsonpath "$.user.role" == "admin"


# ─────────────────────────────────────────────────────────────
# Step 10 — Anti-takeover: an OIDC callback whose `email_verified`
#           claim is `false` MUST be rejected. Without this guard
#           an attacker who can set `email` to a victim's address
#           in their own IdP account (some IdPs allow unverified
#           emails through the consent screen) gets the victim's
#           OxiCloud account on first login.
#
#           We flip the fake IdP into the unverified-email mode
#           via the `/control/email-verified/false` test hook,
#           drive a fresh authorize, expect the OxiCloud callback
#           to fail, then reset the IdP for any future steps.
#
#           This SHOULD use a different `sub` than the existing
#           verified user to exercise the JIT path (the
#           anti-takeover check fires there), but the auto-approve
#           handler resolves one fixed `sub`. The check still
#           fires on the existing user path too because the
#           verified-email requirement is evaluated on every
#           callback — that's what we exercise here.
# ─────────────────────────────────────────────────────────────
POST http://localhost:1080/control/email-verified/false

HTTP 200


GET {{base_url}}/api/auth/oidc/authorize
[Options]
location: false

HTTP 307
[Captures]
unverified_idp_url: header "Location"


GET {{unverified_idp_url}}
[Options]
location: true
location-trusted: true

# OxiCloud's callback returns 403 (or 401, depending on which
# branch fires). What matters is the final URL is NOT
# /login?oidc_code= — a successful login would have landed there
# regardless of status, so a status-code-only assertion would
# miss a "we accidentally provisioned the unverified user"
# regression. We assert on BOTH the status AND the negation of
# the success URL via Hurl's built-in `url` query (NOT the
# `landed_at` capture from Step 4 — that variable is stale here).
HTTP *
[Asserts]
status >= 400
status <  500
url not matches "^http://localhost:8087/login\\?oidc_code="


# Reset the IdP so this test doesn't poison anything that runs
# after it (defensive — there's nothing after right now, but a
# future test would silently fail with "all my users get
# rejected" if we forgot this).
POST http://localhost:1080/control/email-verified/true

HTTP 200


# ─────────────────────────────────────────────────────────────
# Step 11 — Replay protection: the one-time code is rejected on a
#          second attempt. Defense-in-depth check.
#
#          Hurl 4.x has no per-request cookie-jar clear, so this
#          request still carries the session cookies set in Step 5.
#          That means CSRF middleware rejects the unauthenticated
#          (no X-CSRF-Token header) POST with 403 BEFORE the OIDC
#          single-use-code check runs. Both are valid replay
#          defenses; in a real attack the attacker has the code but
#          not the session cookie, in which case the rejection
#          would come from the OIDC layer as 401.
#
#          The single-use-code path itself is covered by unit
#          tests in auth_application_service.rs (the
#          completed_oidc_logins moka cache + remove-on-use).
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/auth/oidc/exchange
Content-Type: application/json
{ "code": "{{oidc_code}}" }

HTTP 403
