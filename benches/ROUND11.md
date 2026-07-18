# Round 11 — StoragePath re-representation, classifier fusion, memoized static bodies, query-shape pack, SPA fine-grained stars

Benchmark-gated, same rule as ROUND2-10: every change ships with a
BEFORE/AFTER benchmark and an equivalence/safety gate; an AFTER that doesn't
beat its BEFORE gets rolled back or redesigned. Three candidates went
through exactly that loop this round (§Rejected): the moka `and_upsert_with`
rate-limiter rewrite measured SLOWER than the two-op shape it was meant to
replace and was redesigned as a lock-free `get`+`insert`; the GET/HEAD
`Last-Modified` stack-render port measured neutral-to-worse (the chrono
String is already the terminal allocation) and was dropped; the first
search-page `drain(range)` model lost to `to_vec` on wall and was reshaped
as `into_iter().skip().take()`.

Measured on 4 cores / 15 GiB, local PostgreSQL 16 (fsync off), release
profile; frontend on Node 22 / vitest 4 (jsdom). Reproduce any row with the
command in its section.

## Summary

(numbers filled from the final runs below)

| # | change | key metric | before → after |
|--:|---|---|---|
| 1 | REST download: dead `FileDto` clone → capture mime/size + move | allocs per download | 7 → 0 (dead clone removed) |
| 2 | `StoragePath` → single canonical joined `String` (segments derived lazily; entity `path_string` duplicate field removed) | allocs / wall per 500-row page | TBD |
| 3 | Display classifier fusion (`classify_display`, stack-lowered ext shared by the three trees) | ns / allocs per listing row | TBD |
| 4 | `/status.php` → `OnceLock<Bytes>` | ns / allocs per poll | TBD |
| 5 | `/openapi.json` → `OnceLock<Bytes>` (was: rebuild 171 KiB spec per request) | ns per request | TBD |
| 6 | NC upload-session PROPFIND: `write!` + capacity + stack dates | ns / allocs per 256-chunk PROPFIND | TBD |
| 7 | CSRF header token borrow compare | ns / allocs per state-changing request | TBD |
| 8 | Thumbnail/preview ETag: `as_str` push (Debug-identical bytes) | ns per thumbnail request | TBD |
| 9 | Recent-handler id: stack `encode_lower` | ns / allocs per call | TBD |
| 10 | 4xx body: borrowed serialize + `ErrorKind::as_str` + `not_found` clone kill | ns / allocs per 404 | TBD |
| 11 | vCard emit: `write!` (+ borrowed address fields) | ns / allocs per vCard | TBD |
| 12 | Search page: `into_iter().skip().take()` move | allocs per 50-item page | TBD |
| 13 | Content-hit verify: parse-once pairs | ns per 100-hit page | TBD |
| 14 | Group last-user check: HashSet probe | ns per 500×500 check | TBD |
| 15 | Retry op-label: lazy closure | ns / allocs per blob op | TBD |
| 16 | `encrypt_bytes`: in-place detached (write side now mirrors the in-place read) | ns / allocs per 256 KiB chunk | TBD |
| 17 | Encrypted `collect_stream`: chunk-sized reserve | allocs per 1 MiB read | TBD |
| 18 | Recluster cosine: precomputed norms (bit-identical) | ns per 200-face pass | TBD |
| 19 | `CalendarEventDto`: `into_parts` move (11 KiB `ical_data` copy gone) | ns / allocs per event | TBD |
| 20 | RateLimiter: lock-free `get` + `insert` | ns / allocs per limited request | TBD |
| Q1 | Deferred upload registration: 3 round-trips → 1 CTE insert | ms per uploaded file | TBD |
| Q2 | Calendar/AddressBook/Playlist authz `direct_grant_cache` | ms per DAV check | TBD |
| Q3 | `expand_user`: `tokio::join!` the 2 independent queries | ms per cold expansion | TBD |
| Q4 | Geo clusters: `min(file_id)::text` (cast per cluster, not per row) | ms per viewport | TBD |
| Q5 | Recluster persistence: per-face UPDATEs → one UNNEST batch | ms per 200-face apply | TBD |
| L1 | Log writer: `tracing-appender` non_blocking (lossy=false) | p99 emit µs under contention | TBD |
| S1 | SPA `ResourceList.selectedEntries`: O(N)×2 per toggle → O(k·log k), hosts consume the snippet param | comparisons per toggle | 2N → k |
| S2 | SPA Recent: star reads `favoriteIds` prop, mapper no longer set-dependent | rows re-mapped per star click | N → 0 |
| S3 | SPA admin `timeAgo`: cached `Intl.DateTimeFormat` | constructions per 1000 formats | ≤1 (was 1000) |

Also shipped without a dedicated row: `already_exists` clone kill (same
shape as `not_found`), CardDAV `getlastmodified` stack render (per-contact
REPORT path, ROUND10-§13 helper + fallback), NC capabilities poll logs
demoted to `debug` (INFO forced a locked-stdout write per client poll),
trash `to_dto` `into_parts` move + interned display fields (trash listing
+ path-resolver rows now share the ROUND9 interning).

## Rejected / reworked this round (the discipline working)

- **RateLimiter `entry().and_upsert_with`**: 1 846.5 → 1 862.1 ns and
  8.0 → 9.1 allocs/op — moka's compute-entry machinery costs more than the
  two-op shape it replaced. Redesigned as lock-free `get` (borrows the
  key, no alloc) + `insert`; identical counter sequence gated.
- **GET/HEAD `Last-Modified` stack-render port**: chrono's `to_rfc2822()`
  String IS the terminal allocation the header needs (44.3 ns incl. the
  alloc vs 46.5 ns for stack render + the same alloc). Only body-emit
  sites (where `write!` lands in an existing buffer) benefit — those were
  ported (§6, CardDAV); the header sites were left on chrono.
- **Search page `drain(range).collect()`**: −300 allocs but slower on
  wall than `to_vec` in the first model (tail memmove). Reshaped as
  `into_iter().skip().take().collect()` — moves the page, drops the rest,
  no tail shift.

## Deferred / flagged (not shipped this round)

- **NC preview 304 still runs `get_file`** (`preview_handler.rs`): the
  object-id → file fetch on the revalidation path is only needed for
  existence semantics (a deleted file must 404, not 304). Dropping it is
  a behavior decision — same class as the standing CalDAV
  authz-before-fetch reorder — flagged for maintainer sign-off.
- **`CachedBlobBackend`'s `Mutex<LruCache>` index** serializes every
  cached read; a moka byte-weigher migration (the file-content-cache
  pattern) is the natural fix but touches eviction-unlink semantics —
  deserves its own round with a concurrency bench.
- **Capture-metadata extraction reads each media file 2-3×**
  (`media_metadata_service.rs`: kamadak full read + nom-exif path re-read
  + track fallback re-read). Feeding nom-exif from the in-memory buffer
  needs its `MediaSource` API verified on the pinned version.
- **`CachedBlobBackend::local_blob_path` sync `stat`** (ROUND10 flag
  stands): needs an async port variant.
- **Azure SDK 0.21 stack** drags duplicate dependency trees (h2 0.3+0.4,
  two hashbrown generations, base64 0.13) into the binary; an SDK bump is
  a dedicated migration, not a perf tweak.
- **`AudioMetadataRepository::list_by_{artist,album,genre}`** are dead
  code (never called) with seq-scan `ILIKE` shapes — flag for deletion
  rather than indexing.
- **`CachedBlobBackend::put_blob` cache population** silently fails for
  S3/Azure whole-file puts (the inner backend deletes the source before
  the cache copy runs) — correctness note for maintainers, not perf.
- **CalDAV authz-before-fetch reorder** — ROUND9/10 flag stands.

## Environment / methodology

- `cargo run --release --features bench --example bench_round11_micro`
  (pure CPU, counting allocator, BEFORE replicas vs shipped code).
- `cargo run --release --features bench --example bench_round11_queries`
  (Postgres; seeds + sweeps its own fixtures).
- `BENCH_LOG_ARM=sync|nonblocking [BENCH_LOG_WRITER=slow] cargo run
  --release --features bench --example bench_log_writer >/dev/null`.
- `cd frontend && npx vitest run src/lib/components/round11.bench.test.ts`.
- Regression guards from earlier rounds re-run after the StoragePath /
  classifier changes: `bench_row_path` (round-4 gates) and `bench_dto_map`
  (round-3 gates).
