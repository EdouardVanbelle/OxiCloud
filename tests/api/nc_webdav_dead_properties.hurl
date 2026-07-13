# =============================================================
# OxiCloud — NextCloud WebDAV: dead-properties (RFC 4918 §4.2)
# =============================================================
# `tests/api/webdav_dead_properties.hurl` covers the native
# `/webdav/` surface end-to-end. This file covers the same
# PROPPATCH/PROPFIND contract on the NextCloud-compatible surface
# (`/remote.php/dav/files/{user}/...`), which — until now — had NO
# generic dead-property support: PROPPATCH only special-cased
# `oc:favorite` via an ad hoc XML scan and silently discarded any
# other property while still claiming `200 OK`; PROPFIND always
# emitted a fixed hardcoded property set with no dead-property
# lookup at all. A client (or litmus) PROPPATCHing a custom label
# through the NextCloud mount got a false success and then never
# saw the property again.
#
# Coverage:
#   1.  Setup: JWT login, mint an NC app password.
#   2.  PUT a probe file via the NC DAV surface.
#   3.  PROPPATCH set a custom property → 207.
#   4.  PROPFIND → value round-trips verbatim.
#   5.  PROPPATCH upsert (same name, new value) → PROPFIND confirms
#       overwrite, not a duplicate row.
#   6.  PROPPATCH remove → PROPFIND confirms absence.
#   7.  PROPPATCH on a nonexistent resource → 404 (the tightened
#       contract: PROPPATCH now does real work, so a previous
#       "always claim success" no-op on a missing resource would be
#       a foot-gun, not a feature).
#   8.  Re-set a property, MOVE the file → PROPFIND on the new path
#       still returns it (resource id is stable across MOVE).
#   9.  DELETE, then PUT a fresh file at the same path → PROPFIND
#       does NOT see the old marker (new resource, no leaked state).
#   10. Regression guard: `oc:favorite` PROPPATCH/PROPFIND still
#       works, unaffected by the refactor from the ad hoc favorite
#       scanner to generic `WebDavAdapter::parse_proppatch`.
#   11. Folder coverage: MKCOL, PROPPATCH a dead property on the
#       folder, PROPFIND confirms it, cleanup.
#
# XPath assertions use `local-name()` so the test is robust against
# the server's chosen namespace prefix for dead properties (`X:`).
#
# NOTE: in Hurl, [BasicAuth] must be the LAST section before the
# blank-line/body — any request headers go above it, not below.
# =============================================================


# ─────────────────────────────────────────────────────────────
# Step 1 — JWT login, then mint an NC app password (NC DAV uses
#          Basic Auth, not the JWT bearer token).
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/auth/login
Content-Type: application/json
{ "username": "{{username}}", "password": "{{password}}" }

HTTP 200
[Captures]
jwt: jsonpath "$.access_token"


POST {{base_url}}/api/auth/app-passwords
Authorization: Bearer {{jwt}}
Content-Type: application/json
{ "label": "nc_webdav_dead_properties" }

HTTP 200
[Captures]
nc_username: jsonpath "$.username"
nc_password: jsonpath "$.password"
ap_id:       jsonpath "$.id"


# ─────────────────────────────────────────────────────────────
# Step 2 — PUT a probe file through the NC DAV surface.
# ─────────────────────────────────────────────────────────────
PUT {{base_url}}/remote.php/dav/files/{{username}}/nc-dead-props-probe.txt
Content-Type: text/plain
[BasicAuth]
{{nc_username}}: {{nc_password}}
```
hello nc dead properties
```

HTTP 201


# ─────────────────────────────────────────────────────────────
# Step 3 — PROPPATCH set a custom (dead) property.
# ─────────────────────────────────────────────────────────────
PROPPATCH {{base_url}}/remote.php/dav/files/{{username}}/nc-dead-props-probe.txt
Content-Type: application/xml; charset=utf-8
[BasicAuth]
{{nc_username}}: {{nc_password}}
```
<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:" xmlns:X="oxi:test">
  <D:set>
    <D:prop>
      <X:testlabel>hello-nc-dead-property</X:testlabel>
    </D:prop>
  </D:set>
</D:propertyupdate>
```

HTTP 207
[Asserts]
xpath "string(//*[local-name()='propstat']/*[local-name()='status'])" contains "200 OK"


# ─────────────────────────────────────────────────────────────
# Step 4 — PROPFIND confirms the round-trip.
# ─────────────────────────────────────────────────────────────
PROPFIND {{base_url}}/remote.php/dav/files/{{username}}/nc-dead-props-probe.txt
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
xpath "string(//*[local-name()='testlabel'])" == "hello-nc-dead-property"


# ─────────────────────────────────────────────────────────────
# Step 5 — Upsert: setting the same name again overwrites rather
#          than duplicating (ON CONFLICT DO UPDATE at the store).
# ─────────────────────────────────────────────────────────────
PROPPATCH {{base_url}}/remote.php/dav/files/{{username}}/nc-dead-props-probe.txt
Content-Type: application/xml; charset=utf-8
[BasicAuth]
{{nc_username}}: {{nc_password}}
```
<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:" xmlns:X="oxi:test">
  <D:set>
    <D:prop>
      <X:testlabel>updated-nc-value</X:testlabel>
    </D:prop>
  </D:set>
</D:propertyupdate>
```

HTTP 207


PROPFIND {{base_url}}/remote.php/dav/files/{{username}}/nc-dead-props-probe.txt
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
xpath "string(//*[local-name()='testlabel'])" == "updated-nc-value"
xpath "count(//*[local-name()='testlabel'])"  == 1


# ─────────────────────────────────────────────────────────────
# Step 6 — Remove the property; PROPFIND confirms absence.
# ─────────────────────────────────────────────────────────────
PROPPATCH {{base_url}}/remote.php/dav/files/{{username}}/nc-dead-props-probe.txt
Content-Type: application/xml; charset=utf-8
[BasicAuth]
{{nc_username}}: {{nc_password}}
```
<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:" xmlns:X="oxi:test">
  <D:remove>
    <D:prop>
      <X:testlabel/>
    </D:prop>
  </D:remove>
</D:propertyupdate>
```

HTTP 207


PROPFIND {{base_url}}/remote.php/dav/files/{{username}}/nc-dead-props-probe.txt
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
xpath "count(//*[local-name()='testlabel'])" == 0


# ─────────────────────────────────────────────────────────────
# Step 7 — PROPPATCH against a nonexistent resource → 404.
#          Prior behaviour on this handler silently no-opped
#          (and still claimed success) when the body carried no
#          `oc:favorite` directive; now that PROPPATCH performs
#          real dead-property writes, a missing resource must be
#          a hard failure, matching the native `/webdav/` handler.
# ─────────────────────────────────────────────────────────────
PROPPATCH {{base_url}}/remote.php/dav/files/{{username}}/nc-dead-props-does-not-exist.txt
Content-Type: application/xml; charset=utf-8
[BasicAuth]
{{nc_username}}: {{nc_password}}
```
<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:" xmlns:X="oxi:test">
  <D:set>
    <D:prop>
      <X:testlabel>should-not-be-stored</X:testlabel>
    </D:prop>
  </D:set>
</D:propertyupdate>
```

HTTP 404


# ─────────────────────────────────────────────────────────────
# Step 8 — Re-set a marker, MOVE the file, confirm the property
#          followed the resource (id-stable across MOVE — no
#          store-side rename bookkeeping needed).
# ─────────────────────────────────────────────────────────────
PROPPATCH {{base_url}}/remote.php/dav/files/{{username}}/nc-dead-props-probe.txt
Content-Type: application/xml; charset=utf-8
[BasicAuth]
{{nc_username}}: {{nc_password}}
```
<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:" xmlns:X="oxi:test">
  <D:set>
    <D:prop>
      <X:testlabel>survives-nc-move</X:testlabel>
    </D:prop>
  </D:set>
</D:propertyupdate>
```

HTTP 207


MOVE {{base_url}}/remote.php/dav/files/{{username}}/nc-dead-props-probe.txt
Destination: {{base_url}}/remote.php/dav/files/{{username}}/nc-dead-props-moved.txt
[BasicAuth]
{{nc_username}}: {{nc_password}}

# Fresh destination → 201 (RFC 4918 §9.9.4).
HTTP 201


PROPFIND {{base_url}}/remote.php/dav/files/{{username}}/nc-dead-props-moved.txt
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
xpath "string(//*[local-name()='testlabel'])" == "survives-nc-move"


# ─────────────────────────────────────────────────────────────
# Step 9 — DELETE, then PUT a fresh file at the same path: the
#          old marker must NOT resurface (new resource, no leaked
#          dead-property state). Whether DELETE soft-deletes to
#          trash or hard-deletes, the recreated path resolves to
#          a brand-new resource id with no dead-property rows of
#          its own.
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/remote.php/dav/files/{{username}}/nc-dead-props-moved.txt
[BasicAuth]
{{nc_username}}: {{nc_password}}

HTTP 204


PUT {{base_url}}/remote.php/dav/files/{{username}}/nc-dead-props-moved.txt
Content-Type: text/plain
[BasicAuth]
{{nc_username}}: {{nc_password}}
```
fresh file at the same nc path
```

HTTP 201


PROPFIND {{base_url}}/remote.php/dav/files/{{username}}/nc-dead-props-moved.txt
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
xpath "count(//*[local-name()='testlabel'])" == 0


# ─────────────────────────────────────────────────────────────
# Step 10 — Regression guard: `oc:favorite` still works after the
#           PROPPATCH handler was rewritten from an ad hoc
#           favorite-only scanner to generic dead-property
#           handling with an `oc:favorite` special case.
# ─────────────────────────────────────────────────────────────
PROPPATCH {{base_url}}/remote.php/dav/files/{{username}}/nc-dead-props-moved.txt
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


PROPFIND {{base_url}}/remote.php/dav/files/{{username}}/nc-dead-props-moved.txt
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


PROPPATCH {{base_url}}/remote.php/dav/files/{{username}}/nc-dead-props-moved.txt
Content-Type: application/xml; charset=utf-8
[BasicAuth]
{{nc_username}}: {{nc_password}}
```
<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:" xmlns:oc="http://owncloud.org/ns">
  <D:set>
    <D:prop>
      <oc:favorite>0</oc:favorite>
    </D:prop>
  </D:set>
</D:propertyupdate>
```

HTTP 207


PROPFIND {{base_url}}/remote.php/dav/files/{{username}}/nc-dead-props-moved.txt
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
xpath "string(//*[local-name()='favorite'])" == "0"


# ─────────────────────────────────────────────────────────────
# Cleanup — probe file.
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/remote.php/dav/files/{{username}}/nc-dead-props-moved.txt
[BasicAuth]
{{nc_username}}: {{nc_password}}

HTTP 204


# ─────────────────────────────────────────────────────────────
# Step 11 — Folder coverage: MKCOL, PROPPATCH, PROPFIND, cleanup.
# ─────────────────────────────────────────────────────────────
MKCOL {{base_url}}/remote.php/dav/files/{{username}}/nc-dead-props-folder/
[BasicAuth]
{{nc_username}}: {{nc_password}}

HTTP 201


PROPPATCH {{base_url}}/remote.php/dav/files/{{username}}/nc-dead-props-folder/
Content-Type: application/xml; charset=utf-8
[BasicAuth]
{{nc_username}}: {{nc_password}}
```
<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:" xmlns:X="oxi:test">
  <D:set>
    <D:prop>
      <X:foldermark>nc-folder-keeps-this</X:foldermark>
    </D:prop>
  </D:set>
</D:propertyupdate>
```

HTTP 207


PROPFIND {{base_url}}/remote.php/dav/files/{{username}}/nc-dead-props-folder/
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
xpath "string(//*[local-name()='foldermark'])" == "nc-folder-keeps-this"


DELETE {{base_url}}/remote.php/dav/files/{{username}}/nc-dead-props-folder/
[BasicAuth]
{{nc_username}}: {{nc_password}}

HTTP 204


# ─────────────────────────────────────────────────────────────
# Teardown — revoke the app password minted in Step 1.
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/auth/app-passwords/{{ap_id}}
Authorization: Bearer {{jwt}}

HTTP 200
