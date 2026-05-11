# =============================================================
# OxiCloud – Trash API end-to-end scenario
# =============================================================
# Depends on files-folders.hurl having run first (home folder
# with test1 and test2-renamed must exist).
#
# Run:
#   hurl --variables-file tests/api/test.env --test tests/api/trash.hurl
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
# Step 2 – Trash is empty at the start
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/trash
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$" isCollection
jsonpath "$" count == 0


# ─────────────────────────────────────────────────────────────
# Step 3 – Capture the home folder ID
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/folders
Authorization: Bearer {{token}}

HTTP 200
[Captures]
home_folder_id: jsonpath "$[0].id"


# ─────────────────────────────────────────────────────────────
# Step 4 – Create "to-delete" folder inside the home folder
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/folders
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "name": "to-delete",
  "parent_id": "{{home_folder_id}}"
}

HTTP 201
[Captures]
to_delete_id: jsonpath "$.id"
[Asserts]
jsonpath "$.name" == "to-delete"
jsonpath "$.parent_id" == {{home_folder_id}}


# ─────────────────────────────────────────────────────────────
# Step 5 – Upload hello.txt into "to-delete"
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/files/upload
Authorization: Bearer {{token}}
[MultipartFormData]
folder_id: {{to_delete_id}}
file: file,fixtures/hello.txt; text/plain

HTTP 201
[Asserts]
jsonpath "$.name" == "hello.txt"
jsonpath "$.folder_id" == {{to_delete_id}}


# ─────────────────────────────────────────────────────────────
# Step 6 – Delete "to-delete" folder (moves it to trash)
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/folders/{{to_delete_id}}
Authorization: Bearer {{token}}

HTTP 204


# ─────────────────────────────────────────────────────────────
# Step 7 – Trash contains exactly the "to-delete" folder
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/trash
Authorization: Bearer {{token}}

HTTP 200
[Captures]
trash_id: jsonpath "$[0].id"
[Asserts]
jsonpath "$" count == 1
jsonpath "$[0].item_type" == "folder"
jsonpath "$[0].name" == "to-delete"
jsonpath "$[0].original_id" == {{to_delete_id}}


# ─────────────────────────────────────────────────────────────
# Step 8 – "to-delete" is no longer listed in the home folder
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/folders/{{home_folder_id}}/contents
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$[*].name" not contains "to-delete"


# ─────────────────────────────────────────────────────────────
# Step 9 – Restore "to-delete" from trash
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/trash/{{trash_id}}/restore
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$.success" == true


# ─────────────────────────────────────────────────────────────
# Step 10 – "to-delete" folder is back in the home folder
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/folders/{{home_folder_id}}/contents
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$[*].name" contains "to-delete"


# ─────────────────────────────────────────────────────────────
# Step 11 – hello.txt is still inside the restored folder
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/files?folder_id={{to_delete_id}}
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$" count == 1
jsonpath "$[0].name" == "hello.txt"


# ─────────────────────────────────────────────────────────────
# Step 12 – Trash is empty after restore
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/trash
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$" count == 0


# ─────────────────────────────────────────────────────────────
# Step 13 – Delete "to-delete" folder again (moves to trash)
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/folders/{{to_delete_id}}
Authorization: Bearer {{token}}

HTTP 204


# ─────────────────────────────────────────────────────────────
# Step 14 – Empty the trash
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/trash/empty
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$.success" == true


# ─────────────────────────────────────────────────────────────
# Step 15 – Trash is empty after purge
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/trash
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$" count == 0


# ─────────────────────────────────────────────────────────────
# Step 16 – "to-delete" folder no longer exists in home folder
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/folders/{{home_folder_id}}/contents
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$[*].name" not contains "to-delete"
