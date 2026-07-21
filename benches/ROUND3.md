# Round 3 — listing/timeline SQL shapes, auth herd, blob-cache stampede, spool I/O, DTO allocs

Twelve benchmark-gated changes. Rule of the round (same as ROUND2): every
change ships with a BEFORE/AFTER benchmark; an AFTER that doesn't beat its
BEFORE gets rolled back — none did. Equivalence gates (byte-identical
output / identical row sequences) guard every behavior-preserving rewrite.

Measured on 4 cores / 15 GiB, local PostgreSQL 16 (fsync off), release
profile. Reproduce any row with the command in its section.

## Summary

| # | change | key metric | before → after |
|--:|---|---|---|
| 1 | Web-UI listing keyset pushdown | ms/page p50, 20k-entry folder | 26.6 → 1.30 (**19.5x**) |
| 2 | Photos timeline LATERAL top-N | ms/page p50, 50k-photo library | 97.4 → 1.61 (**55.7x**) |
| 3 | PROPFIND subfolder keyset | full walk, 5k dirs | 79.7 → 17.9 ms (**4.5x**) |
| 4 | Basic-auth single-flight | herd CPU, 8 conns | 2620 → 300 ms (**8.7x**) |
| 5 | Blob-cache miss single-flight | remote fetches / wall | 16 → 1, 519 → 188 ms (**2.8x**) |
| 6 | Chunk-assembly read buffer 512K | wall / read syscalls | 251 → 109 ms (**2.3x**), 2580 → 340 |
| 7 | Chunk-spool BufWriter 512K | wall / write syscalls | 877 → 158 ms (**5.6x**), 12800 → 400 |
| 8 | S3/Azure unsynced PUT (no HEAD) | wall / requests, 500 chunks | 1604 → 868 ms (**1.8x**), 1000 → 500 |
| 9 | DTO mapping interning | allocs/row file / folder | 11.0 → 4.0, 11.8 → 1.0 |
| 10 | CardDAV REPORT dead work | 5k contacts, getetag | 55.7 → 5.7 ms (**9.8x**) |
| 11 | Search-cache byte weigher | retained RSS worst case | ~298 MiB → 31.9 MiB (bounded) |
| 12 | Drop aws-config/aws-smithy-types | dep-graph nodes | 1728 → 1646 |

Frontend (gated by vitest, `frontend/src/lib/utils/formatDate.bench.test.ts`):
cached `Intl.DateTimeFormat` — 20k dates 2612 → 50.6 ms (**51.6x**), output
identity asserted across locales.

---

## [1] Web-UI folder listing — whole-folder rescan → per-branch keyset — 19.5x

`list_resources_paged` (SPA files view) applied its keyset cursor OUTSIDE
the folders/files UNION-ALL on computed columns (`sort_str = LOWER(name)`,
`folder_first`), so Postgres re-scanned and top-N-sorted every remaining
row of the folder on every page (EXPLAIN: Seq Scan, 17,999 rows removed by
filter, 29 ms / 565 buffers per 200-row page on a 20k-file folder).

Now the cursor is pushed into each branch as a sargable row-value
comparison on base columns (`(LOWER(name), id) > ($str, $id)`), constants
folded per branch in Rust (a cursor in the file group drops the folder
branch outright), each branch pre-sorts + pre-limits, and the outer query
merges ≤ 2·limit rows. Two new expression indexes (migration
`20260918000000`): `idx_files_folder_lname (folder_id, LOWER(name), id)`
and `idx_folders_parent_lname (parent_id, LOWER(name), id)`, both partial
on `NOT is_trashed`.

```
cargo run --release --features bench --example bench_listing_keyset
# full drain, 20k files + 300 dirs, 200/page       total ms   p50/pg   p99/pg
# name        OLD/no-idx                             2717.2    26.57    33.55
# name        OLD/idx (indexes alone don't help)     2786.8    27.83    35.62
# name        NEW/idx                                 139.6     1.30     1.81   19.5x
# modified_at OLD → NEW (no dedicated index)         1653.4 → 1367.5             1.2x
```

Equivalence: the drained `(type, id)` sequence is asserted identical across
all modes and both sort orders; the example exits 1 on mismatch.

## [2] Photos timeline — full-library scan → per-drive LATERAL top-N — 55.7x

`list_media_files` claimed `idx_files_media_timeline_by_drive` let LIMIT
stop the scan early; EXPLAIN refuted it — the folders/file_metadata joins
and the global sort sat ABOVE the `drive_id IN (grants)` nested loop, so
every page fed the ENTIRE media library through the join into a top-N
heapsort. Now the accessible drive ids materialise once, a
`CROSS JOIN LATERAL (… ORDER BY media_sort_date DESC LIMIT k)` per drive
does one bounded index scan each, and the joins run on the k emitted rows
only.

```
cargo run --release --features bench --example bench_photos_timeline
# 10 pages of 100, 50k photos, 3 drives      total ms   p50 ms/page
# OLD                                          1032.1        97.41
# NEW                                            18.5         1.61   55.7x
```

Equivalence: page-by-page id sequences asserted identical (seed uses
strictly distinct capture dates so ties can't mask reordering).

## [3] PROPFIND subfolder paging — LIMIT/OFFSET + COUNT(*) OVER() → keyset — 4.5x

The exact quadratic shape PROPFIND-PAGING fixed for files still applied to
sub-folders on both DAV surfaces: every page window-aggregated and
re-scanned all N sub-folders, and the total was only used for `has_next`.
New `FolderRepository::list_folders_batch` (keyset `name > $last`, served
by the existing `idx_folders_unique_name`, no migration) wired into both
streaming PROPFIND walkers via `list_folders_batch_with_perms` (same
per-batch authz as before).

```
cargo run --release --features bench --example bench_folder_keyset
# full walk, 5k dirs, 500/page     total ms   p50 ms/page
# OFFSET                               79.7          6.54
# KEYSET                               17.9          1.64   4.5x
```

## [4] Basic-auth cache — thundering herd → single-flight — 8.7x CPU

Every DAV/NC request authenticates via `verify_basic_auth`. On a cache
miss each concurrent caller independently ran the full slow path — an
Argon2id verification (m=64 MiB, t=3, p=2 ≈ 290 ms CPU here) apiece. DAV
sync clients hold 4-8 parallel connections, so every TTL expiry (300 s)
fanned out K verifications: a recurring p99 spike + CPU/RAM burst.
`try_get_with` now coalesces concurrent misses; errors are never cached
(brute-force cost preserved), revocation via `invalidate_entries_if`
unchanged.

```
cargo run --release --features bench --example bench_auth_herd
# herd of 8, cold cache          wall ms   CPU ms   verifications
# BEFORE (per-caller)                764     2620             9.0
# AFTER  (single-flight)             311      300             1.0
# warm hit p50: 0.6 us
```

## [5] CachedBlobBackend — miss stampede → per-hash single-flight — 16 fetches → 1

K concurrent cold readers of one blob (video player's parallel Range
probes; N clients pulling the same new file) each downloaded the FULL blob
from S3/Azure — and raced truncating writes on ONE deterministic `.tmp`
path (a torn interleaving could be renamed into the cache). Fixes: a
per-hash DashMap gate (leader fetches, waiters re-check and serve
locally), plus unique `.{uuid}.tmp` names + error-path cleanup so a
corrupt file can never land at the final path.

```
cargo run --release --features bench --example bench_blob_cache
# 16 cold readers, 32 MiB blob, shared 1 GiB/s link   wall ms   fetches   remote MiB
# BEFORE (per-caller)                                     519        16          512
# AFTER  (single-flight)                                  188         1           32
# gates: fetch count == 1; BLAKE3 of served + durable cache file == source
```

## [6][7] Upload spool I/O — 64 KiB reads, unbuffered frame writes

Assembly read (`stream_from_files`, the single read pass over every
completed chunked upload) used 64 KiB `ReaderStream` polls — one
blocking-pool dispatch + read(2) each — while every other blob path uses
256 KiB+. Capacity sweep picked 512 KiB. Chunk-spool writes
(`stream_body_to_path`, every chunk PUT on both surfaces) went straight to
a bare tokio File — one dispatch + write(2) per ~16-64 KiB HTTP frame; now
wrapped in `BufWriter::with_capacity(512 KiB)` like the dedup handler's
spool loop.

```
cargo run --release --features bench --example bench_upload_spool
# [1] read 16 x 10 MiB parts    wall ms   read syscalls
#   64K  (BEFORE)                 250.8            2580
#   256K                          125.1             660
#   512K (AFTER)                  108.8             340   2.3x
#   1M                            111.3             180
# [2] spool 640 x 16 KiB frames x 20 files
#   bare File (BEFORE)            877.4    12800 syscw
#   BufWriter 512K (AFTER)        157.9      400 syscw    5.6x
```

## [8] S3/Azure chunk writes — HEAD-before-PUT → unconditional PUT — 1.8x

Neither remote backend overrode `put_blob_from_bytes_unsynced`, so the
dedup settle path (every NEW chunk of every upload) routed through
`put_blob_from_bytes` and its "idempotent" HEAD/get_properties probe —
2 round-trips per chunk for chunks the dedup layer already knows are new.
Content-addressed keys make re-PUTs overwrite-safe, so the new overrides
PUT directly. Azure additionally stopped copying every chunk
(`data.to_vec()` → `Bytes` into `azure_core::Body`): 0.44 ms + 4 MiB
transient alloc per 4 MiB chunk removed.

```
cargo run --release --features bench --example bench_s3_put
# 500 x 256 KiB chunks, concurrency 8, 10 ms/request stub
# BEFORE (HEAD+PUT)   1604 ms   500 HEADs + 500 PUTs
# AFTER  (PUT only)    868 ms   500 PUTs             1.8x
```

## [9] Entity → DTO mapping — closed-set interning + 1-alloc formatting

`Arc::<str>::from(&'static str)` always allocates+copies, so every file
row paid 4 allocations for values drawn from a ~60-string closed set
(icon class, special class, category, mime), plus 2-alloc etag and 2-alloc
size formatting; FolderDto additionally built its etag twice and cloned 4
Strings it could move. Now: `LazyLock` intern tables (lookup + refcount
bump; unknown values fall back to `Arc::from`, same bytes), single-alloc
`compute_etag`/`format_file_size`, and `Folder::into_parts()` moves.

```
cargo run --release --features bench --example bench_dto_map
# 10k rows                       ns/row   allocs/row
# File→FileDto    BEFORE         1229.2        10.96
# File→FileDto    AFTER          1004.9         3.96
# Folder→FolderDto BEFORE         425.2        11.80
# Folder→FolderDto AFTER          204.5         1.00
# gate: all DTO fields byte-identical BEFORE vs AFTER (10k files + 10k folders)
```

## [10] CardDAV REPORT — dead double vCard generation + O(N²) scan — 9.8x

`handle_report` pre-generated a vCard for EVERY contact; the adapter then
did a linear uid `find` per contact — O(N²) string compares — and
DISCARDED the result (`let _ = vcard`), regenerating on demand inside
`write_contact_response` anyway. Pure dead work, deleted; `contact_to_vcard`
also switched `push_str(&format!(…))` → `write!` (one temp String per
vCard line removed).

```
cargo run --release --features bench --example bench_carddav_report
# N=5000   getetag                55.7 →  5.7 ms   9.8x
# N=5000   getetag+address-data   76.2 → 15.3 ms   5.0x
# gate: REPORT XML byte-identical BEFORE vs AFTER for all prop sets
```

## [11] Search-results cache — entry count → byte weigher — bounded RSS

The cache was capped at 1000 ENTRIES with a 300 s TTL; each entry holds up
to 500 enriched rows (~10 owned Strings each) and keys include
user+query+offset+limit, so every keystroke/page/user minted an entry —
~300 MiB of invisible RSS was reachable. Now a byte weigher + 32 MiB
budget (`OXICLOUD_SEARCH_CACHE_MAX_BYTES`), same TTL, same read latency.

```
cargo run --release --features bench --example bench_search_cache_mem
# 1000 pages x 500 rows          retained bytes    get() p50
# BEFORE (1000 entries)          ~298 MiB (9.3x)      155 ns
# AFTER  (32 MiB weigher)         31.9 MiB            155 ns   parity 1.00x
```

## [12] Cargo — drop aws-config + aws-smithy-types

Both were direct dependencies with ZERO references in the codebase —
`S3BlobBackend` builds its client purely from `aws_sdk_s3::config` with
static credentials. `aws-config` alone dragged aws-sdk-sso, aws-sdk-ssooidc
and aws-sdk-sts into every build. Dependency-graph nodes: 1728 → 1646.
`tokio`'s `process` feature (used by the ffmpeg thumbnailer) was only
enabled transitively through aws-config's feature unification — it is now
declared explicitly.

## Frontend — cached Intl.DateTimeFormat — 51.6x

`formatDate` (and four sibling callsites) constructed a fresh
`Intl.DateTimeFormat` per call (~131 µs each here) — paid roughly twice
per row while rendering/scrolling file lists. Module-scope cache keyed by
(locale, options), invalidated on `languagechange`.

```
cd frontend && npx vitest run src/lib/utils/formatDate.bench.test.ts
# 20k dates: cached 50.6 ms vs per-call 2612.0 ms (51.6x); output-identity
# matrix across en/es/ar/ja and every option shape used by the app
```

## Audited but NOT adopted (for the record)

- **Fat LTO / panic=abort / OpenAPI LazyLock**: refuted by the verification
  pass (sub-1% plausible gain, or cold paths; `catch_unwind` shields
  pdf-extract so panic=abort is off the table).
- **Chained clone-on-hit drive caches, localeCompare→Intl.Collator**:
  measured previously — residual gains are noise or regressions
  (benches/CHROOT-CACHE.md, benches/NPLUS1-AND-CACHES.md).
- **Follow-ups worth a future round** (confirmed real, not yet gated):
  grouped/swimlane files view is unvirtualized (10k-row DOM); Azure
  download path buffers whole blobs in RAM (needs an Azurite-gated bench);
  face-indexing spawns unbounded per-image tasks; WebDAV drive-selector
  resolution re-runs the grants join per request (cacheable like
  CHROOT-CACHE); `make_file_path` split→rejoin + NFC copy per listing row.
