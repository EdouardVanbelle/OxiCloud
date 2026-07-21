# Round 4 — row-path allocs, drive-selector cache, CalDAV parse, PROPFIND emit, N+1 hydration, Azure streaming, faces bound

Eight benchmark-gated changes. Rule of the round (same as ROUND2/ROUND3):
every change ships with a BEFORE/AFTER benchmark; an AFTER that doesn't
beat its BEFORE gets rolled back — none did. Equivalence gates
(byte-identical output / identical row or id sets / BLAKE3 payload
identity) guard every behavior-preserving rewrite.

Measured on 4 cores / 15 GiB, local PostgreSQL 16 (fsync off), release
profile. Reproduce any row with the command in its section.

## Summary

| # | change | key metric | before → after |
|--:|---|---|---|
| 1 | Row→entity path build (one-pass) | ns/row file / allocs | 743 → 417 (**1.78x**), 15.8 → 10.5 |
| 2 | Drive-selector readable-cache | µs/resolution p50, 8 conns | 441 → 0.80 (**~550x**), queries → 0 |
| 3 | CalDAV single-parse `from_ical` | µs/event PUT parse | 83.8 → 11.8 (**7.1x**) |
| 4 | CalDAV read-side copies | chunk ns / group µs (5k) | 297 → 215 (**1.4x**) / 1221 → 951 (**1.3x**) |
| 5 | PROPFIND XML emit | µs/1100-row page / allocs/row | 1535 → 1253 (**1.22x**), 17.9 → 12.0 |
| 6 | Grant-listing hydration batch | ms/listing K=15 | 4.4 → 0.33 (**~13x**), 15 queries → 1 |
| 7 | user-flags single-flight | cold herd of 32 | 32 → 1 query, 4.7 → 0.6 ms |
| 8 | Azure download streaming | TTFB / peak heap, 256 MiB | 349 → 4 ms (**87x**), 480 → 1.9 MiB (**254x**) |
| 9 | Face-indexing semaphore | peak live heap, 48 images | 1175 → 176 MiB (**6.7x**), wall also −13% |

---

## [1] PG row → entity path materialization — one-pass builders — 1.78x

Every listing row (PROPFIND batches, photos timeline, search pages,
by-ids enrichment, subtree ZIP streams) paid this chain: files re-joined
the materialized folder path with `format!`, split the copy into a
per-segment `Vec<String>`, NFC-copied the already-NFC name
(`normalize_storage_name` always allocated), then `Display`/`join`
re-joined the segments it had just split into `path_string` — the only
form the DTOs actually serve. Folders arrived with an owned canonical
`path` column, split it, dropped it, and rebuilt an identical String.

Now: `StoragePath::from_folder_and_name` / `from_joined` build segments
AND the joined string in one pass (`from_joined` reuses the owned input
when canonical — every row the repository writes), the entity
constructors take the name by value through the new zero-copy
`normalize_storage_name_owned`, `Display` writes segments without the
`join` temp, and both duplicated repo-side `make_file_path` copies were
replaced by the shared builder (`File::from_materialized_row` /
`Folder::from_materialized_row`).

```
cargo run --release --features bench --example bench_row_path
# 10k rows, 100 passes             ns/row (p50)   allocs/row
# File    BEFORE                        743.2         15.75
# File    AFTER                         416.8  1.78x  10.51
# Folder  BEFORE                        704.8         14.08
# Folder  AFTER                         620.4  1.14x  10.08
# gate: (name, path_string, segments) byte-identical + error parity,
#       realistic corpus + adversarial (traversal, //, NFD, empties)
```

## [2] WebDAV drive-selector — grants join/request → per-user cache — ~550x

`lookup_drive_selector` (every native `/webdav/<selector>/…` request,
all verbs, MOVE/COPY twice) ran `list_readable_by`: a
role_grants ⋈ drives ⋈ folders join with inline transitive-group
expansion, GROUP BY + MIN(role) + ORDER BY — per request, uncached. The
same join also ran per request in search, trash listing and the
`GET /api/drives` picker.

Now `DrivePgRepository` carries a `readable_cache`
(user → `Arc<Vec<DriveWithRootName>>`, 30 s TTL, `try_get_with`
single-flight, errors never cached) mirroring the CHROOT-CACHE
precedent. Every mutation that can change a user's drive list
invalidates explicitly: personal/shared drive creation, deletion, policy
edits (repo), membership set/remove (`DriveManagementService`, per-User
subject or full clear for Group subjects), and group-membership changes
(`SubjectGroupService` invalidates per affected transitive user). The
residual staleness sources (root-folder rename; grant writes that can't
reach this cache) stay bounded by the same 30 s TTL the sibling caches
accept; permission *enforcement* is unaffected (the ACL engine
re-checks per operation with its own invalidation).

```
cargo run --release --features bench --example bench_drive_selector
# pool=20, window=4s, 3 drives/user       req/s    p50 µs    p99 µs   queries
# conc=8   BEFORE (join/request)         17,098    441.23   1143.85    68,394
# conc=8   AFTER  (readable_cache)    2,371,541      0.80      8.61         0
# conc=64  BEFORE                        21,462   2818.27   5440.99    85,850
# conc=64  AFTER                      1,506,230      1.71     17.08         0
# gate: (id, name) sequences identical — BEFORE == cold == warm
```

## [3] CalDAV `from_ical` — 8 full parses per VEVENT → 1 — 7.1x

`CalendarEvent::from_ical` funnelled each of its 8 property lookups
(SUMMARY, DTSTART, DTEND, DESCRIPTION, LOCATION, RRULE, UID,
RECURRENCE-ID) through an extractor that re-ran the complete
`IcalParser` — line unfolding + full component-tree build — over the
whole body. Every CalDAV PUT paid 8 parses per VEVENT; a master+M-
exceptions PUT paid `8·(M+1)`; an N-event import `8·N`.
`update_ical_data` had the same shape (7 lookups). Now both parse ONCE
and read properties from the parsed component; value-only lookups also
skip the parameter-map build, and `split_vevents` stopped uppercasing
every line into a fresh String (allocation-free CI prefix test).

```
cargo run --release --features bench --example bench_caldav_parse
# 200 realistic ~1.3 KiB VEVENTs (params, folding, VALARM, exceptions)
# [1] from_ical µs/event         83.81 → 11.76 (excl. body clone)   7.1x
# [2] 50-event import body µs     4412.5 → 1002.3                   4.4x
# gates: parsed fields byte-identical (incl. all-day, exceptions,
#        mixed-case tags, LF-only bodies), error parity, wrapped
#        per-row ical_data identical
```

## [4] CalDAV read side — per-event copies removed — 1.3-1.4x

`extract_vevent_chunk` (every REPORT / collection-GET, per event)
allocated a full `to_ascii_uppercase()` copy of the stored body just to
locate two tags — now a memchr fast path (stored bodies carry uppercase
tags) with an allocation-free case-insensitive scan fallback.
`group_events_by_uid` cloned every event's UID String into its map —
now borrowed keys. `generate_calendar_events_response` also stopped
cloning the requested-props Vec per REPORT.

```
# [3] extract_vevent_chunk ns/event      297 → 215    1.4x  (stable
#     across 3 isolated re-runs; one battery pass showed 0.9x noise)
# [4] group_events_by_uid µs/5k events  1221.0 → 951.1   1.3x
# gates: identical chunk slices (incl. mixed-case, missing-terminator,
#        malformed bodies), identical grouping shape
```

## [5] PROPFIND XML emit — single-pass + stack-rendered fields — 1.22x

For EVERY file/folder row of every PROPFIND page the writers paid a
`partition` into two throwaway `Vec<&QualifiedName>`s (+ a third for the
404 list) even though the requested-props writer already skips unknown
names itself, plus `to_rfc3339()` + `to_rfc2822()` (chrono's format-spec
interpreter + a heap String each), `size.to_string()` and a
`format!("\"{etag}\"")`. Now: one pass computing only the
usually-empty 404 list, and `common::fmt` stack renderers — RFC 3339 /
RFC 2822 / integers written into stack buffers, byte-identical to chrono
(sweep-tested across 60 years; out-of-range values keep the chrono
fallback). The same renderers replaced the per-row date/etag/size
formatting in the NextCloud PROPFIND emitters.

The first version of `rfc2822_utc` zero-padded the day; chrono does not
(`Thu, 1 Jan`). **The byte-identity gate caught it** and the padded
version never shipped — exactly the failure mode these gates exist for.

```
cargo run --release --features bench --example bench_propfind_xml
# 1000 files + 100 folders/page, 200 passes    µs/page   allocs/row
# named-prop (sync set)  BEFORE                 1534.9        17.91
#                        AFTER                  1253.1  1.22x  12.00
# allprop (+quota)       BEFORE                 1072.1         9.67
#                        AFTER                   895.1  1.20x   4.58
# gate: multistatus XML byte-identical (named-prop incl. unknown + dead
#       props, allprop with quota; epoch/padded-day/2099 timestamps)
```

## [6] Grant-listing hydration — K point SELECTs → one `= ANY` — ~13x

After `list_incoming_grants`, the CalDAV calendar discovery, CardDAV
book discovery and playlist listing each hydrated their K accessible
resources with K SERIAL point SELECTs, awaited one by one, on every
client sync poll / dashboard load. New batch methods
(`find_calendars_by_ids` / `get_address_books_by_ids` /
`find_playlists_by_ids`) collapse each listing to one round-trip;
missing rows still drop out silently (deleted/trashed race carve-out
preserved).

```
cargo run --release --features bench --example bench_n1_hydration
# K=15 resources, 200 passes            ms/listing p50   queries
# calendars      BEFORE → AFTER          4.411 → 0.338   15 → 1   13.0x
# address books  BEFORE → AFTER          4.365 → 0.325   15 → 1   13.4x
# playlists      BEFORE → AFTER          4.378 → 0.342   15 → 1   12.8x
# gate: identical id sets loop vs batch (+ ghost-id drop-out parity)
```

## [7] user-flags cache — get→insert → single-flight — 32 → 1 queries

`get_user_flags` backs the auth middleware's per-request role/active
guard. Its cache was get→insert: on every 30 s TTL expiry, every
in-flight request of that user fired the SELECT concurrently (the same
herd shape ROUND3 fixed for basic-auth, minus the Argon2 cost). Now
`moka::future` + `try_get_with`: concurrent misses coalesce, errors are
never cached, eager invalidation on role/active changes unchanged.

```
# cold-cache herd of 32 concurrent callers
# BEFORE (get→insert)    4.72 ms   32 queries
# AFTER  (try_get_with)  0.57 ms    1 query
# gate: identical flags from every caller
```

## [8] Azure download path — whole-blob buffering → streaming — 87-254x

`AzureBlobBackend::get_blob_stream` / `get_blob_range_stream` drained
the ENTIRE blob (or range) into one `Vec<u8>` before yielding a single
mega-chunk: whole-blob RAM residency per reader, TTFB = full download
time, and with `read_prefetch() = 8` the CDC reassembly path could hold
8 entire chunk-blobs at once. Now the SDK's page/body streams forward
directly (first page still awaited eagerly so a missing blob surfaces
as the same up-front NotFound). `AzureStorageConfig` gained
`endpoint_url` (`OXICLOUD_AZURE_ENDPOINT_URL`) mirroring S3's override —
it powers the bench stub and enables Azurite for local dev.

```
cargo run --release --features bench --example bench_azure_stream
# 256 MiB blob, local Azure-GET stub    TTFB ms   wall ms   peak heap MiB
# full  BEFORE (collect-then-yield)       349.3     465.3       479.8
# full  AFTER  (streamed)                   4.0     308.5         1.9   87x / 254x
# tail-128 MiB range BEFORE               165.5     225.3       240.7
# tail-128 MiB range AFTER                  1.3     147.3         1.9   125x / 127x
# gate: BLAKE3(BEFORE) == BLAKE3(AFTER) == source, full + range
```

## [9] Face indexing — unbounded per-image spawn → semaphore — 6.7x RAM

`FaceIndexingService::spawn_index` fired one `tokio::spawn` per
uploaded/copied image with no ceiling; each task reads the full blob
and decodes it before inference, so a bulk upload of N photos held up
to N decoded images in flight. Now an `Arc<Semaphore>` sized to the
effective core count (`OXICLOUD_FACES_INDEX_CONCURRENCY` override),
permit acquired BEFORE the blob read — the exact
`ThumbnailService::decode_semaphore` invariant ("peak memory =
permits × image size"). Pattern bench (the real service needs
Postgres + an ONNX model): task body = full-file read + JPEG/PNG decode
on the `bench_support` corpus, spawn/permit shape copied verbatim.

```
cargo run --release --features bench --example bench_faces_bound
# 48 × 11.1 MiB images, permits=4      wall ms   peak live heap MiB
# BEFORE (unbounded)                     870.5      1175.4
# AFTER  (semaphore 4)                   755.1       176.0   6.7x lower
# gate: all 48 images decoded identically in both modes
```

## Follow-ups worth a future round (confirmed real, not gated here)

- Grouped/swimlane files view is still unvirtualized (10k-row DOM) —
  frontend, carried over from ROUND3.
- CalDAV REPORT / collection-GET still buffer the full multistatus /
  VCALENDAR in RAM (`caldav_handler.rs`) — the WebDAV surface streams,
  the CalDAV one doesn't yet; pairs with paged event loading.
- Auth middleware per-request `user_id.to_string()` span records and
  owned `CurrentUser` strings (`interfaces/middleware/auth.rs`) —
  small but ubiquitous.
- Search suggest clones each entity before DTO conversion
  (`search_service.rs:525/539`).
