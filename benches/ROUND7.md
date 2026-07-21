# Round 7 — photo timeline O(N²) → incremental, range-seek authz duplication, row-map clone

Benchmark-gated changes, same rule as ROUND2-6: every change ships with a
BEFORE/AFTER benchmark; an AFTER that doesn't beat its BEFORE gets rolled
back. Equivalence gates (identical output / byte-identical responses) guard
every behavior-preserving rewrite. Frontend changes carry vitest benchmark
gates (verbatim BEFORE replica + equivalence + perf assertion) committed
beside the code so CI re-verifies the win on every run.

Measured on 4 cores / 15 GiB, local PostgreSQL 16 (fsync off), release
profile; frontend on Node 22 / vitest 4 (jsdom). Reproduce any row with the
command in its section.

## Summary

| # | change | key metric | before → after |
|--:|---|---|---|
| 1 | Photos timeline incremental grouping/layout | 50-page (3k-photo) scroll drain | 76 500 → 3 000 group ops (**25.5x**) / 23.0 → 2.2 ms (**10.6x**) |
| 2 | Range-seek per-request authz duplication removed | per-seek authz on a shared-drive scrub | WARM 0.67 → 0 µs/seek; **COLD 1362.66 → 0 µs/seek** (a drive-resolve query per seek) |
| 3 | `/resources` row→DTO name clone → move | allocs/row (500-row page) | 10.004 → 9.004 (**500 allocs saved**, 1.00/row) |

## [1] Photos timeline — O(N²) re-group + re-layout per page → incremental builder

The photos view appended each 60-item page with `items = [...items, ...page]`
and re-derived both `groups` (O(N), a `new Date()` per photo) and `photoRows`
(O(N) row layout) over the whole accumulated list on every page — so paging to
photo N re-grouped + re-laid-out everything loaded so far, Σ ≈ O(N²/60) of
main-thread work during the scroll (the exact class ROUND6 fixed for the files
listing). The DOM was already windowed (`VirtualRows`); this was the derivation
feeding it.

Because photos arrive newest-first (`media_sort_date DESC`), grouping is
append-only: a page only ever extends the last date bucket or adds buckets
after it, never mutates an earlier group. The new `PhotoTimeline`
(`lib/utils/photoTimeline.ts`) exploits that — an append re-buckets only the
fresh page and re-lays-out only the groups that changed, reusing every
untouched group's cached rows; any other change (config, deletion, filter
toggle, non-append) falls back to a full rebuild. The pure `buildPhotoRows` is
the verbatim reference the gate holds it equal to.

Gates: the incremental output is deep-equal to `buildPhotoRows` at EVERY page
of the drain (both square + justified layouts); config-change / deletion /
width=0 fall back to a correct full rebuild; grouping work collapses ≥5x and
wall ≥3x.

```
cd frontend && npx vitest run src/lib/utils/photoTimeline.bench.test.ts --disable-console-intercept
# photo timeline 50×60: before 76500 timestamp reads / 23.0 ms
#                       after   3000 timestamp reads /  2.2 ms
#                       (25.5x fewer grouping ops, 10.6x wall)
```

## [2] Range downloads — duplicate per-seek authz + access-notify removed

`download_file_impl` resolves the file once via `get_file_with_perms` (authz +
access-notify + metadata), then the Range branch called
`get_file_range_preloaded_with_perms`, which re-ran `require_file` (authz) +
`notify_file_accessed` per request. Media players and PDF viewers fetch a file
*exclusively* through Range requests — a `bytes=0-` probe then one request per
seek — so every seek in a scrub re-authorized a file the request-level gate had
already cleared. The share-landing and WebDAV range paths already authorize
once then read via the non-perms `get_file_range_preloaded`; the REST handler
now does the same (and the now-unused `_with_perms` range method is deleted).

Safety: the request-level `get_file_with_perms` still gates every request
(denies before the Range branch runs), so the removed per-seek re-check
bypasses nothing — the bench asserts the member is granted and a non-member
denied.

```
cargo run --release --features bench --example bench_range_seek_authz
# seeks/scrub=200 (member of a shared drive, viewer grant)
# arm                        wall ms    µs/seek
# BEFORE per-seek (WARM)        0.13       0.67   <- moka hit + uuid parse, removed
# BEFORE per-seek (COLD)      272.53    1362.66   <- a grant-cascade drive-resolve
#                                                    QUERY per seek, removed
# AFTER  per-seek (removed)     0.00       0.00
# A 200-seek scrub of a shared video stops paying ~272 ms of authz queries
# when the drive-role cache is cold (cross-drive recipient, or 30 s TTL expiry
# mid-scrub). notify_file_accessed (a throttled hook call) is likewise removed
# per seek.
```

## [3] `/api/folders/{id}/resources` row→DTO mapping — clone name → move name

The listing maps each owned `FolderResourceRow` into a DTO but cloned
`row.name` into it (`name: row.name.clone()`) — one avoidable `String` heap
alloc per listed folder/file. The folder branch uses fixed icon classes, so
`row.name` is simply moved; the file branch computes its name-derived icon /
category classes first (they borrow `&row.name`), then moves `row.name` in. One
fewer alloc per row, identical output.

```
cargo run --release --features bench --example bench_resource_row_map
# rows=500
# arm             allocs   wall ms   allocs/row
# BEFORE (clone)    5002     0.841       10.004
# AFTER  (move)     4502     0.810        9.004
# Saved 500 allocs (1.00/row) — the per-row name clone removed; output identical.
```

## Deferred / flagged (not shipped this round)

- **Thumbnail ACL-before-304 (security posture — needs maintainer decision).**
  `get_thumbnail_impl` runs `require_permission(Read)` before the ETag-304 and
  moka/disk short-circuits, so a shared-album recipient pays a grant-cascade
  query per thumbnail revalidation. Moving authz *after* the cache would make
  thumbnails "authorized at creation time only" — a user whose access was
  revoked could still fetch cached thumbnails of files they once could see.
  That is a deliberate security-posture change, not a perf tweak; left for a
  security review. The safe alternative (back the non-owner authz with the
  existing `drive_role_cache`, or a `Borrow<str>` cache key that removes the
  per-request `to_string`) is queued for round 8 with an alloc/query bench.
- **`batch_operations` `Arc<str>` → `String` per item.** `copy_file_with_perms`
  / `move_file_with_perms` take `Option<String>`, so the batch path's
  `target_folder: Arc<str>` is re-`to_string()`-ed per item, defeating the
  Arc. Widening those `_with_perms` signatures to `Option<&str>` touches the
  trait + impl + stub + ~7 call sites — a contained refactor better done
  deliberately with its own alloc bench; queued for round 8.
- **List-view O(N²) re-derive (favorites / recent / trash / shared-with-me /
  shared swimlanes).** Same class as [1] but on typically-smaller lists;
  each infinite-scroll page re-derives `entries` / `byId` / `sections` /
  `lanes` over the full accumulated set. Deferred — the incremental-builder
  cost isn't yet justified at those sizes; revisit if any surface reaches
  thousands of rows.
- **Serial independent DB pairs → `join!` (token refresh, login, cross-drive
  move, CardDAV discovery, NC PROPFIND enrichment).** Overlapping independent
  round-trips saves 1 RTT *under real PG latency*, but the ROUND6 authz-fan-out
  rejection showed the overhead can wash the win out on local-socket PG. These
  need a decide-by-bench with an injected-latency arm (like the ROUND6 `::text`
  A/B) before adoption — queued for round 8, not guessed at here.

## Correctness-adjacent (surfaced by the round-7 hunt — not perf, flagged for follow-up)

- **`fetchFolderListing` returns empty `favoriteIds`/`sharedIds`**
  (`frontend/src/lib/api/endpoints/folders.ts`) since the combined `/listing`
  route was removed — the files-grid star/shared badges are seeded empty on
  every navigation. The same removal also dropped the 304 conditional
  fast-path, so a folder navigation now pages the full body (`cache: no-store`)
  instead of a bodiless 304 on unchanged folders (mitigated only by the
  in-memory `folderCache`). Functional regression, not perf.
- **Search page lacks a stale-response guard**
  (`frontend/src/routes/search/+page.svelte`): the query `$effect` awaits
  `searchFiles` with no `seq`/AbortController, so a slow stale query can
  resolve after and clobber a newer one. The files view's `loadSeq` is the
  pattern to mirror.
