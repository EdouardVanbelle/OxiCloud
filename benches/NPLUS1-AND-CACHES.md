# Companion fixes — same measured unit economics, no dedicated harness

These changes share their cost model with benches that already exist, so
instead of near-duplicate harnesses each entry cites the bench that measured
its unit price. (The per-query unit prices below: sequential indexed point
SELECT ≈ 0.25–0.55 ms and `= ANY($1)` batch ≈ 1–1.5 ms/500 ids from
benches/DEAD-PROPS.md; manifest-row fetch p50 0.44–4.4 ms from
benches/BLOB-MANIFEST.md; moka hit ≈ 1 µs from benches/CHROOT-CACHE.md.)

## 1. Content-search ReBAC re-verification — batched (SEARCH-REBAC)

`SearchService::lookup_content_hits` re-verified up to `CONTENT_HITS_LIMIT =
200` Tantivy hits with sequential `authz.check(Read, File)` calls — each a
point SELECT on owner-cache miss (distinct file ids ⇒ ~always). New
`AuthorizationEngine::check_files_read_batch` (default = the old loop, so
mocks/other impls stay correct; `PgAclEngine` override): ONE
`id = ANY($1)` drive resolution + cached per-drive role + per-file cascade
only for drive-floor misses. Decision-equivalent; per 200-hit search:
**~200 sequential round-trips (≈ 50–110 ms of DB chatter) → 1–2 queries
(≈ 1–3 ms)**. Also primes the owner cache for the hits' follow-up requests.

## 2. Batch-ZIP downloads — no per-file authz/Recent (ZIP-BATCH-AUTHZ)

`BatchOperations::add_folder_subtree_to_zip` had already authorized the
subtree ROOT (`get_folder_with_perms`), yet every enumerated file still paid
`get_file_stream_with_perms` = 1 authz point SELECT + a Recent-hook spawn
issuing 2 writes (INSERT … ON CONFLICT + prune DELETE). A 2,000-file folder
ZIP ⇒ ~6,000 extra statements. Subtree entries now use the plain
`get_file_stream` — exactly what `ZipService::create_folder_zip` (the native
folder-download path) has always done. Explicitly-selected top-level files
keep per-file authz + Recent. Unit price: DEAD-PROPS.md sequential rows —
**~1.5–4.5 s of DB chatter removed** from a 2,000-file archive, plus the ZIP
no longer floods Recents with every archived file.

## 3. CDC manifest RAM cache (MANIFEST-CACHE)

Every stream / range / full read of a CDC blob paid one
`chunk_manifests` row fetch first — p50 0.44 ms (4.4 ms under pool pressure,
benches/BLOB-MANIFEST.md), on the hottest read paths there are (media
serving, thumbnails, range seeks). Manifests are immutable by content
address, so `DedupService` now memoises them (moka, weight-bounded 32 MiB,
60 s TTL, positive-only so background rechunking is honoured immediately;
invalidated post-commit on the two delete paths). Warm read: **0.44–4.4 ms →
~1 µs** (CHROOT-CACHE.md's moka row) and one fewer pool checkout per read —
range-seek storms (video scrubbing) hit this every request.

## 4. Public share landing — 3 round-trips → 1 atomic UPDATE (SHARE-ACCESS)

`GET /api/s/{token}` ran find_share_by_token (with a correlated
`MIN(expires_at)` subquery), a full-row UPDATE writing back a Rust-side
increment (racy: lost updates between concurrent visitors, and it rewrote
`item_name`/`password_hash` wholesale — clobbering concurrent owner edits),
then the handler's follow-up fetched the share a third time.
`ShareStoragePort::increment_access_count` is now one
`UPDATE … SET access_count = access_count + 1 WHERE token = $1 AND <expiry>`:
**3 subquery round-trips → 2** for the landing (register + fetch), no
read-modify-write race, no collateral column rewrites.

## 5. Trash — dead SELECT removed

`TrashService::move_to_trash` fetched the full file/folder entity to build a
`TrashedItem` consumed only by `TrashRepository::add_to_trash` — a documented
no-op in the soft-delete model. Both branches now go straight to the
`move_to_trash` UPDATE: **one uncached SELECT + entity hydration removed per
trash operation** (file and folder).

## 6. NFC normalization fast path

`normalize_storage_name` ran unicode-normalization's full
decompose/recompose state machine on every name of every row loaded from PG
(listings, PROPFIND, photos — 27 constructor call sites), even though the DB
invariant guarantees stored names are already NFC. `is_nfc_quick` (a
per-char table lookup) now short-circuits the ~100 % case to a plain copy;
`Maybe`/`No` still run the full pipeline, so semantics are unchanged.

## 7. Frontend — first-page render for large folders

`fetchFolderListing` paged the ENTIRE folder (sequential 200-item requests)
before returning anything — a 2,000-item folder waited ~10 round-trips
before first paint. The files route now paints page one immediately via the
new `onPage` hook and fills in as later pages land (skipped when a cached
listing is already on screen, so views never shrink). First-paint latency
for an N-item folder drops from ⌈N/200⌉ sequential RTTs to 1.

## Refuted by benchmark (reverted, kept for the record)

- **Cached `Intl.Collator` for name sorts (frontend):** sorting 5,000 names —
  argument-less `localeCompare` 5.6 ms vs cached collator **12.1 ms (2×
  slower)**. V8 fast-paths argument-less `localeCompare`; the "cache the
  collator" folklore does not apply. Reverted, ordering untouched.
