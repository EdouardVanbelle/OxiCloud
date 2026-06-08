# =============================================================
# OxiCloud — Chunked upload size cap + streaming verification
# =============================================================
# Validates `storage.chunk_max_bytes` (env `OXICLOUD_CHUNK_MAX_BYTES`,
# set to 4 MiB by `tests/common/server.env`) on the REST chunked upload
# surface `/api/uploads/{id}`.
#
# Two scenarios:
#   1. SUCCESS path — uploads `hello.txt` (32 bytes) in one chunk and
#      asserts the stored file's BLAKE3 matches the known hash of the
#      fixture. Confirms streaming + assembly + dedup integrity.
#   2. CAP REJECTION — uploads `chunk-over-cap-5mb.bin` (5 MiB) which
#      exceeds the 4 MiB cap, asserting the server returns 413 Payload
#      Too Large instead of OOMing or silently truncating.
#
# Why REST chunked, not NC chunked: both surfaces share the same
# `chunk_max_bytes` config; the REST path is JWT-authed (already set
# up in this Hurl run), the NC path would require minting an app
# password mid-test.
# =============================================================


# ─────────────────────────────────────────────────────────────
# Step 1 — Login and capture the JWT token
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


# ─────────────────────────────────────────────────────────────
# Step 2 — Resolve home folder id (single root folder for admin)
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/folders
Authorization: Bearer {{token}}

HTTP 200
[Captures]
home_folder_id: jsonpath "$[0].id"


# ─────────────────────────────────────────────────────────────
# Step 3 — Create chunked upload session for hello.txt (32 bytes)
#           chunk_size = 1 MiB (server minimum is 1 MiB). With
#           total_size < chunk_size the server computes a single
#           chunk of `total_size` bytes for index 0.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/uploads
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "filename": "chunked-cap-hello.txt",
  "folder_id": "{{home_folder_id}}",
  "content_type": "text/plain",
  "total_size": 32,
  "chunk_size": 1048576
}

HTTP 201
[Captures]
upload_id_ok: jsonpath "$.upload_id"


# ─────────────────────────────────────────────────────────────
# Step 4 — Upload the single 32-byte chunk → 200
# ─────────────────────────────────────────────────────────────
PATCH {{base_url}}/api/uploads/{{upload_id_ok}}?chunk_index=0
Authorization: Bearer {{token}}
Content-Type: application/octet-stream
file,fixtures/hello.txt;

HTTP 200
[Asserts]
header "Upload-Complete" == "true"


# ─────────────────────────────────────────────────────────────
# Step 5 — Complete the upload → 201, capture file id
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/uploads/{{upload_id_ok}}/complete
Authorization: Bearer {{token}}

HTTP 201
[Captures]
file_id: jsonpath "$.file_id"
[Asserts]
jsonpath "$.filename" == "chunked-cap-hello.txt"
jsonpath "$.size" == 32


# ─────────────────────────────────────────────────────────────
# Step 6 — Verify the stored file's BLAKE3 matches the known
#           hash of `tests/fixtures/hello.txt`. This proves the
#           streaming path wrote the exact bytes — no truncation,
#           no buffering corruption, no off-by-one.
#
#           Expected hash:
#             b2208c5dc33ff951227bd0c139f5eccb04105d6da6a7519ee23f7bc00a17bb5a
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/files?folder_id={{home_folder_id}}
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$[?(@.id == '{{file_id}}')].content_hash" includes "b2208c5dc33ff951227bd0c139f5eccb04105d6da6a7519ee23f7bc00a17bb5a"


# ─────────────────────────────────────────────────────────────
# Step 7 — CAP REJECTION: create a session declaring a 5 MiB
#           chunk and attempt to upload exactly 5 MiB → 413.
#
#           The cap fires in `axum::body::to_bytes(req, max_chunk)`
#           BEFORE the inner chunk-size check, so the server never
#           materialises the full 5 MiB body in memory.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/uploads
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "filename": "chunked-cap-over.bin",
  "folder_id": "{{home_folder_id}}",
  "content_type": "application/octet-stream",
  "total_size": 5242880,
  "chunk_size": 5242880
}

HTTP 201
[Captures]
upload_id_big: jsonpath "$.upload_id"


# ─────────────────────────────────────────────────────────────
# Step 8 — Send a 5 MiB chunk → 413 (cap is 4 MiB).
#           Pre-fix this PATCH would either OOM the server or be
#           silently treated as an empty body (via the old
#           `to_bytes(.., usize::MAX).unwrap_or_default()` path).
# ─────────────────────────────────────────────────────────────
PATCH {{base_url}}/api/uploads/{{upload_id_big}}?chunk_index=0
Authorization: Bearer {{token}}
Content-Type: application/octet-stream
file,fixtures/chunk-over-cap-5mb.bin;

HTTP 413


# ─────────────────────────────────────────────────────────────
# Step 9 — Clean up the abandoned over-cap session.
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/uploads/{{upload_id_big}}
Authorization: Bearer {{token}}

HTTP 204
