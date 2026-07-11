# =============================================================
# OxiCloud — WebDAV PATCH (RFC 5789) partial content update
# =============================================================
# RFC 4918 §9.7.1 forbids partial updates on PUT (a `Content-Range`
# on PUT is rejected, see webdav_handler.rs::handle_put). PATCH is
# the mechanism this server offers instead, via a dedicated
# `X-Update-Range` header: `bytes=<start>-<end>` (inclusive) or
# `append`. See webdav_handler.rs::handle_patch /
# parse_update_range for the implementation.
#
# Coverage:
#   1. Mid-file byte-range overwrite → 204, GET reflects the splice.
#   2. Append → 204, GET reflects the appended tail.
#   3. Out-of-range span (end >= size) → 416.
#   4. If-Match precondition failure → 412.
#   5. Locked resource without a lock token → 423.
#   6. PATCH on a directory → 409.
#   7. PATCH on a missing resource → 404.
#   8. PATCH without X-Update-Range → 400.
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
# Step 2 — PUT a 10-byte probe file: "0123456789"
# ─────────────────────────────────────────────────────────────
PUT {{base_url}}/webdav/patch-probe.txt
Authorization: Bearer {{token}}
Content-Type: text/plain
```
0123456789
```

HTTP 201
[Captures]
probe_etag: header "ETag"


# ─────────────────────────────────────────────────────────────
# Step 3 — Mid-file overwrite: replace bytes 3-5 (inclusive,
#          0-based) with "XYZ".
# ─────────────────────────────────────────────────────────────
PATCH {{base_url}}/webdav/patch-probe.txt
Authorization: Bearer {{token}}
X-Update-Range: bytes=3-5
Content-Type: text/plain
```
XYZ
```

HTTP 204
[Asserts]
header "Content-Range" matches "^bytes 3-\\d+/\\d+$"


GET {{base_url}}/webdav/patch-probe.txt
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
body startsWith "012XYZ"


# ─────────────────────────────────────────────────────────────
# Step 4 — Append to the end of the file.
# ─────────────────────────────────────────────────────────────
PATCH {{base_url}}/webdav/patch-probe.txt
Authorization: Bearer {{token}}
X-Update-Range: append
Content-Type: text/plain
```
-APPENDED
```

HTTP 204


GET {{base_url}}/webdav/patch-probe.txt
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
body endsWith "-APPENDED"


# ─────────────────────────────────────────────────────────────
# Step 5 — Out-of-range span: `end` must be strictly within the
#          current file size (growing via a byte-range PATCH
#          isn't supported — use `append` for that).
# ─────────────────────────────────────────────────────────────
PATCH {{base_url}}/webdav/patch-probe.txt
Authorization: Bearer {{token}}
X-Update-Range: bytes=1000-1005
Content-Type: text/plain
```
oops
```

HTTP 416


# ─────────────────────────────────────────────────────────────
# Step 6 — If-Match precondition failure.
# ─────────────────────────────────────────────────────────────
PATCH {{base_url}}/webdav/patch-probe.txt
Authorization: Bearer {{token}}
X-Update-Range: bytes=0-2
If-Match: "not-the-real-etag"
Content-Type: text/plain
```
NOP
```

HTTP 412


# ─────────────────────────────────────────────────────────────
# Step 7 — Locked resource without a matching lock token.
# ─────────────────────────────────────────────────────────────
LOCK {{base_url}}/webdav/patch-probe.txt
Authorization: Bearer {{token}}
Content-Type: application/xml; charset=utf-8
```
<?xml version="1.0" encoding="utf-8"?>
<D:lockinfo xmlns:D="DAV:">
  <D:lockscope><D:exclusive/></D:lockscope>
  <D:locktype><D:write/></D:locktype>
  <D:owner>patch-test</D:owner>
</D:lockinfo>
```

HTTP 200
[Captures]
lock_token: xpath "string(//*[local-name()='locktoken']/*[local-name()='href'])"


PATCH {{base_url}}/webdav/patch-probe.txt
Authorization: Bearer {{token}}
X-Update-Range: bytes=0-2
Content-Type: text/plain
```
NOP
```

HTTP 423


# Release the lock so cleanup below can proceed.
UNLOCK {{base_url}}/webdav/patch-probe.txt
Authorization: Bearer {{token}}
Lock-Token: <{{lock_token}}>

HTTP 204


# ─────────────────────────────────────────────────────────────
# Step 8 — PATCH on a directory → 409 Conflict.
# ─────────────────────────────────────────────────────────────
MKCOL {{base_url}}/webdav/patch-probe-dir/
Authorization: Bearer {{token}}

HTTP 201


PATCH {{base_url}}/webdav/patch-probe-dir/
Authorization: Bearer {{token}}
X-Update-Range: bytes=0-2
Content-Type: text/plain
```
NOP
```

HTTP 409


# ─────────────────────────────────────────────────────────────
# Step 9 — PATCH on a missing resource → 404.
# ─────────────────────────────────────────────────────────────
PATCH {{base_url}}/webdav/patch-probe-does-not-exist.txt
Authorization: Bearer {{token}}
X-Update-Range: bytes=0-2
Content-Type: text/plain
```
NOP
```

HTTP 404


# ─────────────────────────────────────────────────────────────
# Step 10 — PATCH without X-Update-Range → 400.
# ─────────────────────────────────────────────────────────────
PATCH {{base_url}}/webdav/patch-probe.txt
Authorization: Bearer {{token}}
Content-Type: text/plain
```
NOP
```

HTTP 400


# ─────────────────────────────────────────────────────────────
# Cleanup
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/webdav/patch-probe.txt
Authorization: Bearer {{token}}

HTTP 204


DELETE {{base_url}}/webdav/patch-probe-dir/
Authorization: Bearer {{token}}

HTTP 204
