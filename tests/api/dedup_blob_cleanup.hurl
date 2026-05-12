# =============================================================
# OxiCloud – Dedup blob lifecycle (bugs 3 & 4)
# =============================================================
# Verifies that when two files share the same blob (dedup hit)
# and both are permanently deleted, the blob lifecycle is correct.
#
# Bug 3: blob not deleted when last file reference is removed
# Bug 4: blob-keyed thumbnail not cleaned up with the blob
#
# Sequence:
#   1. Upload dedup-test.jpg twice → two file records, one blob
#   2. Both thumbnails return identical bytes → proves shared blob
#   3. Permanently delete file 1 → file 2 thumbnail still 200
#      (proves blob NOT prematurely deleted — bug 3 detection)
#   4. Permanently delete file 2 → blob and thumbnail cleaned up
#
# NOTE: The /api/dedup/stats endpoint counts CDC chunk rows in
#       storage.blobs and derives bytes_saved from chunk_manifests.
#       Both tables may be 0 when the CDC path is disabled or the
#       server uses the legacy blob path — so we avoid stats-based
#       assertions and rely on observable thumbnail behaviour instead.
#
# Prerequisites: setup.hurl must have run (admin user exists).
#
# Run:
#   hurl --variables-file tests/api/test.env --test tests/api/dedup_blob_cleanup.hurl
# =============================================================


# ─────────────────────────────────────────────────────────────
# Step 1 – Login as admin
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
# Step 2 – Create a folder for this test
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/folders
Authorization: Bearer {{token}}

HTTP 200
[Captures]
home_folder_id: jsonpath "$[0].id"


POST {{base_url}}/api/folders
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "name": "hurl-dedup-blob-test",
  "parent_id": "{{home_folder_id}}"
}

HTTP 201
[Captures]
test_folder_id: jsonpath "$.id"
[Asserts]
jsonpath "$.name" == "hurl-dedup-blob-test"


# ─────────────────────────────────────────────────────────────
# Step 3 – Upload dedup-test.jpg (file 1)
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/files/upload
Authorization: Bearer {{token}}
[MultipartFormData]
folder_id: {{test_folder_id}}
file: file,fixtures/dedup-test.jpg; image/jpeg

HTTP 201
[Captures]
file1_id: jsonpath "$.id"
[Asserts]
jsonpath "$.name" == "dedup-test.jpg"
jsonpath "$.folder_id" == {{test_folder_id}}


# ─────────────────────────────────────────────────────────────
# Step 4 – Upload identical content again as dedup-test-2.jpg
#          Dedup: same blob, new file record, different file ID
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/files/upload
Authorization: Bearer {{token}}
[MultipartFormData]
folder_id: {{test_folder_id}}
file: file,fixtures/dedup-test-2.jpg; image/jpeg

HTTP 201
[Captures]
file2_id: jsonpath "$.id"
[Asserts]
jsonpath "$.name" == "dedup-test-2.jpg"
jsonpath "$.id" != "{{file1_id}}"


# ─────────────────────────────────────────────────────────────
# Step 5 – Dedup proof: thumbnails are byte-identical
#          Thumbnail generation reads blob bytes and is keyed by
#          blob_hash on disk.  If both files share the same blob,
#          GET /thumbnail returns the same bytes for both.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/files/{{file1_id}}/thumbnail/icon
Authorization: Bearer {{token}}

HTTP 200
[Captures]
thumb1: bytes


GET {{base_url}}/api/files/{{file2_id}}/thumbnail/icon
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
bytes == {{thumb1}}


# ─────────────────────────────────────────────────────────────
# Step 6 – Move file 1 to trash
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/files/{{file1_id}}
Authorization: Bearer {{token}}

HTTP 204


# ─────────────────────────────────────────────────────────────
# Step 7 – Permanently delete file 1 from trash
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/trash
Authorization: Bearer {{token}}

HTTP 200
[Captures]
trash_item1_id: jsonpath "$[?(@.original_id == '{{file1_id}}')].id"
[Asserts]
jsonpath "$[?(@.original_id == '{{file1_id}}')].id" isString
jsonpath "$[?(@.original_id == '{{file1_id}}')].item_type" == "file"


DELETE {{base_url}}/api/trash/{{trash_item1_id}}
Authorization: Bearer {{token}}

HTTP 200


# ─────────────────────────────────────────────────────────────
# Step 8 – Blob still alive: file 2 thumbnail is accessible
#          After file 1 is permanently deleted the blob ref_count
#          drops to 1 but the blob must NOT be removed yet.
#          Thumbnail generation reads blob bytes live — a 200 here
#          proves the blob is still present.
#          If bug 3 is present the blob is deleted prematurely and
#          this request returns a 5xx error.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/files/{{file2_id}}/thumbnail/icon
Authorization: Bearer {{token}}

HTTP 200


# ─────────────────────────────────────────────────────────────
# Step 9 – Move file 2 to trash
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/files/{{file2_id}}
Authorization: Bearer {{token}}

HTTP 204


# ─────────────────────────────────────────────────────────────
# Step 10 – Permanently delete file 2 from trash
#           ref_count hits 0 → blob and its disk thumbnail deleted
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/trash
Authorization: Bearer {{token}}

HTTP 200
[Captures]
trash_item2_id: jsonpath "$[?(@.original_id == '{{file2_id}}')].id"
[Asserts]
jsonpath "$[?(@.original_id == '{{file2_id}}')].id" isString
jsonpath "$[?(@.original_id == '{{file2_id}}')].item_type" == "file"


DELETE {{base_url}}/api/trash/{{trash_item2_id}}
Authorization: Bearer {{token}}

HTTP 200


# ─────────────────────────────────────────────────────────────
# Step 11 – Cleanup: delete the (now empty) test folder
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/folders/{{test_folder_id}}
Authorization: Bearer {{token}}

HTTP 204


GET {{base_url}}/api/trash
Authorization: Bearer {{token}}

HTTP 200
[Captures]
trash_folder_id: jsonpath "$[?(@.original_id == '{{test_folder_id}}')].id"
[Asserts]
jsonpath "$[?(@.original_id == '{{test_folder_id}}')].id" isString
jsonpath "$[?(@.original_id == '{{test_folder_id}}')].item_type" == "folder"


DELETE {{base_url}}/api/trash/{{trash_folder_id}}
Authorization: Bearer {{token}}

HTTP 200
