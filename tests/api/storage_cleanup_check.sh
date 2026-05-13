#!/usr/bin/env bash
# =============================================================
# OxiCloud – Storage disk-cleanup verification
# =============================================================
# 1. Moves every live file and folder to trash via the REST API.
# 2. Calls DELETE /api/trash/empty to permanently delete all
#    remaining trash items (including any left by previous tests).
# 3. Asserts that no regular files remain under
#    $OXICLOUD_STORAGE_PATH/.thumbnails or .blobs.
#
# Called by run.sh after all Hurl tests have passed.
# Can also be run standalone (server must already be up):
#   bash tests/api/storage_cleanup_check.sh
# =============================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
STORAGE_PATH="${OXICLOUD_STORAGE_PATH:-$REPO_ROOT/tests/api/storage}"

# shellcheck source=test.env
source "$SCRIPT_DIR/test.env"

log()  { echo "[storage-check] $*"; }
fail() { echo $'\e[31m'"[storage-check] FAIL: $*"$'\e[0m' >&2; exit 1; }

# ── 1. Login ──────────────────────────────────────────────────────────────────

TOKEN=$(curl -sf -X POST "$base_url/api/auth/login" \
  -H "Content-Type: application/json" \
  -d "{\"username\":\"$username\",\"password\":\"$password\"}" \
  | jq -r '.access_token')

[[ -z "$TOKEN" || "$TOKEN" == "null" ]] && fail "login failed"
log "Logged in."

AUTH="Authorization: Bearer $TOKEN"

# ── 1b. Upload a probe image and verify its blob + thumbnail exist on disk ─────

# shellcheck source=../common/internal_storage_helper.sh
source "$REPO_ROOT/tests/common/internal_storage_helper.sh"

FIXTURE="$REPO_ROOT/tests/fixtures/blue-image.png"

HOME_FOLDER_ID=$(curl -sf -H "$AUTH" "$base_url/api/folders" | jq -r '.[0].id')
[[ -z "$HOME_FOLDER_ID" || "$HOME_FOLDER_ID" == "null" ]] && fail "could not get home folder id"

PROBE_FILE_ID=$(curl -sf -X POST -H "$AUTH" \
    -F "folder_id=$HOME_FOLDER_ID" \
    -F "file=@$FIXTURE;type=image/png" \
    "$base_url/api/files/upload" | jq -r '.id')
[[ -z "$PROBE_FILE_ID" || "$PROBE_FILE_ID" == "null" ]] && fail "probe file upload failed"
log "Probe file uploaded (id=$PROBE_FILE_ID)."

# GET thumbnail to trigger on-demand generation
HTTP_STATUS=$(curl -sf -o /dev/null -w "%{http_code}" -H "$AUTH" \
    "$base_url/api/files/$PROBE_FILE_ID/thumbnail/icon")
[[ "$HTTP_STATUS" != "200" ]] && fail "thumbnail GET returned HTTP $HTTP_STATUS (expected 200)"
log "Thumbnail fetched (HTTP 200)."

assert_local_blob_existsy "$FIXTURE" "$STORAGE_PATH" || fail "probe blob not found on disk"
assert_preview_existsy    "$FIXTURE" "$STORAGE_PATH" || fail "probe thumbnail not found on disk"
log "Probe blob and thumbnail confirmed present on disk."

# ── 2. Move all live files and folders to trash ───────────────────────────────
#
# For each root folder, list its direct children and soft-delete them.
# The server cascades folder deletion to all nested contents, so we only
# need to iterate one level deep.

ROOT_FOLDERS=$(curl -sf -H "$AUTH" "$base_url/api/folders" | jq -r '.[].id')

for folder_id in $ROOT_FOLDERS; do
    CONTENTS=$(curl -sf -H "$AUTH" "$base_url/api/folders/$folder_id/listing")

    while IFS= read -r sub_id; do
        [[ -z "$sub_id" ]] && continue
        curl -sf -X DELETE -H "$AUTH" "$base_url/api/folders/$sub_id" >/dev/null
    done < <(echo "$CONTENTS" | jq -r '.folders[].id')

    while IFS= read -r file_id; do
        [[ -z "$file_id" ]] && continue
        curl -sf -X DELETE -H "$AUTH" "$base_url/api/files/$file_id" >/dev/null
    done < <(echo "$CONTENTS" | jq -r '.files[].id')
done

log "All live objects moved to trash."

# ── 2b. Verify all root folders are empty according to the API ────────────────

for folder_id in $ROOT_FOLDERS; do
    CONTENTS=$(curl -sf -H "$AUTH" "$base_url/api/folders/$folder_id/listing")
    SUB_COUNT=$(echo "$CONTENTS"  | jq '.folders | length')
    FILE_COUNT=$(echo "$CONTENTS" | jq '.files   | length')
    if [[ "$SUB_COUNT" -ne 0 || "$FILE_COUNT" -ne 0 ]]; then
        fail "folder $folder_id still has $SUB_COUNT subfolder(s) and $FILE_COUNT file(s)"
    fi
done

log "API confirms all root folders are empty."

# ── 3. Permanently delete everything in trash ─────────────────────────────────

curl -sf -X DELETE -H "$AUTH" "$base_url/api/trash/empty" >/dev/null
log "Trash emptied."

# ── 3b. Verify trash is empty according to the API ───────────────────────────

TRASH_COUNT=$(curl -sf -H "$AUTH" "$base_url/api/trash" | jq 'length')
if [[ "$TRASH_COUNT" -ne 0 ]]; then
    fail "trash still contains $TRASH_COUNT item(s) after empty"
fi

log "API confirms trash is empty."

# ── 4. Disk verification ──────────────────────────────────────────────────────

THUMB_FILES=$(find "$STORAGE_PATH/.thumbnails" -type f 2>/dev/null || true)
BLOB_FILES=$(find  "$STORAGE_PATH/.blobs"      -type f 2>/dev/null || true)

if [[ -n "$THUMB_FILES" ]]; then
    THUMB_COUNT=$(echo "$THUMB_FILES" | wc -l | tr -d ' ')
    log "Leftover thumbnail files ($THUMB_COUNT):"
    echo "$THUMB_FILES"
    fail "$THUMB_COUNT thumbnail file(s) remain on disk after full cleanup"
fi

if [[ -n "$BLOB_FILES" ]]; then
    BLOB_COUNT=$(echo "$BLOB_FILES" | wc -l | tr -d ' ')
    log "Leftover blob files ($BLOB_COUNT):"
    echo "$BLOB_FILES"
    fail "$BLOB_COUNT blob file(s) remain on disk after full cleanup"
fi

log "OK — no blobs or thumbnails remain on disk."
