# Round 13 — grouped-view virtualization, notification/login query narrowing, HTTP dedup, locale precompute

Benchmark-gated, same rule as ROUND2-12: every change ships with a
BEFORE/AFTER benchmark and an equivalence/safety gate; an AFTER that doesn't
beat its BEFORE gets rolled back or redesigned. This round's discipline
story is a *correctness* finding the sweep surfaced under a perf banner: the
"media hooks read the same blob 3×" lead turned out to be "1 real read + 2
*broken* reads" (the raw-path readers resolve only for local + unencrypted +
single-chunk blobs), so it is flagged for maintainers as a correctness bug,
NOT shipped as a perf change (§Not shipped).

Measured on 4 cores / 15 GiB, local PostgreSQL 16 (fsync off), release
profile; frontend on Node 22 / vitest 4 (jsdom). Reproduce any row with the
command in its section.

## Summary

| # | change | key metric | before → after |
|--:|---|---|---|
| V1 | Grouped views windowed (files route + ResourceList; grid was the last unwindowed path — trash is grouped-by-default in grid) | `.file-item` mounted, 800-item group | **800 → <120** (viewport-bounded) |
| Q1 | Group-notification recipient expansion: drop the ≤512 KiB avatar `image` + `ui_preferences` JSONB from `get_users_by_ids` (email path never reads them) | 30-member fan-out | 8.60 → 0.25 ms (**34.3x**) · ~7.7 MB off the wire |
| Q2 | Login provisioning idempotency: `list_*_by_owner().is_empty()` → `SELECT EXISTS` (×2: calendar + address book, on EVERY login) | 4 owned calendars | 0.193 → 0.170 ms (**1.13x**, widens with owned-row count) |
| Q3 | Recent-access: prune only when the upsert actually inserted (`RETURNING xmax=0`) — a re-access can't grow the set | per re-access | 0.567 → 0.324 ms (**1.75x**) · prune round-trip skipped |
| L1 | Locale `Accept-Language`: precomputed supported-codes list vs rebuilding N heap Strings per anonymous request | 16 locales | 616 → 17.3 ns (**35.7x**) · 18 → 1 allocs |
| H1 | Duplicate `TraceLayer` on `/api` removed (the global stack already wraps it) | per `/api` request | 1.86 → 1.42 µs (**1.31x**) · −6 allocs |
| H2 | `client_ip` span field: borrow-only `ClientIpDisplay` vs an owned `String` per request | per request | 187 → 173 ns · −1 alloc |

## [V1] Grouped views are windowed (the ROUND10-deferred headline)

```
cd frontend && npx vitest run src/lib/components/round13.bench.test.ts
```

The moment any group-by was active, both the files route
(`routes/files/[...path]/+page.svelte`) and `ResourceList` left their
windowed `VirtualList` paths and rendered `{#each groups}{#each rows}` — the
GRID arm mounted **every** card, and the accumulated listing is the whole
folder, so a big grouped grid mounted thousands of `.file-item`s (~8-10
`<Icon>`s + ~8 buttons each), a multi-second main-thread block. `/trash` is
grouped-by-default, so a grid-view trash page hit this on first load.

The fix is the symmetric one the grouped-LIST arm already used and the
flat-GRID arm already proved: **window each swimlane with its own
`VirtualList`** (`windowClass="files-grid-view"` puts the card grid on the
list's inner window). The outer grouped-grid container is a flex column
(`.files-grouped-grid` / `.rl-grouped-grid`), NOT `.files-grid-view` — that
class is itself a grid and would place each header/VirtualList into a cell;
the grid now lives per-section. The files route additionally folds each
group's separate `folders`/`files` into one ordered `Entry` stream
(`groupedEntries`, folders-then-files — the exact old render order) so a
section feeds one `VirtualList`. The prior claim in a code comment that
"`files-grid-view` … can't host the windowing spacer" was simply wrong (the
flat grid disproves it).

Gate: render the real `ResourceList` in grouped GRID mode at N=800 in one
bucket — mounted `.file-item` count is **<120** (viewport+overscan bounded,
`<N/4`), a swimlane header confirms the grouped path, and the `.vlist`
spacer still reserves the full scroll height (cards windowed, not dropped).
Preserved (unchanged, verified by the existing files/trash/recent tests):
selection (`SvelteSet.has` reads stay inside the row), the ROUND11 §S2
fine-grained favorite star (the grouping derive reads only item identity +
order, never `favoriteIds`/`selected`), drag-drop, keyboard, and the
ROUND12 §F1 `thumbSizeForView` icon/preview switch.

Scope note: this windows the grouped arms (the actual defect) while leaving
the already-windowed flat arms on `VirtualList`. Unifying all four arms onto
one `VirtualRows` (the photos-timeline single-pass model) is a clean
follow-up that also removes the per-section scroll listeners — deferred so a
pitch-measurement change can't regress the flat views that are fine today.

## [Q1] Group-notification recipient expansion

```
cargo run --release --features bench --example bench_round13_queries   # §1
```

`RecipientNotificationService` fans a group share out to its members via
`get_users_by_ids`, whose 21-column projection dragged the ≤512 KiB avatar
`image` (TOAST-detoasted per row) and the `ui_preferences` JSONB — of which
the notification path reads *neither* (only email/eligibility fields). It is
the ROUND12 §Q1 sharee-avatar pattern on the group-notify path, ×M members.
`get_users_by_ids` has exactly one production caller, so it is narrowed
in-place (image + ui_preferences dropped; doc updated: notification-recipient
projection). 30-member fan-out: 8.60 → 0.25 ms, ~7.7 MB of avatar/JSONB kept
off the wire. Gate: identical `(id, email, notify_on_share)` set.

## [Q2] Login provisioning EXISTS probes

The Personal-Drive / Default-Calendar / Default-Address-Book provisioning
hooks fire on EVERY login; the calendar and address-book hooks tested
"already provisioned?" by `list_*_by_owner(..).is_empty()` — hydrating every
owned row (calendars carry description/color TEXT) just to look at
emptiness. New `has_owned_calendar` / `has_owned_address_book` back it with
`SELECT EXISTS(...)` (the ROUND9 §7 `Drive::is_empty` COUNT→EXISTS pattern),
short-circuiting at the first row. 4 owned calendars: 0.193 → 0.170 ms; the
margin widens with the owned-row count. Gate: EXISTS agrees with
hydrate-all, present and absent. (The drive hook's unconditional `set_role`
re-emit on every login — an authz write — is flagged, not shipped: it's a
deliberate self-heal and touches authz semantics, the ROUND12 class that
awaits maintainer sign-off.)

## [Q3] Recent-access prune only on insert

`RecentService::record_access` ran `upsert_access` then `prune`
unconditionally — but a re-access is an `ON CONFLICT DO UPDATE` that only
refreshes a timestamp and can never push the user over the cap, so the prune
(a DELETE over an `OFFSET` self-subquery) was a wasted round-trip on that
common path. `upsert_access` now `RETURNING (xmax = 0)` reports whether it
inserted; the service prunes only then. A single fused CTE was rejected: a
data-modifying CTE's outer DELETE sees the pre-insert snapshot, so it would
under-prune by one on the boundary insert — the two-statement,
prune-on-insert shape is the correct one. Re-access: 0.567 → 0.324 ms. Gate:
`xmax` flags insert vs update correctly and the row count stays at the cap.

## [L1] Locale supported-codes precompute

```
cargo run --release --features bench --example bench_round13_micro   # §L1
```

The `Accept-Language` extractor rebuilt the supported-locale list — N fresh
heap `String`s + two `Vec`s — on every anonymous request, though the set is
fixed at startup (the ROUND10 §15 "process-invariant rebuilt per request"
class). `LocaleRegistry` now materializes `supported_codes: Arc<Vec<String>>`
once in `discover()`; the extractor borrows it and builds only the `&[&str]`
view the crate needs. 16 locales: 616 → 17.3 ns, 18 → 1 allocs per anonymous
request. Gate: precomputed and rebuilt code SETS identical (order is
irrelevant — `accept_language::intersection` ranks by header q-values).

## [H1][H2] HTTP micro-pack

```
cargo run --release --features bench --example bench_round13_micro   # §H1, §H2
```

- **Duplicate `TraceLayer` on `/api`** — `routes.rs` layered its own
  `TraceLayer::new_for_http()`, but the global `TraceLayer +
  ClientIpMakeSpan` stack in `main.rs` wraps the whole app (the `/api`
  router is nested into it), so every `/api` request paid TWO span +
  response-future layers. Removed; end-to-end 1.86 → 1.42 µs/request, −6
  allocs. Gate: response status identical with 1 vs 2 layers.
- **`client_ip` span field** — `ClientIpMakeSpan::make_span` allocated an
  owned `String` per request purely to feed `%client_ip` (Display). New
  borrow-only `ClientIpDisplay` renders straight into the span's field
  storage (forwarded header borrowed, peer rendered in place): 187 → 173 ns,
  −1 alloc. Gate: byte-identical to the owned resolver across all four
  resolution cases.

## Not shipped — correctness finding surfaced by the perf sweep

- **Media hooks' raw blob reads are broken, not merely duplicated.** The
  round-12 deferred "media metadata + faces + thumbnail each read the blob"
  lead was investigated for a shared-read refactor. The investigation found
  the premise was wrong: `MediaMetadataService` and `FaceIndexingService`
  read `.blobs/{file_hash}.blob` **directly**, but that path exists only for
  **local + unencrypted + single-chunk** blobs — for a normal multi-MB
  (multi-chunk) photo it does not exist, on S3/Azure there is no local
  `.blobs` tree, and on encrypted backends it is ciphertext. So today those
  two hooks silently produce **no capture date / no GPS / no faces** for the
  common case, while only the thumbnail hook (which goes through
  `dedup.read_blob_bytes`, honoring chunk-reassembly + decryption) works.
  The fix is to route both through `read_blob_bytes` — but that is a
  **correctness fix that is perf-neutral-to-negative** (it makes reads that
  currently fail actually run), so it does not belong in a benchmark-gated
  perf round. Flagged for maintainers as a correctness bug with the exact
  call sites; a shared-`Bytes` provider (single decode-plaintext read fanned
  to the hooks) is the perf follow-up once the correctness fix lands.

## Deferred / flagged (not shipped this round)

- **Unify all four listing arms onto one `VirtualRows`** (flat + grouped ×
  list + grid), the photos-timeline single-pass model — removes the
  per-section scroll listeners the grouped paths now carry and the
  four-branch render in both files route and ResourceList. Wants a
  pitch-measurement pass so it can't drift the flat views that work today
  (V1 scope note).
- **Drive-provisioning `set_role` re-emit on every login** (authz write; a
  self-heal for a historical partial-provision case) — needs maintainer
  sign-off, same class as the ROUND12 auth-write deferrals.
- **NC per-session quota budget cache** (0 queries/chunk instead of the
  ROUND12 fused 1) — needs a staleness/invalidation story (ROUND12 flag
  stands).
- **`mp3_duration` full-file scan when the ID3 `TLEN` tag is present**
  (ingest path) — preferring TLEN is a speed/accuracy tradeoff on VBR files;
  maintainer call.
- **Thumbnail orientation re-parses EXIF** that capture-metadata already
  parsed — reusing the persisted `orientation` is ordering-dependent (hooks
  run concurrently).
- **`CachedBlobBackend::local_blob_path` sync `stat`** (ROUND10-12 flag
  stands; needs an async port variant).
- **`admin_settings_service` ~7 sequential autocommit upserts on OIDC save**
  — admin-only, fired a handful of times per deployment; confirmed still
  present, judged not worth entangling the hot-reload logic (same verdict as
  ROUND12's skipped REST quota-pair fusion).

## Environment / methodology

- `cargo run --release --features bench --example bench_round13_queries`
  — needs Postgres; seeds + sweeps its own fixtures (`BENCH_PASSES`,
  `BENCH_GROUP`, `BENCH_CALS`, `BENCH_RECENT_CAP`).
- `cargo run --release --features bench --example bench_round13_micro`
  — counting allocator; §L1 reads the shipped `frontend/static/locales`.
- `cd frontend && npx vitest run src/lib/components/round13.bench.test.ts`.
