# Round 21 — CalDAV/CardDAV row-mapper pre-size, dedup hash-bind & digest-key dedup, CardDAV etag & BDAY emit, NC trashbin content-type

Benchmark-gated, same rule as ROUND2–20: every change ships with a BEFORE/AFTER
benchmark and a byte/-value equivalence gate; an AFTER that doesn't beat its
BEFORE is rolled back (never applied). The roll-back rule is encoded directly in
the harness — a `GATE FAIL … rollback` non-zero exit if an AFTER arm fails to
reduce allocations — so a regression fails CI rather than shipping.

This round drains the sibling seams the earlier passes explicitly deferred. The
file-listing repositories got their result-`Vec` pre-sizing in ROUND20 §I1, but
the **CalDAV/CardDAV row mappers** (bulk address-book / calendar sync builds
thousands of rows) were left growing from capacity 0. The **streaming ingest**
loop got its `[u8; 32]`-digest dedup key in ROUND17 §D2, but its **delta-upload
sibling** `store_loose_chunks` kept a `HashSet<String>` and a double hex clone
per frame. The **NextCloud** etag emitter got the borrowed-pre-escaped-quote
treatment in ROUND20 §C1, but the **CardDAV** emitter still built a quoted
`String`. And two dedup/DAV emit micro-cuts the earlier rounds named but held
back: the `settle_batch` clone-to-bind and the `BDAY` strftime stamp.

Reproduce:

```
cargo run --release --features bench --example bench_round21_micro
```

All arms are **no-Postgres** (release-profile counting-allocator example).

## Summary

| # | change | key metric | before → after |
|--:|---|---|---|
| **R1** | The CalDAV/CardDAV row-mapping repositories (`calendar_event_pg_repository`, `calendar_pg_repository`, `contact_pg_repository`, `contact_group_pg_repository`) built their result `Vec` with `let mut v = Vec::new(); for row in rows { v.push(map(row)?) }` — growing from capacity 0 (~⌈log₂N⌉ reallocations, each memcpy-ing the accumulated rows) on **every CalDAV/CardDAV listing, multiget & bulk sync**. Now `Vec::with_capacity(rows.len())` (the ROUND20 §I1 file-side pattern extended to the 16 calendar/contact sites it deferred). Plus one `HashMap` (`get_calendar_properties`). | 200-row listing | **7 → 1 allocs/op** (6 fewer) |
| **R2** | `DedupService::settle_batch` cloned every 64-char chunk hash into a `Vec<String>` purely to `.bind()` it to the pin `UPDATE … WHERE hash = ANY($1)`, on **every settle batch of every upload** (~128 batches for a 1 GB fully-unique upload). Now binds a borrowed `Vec<&str>` — sqlx encodes `&[&str]` to `text[]` identically (`favorites_pg_repository.rs:271` already does this). | 32-chunk batch | **33 → 1 allocs/op · 39.4× wall** |
| **R3** | `DedupService::store_loose_chunks` — the delta-upload sibling of the ROUND17 §D2 ingest loop — kept an intra-request dedup `HashSet<String>` and cloned the hex hash **twice per frame** (into `received` and into the set; the set clone dropped on the spot for a duplicate). Now keys the set on the raw `[u8; 32]` BLAKE3 digest (`Copy`, no per-distinct-chunk heap key) and moves the hex into `received` on a duplicate. Runs **per frame** on delta/sync uploads (thousands of frames for a large changed file). | 128 frames, 50% dup | **401 → 209 allocs/op (192 fewer) · 1.50× wall** |
| **R4** | `carddav_adapter::write_contact_response` built a `"…"`-quoted `String` for `getetag` then wrote it auto-escaped — `quick_xml` escapes the `"` → `&quot;`, re-allocating an owned `Cow` — on **every contact of every CardDAV multiget/PROPFIND** (plus the per-address-book collection etag). Now emits the two quotes as borrowed pre-escaped `&quot;` text events (the ROUND20 §C1 NextCloud pattern, via a shared `write_quoted_etag` helper covering all 4 CardDAV etag sites). | per-contact row | **3 → 0 allocs/op · 2.11× wall** |
| **R5** | `contact_to_vcard` stamped `BDAY` via `write!(…, "{}", bday.format("%Y-%m-%d"))`, running chrono's strftime interpreter per **contact-with-birthday**. Now renders the fixed `YYYY-MM-DD` on the stack via the new `fmt::compact_date` (the date-only companion to the §V2 `REV` renderer), chrono fallback for out-of-range years. | per bday contact | **2 → 0 allocs/op · 10.51× wall** |
| **R6** | The NextCloud trashbin PROPFIND row set `d:getcontenttype` for a folder to `"httpd/unix-directory".to_string()` — a heap `String` for a static constant, **per trashed folder row**. Now `Cow::Borrowed` (the ROUND16 §M1 `Cow<'static, str>` pattern); only the file branch (mime_guess) still owns its String. | per folder row | **1 → 0 allocs/op · 5.76× wall** |

> Allocs/op is the deterministic primary gate (identical run to run). Wall
> figures are single-shot and noise-bounded. Every section carries a
> byte/-value equivalence gate; the shipped source now matches each AFTER arm.

## [R1] CalDAV/CardDAV row-mapper container pre-size

`collect::<Result<Vec>>()` was ROUND20 §I1's target on the file side; the
CalDAV/CardDAV repos use the equivalent `Vec::new()` + `for row in rows { … }`
shape, which grows the container the same way — from capacity 0, reserving
nothing, so `push` reallocates ~⌈log₂N⌉ times and memcpy-s the accumulated
(Contact/Event-sized) rows on each grow. `rows` is a materialized `fetch_all`
result, so `rows.len()` is exact:

```rust
let mut events = Vec::with_capacity(rows.len());
for row in rows {
    events.push(Self::row_to_event(row)?);   // ? short-circuits identically
}
```

Applied to the 16 listing/multiget/paginated mappers across the four repos
(`calendar_event` ×6, `calendar` ×2 + the `get_calendar_properties` HashMap,
`contact` ×6, `contact_group` ×1). The `subject_group` and
`nextcloud_object_id` sibling mappers already pre-sized (`with_capacity(rows.len())`),
so they were left untouched. Byte-identical output; on a 200-row listing the
container allocations drop from **7 → 1** (the growth-from-0 reallocations
replaced by a single exact reserve).

## [R2] settle_batch — bind borrowed `&str`, don't clone

`settle_batch` runs once per flushed chunk batch of every upload. It built an
owned `Vec<String>` of the batch's 64-char hashes only to `.bind()` it:

```rust
let hashes: Vec<String> = batch.iter().map(|(h, _)| h.clone()).collect();  // N heap Strings
// … .bind(&hashes) … WHERE hash = ANY($1) …
```

`batch` outlives the query (it is consumed two statements later), so the hashes
can be borrowed. sqlx encodes `&[&str]` to a PostgreSQL `text[]` identically to
the owned `Vec<String>` (the pattern `favorites_pg_repository.rs:271` already
uses, with the comment *"sqlx binds `&[&str]` as text[], so no per-id String is
needed"*). The borrow is scoped in a block so it ends before `batch` is moved:

```rust
let pinned: HashSet<String> = {
    let hashes: Vec<&str> = batch.iter().map(|(h, _)| h.as_str()).collect();
    sqlx::query_scalar::<_, String>("UPDATE … WHERE hash = ANY($1) RETURNING hash")
        .bind(&hashes).fetch_all(pool.as_ref()).await?.into_iter().collect()
};
```

Up to `FLUSH_MAX_CHUNKS` (=32) 64-byte `String` allocations removed per batch —
~4000 over a 1 GB fully-unique upload — for one pointer-only `Vec`.

## [R3] store_loose_chunks — digest-keyed dedup set + move-on-duplicate

The delta-upload ingest (`store_loose_chunks`) is the sibling ROUND17 §D2 didn't
reach. Per frame it allocated the 64-char hex hash and then cloned it twice:

```rust
let hash = blake3::hash(&data).to_hex().to_string();
received.push((hash.clone(), data.len() as u64));   // clone 1 (always)
if seen.insert(hash.clone()) {                       // clone 2 (always; HashSet<String>)
    new_rows.push((hash, len));
}
```

`seen` is the **intra-request** dedup set (has this exact chunk already appeared
in *this* delta stream? — re-chunked near-duplicates, zero-padded regions). Keyed
on the raw 32-byte digest it needs no per-distinct-chunk `String`, and a
duplicate frame **moves** the hex into `received` instead of cloning:

```rust
let digest = blake3::hash(&data);
let hash = digest.to_hex().to_string();
let len = data.len();
if seen.insert(*digest.as_bytes()) {         // HashSet<[u8; 32]>, Copy key
    self.backend.put_blob_from_bytes_unsynced(&hash, data).await?;
    received.push((hash.clone(), len as u64));
    new_rows.push((hash, len as i64));
} else {
    received.push((hash, len as u64));       // move, no clone
}
```

hex ↔ digest is bijective, so membership and the `received`/`new_rows`
sequences are identical. On a 128-frame stream with 50 % intra-request dups the
per-frame hash clones drop from 3 to ~1.5.

## [R4] CardDAV getetag — borrowed pre-escaped quotes

`write_contact_response` (per contact of every CardDAV multiget/PROPFIND) built
a `"…"`-quoted `String` and wrote it auto-escaped; `quick_xml` escapes the `"`
to `&quot;`, so the whole-string escape re-allocated an owned `Cow`. The new
shared `write_quoted_etag` helper emits the two quotes as **borrowed**
pre-escaped `&quot;` text events around the escaped etag body — byte-identical
(the equivalence gate asserts it, including an etag with `&`/`<`/`"`), 0
allocs/contact. Applied to all four CardDAV etag sites (2 per-contact + 2
per-address-book collection), mirroring the NextCloud ROUND20 §C1 fix.

## [R5] BDAY — stack-rendered `%Y-%m-%d`

`contact_to_vcard` already stack-renders `REV` (ROUND19 §V2); `BDAY` still went
through chrono's strftime interpreter (`bday.format("%Y-%m-%d")`). The new
`fmt::compact_date(buf, year, month, day)` renders the fixed 10-byte
`YYYY-MM-DD` with the same `push4`/`push2` LUT the other `fmt` helpers use, and
returns `None` outside the 4-digit-year range (where chrono widens/sign-prefixes
`%Y`) so the caller keeps the chrono path as fallback. Byte-identical for every
representable birthday.

## [R6] NC trashbin folder content-type — borrowed constant

The trashbin PROPFIND folder branch `to_string()`-ed the static
`"httpd/unix-directory"` per row. `Cow::Borrowed` for the folder constant (the
file branch still owns its mime_guess String) drops that allocation per trashed
folder row — the ROUND16 §M1 `Cow<'static, str>` pattern the trashbin loop
missed.

## Not shipped — deferred to a later round

Surfaced by the Round-21 audit (three parallel sub-audits across the HTTP,
storage/dedup and application/parse layers), verified against current source,
but held back — each needs a signature/API decision, a Postgres fixture, or a
gate the deterministic alloc-counter can't provide:

- **Hot GET handlers clone the whole request `HeaderMap`** (`file_handler`
  list/download/thumbnail, `photos_handler`, NC `preview`/`avatar`): axum's
  `HeaderMap` extractor does `parts.headers.clone()` (~2 allocs) purely to read
  1–3 headers — the exact cost `middleware/auth.rs` already eliminated (ROUND14
  §A4) but never propagated to the handlers. The fix takes `req: Request` last
  and reads `req.headers()` by borrow; it's a **multi-handler signature refactor**
  (each `_impl` + its wrapper + the route registration) that wants its own
  validated pass, same class as the ROUND19/20 multi-signature deferrals.
- **`Query<HashMap<String,String>>` on the hot list/download paths** builds a
  `HashMap` + key `String` per request to read one param; a typed
  `Query<ListFilesQuery>` struct drops both (serde ignores unknown params). Same
  signature-surface reason as above; pairs naturally with the HeaderMap pass.
- **Native WebDAV PROPFIND re-extracts the URI path** (`webdav_handler.rs:507`):
  `extract_webdav_path(req.uri())` re-runs a percent-decode + `String` alloc that
  the `path` parameter already holds at that point (the `:503` comment about the
  prefix is stale). One decode + alloc per PROPFIND — but removing it needs a
  careful href-equivalence proof across the chroot/scope resolution, so it wants
  a dedicated correctness check, not a perf banner.
- **`music_service` public-playlist merge is O(owned·public)** (`Vec::any()` per
  public item): a `HashSet` makes it O(owned+public). Because `PlaylistDto.id` is
  a `String`, the set must own the ids (clone) — so the change trades N String
  comparisons for N String clones: a **wall win that ADDS allocations**, which
  the deterministic alloc gate can't score. Wants a wall-gated evaluation on the
  opt-in `include_public` path.
- **WebDAV dead-props filter is O(N·D·R)** (`webdav_adapter.rs:616/705`): the
  loop-invariant requested-props list is re-scanned per dead prop per resource;
  a per-PROPFIND `HashSet<&QualifiedName>` makes it O(N·D). Only bites accounts
  that accumulate client-set custom props (macOS Finder) over large listings —
  and, like music_service, the HashSet build trades compares for an alloc, so
  it's wall-gated. Queued with a synthetic-dead-props bench.
- **`verify_integrity` Phase 1 probes manifest chunks serially** while Phase 2
  is `buffer_unordered(16)` — on a remote backend that's O(total_chunks) serial
  HEADs. Background/admin path; needs a remote-backend fixture to show the win.
- **`subject_group_service::remove_member` runs the same recursive-CTE
  `list_transitive_users(child_id)` twice** for a nested group removal (the
  intervening edge delete can't change the child's descendants). One DB
  round-trip halved; low frequency (admin), needs Postgres.
- **`store_loose_chunks` final registration + `run_rollback` clone hashes to
  bind** (`dedup_service.rs:887/212`), and the **`contact_pg` JSONB columns
  decode through a throwaway `serde_json::Value`** — the R2/ROUND20 patterns
  applied to once-per-upload / per-contact-read sites; both need Postgres to
  bench end-to-end.
- **`GzipCompressionService::{compress,decompress}_data` copy the whole buffer
  via `.to_vec()`** before `spawn_blocking` — forced by the `&[u8]` port
  signature; a `Bytes`-taking port lets an owning caller move. Port API change,
  gated (text > 50 KB), low heat.
- **Fast hasher for trusted-key internal maps** (ROUND20 flag stands): needs a
  `Cargo.toml` dependency decision and must stay DoS-resistant for the
  attacker-controlled delta-hash sets — worth a dedicated, wall-gated pass.

## Environment / methodology

- `cargo run --release --features bench --example bench_round21_micro` —
  counting global allocator, no Postgres. Tunables (env): `BENCH_ITERS` (200000),
  `R1_ROWS` (200), `R3_FRAMES` (128).
- Each section is BEFORE (verbatim replica of the shipped-before shape) vs AFTER
  (verbatim replica of the shipped-after shape, which the source is then made to
  match), with a byte/-value equivalence gate; the shipped source now matches
  each AFTER arm.
- Roll-back rule encoded per section: the harness `std::process::exit(1)`s with
  `GATE FAIL … rollback` if an AFTER arm fails to reduce allocations.
