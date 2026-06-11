# =============================================================
# OxiCloud — Baseline: search surface
# =============================================================
# Pins `/api/search` and `/api/search/suggest` plus the
# cross-user isolation property: a search MUST NEVER surface a
# file the caller doesn't own (and isn't shared with). Search
# is the kind of feature where a sloppy SQL join is exactly
# what introduces a cross-user leak — this test catches that.
#
# Requires OXICLOUD_ENABLE_SEARCH=true (set in tests/common/server.env).
#
# Coverage:
#   1. Admin uploads `unique-search-needle-aaa.txt` to her home
#   2. GET /api/search?query=unique-search-needle returns the file
#   3. GET /api/search?query=does-not-exist-xyz   returns 0 files
#   4. GET /api/search/suggest?query=unique-search-needle returns
#      something (suggestion-shape is allowed to be permissive)
#   5. Cross-user: bob searches "unique-search-needle" → MUST NOT
#      see admin's file (security baseline)
#   6. Teardown: delete the file
#
# Bob is (re-)created inline so this file is order-independent
# with respect to nc_second_user_setup.hurl (which runs later
# in run.sh).
# =============================================================


# ─────────────────────────────────────────────────────────────
# Setup — admin login + bob (re-)provisioning
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/auth/login
Content-Type: application/json
{ "username": "{{username}}", "password": "{{password}}" }

HTTP 200
[Captures]
admin_token: jsonpath "$.access_token"


GET {{base_url}}/api/folders
Authorization: Bearer {{admin_token}}

HTTP 200
[Captures]
admin_home_id: jsonpath "$[0].id"


# Anti-enum registration: 200 whether bob existed or not.
POST {{base_url}}/api/auth/register
Content-Type: application/json
{
  "username": "bob",
  "email":    "bob@example.com",
  "password": "BobPassword1!"
}

HTTP 200


POST {{base_url}}/api/auth/login
Content-Type: application/json
{ "username": "bob", "password": "BobPassword1!" }

HTTP 200
[Captures]
bob_token: jsonpath "$.access_token"


# ─────────────────────────────────────────────────────────────
# 1 — Admin uploads `hello.txt` to a dedicated subfolder, then
#     renames it to a deliberately unique name so the search
#     assertion is unambiguous. The subfolder isolates this
#     test from any other test that already left a `hello.txt`
#     in admin's home (would otherwise 409).
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/folders
Authorization: Bearer {{admin_token}}
Content-Type: application/json
{ "name": "search-basic-test", "parent_id": "{{admin_home_id}}" }

HTTP 201
[Captures]
search_folder_id: jsonpath "$.id"


POST {{base_url}}/api/files/upload
Authorization: Bearer {{admin_token}}
[MultipartFormData]
folder_id: {{search_folder_id}}
file: file,fixtures/hello.txt; text/plain

HTTP 201
[Captures]
needle_file_id: jsonpath "$.id"


PUT {{base_url}}/api/files/{{needle_file_id}}/rename
Authorization: Bearer {{admin_token}}
Content-Type: application/json
{ "name": "unique-search-needle-aaa.txt" }

HTTP 200


# ─────────────────────────────────────────────────────────────
# 2 — Search hits the seeded file by substring of its name.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/search?query=unique-search-needle
Authorization: Bearer {{admin_token}}

HTTP 200
[Asserts]
jsonpath "$.files"        count >= 1
body contains "{{needle_file_id}}"


# ─────────────────────────────────────────────────────────────
# 3 — A search for a phrase that can't match anything must
#     return an empty result set, NOT an error. Empty-results
#     is a hot path; we don't want it to start 500ing.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/search?query=does-not-exist-xyz-zzz-9999
Authorization: Bearer {{admin_token}}

HTTP 200
[Asserts]
jsonpath "$.files"   count == 0
jsonpath "$.folders" count == 0


# ─────────────────────────────────────────────────────────────
# 4 — Suggest returns a usable payload (shape is permissive —
#     just confirm the endpoint serves 200 and isn't truncating
#     to an error envelope).
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/search/suggest?query=unique-search-needle
Authorization: Bearer {{admin_token}}

HTTP 200


# ─────────────────────────────────────────────────────────────
# 5 — HEADLINE: bob MUST NOT see admin's file. If this assertion
#     ever flips, the search service has a cross-user leak.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/search?query=unique-search-needle
Authorization: Bearer {{bob_token}}

HTTP 200
[Asserts]
body not contains "unique-search-needle"
body not contains "{{needle_file_id}}"


# ─────────────────────────────────────────────────────────────
# 6 — Teardown: removing the folder recursively takes the file
#     with it, so a single DELETE is enough.
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/folders/{{search_folder_id}}
Authorization: Bearer {{admin_token}}

HTTP 204
