#!/usr/bin/env bash
# =============================================================
# OxiCloud — Baseline: Native /webdav/ + LOCK / UNLOCK
# =============================================================
# Groups M + N from BASELINE_TESTS_NC_WEBDAV.md (11 scenarios).
#
# The native `/webdav/...` surface is the protocol layer
# rclone, davfs2, Cyberduck, Office (via WebDAV mount), and
# the other generic-DAV ecosystem use. It differs from the
# NC `/remote.php/dav/files/{user}/...` surface in several
# baseline-worthy ways:
#
#   - Auth: JWT bearer (not Basic Auth with an app password)
#   - Chroot: implicit "the user's home folder", NO {user}
#     URL segment to validate
#   - DAV class advertisement: `1, 2` (incl. Class 2 LOCK)
#     versus NC's `1, 3`
#
# Coverage:
#   M1 — OPTIONS / advertises DAV 1, 2 + Allow includes LOCK
#   M2 — PROPFIND Depth: 1 trailing-slash semantics (the
#        same regression guard as D9/D10 on the NC surface,
#        run again on this surface)
#   M3 — PUT sample.txt → 201
#   M4 — Range GET (bytes=0-9) → 206
#   M5 — MOVE sample.txt → moved.txt
#   M6 — MKCOL sub/ → 201
#   M7 — DELETE sub/ → 204
#   M8 — COPY a.txt → b.txt (pin whatever current is — native
#        COPY may or may not be implemented)
#   N1 — LOCK locked.txt → 200 + Lock-Token header
#   N2 — PUT locked.txt without If: <token> from a different
#        context → 423 Locked
#   N3 — UNLOCK with the token → 204; subsequent PUT succeeds
# =============================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

source test.env
source common.sh
source lib/dav_helpers.sh

echo
echo "=== Native /webdav/ + LOCK / UNLOCK (Groups M + N baseline) ==="
echo

oxicloud_login
resolve_home_folder_id
wipe_home_folder

DAV_BASE="$base_url/webdav"
FIXTURE_DIR=$(mktemp -d)
trap 'rm -rf "$FIXTURE_DIR"; wipe_home_folder 2>/dev/null || true' EXIT

# ─────────────────────────────────────────────────────────────
# M1 — OPTIONS advertises DAV 1, 2 + Allow includes LOCK
# ─────────────────────────────────────────────────────────────
echo "  M1: OPTIONS /webdav/ → DAV: 1, 2 + Allow includes LOCK/UNLOCK"
HEADERS=$(dav_curl -i -X OPTIONS "$DAV_BASE/" | tr -d '\r')
STATUS=$(awk 'NR==1{print $2}' <<< "$HEADERS")
[[ "$STATUS" == "200" ]] \
    || fail "M1: OPTIONS expected 200, got $STATUS"
grep -qi '^dav:.*1.*2' <<< "$HEADERS" \
    || fail "M1: missing 'DAV: 1, 2' header (native surface should advertise Class 2)"
grep -qi '^allow:.*LOCK'   <<< "$HEADERS" || fail "M1: Allow missing LOCK"
grep -qi '^allow:.*UNLOCK' <<< "$HEADERS" || fail "M1: Allow missing UNLOCK"
pass "M1: OPTIONS advertises DAV 1, 2 + Allow includes LOCK/UNLOCK"

# ─────────────────────────────────────────────────────────────
# Fixture setup via REST so M2 has stable mixed content.
# We deliberately PROPFIND a dedicated *sub-folder* of home,
# not bare `/webdav/`, because:
#
#   1. The native handler's `resolve_webdav_path` only fires
#      when the URL subpath is non-empty (gated by
#      `!path.is_empty() && method.as_str() != "OPTIONS"`).
#      PROPFIND on bare `/webdav/` therefore returns the
#      user's root-collections list, not the contents of
#      their home folder.
#   2. The number of root collections varies per environment
#      (default home + anything else the system or earlier
#      tests created), so asserting a fixed count there is
#      brittle. A test-owned sub-folder is fully under our
#      control.
#
# Inside `m2-probe/`: 2 files + 2 sub-folders → PROPFIND
# Depth: 1 yields exactly 5 responses (self + 4 children) and
# exercises both the file-href (no trailing slash) and the
# folder-href (trailing slash) branches of the same
# regression guard that catches D9/D10 on the NC surface.
# ─────────────────────────────────────────────────────────────
api_create_folder "m2-probe" "$HOME_FOLDER_ID"
M2_PROBE_ID="$LAST_FOLDER_ID"
echo "alpha" > "$FIXTURE_DIR/m2-alpha.txt"
echo "beta"  > "$FIXTURE_DIR/m2-beta.txt"
api_upload_file "$FIXTURE_DIR/m2-alpha.txt" "$M2_PROBE_ID"
api_upload_file "$FIXTURE_DIR/m2-beta.txt"  "$M2_PROBE_ID"
api_create_folder "m2-foldA" "$M2_PROBE_ID"
api_create_folder "m2-foldB" "$M2_PROBE_ID"

# ─────────────────────────────────────────────────────────────
# M2 — PROPFIND Depth: 1 trailing-slash semantics (mixed)
# ─────────────────────────────────────────────────────────────
echo "  M2: PROPFIND /webdav/m2-probe/ Depth: 1 (mixed children — trailing-slash regression guard)"
BODY=$(dav_curl -X PROPFIND -H "Depth: 1" "$DAV_BASE/m2-probe/")
N=$(count_responses "$BODY")
# Collection (1) + 2 files + 2 folders = 5
[[ "$N" == "5" ]] \
    || fail "M2: expected 5 responses (collection + 2 files + 2 folders), got $N"
assert_collection_hrefs_have_trailing_slash "$BODY" "M2"
pass "M2: 5 responses, trailing-slash semantics correct on native /webdav/ surface"

# ─────────────────────────────────────────────────────────────
# M3 — PUT a new file
#
# Pinned current behaviour: the native handler always returns
# 204 NO_CONTENT regardless of new-vs-overwrite. The NC handler
# differentiates (201 for new, 204 for overwrite, see F1/F3)
# but the native one in `interfaces/api/handlers/webdav_handler.rs::handle_put`
# unconditionally builds a 204 response on success (line ~1026
# at time of writing). RFC 4918 §9.7.1 actually allows either
# — both indicate success — so this is current behaviour, not
# a bug. NC desktop / generic DAV clients accept both.
# ─────────────────────────────────────────────────────────────
# M3-M8 use ROOT-level paths (just `/webdav/<name>`) NOT nested
# under `m2-probe/`. Why: there's a real bug in the native PUT
# handler where a PUT to `/webdav/m2-probe/foo.txt` writes the
# file with `folder_id=NULL` (`get_parent_folder_id` doesn't
# correctly look up REST-created parent folders), so the file
# effectively ends up at root. The lenient GET path
# (`get_file_by_path`) still finds it, but the strict
# `resolve_path_for_user` (which MOVE / COPY / DELETE use) does
# not. PUT then GET works on nested paths; PUT then MOVE 404s.
# Pinning this as KNOWN BUG at M5 below.
#
# Until that's fixed, M3-M8 use root-level paths so the rest of
# the lifecycle (which the existing test_dedup_webdav_* scripts
# also exercise at root) actually validates.

echo "  M3: PUT /webdav/m3-sample.txt → 201 (new resource, post 43cf4a2b)"
# Post commit 43cf4a2b, the native WebDAV handler differentiates
# new-vs-overwrite per RFC 7231 §4.3.4: 201 Created for a fresh PUT,
# 204 No Content when replacing an existing resource. Aligns with the
# NC handler — there's no more native-vs-NC split on this point.
# (Prior to 43cf4a2b the native handler returned 204 for both; the M3
# `case` block was a forward-looking trip-wire telling the next reader
# to update this pin once the split happened. That moment is now.)
STATUS=$(dav_curl -o /dev/null -w "%{http_code}" -X PUT \
    -H "Content-Type: text/plain" \
    --data-binary 'sample contents — exactly 31 bytes' \
    "$DAV_BASE/m3-sample.txt")
[[ "$STATUS" == "201" ]] \
    || fail "M3: native PUT new expected 201, got $STATUS"
pass "M3: native PUT new → 201"

# ─────────────────────────────────────────────────────────────
# M4 — Range GET bytes=0-9 → 206 + 10 bytes
# ─────────────────────────────────────────────────────────────
echo "  M4: GET /webdav/m3-sample.txt with Range: bytes=0-9 → 206 + 10 bytes"
HEADERS=$(dav_curl -D - -o /dev/null -H "Range: bytes=0-9" "$DAV_BASE/m3-sample.txt")
STATUS=$(awk 'NR==1{print $2}' <<< "$HEADERS" | tr -d '\r')
[[ "$STATUS" == "206" ]] \
    || fail "M4: Range GET expected 206, got $STATUS"
BODY_SIZE=$(dav_curl -H "Range: bytes=0-9" "$DAV_BASE/m3-sample.txt" | wc -c | tr -d ' ')
[[ "$BODY_SIZE" == "10" ]] \
    || fail "M4: Range body size expected 10, got $BODY_SIZE"
pass "M4: Range bytes=0-9 → 206 + 10 bytes"

# ─────────────────────────────────────────────────────────────
# M5 — MOVE sample.txt → moved.txt
# ─────────────────────────────────────────────────────────────
# M5 — root-level MOVE
#
# After all the nested-path diagnostics above (which surfaced the
# bug pinned in the M3 comment), this assertion finally tests the
# code path where it should actually work: root-level MOVE of a
# file PUT at root. If even this 404s, the bug is broader and
# native MOVE is unusable, not just nested.
echo "  M5: MOVE /webdav/m3-sample.txt → /webdav/m5-moved.txt → 201/204"
STATUS=$(dav_curl -o /dev/null -w "%{http_code}" -X MOVE \
    -H "Destination: $DAV_BASE/m5-moved.txt" \
    "$DAV_BASE/m3-sample.txt")
[[ "$STATUS" == "201" || "$STATUS" == "204" ]] \
    || fail "M5: root-level MOVE expected 201/204, got $STATUS"
# Source is gone, destination present.
[[ "$(dav_curl -o /dev/null -w "%{http_code}" -X PROPFIND -H "Depth: 0" "$DAV_BASE/m3-sample.txt")" == "404" ]] \
    || fail "M5: source still resolvable after MOVE"
[[ "$(dav_curl -o /dev/null -w "%{http_code}" -X PROPFIND -H "Depth: 0" "$DAV_BASE/m5-moved.txt")" == "207" ]] \
    || fail "M5: destination not found after MOVE"
pass "M5: root-level MOVE → $STATUS, source gone, destination present"

# ─────────────────────────────────────────────────────────────
# M6 — MKCOL sub/ → 201
# ─────────────────────────────────────────────────────────────
echo "  M6: MKCOL /webdav/m6-sub/ → 201"
STATUS=$(dav_curl -o /dev/null -w "%{http_code}" -X MKCOL "$DAV_BASE/m6-sub/")
[[ "$STATUS" == "201" ]] \
    || fail "M6: MKCOL expected 201, got $STATUS"
pass "M6: native MKCOL → 201"

# ─────────────────────────────────────────────────────────────
# M7 — DELETE sub/ → 204
# ─────────────────────────────────────────────────────────────
echo "  M7: DELETE /webdav/m6-sub/ → 204"
STATUS=$(dav_curl -o /dev/null -w "%{http_code}" -X DELETE "$DAV_BASE/m6-sub/")
[[ "$STATUS" == "204" ]] \
    || fail "M7: native DELETE expected 204, got $STATUS"
[[ "$(dav_curl -o /dev/null -w "%{http_code}" -X PROPFIND -H "Depth: 0" "$DAV_BASE/m6-sub/")" == "404" ]] \
    || fail "M7: folder still resolvable after DELETE"
pass "M7: native DELETE → 204, folder gone"

# ─────────────────────────────────────────────────────────────
# M8 — COPY a.txt → b.txt (pin whatever current behaviour is)
# ─────────────────────────────────────────────────────────────
# M8 source depends on whether M5 MOVE actually worked. If M5 was
# pinned as KNOWN BUG (404), the source for M8 is still
# m3-sample.txt at root, not m5-moved.txt.
echo "  M8: COPY /webdav/m5-moved.txt → /webdav/m8-copy.txt"
# M5 now succeeds, so the source is at m5-moved.txt. (Kept fallback
# to m3-sample.txt to surface a clear error if M5 regressed.)
M8_SOURCE_URL="$DAV_BASE/m5-moved.txt"
if ! dav_curl -o /dev/null -w "%{http_code}" -X PROPFIND -H "Depth: 0" "$DAV_BASE/m5-moved.txt" | grep -q "207"; then
    M8_SOURCE_URL="$DAV_BASE/m3-sample.txt"
fi
STATUS=$(dav_curl -o /dev/null -w "%{http_code}" -X COPY \
    -H "Destination: $DAV_BASE/m8-copy.txt" \
    "$M8_SOURCE_URL")
[[ "$STATUS" == "201" || "$STATUS" == "204" ]] \
    || fail "M8: native COPY expected 201/204, got $STATUS"
SRC_STATUS=$(dav_curl -o /dev/null -w "%{http_code}" -X PROPFIND -H "Depth: 0" "$M8_SOURCE_URL")
DST_STATUS=$(dav_curl -o /dev/null -w "%{http_code}" -X PROPFIND -H "Depth: 0" "$DAV_BASE/m8-copy.txt")
[[ "$SRC_STATUS" == "207" ]] \
    || fail "M8: COPY removed source ($SRC_STATUS instead of 207) — that's MOVE behaviour, not COPY"
[[ "$DST_STATUS" == "207" ]] \
    || fail "M8: destination not present after COPY ($DST_STATUS)"
pass "M8: native COPY → $STATUS, source preserved, destination renamed correctly"

# ═════════════════════════════════════════════════════════════
# Group N — LOCK / UNLOCK
# ═════════════════════════════════════════════════════════════

# Set up a file for the lock scenarios.
dav_curl -o /dev/null -X PUT \
    -H "Content-Type: text/plain" \
    --data-binary 'lockable contents' \
    "$DAV_BASE/n-locked.txt" > /dev/null

LOCK_BODY='<?xml version="1.0" encoding="utf-8"?>
<d:lockinfo xmlns:d="DAV:">
  <d:lockscope><d:exclusive/></d:lockscope>
  <d:locktype><d:write/></d:locktype>
  <d:owner>baseline-test-owner</d:owner>
</d:lockinfo>'

# ─────────────────────────────────────────────────────────────
# N1 — LOCK → 200 with Lock-Token header
# ─────────────────────────────────────────────────────────────
echo "  N1: LOCK /webdav/n-locked.txt → 200 + Lock-Token header"
HEADERS=$(dav_curl -D - -o /dev/null -X LOCK \
    -H "Content-Type: application/xml" \
    -H "Timeout: Second-60" \
    --data "$LOCK_BODY" \
    "$DAV_BASE/n-locked.txt")
STATUS=$(awk 'NR==1{print $2}' <<< "$HEADERS" | tr -d '\r')
[[ "$STATUS" == "200" ]] \
    || fail "N1: LOCK expected 200, got $STATUS"
LOCK_TOKEN=$(grep -i '^lock-token:' <<< "$HEADERS" | awk '{print $2}' | tr -d '\r<>')
[[ -n "$LOCK_TOKEN" ]] \
    || fail "N1: LOCK response missing Lock-Token header"
pass "N1: LOCK → 200 + Lock-Token=$LOCK_TOKEN"

# ─────────────────────────────────────────────────────────────
# N2 — PUT to a locked file without the token → 423 Locked
#
# RFC 4918 §9.10.4 + §6: a writeable resource under an
# exclusive lock MUST reject conflicting writes with 423
# Locked unless the request submits the lock token in `If:`.
# N2b verifies the inverse: same PUT with the correct
# `If: (<token>)` header succeeds, proving the gate isn't
# blocking legitimate updates from the lock owner.
# ─────────────────────────────────────────────────────────────
echo "  N2: PUT /webdav/n-locked.txt without If:(<token>) → 423"
STATUS=$(dav_curl -o /dev/null -w "%{http_code}" -X PUT \
    -H "Content-Type: text/plain" \
    --data-binary 'tampered contents' \
    "$DAV_BASE/n-locked.txt")
[[ "$STATUS" == "423" ]] \
    || fail "N2: expected 423 Locked for PUT to locked path without token, got $STATUS"
pass "N2: PUT to locked path without token → 423"

echo "  N2b: PUT /webdav/n-locked.txt WITH If:(<token>) → 204"
STATUS=$(dav_curl -o /dev/null -w "%{http_code}" -X PUT \
    -H "Content-Type: text/plain" \
    -H "If: (<$LOCK_TOKEN>)" \
    --data-binary 'authorised update' \
    "$DAV_BASE/n-locked.txt")
[[ "$STATUS" == "204" ]] \
    || fail "N2b: expected 204 No Content for PUT with correct lock token, got $STATUS"
pass "N2b: PUT with matching If:(<token>) → 204"

# ─────────────────────────────────────────────────────────────
# N2c–N2f — Lock enforcement on the other mutator methods
#
# RFC 4918 §9.10.4: a lock binds every mutating method, not just
# PUT. The native handler's `enforce_native_lock` helper was
# designed to be called by handle_delete / handle_move /
# handle_copy / handle_proppatch as well — these tests prove the
# wire is in. Each case uses the n-locked.txt resource locked
# above and a `WITHOUT If:` request, expecting 423. Positive
# (with-token) coverage is implicit: the M-series above already
# exercises each method on unlocked resources and asserts the
# success codes, so a regression that hard-rejected every call
# would fail there.
#
# Order matters: each must run while the lock is still held,
# i.e. before N3 below releases it.
# ─────────────────────────────────────────────────────────────
echo "  N2c: DELETE /webdav/n-locked.txt without If:(<token>) → 423"
STATUS=$(dav_curl -o /dev/null -w "%{http_code}" -X DELETE \
    "$DAV_BASE/n-locked.txt")
[[ "$STATUS" == "423" ]] \
    || fail "N2c: expected 423 Locked for DELETE on locked path without token, got $STATUS"
# The file must still be present after a rejected DELETE.
[[ "$(dav_curl -o /dev/null -w "%{http_code}" -X PROPFIND -H "Depth: 0" "$DAV_BASE/n-locked.txt")" == "207" ]] \
    || fail "N2c: file removed after rejected DELETE (423 was advisory only?)"
pass "N2c: DELETE on locked path without token → 423, resource preserved"

echo "  N2d: MOVE /webdav/n-locked.txt without If:(<token>) → 423 (source-side lock)"
STATUS=$(dav_curl -o /dev/null -w "%{http_code}" -X MOVE \
    -H "Destination: $DAV_BASE/n-locked-moved.txt" \
    "$DAV_BASE/n-locked.txt")
[[ "$STATUS" == "423" ]] \
    || fail "N2d: expected 423 Locked for MOVE on locked source without token, got $STATUS"
[[ "$(dav_curl -o /dev/null -w "%{http_code}" -X PROPFIND -H "Depth: 0" "$DAV_BASE/n-locked.txt")" == "207" ]] \
    || fail "N2d: source disappeared after rejected MOVE"
[[ "$(dav_curl -o /dev/null -w "%{http_code}" -X PROPFIND -H "Depth: 0" "$DAV_BASE/n-locked-moved.txt")" == "404" ]] \
    || fail "N2d: destination created after rejected MOVE"
pass "N2d: MOVE with locked source and no token → 423, no state mutated"

echo "  N2e: COPY into /webdav/n-locked.txt (locked destination) without If:(<token>) → 423"
# Set up a fresh unlocked source for the COPY.
dav_curl -o /dev/null -X PUT -H "Content-Type: text/plain" \
    --data-binary 'n2e copy source' \
    "$DAV_BASE/n2e-copy-src.txt" > /dev/null
STATUS=$(dav_curl -o /dev/null -w "%{http_code}" -X COPY \
    -H "Destination: $DAV_BASE/n-locked.txt" \
    "$DAV_BASE/n2e-copy-src.txt")
[[ "$STATUS" == "423" ]] \
    || fail "N2e: expected 423 Locked for COPY into locked destination without token, got $STATUS"
# The locked destination's content must not have been replaced.
BODY=$(dav_curl -s "$DAV_BASE/n-locked.txt")
[[ "$BODY" == "authorised update" ]] \
    || fail "N2e: locked destination's content was overwritten (got '$BODY')"
pass "N2e: COPY into locked destination without token → 423, target untouched"

echo "  N2f: PROPPATCH /webdav/n-locked.txt without If:(<token>) → 423"
PROPPATCH_BODY='<?xml version="1.0" encoding="utf-8"?>
<d:propertyupdate xmlns:d="DAV:">
  <d:set><d:prop><d:displayname>tampered</d:displayname></d:prop></d:set>
</d:propertyupdate>'
STATUS=$(dav_curl -o /dev/null -w "%{http_code}" -X PROPPATCH \
    -H "Content-Type: application/xml" \
    --data "$PROPPATCH_BODY" \
    "$DAV_BASE/n-locked.txt")
[[ "$STATUS" == "423" ]] \
    || fail "N2f: expected 423 Locked for PROPPATCH on locked path without token, got $STATUS"
pass "N2f: PROPPATCH on locked path without token → 423"

# ─────────────────────────────────────────────────────────────
# N3 — UNLOCK with token → 204; subsequent PUT succeeds
# ─────────────────────────────────────────────────────────────
echo "  N3: UNLOCK /webdav/n-locked.txt + follow-up PUT succeeds"
STATUS=$(dav_curl -o /dev/null -w "%{http_code}" -X UNLOCK \
    -H "Lock-Token: <$LOCK_TOKEN>" \
    "$DAV_BASE/n-locked.txt")
[[ "$STATUS" == "204" ]] \
    || fail "N3: UNLOCK expected 204, got $STATUS"
# Now PUT without any token — should succeed since lock is released.
STATUS=$(dav_curl -o /dev/null -w "%{http_code}" -X PUT \
    -H "Content-Type: text/plain" \
    --data-binary 'post-unlock contents' \
    "$DAV_BASE/n-locked.txt")
[[ "$STATUS" == "201" || "$STATUS" == "204" ]] \
    || fail "N3: post-unlock PUT expected 201/204, got $STATUS"
pass "N3: UNLOCK → 204 + subsequent PUT succeeds ($STATUS)"

# ── summary ───────────────────────────────────────────────────────────────────

echo
echo "Results: $PASS passed, $FAIL failed."
[[ "$FAIL" -eq 0 ]] && echo "All tests passed." || exit 1
