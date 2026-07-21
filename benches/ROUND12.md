# Round 12 — auth write-path narrowing, fused quota gate, moka blob-cache index, media single-read, sized listing JSON

Benchmark-gated, same rule as ROUND2-11: every change ships with a
BEFORE/AFTER benchmark and an equivalence/safety gate; an AFTER that doesn't
beat its BEFORE gets rolled back or redesigned. One candidate went through
exactly that loop this round (§Rejected): the single-pass compression
predicate — the profiler-plausible "28 redundant Content-Type reads" turned
out to cost ~4.6 ns TOTAL once monomorphized, and the fused replacement
measured within noise, so the declarative chain stays.

Measured on 4 cores / 15 GiB, local PostgreSQL 16 (fsync off), release
profile; frontend on Node 22 / vitest 4. Reproduce any row with the command
in its section.

## Summary

| # | change | key metric | before → after |
|--:|---|---|---|
| Q1 | NC sharee search: username-only projection (was 21 wide columns incl. the ≤512 KiB avatar per match) | 26-row page, 3 000 users, all matches avatared | 11.77 → 2.37 ms (**4.98x**) |
| Q1b | + `gin_trgm_ops` indexes on `auth.users` (migration 20260719000000) | same page, leading-wildcard ILIKE | → 0.215 ms (**54.7x** total) |
| Q2 | Password login: redundant full-row `update_user` deleted (`create_session` already stamps `last_login_at`) | ms/login, 256 KiB avatar | 2.96 → 0.67 (**4.45x**) · −1 txn, −17-column rewrite, −512 KiB clone |
| Q3 | Email-verified stamp → narrow conditional UPDATE (magic-link) | ms/stamp | 2.20 → 0.25 (**8.9x**) |
| Q3b | OIDC repeat login → in-memory compare, sync only on change | queries per repeat login | full-row rewrite (2.37 ms) → **0 queries** |
| Q4 | Refresh-token rotation: 2 transactions → 1 (`rotate_session`) | ms/rotation | 1.135 → 0.959 (**1.18x**) |
| Q5 | WOPI CheckFileInfo triple → `tokio::join!` (real `PgAclEngine`) | ms/call | cold 0.485 → 0.363 (**1.34x**) · warm 0.228 → 0.209 |
| Q6 | Upload quota pair → ONE fused read (user envelope + drive cap) — NC chunk PUT pays it per chunk | ms/check | 0.350 → 0.193 (**1.81x**) · 2 → 1 queries/chunk |
| M1 | Listing JSON: pre-sized buffer (`sized_json`) vs axum `Json`'s 128 B seed | 500-row page | 282.4 → 201.0 µs (**1.40x**) · 13 → 2 allocs |
| M3 | Security headers: 4 `SetResponseHeaderLayer` + CSP middleware → 1 fused pass | per request (incl. router) | 5.35 → 3.74 µs (**1.43x**) · −26 allocs |
| M4 | Media capture-metadata: single-read (images were read 2-3×, videos opened 2×) | warm geomean / cold cache | **1.44x** warm · **1.6-3.2x** cold · opens 2-3 → 1 |
| M5 | Chunked-upload session ops: 5 → 3 map lookups + stack-encoded uuid compare | ns per chunk (prepare+commit) | 469 → 366 (**1.28x**) · −2 allocs |
| B1 | Blob-cache index: `Mutex<LruCache>` → moka byte-weigher | pure index probes, K readers | K=2 **2.17x**, K=4 1.61x, K=8 1.46x (mutex scaled NEGATIVELY: 2.08 → 1.07 Mops/s from 1 → 2 readers) |
| B2 | `put_blob` populates the cache BEFORE the inner backend consumes the source (was: after → failed 100%) | first read after whole-file put | full remote re-download → local hit |
| F1 | SPA list view: 150 px `icon` thumbnails (was 400 px `preview` into a 40 px slot) | pixels per list thumbnail | **~7.1x fewer** (≈4-5x fewer bytes) |

## [Q1] NC sharee search — the 512 KiB-per-row autocomplete

```
cargo run --release --features bench --example bench_round12_queries   # §1
```

`handle_sharees_search` fired `search_users` per keystroke — the full
21-column row (incl. the ≤512 KiB avatar data-URI `image`, TOAST-detoasted
per match) hydrated into `User` → `UserDto`, of which the handler read ONLY
`username`. And the leading-wildcard `ILIKE '%q%'` had no trigram index, so
every keystroke seq-scanned `auth.users` (contacts/files/folders all have
`gin_trgm_ops`; users was the gap). Now: `search_usernames` port method
(same WHERE/ORDER/LIMIT, username-only projection; NULL usernames filtered
app-side exactly like the wide flow's post-limit filter) + the two trgm
indexes. Gates: identical username lists, with and without the indexes.
The wide method stays for the admin table (which serializes `image`).

## [Q2][Q3][Q3b] Auth write-path narrowing

```
cargo run --release --features bench --example bench_round12_queries   # §2-3
```

- **Login** ran `update_user(user.clone())` — a transaction rewriting all
  17 columns (incl. the avatar, plus a 512 KiB deep clone to feed it) —
  purely to persist `last_login_at`… which `create_session` overwrites in
  its own transaction three lines later. Nothing reads the row in between
  (verified). The call is deleted; the in-memory `register_login()` stays
  so the response DTO carries the timestamp.
- **Magic-link redemption** kept its `update_user` for the email-verified
  stamp only (last-login again covered by `create_session`) — now a narrow
  `WHERE … AND email_verified_at IS NULL` single-column UPDATE, idempotency
  moved into SQL (gated: second stamp is a 0-row no-op, first timestamp
  preserved).
- **OIDC repeat login** additionally syncs the IdP avatar. The row fetched
  by `get_user_by_oidc_subject` already carries the stored avatar +
  verification stamp, so the service now compares IN MEMORY and issues NO
  query at all on the repeat-login common case (same picture, already
  verified) — the bench's §3b arm is the reason: even a guarded
  `IS DISTINCT FROM` no-op UPDATE ships the ≤512 KiB avatar parameter over
  the wire just to compare it (1.20 ms vs the 2.37 ms full-row rewrite;
  the in-memory skip makes it 0). When something DID change,
  `sync_oidc_login_profile` runs the guarded narrow UPDATE (image +
  conditional stamp, `update_storage_usage` pattern) instead of the
  17-column rewrite.

## [Q4] Refresh rotation — one transaction

`refresh_token` paid two full BEGIN/COMMIT pairs per rotation
(`revoke_session` then `create_session`), and DAV clients rotate
constantly. New `rotate_session(old_id, new_session)` port method: revoke +
insert + last-login stamp in one `with_transaction`. Gates: old session
revoked, new session live, reuse-detection semantics untouched (family
revocation still fires on replay). The per-rotation "Session … revoked"
info-line is gone with the old method call (routine rotation is not a
security event; explicit logout/family revocation still log).

## [Q5] WOPI CheckFileInfo — three independent lookups overlapped

The handler ran require(Read) → get_file → check(Update) serially; all
three key off `(caller, file)` alone. Now `tokio::join!` with results
evaluated in the original precedence (Read gate first, then 404, then the
can_write hint — deny responses byte-identical; the Update probe still
skips its query when the token has no write claim). Same fusion applied to
`authorize_wopi_access` (host page / editor-url). Cold is the shape that
matters: office editors poll CheckFileInfo through a session, but each
(file × TTL-window) pays the cold chain once.

## [Q6] Fused upload-quota gate

`refuse_if_over_quota` (NC chunked PUT — runs on EVERY chunk) issued the
user-envelope read and the drive-cap read serially. One `LEFT JOIN` row
now carries both counter pairs; the verdict evaluators were extracted
(`eval_user_envelope` / `eval_drive_cap`) and are shared by the old point
methods and the fused one, so every error string is identical by
construction. Gates: verdict identity across ok / drive-over / user-over
(precedence) / unlimited / missing-drive. A `check_upload_quotas_by_folder`
twin exists for folder-keyed callers; the three REST once-per-upload pair
sites were left as-is (their two checks carry different rejection logs, and
one query per whole upload isn't worth entangling that — see §Skipped).

## [M1] `sized_json` — the 128-byte seed on every listing

```
cargo run --release --features bench --example bench_round12_micro    # §1
```

axum's `Json` serializes into `BytesMut::with_capacity(128)`; a 500-row
listing (~190 KB) grows it through ~11 doubling reallocs, memcpy-ing ~1.3×
the payload. `interfaces::api::sized_json` pre-sizes from the row count
(FileDto ≈ 380 B serialized; estimate 384) and serves byte-identical output
(gated). Applied to the four hot listing responses: `list_files` (which is
UNBOUNDED — no page cap), folder resources, photos timeline, search (both
verbs).

## [M3] Security-header stack 5 → 1

The CSP middleware already post-processed every response; the four static
headers (`x-content-type-options`, `x-frame-options`, `referrer-policy`,
`permissions-policy`) each rode their own `SetResponseHeaderLayer` on top.
Folded into the same pass — inserted before the 304 early-return because
the standalone layers stamped 304s too. Gate: status + full sorted header
set byte-identical for json / html / 304 through real axum routers.

## [M4] Media capture-metadata single-read (the ROUND11 deferred lead)

```
cargo run --release --features bench --example bench_round12_micro    # §4
```

`extract_blocking` read each image once wholesale for kamadak, then
nom-exif re-opened the SAME file (`read_exif(path)`), and date-less images
paid a third open (`read_track(path)` fallback). Videos opened twice (a
doomed `read_exif` sniff, then `read_track`). Now: nom-exif parses from the
kamadak buffer zero-copy (`MediaSource::from_memory` over the same `Bytes`
allocation, API verified on the pinned 3.6.1), one reused `MediaParser`,
and videos open once with a `kind()` dispatch. The track fallback for
images SURVIVES (fed from the same bytes) — it covers MIME-mislabeled rows,
the only case where it ever produced a date; behaviour is
observable-identical (gated over dated/undated JPEG, PNG, crafted MP4 —
corpus asserted non-vacuous: the crafted EXIF date and mvhd creation time
must actually extract). Warm: 1.44x geomean. Cold cache (`drop_caches`
arms): dated JPEG 0.81 → 0.34 ms, undated 0.97 → 0.30, PNG 0.12 → 0.06,
MP4 0.050 → 0.032. Per-image opens 2-3 → 1; the backfill sweeps multiply
this by the library size.

## [M5] Chunked-upload session ops

`prepare_chunk` ran `verify_session_owner` (own DashMap lookup + a
`Uuid::to_string`) then re-fetched the same entry; `commit_chunk` did the
same plus its `get_mut` (3 lookups + allocation per chunk). The owner gate
now rides the operation's own lookup (same anti-enum not-found for unknown
and foreign sessions — gated), and the uuid compares against a
stack-encoded hyphenated form. 5 → 3 shard-lock round-trips and −2 allocs
per chunk cycle.

## [B1][B2] Blob-cache: moka byte-weigher index + the put_blob ordering fix

```
cargo run --release --features bench --example bench_blob_cache_index
cargo run --release --features bench --example bench_blob_cache        # regression guard
```

The ROUND11 deferred headline. The cache index was a
`tokio::sync::Mutex<LruCache>`: every cached chunk read took the one global
async mutex to probe+promote (LRU `get` needs `&mut`), so a 100-chunk video
playback was 100 serialized critical sections and concurrent readers
contended process-wide — measured NEGATIVE scaling (2.08 → 1.07 Mops/s
going from 1 to 2 readers). `moka::sync::Cache` with a byte weigher makes
the probe lock-free (K=2 **2.17x**, K=8 1.46x; end-to-end warm reads with
real files 1.00-1.15x on this 4-core box — the gap is the index share of
the path and widens with cores/readers). moka also absorbs the byte budget:
the manual `current_size` counter + `collect_evictions` sweep are gone; an
eviction listener unlinks size-evicted `.blob` files. Safety gates: budget
enforced (100 × 1 MiB into a 10 MiB cap → ≥88 files unlinked, survivors
readable), a Replaced entry does NOT unlink its file, Explicit
invalidations unlink at their call sites, and the per-hash single-flight
still collapses 16 concurrent misses to 1 fetch. The `CachedRef` clone
bundle (incl. a `cache_dir` PathBuf clone paid on every HIT for a miss-only
struct) is gone — internals now borrow `self`.

Two behavioural notes, both strict improvements: the write-through PUT
paths now respect the byte budget (the old index deliberately skipped
eviction there, letting write bursts overshoot until the next read-miss);
and a restored over-budget cache trims at startup instead of on the next
insert.

**B2 (the ROUND11 correctness note):** `put_blob` populated the cache AFTER
`inner.put_blob` — but every inner backend consumes the source file (local
renames it, S3/Azure delete it post-upload), so the `fs::copy` failed 100%
of the time, silently (`let _`), and the first read after a whole-file put
(the backend-migration copier) re-downloaded the blob from the remote.
Cache-first now, with invalidate+unlink if the inner put fails so a
rejected blob can never be served. The round-3 stampede guard re-run passes
against the migrated backend (16 → 1 remote fetches, cache file verified).

## [F1] SPA list-view thumbnails (vitest gate)

```
cd frontend && npx vitest run src/lib/api/endpoints/round12.bench.test.ts
```

Both views requested the 400 px `preview` rendition; the list row draws it
in a 40×40 slot (the 150 px `icon` rendition is already ≥2× retina density
there). `thumbSizeForView` switches list rows to `icon` — ~7.1x fewer
pixels per thumbnail, roughly 4-8 KB vs 20-40 KB encoded WebP each, across
files/recent/favorites/trash/shared list views. Grid keeps `preview`
(100×70 slot at 2x DPR genuinely needs it).

## Rejected / reworked this round (the discipline working)

- **Single-pass compression predicate**: the sweep flagged "~28 redundant
  Content-Type header reads per compressible response" in `main.rs`'s
  `And`-chain. The bench says otherwise: the monomorphized chain runs in
  **4.6 ns / 0 allocs** total (straight-line inlined probes), and the
  hand-fused single-pass node measured 5.2 ns on the compressible hot case
  — within noise, sometimes slower. Not shipped; the declarative chain
  stays. `bench_round12_micro` §2 keeps the reproducible evidence.

## Considered and skipped (cost/benefit, not measurement)

- **REST per-upload quota pair fusion** (multipart / native-chunked /
  delta): the two checks sit in separate `if` blocks with distinct
  rejection logs and folder-id guards; fusing saves ONE query per whole
  upload (not per chunk) and would entangle that flow. The NC per-chunk
  site — the hot one — is fused (Q6).
- **NC per-session quota budget cache** (0 queries per chunk instead of 1):
  needs a staleness/invalidation story vs concurrent sessions; the fused
  read already halves the per-chunk cost with bit-identical semantics.
  Flagged for a future round.
- **`lto = "fat"` on the release profile**: the bench profile already uses
  it; flipping release trades a large link-time regression for every
  contributor and CI/Docker build against a low-single-digit runtime gain.
  That's a project-level call for maintainers, not a bench-gated code
  change — flagged, not shipped.

## Deferred / flagged (not shipped this round)

- **Grouped file/grid views are still unvirtualized** (files route
  `groupBy != ''` mounts EVERY row in both view modes; ResourceList's
  grouped GRID branch too — trash is grouped-by-default). Design prepared
  this round: flatten groups into the existing `VirtualRows`
  (photos-timeline pattern — headers as first-class rows, grid rows as
  fixed-height strips of `gridColumns(width)` tiles), which also collapses
  the per-section `VirtualList` scroll listeners the grouped LIST path
  pays today (one `getBoundingClientRect` per section per scroll tick).
  This is the next round's headline; it wants its own pass with UI gates.
- **Duplicate `TraceLayer` on `/api`** (`routes.rs` layers it again under
  the global `ClientIpMakeSpan` layer) and the **per-request `client_ip`
  String** in the span factory — small, want their own measured arms.
- **`CachedBlobBackend::local_blob_path` sync `stat`** (ROUND10/11 flag
  stands — background-extraction paths only; needs an async port variant).
- **Media hooks read the same blob up to 3×** per upload (thumbnail +
  capture-metadata + faces each pull it independently; the latter two read
  the RAW blob path directly, bypassing the content cache — and, on
  encrypted deployments, reading ciphertext: correctness note for
  maintainers, same class as the ROUND11 put_blob note).
- **`mp3_duration::from_path` full-file frame scan** runs even when the
  ID3 `TLEN` tag is present (ingest-path only). Preferring TLEN is a
  speed/accuracy tradeoff on VBR files — maintainer call.
- **Thumbnail orientation re-parses EXIF** that capture-metadata also
  parses; reusing the persisted `orientation` is ordering-dependent
  (hooks run concurrently) — needs a small sequencing decision.

## Environment / methodology

- `cargo run --release --features bench --example bench_round12_queries`
  — needs Postgres; seeds and sweeps its own fixtures (BENCH_PASSES,
  BENCH_SHR_USERS, BENCH_WOPI_FILES, BENCH_WARM_ITERS).
- `cargo run --release --features bench --example bench_round12_micro`
  — counting allocator; §4's cold arms drop the page cache (root; set
  BENCH_COLD_ITERS=0 to skip).
- `cargo run --release --features bench --example bench_blob_cache_index`
  — index scaling + eviction/single-flight safety gates.
- `cargo run --release --features bench --example bench_blob_cache`
  — round-3 cross-round regression guard (passes against the moka index).
- `cd frontend && npx vitest run src/lib/api/endpoints/round12.bench.test.ts`.
