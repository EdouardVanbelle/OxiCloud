# Round 11 — StoragePath re-representation, classifier fusion, memoized static bodies, query-shape pack, SPA fine-grained stars

Benchmark-gated, same rule as ROUND2-10: every change ships with a
BEFORE/AFTER benchmark and an equivalence/safety gate; an AFTER that doesn't
beat its BEFORE gets rolled back or redesigned. Four candidates went through
exactly that loop this round (§Rejected below): the moka `and_upsert_with`
rate-limiter rewrite, the GET/HEAD `Last-Modified` stack-render port, the
`min(uuid)` geo-cluster cast (PostgreSQL has no such aggregate — the gate
caught it before it could ship broken), and the `tracing-appender`
non-blocking log writer (slower than sync on fast sinks, tail-loss risk on
slow ones).

Measured on 4 cores / 15 GiB, local PostgreSQL 16 (fsync off), release
profile; frontend on Node 22 / vitest 4 (jsdom). Reproduce any row with the
command in its section (`benches/ROUND11.md` §Environment).

## Summary

| # | change | key metric | before → after |
|--:|---|---|---|
| 1 | REST download: dead `FileDto` clone → capture mime/size + move | ns / allocs per download | 295.3 → 20.7 ns · 7 → 0 allocs |
| 2 | `StoragePath` → single canonical joined `String`; `File`/`Folder` drop the duplicate `path_string` field | 500-row page (depth 4) | 121.0 → 80.3 µs (**1.51x**) · 4 000 → 1 000 allocs |
| 3 | Display classifier fusion (`classify_display`, one stack-lowered ext for the three trees) | 13-row mixed corpus | 4 390 → 2 074 ns (**2.12x**) · 21 → 0 allocs |
| 4 | `/status.php` → `OnceLock<Bytes>` | per NC client poll | 836 → 28.8 ns (**29x**) · 14 → 0 allocs |
| 5 | `/openapi.json` → `OnceLock<Bytes>` (was rebuilding the 171 KiB spec per request) | per request | 2.77 ms → 18.5 ns · 12 474 → 0 allocs |
| 6 | NC upload-session PROPFIND: `write!` + pre-sized body + stack dates | 256-chunk session | 203.7 → 75.4 µs (**2.70x**) · 2 582 → 772 allocs |
| 7 | CSRF token: borrow-only compare (+ borrowed cookie extraction) | per state-changing request | 55.2 → 2.2 ns · 1 → 0 allocs |
| 8 | Thumbnail/preview ETag via `as_str` (Debug-identical bytes — cached client ETags stay valid) | per thumbnail request | 150.5 → 87.7 ns · 3 → 2 allocs |
| 9 | Recent-handler id: stack `encode_lower` | per record/remove | 52.2 → 10.7 ns · 1 → 0 allocs |
| 10 | 4xx body: borrowed `ErrorResponse` + `ErrorKind::as_str` + `not_found`/`already_exists` clone kill | per 404 | 426 → 367 ns · 11 → 8 allocs |
| 11 | vCard emit: `write!` + borrowed address fields | per contact create/update | 804 → 386 ns (**2.08x**) · 21 → 5 allocs |
| 12 | Search page: `into_iter().skip().take()` move (both branches) | 50-item page | 108.3 → 89.4 µs · −301 allocs |
| 13 | Content-hit verify: parse each UUID once | 100-hit page | 7.90 → 5.20 µs (**1.52x**) |
| 14 | Group last-user check: HashSet probe | 500×500 check | 105.3 → 17.1 µs (**6.1x**) |
| 15 | Retry op-label: lazy closure (success path never formats) | per blob op | 71.8 → 0.7 ns · 2 → 0 allocs |
| 16 | `encrypt_bytes`: in-place detached, single buffer (write now mirrors the in-place read) | 256 KiB chunk | 231.1 → 208.1 µs (**1.11x**) · 2 → 1 allocs |
| 17 | Encrypted `collect_stream`: chunk-sized reserve | 1 MiB blob, 4 KiB frames | 60.4 → 53.2 µs · 9 → 1 allocs |
| 18 | Recluster cosine: norms precomputed once (bit-identical gate over all pairs) | 200 faces × 512-dim pass | 7.70 → 7.17 ms (**1.07x**) |
| 19 | `CalendarEventDto`: `into_parts` move (the ~11 KiB `ical_data` memcpy gone) | per CalDAV event row | 497 → 283 ns (**1.76x**) · 14 → 8 allocs |
| 20 | RateLimiter: lock-free `get` (borrows the key) + `insert` | per limited request | allocs 8.0 → 6.0 · wall neutral (1 657 vs 1 695 ns, within run variance) |
| Q1 | Deferred upload registration: 3 round-trips → 1 CTE insert (the `persist_file` template) | per uploaded file (incl. cleanup DELETE) | 1.83 → 1.32 ms · gates: identical path/drive, missing-parent → not-found |
| Q2 | Calendar/AddressBook/Playlist authz `direct_grant_cache` (single-flight, invalidated on `set_role`/`clear_role`) | per DAV check | 0.197 ms → 0.2 µs on hit (**~1000x**) · revocation-flip gate OK |
| Q3 | `expand_user`: `tokio::join!` the `is_external` read + groups CTE | per cold expansion | 0.426 → 0.204 ms (**2.1x**) |
| Q5 | Recluster persistence: per-face UPDATE loop → one UNNEST batch | 200-face apply (incl. reset) | 80.0 → 8.7 ms (**9.2x**) · final column state identical |
| S1 | SPA `ResourceList.selectedEntries`: O(N)×2 per toggle → id-index O(k·log k); hosts consume the snippet param | comparisons per 51-toggle gesture (N=2 000) | 204 000 → <2 602 · identical output/order gated |
| S2 | SPA Recent: star reads the new `favoriteIds` prop — mapper no longer depends on the set | rows re-mapped per star click (N=400) | 400 → 0 · identical star states gated |
| S3 | SPA admin `timeAgo` >30d: cached `Intl.DateTimeFormat` | constructions per 1 000 formats | ≤1 · output equals `toLocaleDateString()` |

Also shipped without a dedicated row: CardDAV `getlastmodified` per-contact
stack render (ROUND10-§13 helper + chrono fallback), NC capabilities poll
logs demoted to `debug` (each poll forced a formatted line + locked stdout
write), trash `to_dto` `into_parts` move + fused/interned display fields
(trash listing and path-resolver rows now share the ROUND9 interning),
`TrashedItemDto` name/path moves.

Cross-round regression guards re-run after the StoragePath / classifier
rework — both gates PASS byte-identical, and the shipped code now beats the
numbers those rounds recorded:

- `bench_row_path` (round 4): file row 705 → 280 ns/row (2.52x), allocs
  15.75 → 6.00; folder row 684 → 365 ns (1.87x), 14.08 → 5.24.
- `bench_dto_map` (round 3): File→FileDto 839 → 606 ns/row, 10.09 → 3.08
  allocs; Folder→FolderDto 314 → 161 ns, 11.80 → 1.00 allocs.

## [1] REST download dead clone → move

```
cargo run --release --features bench --example bench_round11_micro   # §1
```

`download_file_impl` cloned the whole `FileDto` (7 owned Strings) into
`get_file_optimized_preloaded` on every authenticated download, purely to
read `mime_type`/`size` afterwards — the share path already captured+moved
(ROUND10 §3 fixed its double-*fetch*, not this clone). Now: one `Arc<str>`
bump + a `u64` copy, then move. 295.3 → 20.7 ns, 7 → 0 allocs per download.

## [2] StoragePath joined-only representation

```
cargo run --release --features bench --example bench_round11_micro   # §20
cargo run --release --features bench --example bench_row_path        # cross-round gate
```

`StoragePath` stored `segments: Vec<String>` — one heap String per path
component built on EVERY hydrated row — while the DTO path only ever
consumed the joined form, and the entities carried a second `path_string`
duplicate. The value object now stores the canonical joined `String` alone
(`"/"` or `/seg(/seg)*`); `file_name`/`parent`/`segments()`/`Display`
derive on demand; `File`/`Folder` lost the duplicate field (`path_string()`
borrows). `.segments()` had zero external callers — verified before the
rework. Equivalence gates: identical `path_string`, `file_name`, `parent`,
`Display` across the corpus, plus the round-4 harness's byte-identical
gate. 500-row page: 121.0 → 80.3 µs, 4 000 → 1 000 allocs; per-row RAM
drops by the Vec + per-segment String headers + the duplicate path.

## [3] Display classifier fusion

```
cargo run --release --features bench --example bench_round11_micro   # §21
```

Every listed file ran the full MIME classification three times
(`icon_class_for`, `icon_special_class_for`, `category_for`), each
heap-allocating its own `to_ascii_lowercase()` on the extension-fallback
path. The three decision trees are byte-for-byte preserved (they diverge
deliberately, so no merged tree); `classify_display` lowers the extension
once into a 16-byte stack buffer shared by all three. Extensions longer
than any table entry short-circuit to the same `_`-arm defaults (gated).
Call sites: `FileDto::from`, folder/favorites/recent handlers, trash
listing (×2), path-resolver — the last two also gained the ROUND9
interning they had missed (`Arc::from` per row → refcount bump).
13-row corpus: 4 390 → 2 074 ns, 21 → 0 allocs.

## [4][5] Memoized process-invariant bodies

```
cargo run --release --features bench --example bench_round11_micro   # §3, §18
```

`/status.php` rebuilt its `json!` tree per NC client poll (836 ns / 14
allocs → 28.8 ns / 0). `/openapi.json` was the extreme case: utoipa
reconstructed and re-serialized the whole 171 KiB spec on every request —
2.77 ms / 12 474 allocs → 18.5 ns / 0 via the same `OnceLock<Bytes>`
pattern as ROUND9's capabilities memoization. Byte-identical gates on both.

## [6] NC upload-session PROPFIND emit

The last hand-built XML handler: `String::new()` + `push_str(&format!(…))`
per element per chunk + chrono `to_rfc2822()` per chunk. Now pre-sized +
`write!` + `common::fmt::rfc2822_utc` (chrono fallback out-of-range).
Byte-identical gates at 16 and 256 chunks (escape distributes over
concatenation; RFC 2822 output has no XML-special chars). 16 chunks:
13.2 → 5.2 µs; 256 chunks: 203.7 → 75.4 µs, 2 582 → 772 allocs.

## [Q1] Deferred upload registration 3 → 1

```
cargo run --release --features bench --example bench_round11_queries # §1
```

The write-behind REST upload path ran parent-drive SELECT → INSERT →
parent-path SELECT, the first and third re-reading the identical
`storage.folders` row. Ported to the `WITH parent AS (…) INSERT … SELECT
… RETURNING (SELECT path FROM parent)` template `persist_file` has used
since ROUND2 — 0 rows ⇒ the same `not_found("Folder")` the old first query
produced (gated, anti-enum shape preserved). Root uploads (no parent)
keep their previous two-step shape.

## [Q2] direct_grant_cache

Calendar/AddressBook/Playlist were the only `check()` arms with no result
cache — every CalDAV/CardDAV/music request re-ran the `role_grants` point
query, and DAV clients poll continuously. Added
`direct_grant_cache: Cache<(Subject, Resource, Permission), bool>`
(30 s TTL / 100k, `try_get_with` single-flight — the ROUND8/10 pattern),
flushed on `set_role`/`clear_role` for those resource types; group/expiry
churn self-heals within the TTL exactly like `cascade_grant_cache`.
Gates: identical verdict; a revocation + flush flips the next check.
0.197 ms → 0.2 µs on hit.

## [Q3] expand_user join!

The `is_external` point read and the recursive groups CTE are independent;
serial await paid two round-trips end-to-end on every cold expansion
(per user per 30 s TTL window). `tokio::join!`: 0.426 → 0.204 ms.

## [Q5] Recluster UNNEST batch

`POST /api/people/recluster` issued one `UPDATE faces.faces SET person_id`
per face sequentially (both the unassign-small-clusters and assign loops).
Assignments now accumulate and apply as a single
`UPDATE … FROM unnest($1::uuid[], $2::uuid[])` (the ROUND10 `save_faces`
pattern) — 200-face library: 80.0 → 8.7 ms, final column state identical.
Pairs with §18's norm precomputation on the CPU side (7.70 → 7.17 ms for
the O(N²) pass, bit-identical similarity gated over every pair).

## [S1][S2][S3] SPA pack (vitest gates)

```
cd frontend && npx vitest run src/lib/components/round11.bench.test.ts
```

- **`ResourceList.selectedEntries`** (the ROUND10 flagged item): the
  component re-filtered the ENTIRE items array per selection change, and
  favorites/recent ignored the snippet param and recomputed their own
  `entries.filter(…)` shadow — two full O(N) scans per toggle, O(N²)-ish
  across a shift-range. Now an id→index Map (rebuilt only when `items`
  changes) projects the selection in O(k·log k) preserving item order;
  hosts consume the snippet param and their dead `selectedIds` mirror is
  gone (the component's prune effect already self-heals on reload).
  51-toggle gesture on 2 000 items: 204 000 → <2 602 comparisons.
- **Recent favorite star**: the entry mapper read `favoriteIds.has(id)`,
  subscribing the whole O(N) map to the SvelteSet — one star click rebuilt
  all N entries and re-rendered every visible row. `ResourceList` gained a
  `favoriteIds` prop read directly by the star widget; the mapper is
  set-independent. Star click: N → 0 rows re-mapped, star states gated
  identical.
- **admin `timeAgo`**: the >30-day fallback called `toLocaleDateString()`
  (a fresh `Intl.DateTimeFormat` per call); now the app-wide cached
  formatter — output equality gated, ≤1 construction per 1 000 formats.

## Rejected / reworked this round (the discipline working)

- **`min(fm.file_id)::text` geo-cluster cast (Q4)**: PostgreSQL has **no
  `min(uuid)` aggregate** — the "cast once per cluster" rewrite fails to
  parse (`42883`). The gate caught it before it could ship broken; the
  per-row-cast original stays (22.6 ms per 5k-row viewport, admin-shaped
  traffic), and a custom `CREATE AGGREGATE` was judged schema surface this
  query doesn't justify. The bench section now reproduces the rejection.
- **RateLimiter `entry().and_upsert_with`**: 1 657 → 2 365 ns and
  8.0 → 9.1 allocs — moka's compute-entry machinery costs more than the
  two ops it replaced. Redesigned as lock-free `get` (borrows the key, no
  alloc) + `insert`: 6.0 allocs, wall within variance; identical counter
  sequence gated. Adopted in that form.
- **GET/HEAD `Last-Modified` stack-render port**: chrono's `to_rfc2822()`
  String IS the terminal allocation the header needs (38.7 ns incl. alloc
  vs 47.0 ns stack render + the same alloc). Only body-emit sites (where
  `write!` lands in an existing buffer, 31.8 ns / 0 allocs) benefit —
  those were ported (§6 + CardDAV); header sites stay on chrono.
- **`tracing-appender` non-blocking log writer (L1)**: on a fast sink
  (stdout→/dev/null — the containerized default) sync sustains 1.41M ev/s
  vs 0.99M non-blocking, with a better tail (p999 76 vs 151 µs, max 0.8
  vs 8.3 ms — the channel hop costs more than the write). Non-blocking
  only wins on a slow sink (20 µs/line: 6.4x wall, p50 22 → 1.9 µs), but
  there the drain gate FAILED — buffered tail lines can be lost at
  shutdown, unacceptable for the audit channel. Not adopted;
  `tracing-appender` stays as a dev-dependency for the reproducible
  harness (`bench_log_writer`, 4 arms via `BENCH_LOG_ARM`/`_WRITER`).
- **First search-page model (`drain(range)`)**: −300 allocs but slower on
  wall (tail memmove). Reshaped as `into_iter().skip().take().collect()`
  — moves the page, drops the rest, wins both axes (§12 row in Summary).

## Deferred / flagged (not shipped this round)

- **NC preview 304 still runs `get_file`**: dropping the fetch on the
  revalidation path changes existence semantics (deleted file → 304
  instead of 404) — needs maintainer sign-off, same class as the standing
  CalDAV authz-before-fetch reorder (ROUND9/10 flag).
- **`CachedBlobBackend`'s `Mutex<LruCache>` index** serializes every
  cached read on remote+cache deployments; the moka byte-weigher
  migration (file-content-cache pattern) deserves its own round with a
  concurrency bench and eviction-unlink care.
- **Capture-metadata extraction reads each media file 2-3×**
  (`media_metadata_service`: kamadak full read + nom-exif path re-read +
  track fallback). Feeding nom-exif from the in-memory buffer needs its
  `MediaSource` API verified on the pinned version.
- **`CachedBlobBackend::local_blob_path` sync `stat`** (ROUND10 flag
  stands — needs an async port variant).
- **Azure SDK 0.21 stack** drags duplicate dependency trees (h2 0.3+0.4,
  three hashbrown generations, base64 0.13, getrandom 0.1) into every
  build — an SDK bump is a dedicated migration.
- **`AudioMetadataRepository::list_by_{artist,album,genre}`** are dead
  code with seq-scan `ILIKE` shapes — flag for deletion, not indexing.
- **`CachedBlobBackend::put_blob` cache population** silently fails for
  S3/Azure whole-file puts (inner backend deletes the source before the
  cache copy) — correctness note for maintainers.
- **Grouped file/grid views unvirtualized** (ROUND10 flag stands).

## Environment / methodology

- `cargo run --release --features bench --example bench_round11_micro`
  — 21 sections, counting allocator, BEFORE replicas vs shipped shapes,
  equivalence gates inline (`BENCH_ITERS`, default 100k).
- `cargo run --release --features bench --example bench_round11_queries`
  — needs Postgres; seeds and sweeps its own fixtures (`BENCH_PASSES`).
- `BENCH_LOG_ARM=sync|nonblocking [BENCH_LOG_WRITER=slow] cargo run
  --release --features bench --example bench_log_writer >/dev/null`.
- `cd frontend && npx vitest run src/lib/components/round11.bench.test.ts`.
- Cross-round guards: `bench_row_path`, `bench_dto_map`.
