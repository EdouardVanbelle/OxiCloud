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
jsonpath "$[?(@.id == '{{file_id}}')].content_hash" == "b2208c5dc33ff951227bd0c139f5eccb04105d6da6a7519ee23f7bc00a17bb5a"


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


# ═════════════════════════════════════════════════════════════
# Checksum verification scenarios
# ═════════════════════════════════════════════════════════════
# `?checksum=<hex>&checksumalg=<name>` (default md5 if alg
# omitted, preserving the legacy `Content-MD5` contract). All
# three algorithms compute incrementally during the streaming
# write — zero extra disk reads. Known hashes of hello.txt:
#   md5:    f02bc35b153756ad11e07885cd86cbcf
#   sha256: 0237134783df857fd9634c004341dbfccd374be0a1dd3c08e257522fa4d44e20
#   blake3: b2208c5dc33ff951227bd0c139f5eccb04105d6da6a7519ee23f7bc00a17bb5a


# ─────────────────────────────────────────────────────────────
# Step 10 — MD5 (default alg, no `checksumalg` param).
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/uploads
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "filename": "chunked-cap-md5.txt",
  "folder_id": "{{home_folder_id}}",
  "content_type": "text/plain",
  "total_size": 32,
  "chunk_size": 1048576
}

HTTP 201
[Captures]
upload_id_md5: jsonpath "$.upload_id"

PATCH {{base_url}}/api/uploads/{{upload_id_md5}}?chunk_index=0&checksum=f02bc35b153756ad11e07885cd86cbcf
Authorization: Bearer {{token}}
Content-Type: application/octet-stream
file,fixtures/hello.txt;

HTTP 200

POST {{base_url}}/api/uploads/{{upload_id_md5}}/complete
Authorization: Bearer {{token}}

HTTP 201


# ─────────────────────────────────────────────────────────────
# Step 11 — SHA-256 (`?checksumalg=sha256`).
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/uploads
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "filename": "chunked-cap-sha256.txt",
  "folder_id": "{{home_folder_id}}",
  "content_type": "text/plain",
  "total_size": 32,
  "chunk_size": 1048576
}

HTTP 201
[Captures]
upload_id_sha: jsonpath "$.upload_id"

PATCH {{base_url}}/api/uploads/{{upload_id_sha}}?chunk_index=0&checksum=0237134783df857fd9634c004341dbfccd374be0a1dd3c08e257522fa4d44e20&checksumalg=sha256
Authorization: Bearer {{token}}
Content-Type: application/octet-stream
file,fixtures/hello.txt;

HTTP 200

POST {{base_url}}/api/uploads/{{upload_id_sha}}/complete
Authorization: Bearer {{token}}

HTTP 201


# ─────────────────────────────────────────────────────────────
# Step 12 — BLAKE3 (`?checksumalg=blake3`). Same algorithm the
#           blob-storage layer uses for dedup, so the chunk-level
#           hash and the assembled-file hash are comparable.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/uploads
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "filename": "chunked-cap-blake3.txt",
  "folder_id": "{{home_folder_id}}",
  "content_type": "text/plain",
  "total_size": 32,
  "chunk_size": 1048576
}

HTTP 201
[Captures]
upload_id_blake3: jsonpath "$.upload_id"

PATCH {{base_url}}/api/uploads/{{upload_id_blake3}}?chunk_index=0&checksum=b2208c5dc33ff951227bd0c139f5eccb04105d6da6a7519ee23f7bc00a17bb5a&checksumalg=blake3
Authorization: Bearer {{token}}
Content-Type: application/octet-stream
file,fixtures/hello.txt;

HTTP 200

POST {{base_url}}/api/uploads/{{upload_id_blake3}}/complete
Authorization: Bearer {{token}}

HTTP 201


# ─────────────────────────────────────────────────────────────
# Step 13 — Checksum MISMATCH: send a chunk with a deliberately
#           wrong MD5. `commit_chunk` must reject (the chunk
#           file is removed in the same path so a retry against
#           the same index starts clean).
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/uploads
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "filename": "chunked-cap-badmd5.txt",
  "folder_id": "{{home_folder_id}}",
  "content_type": "text/plain",
  "total_size": 32,
  "chunk_size": 1048576
}

HTTP 201
[Captures]
upload_id_bad: jsonpath "$.upload_id"

PATCH {{base_url}}/api/uploads/{{upload_id_bad}}?chunk_index=0&checksum=00000000000000000000000000000000
Authorization: Bearer {{token}}
Content-Type: application/octet-stream
file,fixtures/hello.txt;

HTTP 400

DELETE {{base_url}}/api/uploads/{{upload_id_bad}}
Authorization: Bearer {{token}}

HTTP 204


# ─────────────────────────────────────────────────────────────
# Step 14 — Unknown checksumalg → 400 BadRequest with the
#           offending value echoed back. Guards against a typo
#           silently disabling integrity verification.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/uploads
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "filename": "chunked-cap-badalg.txt",
  "folder_id": "{{home_folder_id}}",
  "content_type": "text/plain",
  "total_size": 32,
  "chunk_size": 1048576
}

HTTP 201
[Captures]
upload_id_badalg: jsonpath "$.upload_id"

PATCH {{base_url}}/api/uploads/{{upload_id_badalg}}?chunk_index=0&checksum=deadbeef&checksumalg=zoiberg
Authorization: Bearer {{token}}
Content-Type: application/octet-stream
file,fixtures/hello.txt;

HTTP 400

DELETE {{base_url}}/api/uploads/{{upload_id_badalg}}
Authorization: Bearer {{token}}

HTTP 204


# ─────────────────────────────────────────────────────────────
# Step 15 — Size MISMATCH: declare chunk_size 32, send 4 bytes.
#           Streaming write succeeds; `commit_chunk` rejects on
#           the size check, removes the partial file.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/uploads
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "filename": "chunked-cap-shortbody.txt",
  "folder_id": "{{home_folder_id}}",
  "content_type": "text/plain",
  "total_size": 32,
  "chunk_size": 1048576
}

HTTP 201
[Captures]
upload_id_short: jsonpath "$.upload_id"

PATCH {{base_url}}/api/uploads/{{upload_id_short}}?chunk_index=0
Authorization: Bearer {{token}}
Content-Type: application/octet-stream
base64,aGFsdA==;

HTTP 400

DELETE {{base_url}}/api/uploads/{{upload_id_short}}
Authorization: Bearer {{token}}

HTTP 204


# ═════════════════════════════════════════════════════════════
# End-to-end checksum on `/complete`
# ═════════════════════════════════════════════════════════════
# Optional body `{checksum, checksumalg}` on the complete request
# lets the client lock in end-to-end integrity: if the assembled
# file's hash doesn't match the value the client expected, the
# blob is NOT promoted to storage and no DB row is created.
# `blake3` is the recommended algorithm — same one the server
# computes during hash-on-write assembly, so verification costs
# nothing extra.


# ─────────────────────────────────────────────────────────────
# Step 16 — `/complete` with matching BLAKE3 → 201.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/uploads
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "filename": "complete-blake3-ok.txt",
  "folder_id": "{{home_folder_id}}",
  "content_type": "text/plain",
  "total_size": 32,
  "chunk_size": 1048576
}

HTTP 201
[Captures]
upload_id_b3_ok: jsonpath "$.upload_id"

PATCH {{base_url}}/api/uploads/{{upload_id_b3_ok}}?chunk_index=0
Authorization: Bearer {{token}}
Content-Type: application/octet-stream
file,fixtures/hello.txt;

HTTP 200

POST {{base_url}}/api/uploads/{{upload_id_b3_ok}}/complete
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "checksum": "b2208c5dc33ff951227bd0c139f5eccb04105d6da6a7519ee23f7bc00a17bb5a",
  "checksumalg": "blake3"
}

HTTP 201


# ─────────────────────────────────────────────────────────────
# Step 17 — `/complete` with matching SHA-256 → 201 (re-hashes
#           the assembled file on the blocking pool).
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/uploads
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "filename": "complete-sha256-ok.txt",
  "folder_id": "{{home_folder_id}}",
  "content_type": "text/plain",
  "total_size": 32,
  "chunk_size": 1048576
}

HTTP 201
[Captures]
upload_id_sha_ok: jsonpath "$.upload_id"

PATCH {{base_url}}/api/uploads/{{upload_id_sha_ok}}?chunk_index=0
Authorization: Bearer {{token}}
Content-Type: application/octet-stream
file,fixtures/hello.txt;

HTTP 200

POST {{base_url}}/api/uploads/{{upload_id_sha_ok}}/complete
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "checksum": "0237134783df857fd9634c004341dbfccd374be0a1dd3c08e257522fa4d44e20",
  "checksumalg": "sha256"
}

HTTP 201


# ─────────────────────────────────────────────────────────────
# Step 18 — `/complete` with MISMATCHED BLAKE3 → 400. Blob is
#           NOT promoted; session stays open for retry.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/uploads
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "filename": "complete-blake3-bad.txt",
  "folder_id": "{{home_folder_id}}",
  "content_type": "text/plain",
  "total_size": 32,
  "chunk_size": 1048576
}

HTTP 201
[Captures]
upload_id_b3_bad: jsonpath "$.upload_id"

PATCH {{base_url}}/api/uploads/{{upload_id_b3_bad}}?chunk_index=0
Authorization: Bearer {{token}}
Content-Type: application/octet-stream
file,fixtures/hello.txt;

HTTP 200

POST {{base_url}}/api/uploads/{{upload_id_b3_bad}}/complete
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "checksum": "00000000000000000000000000000000000000000000000000000000deadbeef",
  "checksumalg": "blake3"
}

HTTP 400

DELETE {{base_url}}/api/uploads/{{upload_id_b3_bad}}
Authorization: Bearer {{token}}

HTTP 204


# ─────────────────────────────────────────────────────────────
# Step 19 — `/complete` with unknown `checksumalg` → 400.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/uploads
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "filename": "complete-badalg.txt",
  "folder_id": "{{home_folder_id}}",
  "content_type": "text/plain",
  "total_size": 32,
  "chunk_size": 1048576
}

HTTP 201
[Captures]
upload_id_c_badalg: jsonpath "$.upload_id"

PATCH {{base_url}}/api/uploads/{{upload_id_c_badalg}}?chunk_index=0
Authorization: Bearer {{token}}
Content-Type: application/octet-stream
file,fixtures/hello.txt;

HTTP 200

POST {{base_url}}/api/uploads/{{upload_id_c_badalg}}/complete
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "checksum": "deadbeef",
  "checksumalg": "zoiberg"
}

HTTP 400

DELETE {{base_url}}/api/uploads/{{upload_id_c_badalg}}
Authorization: Bearer {{token}}

HTTP 204


# ─────────────────────────────────────────────────────────────
# Step 20 — `/complete` with NO body → 201 (backwards-compat).
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/uploads
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "filename": "complete-nobody.txt",
  "folder_id": "{{home_folder_id}}",
  "content_type": "text/plain",
  "total_size": 32,
  "chunk_size": 1048576
}

HTTP 201
[Captures]
upload_id_nobody: jsonpath "$.upload_id"

PATCH {{base_url}}/api/uploads/{{upload_id_nobody}}?chunk_index=0
Authorization: Bearer {{token}}
Content-Type: application/octet-stream
file,fixtures/hello.txt;

HTTP 200

POST {{base_url}}/api/uploads/{{upload_id_nobody}}/complete
Authorization: Bearer {{token}}

HTTP 201


# ═════════════════════════════════════════════════════════════
# Whole-file size cap (OXICLOUD_MAX_UPLOAD_SIZE)
# ═════════════════════════════════════════════════════════════
# The whole-file cap is checked at session creation against the
# JSON-declared `total_size` — no upload body required to trip
# it. Test default `OXICLOUD_MAX_UPLOAD_SIZE` is 10 GiB; we
# declare 100 GiB to be safely above without bothering with a
# Hurl `--variables` override.
#
# This guards the case where chunks would otherwise accumulate
# disk space against an oversized declared upload — the reject
# fires BEFORE any chunk is PATCHed.


# ─────────────────────────────────────────────────────────────
# Step 21 — POST /api/uploads with `total_size` above
#           OXICLOUD_MAX_UPLOAD_SIZE → 413 Payload Too Large.
#           No body bytes are sent; the check is purely
#           against the declared JSON field.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/uploads
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "filename": "way-too-big.bin",
  "folder_id": "{{home_folder_id}}",
  "content_type": "application/octet-stream",
  "total_size": 107374182400,
  "chunk_size": 5242880
}

HTTP 413


# ─────────────────────────────────────────────────────────────
# Step 22 — Sanity check: a session JUST BELOW the cap is
#           accepted. Uses a sub-cap value (32 bytes — same
#           pattern as earlier steps so no extra fixture is
#           needed). Confirms that the reject in step 21 was
#           the size cap, not an unrelated regression.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/uploads
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "filename": "small-and-fine.txt",
  "folder_id": "{{home_folder_id}}",
  "content_type": "text/plain",
  "total_size": 32,
  "chunk_size": 1048576
}

HTTP 201
[Captures]
upload_id_sanity: jsonpath "$.upload_id"

# Cleanup — cancel without uploading.
DELETE {{base_url}}/api/uploads/{{upload_id_sanity}}
Authorization: Bearer {{token}}

HTTP 204


# ═════════════════════════════════════════════════════════════
# Direct-PUT body cap (OXICLOUD_DIRECT_PUT_MAX_BYTES)
# ═════════════════════════════════════════════════════════════
# `OXICLOUD_DIRECT_PUT_MAX_BYTES` (4 MiB in the test env) bounds
# a single non-chunked PUT body. Larger uploads must come through
# the chunked protocol — the server returns 413 with a hint
# pointing at `/api/uploads/...` / `/dav/uploads/...`. The cap
# fires mid-stream via the same accumulator that bounds chunks.
#
# Tested via the native REST WebDAV PUT endpoint (`/webdav/...`)
# because it's JWT-authed (no app-password mint dance) and
# straightforwardly path-mapped. The cap is wired into the same
# `spool_body_to_temp` helper from the NextCloud single-file PUT
# path, so this exercise covers both wirings.


# ─────────────────────────────────────────────────────────────
# Step 23 — Direct PUT of the 5 MiB fixture → 413. The same
#           fixture used in step 8 for the chunked-PATCH cap;
#           here it lands against the direct-PUT cap.
# ─────────────────────────────────────────────────────────────
PUT {{base_url}}/webdav/test-direct-put-over-cap.bin
Authorization: Bearer {{token}}
Content-Type: application/octet-stream
file,fixtures/chunk-over-cap-5mb.bin;

HTTP 413


# ─────────────────────────────────────────────────────────────
# Step 24 — Sanity: direct PUT WELL under the cap → 201 or 204.
#           Confirms step 23's reject was the cap, not a route /
#           auth / handler regression.
# ─────────────────────────────────────────────────────────────
PUT {{base_url}}/webdav/test-direct-put-under-cap.txt
Authorization: Bearer {{token}}
Content-Type: text/plain
file,fixtures/hello.txt;

# WebDAV PUT returns 201 on new file, 204 on overwrite. Either
# value confirms a successful body acceptance.
HTTP *
[Asserts]
status >= 200
status < 300

# Cleanup so a re-run finds a clean slate (DELETE is idempotent
# enough for this — 204 on success, 404 if nothing left).
DELETE {{base_url}}/webdav/test-direct-put-under-cap.txt
Authorization: Bearer {{token}}

HTTP *
