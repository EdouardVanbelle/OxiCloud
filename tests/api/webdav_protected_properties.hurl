# =============================================================
# OxiCloud — WebDAV protected properties (RFC 4918 §9.2 / §15)
# =============================================================
# `DeadPropertyStore` lets a PROPPATCH set arbitrary namespace/name
# pairs verbatim (RFC 4918 §4.2). Without a denylist, a client could
# PROPPATCH `DAV:getetag`, `oc:fileid`, `oc:permissions`, etc. — names
# the server ALSO emits as live state in PROPFIND/REPORT responses
# (see `write_file_response` / `write_folder_response` in the NC
# handler and the native PROPFIND writer). That produces either a
# forged live property (the server would need to pick which of two
# values to emit) or a silently stored, never-read row.
#
# `is_protected_property()` (src/application/adapters/webdav_adapter.rs)
# defends the whole `DAV:` namespace plus the specific oc:/nc:/ocs:
# names the server actually emits elsewhere. Both PROPPATCH handlers
# (native `/webdav/` and NC `/remote.php/dav/`) consult it before
# touching `DeadPropertyStore`, and reject with RFC 4918 §9.2's
# per-property `403 Forbidden` inside the 207 multi-status — not a
# blanket request failure, and not a silent no-op success.
#
# Coverage:
#   1.  Native /webdav/: PROPPATCH set on `D:displayname` (DAV:
#       namespace) → 207 envelope, inner 403 for that property.
#   2.  PROPFIND confirms the live displayname is unchanged — the
#       forged value never landed anywhere.
#   3.  Native /webdav/: PROPPATCH remove on `D:getetag` → same 403
#       contract on the Remove path, not just Set.
#   4.  Native /webdav/: an oc:-namespaced protected name
#       (`oc:fileid`) is blocked even on the surface that doesn't
#       normally speak NextCloud namespaces — protection is
#       namespace-global, not surface-scoped.
#   5.  Mixed request: one protected DAV: prop + one ordinary custom
#       dead property in the SAME PROPPATCH → 207 with both a 403
#       propstat block and a 200 propstat block; the custom property
#       DOES get stored (per-property granularity, not all-or-nothing
#       rejection).
#   6.  NC surface: PROPPATCH set on a protected oc: name
#       (`oc:permissions`) → 403; PROPFIND confirms it was never
#       written to the dead-property store.
#   7.  NC surface: PROPPATCH set on a protected nc: name
#       (`nc:has-preview`) → 403.
#   8.  Regression guard: `oc:favorite` is on the protected list too
#       (it's live state the NC handler emits), but the handler's
#       favorite special-case runs BEFORE the protected-property
#       check, so toggling favorite through PROPPATCH still works —
#       protection must not swallow the one oc: name that's
#       legitimately client-writable via a side channel.
# =============================================================


# ─────────────────────────────────────────────────────────────
# Step 1 — Login, capture JWT; mint an NC app password for the
#          NC-surface half of this file (NC DAV uses Basic Auth).
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/auth/login
Content-Type: application/json
{ "username": "{{username}}", "password": "{{password}}" }

HTTP 200
[Captures]
token: jsonpath "$.access_token"


POST {{base_url}}/api/auth/app-passwords
Authorization: Bearer {{token}}
Content-Type: application/json
{ "label": "webdav_protected_properties" }

HTTP 200
[Captures]
nc_username: jsonpath "$.username"
nc_password: jsonpath "$.password"
ap_id:       jsonpath "$.id"


# ─────────────────────────────────────────────────────────────
# Step 2 — PUT a probe file via native WebDAV.
# ─────────────────────────────────────────────────────────────
PUT {{base_url}}/webdav/protected-props-probe.txt
Authorization: Bearer {{token}}
Content-Type: text/plain
```
hello protected properties
```

HTTP 201


# ─────────────────────────────────────────────────────────────
# Step 3 — PROPPATCH set on DAV:displayname (live property) must
#          be rejected with a per-property 403, not silently
#          accepted into DeadPropertyStore.
# ─────────────────────────────────────────────────────────────
PROPPATCH {{base_url}}/webdav/protected-props-probe.txt
Authorization: Bearer {{token}}
Content-Type: application/xml; charset=utf-8
```
<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:">
  <D:set>
    <D:prop>
      <D:displayname>forged-name</D:displayname>
    </D:prop>
  </D:set>
</D:propertyupdate>
```

HTTP 207
[Asserts]
xpath "string(//*[local-name()='propstat']/*[local-name()='status'])" contains "403"


# ─────────────────────────────────────────────────────────────
# Step 4 — PROPFIND confirms the live displayname is untouched.
# ─────────────────────────────────────────────────────────────
PROPFIND {{base_url}}/webdav/protected-props-probe.txt
Authorization: Bearer {{token}}
Depth: 0
Content-Type: application/xml; charset=utf-8
```
<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:">
  <D:allprop/>
</D:propfind>
```

HTTP 207
[Asserts]
xpath "string(//*[local-name()='displayname'])" == "protected-props-probe.txt"


# ─────────────────────────────────────────────────────────────
# Step 5 — PROPPATCH remove on DAV:getetag → same 403 contract
#          on the Remove path.
# ─────────────────────────────────────────────────────────────
PROPPATCH {{base_url}}/webdav/protected-props-probe.txt
Authorization: Bearer {{token}}
Content-Type: application/xml; charset=utf-8
```
<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:">
  <D:remove>
    <D:prop>
      <D:getetag/>
    </D:prop>
  </D:remove>
</D:propertyupdate>
```

HTTP 207
[Asserts]
xpath "string(//*[local-name()='propstat']/*[local-name()='status'])" contains "403"


# ─────────────────────────────────────────────────────────────
# Step 6 — Protection is namespace-global: an oc:-namespaced
#          protected name is blocked even on the native /webdav/
#          surface, which doesn't otherwise speak NextCloud
#          namespaces.
# ─────────────────────────────────────────────────────────────
PROPPATCH {{base_url}}/webdav/protected-props-probe.txt
Authorization: Bearer {{token}}
Content-Type: application/xml; charset=utf-8
```
<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:" xmlns:oc="http://owncloud.org/ns">
  <D:set>
    <D:prop>
      <oc:fileid>should-not-be-stored</oc:fileid>
    </D:prop>
  </D:set>
</D:propertyupdate>
```

HTTP 207
[Asserts]
xpath "string(//*[local-name()='propstat']/*[local-name()='status'])" contains "403"


# ─────────────────────────────────────────────────────────────
# Step 7 — Mixed request: one protected DAV: prop + one ordinary
#          custom dead property in the SAME PROPPATCH → per-
#          property granularity, not all-or-nothing rejection.
# ─────────────────────────────────────────────────────────────
PROPPATCH {{base_url}}/webdav/protected-props-probe.txt
Authorization: Bearer {{token}}
Content-Type: application/xml; charset=utf-8
```
<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:" xmlns:X="oxi:test">
  <D:set>
    <D:prop>
      <D:resourcetype>forged</D:resourcetype>
      <X:testlabel>allowed-alongside-protected</X:testlabel>
    </D:prop>
  </D:set>
</D:propertyupdate>
```

HTTP 207
[Asserts]
xpath "count(//*[local-name()='propstat'])" == 2
xpath "string(//*[local-name()='propstat'][*[local-name()='prop']/*[local-name()='resourcetype']]/*[local-name()='status'])" contains "403"
xpath "string(//*[local-name()='propstat'][*[local-name()='prop']/*[local-name()='testlabel']]/*[local-name()='status'])" contains "200 OK"


PROPFIND {{base_url}}/webdav/protected-props-probe.txt
Authorization: Bearer {{token}}
Depth: 0
Content-Type: application/xml; charset=utf-8
```
<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:">
  <D:allprop/>
</D:propfind>
```

HTTP 207
[Asserts]
xpath "string(//*[local-name()='testlabel'])" == "allowed-alongside-protected"


# ─────────────────────────────────────────────────────────────
# Cleanup — native probe file.
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/webdav/protected-props-probe.txt
Authorization: Bearer {{token}}

HTTP 204


# ─────────────────────────────────────────────────────────────
# Step 8 — NC surface: PUT a probe file via the NC DAV mount.
# ─────────────────────────────────────────────────────────────
PUT {{base_url}}/remote.php/dav/files/{{username}}/nc-protected-props-probe.txt
Content-Type: text/plain
[BasicAuth]
{{nc_username}}: {{nc_password}}
```
hello nc protected properties
```

HTTP 201


# ─────────────────────────────────────────────────────────────
# Step 9 — NC surface: PROPPATCH set on a protected oc: name
#          (`oc:permissions`, not the specially-handled favorite)
#          → 403.
# ─────────────────────────────────────────────────────────────
PROPPATCH {{base_url}}/remote.php/dav/files/{{username}}/nc-protected-props-probe.txt
Content-Type: application/xml; charset=utf-8
[BasicAuth]
{{nc_username}}: {{nc_password}}
```
<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:" xmlns:oc="http://owncloud.org/ns">
  <D:set>
    <D:prop>
      <oc:permissions>forged</oc:permissions>
    </D:prop>
  </D:set>
</D:propertyupdate>
```

HTTP 207
[Asserts]
xpath "string(//*[local-name()='propstat']/*[local-name()='status'])" contains "403"


PROPFIND {{base_url}}/remote.php/dav/files/{{username}}/nc-protected-props-probe.txt
Depth: 0
Content-Type: application/xml; charset=utf-8
[BasicAuth]
{{nc_username}}: {{nc_password}}
```
<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:">
  <D:allprop/>
</D:propfind>
```

HTTP 207
[Asserts]
xpath "string(//*[local-name()='permissions'])" != "forged"


# ─────────────────────────────────────────────────────────────
# Step 10 — NC surface: PROPPATCH set on a protected nc: name
#           (`nc:has-preview`) → 403.
# ─────────────────────────────────────────────────────────────
PROPPATCH {{base_url}}/remote.php/dav/files/{{username}}/nc-protected-props-probe.txt
Content-Type: application/xml; charset=utf-8
[BasicAuth]
{{nc_username}}: {{nc_password}}
```
<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:" xmlns:nc="http://nextcloud.org/ns">
  <D:set>
    <D:prop>
      <nc:has-preview>forged</nc:has-preview>
    </D:prop>
  </D:set>
</D:propertyupdate>
```

HTTP 207
[Asserts]
xpath "string(//*[local-name()='propstat']/*[local-name()='status'])" contains "403"


# ─────────────────────────────────────────────────────────────
# Step 11 — Regression guard: oc:favorite is on the protected
#           list too, but the handler's favorite special-case
#           runs before the protection check, so toggling it via
#           PROPPATCH must still work end to end.
# ─────────────────────────────────────────────────────────────
PROPPATCH {{base_url}}/remote.php/dav/files/{{username}}/nc-protected-props-probe.txt
Content-Type: application/xml; charset=utf-8
[BasicAuth]
{{nc_username}}: {{nc_password}}
```
<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:" xmlns:oc="http://owncloud.org/ns">
  <D:set>
    <D:prop>
      <oc:favorite>1</oc:favorite>
    </D:prop>
  </D:set>
</D:propertyupdate>
```

HTTP 207
[Asserts]
xpath "string(//*[local-name()='propstat']/*[local-name()='status'])" contains "200 OK"


PROPFIND {{base_url}}/remote.php/dav/files/{{username}}/nc-protected-props-probe.txt
Depth: 0
Content-Type: application/xml; charset=utf-8
[BasicAuth]
{{nc_username}}: {{nc_password}}
```
<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:">
  <D:allprop/>
</D:propfind>
```

HTTP 207
[Asserts]
xpath "string(//*[local-name()='favorite'])" == "1"


# ─────────────────────────────────────────────────────────────
# Cleanup — NC probe file, teardown app password.
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/remote.php/dav/files/{{username}}/nc-protected-props-probe.txt
[BasicAuth]
{{nc_username}}: {{nc_password}}

HTTP 204


DELETE {{base_url}}/api/auth/app-passwords/{{ap_id}}
Authorization: Bearer {{token}}

HTTP 200
