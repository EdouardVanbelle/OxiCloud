#!/usr/bin/env bash
# =============================================================
# OxiCloud – Dedup ref_count: multi-chunk CDC file via WebDAV
# =============================================================
# Case A (files < 64kB = 1 chunk) is treated in tests/api/dedup_blob_cleanup.hurl
# Validates Case B of the cleanup_if_orphaned fix:
#   file_hash ≠ chunk_hashes  (file split into 8 CDC chunks)
#
# free_video_over_1MB.mp4 — 2.6 MB, 8 CDC chunks
#   BLAKE3: 95d42b25a2d39f24f1b2f38bf1b947d4ec74201271a98ea0e76a9cea421eff80
#
# Sequence:
#   1. PUT video as file A  → new manifest (ref_count=1, 8 chunk blobs)
#   2. PUT video as file B  → dedup hit   (ref_count=2, chunks unchanged)
#   3. /api/dedup/check     → ref_count == 2
#   4. Permanently delete file A
#   5. /api/dedup/check     → ref_count == 1  (chunks must NOT be freed)
#   6. Permanently delete file B
#   7. /api/dedup/check     → exists == false (manifest + chunks cleaned up)
#
# Case B regression: cleanup_if_orphaned must decrement chunk_manifests
# ref_count without touching chunk blobs (the PG trigger is a no-op for
# multi-chunk files because file_hash is not stored in storage.blobs).
#
# Prerequisites:
#   - Server running at base_url with credentials from test.env
#   - OXICLOUD_ENABLE_AUTH=true
#   - jq in PATH
#
# Run (from repo root):
#   bash tests/webdav/test_dedup_webdav_multichunk.sh
# =============================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$SCRIPT_DIR"

source test.env
source common.sh

# ── helpers ──────────────────────────────────────────────────────────────────

PASS=0
FAIL=0

pass() { PASS=$(( PASS + 1 )); echo "  PASS: $*"; }
fail() { FAIL=$(( FAIL + 1 )); echo "  FAIL: $*" >&2; exit 1; }

webdav_put() {
    local remote_name="$1" local_file="$2" mime="${3:-application/octet-stream}"
    curl -s -o /dev/null -w "%{http_code}" \
        -X PUT \
        -H "Authorization: Bearer $TOKEN" \
        -H "Content-Type: $mime" \
        --data-binary "@$local_file" \
        "$base_url/webdav/$remote_name"
}

webdav_delete() {
    local remote_name="$1"
    curl -s -o /dev/null -w "%{http_code}" \
        -X DELETE \
        -H "Authorization: Bearer $TOKEN" \
        "$base_url/webdav/$remote_name"
}

rest_get()    { curl -s -H "Authorization: Bearer $TOKEN" "$base_url$1"; }
rest_delete() { curl -s -o /dev/null -w "%{http_code}" -X DELETE -H "Authorization: Bearer $TOKEN" "$base_url$1"; }
dedup_check() { curl -s -H "Authorization: Bearer $TOKEN" "$base_url/api/dedup/check/$1"; }

purge_from_trash() {
    local name="$1"
    local tid
    tid=$(rest_get "/api/trash" \
        | jq -r --arg n "$name" 'first(.[] | select(.name == $n) | .id) // empty')
    [[ -n "$tid" ]] && rest_delete "/api/trash/$tid" > /dev/null || true
}

# ── fixture ───────────────────────────────────────────────────────────────────

# free_video_over_1MB.mp4 → 2.6 MB → 8 CDC chunks → confirmed Case B
BLOB_HASH="95d42b25a2d39f24f1b2f38bf1b947d4ec74201271a98ea0e76a9cea421eff80"
FIXTURE="$REPO_ROOT/tests/fixtures/free_video_over_1MB.mp4"
[[ -f "$FIXTURE" ]] || { echo "Missing fixture: $FIXTURE" >&2; exit 1; }

FILE_A="webdav-dedup-mc-a.mp4"
FILE_B="webdav-dedup-mc-b.mp4"

echo
echo "=== Dedup ref_count: multi-chunk CDC (Case B) via WebDAV ==="
echo

# ── authenticate ──────────────────────────────────────────────────────────────

oxicloud_login

# ── home folder ───────────────────────────────────────────────────────────────

HOME_FOLDER_ID=$(rest_get "/api/folders" | jq -r '.[0].id')
[[ -n "$HOME_FOLDER_ID" && "$HOME_FOLDER_ID" != "null" ]] \
    || fail "Could not retrieve home folder ID"
echo "  home folder id: $HOME_FOLDER_ID"

# ── idempotent pre-test cleanup ───────────────────────────────────────────────

for REMOTE in "$FILE_A" "$FILE_B"; do
    EXISTING_ID=$(rest_get "/api/files?folder_id=$HOME_FOLDER_ID" \
        | jq -r --arg n "$REMOTE" 'first(.[] | select(.name == $n) | .id) // empty')
    if [[ -n "$EXISTING_ID" ]]; then
        echo "  cleanup: deleting existing $REMOTE (id=$EXISTING_ID)"
        rest_delete "/api/files/$EXISTING_ID" > /dev/null
    fi
    purge_from_trash "$REMOTE"
done

# ── Step 1: Upload file A ─────────────────────────────────────────────────────

echo "  step 1: PUT $FILE_A..."
STATUS=$(webdav_put "$FILE_A" "$FIXTURE" "video/mp4")
[[ "$STATUS" == "204" ]] || fail "PUT $FILE_A expected 204, got $STATUS"
pass "PUT $FILE_A → 204  (new manifest, 8 chunk blobs created)"

# ── Step 2: Upload file B (same content, different name → dedup hit) ──────────

echo "  step 2: PUT $FILE_B (same bytes → dedup hit)..."
STATUS=$(webdav_put "$FILE_B" "$FIXTURE" "video/mp4")
[[ "$STATUS" == "204" ]] || fail "PUT $FILE_B expected 204, got $STATUS"
pass "PUT $FILE_B → 204  (dedup hit: manifest ref_count → 2, chunks unchanged)"

# ── Resolve file IDs ──────────────────────────────────────────────────────────

LISTING=$(rest_get "/api/files?folder_id=$HOME_FOLDER_ID")
FILE_A_ID=$(jq -r --arg n "$FILE_A" '.[] | select(.name == $n) | .id' <<< "$LISTING")
FILE_B_ID=$(jq -r --arg n "$FILE_B" '.[] | select(.name == $n) | .id' <<< "$LISTING")

[[ -n "$FILE_A_ID" && "$FILE_A_ID" != "null" ]] || fail "File A not found in listing"
[[ -n "$FILE_B_ID" && "$FILE_B_ID" != "null" ]] || fail "File B not found in listing"
[[ "$FILE_A_ID" != "$FILE_B_ID" ]] \
    || fail "File A and B share the same ID — dedup must create two distinct records"
pass "Two distinct file records: A=$FILE_A_ID  B=$FILE_B_ID"

# ── Step 3: ref_count == 2 ────────────────────────────────────────────────────

echo "  step 3: dedup/check → expect ref_count=2..."
RESP=$(dedup_check "$BLOB_HASH")
EXISTS=$(jq -r '.exists'    <<< "$RESP")
RC=$(    jq -r '.ref_count' <<< "$RESP")

[[ "$EXISTS" == "true" ]] \
    || fail "dedup/check: expected exists=true, got $EXISTS  (response: $RESP)"
[[ "$RC" == "2" ]] \
    || fail "dedup/check: expected ref_count=2, got $RC"
pass "ref_count == 2: both files reference the same 8-chunk blob"

# ── Step 4: Permanently delete file A ────────────────────────────────────────
# For multi-chunk files: PG trigger is a no-op (file_hash not in storage.blobs).
# cleanup_if_orphaned must decrement chunk_manifests.ref_count only (2→1)
# and leave the 8 chunk blobs untouched.

echo "  step 4: trash + permanently delete $FILE_A..."
ST=$(rest_delete "/api/files/$FILE_A_ID")
[[ "$ST" == "204" ]] || fail "DELETE $FILE_A expected 204, got $ST"

TRASH_A=$(rest_get "/api/trash" \
    | jq -r --arg n "$FILE_A" 'first(.[] | select(.name == $n) | .id) // empty')
[[ -n "$TRASH_A" ]] || fail "File A not found in trash"
ST=$(rest_delete "/api/trash/$TRASH_A")
[[ "$ST" == "200" ]] || fail "Permanent delete file A expected 200, got $ST"
pass "File A permanently deleted"

# ── Step 5: ref_count == 1 — chunk blobs must still be alive ─────────────────

echo "  step 5: dedup/check → expect ref_count=1 (chunks must survive)..."
RESP=$(dedup_check "$BLOB_HASH")
EXISTS=$(jq -r '.exists'    <<< "$RESP")
RC=$(    jq -r '.ref_count' <<< "$RESP")

[[ "$EXISTS" == "true" ]] \
    || fail "dedup/check: expected exists=true (file B still references blob), got $EXISTS"
[[ "$RC" == "1" ]] \
    || fail "dedup/check: expected ref_count=1, got $RC  (chunk blobs may have been freed prematurely)"
pass "ref_count == 1: manifest decremented, all 8 chunk blobs still alive"

# ── Step 6: Permanently delete file B ────────────────────────────────────────
# ref_count hits 0 → manifest deleted, all 8 chunk blobs freed.

echo "  step 6: trash + permanently delete $FILE_B..."
ST=$(rest_delete "/api/files/$FILE_B_ID")
[[ "$ST" == "204" ]] || fail "DELETE $FILE_B expected 204, got $ST"

TRASH_B=$(rest_get "/api/trash" \
    | jq -r --arg n "$FILE_B" 'first(.[] | select(.name == $n) | .id) // empty')
[[ -n "$TRASH_B" ]] || fail "File B not found in trash"
ST=$(rest_delete "/api/trash/$TRASH_B")
[[ "$ST" == "200" ]] || fail "Permanent delete file B expected 200, got $ST"
pass "File B permanently deleted"

# ── Step 7: blob gone ─────────────────────────────────────────────────────────

echo "  step 7: dedup/check → expect exists=false (manifest + chunks freed)..."
RESP=$(dedup_check "$BLOB_HASH")
EXISTS=$(jq -r '.exists' <<< "$RESP")

[[ "$EXISTS" == "false" ]] \
    || fail "dedup/check: expected exists=false after both files deleted, got $EXISTS"
pass "exists == false: manifest and all 8 chunk blobs cleaned up"

# ── summary ───────────────────────────────────────────────────────────────────

echo
echo "Results: $PASS passed, $FAIL failed."
[[ "$FAIL" -eq 0 ]] && echo "All tests passed." || exit 1
