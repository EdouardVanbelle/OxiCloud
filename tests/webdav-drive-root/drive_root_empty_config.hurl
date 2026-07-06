# =============================================================
# OxiCloud — WebDAV drive-root URL scheme, `OXICLOUD_WEBDAV_DRIVE_LISTING_PREFIX=""` variant
# =============================================================
# Companion to `webdav_drive_root.hurl`. That file exercises the
# default config (`OXICLOUD_WEBDAV_DRIVE_LISTING_PREFIX="@drive"`); this
# one exercises the empty-string config where `/webdav/` IS the
# drive listing and there's no default-drive shortcut.
#
# Server env for this test: `tests/common/server-webdav-drive-root.env`
# sets `OXICLOUD_WEBDAV_DRIVE_LISTING_PREFIX=""`. This file assumes that
# config is active — it is NOT part of the standard `run.sh`
# invocation (which starts the default-config server).
#
# Coverage:
#   1. Login, capture JWT
#   2. Resolve caller's default drive (id + display name)
#   3. Create a magic folder under the home root via REST
#   4. PROPFIND `/webdav/` — drive listing (default drive
#      appears as a virtual child under its display name).
#   5. PROPFIND `/webdav/<uuid>/` — descend into a drive by
#      UUID. Magic folder appears.
#   6. PROPFIND `/webdav/<name>/` — descend into a drive by
#      display name. Magic folder appears.
#   7. `/webdav/@drive/` returns 404 in this mode — the sigil
#      has no reserved meaning when `webdav_drive_listing_prefix=""`.
#      A drive genuinely named `@drive` would resolve here; the
#      404 comes from "no such drive," not the sigil.
#   8. Cleanup: DELETE the magic folder via REST.
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
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/drives
Authorization: Bearer {{token}}

HTTP 200
[Captures]
default_drive_id:   jsonpath "$[0].id"
default_drive_name: jsonpath "$[0].name"


# ─────────────────────────────────────────────────────────────
# Step 3 — Resolve the caller's home root folder id.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/folders
Authorization: Bearer {{token}}

HTTP 200
[Captures]
home_folder_id: jsonpath "$[0].id"


# ─────────────────────────────────────────────────────────────
# Step 4 — Create a magic folder under the home root via REST.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/folders
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "name": "hurl-drive-root-empty-magic-marker",
  "parent_id": "{{home_folder_id}}"
}

HTTP 201
[Captures]
magic_folder_id: jsonpath "$.id"


# ─────────────────────────────────────────────────────────────
# Step 5 — PROPFIND on `/webdav/` (bare root). With
#           `OXICLOUD_WEBDAV_DRIVE_LISTING_PREFIX=""` this IS the drive
#           listing — the default drive appears as a virtual
#           child under its display name. The magic folder does
#           NOT appear here (it lives one level deeper).
# ─────────────────────────────────────────────────────────────
PROPFIND {{base_url}}/webdav/
Authorization: Bearer {{token}}
Depth: 1

HTTP 207
[Asserts]
xpath "//*[local-name()='response']/*[local-name()='href' and contains(text(), '{{default_drive_name}}')]" exists
# Magic folder is one level deeper — must NOT show up at root.
xpath "//*[local-name()='response']/*[local-name()='href' and contains(text(), 'hurl-drive-root-empty-magic-marker')]" not exists


# ─────────────────────────────────────────────────────────────
# Step 6 — PROPFIND on `/webdav/<uuid>/`. Descends into the
#           default drive; magic folder is a top-level child.
# ─────────────────────────────────────────────────────────────
PROPFIND {{base_url}}/webdav/{{default_drive_id}}/
Authorization: Bearer {{token}}
Depth: 1

HTTP 207
[Asserts]
xpath "//*[local-name()='response']/*[local-name()='href' and contains(text(), 'hurl-drive-root-empty-magic-marker')]" exists


# ─────────────────────────────────────────────────────────────
# Step 7 — PROPFIND on `/webdav/<name>/`. Same descent via
#           display name.
# ─────────────────────────────────────────────────────────────
PROPFIND {{base_url}}/webdav/{{default_drive_name}}/
Authorization: Bearer {{token}}
Depth: 1

HTTP 207
[Asserts]
xpath "//*[local-name()='response']/*[local-name()='href' and contains(text(), 'hurl-drive-root-empty-magic-marker')]" exists


# ─────────────────────────────────────────────────────────────
# Step 8 — `/webdav/@drive/` has no reserved meaning in the
#           empty-config mode. `@drive` is treated as a plain
#           drive selector; no drive by that name → 404.
# ─────────────────────────────────────────────────────────────
PROPFIND {{base_url}}/webdav/@drive/
Authorization: Bearer {{token}}
Depth: 1

HTTP 404


# ─────────────────────────────────────────────────────────────
# Step 9 — Reject MKCOL at `/webdav/` (bare pseudo-root).
#           In the empty-config mode `/webdav/` IS the drive
#           listing — there's no writable parent, so 405
#           Method Not Allowed. This guard prevents a client
#           from creating something at "root" that shadows a
#           drive name.
# ─────────────────────────────────────────────────────────────
MKCOL {{base_url}}/webdav/
Authorization: Bearer {{token}}

HTTP 405


# ─────────────────────────────────────────────────────────────
# Step 10 — Reject MKCOL at `/webdav/<not-a-drive>`. The first
#           URL segment is the drive selector in this config;
#           an unknown selector yields 404. A client cannot
#           "create a drive" via MKCOL — the drive-create
#           surface is `POST /api/drives`.
# ─────────────────────────────────────────────────────────────
MKCOL {{base_url}}/webdav/hurl-not-a-real-drive
Authorization: Bearer {{token}}

HTTP 404


# ─────────────────────────────────────────────────────────────
# Step 11 — Reject PUT at `/webdav/<not-a-drive>/x.txt`. Same
#           rejection shape as MKCOL.
# ─────────────────────────────────────────────────────────────
PUT {{base_url}}/webdav/hurl-not-a-real-drive/probe.txt
Authorization: Bearer {{token}}
Content-Type: text/plain
```
probe
```

HTTP 404


# ─────────────────────────────────────────────────────────────
# Step 12 — Cleanup: DELETE the magic folder via REST.
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/folders/{{magic_folder_id}}
Authorization: Bearer {{token}}

HTTP 204
