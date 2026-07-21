# =============================================================
# OxiCloud — WebDAV: nested-folder MOVE descendant cascade
# =============================================================
# Regression guard for the litmus `copymove → move_coll`
# scenario: when a parent folder is renamed (or moved), every
# DESCENDANT folder row must have its `path` / `lpath` columns
# rewritten by the AFTER cascade trigger
# `trg_folders_cascade_path` so that path-keyed lookups
# (WebDAV, CalDAV, CardDAV, and a handful of REST endpoints)
# resolve the descendant at its new location.
#
# Why this needs a dedicated test:
#   * REST API tests overwhelmingly use folder IDs, not paths —
#     a stale `folders.path` column is invisible to `WHERE id =
#     $1` lookups. So they can't catch this regression even when
#     they MOVE.
#   * Existing WebDAV tests are flat: MOVE a file, or MKCOL +
#     DELETE on a single-level folder. None combine "MOVE a
#     folder that has folder descendants" with "look the
#     descendant up by its post-move path".
#   * litmus's `copymove → move_coll` IS this test, but litmus
#     isn't installed on every contributor's machine — it lives
#     on the CI side only. This Hurl scenario runs in every
#     standard `just api-test`.
#
# Bug shape it catches: the `UPDATE OF <cols>` column list on
# `trg_folders_cascade_path` must include `name` AND `parent_id`
# (not just `path, lpath, drive_id`) — otherwise the AFTER
# trigger never fires on the rename SQL `UPDATE folders SET
# name = $1` or the move SQL `UPDATE folders SET parent_id =
# $1`, descendant rows stay at their pre-move path, and any
# subsequent path-keyed lookup of a descendant returns 404.
#
# What this test does:
#   1. Login (admin).
#   2. MKCOL  /webdav/regress-cascade-a/
#   3. MKCOL  /webdav/regress-cascade-a/b/        (descendant folder)
#   4. PUT    /webdav/regress-cascade-a/b/leaf.txt  (leaf file)
#   5. MOVE   /webdav/regress-cascade-a/ → /webdav/regress-cascade-c/
#   6. DELETE /webdav/regress-cascade-c/b/leaf.txt  ← path-based file
#                                                     lookup at the
#                                                     new descendant
#                                                     location
#   7. DELETE /webdav/regress-cascade-c/b/          ← path-based
#                                                     descendant
#                                                     folder lookup
#                                                     (this is the
#                                                     one that 404s
#                                                     when the bug
#                                                     is present)
#   8. DELETE /webdav/regress-cascade-c/            (cleanup root)
#
# Steps 6 and 7 are the load-bearing assertions; without the
# cascade, the descendant's `path` column is still
# `Personal/regress-cascade-a/b` and both DELETEs return 404.
# =============================================================


# ─────────────────────────────────────────────────────────────
# Step 1 — Login, capture JWT.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/auth/login
Content-Type: application/json
{ "username": "{{username}}", "password": "{{password}}" }

HTTP 200
[Captures]
token: jsonpath "$.access_token"


# ─────────────────────────────────────────────────────────────
# Step 2 — MKCOL the parent collection. Fresh names; 201 expected.
# ─────────────────────────────────────────────────────────────
MKCOL {{base_url}}/webdav/regress-cascade-a/
Authorization: Bearer {{token}}

HTTP 201


# ─────────────────────────────────────────────────────────────
# Step 3 — MKCOL the descendant collection inside the parent.
#          This is the folder row whose `path` column the
#          cascade trigger must rewrite when the parent is
#          renamed in Step 5.
# ─────────────────────────────────────────────────────────────
MKCOL {{base_url}}/webdav/regress-cascade-a/b/
Authorization: Bearer {{token}}

HTTP 201


# ─────────────────────────────────────────────────────────────
# Step 4 — PUT a leaf file inside the descendant. We use it in
#          Step 6 to verify the post-move file lookup works
#          (files resolve via their parent folder's `path`, so
#          this branch caught fire too when the cascade was
#          broken — even though `storage.files` has no `path`
#          column of its own).
# ─────────────────────────────────────────────────────────────
PUT {{base_url}}/webdav/regress-cascade-a/b/leaf.txt
Authorization: Bearer {{token}}
Content-Type: text/plain
```
nested cascade regression probe
```

HTTP 201


# ─────────────────────────────────────────────────────────────
# Step 5 — MOVE the parent collection. The SQL the service
#          issues is `UPDATE storage.folders SET name = $1, ...`
#          on the parent row (intra-drive same-parent rename).
#          The BEFORE trigger `trg_folders_path` rewrites the
#          parent's own `path`; the AFTER trigger
#          `trg_folders_cascade_path` must fire to rewrite
#          every descendant folder's `path` / `lpath`.
#
# RFC 4918 §9.9.4: destination is fresh → 201 Created.
# ─────────────────────────────────────────────────────────────
MOVE {{base_url}}/webdav/regress-cascade-a/
Authorization: Bearer {{token}}
Destination: {{base_url}}/webdav/regress-cascade-c/

HTTP 201


# ─────────────────────────────────────────────────────────────
# Step 6 — Resolve the leaf FILE by its post-move path. The
#          DELETE handler's resolver joins `storage.files`
#          against `storage.folders` on `folder_id`, then
#          filters `fo.path = 'Personal/regress-cascade-c/b'`.
#          That match depends on the descendant folder's
#          `path` column having been cascade-rewritten in
#          Step 5.
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/webdav/regress-cascade-c/b/leaf.txt
Authorization: Bearer {{token}}

HTTP 204


# ─────────────────────────────────────────────────────────────
# Step 7 — Resolve the descendant FOLDER by its post-move path.
#          This is the assertion that was failing as litmus
#          test 10. Lookup SQL: `SELECT … FROM storage.folders
#          WHERE path = 'Personal/regress-cascade-c/b' …`.
#          Without the cascade, the row still has path
#          `Personal/regress-cascade-a/b` → 0 rows → 404.
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/webdav/regress-cascade-c/b/
Authorization: Bearer {{token}}

HTTP 204


# ─────────────────────────────────────────────────────────────
# Step 8 — Cleanup: DELETE the moved root so subsequent test
#          runs start clean even on a non-pristine DB.
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/webdav/regress-cascade-c/
Authorization: Bearer {{token}}

HTTP 204
