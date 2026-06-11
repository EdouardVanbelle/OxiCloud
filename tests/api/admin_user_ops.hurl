# =============================================================
# OxiCloud — Baseline: admin user-mutation surface
# =============================================================
# Exercises the cluster of admin-only `PUT /api/admin/users/{id}/*`
# endpoints that operators rely on for incident response:
#   - disable a compromised account
#   - rotate its password
#   - change its role
#   - set / clear its quota
#
# Each is a one-shot mutation, but the failure mode of any one
# is severe (operator can't lock out an attacker, can't reset a
# password). Pinning them together keeps the cluster intact
# under refactors.
#
# Coverage:
#   1. Admin creates a fresh fixture user via POST /api/admin/users
#   2. Fixture user logs in successfully (baseline)
#   3. PUT /quota → fixture user's /me reports updated quota
#   4. PUT /role  → fixture user becomes admin
#   5. PUT /password (admin reset) → old password no longer works,
#      new password works
#   6. PUT /active=false → fixture user login → 403
#   7. PUT /active=true  → fixture user login works again
#   8. Cleanup via DELETE /api/admin/users/{id}
# =============================================================


# ─────────────────────────────────────────────────────────────
# Setup — admin login
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/auth/login
Content-Type: application/json
{ "username": "{{username}}", "password": "{{password}}" }

HTTP 200
[Captures]
admin_token: jsonpath "$.access_token"


# ─────────────────────────────────────────────────────────────
# 1 — Admin creates the fixture user "charlie-ops"
#     Uses a name that doesn't collide with charlie in
#     registration.hurl (which uses just "charlie"), so this
#     file is order-independent.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/admin/users
Authorization: Bearer {{admin_token}}
Content-Type: application/json
{
  "username": "charlie-ops",
  "password": "OriginalPassword1!",
  "email":    "charlie-ops@example.com",
  "role":     "user"
}

HTTP 201
[Captures]
charlie_id: jsonpath "$.id"


# ─────────────────────────────────────────────────────────────
# 2 — Baseline: fixture user can log in with the password
#     admin assigned.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/auth/login
Content-Type: application/json
{ "username": "charlie-ops", "password": "OriginalPassword1!" }

HTTP 200
[Captures]
charlie_token_v1: jsonpath "$.access_token"


# ─────────────────────────────────────────────────────────────
# 3 — Set a non-default quota; charlie's own /me must reflect it.
#     200 MiB = 209715200 bytes — keeps the assertion exact while
#     still being a believable per-user cap.
# ─────────────────────────────────────────────────────────────
PUT {{base_url}}/api/admin/users/{{charlie_id}}/quota
Authorization: Bearer {{admin_token}}
Content-Type: application/json
{ "quota_bytes": 209715200 }

HTTP 200

GET {{base_url}}/api/auth/me
Authorization: Bearer {{charlie_token_v1}}

HTTP 200
[Asserts]
jsonpath "$.storage_quota_bytes" == 209715200


# ─────────────────────────────────────────────────────────────
# 4 — Promote charlie to admin. After this the /me payload's
#     role field must reflect the change.
# ─────────────────────────────────────────────────────────────
PUT {{base_url}}/api/admin/users/{{charlie_id}}/role
Authorization: Bearer {{admin_token}}
Content-Type: application/json
{ "role": "admin" }

HTTP 200

GET {{base_url}}/api/auth/me
Authorization: Bearer {{charlie_token_v1}}

HTTP 200
[Asserts]
jsonpath "$.role" == "admin"


# ─────────────────────────────────────────────────────────────
# 5 — Admin resets charlie's password.
# ─────────────────────────────────────────────────────────────
PUT {{base_url}}/api/admin/users/{{charlie_id}}/password
Authorization: Bearer {{admin_token}}
Content-Type: application/json
{ "new_password": "AdminResetPassword2!" }

HTTP 200

# Old password no longer works. Login failures map to 403
# (AccessDenied) in this codebase — both "invalid credentials"
# and "account deactivated" go through the same error kind.
POST {{base_url}}/api/auth/login
Content-Type: application/json
{ "username": "charlie-ops", "password": "OriginalPassword1!" }

HTTP 403

# New password works.
POST {{base_url}}/api/auth/login
Content-Type: application/json
{ "username": "charlie-ops", "password": "AdminResetPassword2!" }

HTTP 200
[Captures]
charlie_token_v2: jsonpath "$.access_token"


# ─────────────────────────────────────────────────────────────
# 6 — Disable the account. The next login attempt must report
#     403 (account disabled) — distinct from 401 (bad creds)
#     so operators can tell "I locked you out" from "you typed
#     the wrong password".
# ─────────────────────────────────────────────────────────────
PUT {{base_url}}/api/admin/users/{{charlie_id}}/active
Authorization: Bearer {{admin_token}}
Content-Type: application/json
{ "active": false }

HTTP 200

POST {{base_url}}/api/auth/login
Content-Type: application/json
{ "username": "charlie-ops", "password": "AdminResetPassword2!" }

HTTP 403


# ─────────────────────────────────────────────────────────────
# 7 — Re-enable; login works again.
# ─────────────────────────────────────────────────────────────
PUT {{base_url}}/api/admin/users/{{charlie_id}}/active
Authorization: Bearer {{admin_token}}
Content-Type: application/json
{ "active": true }

HTTP 200

POST {{base_url}}/api/auth/login
Content-Type: application/json
{ "username": "charlie-ops", "password": "AdminResetPassword2!" }

HTTP 200


# ─────────────────────────────────────────────────────────────
# 8 — Teardown
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/admin/users/{{charlie_id}}
Authorization: Bearer {{admin_token}}

HTTP 200
