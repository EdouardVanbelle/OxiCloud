# Plan — Expose dead properties as a REST metadata API

## Context

Migration `20260830000001` rekeyed `storage.webdav_dead_properties` from
`(resource_path, user_id)` to a polymorphic resource id
(`folder_id` XOR `file_id`) with `ON DELETE CASCADE`. The table is now a
clean per-resource key-value store: `(resource id, namespace, local_name) → value`,
shaped exactly like a generic metadata layer.

Today only WebDAV (PROPPATCH / PROPFIND) reads and writes it. NextCloud DAV
sees the rows (id-keyed, no path coupling) but isn't yet wired up to
emit/consume them. No REST surface exists.

We discussed the question "any interest in exposing this as a REST
metadata API?" on **2026-06-30** during the rekey landing and agreed it's
worth a follow-up plan but not part of the rekey itself. This document
captures the design we sketched so we can pick it up without
re-litigating.

## Why this is worth doing now (and not before the rekey)

| Pre-rekey schema | Post-rekey schema |
|---|---|
| `(resource_path, user_id)` key | `(folder_id XOR file_id, namespace, local_name)` key |
| Path-keyed → invalidated on rename / move | Id-keyed → stable across rename / move (DB invariant) |
| User-siloed → wrong for shared drives | Resource-state — correct under D1+ shared-drive semantics |
| Service-layer deletes leak tombstones | FK `ON DELETE CASCADE` reaps on every delete code path |

Pre-rekey, exposing the store via REST would have been wrong: REST clients
operate on resource ids, but the store keyed on paths; cross-protocol
parity would have been a mess. Post-rekey, the store IS already shaped
like the API we'd want — a thin REST layer matches it 1:1.

## Use cases

| Use case | What it looks like | Why dead-props help |
|---|---|---|
| Photo annotations | captions, ratings (1-5), notes per photo | already keyed by `file_id`; round-trips via WebDAV without re-implementing |
| Web-UI tags / labels | `oxi:user:tag/project=alpha`, color flags, "archived" markers | per-resource user metadata without new tables |
| Folder UI preferences | default sort, default view mode, "favourite" flag | persistent per-folder, shared across users on shared drives |
| Cross-protocol bridge | Thunderbird sets `oxi:lastsync=...` via PROPPATCH → web UI reads it via REST | one store, two surfaces — visibility goes both ways |
| Workflow / approval state | `reviewed_by=alice`, `due=2026-09-15` | ad-hoc state per resource without schema sprawl |
| Third-party integrations | external apps store scratch space per resource | lower barrier than implementing WebDAV |

Each use case is the same store; only the values differ. That's why
exposing it as a generic API is more leverage than adding ad-hoc columns
for any one of them.

## API shape (decided)

### Per-resource CRUD — nested under the resource

```
GET    /api/files/{id}/metadata                            → list all keys
GET    /api/files/{id}/metadata/{namespace}/{name}         → fetch one value
PUT    /api/files/{id}/metadata/{namespace}/{name}         → upsert (body = value)
DELETE /api/files/{id}/metadata/{namespace}/{name}         → remove one key

GET    /api/folders/{id}/metadata                          → list all keys
GET    /api/folders/{id}/metadata/{namespace}/{name}       → fetch one value
PUT    /api/folders/{id}/metadata/{namespace}/{name}       → upsert
DELETE /api/folders/{id}/metadata/{namespace}/{name}       → remove one key
```

The `{kind}` is encoded in the URL prefix, so we don't carry a
discriminator field. `{namespace}` and `{name}` are passed verbatim to
the store; URL-encode the colon-containing namespaces
(`oxi:user:tag` → `oxi%3Auser%3Atag`).

This shape matches the rest of the API — `/api/files/{id}/thumbnail`,
`/api/files/{id}/preview`, `/api/folders/{id}/contents` — and stays
discoverable as a sub-resource of the file/folder.

AuthZ goes through the `_with_perms` service path:
- `Read` on the resource → GET allowed.
- `Update` on the resource → PUT / DELETE allowed.
- 404 on no-Read (anti-enumeration), 403 on Read-but-no-Update.

GET (list) response shape:

```json
{
  "properties": [
    {
      "namespace": "oxi:user:tag",
      "name": "project",
      "value": "alpha",
      "updated_by": "<user-uuid-or-null>",
      "updated_at": "2026-06-30T20:33:38Z"
    },
    ...
  ]
}
```

The resource itself is identified by the URL — no need to echo
`{ "kind": ..., "id": ... }` in the body.

### Cross-resource lookup — separate search endpoint (deferred to phase 3)

Cross-resource search ("which files have `oxi:user:tag/project=alpha`?")
is a fundamentally different operation from CRUD — it's a SEARCH, not a
fetch. Nesting it under a single resource URL would be wrong, and
overloading CRUD with `?filter=...` would muddy the shape. It lives at
its own endpoint:

```
GET /api/search/metadata?namespace=...&name=...&value=...&kind=file
```

This separation has three concrete payoffs:

- **CRUD path stays simple**: per-resource fetch/upsert/remove with no
  query-string filter logic.
- **AuthZ shape differs**: per-resource CRUD enforces permissions on
  ONE resource; search must enumerate every resource the caller can
  Read, then filter. That's expensive enough to need its own
  rate-limit / pagination story. Isolating it keeps the CRUD path
  cheap.
- **Search can grow** more filter syntax (multiple keys, value
  patterns, `>` / `<` comparisons) without touching the CRUD shape.

Search is **phase 3** — it's not required for the read-only or
read-write cases (phases 1 and 2). Don't build it until a UI feature
asks for it.

Note this is the LOW-VOLUME lookup option. Genuine tag-based faceted
browse at scale needs a first-class tags table with indexes — not a
metadata-table scan. The search endpoint exists for debugging, small
instances, and occasional one-off queries. See "Out of scope" below.

## Schema additions needed

```sql
ALTER TABLE storage.webdav_dead_properties
    ADD COLUMN updated_by UUID NULL REFERENCES auth.users(id) ON DELETE SET NULL;
```

The `updated_at` column already exists. `updated_by` is the new bit —
load-bearing if both WebDAV and REST are writing. Without it, "why did
this caption change overnight?" is blind.

Set on every `set()` / `remove()` (the latter currently has no provenance
concept, but the audit value would be "who reaped it" — same column).
`ON DELETE SET NULL` so a user delete doesn't lose the property itself,
only the authorship — symmetric with how other audit columns in the
schema behave (`created_by` on folders/files is `ON DELETE SET NULL` for
the same reason).

## Decisions to lock in before implementation

### 1. Namespace policy — denylist or allowlist?

Server-managed namespaces (`DAV:`, anything we want to use internally for
sync state, locks, etc.) should be REST-write-rejected so REST can't
poison live WebDAV behaviour.

**Recommendation: denylist.** More permissive, less surprising, matches
the WebDAV side (which lets clients write any namespace they please).
Initial denylist:

- `DAV:` — RFC 4918 live properties; server-managed.
- `oxi:internal:*` — reserved for future server-managed properties.

REST read is unrestricted; only write is filtered.

### 2. Size limits

Today no cap. WebDAV is bounded by `MAX_XML_BODY` (1 MB) on the request,
but per-row there's no limit and no per-resource key-count limit. A
REST API in the wild needs both:

- per-value: **64 KB** (enough for any human-authored caption, JSON blob,
  or sync token; rejects "use the metadata table as a file store"
  abuse).
- per-resource: **100 keys** (enough for any reasonable application;
  rejects "use it as a directory listing").

Both as 413 Payload Too Large on the offending endpoint.

WebDAV PROPPATCH should adopt the same per-key limit (currently bounded
only by the 1 MB body); per-resource count limit applies on the
incremental write.

### 3. Value content type

Stored as `TEXT` today. If REST PUTs JSON, WebDAV clients reading it
back via PROPFIND wrap it in their XML envelope and see `"{...}"` as a
literal string. Defensible (it's "just a string"); document the
convention.

If we add a `content_type` column (RFC 4918 §15.5 `getcontenttype` on
properties is murky), REST can return the original `Content-Type` to
REST callers and WebDAV continues to see the literal value. Probably
**not worth it** until a real use case needs it — adds a column + a
write path branch for zero functional benefit today.

### 4. Listing semantics

Inlining child metadata into `GET /api/folders/{id}/contents` is
tempting — fewer round-trips for the UI — but PROPFIND already pays this
O(N) cost and it's expensive on big folders.

**Recommendation: dedicated endpoint only.** No inlining. UIs that need
per-child metadata can batch via `GET /api/files/{id}/metadata` calls
in parallel (HTTP/2 multiplexing makes that cheap) until measured
demand justifies a bulk endpoint.

### 5. WebDAV-write hygiene

REST writes go through the same `DeadPropertyStore::set` as PROPPATCH —
no special branch. The denylist (above) gates which namespaces REST may
write; WebDAV stays unrestricted.

## Scope: phase 1 / 2 / 3

### Phase 1 (read-only)

GET endpoints only, nested under `/api/{files,folders}/{id}/metadata`:

- `GET /api/files/{id}/metadata`                            → list
- `GET /api/files/{id}/metadata/{namespace}/{name}`         → single
- `GET /api/folders/{id}/metadata`                          → list
- `GET /api/folders/{id}/metadata/{namespace}/{name}`       → single

Plus the `updated_by` schema migration (so phase 2 doesn't break wire
contracts).

Use case unlocked: the SvelteKit UI can READ properties Thunderbird /
DAVx5 / Cyberduck have written. Cross-protocol visibility, one
direction.

Cost: ~150 LOC handler + ~20 LOC migration. No new authz primitives —
`Read` permission already exists.

### Phase 2 (write)

PUT + DELETE. Namespace denylist. Size limits.

Decide first what the primary REST writer is:

- **Photo captions**: probably wants a dedicated `/api/photos/{id}/caption`
  endpoint that stores under a fixed `oxi:photo:caption` key. Generic
  API still useful but not the obvious surface.
- **Tags**: deserves a structured tags table (queryable, faceted search)
  rather than k/v. Generic API is the wrong shape.
- **Workflow state**: generic API IS the right shape — exactly what k/v
  was designed for.
- **Third-party integrations**: generic API is the right shape.

If the primary writer turns out to be one of the structured-data cases,
phase 2 may never ship — the read API plus a dedicated write endpoint
per feature is the better factoring. The decision should be driven by
real demand, not speculative design.

### Phase 3 (cross-resource search)

`GET /api/search/metadata?namespace=...&name=...&value=...&kind=file`
with pagination. AuthZ enumerates resources the caller has Read on
and filters in-engine.

Scope guidance:

- Low-volume **only**. Implemented as a sequential scan with
  permission filter. No new indexes (the (`namespace`, `local_name`,
  `value`) shape doesn't index cheaply, and adding `value` to a
  composite index changes the table's write pattern).
- Filter syntax stays minimal until a UI feature drives it. Start
  with `namespace=` and `name=`; add `value=` literal match next; add
  `value~=` pattern match only when needed.
- Hard rate-limit per caller — search is expensive enough that an
  unbounded REST client could starve the database.

If demand for tag-based browse at scale ever materialises, do NOT
extend this endpoint — build a dedicated tags table with an inverted
index. Search-on-metadata is the debug/scratch tool, not the
production tag system.

## Out of scope

- **Tags / faceted search** — deserves a first-class table with foreign
  keys and a search index. The metadata API can hold tags as values,
  but querying "all files tagged `alpha`" via the metadata table is a
  full scan. Don't build features that need indexed tag queries on top
  of this.
- **Live properties** (RFC 4918 §15) — `getcontentlength`,
  `creationdate`, `getetag`, etc. are server-computed. The REST API
  exposes only dead properties; live properties are derived from the
  resource and surface through their existing endpoints.
- **Bulk write** — phase 2 could add a batch endpoint if needed, but
  initial design ships one-at-a-time. Bulk read (list) is already
  there.
- **Versioning / history** — `updated_at` + `updated_by` give an audit
  timestamp but not history. If someone wants "show me the previous
  caption", that's a separate append-only journal table.
- **WebDAV PROPPATCH size enforcement** mentioned above as "should
  adopt the same limit" — actually applying the limit to PROPPATCH is
  a separate small change, deferable.

## Open questions left for implementation

1. **Sub-resource name** — `/metadata` matches REST conventions and
   reads naturally to API consumers; `/properties` would be more
   WebDAV-faithful but users don't know what "dead properties" means.
   Locked in: `/metadata`.
2. **Permission for empty list** — GET on a resource with zero metadata
   returns `{ properties: [] }` and 200, OR 404? RFC 4918 §9.1 says
   PROPFIND on a resource that exists but has no requested properties
   is 207 with empty `<D:prop>`. REST should mirror: 200 with empty
   array. 404 only when the underlying resource doesn't exist (or
   anti-enum 404 on no-Read).
3. **Listing order** — alphabetical by `(namespace, name)`, or by
   `updated_at DESC`? Probably the former (stable, deterministic).
4. **Caching** — `ETag` on the list response? Cheap if the underlying
   resource already emits one; we could derive a sub-ETag from
   `MAX(updated_at)` across the metadata rows. Defer until UI asks.
5. **NC DAV wiring** (see memory note
   `project-nc-webdav-dead-props-unwired.md`) — the read API would
   work over the unwired NC surface trivially since REST and NC DAV
   read the same store; the unwired bit is just PROPPATCH/PROPFIND on
   the NC URL prefix. Worth doing alongside phase 1 read so the cross-
   protocol story is complete on day one.
6. **Search endpoint shape** (phase 3, not phase 1) — the cross-
   resource lookup `/api/search/metadata?...` will need pagination,
   sort, and resource-kind filter; design when the first concrete
   use case lands. Don't speculatively build it.

## Verification (phase 1)

1. Migration adds `updated_by`; downgrade leaves data intact (no
   `DROP COLUMN` on rollback — just leave it; harmless).
2. `cargo check` green; `cargo clippy --all-features --all-targets -D
   warnings` green.
3. New Hurl scenario `tests/api/metadata_api.hurl`:
   - PUT a file via WebDAV, PROPPATCH a marker → 207.
   - GET `/api/files/{id}/metadata/oxi%3Atest/marker` → 200 with the
     value.
   - GET `/api/files/{id}/metadata` → 200 with list containing the
     marker.
   - Asserts `updated_by` field is the authenticated user id.
   - GET against a foreign user's file → 404 (anti-enum).
   - GET `/api/folders/{id}/metadata` round-trip on a folder PROPPATCH.
4. The existing `webdav_dead_properties.hurl` continues to pass —
   read-only REST shouldn't perturb any existing write path.

## Verification (phase 2)

To be expanded when phase 2 starts. At minimum:

- PUT writes round-trip via PROPFIND.
- PUT to denylisted namespace → 403.
- PUT exceeding per-value cap → 413.
- PUT exceeding per-resource cap → 413.
- DELETE removes via PROPFIND verification.
- PUT/DELETE without Update permission → 403.

## Memory notes to write when this lands

- `project_metadata_api.md` — phase shipped, what's still open.
- Update `project_webdav_dead_properties_drive_rekey.md` to mention
  this API as the consumer that justified the id-rekey effort.
- If the namespace denylist is contentious, capture it as
  `feedback_metadata_namespace_denylist.md`.
