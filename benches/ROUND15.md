# Round 15 — grouped-listing O(N²) rebucket, exif/reseed allocations, tantivy zero-hit snippet skip

Benchmark-gated, same rule as ROUND2–14: every change ships with a
BEFORE/AFTER benchmark and an equivalence/safety gate; an AFTER that doesn't
beat its BEFORE is rolled back (never applied). The roll-back rule is encoded
directly into each harness as a `GATE FAIL … rollback` non-zero exit (Rust) or
a threshold `expect()` (frontend), so a regression fails CI rather than
shipping.

This round lands the ROUND14-deferred **flagship** — the grouped-listing
`sections` rebuild that was the last O(N²)-per-page accumulation left in the
SvelteKit listing surfaces — plus three backend items pulled from the same
deferred list: two allocation cuts on the photo-ingest / content-reseed worker
paths, and a wasted `SnippetGenerator` build removed from the zero-hit content
search path.

Measured on 4 cores / 15 GiB, local PostgreSQL not needed for any Round-15 arm
(all no-Postgres: release profile for the Rust examples; Node 22 / vitest for
the frontend). Reproduce any row with the command in its section.

## Summary

| # | change | key metric | before → after |
|--:|---|---|---|
| F1 | Grouped listings (trash / recent / favorites / shared-with-me) re-bucketed the WHOLE accumulated list on every infinite-scroll page; `ResourceSectionsBuilder` re-buckets only the fresh page and reuses each untouched bucket's array reference | 50×50 (2 500-item) drain, month buckets | **63 750 → 2 500 `bucketOf` calls (25.5×)** · **12.5 → 1.3 ms wall (9.9×)** · O(N²/page) → O(N) |
| B1 | exif `Make`/`Model` — `display_value().to_string().trim_matches('"').trim().to_string()` throws the display `String` away to allocate the trimmed copy; the in-place `drain`+`truncate` helper keeps one allocation | 4 sample values/op | **150.4 → 119.4 ns (1.26×)** · **8 → 4 allocs/op** (2 → 1 per field) |
| B2 | Content-index worker called `text_extractor::supports` (lowercases MIME + extension) TWICE per file per drain batch; classify once into a `Vec<bool>` and thread it through both uses | 256-file reseed batch | **34.5 → 16.7 µs (2.07×)** · **704 → 353 allocs/op** |
| B3 | Zero-hit content search still built a `SnippetGenerator` (query-compile + term weighting) though the per-hit loop was empty; return `Ok(vec![])` as soon as `top_docs.is_empty()` | no-hit query, 400-doc index | **1 575.6 → 1 237.2 ns (1.27×)** · **21 → 19 allocs/op** (widens with index size) |

## [F1] Grouped listings — incremental swimlane builder

```
cd frontend && npx vitest run src/lib/utils/resourceSections.bench.test.ts
```

Every grouped listing page (`/trash`, `/recent`, `/favorites`,
`/shared-with-me`) loads its rows via infinite scroll (`raw = [...raw,
...page]`), and `ResourceList`'s `sections` `$derived.by` re-bucketed the
**whole accumulated list** on every page: Σ ≈ O(N²/page) `bucketOf` + `ctxOf`
calls across a drain, and a brand-new rows array for *every* bucket each page
(so `VirtualList` re-diffed every swimlane every page). This was the ROUND14
"flagship follow-up" — the same O(N²)-per-page class ROUND6 fixed for the files
listing, ROUND14 §F2 for favorites' `favoriteIds`, and `PhotoTimeline` for the
photos grid.

`ResourceSectionsBuilder` (extracted to `$lib/utils/resourceSections`, off the
Svelte reactive graph so it's unit/benchmark-testable) exploits the append
invariant: a grouped listing is server-sorted by the active group's `orderBy`,
so a fresh page only ever extends existing buckets or appends new ones. It
detects the append (prefix-identity on the boundary object), re-buckets only
the fresh page, and hands back the **same array reference** for every untouched
bucket — the property `VirtualList` (which diffs its `items` prop by reference)
relies on to skip re-rendering it — while emitting a fresh array only for
buckets the page actually grew. Any non-append (group-by switch, deletion,
dotfile-filter toggle) falls back to a full rebuild, so the output is always
deep-equal to the pure `buildResourceSections` reference.

Correctness does **not** depend on bucket contiguity: the one non-monotonic
group-by in the set — trash grouped **by drive** but ordered by name, so a page
sprays items across every already-emitted drive bucket — stays byte-for-byte
equal to the full rebuild (it just refreshes more buckets per page). Header
labels are recomputed every sync (never cached): a group-by's `labelOf` can
resolve asynchronously (owner / sharer names arrive after the rows), and a
cached label would freeze the header at its fallback.

50×50 (2 500-item) month-bucketed drain: **63 750 → 2 500 `bucketOf` calls
(25.5× fewer), 12.5 → 1.3 ms wall (9.9×)**. Gates: (1) equivalence — the
incremental output is deep-equal to `buildResourceSections` at *every* page for
both a contiguous (date) and a non-contiguous (drive) group-by; (2) reference
stability — untouched buckets keep their exact array reference across an append
while a grown bucket gets a fresh one; (3) correct fallback on group-by switch,
deletion and the flat pass-through; (4) perf — `bucketOf` work is exactly O(N)
across the drain and wall drops ≥3×.

## [B1]–[B2] exif / content-reseed allocation cuts

```
cargo run --release --features bench --example bench_round15_micro
```

Counting-allocator micro-bench; each section is BEFORE (the shipped-before
shape) vs AFTER (the shipped function / shape) with a byte-identity gate.

- **[B1] exif `Make`/`Model` single-allocation trim.** `ExifService::extract`
  read the camera make + model as
  `field.display_value().to_string().trim_matches('"').trim().to_string()` —
  the first `to_string()` materializes the display value (unavoidable), then
  `.trim_matches('"').trim().to_string()` allocates a **second** `String` for
  the trimmed copy and drops the first. The new `display_value_trimmed` applies
  the same two-stage trim in place on the already-owned buffer (`drain` drops
  the stripped prefix, `truncate` the suffix — both reuse the allocation), so a
  quoted `"Canon"` costs one allocation instead of two. Per ingested photo (the
  Make + Model fields). 4 sample values/op: **8 → 4 allocs/op (2 → 1 per
  field), 1.26× wall**. Gate: byte-identical to the old chain across quoted /
  padded / clean shapes.
- **[B2] Content-index worker single `supports()` classify.** `supports`
  lowercases the MIME (and, on a generic MIME, the extension) — 1–2 allocations
  — and the drain loop called it **twice per file**: once in the
  `wanted_hashes` filter, once again in the per-file records loop. The worker
  now classifies each file once into a `Vec<bool>` and threads the flag through
  both. On a full reseed that is one redundant classify (and its allocations)
  removed for *every file in the library*. 256-file batch: **704 → 353
  allocs/op, 34.5 → 16.7 µs (2.07×)**. Gate: the `(wanted, supported)` tallies
  are identical before/after.

## [B3] Tantivy zero-hit snippet skip

```
cargo run --release --features bench --example bench_round15_tantivy
```

`TantivyContentIndex::search_blocking` built the `SnippetGenerator` from the
query right after the `TopDocs` search — but `SnippetGenerator::create`
compiles the query against the index (collects query terms, looks up each
term's document frequency, builds the weighting), and when the query matched
**no** documents that generator is never used: the per-hit loop is empty. A
content search for a term that isn't in any indexed document (a common miss)
paid that build for nothing on the request path.

The fix returns `Ok(Vec::new())` the moment `top_docs.is_empty()`, before the
create. The bench reproduces the exact skipped operation on a RAM index built
with the public tantivy API (same crate + version): BEFORE = search + create,
AFTER = search + the `is_empty()` early return; the delta is the wasted create.
No-hit query against a 400-document index: **1 575.6 → 1 237.2 ns (1.27×), 21 →
19 allocs/op** — the create adds ~338 ns + 2 allocs on top of the search on
*every* zero-hit content query, and its per-term `doc_freq` lookups grow with
the index (the RAM bench's 400 docs understate the production term dictionary).
Gates: the miss query genuinely returns zero hits, and a control arm confirms a
term that *does* hit still yields a snippet fragment (the skip only ever
triggers on a true zero-hit query).

## Not shipped — carried forward from the ROUND14 deferred list

Still queued, unchanged in scope (each wants its own decision, Postgres
fixture, or bigger refactor):

- **Query-shape (needs Postgres):** `music_storage_adapter::list_public_playlists`
  1 + N `COUNT(*)` fold (opt-in public-gallery path); contact REST listings
  (`search_contacts`, `get_contacts_by_address_book_paginated`,
  `get_contacts_in_group`) over-fetch the multi-KB `vcard` TEXT though every
  caller maps to a `ContactDto` with no `vcard` field (wants a *lite* row
  mapper, since the non-paginated sibling is shared with the CardDAV stream).
- **Frontend:** `shared/+page.svelte` rebuilds the full `lanes` tree per page
  and on every grant edit; the same incremental-builder pattern F1 uses is the
  follow-up. (F1 removed the `ResourceList.sections` half of the ROUND14
  "flagship" bullet; the `lanes` half remains.)
- **CPU/alloc (background):** REST calendar-event edit re-`format!`s the whole
  `ical_data` body once per changed property; `dedup_service` hash-`String`
  re-allocations; `exif_service` still double-allocates the GPS-ref display in
  `parse_gps_coord` (single-alloc, low-frequency — folded into B1's helper is
  possible but the ref is compared to `"S"`/`"W"` as a borrow, so it never
  needed the second alloc the Make/Model path did).
- **Storage I/O (cached-remote class):** `CachedBlobBackend` per-write
  `create_dir_all` + inline eviction `remove_file` on the reactor thread;
  `encrypted_blob_backend` 64 KiB vs 256 KiB plaintext frames.

## Environment / methodology

- `cargo run --release --features bench --example bench_round15_micro`
  — counting allocator, no Postgres (`BENCH_ITERS`, `BENCH_BATCH`).
- `cargo run --release --features bench --example bench_round15_tantivy`
  — builds a RAM tantivy index, no Postgres (`BENCH_ITERS`, `BENCH_DOCS`).
- `cd frontend && npx vitest run src/lib/utils/resourceSections.bench.test.ts`.
- Roll-back rule encoded per harness: the Rust examples `std::process::exit(1)`
  with `GATE FAIL … rollback` if an AFTER arm fails to beat its BEFORE; the
  vitest gate `expect()`s the O(N) call count and the ≥3× wall.
