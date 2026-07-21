# Round 26 — drive-policy JSONB decode (alloc), CachedBlobBackend shard-dir pre-create (disk), delta-upload foldhash (CPU); eviction-unlink off-reactor tested & reverted

This round drains three high-confidence items from the ROUND25 backlog, each
behind a BEFORE/AFTER benchmark that `std::process::exit(1)`s ("`GATE FAIL …
rollback`") unless AFTER strictly beats BEFORE. A fourth candidate (moving the
cache eviction unlink off the reactor) was **tested and reverted** — the
benchmark refuted it. All three shipped items target the owner's priorities:
allocations, disk-I/O, and CPU.

Reproduce:

```bash
RUSTFLAGS="-C target-cpu=x86-64-v3" cargo run --release --features bench --example bench_round26_micro   # P1
RUSTFLAGS="-C target-cpu=x86-64-v3" cargo run --release --features bench --example bench_round26_diskio  # D1
RUSTFLAGS="-C target-cpu=x86-64-v3" cargo run --release --features bench --example bench_round26_hasher   # G1
```

---

## [P1] Drive-policy reads: throwaway `serde_json::Value` DOM → `from_slice::<DrivePolicies>` (allocations)

`drive_pg_repository`'s four policy reads (`get_policies_for_file/_folder`,
`get_drive_id_and_policies_for_file/_folder`) fetched `d.policies` as a
`serde_json::Value` and then called `DrivePolicies::from_value(&raw)`
(`Self::deserialize(&Value)`). The `Value` tree — a `Map` + a boxed `String` key
+ a `Value` node per policy field — is built once, walked once, and dropped. This
is the exact throwaway-DOM pattern ROUND23 §J1 removed for contacts; §J2 removed
only the `from_value` *clone*, not the DOM. These reads fire on file/folder
move & copy and on every share/grant creation.

AFTER fetches through `sqlx::types::Json<DrivePolicies>` — one
`serde_json::from_slice::<DrivePolicies>` over the raw JSONB bytes, no
intermediate DOM — via a shared `policies_from_row` helper that preserves the
lenient `unwrap_or_default` fallback exactly (a malformed bag → all-false,
`try_get(...).unwrap_or_default()`, mirroring §J1).

| arm    |  ns/op | allocs/op | bytes/op |
|--------|-------:|----------:|---------:|
| BEFORE | 399.4  |      6.00 |      719 |
| AFTER  | 161.0  |      0.00 |        0 |

**6 → 0 allocs/op, −719 bytes/op, 2.48× wall** — the entire Value DOM removed per
policy read. Gate: AFTER allocs/op strictly lower. Equivalence: the decoded
`DrivePolicies` is asserted identical BEFORE vs AFTER.

## [D1] `CachedBlobBackend`: pre-create the 256 shard dirs at init, drop the per-write `create_dir_all` (disk-I/O)

`CachedBlobBackend::initialize` created only `cache_dir`, never the 256
`{00..ff}` shard dirs (the line-122 comment claimed otherwise). So all three
cache-write sites (`cache_bytes_write_through`, `insert_into_cache`,
`fetch_and_cache`) re-ran `tokio::fs::create_dir_all(parent)` per chunk — a
wasted `mkdirat(EEXIST)` + component stat + blocking-pool dispatch on a shard
that already exists, on every cached-remote write. AFTER creates all 256 shards
once at init (mirroring `LocalBlobBackend::initialize`, reusing its
`HEX_PREFIXES` table) and deletes the three per-write calls; the shard for any
`&hash[..2]` prefix always exists, so the writes just `fs::write`/`fs::copy`.

Measured on a tmpfs tempdir (`create_dir_all` on an already-existing shard vs the
skip):

| arm    | ns/write |
|--------|---------:|
| BEFORE |  44 801.8 |
| AFTER  |      0.3 |

**~45 µs removed per cache write.** Gate: AFTER ns/write strictly lower. The
on-disk layout is identical; the directory creation simply moved from the hot
path to one-time startup.

## [G1] Delta-upload have/need hash sets: SipHash → `foldhash::quality::RandomState` (CPU)

The delta-upload negotiation builds `HashSet`s over up to `max_chunk_count()`
client-supplied 64-hex BLAKE3 hashes per request (`distinct_hashes`, and
`authorize_chunk_download`'s `distinct_seen`). std `HashSet` uses SipHash-1-3 —
DoS-resistant but ~2-4× slower than a modern hash on short keys. AFTER uses
`foldhash::quality::RandomState`, a fast non-cryptographic hasher that **stays
DoS-resistant** because it is per-instance random-seeded — the required property
for these *attacker-controlled* inputs (not `FxHash`/a fixed seed). `foldhash` is
already in the lockfile transitively (via `hashbrown`), so the direct dep adds no
newly-compiled crate.

Build + membership scan over 40 000 client hashes, p50 over 50 passes:

| arm               | p50 ms (build+scan) |
|-------------------|--------------------:|
| BEFORE (SipHash)  |               4.768 |
| AFTER (foldhash)  |               2.007 |

**2.37× wall** on the delta negotiation's hottest set — a bulk sync of a large
file negotiates thousands of chunks. Scales with `max_chunk_count()`.

Gate: AFTER p50 wall (build set + membership scan over N hashes) strictly lower,
**and** two `RandomState::default()` instances must produce different hashes for
the same key (asserting the random per-instance seed — DoS resistance retained).
The set membership decisions are unchanged, so behaviour is identical.

---

## Tested and reverted

- **[D2] Move the cache eviction unlink off the reactor via `spawn_blocking`.**
  The moka eviction listener unlinks a size-evicted blob with a synchronous
  `std::fs::remove_file` inline on the tokio worker that triggered the insert.
  The hypothesis: hand it to `spawn_blocking` so the reactor isn't blocked on
  `unlink(2)`. The benchmark refutes it on the relevant configuration:

  | arm                         | ns on reactor / eviction |
  |-----------------------------|-------------------------:|
  | BEFORE (inline remove_file) |                  7 055.7 |
  | AFTER (spawn_blocking)      |                 19 848.1 |

  `CachedBlobBackend` caches on a **local** dir (fast unlink, ~7 µs), and
  `spawn_blocking`'s task-dispatch overhead (~20 µs) costs *more* on the reactor
  than the inline unlink it replaces — a net loss. The original code comment ("a
  quick unlink on the inserting task's thread, off the hot get path") is correct
  for the fast-local-cache case. A win would only materialize on genuinely slow
  storage (network-backed cache dir), which there is no fixture for here.
  **Reverted; kept the inline unlink.** (A "measure before believing" result, like
  BASELINE's dropped Task 2.1 / reverted Phase 1.7.)

## Not shipped — carried forward

Named in the ROUND25 backlog, still queued (each wants a multi-signature change,
a remote-backend fixture, or a different toolchain):

- **`format_oc_id_into` buffer** through the NC PROPFIND/REPORT/trashbin emit
  loops — a per-row `String` → reused buffer. Threads a buffer through ~4 loop
  sites across 3 files and depends on `NextcloudFileIdService`'s instance-id
  format; wants its own validated pass so a wrong `oc:id` can't reach a client.
- **S3 read zero-copy forward** (`into_async_read()+ReaderStream` → forward the
  SDK `Bytes` frames, Azure-style) — needs a MinIO/stub `ByteStream` fixture.
- **Frontend folder-listing cache** (`getCachedFolder`/`cacheFolder` is dead
  code; every navigation refetches with `cache:'no-store'`) — a SvelteKit/Vitest
  pass (bandwidth + instant paint on revisits).
- **foldhash for the NC PROPFIND trusted-key maps** (`favorite_ids`, `nc_id`) —
  `foldhash::fast` (no random seed needed; server-generated keys). Threads the
  hasher type through the emit-loop map builders.
- **Contact create/update `Json<T>` bind** (write-side twin of §J1) and the other
  ROUND25 backlog items.

## Environment / methodology

- **P1:** counting global allocator (count + bytes), no Postgres. The real
  `DrivePolicies` type is imported from the crate; BEFORE replicates the shipped
  `serde_json::from_slice::<Value>` + `deserialize(&Value)`, AFTER the shipped
  `from_slice::<DrivePolicies>`. Value-equivalence asserted; gate on allocs/op.
- **D1:** async wall on a tmpfs `tempfile::tempdir`; BEFORE = `create_dir_all` on
  a pre-existing shard, AFTER = the skip. Gate on ns/write.
- **G1:** wall-gated (a hasher swap changes 0 allocations); SipHash vs
  `foldhash::quality` build+scan over N random 64-hex hashes; DoS-seed assertion.
- Built with `RUSTFLAGS="-C target-cpu=x86-64-v3"` (the checked-in
  `.cargo/config.toml` pins `target-cpu=native`, which `SIGILL`s on this host —
  see ROUND23/24; local override only).
- Verified beyond the benches: `cargo fmt --all --check` clean,
  `cargo clippy --features bench -- -D warnings` clean, `cargo test --lib
  --features bench` green.
