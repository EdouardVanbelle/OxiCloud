# =============================================================
# OxiCloud — WebDAV dead-properties (RFC 4918 §4.2) end-to-end
# =============================================================
# Exercises the PROPPATCH/PROPFIND round-trip backed by
# `storage.webdav_dead_properties` (the table introduced in
# migration 20260825000000) and the DeadPropertyStore service at
# src/infrastructure/services/webdav_dead_property_store.rs.
#
# Dead properties are client-authored XML that the server stores
# verbatim — Thunderbird, DAVx5, NextCloud-desktop, Cyberduck all
# use them to persist per-resource labels / sync state. A
# regression where PROPPATCH succeeds but PROPFIND returns nothing
# is silently catastrophic for those clients (they think the
# server is broken; OxiCloud sees nothing wrong in its logs).
#
# Coverage:
#   1.  Setup admin, capture JWT, PUT a probe file.
#   2.  PROPPATCH set → 207
#   3.  PROPFIND get → value round-trips verbatim
#   4.  PROPPATCH upsert (set same name → new value) → 207
#   5.  PROPFIND get → new value (upsert worked)
#   6.  PROPPATCH remove → 207
#   7.  PROPFIND get → property absent
#   8.  MOVE file → properties follow the resource id automatically
#       (no rename_resource() call; the row's file_id is stable
#       across MOVE so dead-props travel with the resource).
#   9.  PROPFIND on moved path returns the property.
#   10. DELETE via WebDAV → FK CASCADE reaps dead-prop rows.
#   11. PROPPATCH + REST DELETE `/api/files/{id}` → FK CASCADE
#       reaps via the REST-side delete path too. This is the
#       new coverage unlocked by migration 20260830000001 — the
#       old path-keyed store had no way to clean up here, so
#       the SvelteKit web UI (which deletes via REST) was
#       silently leaking tombstones every time a user deleted
#       a file that had ever carried dead properties.
#   12. Folder MOVE preserves dead properties (id-stable
#       guarantee under rename). The Hurl suite had no folder-
#       side coverage of this until 20260830000001; only the
#       file MOVE case (step 9) was guarded.
#   13. Single-file COPY duplicates dead properties (RFC 4918
#       §8.8). Destination carries a copy of the source's
#       marker; source retains its copy (COPY ≠ MOVE).
#       Implementation: `dead_prop_copy` CTE branch in
#       `copy_file` (migration 20260830000002).
#   14. Folder COPY (Depth: infinity) duplicates dead
#       properties for every descendant — both folder and file
#       dead-props. Implementation: the two INSERT...SELECT
#       branches in `storage.copy_folder_tree` (migration
#       20260830000002) that walk `_copy_map` and the new
#       `_copy_file_map` respectively.
#
# XPath assertions deliberately use `local-name()` so the test
# is robust against the server's choice of namespace prefix —
# DeadPropertyStore generates `X:` but a future implementation
# is free to pick something else as long as `xmlns:X` is correct.
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


# Resolve the user's home folder so the WebDAV path lives somewhere
# valid. tests/api/files-folders.hurl runs before us and may have
# left state; we deliberately pick a unique filename below to
# avoid collisions.
GET {{base_url}}/api/folders
Authorization: Bearer {{token}}

HTTP 200


# ─────────────────────────────────────────────────────────────
# Step 2 — PUT a probe file via native WebDAV. The dead-property
#          handler keys on the resource path; we need a real file
#          there so MOVE/DELETE assertions later are meaningful.
# ─────────────────────────────────────────────────────────────
PUT {{base_url}}/webdav/dead-props-probe.txt
Authorization: Bearer {{token}}
Content-Type: text/plain
```
hello dead properties
```

# Post 43cf4a2b: PUT returns 201 on create, 204 on overwrite.
# This file is fresh (no prior PUT in the test), so 201 is the
# canonical answer.
HTTP 201


# ─────────────────────────────────────────────────────────────
# Step 3 — PROPPATCH set a single dead property.
#
#          The XML body sets `<X:testlabel xmlns:X="oxi:test">
#          hello</X:testlabel>`. RFC 4918 §9.2 says PROPPATCH
#          MUST return 207 Multi-Status with a per-property
#          status; we assert both the envelope status and the
#          inner 200 OK for our property.
# ─────────────────────────────────────────────────────────────
PROPPATCH {{base_url}}/webdav/dead-props-probe.txt
Authorization: Bearer {{token}}
Content-Type: application/xml; charset=utf-8
```
<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:" xmlns:X="oxi:test">
  <D:set>
    <D:prop>
      <X:testlabel>hello-dead-property</X:testlabel>
    </D:prop>
  </D:set>
</D:propertyupdate>
```

HTTP 207
[Asserts]
# At least one propstat reports success for the property we set.
# Using local-name() so we don't have to bind a prefix to DAV:.
xpath "string(//*[local-name()='propstat']/*[local-name()='status'])" contains "200 OK"


# ─────────────────────────────────────────────────────────────
# Step 4 — PROPFIND. The dead-property propstat block should
#          contain `testlabel` with the value we set. The server's
#          response uses an `X:` prefix bound via `xmlns:X` to our
#          original namespace — we match by local-name() to stay
#          decoupled from that choice.
# ─────────────────────────────────────────────────────────────
PROPFIND {{base_url}}/webdav/dead-props-probe.txt
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
xpath "string(//*[local-name()='testlabel'])" == "hello-dead-property"


# ─────────────────────────────────────────────────────────────
# Step 5 — Upsert: setting the same property with a new value
#          must overwrite, not duplicate (ON CONFLICT DO UPDATE).
# ─────────────────────────────────────────────────────────────
PROPPATCH {{base_url}}/webdav/dead-props-probe.txt
Authorization: Bearer {{token}}
Content-Type: application/xml; charset=utf-8
```
<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:" xmlns:X="oxi:test">
  <D:set>
    <D:prop>
      <X:testlabel>updated-value</X:testlabel>
    </D:prop>
  </D:set>
</D:propertyupdate>
```

HTTP 207


# ─────────────────────────────────────────────────────────────
# Step 6 — PROPFIND confirms the new value AND that there's still
#          only one such property (no duplicate row in the DB).
#          `count(//*[local-name()='testlabel'])` is the
#          dup-detection assertion.
# ─────────────────────────────────────────────────────────────
PROPFIND {{base_url}}/webdav/dead-props-probe.txt
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
xpath "string(//*[local-name()='testlabel'])" == "updated-value"
xpath "count(//*[local-name()='testlabel'])"  == 1


# ─────────────────────────────────────────────────────────────
# Step 7 — Remove the dead property.
# ─────────────────────────────────────────────────────────────
PROPPATCH {{base_url}}/webdav/dead-props-probe.txt
Authorization: Bearer {{token}}
Content-Type: application/xml; charset=utf-8
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


# ─────────────────────────────────────────────────────────────
# Step 8 — PROPFIND now returns no instance of `testlabel`.
# ─────────────────────────────────────────────────────────────
PROPFIND {{base_url}}/webdav/dead-props-probe.txt
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
xpath "count(//*[local-name()='testlabel'])" == 0


# ─────────────────────────────────────────────────────────────
# Step 9 — Re-set a property, then MOVE the file. Post-rekey
#          (migration 20260830000001) the dead-property row
#          keys on `file_id`, which never changes across MOVE
#          or RENAME — so properties follow the resource by a
#          database invariant, without any store-side call.
#          A regression that broke this would be a regression
#          on the id-stability guarantee in the move SQL itself
#          (i.e. it would surface elsewhere too); this assertion
#          locks it in for sync clients that do MOVE → PROPFIND.
# ─────────────────────────────────────────────────────────────
PROPPATCH {{base_url}}/webdav/dead-props-probe.txt
Authorization: Bearer {{token}}
Content-Type: application/xml; charset=utf-8
```
<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:" xmlns:X="oxi:test">
  <D:set>
    <D:prop>
      <X:testlabel>survives-move</X:testlabel>
    </D:prop>
  </D:set>
</D:propertyupdate>
```

HTTP 207


MOVE {{base_url}}/webdav/dead-props-probe.txt
Authorization: Bearer {{token}}
Destination: {{base_url}}/webdav/dead-props-moved.txt

# RFC 4918 §9.9.4: MOVE returns 201 Created when the destination
# didn't exist (the resource appears there for the first time);
# 204 No Content when overwriting an existing destination. The
# destination is fresh here → 201.
HTTP 201


PROPFIND {{base_url}}/webdav/dead-props-moved.txt
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
xpath "string(//*[local-name()='testlabel'])" == "survives-move"


# ─────────────────────────────────────────────────────────────
# Step 10 — DELETE the file via WebDAV; the FK
#           `webdav_dead_properties.file_id → storage.files.id
#           ON DELETE CASCADE` (migration 20260830000001) must
#           reap the dead-property rows automatically, so they
#           don't accumulate as tombstones the next time a file
#           is created at the same path. We verify by recreating
#           the same path and PROPFIND'ing — a leak would
#           resurface the old "survives-move" value.
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/webdav/dead-props-moved.txt
Authorization: Bearer {{token}}

HTTP 204


PUT {{base_url}}/webdav/dead-props-moved.txt
Authorization: Bearer {{token}}
Content-Type: text/plain
```
fresh file at the same path
```

# Fresh resource at the same path after DELETE → 201, same shape
# as Step 2's initial PUT.
HTTP 201


PROPFIND {{base_url}}/webdav/dead-props-moved.txt
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
# Old value MUST NOT come back — proves DELETE cleaned up.
xpath "count(//*[local-name()='testlabel'])" == 0


# ─────────────────────────────────────────────────────────────
# Step 11 — Same FK-cascade property test but via the REST API
#           delete path. The path-based store would have leaked
#           here forever (REST DELETE receives a file_id, not a
#           path; the old store had no efficient way to clean
#           up). The id-keyed schema reaps the dead-property
#           row through the same FK CASCADE on `storage.files`,
#           so this proves the new coverage end-to-end.
#
#           Sequence:
#             a. PROPPATCH a marker dead property on the file.
#             b. PROPFIND — confirm it's stored.
#             c. Resolve the file's id via REST listing of the
#                home folder.
#             d. DELETE via `/api/files/{id}` — pure REST,
#                never touches the WebDAV surface.
#             e. PUT a fresh file at the same WebDAV path.
#             f. PROPFIND — must not see the marker.
# ─────────────────────────────────────────────────────────────

# Step 11a — set a new marker dead property on the just-PUT file
PROPPATCH {{base_url}}/webdav/dead-props-moved.txt
Authorization: Bearer {{token}}
Content-Type: application/xml; charset=utf-8
```
<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:">
  <D:set>
    <D:prop>
      <X:restmarker xmlns:X="oxi:test">rest-delete-coverage</X:restmarker>
    </D:prop>
  </D:set>
</D:propertyupdate>
```

HTTP 207


# Step 11b — confirm the marker is stored
PROPFIND {{base_url}}/webdav/dead-props-moved.txt
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
xpath "string(//*[local-name()='restmarker'])" == "rest-delete-coverage"


# Step 11c — resolve the file id from the home folder listing.
# The home folder is whatever `GET /api/folders` returns as the
# first root-level entry for the admin user (Personal drive root,
# post drive-no-wrapper).
GET {{base_url}}/api/folders
Authorization: Bearer {{token}}

HTTP 200
[Captures]
home_folder_id: jsonpath "$[0].id"


GET {{base_url}}/api/files?folder_id={{home_folder_id}}
Authorization: Bearer {{token}}

HTTP 200
[Captures]
# Hurl quirk: `$[?(...)]` collapses to a scalar (not a list) when the
# filter matches exactly one element, so `nth 0` fails with "invalid
# filter input type". The bare filter capture returns that scalar
# directly. Filename uniqueness across the home folder makes the
# single-match assumption safe — `dead-props-moved.txt` is created
# only by this test (no other Hurl test ever PUTs that name).
rest_file_id: jsonpath "$[?(@.name=='dead-props-moved.txt')].id"


# Step 11d — REST DELETE. No webdav, no dead-prop API call —
# the cleanup must happen via the FK CASCADE on storage.files.
DELETE {{base_url}}/api/files/{{rest_file_id}}
Authorization: Bearer {{token}}

# The REST delete handler returns 204 No Content on success.
HTTP 204


# Step 11e — recreate the file at the same WebDAV path
PUT {{base_url}}/webdav/dead-props-moved.txt
Authorization: Bearer {{token}}
Content-Type: text/plain
```
fresh file post REST DELETE
```

HTTP 201


# Step 11f — PROPFIND must not surface the old marker
PROPFIND {{base_url}}/webdav/dead-props-moved.txt
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
# If REST DELETE failed to cascade, the marker would still be
# attached to the (recreated) path under the old `(path, user_id)`
# key — but the new schema keys by file_id, and the REST DELETE
# took the storage.files row with it. Asserting absence proves
# the cascade fired.
xpath "count(//*[local-name()='restmarker'])" == 0


# ─────────────────────────────────────────────────────────────
# Cleanup
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/webdav/dead-props-moved.txt
Authorization: Bearer {{token}}

HTTP 204


# ─────────────────────────────────────────────────────────────
# Step 12 — Folder MOVE dead-property preservation.
#           Same invariant as step 9 (id-stability under MOVE)
#           but for folders. The Hurl suite had no folder-side
#           coverage of this until now, so a regression that
#           broke folder dead-property preservation could
#           silently land — calendar / contacts / NextCloud
#           clients that PROPPATCH per-folder sync state would
#           lose it on every rename.
#
#           Sequence:
#             a. MKCOL a fresh test folder.
#             b. PROPPATCH a dead property on it.
#             c. MOVE / rename the folder.
#             d. PROPFIND the new collection path; assert
#                the property survived.
#             e. Cleanup: DELETE the renamed folder.
# ─────────────────────────────────────────────────────────────

# Step 12a — fresh collection (no prior state at this path)
MKCOL {{base_url}}/webdav/dead-props-folder/
Authorization: Bearer {{token}}

HTTP 201


# Step 12b — attach a marker dead property to the FOLDER row
PROPPATCH {{base_url}}/webdav/dead-props-folder/
Authorization: Bearer {{token}}
Content-Type: application/xml; charset=utf-8
```
<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:">
  <D:set>
    <D:prop>
      <X:foldermark xmlns:X="oxi:test">folder-keeps-this</X:foldermark>
    </D:prop>
  </D:set>
</D:propertyupdate>
```

HTTP 207


# Step 12c — rename the folder via MOVE. Same-parent rename
# (intra-collection name change) — the most common shape clients
# issue and the one that previously needed `rename_resource()`
# to keep dead properties attached.
MOVE {{base_url}}/webdav/dead-props-folder/
Authorization: Bearer {{token}}
Destination: {{base_url}}/webdav/dead-props-folder-renamed/

HTTP 201


# Step 12d — PROPFIND the new collection path; the dead property
# must still be attached. If the folder row's id had changed
# under MOVE (it doesn't), or if anything had reaped the
# webdav_dead_properties row, the property would be gone.
PROPFIND {{base_url}}/webdav/dead-props-folder-renamed/
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
xpath "string(//*[local-name()='foldermark'])" == "folder-keeps-this"


# Step 12e — cleanup. The DELETE cascades the foldermark row
# away via FK ON DELETE CASCADE, leaving the schema clean for
# any subsequent test that touches this path.
DELETE {{base_url}}/webdav/dead-props-folder-renamed/
Authorization: Bearer {{token}}

HTTP 204


# ─────────────────────────────────────────────────────────────
# Step 13 — Single-file COPY duplicates dead properties.
#           RFC 4918 §8.8: dead properties MUST be duplicated.
#           Implementation is the `dead_prop_copy` CTE branch in
#           `file_blob_write_repository::copy_file` (inserts a
#           new dead-prop row per source row, keyed on the new
#           file's id).
#
#           Sequence:
#             a. PUT a source file.
#             b. PROPPATCH a marker dead property.
#             c. COPY (WebDAV) to a new path.
#             d. PROPFIND the new path; marker must be present.
#             e. PROPFIND the source path; marker still present
#                on source too (COPY duplicates — it doesn't
#                move).
#             f. Cleanup both files.
# ─────────────────────────────────────────────────────────────

# Step 13a — source file
PUT {{base_url}}/webdav/dead-props-copy-src.txt
Authorization: Bearer {{token}}
Content-Type: text/plain
```
copy source
```

HTTP 201


# Step 13b — set the marker dead property on the source
PROPPATCH {{base_url}}/webdav/dead-props-copy-src.txt
Authorization: Bearer {{token}}
Content-Type: application/xml; charset=utf-8
```
<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:">
  <D:set>
    <D:prop>
      <X:copymark xmlns:X="oxi:test">survives-copy</X:copymark>
    </D:prop>
  </D:set>
</D:propertyupdate>
```

HTTP 207


# Step 13c — COPY the file. Destination is fresh → 201 Created.
# §9.8.5: 201 when destination is new, 204 when overwriting.
COPY {{base_url}}/webdav/dead-props-copy-src.txt
Authorization: Bearer {{token}}
Destination: {{base_url}}/webdav/dead-props-copy-dst.txt

HTTP 201


# Step 13d — destination must carry the property (RFC 4918 §8.8)
PROPFIND {{base_url}}/webdav/dead-props-copy-dst.txt
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
xpath "string(//*[local-name()='copymark'])" == "survives-copy"


# Step 13e — source still has it too (COPY, not MOVE)
PROPFIND {{base_url}}/webdav/dead-props-copy-src.txt
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
xpath "string(//*[local-name()='copymark'])" == "survives-copy"


# Step 13f — cleanup both
DELETE {{base_url}}/webdav/dead-props-copy-src.txt
Authorization: Bearer {{token}}

HTTP 204


DELETE {{base_url}}/webdav/dead-props-copy-dst.txt
Authorization: Bearer {{token}}

HTTP 204


# ─────────────────────────────────────────────────────────────
# Step 14 — Folder COPY duplicates dead properties on every
#           descendant. RFC 4918 §8.8 + §9.8.3 (Depth: infinity
#           for collections). Implementation is the two
#           INSERT...SELECT branches added to
#           `storage.copy_folder_tree` in migration
#           20260830000002:
#             - folders mapped via `_copy_map`
#             - files mapped via the new `_copy_file_map`
#
#           Test shape:
#             a. MKCOL outer collection.
#             b. MKCOL inner collection (descendant).
#             c. PUT a leaf file inside inner.
#             d. PROPPATCH a marker on the descendant FOLDER.
#             e. PROPPATCH a different marker on the leaf FILE.
#             f. COPY outer/ → outer-copy/ (Depth: infinity).
#             g. PROPFIND descendant in copy; marker present.
#             h. PROPFIND leaf in copy; marker present.
#             i. Cleanup both trees.
# ─────────────────────────────────────────────────────────────

# Step 14a/b/c — build the source subtree
MKCOL {{base_url}}/webdav/dead-props-copy-tree/
Authorization: Bearer {{token}}

HTTP 201


MKCOL {{base_url}}/webdav/dead-props-copy-tree/inner/
Authorization: Bearer {{token}}

HTTP 201


PUT {{base_url}}/webdav/dead-props-copy-tree/inner/leaf.txt
Authorization: Bearer {{token}}
Content-Type: text/plain
```
leaf inside the copy tree
```

HTTP 201


# Step 14d — marker on the descendant FOLDER
PROPPATCH {{base_url}}/webdav/dead-props-copy-tree/inner/
Authorization: Bearer {{token}}
Content-Type: application/xml; charset=utf-8
```
<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:">
  <D:set>
    <D:prop>
      <X:innermark xmlns:X="oxi:test">inner-folder-mark</X:innermark>
    </D:prop>
  </D:set>
</D:propertyupdate>
```

HTTP 207


# Step 14e — marker on the leaf FILE
PROPPATCH {{base_url}}/webdav/dead-props-copy-tree/inner/leaf.txt
Authorization: Bearer {{token}}
Content-Type: application/xml; charset=utf-8
```
<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:">
  <D:set>
    <D:prop>
      <X:leafmark xmlns:X="oxi:test">leaf-file-mark</X:leafmark>
    </D:prop>
  </D:set>
</D:propertyupdate>
```

HTTP 207


# Step 14f — recursive COPY (Depth: infinity is the default for
# collections per RFC 4918 §9.8.3). Destination is fresh → 201.
COPY {{base_url}}/webdav/dead-props-copy-tree/
Authorization: Bearer {{token}}
Destination: {{base_url}}/webdav/dead-props-copy-tree-clone/

HTTP 201


# Step 14g — descendant folder in the COPY carries the folder marker.
# The path resolves only if `storage.copy_folder_tree` correctly
# duplicated the descendant folder AND its dead-prop row.
PROPFIND {{base_url}}/webdav/dead-props-copy-tree-clone/inner/
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
xpath "string(//*[local-name()='innermark'])" == "inner-folder-mark"


# Step 14h — leaf file in the COPY carries the file marker
PROPFIND {{base_url}}/webdav/dead-props-copy-tree-clone/inner/leaf.txt
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
xpath "string(//*[local-name()='leafmark'])" == "leaf-file-mark"


# Step 14i — cleanup both trees. Recursive DELETE cascades each
# subtree's folder + file rows, and the FK ON DELETE CASCADE on
# webdav_dead_properties takes the dead-prop rows with them.
DELETE {{base_url}}/webdav/dead-props-copy-tree/
Authorization: Bearer {{token}}

HTTP 204


DELETE {{base_url}}/webdav/dead-props-copy-tree-clone/
Authorization: Bearer {{token}}

HTTP 204
