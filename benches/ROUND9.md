# Round 9 — decorator PUT reactivation, session/search/dedup alloc purges, PROPFIND `join!`, folder-level cascade

Benchmark-gated, same rule as ROUND2-8: every change ships with a
BEFORE/AFTER benchmark and equivalence/safety gates; an AFTER that doesn't
beat its BEFORE gets rolled back. The two decide-by-bench items this round
(PROPFIND enrichment `join!`, folder binary-UUID) were adopted only after
their gates passed; the authz change carries hard safety gates plus a new
direct-grant-sibling isolation gate and was validated against the full
authz-relevant unit suite.

Measured on 4 cores / 15 GiB, local PostgreSQL 16 (fsync off), release
profile; frontend on Node 22 / vitest 4 (jsdom). Reproduce any row with the
command in its section.

## Summary

| # | change | key metric | before → after |
|--:|---|---|---|
| 1 | Blob decorators forward `put_blob_from_bytes_unsynced` | HEAD probes / wall, 500-chunk upload @10 ms RTT | 500 → 0 probes; full stack 1571 → 812 ms (**1.9x**) |
| 2 | NC PROPFIND page enrichment triple → `tokio::join!` | p50 ms/page (500 children) | local 2.28 → 1.10 (**2.07x**); @5 ms RTT 22.1 → 7.7 (**2.86x**) |
| 3 | Search enrich consume+carry (`Arc<str>` result fields) | enrich_file ns/row · allocs/row | 456 → 223 (**2.0x**) · 11.6 → 2.2; NC conversion 15.4 → 7.0 allocs/row |
| 4 | NC session end-to-end `Arc` (extractor/chroot/build) | allocs per authenticated NC request | extractor 8→0, chroot hit 4→0, build 11→6 (**~17 fewer/req**) |
| 5 | Storage micro-pack (create_new · manifest Arc · single-flight · hex) | see §5 | fresh chunk writes **2.1x**; 4097→0 allocs/read; herd 64→1 loads; 18→1 allocs/digest |
| 6 | OCS capabilities memoized (`OnceLock<Bytes>`) | 50k polls wall · allocs/poll | 269.6 → 1.1 ms (**237x**) · 102 → 0 |
| 7 | `Drive::is_empty` COUNT(*) → `EXISTS` | ms/call, 100k-file drive | 13.6 → 0.40 (**34.4x**) |
| 8 | favorites/recents row-map move (ROUND7 port) | allocs/row | 12.00 → 9.25 (**−2.75/row**) |
| 9 | Folder rows: binary UUID decode (ROUND6 port) | 500-row page mean | 1.06–1.10 → 1.03–1.04 ms (**1.03–1.07x**, first run a wash — see §9) |
| 10 | Folder-level cascade decision (authz, ROUND8 deferred) | cold first view µs/thumb (100-photo album) | 592 → 418 (**1.42x**); warm 1.33 µs unchanged |
| 11 | SPA: `resolveLabel` O(C)→O(1) index | 50 frames × 30 rows @ 5k contacts | 11.0 → 0.8 ms (**13.9x**); comparisons rows×C → C |
| 12 | SPA: selection-prune guard + `matchMedia` hoist | per-page Set builds / matchMedia calls | 100 → 0 · P → 1 |

## [1] Blob decorators — the trait-default fallthrough was re-adding HEAD-before-PUT

ROUND3 §8 made chunk writes skip the remote exists-probe by introducing
`put_blob_from_bytes_unsynced` (content-addressed keys make re-PUTs
overwrite-safe). But `RetryBlobBackend` and `CachedBlobBackend` never
overrode it, so the **trait default** routed every decorated `_unsynced`
call back through the probing `put_blob_from_bytes` — silently reinstating
HEAD+PUT per chunk on every remote deployment with retry or cache enabled
(the recommended object-store setup). `EncryptedBlobBackend` and
`MigrationBlobBackend` already forwarded correctly.

Both decorators now forward `put_blob_from_bytes_unsynced` and `sync_blobs`
to their inner backend (Retry wraps the former in its retry loop; the
durability sweep is deliberately NOT retried — a failed fsync must surface,
not be re-issued after the kernel may have dropped the dirty pages).
`CachedBlobBackend` keeps its local write-through population on the
unsynced path (shared `cache_bytes_write_through` helper, no eviction sweep
— matching the historical write-path behavior) so post-upload readers
(thumbnail/EXIF/face hooks) still hit the cache.

```
cargo run --release --features bench --example bench_s3_put
# 500 x 256 KiB chunk PUTs at concurrency 8, 10 ms/request stub
# [1] raw backend    BEFORE 1519 ms (500 HEADs) → AFTER 765 ms (0)   2.0x
# [3] retry(s3)      BEFORE 1524 ms (500 HEADs) → AFTER 766 ms (0)   2.0x
#     cache(s3)      BEFORE 1535 ms (500 HEADs) → AFTER 803 ms (0)   1.9x
#     cache(enc(retry(s3)))  1571 ms (500)      →       812 ms (0)   1.9x
# gates: BEFORE probes == chunks, AFTER probes == 0, cache write-through
#        populated on BOTH routes (2×chunks files present)
```

## [2] NC PROPFIND page enrichment — 3 serial round-trips → `tokio::join!`

Every Depth:1 PROPFIND page enriches its ≤500 children with three
INDEPENDENT batched reads (favorites `= ANY`, oc:fileid `= ANY`, dead
props `= ANY`), previously awaited in sequence. This is the round-7
deferred "serial pairs" item, and the one pair the round-7 notes ranked
worth gating (3 round-trips, per page, on the hottest sync path).

Decide-by-bench with injected per-round-trip latency (0/0.25/1/5 ms),
because ROUND6 showed concurrency can LOSE on local-socket PG (the authz
`try_join_all` rejection). It doesn't here — these are three fat batched
queries whose **server-side execution** parallelizes across PG backends,
so even the local-socket floor wins, not just the RTT overlap:

```
cargo run --release --features bench --example bench_nc_enrich_join
# children=500, passes=100, p50 ms/page      serial    join!   ratio
#   0 µs injected                             2.275    1.097   2.07x
#   250 µs                                    6.273    2.481   2.53x
#   1000 µs                                   9.163    3.441   2.66x
#   5000 µs                                  22.050    7.709   2.86x
# gate: identical favorite sets / id maps / dead-prop rows; adoption
#       required no local-socket regression — it's a 2x win even there
```

Contrast with ROUND6 §8 (rejected): that fan-out issued ~200 single-row
authz checks through the engine's cache layers; this overlaps exactly 3
page-batched queries. Both files' and folders' page loops adopted it.

## [3] Search enrichment — borrow+clone+reclassify → consume+carry

`enrich_file` took `&FileDto`, cloned every owned String out of it, and
RE-RAN the three display classifiers whose results the DTO already carried
interned (`Arc<str>`, computed once in `FileDto::from`); the recursive
branch maps the ENTIRE pre-pagination match set. The NC REPORT conversion
(`file_dto_from_search`) then re-ran all three classifiers a SECOND time
per emitted row. `SearchFileResultDto.{mime_type,icon_class,
icon_special_class,category}` are now `Arc<str>` (`#[schema(value_type =
String)]` keeps the OpenAPI shape; JSON output byte-identical), both
enrichers consume their DTO, the intermediate `Vec<FileDto>`/`Vec<FolderDto>`
materializations are fused away, suggest reuses the interned fields, and
the NC conversion carries them (refcount bumps). The search-cache byte
weigher keeps counting `.len()` per row — now an over-count of shared
bytes, i.e. the conservative direction.

```
cargo run --release --features bench --example bench_search_enrich
# rows=10000 passes=50 (p50 ns/row; allocs from pass 0)
# [1] enrich_file   BEFORE 455.8 ns / 11.60 allocs → AFTER 222.7 / 2.20
# [2] enrich_folder BEFORE 116.2 ns /  5.00 allocs → AFTER 127.6 / 1.00
#     (folder wall flat: the AFTER window absorbs the input drop the
#      BEFORE arm defers outside its timing; the alloc gate is the win)
# [3] NC conversion BEFORE 2.700 ms / 15.40 allocs → AFTER 1.524 / 7.00
# gates: 500 files + 500 folders field-identical; NC conversion
#        field-identical vs a fresh classifier run
```

## [4] NC session — deep-clone per request → `Arc` end-to-end

Every authenticated NC request paid: the extractor's `(**arc).clone()` — a
DEEP clone of `NcSession` (~8-9 String allocs) despite its doc claiming
"one Arc increment"; a chroot-cache hit cloning the stored `FolderDto` by
value (~5 allocs, moka `get` clones `V`); and a session build that cloned
`CurrentUser` for the extension, cloned `raw_username`, and `to_string`ed
the span value. Now: `NC_CHROOT_CACHE` stores `Arc<FolderDto>`,
`NcSession.user` is the same `Arc<CurrentUser>` the extension holds,
`raw_username` moves, the span renders lazily (`field::display`, the
ROUND5 §7 pattern the NC path had missed), and handlers extract
`SharedNcSession` — an `Arc` handle that derefs to `NcSession`, so the 64
field-access sites are untouched.

```
cargo run --release --features bench --example bench_nc_session
# 100k iterations                          wall ms   allocs/op
# [1] extractor  BEFORE deep clone           17.0      8.000
#                AFTER  SharedNcSession       4.2      0.000   (4.0x)
# [2] chroot hit BEFORE FolderDto value      21.3      4.000
#                AFTER  Arc<FolderDto>       11.7      0.000   (1.8x)
# [3] build      BEFORE clone×2 + span       17.6     11.000
#                AFTER  shared Arc           11.8      6.000   (1.5x)
# gate: every field handlers consume identical (incl. the URL-user check)
```

## [5] Storage micro-pack

Four independent A/Bs in one harness (`bench_storage_micro`, no Postgres):

- **(a) Local chunk write** — `try_exists` (stat) + `File::create` →
  one atomic `create_new` open; `AlreadyExists` IS the idempotent skip.
  20k × 4 KiB fresh writes 2707 → 1286 ms (**2.1x**); re-put skips 1.08x.
- **(b) CDC read prep** — `stream_chunks` took `Vec<String>`, forcing
  every read to deep-clone the cached manifest's whole hash list before
  the first byte; now it takes the manifest `Arc` and indexes. A
  4096-chunk manifest × 200 reads: 819 400 → 0 allocs, 49.4 → 0.16 ms.
  The Range path selects by index too — a `bytes=0-` probe of an N-chunk
  video no longer clones N hashes.
- **(c) Manifest miss herd** — `manifest_cached` used get→insert; K
  concurrent cold readers each ran the SELECT. Now fast-get +
  `try_get_with` (sentinel miss error keeps the positive-only contract —
  moka never caches loader errors, so legacy blobs and DB failures stay
  uncached). Herd of 64: 64 → 1 loads.
- **(d) Chunk `Content-MD5` hex** — the last `format!("{b:02x}")`-per-byte
  straggler (ROUND6 §7 shipped `hex_lower`); 18 → 1 allocs/digest, 10x.

```
cargo run --release --features bench --example bench_storage_micro
```

## [6] OCS capabilities — rebuilt per poll → memoized bytes

`/ocs/v{1,2}.php/cloud/capabilities` is process-invariant (pure config),
yet every poll re-built the ~40-node `json!` tree, re-read
`OXICLOUD_BASE_URL` from the **environment**, ran three `format!`s and
re-serialized. Both versions now serialize once into
`OnceLock<[Bytes; 2]>`; a poll is a refcount bump. The payload builder
takes its three config inputs directly (testable without `AppState`).

```
cargo run --release --features bench --example bench_capabilities_static
# 50k polls   BEFORE 269.6 ms / 102 allocs/poll → AFTER 1.1 ms / 0  (237x)
# gate: served bytes byte-identical for v1 and v2
```

## [7] `Drive::is_empty` — full-drive COUNT(*) sum → `EXISTS OR EXISTS`

The deletion precheck only needs a boolean, but aggregated every live
folder + file in the drive. `EXISTS` stops at the first row.

```
cargo run --release --features bench --example bench_drive_is_empty
# populated (100k files)  13.615 → 0.396 ms  (34.4x)
# empty                    0.219 → 0.166 ms  (1.3x)
# gate: identical booleans on both data shapes
```

## [8] favorites/recents row-map — the ROUND7 move that never got ported

ROUND7 §3 removed the per-row `name` clone in `/folders/{id}/resources`;
the same mapping in `/api/favorites/resources` and `/api/recent/resources`
still cloned `path` + `name` + `blob_hash` per row (and `folder_handler`
kept one `blob_hash` clone). All moved now — display classes computed
before `name` moves, `path`/`blob_hash` moved instead of cloned.

```
cargo run --release --features bench --example bench_resource_row_map
# [2] favorites/recents shape, rows=500
# BEFORE (clone) 12.004 allocs/row → AFTER (move) 9.254  (−2.75/row)
# gate: (name, path, content_hash, icon_class, category) identical per row
```

## [9] Folder rows — binary UUID decode (the ROUND6 §10 port)

ROUND6 adopted binary-UUID decode for file listing rows (1.17x) and queued
"other repos with the same shape"; `FolderDbRepository` never got it. All
folder-row queries (`list_folders_batch` — every Depth:1 PROPFIND subfolder
page — `get_folder`, descendants, search, suggest, and the write-path
RETURNINGs, which share `row_to_folder`) now decode `id`/`parent_id` as
binary `Uuid` (16 B vs 36 B on the wire, no server cast) and render once
app-side. Param casts (`$3::text IS NULL`), enum casts and the ltree
`path::text` renders are untouched.

**Honest verdict:** weaker than the file side. Four interleaved runs:
1.00x (wash), 1.05x, 1.03x, and 1.07x at 1000 rows — folder rows are
thinner than file rows, so the two casts are a smaller fraction of the
page. Adopted on the consistent small win + growth with page size + the
wire-bytes reduction; the first-run wash is inside the noise band.

```
cargo run --release --features bench --example bench_folder_uuid_decode
# rows/page=500 passes=400 (interleaved)   mean      p50      p95
# A ::text (before)                        1.061    1.039    1.310
# B binary (after)                         1.027    1.012    1.269   1.03x
# rows/page=1000: 1.758 → 1.639 mean                              1.07x
# gate: identical (id, name, path, parent_id) tuples
```

## [10] Authz — folder-level cascade decision (the ROUND8 deferred item)

ROUND8 memoised the per-file cascade decision, fixing revalidation; a
shared N-photo album's **cold first view** still ran N near-identical
ltree ancestor queries. The file decision now decomposes into exactly the
two branches of the historical UNION: parent point-read (new
`file_parent_cache`, 30 s TTL — grant writes don't alter parentage; moves
are the same TTL-healed indirect path as before) → the FOLDER cascade
decision (one ltree query per folder, shared by every sibling via the
existing `cascade_grant_cache`, recursing into the Folder arm) → a
direct-file-grant point lookup only when the folder half denies. The old
UNION query is deleted; no decision changes, including the parentless
edge (`folder_id IS NOT NULL` guard ≡ direct-only fallback).

Safety gates (hard asserts): recipient allowed on every file, outsider
denied, `clear_role` revoke denies IMMEDIATELY (the flush covers file and
folder decisions — same cache), and NEW: a caller holding only a direct
grant on one file is allowed that file and denied its siblings — proving
the folder-level decomposition neither shadows direct grants nor leaks a
file decision across siblings.

```
cargo run --release --features bench --example bench_thumbnail_cascade_cache
# thumbs=100 (folder-grant recipient, no drive membership)
# ROUND8 cold (union/file)      59.19 ms    591.91 µs/thumb
# AFTER  cold (first view)      41.77 ms    417.73 µs/thumb   (1.42x)
# AFTER  warm (revalidation)     0.13 ms      1.33 µs/thumb   (unchanged)
```

The first view is now bounded by the per-file parent PK reads (cheap, but
still N point queries) + 1 ltree query — batching the parent resolution
per page would need a wider API change; noted for a future round.

## [11] SPA — `resolveLabel` linear directory scan → id-keyed index

`resolveLabel`/`resolveRecipient` ran `contactCache.find(...)` — a linear
scan over the whole system address book — once per rendered grant row /
lane header on `/shared`, re-rendering on every page and role change:
O(rows × directory). Now a `Map<id, Contact>` built once per cache
identity (exactly like the existing `groupCache`).

```
cd frontend && npx vitest run src/lib/api/endpoints/recipients.bench.test.ts --disable-console-intercept
# 50 frames × 30 rows @ C=5000: before 11.0 ms, after 0.8 ms (13.9x)
# gates: labels identical (present + absent ids); comparisons rows×C → C
```

## [12] SPA — selection-prune guard + photos `matchMedia` hoist

- `ResourceList`'s prune `$effect` built an O(N) id `Set` on every
  infinite-scroll page even with nothing selected; guarded with
  `selected.size === 0` (reactive, so it re-arms when a selection
  appears). 100-page drain: 100 → 0 Set builds; pruned result identical
  when a selection exists.
- The photos timeline derive called `window.matchMedia(...)` per
  recompute (every 60-photo page); hoisted to state fed by one
  MediaQueryList `change` listener. P recomputes: P → 1 calls, identical
  booleans, crossings propagate.

```
cd frontend && npx vitest run src/lib/components/listDerives.bench.test.ts --disable-console-intercept
```

## Deferred / flagged (not shipped this round)

- **CalDAV authz-before-fetch reorder** (`calendar_service::get_event` /
  `list_events` / by-uid fetch the calendar row before the authz check
  only to read `.is_public`; running the already-required authz first and
  fetching only on denial saves one SELECT per authorized private-calendar
  read). Behavior-preserving (the OR commutes) but it reorders an authz
  check relative to a data fetch — flagged for maintainer sign-off per the
  authz-change convention, with the bench sketch in this round's notes.
- **Per-page batched parent resolution** for §10 — would cut the cold
  first view's N parent PK reads to one `= ANY` per page; needs a wider
  engine API (batch check) — future round.
- **`batch_operations` `Arc<str>` → `Option<&str>` widening** (ROUND7
  deferred) — re-audited: 1 small alloc/item vs a per-item DB roundtrip;
  still not worth the 2-trait/7-site churn alone. Standing verdict.
- **JWT-claims `Arc<str>`** (ROUND6 deferred) — still open; touches
  serde `rc` on `TokenClaims` + dozens of read sites. The 2 allocs/request
  remain the cheapest known win on the /api path for a future round.

## Correctness-adjacent (surfaced by the round-9 hunt — not perf)

- `trash_service.rs` restore matches error text
  (`format!("{}", e).contains("not found")`) instead of
  `e.kind == ErrorKind::NotFound` — fragile to rewording; flagged.
- The round-7 flags remain open: `fetchFolderListing` seeds empty
  `favoriteIds`/`sharedIds`; the search page still lacks a stale-response
  guard.
