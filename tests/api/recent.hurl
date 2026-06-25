# =============================================================
# OxiCloud – Recent items API end-to-end scenario
# =============================================================
# Depends on files-folders.hurl having run first:
#   - home folder exists with test1 and test2-renamed
#   - test2-renamed contains hello-renamed.txt
#
# Run:
#   hurl --variables-file tests/api/test.env --test tests/api/recent.hurl
# =============================================================


# ─────────────────────────────────────────────────────────────
# Step 1 – Login and capture the JWT token
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/auth/login
Content-Type: application/json
{
  "username": "{{username}}",
  "password": "{{password}}"
}

HTTP 200
[Captures]
token: jsonpath "$.access_token"
[Asserts]
jsonpath "$.access_token" isString


# ─────────────────────────────────────────────────────────────
# Step 2 – Discover the file ID of hello-renamed.txt
#           Folders are ORDER BY name: test1 ($[0]), test2-renamed ($[1])
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/folders
Authorization: Bearer {{token}}

HTTP 200
[Captures]
home_folder_id: jsonpath "$[0].id"


GET {{base_url}}/api/folders/{{home_folder_id}}/resources?resource_types=folder
Authorization: Bearer {{token}}

HTTP 200
[Captures]
test2_id: jsonpath "$.items[1].resource.id"
[Asserts]
jsonpath "$.items[1].resource.name" == "test2-renamed"


GET {{base_url}}/api/files?folder_id={{test2_id}}
Authorization: Bearer {{token}}

HTTP 200
[Captures]
file_id: jsonpath "$[0].id"
[Asserts]
jsonpath "$[0].name" == "hello-renamed.txt"


# Defensive clear before the explicit-POST assertions: earlier
# scenarios in the runner (files-folders.hurl) auto-record every
# file they upload / GET through the service-layer
# `ResourceAccessHook`, so Recent already has rows by the time we
# arrive here. Clearing first lets step 4 assert `count == 1`
# against a known-empty baseline.
DELETE {{base_url}}/api/recent/clear
Authorization: Bearer {{token}}

HTTP 200


# ─────────────────────────────────────────────────────────────
# Step 3 – Record access to hello-renamed.txt
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/recent/file/{{file_id}}
Authorization: Bearer {{token}}

HTTP 200


# ─────────────────────────────────────────────────────────────
# Step 4 – Recent list contains hello-renamed.txt
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/recent/resources
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$.items" count == 1
jsonpath "$.items[0].resource_type" == "file"
jsonpath "$.items[0].resource.name" == "hello-renamed.txt"


# ─────────────────────────────────────────────────────────────
# Step 5 – Clear all recent items
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/recent/clear
Authorization: Bearer {{token}}

HTTP 200


# ─────────────────────────────────────────────────────────────
# Step 6 – Recent list is empty after clear
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/recent/resources
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$.items" isCollection
jsonpath "$.items" count == 0


# ─────────────────────────────────────────────────────────────
# Step 7 – Auto-recording on upload
#           The backend `ResourceAccessHook` fires on a successful
#           authorised upload, so the new file lands in Recent
#           without the client POSTing /api/recent/file/{id}.
#           This is the SvelteKit-era contract: the legacy
#           vanilla-JS frontend did the POST itself; the new shell
#           relies on the service-layer hook instead.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/files/upload
Authorization: Bearer {{token}}
[MultipartFormData]
folder_id: {{home_folder_id}}
file: file,fixtures/hello.txt; text/plain

HTTP 201
[Captures]
auto_uploaded_id: jsonpath "$.id"


GET {{base_url}}/api/recent/resources
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$.items" count == 1
jsonpath "$.items[0].resource.id" == "{{auto_uploaded_id}}"


# ─────────────────────────────────────────────────────────────
# Step 8 – Auto-recording on GET (file download)
#           Clear first, then download the file content and
#           assert it reappears in Recent. The per-(user, file)
#           60 s throttle inside the recording hook means the
#           cleared row may re-record on the very next GET only
#           because we just emptied the table — moka stores the
#           throttle entry independently of the DB row, but the
#           upsert is idempotent and harmless either way.
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/recent/clear
Authorization: Bearer {{token}}

HTTP 200


GET {{base_url}}/api/files/{{auto_uploaded_id}}
Authorization: Bearer {{token}}

HTTP 200


GET {{base_url}}/api/recent/resources
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$.items" count == 1
jsonpath "$.items[0].resource.id" == "{{auto_uploaded_id}}"


# ─────────────────────────────────────────────────────────────
# Step 9 – Cleanup so the test is idempotent across runs.
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/files/{{auto_uploaded_id}}
Authorization: Bearer {{token}}

HTTP 204


DELETE {{base_url}}/api/recent/clear
Authorization: Bearer {{token}}

HTTP 200
