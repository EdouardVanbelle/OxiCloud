# Round 24 — `download_zip` per-item authz+metadata N+1 → batch (validated authorization pass)

This is the ROUND23 "not shipped" item #2, given the dedicated validated pass it
needed. Unlike the other rounds it is **authorization-sensitive**, so the gate is
not an allocation count or a latency floor — it is the **security property
itself**: the batched authorization must make the *identical* per-file inclusion
decision as the shipped-before per-file `require` loop, and must never let a
denied or missing file into the archive.

Reproduce (needs the dev Postgres up; reads `DATABASE_URL` from `.env`):

```
cargo run --release --features bench --example bench_round24_zip_authz
```

## The change

`BatchOperations::download_zip` streamed a client's multi-selection into a ZIP.
For the **individually-selected files** it looped, per file:

```rust
for file_id in &file_ids {
    match self.file_retrieval.get_file_with_perms(file_id, user_id).await {   // require + get = 2 round-trips
        Ok(file_dto) => { self.add_file_entry_streamed(&mut zip, file_id, &file_dto.name, &file_dto.mime_type, Some(user_id)).await … }
        Err(_) => { /* skip + log */ }
    }
}
```

`get_file_with_perms` is `require_file` (a `Read` authz round-trip) **plus**
`get_file` (a metadata round-trip) — so a selection of N files is **2N serial
round-trips** before a single byte is streamed. AFTER routes the whole selection
through one new service method:

```rust
let authorized = self.file_retrieval
    .get_files_by_ids_with_perms(&file_ids, user_id).await?;      // 1 batch check + 1 batch get
let by_id: HashMap<Uuid, FileDto> = authorized.into_iter()
    .filter_map(|f| Uuid::parse_str(&f.id).ok().map(|u| (u, f))).collect();
for file_id in &file_ids {                                        // same input order
    let Some(file_dto) = Uuid::parse_str(file_id).ok().and_then(|u| by_id.get(&u)) else {
        info!("Skipping file {file_id} (not accessible or missing)"); continue;
    };
    self.add_file_entry_streamed(&mut zip, file_id, &file_dto.name, &file_dto.mime_type, Some(user_id)).await …
}
```

`FileRetrievalService::get_files_by_ids_with_perms` authorizes every id in ONE
`AuthorizationEngine::check_files_read_batch` (the `PgAclEngine` override resolves
all files' drives in a single query and reuses the per-drive role cache) and
fetches only the authorized ids in ONE `get_files_by_ids`. **2N round-trips → 2.**

### Why this is authorization-safe (the part that made it a dedicated pass)

Three properties had to hold, all verified against the source before touching it:

1. **Authorization still happens before any ZIP entry is written.**
   `add_file_entry_streamed` writes the entry header (the **filename**) *before*
   it opens the authorized stream (`write_entry_stream` then
   `get_file_stream_with_perms`). So the pre-filter is load-bearing: a denied
   file must never reach `add_file_entry_streamed`, or its name would leak into
   the archive (and leave a dangling entry). AFTER preserves this exactly — a
   denied/missing id is absent from `by_id`, so it is `continue`-skipped and
   never reaches the entry write. The authz simply moved from a per-file
   `require` to one batch `check` **earlier** in the same function, not into or
   after the stream.

2. **The per-file stream-open Read check + Recents recording are unchanged.**
   `add_file_entry_streamed(Some(user_id))` still calls
   `get_file_stream_with_perms`, which re-checks `Read` (now a primed-cache hit —
   `check_files_read_batch` seeds the resource→drive cache) and records the
   access in Recents. The old loop double-notified Recents (once in
   `get_file_with_perms`, once in the stream open) and the throttle coalesced it
   to one entry; AFTER notifies once (the stream open) — identical net effect.

3. **The batch authorization is identical to looping `require`.**
   `check_files_read_batch` is documented and gated as "semantically identical to
   looping `check`", and `require(Read)` succeeds iff `check(Read)` is true (a
   denied `Read` is the 404 anti-enumeration shape). The §validation gate proves
   this empirically on a mix of granted / denied / missing ids.

The **folder** selections (`get_folder_with_perms` per root, then the already-bulk
`add_folder_subtree_to_zip`) are left as-is: root counts are small and there is no
`check_folders_read_batch` primitive to batch through — see *Not shipped*.

## The validation

`bench_round24_zip_authz` drives the **real `PgAclEngine`** (the `fresh_engine`
shape from `bench_favorites_authz`) against a seeded fixture designed to exercise
every inclusion outcome:

- `owned` — N files on **drive A**, which the caller holds an `editor` grant on → **must be INCLUDED**
- `denied` — N files on **drive B**, which the caller has **no** grant on → **must be DENIED**
- `missing` — N random UUIDs that don't exist → **must be MISSING**

interleaved `owned, denied, missing, owned, …` so the **order** test is real. The
gate asserts, and `exit(1)`s on any failure:

- `before_included` (the per-file `require` filter, in input order) **==**
  `after_included` (the batch `check_files_read_batch` filter, in input order) —
  identical **set and order**;
- the included set is **exactly** the caller's `owned` files;
- **no** `denied` (other-drive) file is included — the authz-regression tripwire;
- **no** `missing` id is included;
- the batch `get_files_by_ids` of the authorized ids returns **exactly** the
  `owned` files.

Latency (cold engine, empty caches — the first-download shape), `BENCH_FILES=200`
(600-item interleaved selection, ⅓ owned / ⅓ denied / ⅓ missing):

| arm | wall (600 items) | per file |
|---|---|---|
| per-file `require` loop | 559.47 ms | 932.45 µs |
| batch `check_files_read_batch` | 266.58 ms | 444.30 µs |

**2.10×** — and this is the *conservative* case: with ⅓ of the ids on a drive
the caller has no role on, `check_files_read_batch` still falls back to a per-file
`check_inner` for each un-readable-drive file. The realistic "download my own N
files" selection is **all** on drives the caller has a role on, where the batch
is genuinely O(1) (one drive-resolve query + cached role checks) against the
loop's 2N round-trips — a far larger win.

## Not shipped

- **Folder selections** (`download_zip`'s folder loop): `get_folder_with_perms`
  per selected root. Root counts are typically 1–3, and there is no
  `check_folders_read_batch` batch-authz primitive (only files have one), so
  batching would still loop `check` per root — no round-trip win. Left as-is.
- **Dropping the stream-open re-check**: since the batch pre-check already
  authorized (and primed the cache), `add_file_entry_streamed`'s
  `get_file_stream_with_perms` re-check is now redundant (a cache hit). Replacing
  it with the no-perms `get_file_stream` would save the cache lookups but would
  also drop the Recents recording and the second authz barrier — not worth the
  behavior change; kept as belt-and-suspenders.

## Environment / methodology

- Real `PgAclEngine` + `FileBlobReadRepository` against a local **PostgreSQL 16**
  (schema from `migrations/`). The bench seeds its own two-drive fixture
  (`bench_zipauthz_*` markers) and tears it down around the run.
- Built with `RUSTFLAGS="-C target-cpu=x86-64-v3"` (this session's host
  intermittently `SIGILL`ed rustc under the repo's default `-C target-cpu=native`
  AVX-512 after a host migration — see benches/ROUND23.md). Local build-flag
  override only; the checked-in `.cargo/config.toml` is unchanged.
- The gate is the security equivalence (set + order + denied/missing exclusion),
  not a perf threshold; the latency table is supporting evidence for the
  round-trip collapse.
- Verified beyond the bench: `cargo clippy --features bench --all-targets
  -D warnings` clean, `cargo fmt --all --check` clean, `cargo test --lib
  --features bench` = 529 passed / 0 failed.
