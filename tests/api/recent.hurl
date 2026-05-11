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


GET {{base_url}}/api/folders/{{home_folder_id}}/contents
Authorization: Bearer {{token}}

HTTP 200
[Captures]
test2_id: jsonpath "$[1].id"
[Asserts]
jsonpath "$[1].name" == "test2-renamed"


GET {{base_url}}/api/files?folder_id={{test2_id}}
Authorization: Bearer {{token}}

HTTP 200
[Captures]
file_id: jsonpath "$[0].id"
[Asserts]
jsonpath "$[0].name" == "hello-renamed.txt"


# ─────────────────────────────────────────────────────────────
# Step 3 – Record access to hello-renamed.txt
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/recent/file/{{file_id}}
Authorization: Bearer {{token}}

HTTP 200


# ─────────────────────────────────────────────────────────────
# Step 4 – Recent list contains hello-renamed.txt
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/recent
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$" count == 1
jsonpath "$[0].item_type" == "file"
jsonpath "$[0].item_name" == "hello-renamed.txt"


# ─────────────────────────────────────────────────────────────
# Step 5 – Clear all recent items
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/recent/clear
Authorization: Bearer {{token}}

HTTP 200


# ─────────────────────────────────────────────────────────────
# Step 6 – Recent list is empty after clear
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/recent
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$" isCollection
jsonpath "$" count == 0
