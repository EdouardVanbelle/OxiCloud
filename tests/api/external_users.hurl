# =============================================================
# OxiCloud — invite-by-email + magic-link redemption (PR 9)
# =============================================================
# End-to-end: Alice shares a folder with bob@externalcompany.com,
# the server lazily provisions bob as an external user, sends the
# invitation through MockEmailSender, and bob redeems the magic
# link to land authenticated on the resource.
#
# Requires `OXICLOUD_SMTP_MOCK=true` in tests/common/server.env so
# the in-process capture endpoint at /api/admin/smtp/test/captured
# is mounted. The .hurl file would error on a real SMTP setup
# because the magic link wouldn't be retrievable.
# =============================================================


# ─────────────────────────────────────────────────────────────
# Step 1 — Alice logs in (admin) and grabs her home folder id.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/auth/login
Content-Type: application/json
{ "username": "{{username}}", "password": "{{password}}" }

HTTP 200
[Captures]
alice_token: jsonpath "$.access_token"

GET {{base_url}}/api/folders
Authorization: Bearer {{alice_token}}

HTTP 200
[Captures]
alice_home_id: jsonpath "$[0].id"


# ─────────────────────────────────────────────────────────────
# Step 2 — Alice creates a folder she's about to share by email.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/folders
Authorization: Bearer {{alice_token}}
Content-Type: application/json
{ "name": "ext-share", "parent_id": "{{alice_home_id}}" }

HTTP 201
[Captures]
ext_folder_id: jsonpath "$.id"


# ─────────────────────────────────────────────────────────────
# Step 3 — Alice shares with bob@externalcompany.com via the new
#           subject.type=email payload. Server lazily provisions
#           bob as an external user.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/grants
Authorization: Bearer {{alice_token}}
Content-Type: application/json
{
  "subject":  { "type": "email", "email": "bob@externalcompany.com" },
  "resource": { "type": "folder", "id": "{{ext_folder_id}}" },
  "role":     "viewer"
}

HTTP 201
# The response carries the resolved subject as a regular user UUID —
# externals never surface as a distinct subject_type post-PR-9.3a.
[Asserts]
jsonpath "$[0].subject.type" == "user"
jsonpath "$[0].resource.id" == "{{ext_folder_id}}"
[Captures]
bob_user_id: jsonpath "$[0].subject.id"


# ─────────────────────────────────────────────────────────────
# Step 4 — Alice's /grants/outgoing lists bob as a grantee.
#           The endpoint groups by resource and exposes the
#           subject display string (here: bob's email-as-username).
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/grants/outgoing/resources
Authorization: Bearer {{alice_token}}

HTTP 200
# Hurl's `includes` predicate type-mismatches when JSONPath returns a
# scalar (single-grantee case) instead of an array, so we assert on
# the raw body — robust regardless of result count + ordering.
[Asserts]
body contains "bob@externalcompany.com"
body contains "{{bob_user_id}}"


# ─────────────────────────────────────────────────────────────
# Step 5 — Defense gap #2: bob must NOT appear in the system
#           address book. The contacts handler filters externals
#           via `include_external = false` (PR 6).
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/address-books/system/contacts
Authorization: Bearer {{alice_token}}

HTTP 200
[Asserts]
body not contains "bob@externalcompany.com"
body not contains "{{bob_user_id}}"


# ─────────────────────────────────────────────────────────────
# Step 6 — Retrieve the invitation email captured by the mock
#           sender BEFORE issuing any further mail (the mock only
#           remembers the latest message per recipient), then
#           extract the magic-link URL out of the plain-text body.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/admin/smtp/test/captured?to=bob@externalcompany.com
Authorization: Bearer {{alice_token}}

HTTP 200
[Asserts]
jsonpath "$.to" == "bob@externalcompany.com"
jsonpath "$.subject" contains "shared a folder with you"
jsonpath "$.text_body" matches "/magic/v1/[A-Za-z0-9_-]+"
[Captures]
magic_url: jsonpath "$.text_body" regex "(https?://[^\\s]+/magic/v1/[A-Za-z0-9_-]+)"


# ─────────────────────────────────────────────────────────────
# Step 7 — Re-sharing the same email reuses bob — no second
#           external user gets created. The response carries the
#           same user_id captured in Step 3.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/folders
Authorization: Bearer {{alice_token}}
Content-Type: application/json
{ "name": "ext-share-2", "parent_id": "{{alice_home_id}}" }

HTTP 201
[Captures]
ext_folder_id_2: jsonpath "$.id"

POST {{base_url}}/api/grants
Authorization: Bearer {{alice_token}}
Content-Type: application/json
{
  "subject":  { "type": "email", "email": "bob@externalcompany.com" },
  "resource": { "type": "folder", "id": "{{ext_folder_id_2}}" },
  "role":     "viewer"
}

HTTP 201
[Asserts]
jsonpath "$[0].subject.id" == "{{bob_user_id}}"


# ─────────────────────────────────────────────────────────────
# Step 8 — Redeem the magic link. The handler 302s to the SPA
#           hash-route for the shared folder and sets the auth
#           cookies. Hurl follows-mode is OFF by default; we want
#           to inspect the Location header AND the Set-Cookie.
# ─────────────────────────────────────────────────────────────
GET {{magic_url}}

HTTP 302
[Asserts]
header "Location" == "/#/files/folder/{{ext_folder_id}}"
[Captures]
bob_access_token: cookie "oxicloud_access"


# ─────────────────────────────────────────────────────────────
# Step 9 — Bob (now carrying the cookie-issued JWT as bearer)
#           can read the shared folder. Without the magic-link
#           grant this would be 404 anti-enumeration.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/folders/{{ext_folder_id}}
Authorization: Bearer {{bob_access_token}}

HTTP 200
[Asserts]
jsonpath "$.id" == "{{ext_folder_id}}"
jsonpath "$.name" == "ext-share"


# ─────────────────────────────────────────────────────────────
# Step 10 — Bob sees the shared folder in his /grants/incoming.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/grants/incoming/resources
Authorization: Bearer {{bob_access_token}}

HTTP 200
[Asserts]
body contains "{{ext_folder_id}}"


# ─────────────────────────────────────────────────────────────
# Step 11 — External-user lockouts (PR 11.1 + ContactsHandler).
#           Bob (external) must NOT reach the system address book
#           or the per-user profile endpoint. Defense-in-depth on
#           top of the PR 6 service-level filter.
# ─────────────────────────────────────────────────────────────

# 11a — system address book: visible at the catalog level
# (`GET /api/address-books`) for bob? It must NOT list the system entry.
GET {{base_url}}/api/address-books
Authorization: Bearer {{bob_access_token}}

HTTP 200
[Asserts]
body not contains "OxiCloud Users"
body not contains "\"id\":\"system\""

# 11b — system contacts listing: 403 for bob.
GET {{base_url}}/api/address-books/system/contacts
Authorization: Bearer {{bob_access_token}}

HTTP 403

# 11c — /api/users/{id}: bob cannot query anyone's profile, not even
#       Alice's. Service-level external lockout in get_user_profile.
GET {{base_url}}/api/users/{{bob_user_id}}
Authorization: Bearer {{bob_access_token}}

HTTP 403


# ─────────────────────────────────────────────────────────────
# Step 12 — /api/users/{id} happy path (Alice → Bob).
#           Visibility rule: they share a grant, so Alice sees
#           Bob's profile (with is_external=true).
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/users/{{bob_user_id}}
Authorization: Bearer {{alice_token}}

HTTP 200
[Asserts]
jsonpath "$.id" == "{{bob_user_id}}"
jsonpath "$.is_external" == true


# ─────────────────────────────────────────────────────────────
# Step 13 — /api/users/{id} 404 anti-enumeration for an
#           unrelated UUID (random Uuid that doesn't exist).
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/users/00000000-0000-0000-0000-deadbeefcafe
Authorization: Bearer {{alice_token}}

HTTP 404


# ─────────────────────────────────────────────────────────────
# Step 14 — Second redemption of the same magic-link token is
#           rejected — single-use is enforced by the SQL UPDATE in
#           magic_link_token_pg_repository::mark_used.
# ─────────────────────────────────────────────────────────────
GET {{magic_url}}

HTTP 410


# ─────────────────────────────────────────────────────────────
# Step 12 — Cleanup. Alice trashes the two test folders and
#           deletes bob via the admin API so the suite's
#           storage-check sweep at run.sh end sees a clean DB.
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/folders/{{ext_folder_id_2}}
Authorization: Bearer {{alice_token}}

HTTP 204

DELETE {{base_url}}/api/folders/{{ext_folder_id}}
Authorization: Bearer {{alice_token}}

HTTP 204

DELETE {{base_url}}/api/admin/users/{{bob_user_id}}
Authorization: Bearer {{alice_token}}

HTTP *
