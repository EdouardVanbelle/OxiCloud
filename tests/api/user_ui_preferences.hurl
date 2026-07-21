# =============================================================
# OxiCloud — auth.users.ui_preferences round-trip
# =============================================================
# The `ui_preferences` JSONB column is the SPA's cross-device
# backing store for pure UI toggles (hide dotfiles, view mode,
# sidebar collapse, …). The server treats the contents as
# opaque; this suite pins the semantics of the PATCH surface
# so a future refactor can't silently break cross-device sync:
#
#   1. Fresh user starts with an empty object bag (`{}`), not
#      `null` and not missing from the response body.
#   2. PATCH does a SHALLOW merge — a partial write only
#      touches the keys it mentions; siblings survive. Load-
#      bearing invariant: without it, Device A's write would
#      silently wipe preferences Device B just set.
#   3. Sending `{key: null}` in the patch REMOVES that key
#      server-side (jsonb_strip_nulls after the merge). This
#      is the documented delete-a-key path.
#   4. Non-object patch shape is rejected with 400. Prevents
#      the endpoint from being a scratch scalar store and
#      catches malformed clients early.
#
# Not covered here (intentional):
#   • 16 KiB size cap — the CHECK is at the schema layer and
#     is exercised by unit tests without needing an integration
#     round-trip; constructing a 16 KiB JSON body in Hurl adds
#     line noise without meaningful signal.
#   • Concurrency safety of the shallow merge under two
#     simultaneous PATCHes — postgres' `||` operator is atomic
#     per row, so this is a DB-guarantee test rather than an
#     API test.
# =============================================================


# ─────────────────────────────────────────────────────────────
# Step 1 — Admin login. All PATCH/GET below use this token so
#          the same user's bag is under test.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/auth/login
Content-Type: application/json
{ "username": "{{username}}", "password": "{{password}}" }

HTTP 200
[Captures]
admin_token: jsonpath "$.access_token"


# ─────────────────────────────────────────────────────────────
# Step 2 — Fresh state: bag is present in the response and is
#          an empty object.
#
#          Note: if a PRIOR test in the API suite has already
#          PATCHed this user's ui_preferences, this step's
#          `count == 0` check would fail. Currently no other
#          test writes to `ui_preferences` — if a future test
#          does, it MUST clean up its keys at teardown
#          (`PATCH { key: null }`) to keep this baseline valid.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/auth/me
Authorization: Bearer {{admin_token}}

HTTP 200
[Asserts]
jsonpath "$.ui_preferences" exists
jsonpath "$.ui_preferences" isCollection
# Empty-object baseline. Neither `count == 0` on `.*` nor the
# `== {}` object-literal predicate are supported by this Hurl
# version. Fall back to a body-shape check on the serialised
# response — serde_json emits `"ui_preferences":{}` without
# whitespace inside the braces on Rust's default JSON writer,
# so this pins the empty-object serialisation reliably.
body contains "\"ui_preferences\":{}"


# ─────────────────────────────────────────────────────────────
# Step 3 — Write one key. Response echoes the merged bag with
#          the new key. Bumps updated_at (not asserted — it's
#          set by the repo unconditionally so no branch to pin).
# ─────────────────────────────────────────────────────────────
PATCH {{base_url}}/api/auth/me/profile
Authorization: Bearer {{admin_token}}
Content-Type: application/json
{ "ui_preferences": { "hide_dotfiles": true } }

HTTP 200
[Asserts]
jsonpath "$.ui_preferences.hide_dotfiles" == true


# ─────────────────────────────────────────────────────────────
# Step 4 — Write a SECOND key. Shallow merge must preserve the
#          first key. This is the load-bearing regression
#          assertion: a full-replacement bug here would show
#          `hide_dotfiles` missing from the response.
# ─────────────────────────────────────────────────────────────
PATCH {{base_url}}/api/auth/me/profile
Authorization: Bearer {{admin_token}}
Content-Type: application/json
{ "ui_preferences": { "view_mode": "grid" } }

HTTP 200
[Asserts]
jsonpath "$.ui_preferences.hide_dotfiles" == true
jsonpath "$.ui_preferences.view_mode"     == "grid"


# ─────────────────────────────────────────────────────────────
# Step 5 — GET reflects the merged state after the round-trip
#          (belt-and-braces — Step 4's PATCH response could
#          have been returning a computed value while the DB
#          state diverged; the fresh GET catches that).
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/auth/me
Authorization: Bearer {{admin_token}}

HTTP 200
[Asserts]
jsonpath "$.ui_preferences.hide_dotfiles" == true
jsonpath "$.ui_preferences.view_mode"     == "grid"


# ─────────────────────────────────────────────────────────────
# Step 6 — Null-value deletes the key. `hide_dotfiles` is
#          removed; `view_mode` stays. This exercises the
#          `jsonb_strip_nulls(bag || patch)` path in the repo.
# ─────────────────────────────────────────────────────────────
PATCH {{base_url}}/api/auth/me/profile
Authorization: Bearer {{admin_token}}
Content-Type: application/json
{ "ui_preferences": { "hide_dotfiles": null } }

HTTP 200
[Asserts]
jsonpath "$.ui_preferences.view_mode" == "grid"
# Deleted key must not survive as `null` — it must be absent
# (`jsonb_strip_nulls` in the repo strips it post-merge).
jsonpath "$.ui_preferences.hide_dotfiles" not exists


# ─────────────────────────────────────────────────────────────
# Step 7 — Non-object patch is rejected. Sending an array
#          would be a client bug or an abuse attempt (the bag
#          is documented as a JSON OBJECT). The schema CHECK
#          `users_ui_preferences_is_object` enforces at the DB
#          level; the service layer catches it earlier with a
#          400.
# ─────────────────────────────────────────────────────────────
PATCH {{base_url}}/api/auth/me/profile
Authorization: Bearer {{admin_token}}
Content-Type: application/json
{ "ui_preferences": [1, 2, 3] }

HTTP 400


# ─────────────────────────────────────────────────────────────
# Step 8 — Scalar patch is rejected (same class as array).
#          Both cases route through the same `patch.is_object()`
#          gate in `AuthApplicationService::update_profile`.
# ─────────────────────────────────────────────────────────────
PATCH {{base_url}}/api/auth/me/profile
Authorization: Bearer {{admin_token}}
Content-Type: application/json
{ "ui_preferences": "not-a-bag" }

HTTP 400


# ─────────────────────────────────────────────────────────────
# Teardown — restore the bag to empty so downstream tests
# don't inherit `view_mode`. Sending each surviving key with
# `null` deletes them via jsonb_strip_nulls, leaving `{}`.
# ─────────────────────────────────────────────────────────────
PATCH {{base_url}}/api/auth/me/profile
Authorization: Bearer {{admin_token}}
Content-Type: application/json
{ "ui_preferences": { "view_mode": null } }

HTTP 200
[Asserts]
# Same empty-object serialised shape as Step 2's baseline —
# `body contains "\"ui_preferences\":{}"` is the tightest empty
# check available on this Hurl version.
body contains "\"ui_preferences\":{}"
