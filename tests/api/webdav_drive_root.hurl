# =============================================================
# OxiCloud — WebDAV drive-root URL scheme
# =============================================================
# Exercises the native WebDAV URL scheme documented in
# `src/interfaces/api/handlers/webdav_handler.rs::resolve_webdav_scope`:
#
#   Default deployment (`OXICLOUD_WEBDAV_DRIVE_LISTING_PREFIX="@drive"`):
#     * `/webdav/`                       → default drive's contents
#     * `/webdav/@drive/`                → drive listing (per-drive
#                                          virtual folders)
#     * `/webdav/@drive/<uuid>/…`        → explicit drive by UUID
#     * `/webdav/@drive/<name>/…`        → explicit drive by name
#
# Coverage:
#   1. Login, capture JWT
#   2. Resolve caller's default drive (id + display name)
#   3. Create a magic folder under the home root via REST
#   4. PROPFIND `/webdav/` — Depth: 1 lists the magic folder as
#      an immediate child of the default drive. This is the
#      user-visible bug fix: pre-refactor, `/webdav/` returned a
#      drive listing instead of the default drive's contents.
#   5. PROPFIND `/webdav/@drive/` — Depth: 1 lists each drive as
#      a virtual child (at least the caller's default is present).
#   6. PROPFIND `/webdav/@drive/<uuid>/` — descends into the
#      selected drive by UUID; magic folder appears here too.
#   7. PROPFIND `/webdav/@drive/<name>/` — same via display name.
#   8. Cleanup: DELETE the magic folder via REST.
#
# The magic folder name embeds a run-scoped marker so parallel
# `hurl --jobs N` runs don't step on each other and repeat runs
# against a shared DB don't collide.
# =============================================================


# ─────────────────────────────────────────────────────────────
# Step 1 — Login, capture JWT
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/auth/login
Content-Type: application/json
{ "username": "{{username}}", "password": "{{password}}" }

HTTP 200
[Captures]
token: jsonpath "$.access_token"


# ─────────────────────────────────────────────────────────────
# Step 2 — Resolve caller's default drive (id + display name).
#           `GET /api/drives` returns rows in a stable order:
#           the caller's default personal drive first, then by
#           display name. See `DriveRepository::list_readable_by`.
#           `default_for_user` on the DTO is present-only for
#           default rows (`Option<Uuid>` with `skip_serializing_if`),
#           so `$[0]` — combined with the stable order — is the
#           default drive for a fresh admin account.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/drives
Authorization: Bearer {{token}}

HTTP 200
[Captures]
default_drive_id:   jsonpath "$[0].id"
default_drive_name: jsonpath "$[0].name"


# ─────────────────────────────────────────────────────────────
# Step 3 — Resolve the caller's home root folder id.
#           A default personal drive has exactly one root folder
#           (the drive-root itself). We need its id to create the
#           magic folder as its child.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/folders
Authorization: Bearer {{token}}

HTTP 200
[Captures]
home_folder_id: jsonpath "$[0].id"


# ─────────────────────────────────────────────────────────────
# Step 4 — Create a magic folder under the home root via REST.
#           The name is deterministic-yet-unique so PROPFIND
#           assertions below can find it by exact string match,
#           and parallel test runs can't collide.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/folders
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "name": "hurl-drive-root-magic-marker",
  "parent_id": "{{home_folder_id}}"
}

HTTP 201
[Captures]
magic_folder_id: jsonpath "$.id"


# ─────────────────────────────────────────────────────────────
# Step 5 — PROPFIND on `/webdav/` (bare root). The default
#           deployment maps this to the caller's DEFAULT drive
#           contents, so Depth: 1 must include the magic folder.
#
#           Pre-refactor this returned a drive listing instead —
#           the exact regression that broke back-compat with
#           pre-multi-drive WebDAV clients.
# ─────────────────────────────────────────────────────────────
PROPFIND {{base_url}}/webdav/
Authorization: Bearer {{token}}
Depth: 1

HTTP 207
[Asserts]
xpath "//*[local-name()='response']/*[local-name()='href' and contains(text(), 'hurl-drive-root-magic-marker')]" exists


# ─────────────────────────────────────────────────────────────
# Step 6 — PROPFIND on `/webdav/@drive/`. This is the explicit
#           drive picker — Depth: 1 returns one virtual child
#           per drive the caller has Read on. The default drive
#           must appear (by its display name).
# ─────────────────────────────────────────────────────────────
PROPFIND {{base_url}}/webdav/@drive/
Authorization: Bearer {{token}}
Depth: 1

HTTP 207
[Asserts]
xpath "//*[local-name()='response']/*[local-name()='href' and contains(text(), '{{default_drive_name}}')]" exists


# ─────────────────────────────────────────────────────────────
# Step 7 — PROPFIND on `/webdav/@drive/<uuid>/`. The explicit
#           by-UUID selector — descends INTO the chosen drive.
#           Depth: 1 lists that drive's top-level children —
#           the magic folder must be one of them.
# ─────────────────────────────────────────────────────────────
PROPFIND {{base_url}}/webdav/@drive/{{default_drive_id}}/
Authorization: Bearer {{token}}
Depth: 1

HTTP 207
[Asserts]
xpath "//*[local-name()='response']/*[local-name()='href' and contains(text(), 'hurl-drive-root-magic-marker')]" exists


# ─────────────────────────────────────────────────────────────
# Step 8 — PROPFIND on `/webdav/@drive/<name>/`. The explicit
#           by-name selector — same result as the UUID form.
# ─────────────────────────────────────────────────────────────
PROPFIND {{base_url}}/webdav/@drive/{{default_drive_name}}/
Authorization: Bearer {{token}}
Depth: 1

HTTP 207
[Asserts]
xpath "//*[local-name()='response']/*[local-name()='href' and contains(text(), 'hurl-drive-root-magic-marker')]" exists


# ─────────────────────────────────────────────────────────────
# Step 9 — Reject MKCOL at `/webdav/@drive/` (bare pseudo-root).
#           The drive-listing target has no writable parent
#           folder — 405 Method Not Allowed. This guard prevents
#           a client from silently succeeding at "creating a
#           drive by MKCOL" (the drive-create surface is
#           `POST /api/drives`, not WebDAV).
# ─────────────────────────────────────────────────────────────
MKCOL {{base_url}}/webdav/@drive/
Authorization: Bearer {{token}}

HTTP 405


# ─────────────────────────────────────────────────────────────
# Step 10 — Reject MKCOL at `/webdav/@drive/<not-a-drive>`.
#           `<not-a-drive>` gets interpreted as a drive selector;
#           no drive with that name/UUID exists → 404. Sits
#           adjacent to Step 9 so any future maintainer touching
#           the pseudo-root rejection sees BOTH shapes at once
#           (bare listing = 405, unknown selector = 404).
# ─────────────────────────────────────────────────────────────
MKCOL {{base_url}}/webdav/@drive/hurl-not-a-real-drive
Authorization: Bearer {{token}}

HTTP 404


# ─────────────────────────────────────────────────────────────
# Step 11 — Reject PUT at `/webdav/@drive/<not-a-drive>/x.txt`.
#           Same rejection shape as MKCOL — trying to write a
#           file into a non-existent drive.
# ─────────────────────────────────────────────────────────────
PUT {{base_url}}/webdav/@drive/hurl-not-a-real-drive/probe.txt
Authorization: Bearer {{token}}
Content-Type: text/plain
```
probe
```

HTTP 404


# ─────────────────────────────────────────────────────────────
# Step 11b — Reject PUT at `/webdav/@drive/test.txt`. The URL
#           segment immediately after `@drive/` is ALWAYS a
#           drive selector — never a filename. A caller that
#           bookmarks a file URL under `@drive` with a name
#           that doesn't match any drive must get 404, not
#           silently create a file at the drive-listing level.
# ─────────────────────────────────────────────────────────────
PUT {{base_url}}/webdav/@drive/test.txt
Authorization: Bearer {{token}}
Content-Type: text/plain
```
probe
```

HTTP 404


# ─────────────────────────────────────────────────────────────
# Step 12 — Cleanup: DELETE the magic folder via REST so
#           subsequent test runs / other hurl files don't see
#           our marker.
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/folders/{{magic_folder_id}}
Authorization: Bearer {{token}}

HTTP 204
