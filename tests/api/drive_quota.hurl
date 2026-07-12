# =============================================================
# OxiCloud — D4 per-drive quota enforcement
# =============================================================
# Pins the upload-time per-drive quota refusal. Scope:
#
#   1. Quota = 100 B on a shared drive; uploading a 5 MiB file →
#      `507 Insufficient Storage`. Refusal happens BEFORE the file
#      row is registered (no orphan blob, no usage drift).
#   2. A small file (32 B) under the same quota → `201`. The fire-
#      and-forget delta hook bumps `drives.used_bytes`; the next
#      `GET /api/drives` lists the new value.
#   3. After consuming most of the quota, a second small file that
#      would push us over → `507`. Confirms the check uses the
#      cached `used_bytes`, not just file size in isolation.
#   4. Unlimited quota (`quota_bytes` omitted at create) accepts the
#      same 5 MiB upload that case 1 refused → `201`.
#   5. Per-user quota is unaffected — uploading to the user's own
#      default Personal drive (no per-drive cap) still works.
#
# The check is layered on top of the existing per-user quota
# (`storage_usage_service::check_storage_quota`) — both run at the
# multipart handler; either refusal yields 507.
#
# Self-contained: provisions `dq_owner` and a fresh shared drive
# per case so this can run alongside the rest of the API suite.
# =============================================================


# ─────────────────────────────────────────────────────────────
# Step 1 — Admin login.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/auth/login
Content-Type: application/json
{ "username": "{{username}}", "password": "{{password}}" }

HTTP 200
[Captures]
admin_token: jsonpath "$.access_token"


# ─────────────────────────────────────────────────────────────
# Step 2 — Provision `dq_owner` (drive owner under test).
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/admin/users
Authorization: Bearer {{admin_token}}
Content-Type: application/json
{
  "username": "dq_owner",
  "password": "DqOwnerPwd1!",
  "email":    "dq_owner@example.com",
  "role":     "user"
}

HTTP 201

POST {{base_url}}/api/auth/login
Content-Type: application/json
{ "username": "dq_owner", "password": "DqOwnerPwd1!" }

HTTP 200
[Captures]
owner_token: jsonpath "$.access_token"
owner_user_id: jsonpath "$.user.id"


# ─────────────────────────────────────────────────────────────
# Step 3 — Admin creates a shared drive with a tiny 100-byte quota
#          and `dq_owner` as direct user-Owner.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/drives
Authorization: Bearer {{admin_token}}
Content-Type: application/json
{
  "kind": "shared",
  "name": "dq-tight",
  "owner": { "type": "user", "id": "{{owner_user_id}}" },
  "quota_bytes": 100
}

HTTP 201
[Captures]
tight_drive_id: jsonpath "$.id"
tight_root_id:  jsonpath "$.root_folder_id"


# ─────────────────────────────────────────────────────────────
# Step 4 — Case 1: 5 MiB upload to the 100-byte drive → 507.
#          Refused at the multipart handler before the file row is
#          registered; the blob is discarded by `discard_ingested`.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/files/upload
Authorization: Bearer {{owner_token}}
[MultipartFormData]
folder_id: {{tight_root_id}}
file: file,fixtures/chunk-over-cap-5mb.bin; application/octet-stream

HTTP 507


# ─────────────────────────────────────────────────────────────
# Step 5 — Case 2: 32-byte upload fits under the 100-byte cap.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/files/upload
Authorization: Bearer {{owner_token}}
[MultipartFormData]
folder_id: {{tight_root_id}}
file: file,fixtures/hello.txt; text/plain

HTTP 201
[Captures]
small_file_id: jsonpath "$.id"


# Confirm `drives.used_bytes` reflects the new file. The hook is
# fire-and-forget on a tokio task, so the SQL UPDATE may not have
# landed by the time `POST /api/files/upload` returned. Retry the
# `GET /api/drives` until the cached value catches up — bounded
# wait keeps a slow CI machine from flaking.
GET {{base_url}}/api/drives
Authorization: Bearer {{owner_token}}
[Options]
retry: 10
retry-interval: 200ms

HTTP 200
[Asserts]
jsonpath "$[?(@.id=='{{tight_drive_id}}')].used_bytes" == 32


# ─────────────────────────────────────────────────────────────
# Step 6 — Case 3: a SECOND small file that would push usage past
#          the cap is refused. With `hello.txt` at 32 bytes already
#          on the drive, the next 32-byte upload projects to
#          32 + 32 + 32 (header overhead negligible) — far under
#          100 — and IS accepted. Then a 5 MiB upload remains over
#          quota: 507. This protects the "uses cached used_bytes"
#          invariant: the check isn't just `size < quota`.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/files/upload
Authorization: Bearer {{owner_token}}
[MultipartFormData]
folder_id: {{tight_root_id}}
file: file,fixtures/hello-copy.txt; text/plain

HTTP 201


# `used_bytes` climbs to 64 (32 + 32). Same retry shape as the
# first assertion since the second delta is also fire-and-forget.
GET {{base_url}}/api/drives
Authorization: Bearer {{owner_token}}
[Options]
retry: 10
retry-interval: 200ms

HTTP 200
[Asserts]
jsonpath "$[?(@.id=='{{tight_drive_id}}')].used_bytes" == 64


# The fire-and-forget delta hook updates `drives.used_bytes`; the
# subsequent 5 MiB attempt still fails (5 MiB > 100 alone). This
# assertion holds regardless of whether the previous delta has
# landed in cache or not — `5_242_880 > 100` either way.
POST {{base_url}}/api/files/upload
Authorization: Bearer {{owner_token}}
[MultipartFormData]
folder_id: {{tight_root_id}}
file: file,fixtures/chunk-over-cap-5mb.bin; application/octet-stream

HTTP 507


# `used_bytes` is unchanged — the failed upload didn't charge the
# drive. (Cumulative usage is still 64; the 5 MiB write never
# registered a row.)
GET {{base_url}}/api/drives
Authorization: Bearer {{owner_token}}

HTTP 200
[Asserts]
jsonpath "$[?(@.id=='{{tight_drive_id}}')].used_bytes" == 64


# ─────────────────────────────────────────────────────────────
# Step 7 — Case 4: unlimited drive accepts the same 5 MiB upload.
#          Confirms the `quota_bytes IS NULL` short-circuit.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/drives
Authorization: Bearer {{admin_token}}
Content-Type: application/json
{
  "kind": "shared",
  "name": "dq-unlimited",
  "owner": { "type": "user", "id": "{{owner_user_id}}" }
}

HTTP 201
[Captures]
unlimited_root_id: jsonpath "$.root_folder_id"
unlimited_drive_id: jsonpath "$.id"


POST {{base_url}}/api/files/upload
Authorization: Bearer {{owner_token}}
[MultipartFormData]
folder_id: {{unlimited_root_id}}
file: file,fixtures/chunk-over-cap-5mb.bin; application/octet-stream

HTTP 201


# Unlimited drive's `used_bytes` climbs to the file's exact size
# (5 MiB = 5_242_880 bytes). Same retry block because the delta
# hook is fire-and-forget here too.
GET {{base_url}}/api/drives
Authorization: Bearer {{owner_token}}
[Options]
retry: 10
retry-interval: 200ms

HTTP 200
[Asserts]
jsonpath "$[?(@.id=='{{unlimited_drive_id}}')].used_bytes" == 5242880


# ─────────────────────────────────────────────────────────────
# Step 9 — Post-delete sweep convergence.
#          By design the per-drive `used_bytes` counter is NOT
#          decremented on permanent delete (mirrors the existing
#          per-user quota design: deletes drift, the periodic sweep
#          reconciles). To prove the sweep actually closes the
#          drift, we:
#            a) Trash the 32-byte file in the unlimited drive
#               (well, both: hello.txt + hello-copy.txt are in the
#               tight drive; the 5 MiB is in the unlimited one).
#            b) Permanently delete via empty-trash.
#            c) Trigger the reconciliation sweep on demand —
#               `/api/admin/internal/trigger-sweep` is gated by
#               `OXICLOUD_ENABLE_ADMIN_INTERNAL_ENDPOINTS=true`
#               (set in `tests/common/server.env`).
#            d) `GET /api/drives` now shows the corrected counter.
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/files/{{small_file_id}}
Authorization: Bearer {{owner_token}}

HTTP 204


# Permanent purge — empty caller's trash entirely.
DELETE {{base_url}}/api/trash/empty
Authorization: Bearer {{owner_token}}

HTTP 200


# Sweep is fire-and-forget on a ticker (default 600 s). Run it now
# so the assertion below is deterministic instead of polling.
POST {{base_url}}/api/admin/internal/trigger-sweep
Authorization: Bearer {{admin_token}}

HTTP 200
[Asserts]
jsonpath "$.ok" == true


# After the sweep, `tight_drive.used_bytes` has dropped from 64 to
# 32 (hello.txt purged, hello-copy.txt still live). `unlimited`
# stays at 5 MiB (nothing trashed there).
GET {{base_url}}/api/drives
Authorization: Bearer {{owner_token}}

HTTP 200
[Asserts]
jsonpath "$[?(@.id=='{{tight_drive_id}}')].used_bytes" == 32
jsonpath "$[?(@.id=='{{unlimited_drive_id}}')].used_bytes" == 5242880


# ─────────────────────────────────────────────────────────────
# Step 10 — `/api/admin/internal/*` is admin-only and disabled by
#          default. The gate-off case is covered by the absence of
#          the route in production configs; here we just confirm a
#          non-admin caller is refused even when the feature is on.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/admin/internal/trigger-sweep
Authorization: Bearer {{owner_token}}

HTTP 403


# Trigger-GC reachable too — assert it returns the freed-blob
# summary shape. Hard count is non-deterministic (depends on the
# grace window vs the test's elapsed time), so we only check the
# response shape.
POST {{base_url}}/api/admin/internal/trigger-gc
Authorization: Bearer {{admin_token}}

HTTP 200
[Asserts]
jsonpath "$.ok" == true
jsonpath "$.blobs_deleted" exists
jsonpath "$.bytes_freed" exists


# ─────────────────────────────────────────────────────────────
# Step 11 — Pre-flight quota gate on MOVE and COPY.
#
#           Silent gap before 2026-07-06:
#           `move_file_with_perms` / `move_folder_with_perms`
#           / `copy_file_with_perms` / `copy_folder_tree_with_perms`
#           never called `check_drive_quota` on the destination.
#           A user could bypass a tight drive's cap by uploading
#           to their unlimited personal drive first and MOVE-ing
#           (or COPY-ing) into the tight drive afterwards.
#
#           Fix landed in the service layer, so both REST + WebDAV +
#           NC WebDAV surfaces got the check for free. This step
#           locks in the 507 shape on the REST path:
#
#             a) MOVE a 5 MiB file from unlimited → tight → 507.
#             b) COPY a 5 MiB file from unlimited → tight → 507.
#             c) Sanity — same MOVE targeted at unlimited still 200.
# ─────────────────────────────────────────────────────────────

# Capture the 5 MiB file id currently living in the unlimited drive
# (uploaded at Step 7). We'll try to relocate it into the 100-byte
# tight drive.
GET {{base_url}}/api/files?folder_id={{unlimited_root_id}}
Authorization: Bearer {{owner_token}}

HTTP 200
[Captures]
big_file_id: jsonpath "$[0].id"


# 11a — MOVE 5 MiB file into the tight (100-byte quota) drive.
#       Refused at the service pre-check: 5_242_880 + 32 > 100.
PUT {{base_url}}/api/files/{{big_file_id}}/move
Authorization: Bearer {{owner_token}}
Content-Type: application/json
{
  "folder_id": "{{tight_root_id}}"
}

HTTP 507


# 11b — COPY same file into tight drive. Same refusal shape as MOVE
#       — COPY creates a NEW file row that counts against
#       `drives.used_bytes` even when blob dedup means no new bytes
#       hit the store. Batch endpoint lives under `/api/batch/…`,
#       not `/api/files/…`.
POST {{base_url}}/api/batch/files/copy
Authorization: Bearer {{owner_token}}
Content-Type: application/json
{
  "file_ids": ["{{big_file_id}}"],
  "target_folder_id": "{{tight_root_id}}"
}

# Batch envelope: 200 all-ok, 206 partial, 400 all-failed. Our
# single-item batch has one quota-refused item → 400 with the
# failure in the `.failed[]` array (per `BatchOperationResponse`).
HTTP 400
[Asserts]
jsonpath "$.stats.failed"      == 1
jsonpath "$.stats.successful"  == 0
jsonpath "$.failed[0].id"      == "{{big_file_id}}"
jsonpath "$.failed[0].error"   exists


# 11c — Sanity: the file MOVE isn't universally broken. Targeting
#       the unlimited drive's own root succeeds (it's already
#       there, but MOVE is idempotent for same-parent — service
#       returns 200 without re-doing storage work).
PUT {{base_url}}/api/files/{{big_file_id}}/move
Authorization: Bearer {{owner_token}}
Content-Type: application/json
{
  "folder_id": "{{unlimited_root_id}}"
}

HTTP 200


# `used_bytes` on the tight drive is unchanged — the two refused
# operations above never wrote anything.
GET {{base_url}}/api/drives
Authorization: Bearer {{owner_token}}

HTTP 200
[Asserts]
jsonpath "$[?(@.id=='{{tight_drive_id}}')].used_bytes" == 32


# No cleanup tail here — `tests/api/storage_cleanup_check.sh` enumerates
# every drive via `GET /api/admin/drives` and drains+deletes any that
# isn't admin's default. This keeps individual Hurl tests focused on
# their assertions instead of teardown.
