#!/usr/bin/env bash
# =============================================================
# OxiCloud – Dedup ref_count via WebDAV (two uploads + overwrite)
# =============================================================
# Scenario:
#   1. PUT dedup-test.jpg via WebDAV as file A
#   2. PUT dedup-test-2.jpg (identical content) via WebDAV as file B
#      → same blob, two distinct file records, ref_count == 2
#   3. GET /api/dedup/check/{hash} → assert ref_count == 2
#   4. Overwrite file B via WebDAV PUT with different content
#      → file B now references a new blob; original ref_count drops
#   5. GET /api/dedup/check/{hash} → assert ref_count == 1
#
# BLAKE3 hash of dedup-test.jpg (== dedup-test-2.jpg — same content):
#   cde1ca663a2e62e0dadb41c3194e11ecb7d971d84c7451db17063b55c09e8066
#
# Prerequisites:
#   - Server running at base_url with credentials from test.env
#   - OXICLOUD_ENABLE_AUTH=true (/webdav uses JWT Bearer auth)
#   - jq in PATH
#
# Run (from repo root):
#   bash tests/webdav/test_dedup_webdav_ref_count.sh
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

rest_get() {
    curl -s -H "Authorization: Bearer $TOKEN" "$base_url$1"
}

rest_delete() {
    curl -s -o /dev/null -w "%{http_code}" \
        -X DELETE \
        -H "Authorization: Bearer $TOKEN" \
        "$base_url$1"
}

dedup_check() {
    curl -s -H "Authorization: Bearer $TOKEN" "$base_url/api/dedup/check/$1"
}

# ── fixtures ──────────────────────────────────────────────────────────────────

# BLAKE3 hash of dedup-test.jpg (= dedup-test-2.jpg — byte-identical content)
BLOB_HASH="cde1ca663a2e62e0dadb41c3194e11ecb7d971d84c7451db17063b55c09e8066"

FIXTURE_A="$REPO_ROOT/tests/fixtures/dedup-test.jpg"
FIXTURE_B="$REPO_ROOT/tests/fixtures/dedup-test-2.jpg"
FIXTURE_OTHER="$REPO_ROOT/tests/fixtures/oxicloud-logo.jpg"

[[ -f "$FIXTURE_A" ]]     || { echo "Missing fixture: $FIXTURE_A" >&2; exit 1; }
[[ -f "$FIXTURE_B" ]]     || { echo "Missing fixture: $FIXTURE_B" >&2; exit 1; }
[[ -f "$FIXTURE_OTHER" ]] || { echo "Missing fixture: $FIXTURE_OTHER" >&2; exit 1; }

FILE_A="webdav-dedup-ref-a.jpg"
FILE_B="webdav-dedup-ref-b.jpg"

echo
echo "=== Dedup ref_count: two WebDAV uploads + overwrite ==="
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
    STALE=$(rest_get "/api/trash" \
        | jq -r --arg n "$REMOTE" 'first(.[] | select(.name == $n) | .id) // empty')
    if [[ -n "$STALE" ]]; then
        echo "  cleanup: purging $REMOTE from trash (id=$STALE)"
        rest_delete "/api/trash/$STALE" > /dev/null
    fi
done

# ── Step 1: Upload file A ─────────────────────────────────────────────────────

echo "  step 1: PUT $FILE_A (dedup-test.jpg)..."
STATUS=$(webdav_put "$FILE_A" "$FIXTURE_A" "image/jpeg")
[[ "$STATUS" == "204" ]] || fail "PUT $FILE_A expected 204, got $STATUS"
pass "PUT $FILE_A → 204"

# ── Step 2: Upload file B (identical content, different name) ─────────────────

echo "  step 2: PUT $FILE_B (dedup-test-2.jpg, same bytes)..."
STATUS=$(webdav_put "$FILE_B" "$FIXTURE_B" "image/jpeg")
[[ "$STATUS" == "204" ]] || fail "PUT $FILE_B expected 204, got $STATUS"
pass "PUT $FILE_B → 204"

# ── Step 3: Resolve file IDs and assert two distinct records ──────────────────

FILE_LISTING=$(rest_get "/api/files?folder_id=$HOME_FOLDER_ID")
FILE_A_ID=$(jq -r --arg n "$FILE_A" '.[] | select(.name == $n) | .id' <<< "$FILE_LISTING")
FILE_B_ID=$(jq -r --arg n "$FILE_B" '.[] | select(.name == $n) | .id' <<< "$FILE_LISTING")

[[ -n "$FILE_A_ID" && "$FILE_A_ID" != "null" ]] || fail "File A not found in listing"
[[ -n "$FILE_B_ID" && "$FILE_B_ID" != "null" ]] || fail "File B not found in listing"
[[ "$FILE_A_ID" != "$FILE_B_ID" ]] \
    || fail "File A and B share the same ID — dedup must produce two distinct records"
pass "Two distinct file records: A=$FILE_A_ID  B=$FILE_B_ID"

# ── Step 4: Dedup check → ref_count == 2 ─────────────────────────────────────

echo "  step 4: GET /api/dedup/check/$BLOB_HASH..."
RESP=$(dedup_check "$BLOB_HASH")
EXISTS=$(jq -r '.exists'    <<< "$RESP")
RC=$(    jq -r '.ref_count' <<< "$RESP")

[[ "$EXISTS" == "true" ]] \
    || fail "dedup/check: expected exists=true, got $EXISTS (full response: $RESP)"
[[ "$RC" == "2" ]] \
    || fail "dedup/check: expected ref_count=2 after two identical uploads, got $RC"
pass "ref_count == 2: both files reference the same blob"

# ── Step 5: Overwrite file B with different content ───────────────────────────
# swap_blob_hash calls remove_reference on the old hash → manifest ref_count 2→1

echo "  step 5: PUT $FILE_B (oxicloud-logo.jpg, new content)..."
STATUS=$(webdav_put "$FILE_B" "$FIXTURE_OTHER" "image/jpeg")
[[ "$STATUS" == "204" ]] || fail "PUT $FILE_B (overwrite) expected 204, got $STATUS"
pass "PUT $FILE_B overwrite → 204"

# ── Step 6: Dedup check → ref_count == 1 ─────────────────────────────────────
# File B now references a different blob; file A still holds the original.

echo "  step 6: GET /api/dedup/check/$BLOB_HASH..."
RESP=$(dedup_check "$BLOB_HASH")
EXISTS=$(jq -r '.exists'    <<< "$RESP")
RC=$(    jq -r '.ref_count' <<< "$RESP")

[[ "$EXISTS" == "true" ]] \
    || fail "dedup/check: expected exists=true (file A still references blob), got $EXISTS"
[[ "$RC" == "1" ]] \
    || fail "dedup/check: expected ref_count=1 after overwriting file B, got $RC"
pass "ref_count == 1: only file A still references the original blob"

# ── cleanup ───────────────────────────────────────────────────────────────────

echo "  cleanup..."
for REMOTE in "$FILE_A" "$FILE_B"; do
    ST=$(webdav_delete "$REMOTE")
    [[ "$ST" == "204" ]] || fail "WebDAV DELETE $REMOTE expected 204, got $ST"
    TRASH_ITEM=$(rest_get "/api/trash" \
        | jq -r --arg n "$REMOTE" 'first(.[] | select(.name == $n) | .id) // empty')
    if [[ -n "$TRASH_ITEM" ]]; then
        rest_delete "/api/trash/$TRASH_ITEM" > /dev/null
    fi
done
pass "Cleanup complete"

# ── summary ───────────────────────────────────────────────────────────────────

echo
echo "Results: $PASS passed, $FAIL failed."
[[ "$FAIL" -eq 0 ]] && echo "All tests passed." || exit 1
