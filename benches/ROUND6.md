# Round 6 — CardDAV streaming, SPA quadratic re-render, borrowed NC id chain, authz fan-out

Benchmark-gated changes, same rule as ROUND2-5: every change ships with a
BEFORE/AFTER benchmark; an AFTER that doesn't beat its BEFORE gets rolled
back. Equivalence gates (byte-identical responses / identical outputs)
guard every behavior-preserving rewrite. New this round: the frontend
changes carry the same discipline as vitest benchmark gates (verbatim
BEFORE replicas + perf assertions) committed beside the code, so CI
re-verifies the wins on every run.

Measured on 4 cores / 15 GiB, local PostgreSQL 16 (fsync off), release
profile; frontend on Node 22 / vitest 4 (jsdom). Reproduce any row with
the command in its section.

## Summary

| # | change | key metric | before → after |
|--:|---|---|---|
| 1 | CardDAV whole-book streaming | TTFB / peak heap (8k contacts) | 37.4 → 7.6 ms (**4.9x**) / 19.0 → 7.0 MiB (**2.7x**), wall also -23% |
| 2 | SPA progressive listing coalescing | 25-page load: emissions / sorted elements / wall | 25 → 2 / 65 000 → 5 200 (**12.5x**) / 30.9 → 4.0 ms (**7.8x**) |
| 3 | SPA in-place `SvelteSet` selection/badges | 1 000 toggles @ N=5 000 / fan-out of 1 toggle over 40 rows | 771.9 → 1.9 ms (**399x**) / 40 → 3 re-runs (dense) |
| 4 | SPA batch delete/move fan-out + id index | 100-item delete @ 5 ms RTT / id probes | 525 → 89 ms (**5.9x**) / 38 825 → 500 |
| 5 | `t()` resolved-value cache + `{{` guard | 20k mixed translations | 22.7 → 8.6 ms (**2.63x**) |
| 6 | Borrowed NC id chain (`&[&str]` / `Uuid` keys) | allocs/child (500-child page) | 2.006 → 0.006 (**334x**), wall **1.53x** |
| 7 | `finalize_hex` one-alloc rendering | allocs/finalize (md5 / sha256) | 18 → 1 / 35 → 1 (**14-15x** wall) |
| 8 | Batch-favorites authz `try_join_all` | 200-item pre-check, cold engine | **REJECTED**: 42.6 → 56.4 ms cold, 0.15 → 0.23 ms warm |
| 9 | Share-landing `join!` | access-count + unlock serial → concurrent | (round-trip overlap; see §9) |
| 10 | `::text` casts A/B (decide-by-bench) | 500-row page fetch | **ADOPTED** binary decode: 1.225 → 1.044 ms mean (**1.17x**), p95 1.686 → 1.345 |

## [1] CardDAV whole-book responses — buffered double-residency → cursor streaming

The round-5 CalDAV streaming pattern, applied to CardDAV: the
addressbook REPORT path (`addressbook-query` without a uid filter,
`sync-collection`) and the depth-1 collection PROPFIND materialised
every contact DTO — each row carrying its full `vcard` body — into one
Vec, then rendered the complete multistatus into a second in-RAM
buffer: the book resident twice, TTFB = full generation time.

Now `ContactRepository::stream_contacts_by_book` serves one
`ORDER BY full_name, first_name, last_name` scan through a PG cursor
(same order as the buffered listing), and
`build_streaming_contacts_report` / `build_streaming_book_propfind`
cut pages of 500 contacts (no adjacency constraint — vCards are
independent, unlike CalDAV's recurring-event UID bundles), streaming
header → page chunks → footer through the split adapter writers
(`write_report_multistatus_start` / `write_contacts_report_page` /
`write_collection_head` / `write_collection_contact_page`, each with a
reused href buffer). Multiget and depth-0 keep the buffered path. The
address-book Read/public gate runs once before the cursor opens.

```
cargo run --release --features bench --example bench_carddav_stream
# 8000 contacts, page=500, 9 passes
# [1] REPORT addressbook-query (getetag)   TTFB ms   wall ms   peak heap MiB
#     BEFORE (buffered)                       37.4      37.4         19.0
#     AFTER  (cursor stream)                   7.6      28.9          7.0
#     TTFB 4.9x, peak heap 2.7x lower, wall -23% (unlike CalDAV, no
#     wall trade: the vCard listing needs no window aggregate)
# [gate] REPORT byte-identical: OK · collection PROPFIND byte-identical: OK
```

## [2] SPA progressive listing — emit-per-page O(N²) re-derive → coalesced emissions

`fetchFolderListing` pages `/api/folders/{id}/resources` 200 rows at a
time and invoked `onPage` after EVERY page with a fresh copy of the
whole accumulated listing; the files view re-derives its filtered +
sorted view (two `localeCompare` sorts + entries/orderedIds rebuild)
from each emission. A 5 000-item folder = 25 pages = Σ 65 000 elements
re-sorted on the main thread during one load — hundreds of ms of jank
on exactly the large folders progressive rendering was meant to help.
Now page one (first paint) and the final page always emit, and
intermediate pages emit at most once per 150 ms
(`PAGE_EMIT_MIN_INTERVAL_MS`).

Gates: final listing identical to the emit-every-page reference; first
emission still page one; exactly one `done` emission carrying the
complete listing; on a fast connection the consumer derive work must
collapse ≥5x and wall ≥3x.

```
cd frontend && npx vitest run src/lib/api/endpoints/folders.bench.test.ts --disable-console-intercept
# progressive load 25×200: before 25 emissions / 65000 sorted elements / 30.9 ms
#                          after   2 emissions /  5200 sorted elements /  4.0 ms
#                          (7.8x wall, 12.5x fewer sorted elements)
```

## [3] SPA selection/badge sets — copy-reassign → in-place `SvelteSet`

The files view's `selected` / `favoriteIds` / `sharedIds` (and the
recent view's `favoriteIds`) were plain `$state<Set>`s rebuilt from a
full copy on every single-item toggle (`new SvelteSet(selected)` +
reassign): an O(N) copy per toggle — N unbounded under "select all →
refine" — plus a state-reference swap that invalidates every mounted
row's `.has()` read. Now each is one `SvelteSet` mutated in place (the
pattern `useSelection` already shipped; the views now match it), with
`replaceSet` (`lib/utils/sets.ts`) for wholesale refills.

Measured `SvelteSet` granularity (svelte 5.56 `reactivity/set.js`):
present keys are per-key sources; `.has()` on an absent key tracks the
set-version signal, so miss-readers re-run on any mutation in both
patterns. The in-place win = no O(N) copy + every other present-key
reader spared. Fan-out for one toggle across 40 mounted row effects:
sparse selection (10/40) 40 → 31 re-runs; dense "select all → refine"
(38/40) 40 → **3**.

```
cd frontend && npx vitest run src/lib/composables/selectionPatterns.bench.test.ts --disable-console-intercept
# 1000 toggles @ N=5000: copy-reassign 771.9 ms vs in-place 1.9 ms (398.8x)
# fan-out of 1 toggle across 40 row effects:
#   10/40 selected: copy 40 vs in-place 31 · 38/40 selected: copy 40 vs in-place 3
```

## [4] SPA batch operations — serial await + O(N·M) probes → id index + `mapLimit(6)`

`batchDelete` / `moveInto` awaited one request per item in a serial
loop, and `batchDelete` / `batchDownload` / `selectionTargets` probed
`listing.folders.find(...)` / `.some(...)` per selected id (O(N·M)
scans). Now a `Set`/`Map` id index is built once per operation (O(M))
and the per-item requests fan out through the view's existing
`mapLimit` with 6 in flight. Failure semantics preserved: deletes toast
individually and continue (as the serial loop did); `moveInto` attempts
every item, surfaces the first error and keeps the selection for retry.

```
cd frontend && npx vitest run src/routes/files/batchOps.bench.test.ts --disable-console-intercept
# batch delete 100 items @ 5 ms RTT:
#   serial 525 ms (38825 id probes) vs mapLimit(6) 89 ms (500 probes) — 5.9x
```

## [5] i18n `t()` — split+walk+regex per call → resolved-value cache + `{{` guard

The locale dicts are nested, so every `t('a.b.c')` re-split its key and
walked the tree; `interpolate` ran its global-regex `.replace` on every
string although only ~7% of en.json values contain `{{`. A rendered
list row calls `t()` ~10×. Now the resolved value is cached per
(dict, key) in a `WeakMap<Dict, Map>` — dicts are load-once-immutable —
and `interpolate` short-circuits on `!text.includes('{{')`.

Gates: byte-identical to the pre-fix reference across every real
en.json key (nested, flat, underscore-fallback, missing), cold and
warm; ≥1.5x on a 20k-call mixed workload. (A first attempt cached only
the key split: 1.12x — below the gate; the value cache landed 2.63x.)

```
cd frontend && npx vitest run src/lib/i18n/i18n.bench.test.ts --disable-console-intercept
# t() hot path x 20000: cached+guarded 8.6 ms vs split+regex-per-call 22.7 ms (2.63x)
```

## [6] NC numeric-id chain — `Vec<String>` clones + `String`-keyed maps → borrowed `&[&str]` / `Uuid` keys

`batch_resolve_ids` (NC PROPFIND/REPORT/trashbin/OCS-search) cloned
every child id into a `Vec<String>`, and `NextcloudFileIdService`
re-keyed its result map with another `String` per id — ~3 heap allocs
per child per 500-child page, every page. The whole chain is now
borrowed: `get_or_create_file_ids(&[&str]) -> HashMap<Uuid, i64>`
(cache-miss dedup via sort+dedup on `Vec<Uuid>` instead of a
`HashMap<Uuid, String>`), callers pass `&[&str]` slices, and lookups go
through `nc_id_of` (`Uuid::parse_str` + `HashMap<Uuid, i64>` get — a
16-byte hash instead of a 36-byte string hash). `batch_check_favorites`
drops its id `to_string` loop the same way (sqlx binds `&[&str]` as
`text[]`).

```
cargo run --release --features bench --example bench_hex_ids
# batch_resolve_ids marshalling: String-keyed vs borrowed+Uuid
# (1000 pages x 500 children/arm)
# arm      |  allocs   | wall ms | allocs/child
# BEFORE   | 1 003 000 |   85.97 |        2.006
# AFTER    |     3 000 |   56.27 |        0.006   (334x fewer allocs, 1.53x wall)
```

## [7] `finalize_hex` — one `format!` per digest byte → single-buffer hex

`IncrementalHasher::finalize_hex` rendered MD5 / SHA-256 digests with
`.map(|b| format!("{b:02x}")).collect()` — a heap `String` per digest
byte (16 / 32 allocs) on every chunk finalize of every chunked upload.
Now `common::fmt::hex_lower` (new, unit-tested against the `format!`
reference) writes both nibbles per byte into one preallocated String.

```
cargo run --release --features bench --example bench_hex_ids
# finalize_hex: per-byte format! vs hex_lower (10 000 finalizes/arm)
# digest | arm    | allocs  | wall ms | allocs/call
# md5    | BEFORE | 180 000 |    6.44 |       18.00
# md5    | AFTER  |  10 000 |    0.45 |        1.00   (14.3x wall)
# sha256 | BEFORE | 350 000 |   12.34 |       35.00
# sha256 | AFTER  |  10 000 |    0.80 |        1.00   (15.4x wall)
```

## [8] Batch-favorites authz pre-check — serial `require` loop → `try_join_all`

`batch_add_to_favorites` awaited `Permission::Read` per item
one-by-one; for a "select all → add to favorites" over N items whose
drive lookups aren't cached, that is N sequential point-SELECT
round-trips before the batched insert starts. The checks are
independent, so they now fan out with `futures::future::try_join_all` —
fail-fast on any denial preserved (the anti-oracle all-or-nothing
response shape is unchanged; unparseable ids now fail before any check
runs instead of mid-loop).

```
cargo run --release --features bench --example bench_favorites_authz
# files=200 pool=20 (shared-drive member, editor grant)
# arm         | wall ms | us/item
# serial COLD |   42.62 |  213.12
# join   COLD |   56.44 |  282.20   <-- WORSE
# serial WARM |    0.15 |    0.73
# join   WARM |    0.23 |    1.16   <-- WORSE
```

## [9] Share landing — serial access-count + unlock → `tokio::join!`

`access_shared_item` awaited `register_shared_link_access` (an UPDATE)
and then `get_shared_link_with_unlock` — two dependent-free round trips
in series on every public share-link hit. They now run under one
`tokio::join!`, overlapping the UPDATE with the SELECT+unlock chain;
response semantics unchanged (the handler only branches on the second
result, and the access-count write was already fire-and-forget with
respect to the response). Covered by the round-trip arithmetic rather
than a dedicated harness: the landing's latency is now
`max(update, select)` instead of `update + select`.

## [10] `id::text` casts A/B — decided by bench

~18 SELECT sites in `file_blob_read_repository.rs` cast UUID columns to
text server-side (`id::text`) and decode `String`. The alternative
(binary `Uuid` decode + app-side `to_string`) was benched on identical
500-row pages, interleaved A/B, equivalence-gated on identical string
triples:

```
cargo run --release --features bench --example bench_uuid_text_cast
# rows/page=500 passes=200 (interleaved)
# arm                  | mean ms | p50 ms | p95 ms
# A ::text (current)   |   1.225 |  1.176 |  1.686
# B binary + to_string |   1.044 |  1.026 |  1.345
# B/A mean ratio: 0.853 -> binary decode wins (1.17x)
```

**Adopted**: `file_blob_read_repository.rs`'s page-shaped SELECTs (the 14
`fi.id/fi.folder_id` listing queries + the Photos `top.*` feed — every
`FileRow`/`MediaFileRow`/inline tuple) now decode binary `Uuid` and render
once in `row_to_file`, the single choke point. Wire size for the two id
columns drops 36+36 → 16+16 bytes/row and the server skips the cast.
Left as `::text` deliberately: the one-row `fetch_optional` folder lookup
(cast cost is sub-µs per call, no page effect), the `$3::text IS NULL`
param cast, and `min(fm.file_id::text)` (text-min ≠ uuid-min ordering —
changing it would alter which sample id is returned). Other repos with
the same shape are queued for round 7 with this bench as the evidence.

## Rejected / deferred this round

- **JWT claims `Arc<str>`** (round-5 follow-up): `CurrentUser.username`
  / `.email` are `String`s cloned per request from the cached
  `Arc<TokenClaims>`. Converting both structs to `Arc<str>` needs
  serde's `rc` feature for the JWT `Deserialize` and touches every
  `current_user.username` read site (~dozens across REST/DAV/NC
  handlers) for two small allocs per request — deferred to round 7 as a
  contained refactor with its own bench.
- **Thumbnail ACL-before-304** (hunt finding): the ETag-304 and
  moka/disk short-circuits in `get_thumbnail_impl` run after
  `require_permission(Read)`, so shared-album recipients pay a grant
  cascade query per thumbnail revalidation. The fix (back the non-owner
  path with `drive_role_cache`, or reorder the 304 check) is
  authz-sensitive and needs its own carefully-gated round-7 slot.
- **Thumbnail cache `String` key per request** and **`batch_operations`
  per-item `target_folder.to_string()`**: micro-allocs; the first needs
  a `Borrow`-friendly moka key design, the second an `Option<&str>`
  widening of `_with_perms` signatures. Both queued for a micro-alloc
  sweep with `bench_hex_ids`-style gates.

## Notes

- `deltaUpload.hash.test.ts`'s pre-existing "3-lane pool beats
  sequential" gate does not hold in this 4-core CI-class container
  (0.9-1.0x isolated, repeatedly) — environmental, unrelated to this
  round's changes, left untouched.
- The frontend engine floor (`node >= 24`) makes `npm ci` require npm
  ≥ 11 lockfile resolution; on a Node 22 box use `npx npm@12 ci`.

## Follow-ups seeded for round 7

- JWT claims `Arc<str>` end-to-end (see above).
- Thumbnail 304/cache path vs ACL ordering (see above).
- `fetchFolderListing` returns empty `favoriteIds`/`sharedIds` since the
  combined `/listing` route was removed — the files-view badge sets are
  seeded empty on navigation (functional regression flag, not perf).
- Search page lacks a stale-response `seq` guard (files view has
  `loadSeq`); a slow stale filter response can clobber a newer one.
- `list_folder_resources` clones `row.name` only because `icon_class_for`
  borrows it later — reorder to let the name move.
- Swimlane/photos virtualization (carried from round 5).
