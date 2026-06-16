# =============================================================
# OxiCloud — Upload precheck (`POST /api/files/by-hash`)
# =============================================================
# Validates the content_hash precheck that lets a client skip a
# body upload when the server already has the content for THIS
# caller. The request shape mirrors `FileDto.content_hash` — the
# same 64-char lowercase BLAKE3 hex digest the server returned
# on a previous upload.
#
# Scope-critical security rule (server-enforced): the precheck
# returns 201 only when the calling user already owns at least
# one file with the same BLAKE3 hash. Cross-user matches are
# invisible — hash probing cannot leak other tenants' content.
#
# Coverage:
#   1. Upload hello.txt normally → 201, content_hash captured
#   2. Precheck with the matching content_hash → 201, new
#      file_id but SAME content_hash (dedup hit)
#   3. Precheck with a fake/random hash → 404
#   4. Malformed content_hash (non-hex chars) → 400
#   5. Wrong-length content_hash → 400
#
# Fixture: tests/fixtures/hello-copy.txt (32 bytes; same content as
# hello.txt). We seed with hello-copy.txt — NOT hello.txt — because
# several earlier tests in run.sh (trash, chunked_upload_cap, …)
# upload hello.txt into the home folder and don't reliably purge it,
# which would collide with our seed step here (409). The filename
# `hello-copy.txt` is unique to this test.
#   BLAKE3 hex:    b2208c5dc33ff951227bd0c139f5eccb04105d6da6a7519ee23f7bc00a17bb5a
# =============================================================


# ─────────────────────────────────────────────────────────────
# Step 1 — Login
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
# Step 2 — Resolve home folder
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/folders
Authorization: Bearer {{token}}

HTTP 200
[Captures]
home_folder_id: jsonpath "$[0].id"


# ─────────────────────────────────────────────────────────────
# Step 3 — Seed: upload hello-copy.txt via the normal multipart
#          path. Captures the BLAKE3 server-side `content_hash`
#          that the precheck step then matches against.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/files/upload
Authorization: Bearer {{token}}
[MultipartFormData]
folder_id: {{home_folder_id}}
file: file,fixtures/hello-copy.txt; text/plain

HTTP 201
[Captures]
seed_file_id: jsonpath "$.id"
seed_content_hash: jsonpath "$.content_hash"
[Asserts]
jsonpath "$.name" == "hello-copy.txt"
jsonpath "$.size" == 32
jsonpath "$.content_hash" == "b2208c5dc33ff951227bd0c139f5eccb04105d6da6a7519ee23f7bc00a17bb5a"


# ─────────────────────────────────────────────────────────────
# Step 4 — Duplicate-name probe (incidental but useful): try
#          uploading the same fixture again, same name, same
#          folder → 409 Conflict. This catches the (folder_id,
#          name, user_id) WHERE NOT is_trashed unique index in
#          `storage.files`.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/files/upload
Authorization: Bearer {{token}}
[MultipartFormData]
folder_id: {{home_folder_id}}
file: file,fixtures/hello-copy.txt; text/plain

HTTP 409


# ─────────────────────────────────────────────────────────────
# Step 5 — Precheck HIT: caller now owns the BLAKE3 of
#          hello-copy.txt, so by-hash with that hash must
#          201 and return a NEW file_id (the metadata row is
#          new — the blob is shared).
#
#          Expectations:
#          - status: 201
#          - file_id differs from the seed
#          - content_hash matches the seed (proves dedup hit)
#          - filename is the one we requested
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/files/by-hash
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "name": "hello-precheck-bypass.txt",
  "folder_id": "{{home_folder_id}}",
  "hash": "b2208c5dc33ff951227bd0c139f5eccb04105d6da6a7519ee23f7bc00a17bb5a"
}

HTTP 201
[Captures]
copy_file_id: jsonpath "$.id"
copy_content_hash: jsonpath "$.content_hash"
[Asserts]
jsonpath "$.name" == "hello-precheck-bypass.txt"
jsonpath "$.size" == 32
jsonpath "$.content_hash" == "b2208c5dc33ff951227bd0c139f5eccb04105d6da6a7519ee23f7bc00a17bb5a"


# ─────────────────────────────────────────────────────────────
# Step 6 — Same hash, different filename, in a NEW request →
#          should hit again. Confirms the precheck isn't a
#          one-shot bound to the seed file.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/files/by-hash
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "name": "hello-precheck-bypass-2.txt",
  "folder_id": "{{home_folder_id}}",
  "hash": "b2208c5dc33ff951227bd0c139f5eccb04105d6da6a7519ee23f7bc00a17bb5a"
}

HTTP 201
[Asserts]
jsonpath "$.name" == "hello-precheck-bypass-2.txt"
jsonpath "$.content_hash" == "b2208c5dc33ff951227bd0c139f5eccb04105d6da6a7519ee23f7bc00a17bb5a"


# ─────────────────────────────────────────────────────────────
# Step 7 — Precheck MISS with a random/never-uploaded hash →
#          404 with `error: blob_not_owned_by_caller` and an
#          `upload_path` hint. The 404 shape is the
#          anti-enumeration boundary: same response whether the
#          hash doesn't exist anywhere or exists but is owned
#          by another user.
#
#          The fake hash below is just 32 bytes of 0xFF (well-
#          formed BLAKE3 shape, vanishingly unlikely to collide).
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/files/by-hash
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "name": "should-not-create.bin",
  "folder_id": "{{home_folder_id}}",
  "hash": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
}

HTTP 404
[Asserts]
jsonpath "$.error" == "blob_not_owned_by_caller"
jsonpath "$.upload_path" == "/api/files/upload"


# ─────────────────────────────────────────────────────────────
# Step 8 — Malformed content_hash (non-hex chars) → 400.
#          Defensive parse: typos shouldn't silently disable
#          the integrity check.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/files/by-hash
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "name": "should-not-create.bin",
  "folder_id": "{{home_folder_id}}",
  "hash": "this-is-not-hex-content-hash-zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"
}

HTTP 400


# ─────────────────────────────────────────────────────────────
# Step 9 — Wrong-length content_hash → 400. A 64-char string
#          is required (32-byte BLAKE3); anything else (here a
#          SHA-1-sized 40-char hex) is rejected at the input
#          boundary before the lookup happens.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/files/by-hash
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "name": "should-not-create.bin",
  "folder_id": "{{home_folder_id}}",
  "hash": "a94a8fef8c5d539e7e0f9c4b4d2c93e88f44f3f8"
}

HTTP 400


# ─────────────────────────────────────────────────────────────
# Step 10 — Trashed-blob precheck setup. Upload a fixture whose
#           BLAKE3 is GUARANTEED unique to this test (the
#           `hello-trashed.txt` fixture isn't referenced anywhere
#           else under tests/), so the only file owning the blob
#           after step 11 will be the one we trash.
#
#           Captures `trashed_seed_id` + `trashed_seed_hash`
#           which steps 11 and 12 consume.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/files/upload
Authorization: Bearer {{token}}
[MultipartFormData]
folder_id: {{home_folder_id}}
file: file,fixtures/hello-trashed.txt; text/plain

HTTP 201
[Captures]
trashed_seed_id: jsonpath "$.id"
trashed_seed_hash: jsonpath "$.content_hash"
[Asserts]
jsonpath "$.name" == "hello-trashed.txt"
jsonpath "$.size" == 35


# ─────────────────────────────────────────────────────────────
# Step 11 — Trash the seed (soft-delete). The blob's ref_count
#           stays > 0 because the file row still exists with
#           is_trashed = true. Pre-change behaviour would have
#           hidden the blob from `/api/files/by-hash`; the new
#           behaviour treats trashed-file ownership as ownership.
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/files/{{trashed_seed_id}}
Authorization: Bearer {{token}}

# Either 200 or 204 depending on whether the server returns a
# body. The trash service uses soft-delete (UPDATE is_trashed = true),
# not a hard delete, so the blob row is untouched.
HTTP *


# ─────────────────────────────────────────────────────────────
# Step 12 — Precheck HIT on a trashed-only owner. Must succeed
#           (201) and return a NEW live file pointing at the same
#           blob — even though the only OTHER reference to the
#           blob is a trashed file. Verifies the dropped
#           `AND NOT f.is_trashed` filter in
#           `user_owns_blob_reference` and `pin_claimable_chunks`.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/files/by-hash
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "name": "restored-via-by-hash.txt",
  "folder_id": "{{home_folder_id}}",
  "hash": "{{trashed_seed_hash}}"
}

HTTP 201
[Captures]
trashed_copy_id: jsonpath "$.id"
[Asserts]
jsonpath "$.name" == "restored-via-by-hash.txt"
jsonpath "$.size" == 35
jsonpath "$.content_hash" == "{{trashed_seed_hash}}"


# ─────────────────────────────────────────────────────────────
# Step 13 — Cleanup. Trash every file created by this script;
#           the next run starts clean. Seed (step 4) + copies
#           (steps 5+6) share one blob; trashed seed (step 10)
#           + by-hash restore (step 12) share another. Trashing
#           all four lets the GC ref-count both blobs to zero.
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/files/{{seed_file_id}}
Authorization: Bearer {{token}}

HTTP *

DELETE {{base_url}}/api/files/{{copy_file_id}}
Authorization: Bearer {{token}}

HTTP *

DELETE {{base_url}}/api/files/{{trashed_copy_id}}
Authorization: Bearer {{token}}

HTTP *
