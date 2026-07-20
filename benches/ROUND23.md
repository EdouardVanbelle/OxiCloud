# Round 23 — Postgres query-shape pass: typed JSONB decode, drive-policy borrow-deserialize, user-profile join!, subject-group CTE reuse, dedup unzip

Benchmark-gated, same rule as ROUND2–22: every change ships with a BEFORE/AFTER
benchmark and a value equivalence gate; an AFTER that doesn't beat its BEFORE is
rolled back (never applied). This round is the **PostgreSQL** pass — the
candidates the earlier rounds deferred as "needs a database to bench" — so it
ships two harnesses:

- **`bench_round23_micro`** (no Postgres) — the deterministic **allocation gate**
  for the decode/clone candidates (counting global allocator; a non-winning
  AFTER `std::process::exit(1)`s with `GATE FAIL … rollback`).
- **`bench_round23_queries`** (live Postgres) — end-to-end **p50 latency** + a
  strict **equivalence gate** (identical decoded rows / ids / user-sets from
  BEFORE and AFTER; a mismatch exits 1) against seeded fixtures.

Reproduce (the queries harness reads `DATABASE_URL` from `.env`):

```
cargo run --release --features bench --example bench_round23_micro
cargo run --release --features bench --example bench_round23_queries
```

## Summary

| # | change | metric | before → after |
|--:|---|---|---|
| **J1** | `contact_pg_repository::row_to_contact` (+ the inlined `contact_group_pg_repository` sibling) decoded each of the 3 JSONB columns (`email`/`phone`/`address`) with `row.get::<serde_json::Value>` + `serde_json::from_value::<Vec<Dto>>` — a throwaway `Value` DOM built per column and then walked a **second** time to produce the typed `Vec`. Now `row.try_get::<sqlx::types::Json<Vec<Dto>>>` decodes the JSONB bytes straight into the typed Vec in one `from_slice` pass, no DOM. Runs **per contact row** of every contact list / multiget / CardDAV sync. | micro allocs · PG p50 | **84 → 33 allocs/op** (2.15× wall) · **3794 → 2360 ns/contact** (1.61×) |
| **J2** | `DrivePolicies::from_value` did `serde_json::from_value(value.clone())` — cloning the **entire** policies `Value` DOM before walking it, on every drive-policy read (move/copy, shared-link creation, grant). Now `DrivePolicies::deserialize(value)` deserializes straight from the borrow (serde_json's `Deserializer for &Value`), no clone — a one-line body change, byte-identical, all 7 call sites unchanged. | micro allocs | **5 → 0 allocs/op** (11.51× wall) |
| **P1** | `AuthApplicationService::get_user_profile` issued two **independent, serial** `get_user_by_id` point reads (caller then target; the self-case short-circuit compares input UUIDs, not fetched data). Now the self-case does a single fetch and the non-self path overlaps caller+target with `tokio::join!` (`caller_res?` first preserves the caller-error precedence). | PG p50 | **577 → 312 µs/call** (1.85×) |
| **G1** | `SubjectGroupService::remove_member` ran the child group's transitive-user recursive CTE **twice** for a nested `Group` removal — once in the would-empty pre-check, once in `invalidation_targets` after the remove. The edge delete is *above* the child, so its descendants can't change; now the CTE runs **once** and the result is reused for both. | PG p50 | **829 → 412 µs/removal** (2.01×) |
| **U1** | `dedup_service` (`store_loose_chunks` final registration + the ingest `run_rollback`) built `Vec<String>`/`Vec<i64>` by **cloning** every 64-byte hash out of an owned, dead-after `Vec<(String,i64)>` purely to reshape for `sync_blobs(&[String])` + the `UNNEST` bind. Now `into_iter().unzip()` moves the hashes out — no per-hash content copy. | micro allocs | **256 → 0 hash clones** (1283 → 1027 allocs/op on a 256-chunk batch) |

> The micro allocs/op is the deterministic gate (identical run to run); the PG
> p50 is single-machine, warm-pool, and noise-bounded. Every section carries a
> value-equivalence gate; the shipped source matches each AFTER arm.

## [J1] Contact JSONB — typed `Json<T>` decode, no intermediate `Value` DOM

`row_to_contact` (reached by 11 call sites — every contact GET / list /
paginated list / multiget / CardDAV cursor stream / search / by-email /
by-group / create+update RETURNING) and the identical inlined block in
`contact_group_pg_repository::get_contacts_in_group` both did:

```rust
let email_json: JsonValue = row.get("email");        // sqlx JSONB → Value DOM (alloc tree)
let emails = serde_json::from_value::<Vec<EmailPersistenceDto>>(email_json)  // walk the DOM again
    .map(emails_from_persistence).unwrap_or_default();
// … same for phone, address
```

`sqlx::types::Json<T>` decodes the raw JSONB bytes with a single
`serde_json::from_slice::<T>` (sqlx-core 0.8.6 `types/json.rs`), skipping the
`Value` tree entirely:

```rust
let emails = row
    .try_get::<sqlx::types::Json<Vec<EmailPersistenceDto>>, _>("email")
    .map(|j| emails_from_persistence(j.0))
    .unwrap_or_default();
```

`try_get` (not `get`) preserves the exact malformed-shape fallback — `get`
would panic on a decode error, whereas the old `from_value(...).unwrap_or_default()`
tolerated it. The columns are `JSONB NOT NULL DEFAULT '[]'`, so SQL NULL never
occurs. Byte-identical: both paths run the same derived `Deserialize<Vec<Dto>>`
over the same bytes — the `bench_round23_queries` §Q1 gate asserts the two
decode the 500 seeded contacts field-for-field identically. The micro shows the
3 discarded DOMs/row (84 → 33 allocs); on the real rows the decode is 1.61×.

## [J2] Drive policies — deserialize from the borrow, don't clone the DOM

`DrivePolicies::from_value(value: &serde_json::Value)` is called on every
drive-policy read (`get_policies_for_file/_folder`,
`get_drive_id_and_policies_for_*`, `update_policies` RETURNING, the ACL engine's
enforcement read, and `Drive::typed_policies`). It built the typed struct with
`serde_json::from_value(value.clone())` — a full clone of the policies DOM
purely because `from_value` consumes its argument. serde_json implements
`Deserializer` for `&Value`, so the struct can be built straight from the
borrow:

```rust
use serde::Deserialize as _;
Self::deserialize(value).unwrap_or_default()   // was: serde_json::from_value(value.clone())
```

Byte-identical (same derived `Deserialize`, same lenient `unwrap_or_default`
fallback that keeps unknown keys on disk), a one-line body change, and every
caller keeps its `&Value` argument unchanged — so `typed_policies(&self)`
(which only has a borrow of `self.policies`) also stops cloning. The micro
(a realistic bag with a preserved unknown key) drops 5 → 0 allocs/op.

## [P1] `get_user_profile` — overlap the two independent reads with `join!`

The profile lookup fetched the caller and the target user in two serial
round-trips. The self-case (`caller_id == target_id`) is decided by comparing
the **input** UUIDs, so on the common non-self path the two reads are
independent — query 2 never depends on query 1. AFTER:

```rust
if caller_id == target_id {                       // self: one fetch, unchanged
    let caller = self.user_storage.get_user_by_id(caller_id).await?;
    return Ok(UserDto::from(caller));
}
let (caller_res, target_res) = tokio::join!(       // non-self: overlap
    self.user_storage.get_user_by_id(caller_id),
    self.user_storage.get_user_by_id(target_id));
let caller = caller_res?;                          // caller-error precedence preserved
let target = match target_res { … };               // identical NotFound→anonymized-404 + audit
```

Every observable outcome is preserved (self still 1 fetch, the anti-enumeration
audit unchanged). The §Q4 gate asserts identical ids from both shapes; two
warm-pool serial reads vs the `join!` measured **1.85×**.

## [G1] `remove_member` — compute the child's transitive users once, reuse it

For a nested `GroupMember::Group(child_id)` removal the child's transitive-user
set (a recursive `WITH RECURSIVE` CTE over `subject_group_members`) was computed
**twice**: once in the would-empty self-defense pre-check, and again inside
`invalidation_targets` after `remove_member` deleted the parent→child edge. That
edge is *above* the child, so the child's own descendants are unchanged —
verified empirically on the live DB (child set `{u2,u3}` identical before and
after the edge delete). AFTER computes the CTE once, up front, and reuses it for
both the pre-check and the cache-invalidation set (`invalidation_targets` stays
for `add_member`). The §Q6 gate asserts the child set is both stable and the
expected `{u2,u3}`; 2 CTEs vs 1 measured **2.01×** on the seeded 3-level tree.

## [U1] dedup hash reshape — move via `unzip`, don't clone

`store_loose_chunks`'s final registration and the ingest `run_rollback` both
reshaped an owned `Vec<(String,i64)>` (dead after the block) into the
`Vec<String>` + `Vec<i64>` that `sync_blobs(&[String])` and the `UNNEST` bind
need, by cloning every 64-char hash:

```rust
let hashes: Vec<String> = new_rows.iter().map(|(h, _)| h.clone()).collect();  // N clones
let sizes:  Vec<i64>    = new_rows.iter().map(|(_, s)| *s).collect();
```

Since the source is owned and never read again, `into_iter().unzip()` moves the
hashes out — 0 per-hash content copies:

```rust
let (hashes, sizes): (Vec<String>, Vec<i64>) = new_rows.into_iter().unzip();
```

Byte-identical rows inserted; the micro (256 distinct new chunks) drops exactly
the 256 hash clones. (This is the move-not-borrow refinement of the ROUND21 §R2
`&[&str]` pattern — `sync_blobs` takes `&[String]`, so a borrow would force a
port-signature change across 6 backends, whereas the move needs none.)

## Not shipped — deferred to a dedicated pass

- **`batch_operations::download_zip` per-item N+1** (the audit's #2, highest
  raw-latency candidate): the file loop calls `get_file_with_perms` (itself
  authz + `get_file` = 2 round-trips) per selected file, then
  `add_file_entry_streamed` — which **re-authorizes** internally via
  `get_file_stream_with_perms`. Collapsing the per-item metadata+authz into a
  bulk `get_files_by_ids` + `check_files_read_batch` prefetch is a real win
  (`2N+2M` serial round-trips → ~3 batch queries), **but** it moves the sole
  authorization from before the stream to inside it, so it needs a careful
  AuthZ-ordering + anti-enumeration proof (the project's rule: authz lives in
  the service layer, denials audit-log and return the anti-enum shape). That is
  its own validated pass, not a perf banner — queued with a `download_zip`
  fixture that seeds a large multi-select and asserts identical ZIP entry
  set+order across the change.
- **Contact/Drive JSONB — the SQL-NULL edge**: the typed `try_get`/`deserialize`
  paths return the empty/default on SQL NULL where the old `row.get::<Value>`
  would have panicked. Both columns are `NOT NULL DEFAULT` today so this never
  fires; noted only so a future nullable-column change re-checks it.

## Environment / methodology

- A local **PostgreSQL 16** dev instance was provisioned for this round
  (schema applied via the 67 `migrations/*.sql` in order; `pg_trgm` + `ltree`
  extensions). `bench_round23_queries` seeds its own fixtures (unique
  `bench23-*` markers) and tears them down (idempotent cleanup) around the run.
- **Build note:** this session's host intermittently `SIGILL`ed rustc/LLVM
  codegen under the repo's default `-C target-cpu=native` (a `cascadelake` with
  AVX-512 whose passthrough faulted after a host migration). All Round-23
  builds/benches were run with `RUSTFLAGS="-C target-cpu=x86-64-v3"` (AVX2, no
  AVX-512) to sidestep it. This is a local build-flag override only — the
  checked-in `.cargo/config.toml` is unchanged, and the primary gate (allocs/op)
  is target-cpu-independent; the PG p50 comparisons use the same flag for both
  arms, so the relative speedups hold.
- Each micro section: BEFORE (verbatim shipped-before shape) vs AFTER (verbatim
  shipped-after shape) + a value-equivalence assert + a `GATE FAIL … rollback`
  exit if the AFTER fails to reduce allocations. Each PG section: BEFORE vs
  AFTER shape against real seeded rows + an equivalence gate (mismatch → exit 1)
  + p50 over `BENCH_PASSES`.
- Verified beyond the benches: `cargo fmt --all --check` clean, `cargo clippy
  --features bench --all-targets -D warnings` clean, and the touched modules'
  unit tests pass (`contact`, `drive`, `subject_group`, `dedup`, auth profile).
