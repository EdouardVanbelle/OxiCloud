# Round 14 — narrow projections, per-request auth allocations, CalDAV read-emitter buffers, frontend set churn

Benchmark-gated, same rule as ROUND2–13: every change ships with a
BEFORE/AFTER benchmark and an equivalence/safety gate; an AFTER that doesn't
beat its BEFORE is rolled back (never applied). The roll-back rule is encoded
directly into each harness as a `GATE FAIL … rollback` non-zero exit (Rust) or
a threshold `expect()` (frontend), so a regression fails CI rather than
shipping.

This round is a broad micro-sweep: one over-fetch on the People lightbox path,
five per-request allocations on the authenticated `/api` + DAV hot path, the
CalDAV read-emitters (which never got the allocation treatment their CardDAV
twin already ships), and two frontend per-page set-churn fixes.

Measured on 4 cores / 15 GiB, local PostgreSQL 16 (release profile for the Rust
examples; Node 22 / vitest 4 for the frontend). Reproduce any row with the
command in its section.

## Summary

| # | change | key metric | before → after |
|--:|---|---|---|
| Q1 | Lightbox face boxes — narrow `SELECT id, person_id, bbox` with the caller filter in SQL, vs hydrating the full 10-column row (incl. the 2 KiB `embedding` BYTEA, decoded per face) and filtering in Rust | 15-face group photo | **0.312 → 0.219 ms (1.43×)** · 32 040 → 840 B/req (38× less wire, scales with face count) |
| A1 | Cookie auth reads the access token with the borrow-only `extract_cookie_str` (already backs CSRF) instead of `extract_cookie_value`'s owned `String` | per cookie-authed `/api` req | 157.7 → 146.5 ns · **1 → 0 allocs** |
| A2 | `compute_relevance` ASCII case-fold fast path vs `name.to_lowercase()` per result row (Unicode fallback preserved) | 12-row result page | **661.9 → 473.6 ns (1.40×)** · 12 → 3 allocs |
| A3 | `sub` pre-parsed to `Uuid` at decode time vs re-parsing the 36-char claim on every request (even cache hits) | per authed req | **22.7 → 0.7 ns (32.9×)** CPU |
| A4 | Auth middleware borrows `request.headers()` instead of taking axum's `HeaderMap` extractor (a full map clone) — JWT **and** NextCloud paths | per authed `/api`+DAV+NC req | **239.1 → 7.6 ns (31.5×)** · **2 → 0 allocs** |
| A5 | CalDAV `getlastmodified` via the stack `rfc2822_utc` (byte-identical to chrono) vs `updated_at.to_rfc2822()` heap `String` per event | 5 events | 178.2 → 148.7 ns · **5 → 0 allocs** |
| A6 | CalDAV per-event `href` + quoted `etag` written into reused page buffers vs a fresh `format!` `String` pair per event | 40-event page | **8 399 → 2 417 ns (3.48×)** · **240 → 6 allocs** |
| F1 | `t()` shares one frozen `EMPTY_PARAMS` for the no-interpolation call forms vs a throwaway `{}` per call | 4M no-param calls | 34.1 → 28.8 ms (1.18×) · −1 alloc/call |
| F2 | Favorites `favoriteIds` is a persistent set with per-page `add` vs a brand-new `SvelteSet` over the whole accumulated list each infinite-scroll page | 40-page drain | **35.5 → 1.6 ms (22.3×)** · O(N²) → O(N) |

## [Q1] Lightbox face boxes — narrow projection + SQL-side caller filter

```
cargo run --release --features bench --example bench_round14_queries   # §Q1
```

`GET /api/people/faces/{file_id}` fires on every lightbox open of a
face-tagged photo. `people_service::faces_for_file` (the sole caller of the
repo method) builds `FaceBoxDto { id, person_id, x,y,w,h }` — it reads **only**
`id`, `person_id`, `bbox`. But the repo's `faces_for_file` selected all ten
columns, dragging the 2 048-byte `embedding` BYTEA (`512 × f32`) across the
wire **and decoding it into a `Vec<f32>` per face** (`row_to_face`), plus five
more unused columns, then filtered `user_id == caller` in Rust. For a group
photo that is ~2.1 KB/face fetched where ~40 B is needed.

The fix mirrors the already-accepted `person_face_stats` narrowing (the port
doc there already cites "the 2 KiB embedding BYTEA per row"): a new
`face_boxes_for_file(file_id, user_id)` port method selects only
`id, person_id, bbox` and pushes the caller scope into `WHERE user_id = $2`
(driven by `idx_faces_file`), returning a lightweight `FaceBox`. 15-face group
photo: 0.312 → 0.219 ms, 32 040 → 840 B/req; the margin widens with face count
and is larger over a networked PG. Gate: the `{(id, person_id, bbox)}` set is
byte-identical before/after (all 15 faces), and the caller scope is preserved
(now enforced in SQL rather than a Rust `.filter`).

## [A1]–[A6] Auth + CalDAV micro-pack

```
cargo run --release --features bench --example bench_round14_micro   # §A1–§A6
```

Counting-allocator micro-bench; each section is BEFORE (the shipped shape, or
the shipped function itself) vs AFTER, with a byte-identity/equivalence gate.

- **[A1] Cookie token extract.** `auth_middleware`'s cookie arm called
  `extract_cookie_value` → an owned `String` whose only use is to be reborrowed
  as `&str` into `validate_token`. The borrow-only twin `extract_cookie_str`
  already exists (it backs the CSRF middleware, ROUND11 §6). Swapped: −1 alloc
  on every SPA/browser `/api` request. Gate: byte-identical value.
- **[A2] `compute_relevance` ASCII fast path.** The query side was already
  hoisted, but the *name* side still did `name.to_lowercase()` (full Unicode)
  per result row — and per keystroke on the suggest path. For the
  overwhelmingly common all-ASCII filename that is pure waste. New path:
  `eq_ignore_ascii_case` + an allocation-free ASCII case-insensitive
  `starts_with`/`contains`; non-ASCII names fall back to the exact
  Unicode-lowercase comparison. 12-row page: 1.40×, 12 → 3 allocs. Gate: the
  ASCII path equals the Unicode path across a mixed ASCII/`é`/`ß`/`ï` corpus
  (exact/prefix/substring/miss).
- **[A3] `sub` → `Uuid` pre-parse.** `TokenClaims.sub` is a `String`; the
  middleware re-ran `Uuid::parse_str` on the 36-char subject on *every* request,
  downstream of the validation cache (which returns the same
  `Arc<TokenClaims>`), so the parse repeated on ~all-hit steady state. A new
  `sub_id: Uuid` is parsed once in `From<JwtClaims>` (amortized over the cache
  TTL); the middleware reads a `Copy`. 22.7 → 0.7 ns. A verified token we signed
  always carries a UUID sub; the nil sentinel is rejected defensively, exactly
  like the old parse-error branch. Gate: pre-parsed `sub_id` equals a fresh
  parse.
- **[A4] Drop the `HeaderMap` clone.** Both `auth_middleware` (JWT/Basic/cookie
  — all `/api`, WebDAV, CalDAV, CardDAV) and the NextCloud
  `basic_auth_middleware` took axum's `HeaderMap` extractor, i.e. a full
  `parts.headers.clone()` (~2 allocs) per request, purely to *read* the
  Authorization/Cookie headers. Removed; the middleware borrows
  `request.headers()` directly. This is a borrow restructuring, not a logic
  change: the header borrow is dead (NLL) by the time each arm reaches
  `request.extensions_mut()` / `next.run(request)`, so no owned copy is needed
  and the auth decisions are byte-identical. 239.1 → 7.6 ns, 2 → 0 allocs — the
  single highest-reach allocation removed this round. Gate: the token extracted
  from a cloned map equals the token from the borrowed map.
- **[A5] CalDAV `getlastmodified` stack render.** The CalDAV read-emitters
  (`write_report_page` → event props, `write_collection_event_page`, and the
  two per-calendar prop writers) formatted `updated_at.to_rfc2822()` into a
  fresh heap `String` per event — up to `CALDAV_STREAM_PAGE_EVENTS = 500` per
  page, on the REPORT (`calendar-query`/`multiget`/`sync-collection`) and
  collection-PROPFIND paths every client polls constantly. The CardDAV twin
  already replaced exactly this with the `[u8; 31]` stack renderer
  `common::fmt::rfc2822_utc` (ROUND10 §13), parity-tested byte-for-byte against
  chrono across 60 years, with the chrono fallback for out-of-4-digit-year
  timestamps. Ported via a shared `write_lastmodified_text` helper at all five
  sites: 5 → 0 allocs. Gate: stack render byte-identical to `to_rfc2822`.
- **[A6] CalDAV per-event `href` + `etag` reused buffers.** Same emitters
  allocated a fresh `format!("{}{}.ics", …)` href and a `format!("\"{}\"", id)`
  quoted etag `String` **per event**. The CardDAV emitter already reuses a
  single page buffer (`clear()` + `write!`). Ported: `write_report_page` /
  `write_collection_event_page` hold reusable `href` + `etag` buffers threaded
  through `write_event_response` into the prop writers (its only caller), so a
  40-event page allocates that storage once, not 80 times. 3.48×, 240 → 6
  allocs/page. Gate: reused-buffer bytes identical to the per-event `format!`.

## [F1][F2] Frontend set/alloc micro-pack

```
cd frontend && npx vitest run src/lib/components/round14.bench.test.ts
```

- **[F1] `t()` shared empty params.** The ubiquitous `t('k', 'Fallback')` and
  bare `t('k')` (default `= {}`) allocated a throwaway params object on every
  call, though for a cache-hit string with no `{{…}}` `interpolate` returns
  before reading params. `t()` runs ~10×/row. Hoisted one frozen
  `EMPTY_PARAMS`; 4M no-param calls 34.1 → 28.8 ms (the alloc reduction shows
  as ~1.18× even on V8's cheap young-gen `{}`). Gate: identical output for the
  bare / string-fallback / params forms; perf gate requires the shared arm be
  no slower.
- **[F2] Favorites `favoriteIds` incremental set.** The favorites route derived
  `favoriteIds = new SvelteSet(items.map(i => i.id))`. Every infinite-scroll
  page (`raw = [...raw, ...page]`) rebuilt a **brand-new** set over the whole
  accumulated list — O(N) per page, O(N²) across a drain — and, being a new
  instance each page, invalidated every mounted star reader. Since every item
  on this page is a favorite and removed items aren't rendered, the set only
  has to be a *superset* of the displayed ids, so the fix keeps one persistent
  `SvelteSet` and `add`s only the fresh page's ids (`clear` on reset, `delete`
  on unfavorite) — the shape `recent` already ships (`replaceSet`, ROUND6). A
  40-page × 50 drain: 35.5 → 1.6 ms (22.3×). Gate: final membership identical
  to the rebuild-per-page model.

## Not shipped — investigated, deferred, or flagged

Every item below was surfaced and verified this round but deliberately left
out of the benchmark-gated set — either it needs a decision the perf pass
can't make, or it isn't cleanly wall-benchable, or it's a correctness bug that
must not ride a perf banner.

### Query-shape (needs Postgres; verified, deferred)
- **`music_storage_adapter::list_public_playlists` 1 + N `COUNT(*)`** — one
  `SELECT COUNT(*) FROM audio.playlist_items` per playlist (up to 101
  round-trips at `limit=100`). Foldable into one `LEFT JOIN … GROUP BY`. It's
  the public-gallery path (`include_public` defaults false), so opt-in; queued
  with its bench. Its two dead siblings `list_playlists_by_owner` /
  `list_shared_with_user` carry the same N+1 with **no live caller** (replaced
  by `get_playlists_by_ids` post-ROUND3) — flag for deletion, not optimization.
- **Contact REST listings over-fetch the `vcard` TEXT** — `search_contacts`,
  `get_contacts_by_address_book_paginated`, and `get_contacts_in_group` select
  the full serialized card (the largest column; multi-KB with an embedded
  `PHOTO;ENCODING=b`), but every caller maps to `ContactDto`, which has **no**
  `vcard` field. Wants a *lite* row mapper (the non-paginated sibling is shared
  with the CardDAV stream, which genuinely needs `vcard`), so it's a contained
  refactor rather than a blanket SELECT change.

### CPU/alloc (verified, deferred or below the noise floor)
- **`content_index_worker`**: (a) clones the full extracted text into the
  per-batch `text_by_hash` map even for unique blobs (dead clone in the
  common one-file-per-blob case; hold `Arc<str>` or gate on multiplicity);
  (b) calls `text_extractor::supports()` (which lowercases MIME + extension,
  1–2 allocs) **twice per file** per drain batch. Both are reseed-throughput,
  not request-latency — worth one worker micro-bench of their own.
- **`tantivy_content_index::search_blocking` builds a `SnippetGenerator` even
  when there are zero hits** — trivial `if top_docs.is_empty() { return … }`.
- **`exif_service` double-allocates** on Make/Model/GPS-ref
  (`display_value().to_string().trim_matches('"').trim().to_string()` — the
  intermediate `to_string` is thrown away). Per-image, background.
- **REST calendar-event edit** re-`format!`s the whole `ical_data` body once
  per changed property (`update_ical_property` / `remove_ical_property`), so a
  6-field PATCH reallocates the body ~7×. Per-edit (rare vs CalDAV reads);
  wants one working buffer.

### Storage I/O (cached-remote deployment class; verified, deferred)
- **`CachedBlobBackend` re-runs `fs::create_dir_all(prefix)` per cache write**
  — unlike `LocalBlobBackend::initialize`, which pre-creates all 256 prefix
  dirs; a cached-remote upload pays a redundant `mkdir(EEXIST)+stat` + blocking
  dispatch per chunk. Pre-create at init and drop the hot-path call.
- **`CachedBlobBackend` eviction listener `std::fs::remove_file` on the reactor
  thread** — moka delivers the listener on the calling (tokio worker) thread,
  so at steady state each write-through insert unlinks a victim inline (p99
  stall). Hand the unlink to `spawn_blocking` / a drain task.
- **`dedup_service` hash-`String` re-allocations** — `distinct_hashes` rebuilds
  a set the streaming loop already had (`session_seen`); `settle_batch` clones
  every batch hash to bind the pin query. Alloc-count only (within noise on a
  throughput bench); report as such.
- **`encrypted_blob_backend` emits 64 KiB plaintext frames** where every other
  backend streams 256 KiB (the comment claiming parity is wrong) → 4× frames on
  decrypted reads; AES dominates, so likely within noise — verify before
  shipping.

### Frontend (bigger refactors — their own pass)
- **`ResourceList.sections` re-buckets the whole accumulated list per page**
  (O(N²) across a grouped-view drain; trash is grouped-by-default and its
  `bucketOf` does `Date` math per item). The fix is the proven `PhotoTimeline`
  incremental-builder pattern (persistent `Map` + append-detection); it's the
  flagship follow-up, same class as the ROUND13-deferred "unify all four
  listing arms onto one `VirtualRows`".
- **`shared/+page.svelte` rebuilds the full `lanes` tree** per page **and** on
  every single grant edit (`raw = [...raw]` to force reactivity); and the
  favorites/recent/trash routes re-project `items`/`contextMap` per page.
  Co-solved by the same incremental builder.

### Already done / correctness (not perf)
- **JWT-claims `Arc<str>`** — the ROUND6/ROUND9-deferred "cheapest known win on
  the /api path" was **already shipped in ROUND10** (`TokenClaims.username`/
  `email: Arc<str>`, `CurrentUser` build = 1 alloc). The residual `Arc::new`
  is structurally required (shared with `NcSession`). Do not re-open.
- **Media hooks' raw blob reads are broken, not merely duplicated** (ROUND13
  finding stands): `MediaMetadataService` / `FaceIndexingService` read
  `.blobs/{hash}.blob` directly, which only exists for local + unencrypted +
  single-chunk blobs — silently no capture-date/GPS/faces for the common case.
  Correctness fix (route through `read_blob_bytes`), perf-neutral-to-negative;
  a shared-`Bytes` provider is the perf follow-up once it lands.
- **`calendar_event_pg_repository::list_events_by_calendar_paginated`** selects
  the stale 13-column shape (omits `recurrence_id`), flattening exception
  overrides into masters on paginated listings — a latent correctness bug, not
  a perf win.

## Environment / methodology

- `cargo run --release --features bench --example bench_round14_queries`
  — needs Postgres; seeds + cleans its own fixtures (`BENCH_PASSES`,
  `BENCH_FACES_PER_FILE`).
- `cargo run --release --features bench --example bench_round14_micro`
  — counting allocator, no Postgres (`BENCH_ITERS`).
- `cd frontend && npx vitest run src/lib/components/round14.bench.test.ts`.
- Cross-round guards unchanged (`bench_round10_micro`/`_queries` updated for the
  `TokenClaims.sub_id` field and the narrowed faces read-back).
