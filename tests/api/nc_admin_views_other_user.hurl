# =============================================================
# OxiCloud — Baseline: admin views another user's OCS profile
# =============================================================
# C4 from BASELINE_TESTS_NC_WEBDAV.md.
#
# Post AuthZ audit #11 (2026-07-17), `user_provisioning_response`
# no longer rolls its own admin gate — it delegates to
# `AuthApplicationService::get_user_profile_by_username_with_perms`,
# which shares the visibility engine with the id-keyed REST
# endpoint at `/api/users/{id}`. Consequences for this test:
#
#   - **admin → bob**: still 200 (admin bypass is one of the
#     five visibility paths; see get_user_profile step 5).
#   - **bob → admin**: with `OXICLOUD_EXPOSE_SYSTEM_USERS=true`
#     (tests/common/server.env), both are internal so step 4
#     of the visibility engine says the target is broadly
#     visible via the system address book — bob CAN see
#     admin's basic profile. Pre-fix, the bespoke gate returned
#     `403 Insufficient privileges` and admin bypassed the
#     expose gate silently; both anomalies are gone.
#   - **bob → nonexistent**: `404 User not found`, anti-enum
#     shape identical to "you can't see this user". Audit line
#     `user_profile.rejected reason=target_username_not_found`
#     fires server-side.
#
# Uses admin's app password for Basic Auth (same pattern as
# `nc_ocs_user_info.hurl`).
# =============================================================


# ─────────────────────────────────────────────────────────────
# Setup 1 — JWT login as admin + mint NC app password.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/auth/login
Content-Type: application/json
{ "username": "{{username}}", "password": "{{password}}" }

HTTP 200
[Captures]
admin_jwt: jsonpath "$.access_token"

POST {{base_url}}/api/auth/app-passwords
Authorization: Bearer {{admin_jwt}}
Content-Type: application/json
{ "label": "nc_admin_views_other_user hurl test" }

HTTP 200
[Captures]
admin_nc_user:     jsonpath "$.username"
admin_nc_pw:       jsonpath "$.password"
admin_nc_pw_id:    jsonpath "$.id"


# ─────────────────────────────────────────────────────────────
# Setup 2 — JWT login as bob + mint NC app password.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/auth/login
Content-Type: application/json
{ "username": "bob", "password": "BobPassword1!" }

HTTP 200
[Captures]
bob_jwt: jsonpath "$.access_token"

POST {{base_url}}/api/auth/app-passwords
Authorization: Bearer {{bob_jwt}}
Content-Type: application/json
{ "label": "nc_admin_views_other_user hurl test (bob)" }

HTTP 200
[Captures]
bob_nc_user:    jsonpath "$.username"
bob_nc_pw:      jsonpath "$.password"
bob_nc_pw_id:   jsonpath "$.id"


# ─────────────────────────────────────────────────────────────
# C4-positive — admin CAN read bob's OCS provisioning profile
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/ocs/v1.php/cloud/users/bob?format=json
[BasicAuth]
{{admin_nc_user}}: {{admin_nc_pw}}

HTTP 200
[Asserts]
jsonpath "$.ocs.meta.statuscode" == 100
jsonpath "$.ocs.data.id"         == "bob"
jsonpath "$.ocs.data.email"      == "bob@example.com"


# ─────────────────────────────────────────────────────────────
# C4-symmetric — post-audit-#11: bob CAN read admin's profile
#                because the visibility engine's
#                `expose_system_users` branch treats internal
#                users as broadly visible via the system address
#                book. The bespoke `403 Insufficient privileges`
#                the pre-fix handler emitted is gone.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/ocs/v1.php/cloud/users/{{username}}?format=json
[BasicAuth]
{{bob_nc_user}}: {{bob_nc_pw}}

HTTP 200
[Asserts]
jsonpath "$.ocs.meta.statuscode" == 100
jsonpath "$.ocs.data.id"         == "{{username}}"


# ─────────────────────────────────────────────────────────────
# C4-antienum — bob queries a genuinely nonexistent username.
#               Response body is the SAME shape as any denial
#               case: `statuscode=404 status="failure"`. The
#               NC client cannot distinguish "user doesn't
#               exist" from "you have no visibility on that
#               user" (were expose_system_users off) — which
#               is the anti-enumeration invariant this fix
#               was meant to preserve.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/ocs/v1.php/cloud/users/nonexistent-audit-11-canary?format=json
[BasicAuth]
{{bob_nc_user}}: {{bob_nc_pw}}

HTTP 200
[Asserts]
jsonpath "$.ocs.meta.statuscode" == 404
jsonpath "$.ocs.meta.status"     == "failure"


# ─────────────────────────────────────────────────────────────
# Teardown — revoke both app passwords.
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/auth/app-passwords/{{admin_nc_pw_id}}
Authorization: Bearer {{admin_jwt}}
HTTP 200

DELETE {{base_url}}/api/auth/app-passwords/{{bob_nc_pw_id}}
Authorization: Bearer {{bob_jwt}}
HTTP 200
