# =============================================================
# OxiCloud – Batch folder copy end-to-end scenario
# =============================================================
# Verifies that copying a folder (containing one file) into a
# target folder produces an independent copy: the copied folder
# appears inside the target, and its child file is present.
#
# Run standalone:
#   hurl --variables-file tests/api/test.env --test \
#        tests/api/batch_folder_copy.hurl
# =============================================================


# ─────────────────────────────────────────────────────────────
# Step 1 – Login
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
# Step 2 – Create source folder
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/folders
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "name": "hurl-copy-source"
}

HTTP 201
[Captures]
source_folder_id: jsonpath "$.id"
[Asserts]
jsonpath "$.name" == "hurl-copy-source"


# ─────────────────────────────────────────────────────────────
# Step 3 – Upload one file into the source folder
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/files/upload
Authorization: Bearer {{token}}
[MultipartFormData]
folder_id: {{source_folder_id}}
file: file,fixtures/hello.txt; text/plain

HTTP 201
[Captures]
source_file_id: jsonpath "$.id"
source_file_name: jsonpath "$.name"
[Asserts]
jsonpath "$.folder_id" == "{{source_folder_id}}"


# ─────────────────────────────────────────────────────────────
# Step 4 – Create target folder (copy destination)
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/folders
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "name": "hurl-copy-target"
}

HTTP 201
[Captures]
target_folder_id: jsonpath "$.id"
[Asserts]
jsonpath "$.name" == "hurl-copy-target"


# ─────────────────────────────────────────────────────────────
# Step 5 – Copy source folder into target folder
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/batch/folders/copy
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "folder_ids": ["{{source_folder_id}}"],
  "target_folder_id": "{{target_folder_id}}"
}

HTTP 200
[Captures]
new_root_folder_id: jsonpath "$.successful[0].new_root_folder_id"
[Asserts]
jsonpath "$.stats.successful" == 1
jsonpath "$.stats.failed" == 0
jsonpath "$.successful[0].new_root_folder_id" isString
jsonpath "$.successful[0].folders_copied" >= 1
jsonpath "$.successful[0].files_copied" == 1


# ─────────────────────────────────────────────────────────────
# Step 6 – Verify the copied folder is inside the target folder
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/folders/{{target_folder_id}}/listing
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$.folders" count == 1
jsonpath "$.folders[0].id" == "{{new_root_folder_id}}"
jsonpath "$.folders[0].name" == "hurl-copy-source"
jsonpath "$.files" count == 0


# ─────────────────────────────────────────────────────────────
# Step 7 – Verify the copied file is inside the copied folder
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/files?folder_id={{new_root_folder_id}}
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$" count == 1
jsonpath "$[0].name" == "{{source_file_name}}"
jsonpath "$[0].id" != "{{source_file_id}}"


# ─────────────────────────────────────────────────────────────
# Step 8 – Verify the original source folder is unchanged
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/files?folder_id={{source_folder_id}}
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$" count == 1
jsonpath "$[0].id" == "{{source_file_id}}"


# ─────────────────────────────────────────────────────────────
# Step 9 – Cleanup: delete the copied folder tree
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/folders/{{new_root_folder_id}}
Authorization: Bearer {{token}}

HTTP 204


# ─────────────────────────────────────────────────────────────
# Step 10 – Verify source folder and file are still intact
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/folders/{{source_folder_id}}
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$.id" == "{{source_folder_id}}"
jsonpath "$.name" == "hurl-copy-source"

GET {{base_url}}/api/files?folder_id={{source_folder_id}}
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$" count == 1
jsonpath "$[0].id" == "{{source_file_id}}"


# ─────────────────────────────────────────────────────────────
# Step 11 – Cleanup: delete source file and folders
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/files/{{source_file_id}}
Authorization: Bearer {{token}}

HTTP 204

DELETE {{base_url}}/api/folders/{{source_folder_id}}
Authorization: Bearer {{token}}

HTTP 204

DELETE {{base_url}}/api/folders/{{target_folder_id}}
Authorization: Bearer {{token}}

HTTP 204
