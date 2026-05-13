#!/usr/bin/env bash
# =============================================================
# OxiCloud – Bug 1 & 2: thumbnail refresh after WebDAV PUT
# =============================================================
# Bug 1: stale moka cache served after blob swap (overwrite)
# Bug 2: no background thumbnail generation after update
#
# Fix (webdav_handler.rs handle_put): after update branch,
#   refresh_thumbnails_after_update() calls delete_thumbnails()
#   (evicts moka) then spawns background regen from new blob.
#
# Test strategy:
#   1. PUT dedup-test.jpg via /webdav  → thumbnail generated
#   2. Prime the moka cache with GET /thumbnail
#   3. PUT oxicloud-logo.jpg to the same path (overwrite)
#   4. GET /thumbnail again → must return different bytes
#
# Bug 1 detection: if moka is not evicted, step 4 returns the
#   cached dedup-test thumbnail → SHA-256 matches step 2 → test fails.
#
# Prerequisites:
#   - Server running at base_url with credentials from test.env
#   - OXICLOUD_ENABLE_AUTH=true (/webdav uses JWT Bearer auth)
#   - jq in PATH
#
# Run (from repo root):
#   bash tests/webdav/test_thumbnail_update.sh
# =============================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$SCRIPT_DIR"

source test.env
source common.sh


# ── helpers ──────────────────────────────────────────────────

PASS=0
FAIL=0

pass() { PASS=$(( PASS + 1 )); echo "  PASS: $*"; }
fail() { FAIL=$(( FAIL + 1 )); echo "  FAIL: $*" >&2; exit 1; }

# WebDAV PUT: returns HTTP status code
webdav_put() {
    local remote_name="$1" local_file="$2" mime="${3:-application/octet-stream}"
    curl -s -o /dev/null -w "%{http_code}" \
        -X PUT \
        -H "Authorization: Bearer $TOKEN" \
        -H "Content-Type: $mime" \
        --data-binary "@$local_file" \
        "$base_url/webdav/$remote_name"
}

# WebDAV DELETE: returns HTTP status code
webdav_delete() {
    local remote_name="$1"
    curl -s -o /dev/null -w "%{http_code}" \
        -X DELETE \
        -H "Authorization: Bearer $TOKEN" \
        "$base_url/webdav/$remote_name"
}

# REST GET with JWT bearer
rest_get() {
    curl -s \
        -H "Authorization: Bearer $TOKEN" \
        "$base_url$1"
}

# REST DELETE with JWT bearer, returns HTTP status code
rest_delete() {
    curl -s -o /dev/null -w "%{http_code}" \
        -X DELETE \
        -H "Authorization: Bearer $TOKEN" \
        "$base_url$1"
}

# Download thumbnail and return its SHA-256 checksum
thumbnail_sha256() {
    local file_id="$1"
    curl -s \
        -H "Authorization: Bearer $TOKEN" \
        "$base_url/api/files/$file_id/thumbnail/icon" \
    | sha256sum | cut -d' ' -f1
}

# SHA-256 of an empty stream (thumbnail missing = empty body)
EMPTY_SHA="e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"

FIXTURE_V1="$REPO_ROOT/tests/fixtures/dedup-test.jpg"
FIXTURE_V2="$REPO_ROOT/tests/fixtures/oxicloud-logo.jpg"

[[ -f "$FIXTURE_V1" ]] || { echo "Missing fixture: $FIXTURE_V1" >&2; exit 1; }
[[ -f "$FIXTURE_V2" ]] || { echo "Missing fixture: $FIXTURE_V2" >&2; exit 1; }

echo
echo "=== Bug 1 & 2: thumbnail refresh after WebDAV PUT overwrite ==="
echo

# ── authenticate ─────────────────────────────────────────────

oxicloud_login

# ── home folder ID ────────────────────────────────────────────

echo "  home folder..."
HOME_FOLDER_ID=$(rest_get "/api/folders" | jq -r '.[0].id')
[[ -n "$HOME_FOLDER_ID" && "$HOME_FOLDER_ID" != "null" ]] \
    || fail "Could not retrieve home folder ID"
echo "  home folder id: $HOME_FOLDER_ID"

REMOTE="webdav-thumb-bug12.jpg"

# ── Pre-test cleanup (idempotent) ─────────────────────────────
# Remove any leftover from a previous run via the REST API so that
# the first WebDAV PUT below is guaranteed to be a CREATE (201).

echo "  cleanup: checking regular listing for '$REMOTE'..."
EXISTING_ID=$(rest_get "/api/files?folder_id=$HOME_FOLDER_ID" \
    | jq -r --arg n "$REMOTE" 'first(.[] | select(.name == $n) | .id) // empty')
if [[ -n "$EXISTING_ID" ]]; then
    echo "  cleanup: found existing file id=$EXISTING_ID — deleting..."
    ST=$(rest_delete "/api/files/$EXISTING_ID")
    echo "  cleanup: DELETE /api/files/$EXISTING_ID → $ST"
else
    echo "  cleanup: no existing file in regular listing"
fi

echo "  cleanup: checking trash for '$REMOTE'..."
STALE=$(rest_get "/api/trash" \
    | jq -r --arg n "$REMOTE" 'first(.[] | select(.name == $n) | .id) // empty')
if [[ -n "$STALE" ]]; then
    echo "  cleanup: found trash item id=$STALE — purging..."
    ST=$(rest_delete "/api/trash/$STALE")
    echo "  cleanup: DELETE /api/trash/$STALE → $ST"
else
    echo "  cleanup: trash is clean"
fi

# ── Step 1: PUT dedup-test.jpg ───────────────────────────────
# /webdav always returns 204 (update_file_streaming handles create+update)

echo "  step 1: PUT $REMOTE..."
STATUS=$(webdav_put "$REMOTE" "$FIXTURE_V1" "image/jpeg")
echo "  step 1: WebDAV PUT → $STATUS"
[[ "$STATUS" == "204" ]] || fail "WebDAV PUT expected 204, got $STATUS"
pass "WebDAV PUT dedup-test.jpg → 204"

# ── find file_id from REST listing ───────────────────────────

echo "  step 1: resolving file_id..."
FILE_ID=$(rest_get "/api/files?folder_id=$HOME_FOLDER_ID" \
    | jq -r --arg n "$REMOTE" '.[] | select(.name == $n) | .id')
[[ -n "$FILE_ID" && "$FILE_ID" != "null" ]] \
    || fail "File '$REMOTE' not found in folder listing after WebDAV PUT"
pass "File found via REST API — id=$FILE_ID"

# ── Step 2: GET thumbnail to prime moka cache ────────────────
# Background generation may still be running; wait briefly.

echo "  step 2: waiting 1s for background thumbnail generation..."
sleep 1

echo "  step 2: GET /thumbnail/icon..."
HTTP=$(curl -s -o /dev/null -w "%{http_code}" \
    -H "Authorization: Bearer $TOKEN" \
    "$base_url/api/files/$FILE_ID/thumbnail/icon")
echo "  step 2: GET /thumbnail → $HTTP"
[[ "$HTTP" == "200" ]] \
    || fail "GET /thumbnail after initial upload expected 200, got $HTTP"

THUMB_V1=$(thumbnail_sha256 "$FILE_ID")
echo "  step 2: thumbnail sha256=$THUMB_V1"
[[ -n "$THUMB_V1" && "$THUMB_V1" != "$EMPTY_SHA" ]] \
    || fail "Thumbnail after initial upload is empty (sha256=$THUMB_V1)"
pass "Initial thumbnail present and non-empty (sha256=$THUMB_V1)"

# ── Step 3: PUT oxicloud-logo.jpg (overwrite) ────────────────

echo "  step 3: PUT $REMOTE (overwrite)..."
STATUS=$(webdav_put "$REMOTE" "$FIXTURE_V2" "image/jpeg")
echo "  step 3: WebDAV PUT → $STATUS"
[[ "$STATUS" == "204" ]] || fail "WebDAV PUT (overwrite) expected 204, got $STATUS"
pass "WebDAV PUT oxicloud-logo.jpg overwrite → 204 No Content"

# ── Step 4: GET thumbnail after overwrite ────────────────────
# Fix: delete_thumbnails() evicts moka (bug 1),
#      background regen populates new blob thumbnail (bug 2).
# Without the fix the stale dedup-test thumbnail is served from moka.

echo "  step 4: waiting 1s for background thumbnail regeneration..."
sleep 1

echo "  step 4: GET /thumbnail/icon after overwrite..."
HTTP=$(curl -s -o /dev/null -w "%{http_code}" \
    -H "Authorization: Bearer $TOKEN" \
    "$base_url/api/files/$FILE_ID/thumbnail/icon")
echo "  step 4: GET /thumbnail → $HTTP"
[[ "$HTTP" == "200" ]] \
    || fail "GET /thumbnail after overwrite expected 200, got $HTTP"

THUMB_V2=$(thumbnail_sha256 "$FILE_ID")
echo "  step 4: thumbnail sha256=$THUMB_V2"
[[ -n "$THUMB_V2" && "$THUMB_V2" != "$EMPTY_SHA" ]] \
    || fail "Thumbnail after overwrite is empty"

[[ "$THUMB_V1" != "$THUMB_V2" ]] \
    || fail "Bug 1 present: moka cache not evicted — thumbnail unchanged after overwrite (sha256=$THUMB_V1)"
pass "Thumbnail changed after overwrite — bugs 1 & 2 fixed (sha256=$THUMB_V2)"

# ── cleanup ───────────────────────────────────────────────────

echo "  cleanup: WebDAV DELETE $REMOTE..."
STATUS=$(webdav_delete "$REMOTE")
echo "  cleanup: WebDAV DELETE → $STATUS"
[[ "$STATUS" == "204" ]] || fail "WebDAV DELETE expected 204, got $STATUS"
pass "WebDAV DELETE → 204"

TRASH_ITEM=$(rest_get "/api/trash" \
    | jq -r --arg n "$REMOTE" '.[] | select(.name == $n) | .id // empty')
if [[ -n "$TRASH_ITEM" ]]; then
    ST=$(rest_delete "/api/trash/$TRASH_ITEM")
    echo "  cleanup: DELETE /api/trash/$TRASH_ITEM → $ST"
    pass "Permanently deleted from trash"
fi

# ── summary ───────────────────────────────────────────────────

echo
echo "Results: $PASS passed, $FAIL failed."
[[ "$FAIL" -eq 0 ]] && echo "All tests passed." || exit 1
