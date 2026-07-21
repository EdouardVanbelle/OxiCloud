# Round 16 — shares-lane & contextMap incremental builders, folder/href/disposition/preview alloc cuts

Benchmark-gated, same rule as ROUND2–15: every change ships with a
BEFORE/AFTER benchmark and an equivalence/safety gate; an AFTER that doesn't
beat its BEFORE is rolled back (never applied). The roll-back rule is encoded
directly into each harness as a `GATE FAIL … rollback` non-zero exit (Rust) or
a threshold `expect()` (frontend), so a regression fails CI rather than
shipping.

This round finishes the **route-level half** of the O(N²/page) grouped-listing
class ROUND15 §F1 fixed *inside* `ResourceList` — the two remaining producers
that feed it (the "My shares" `lanes` tree and the trash/recent/favorites/
shared-with-me `contextMap`) — and lands a backend CPU/alloc micro-pack of four
per-request allocation cuts surfaced by a fresh hot-path audit.

Measured on 4 cores / 15 GiB, **no PostgreSQL needed for any Round-16 arm**
(frontend: Node 22 / vitest; backend: release counting-allocator examples).
Reproduce any row with the command in its section.

## Summary

| # | change | key metric | before → after |
|--:|---|---|---|
| F1 | "My shares" (`shared/+page.svelte`) `lanes` `$derived.by` re-bucketed the WHOLE accumulated grant list on every infinite-scroll page (and every grant edit); `SharedLanesBuilder` re-emits only the fresh page and reuses each untouched lane's array reference | 50×50 (2 500-item) drain | **63 750 → 2 500 `emit` calls (25.5×)** · **8.8× wall** · O(N²/page) → O(N) |
| F2 | trash / recent / favorites / shared-with-me rebuilt a fresh `Map` (hashing every accumulated id) as `contextMap = $derived(new Map(raw.map(…)))` every page; `primeContextPage` holds one persistent `SvelteMap` and sets only the fresh page's entries (mirrors `favoriteIds`, ROUND14 §F2) | 50×50 drain, 4 routes | **63 750 → 2 500 `entry` calls (25.5×)** · **7.2× wall** · O(N²/page) → O(N) |
| M1 | folder display constants — the trash-listing / NC-search-REPORT / path-resolver folder branch built `Arc::<str>::from("fas fa-folder")` (+ 2 more): 3 heap allocs/row where the sibling file branch already used the interned `Arc` clone | 3 fields/folder row | **3.00 → 0.00 allocs/op**, 1.14× wall |
| M2 | `build_content_disposition` — every download + Range seek built an `encoded` String, an `ascii_safe` String, and the `format!` result (3 allocs); fast-path all-attr-char names + single in-place buffer do it in 1 | 5 names/op | **30.00 → 5.00 allocs/op (6×)**, 2.67× wall |
| M3 | `nc_href` — every NC PROPFIND/REPORT href allocated a per-segment `Vec<Cow>`, a joined String and the `format!`; one pre-sized buffer keeps `urlencoding::encode` (identical bytes) | 5 hrefs/op | **38.00 → 27.00 allocs/op**, 1.44× wall |
| M4 | NC preview `fileId` — the handler `collect()`ed the digit prefix into a String only to reparse it to `i64`; parse the borrowed prefix slice instead | 5 ids/op | **4.00 → 0.00 allocs/op**, 2.51× wall |

## [F1] "My shares" — incremental lanes builder

```
cd frontend && npx vitest run src/lib/utils/sharedLanes.bench.test.ts
```

The shares page pages its outgoing grants in via infinite scroll
(`raw = [...raw, ...page.items]`), and the `lanes` `$derived.by` re-bucketed the
whole accumulated (kind-filtered) list on every page — allocating a fresh lane
object and a fresh `rows` array for *every* lane each time — Σ ≈ O(N²/page)
`emit` calls across a drain. It also re-fired on every grant edit (role/expiry/
password), each of which reassigns `raw`, re-bucketing the entire list for a
one-row change.

`SharedLanesBuilder` (extracted to `$lib/utils/sharedLanes`, off the Svelte
reactive graph so it's unit/benchmark-testable) is the F1-flagship pattern
generalized for the lanes shape, which differs from `ResourceList`'s sections
in two ways: **fan-out** (one grant item contributes rows to *many* lanes in the
"shared with" group-by) and a **header captured at first appearance** (vs a
label recomputed each sync). On an append it re-emits only the fresh page and
hands back the same `rows` array reference for every untouched lane, emitting a
fresh array only for lanes the page actually grew. Any non-append (group-by
switch, grant edit, kind-filter toggle) falls back to a full rebuild, so the
output is always deep-equal to the pure `buildLanes` reference — including the
non-contiguous "shared with" group-by, where a page sprays rows across
already-emitted subject lanes (the same non-monotonic case F1 handled for trash
grouped by drive). The O(1) append test is shared with `resourceSections` via
`isAppendExtension` (extracted this round, re-validated by F1's own gate).

50×50 (2 500-item) drain: **63 750 → 2 500 `emit` calls (25.5× fewer), 8.8×
wall**. Gates: (1) equivalence — deep-equal to `buildLanes` at *every* page for
both the by-files (contiguous) and by-subject (non-contiguous fan-out)
group-bys; (2) reference stability — untouched lanes keep their exact array
reference across an append while a grown lane gets a fresh one; (3) correct
fallback on group-by switch, grant edit and kind-filter toggle; (4) perf — the
deterministic O(N) `emit`-call count, plus a best-of-3 wall ≥3×.

## [F2] Grouped routes — incremental `contextMap`

```
cd frontend && npx vitest run src/lib/utils/listContext.bench.test.ts
```

`/trash`, `/recent`, `/favorites` and `/shared-with-me` each fed `ResourceList`
a per-item `contextMap` (`id → ItemContext`, carrying the envelope's date /
owner / drive fields the group-by and row render read) built as
`$derived(new Map(raw.map((it) => [id, ctx])))` — a brand-new Map re-hashing
every accumulated id on **every** infinite-scroll page. O(N) per page ⇒ Σ
O(N²/page) across a drain, and a fresh instance each page invalidated every
reader. ROUND15 §F1 fixed the `sections` half *inside* `ResourceList`; this is
the route-level projection that feeds it, flagged on ROUND14's deferred list and
never landed.

`primeContextPage` (`$lib/utils/listContext`) applies the shipped `favoriteIds`
shape (ROUND14 §F2, a persistent `SvelteSet` primed per page): each route holds
one persistent `SvelteMap` for the component's lifetime and, in `load()`, clears
it on a reset and sets only the freshly-fetched page's entries — O(page) per
page, O(N) across the drain, one stable instance. The map only ever needs to be
a superset of the displayed ids (rows removed by a delete aren't rendered, so
their stale entries are never read), and every id entering `raw` comes through a
`load()` page, so the map always covers what's on screen. `shared-with-me`
passes a drive-skipping entry (drives never reach the row UI), so its map
matches the displayed `fileFolderGrants` exactly.

50×50 drain: **63 750 → 2 500 `entry` calls (25.5× fewer), 7.2× wall**. Gates:
(1) equivalence — the primed map is deep-equal to a full `new Map(cumulative.map(…))`
rebuild at every page, including skipped drives and the reset path; (2) perf —
the deterministic O(N) entry-call count, plus a best-of-3 wall ≥3×.

## [M1]–[M4] Backend CPU/alloc micro-pack

```
cargo run --release --features bench --example bench_round16_micro
```

Counting-allocator micro-bench; each section is BEFORE (verbatim replica of the
shipped-before shape) vs AFTER (the shipped function itself where reachable —
`intern_display`, `nc_href` — else a verbatim replica of the shipped-after
shape), with a byte/-value equivalence gate and a `GATE FAIL … rollback` exit.

- **[M1] Folder display constants → interned clone.** The trash-listing
  (`trash_service.rs`), NC-search-REPORT (`report_handler.rs`) and path-resolver
  (`path_resolver_service.rs`) folder branches each built
  `Arc::<str>::from("fas fa-folder")` + `"folder-icon"` + `"Folder"` — three
  heap allocations + memcpys per folder row — although all three literals are in
  the `DISPLAY_INTERN` closed set and the **file branch of the very same
  function** already used `intern_display` (a lookup + refcount bump, 0 allocs).
  ROUND11 interned the file classifiers on these paths but missed the folder
  constants. Per trashed / searched / resolved folder row: **3.00 → 0.00
  allocs/op, 1.14× wall**.
- **[M2] `build_content_disposition` 3 → 1 alloc.** Called on every download and
  every Range seek (media/PDF scrubbing pays it per seek), it built a
  percent-`encoded` String, an `ascii_safe` filtered String, and the `format!`
  result — 3 allocations. The shipped code fast-paths an all-attr-char name
  (`filename` and `filename*` are the name verbatim → one `format!`) and, for
  names needing encoding, writes the ASCII fallback and percent-encoded form
  into a single pre-sized buffer. Byte-identical across ASCII / spaced / unicode
  / quote+backslash names: **30.00 → 5.00 allocs/op (6×), 2.67× wall** (5
  names/op, a fast/slow mix).
- **[M3] `nc_href` Vec+join → single buffer.** Every NC PROPFIND/REPORT href
  allocated a per-segment `Vec<Cow>`, a joined String and the `format!` result;
  the native WebDAV side already fixed this exact shape (`encode_uri_path`). The
  shipped code writes the prefix, user and each encoded segment straight into one
  pre-sized buffer, keeping `urlencoding::encode` so the emitted bytes are
  unchanged (incl. root trailing slash and internal `//`): **38.00 → 27.00
  allocs/op, 1.44× wall** (5 hrefs/op — the Vec + join + format drop; the
  per-segment encode Cows, unavoidable, remain).
- **[M4] NC preview `fileId` borrow-slice parse.** The preview handler
  `collect()`ed the leading digit run into a String only to reparse it to `i64`;
  the shipped code finds the digit-prefix length and parses the borrowed slice —
  0 allocations. Per NC thumbnail request (a gallery fires one per tile): **4.00
  → 0.00 allocs/op, 2.51× wall** (5 ids/op).

## Not shipped — deferred to a later round

Surfaced by the Round-16 audit but not landed (each wants its own decision,
Postgres fixture, or a larger change):

- **Backend query-shape (needs Postgres):** carried forward from ROUND15 —
  `music_storage_adapter::list_public_playlists` 1 + N `COUNT(*)` fold; contact
  REST listings over-fetching the multi-KB `vcard` TEXT (wants a *lite* row
  mapper).
- **Backend CPU/alloc (no Postgres, next micro-pack):** the two WebDAV PROPFIND
  surfaces still quote `d:getetag` into a fresh String per row and `format!` the
  per-row href per child (the CalDAV §A6 reused-buffer treatment never reached
  them); `delta_upload_service` → `hash_chunk_sequence` clones every chunk hash a
  second time (`.iter().cloned()` on an already-owned Vec — change the signature
  to take it by value); `contact_to_vcard` seeds from a 27-byte String and
  `.to_uppercase()`-allocates each TYPE token.
- **Frontend (vitest-benchmarkable):** `VirtualRows.offsets` prefix-sum is
  rebuilt in full on every photos-timeline page (residual O(N²) on the hottest
  scroll surface — an incremental extend needs care to keep the downstream
  `$derived` reference-invalidation correct); the dotfile filter and
  `ResourceList.itemIndexById` re-scan the whole accumulated list per page (both
  conditional — hide-dotfiles on / an active selection — hence lower priority).

## Environment / methodology

- `cd frontend && npx vitest run src/lib/utils/sharedLanes.bench.test.ts`
  and `… listContext.bench.test.ts` — Node 22 / vitest, no Postgres. Wall gates
  take the best-of-3 (min) per arm to shrug off scheduler/GC noise under a
  saturated runner (round14 §F1 pattern); the deterministic O(N) call-count is
  the primary rollback gate.
- `cargo run --release --features bench --example bench_round16_micro`
  — counting allocator, no Postgres (`BENCH_ITERS`).
- Roll-back rule encoded per harness: the Rust example `std::process::exit(1)`
  with `GATE FAIL … rollback` if an AFTER arm fails to reduce allocations; the
  vitest gates `expect()` the O(N) call count and the ≥3× wall.
