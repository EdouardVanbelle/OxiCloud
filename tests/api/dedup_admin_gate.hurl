# =============================================================
# OxiCloud — Dedup admin gate + URL move
# =============================================================
# Regression pin for AuthZ audit #24 + #25 (2026-07-12).
#
# `dedup_handler.rs` previously rolled its own admin check on
# `/api/dedup/stats` and `/api/dedup/recalculate` — a bespoke
# `if auth_user.role != "admin" { 403 with hand-rolled JSON }`
# with no audit line on rejection. That's the same drift class
# the admin middleware layer refactor closed elsewhere on
# 2026-07-17.
#
# Fix:
#   1. Both endpoints moved to `/api/admin/dedup/*` where the
#      `/api/admin` middleware gate covers them by construction.
#      URL declares admin intent up front.
#   2. Inline role check removed from the handlers — reaching
#      them at all means the caller is admin.
#   3. `recalculate` emits `dedup.integrity_recalculated` on
#      success (audit #25). Not asserted here (no log-scrape
#      harness in Hurl); the shape is pinned in the handler
#      code and covered by the `audit` tracing target contract.
#
# This test pins:
#   * Admin can hit both endpoints at the new URL → 200.
#   * Non-admin (bob) hits both → 403 (middleware layer).
#   * The OLD URLs `/api/dedup/stats` and `/api/dedup/recalculate`
#     are no longer registered → 404. Trips if someone
#     re-introduces the routes to `dedup_router` without also
#     removing them from `admin_handler::admin_routes()`.
# =============================================================


# ─────────────────────────────────────────────────────────────
# Setup — admin login + bob (re-)provisioning.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/auth/login
Content-Type: application/json
{ "username": "{{username}}", "password": "{{password}}" }

HTTP 200
[Captures]
admin_token: jsonpath "$.access_token"


# Anti-enum registration.
POST {{base_url}}/api/auth/register
Content-Type: application/json
{
  "username": "dedup_bob",
  "email":    "dedup_bob@example.com",
  "password": "DedupBobPassword1!"
}

HTTP 200


POST {{base_url}}/api/auth/login
Content-Type: application/json
{ "username": "dedup_bob", "password": "DedupBobPassword1!" }

HTTP 200
[Captures]
bob_token: jsonpath "$.access_token"


# ─────────────────────────────────────────────────────────────
# Step 1 — Admin can hit the new URL. `stats` returns a
#          `StatsResponse`-shaped body.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/admin/dedup/stats
Authorization: Bearer {{admin_token}}

HTTP 200
[Asserts]
jsonpath "$.unique_blobs"        isNumber
jsonpath "$.total_references"    isNumber
jsonpath "$.bytes_saved"         isNumber
jsonpath "$.total_logical_bytes"  isNumber
jsonpath "$.total_physical_bytes" isNumber


# ─────────────────────────────────────────────────────────────
# Step 2 — Admin can trigger the integrity recalculation.
#          Response shape mirrors `stats`. Server-side, this
#          also emits the `dedup.integrity_recalculated` audit
#          event (not asserted from Hurl).
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/admin/dedup/recalculate
Authorization: Bearer {{admin_token}}

HTTP 200
[Asserts]
jsonpath "$.unique_blobs"     isNumber
jsonpath "$.total_references" isNumber


# ─────────────────────────────────────────────────────────────
# Step 3 — Bob (non-admin) is denied. The `/api/admin/*`
#          middleware layer emits `AuthError::AccessDenied` →
#          403. No hand-rolled 403 body from the handler; the
#          handler doesn't even run.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/admin/dedup/stats
Authorization: Bearer {{bob_token}}

HTTP 403


POST {{base_url}}/api/admin/dedup/recalculate
Authorization: Bearer {{bob_token}}

HTTP 403


# ─────────────────────────────────────────────────────────────
# Step 4 — The old URLs are no longer registered. Trips if a
#          future refactor re-adds them to `dedup_router` without
#          removing them from `admin_handler::admin_routes()` (or
#          vice versa). Anti-enum catch-all in the `/api/*` router
#          returns 404 for unknown paths.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/dedup/stats
Authorization: Bearer {{admin_token}}

HTTP 404


POST {{base_url}}/api/dedup/recalculate
Authorization: Bearer {{admin_token}}

HTTP 404
