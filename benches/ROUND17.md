# Round 17 — dedup ingest/verify hash-clone purge, CardDAV vCard TYPE tokens

Benchmark-gated, same rule as ROUND2–16: every change ships with a
BEFORE/AFTER benchmark and an equivalence/safety gate; an AFTER that doesn't
beat its BEFORE is rolled back (never applied). The roll-back rule is encoded
directly into the harness as a `GATE FAIL … rollback` non-zero exit, so a
regression fails CI rather than shipping.

This round targets the **content-addressable dedup write path** — the item
carried on the ROUND15 deferred list as "`dedup_service` hash-`String`
re-allocations" — from both ends: the streaming **ingest** loop that hashes and
stores every uploaded chunk, and the delta-commit **verification** read that
re-hashes a proposed chunk sequence. Plus a CardDAV micro-cut: the vCard
emitters allocated a throw-away upper-cased `String` per `TYPE=` token.

Measured on 4 cores / 15 GiB, **no PostgreSQL needed for any Round-17 arm**
(release-profile counting-allocator example). Reproduce any row with:

```
cargo run --release --features bench --example bench_round17_micro
```

## Summary

| # | change | key metric | before → after |
|--:|---|---|---|
| **D2** | Chunk-ingest (`store_from_stream`) allocated the 64-char hex hash `String` **3× per chunk** (`to_hex().to_string()` + `chunk_hashes.push(clone)` + `session_seen.insert(clone)` — the last dropped on the spot for a duplicate). The intra-upload dedup set now keys on the raw 32-byte BLAKE3 digest (`[u8; 32]`, `Copy`, no heap), and the manifest push is split so a duplicate **moves** the hex in. | 64-chunk batch, 1-in-2 dup | **214 → 149 allocs/op (65 fewer)** · **1.14× wall** · set clone gone + dup manifest clone gone |
| **D1** | `hash_chunk_sequence` (delta-commit verification) took `chunks: &[(String,u64)]` and fed the backend stream with `chunks.iter().cloned()` — re-cloning every chunk hash a **second** time on top of the owned `Vec` the caller already built. Take the `Vec` by value and `into_iter()` it. | 64-chunk verify | **65 → 0 internal allocs/op** · **~2.5 µs of clone work removed per verify** |
| **V1** | `contact_to_vcard` / `generate_vcard` emitted every EMAIL/TEL/ADR `TYPE=` token via `ty.to_uppercase()` — one throw-away `String` per token per contact. New shared `fmt::push_upper` writes the upper-cased chars straight into the vCard buffer. | 8 tokens/op | **13 → 5 allocs/op (8 fewer)** · **1.19× wall** |

> Allocs/op is the deterministic primary gate (identical run to run); the wall
> figures are single-shot and noise-bounded (D1's AFTER arm is a near-zero-cost
> read, so its ratio swings 60–120× between runs — the stable fact is the
> ~2.5 µs / 65-alloc clone removed).

## [D2] Chunk-ingest — dedup set keyed on the raw digest

The streaming ingest loop (`DedupService::store_from_stream`) is the hottest
write path in the system: it runs for **every chunk of every upload**. Per
chunk it produced the 64-char hex hash and then allocated it three times:

```rust
let hash = blake3::hash(&data).to_hex().to_string();   // A: the hex String
chunk_hashes.push(hash.clone());                        // B: manifest copy  (always)
if session_seen.insert(hash.clone()) {                  // C: dedup-set copy  (always)
    pending.push((hash, Bytes::from(data)));            // A moved into the write batch
}
```

`session_seen` is the **intra-upload** dedup set (has this exact chunk already
appeared in *this* stream? — repeated blocks, zero-padded regions, re-chunked
near-duplicates). It was a `HashSet<String>`, so clone **C** heap-allocated a
64-byte key for every chunk — and on a duplicate, `insert` allocated the clone
only to drop it when the key already existed. Pure waste on the case a dedup
store exists to make cheap.

A BLAKE3 digest is `[u8; 32]` — `Copy`, no heap, and the hex is a lossless
rendering of it, so keying the set on the raw digest is behaviour-identical:

```rust
let digest = blake3::hash(&data);
let hash = digest.to_hex().to_string();                 // A (once)
if session_seen.insert(*digest.as_bytes()) {            // Copy key — zero heap
    chunk_hashes.push(hash.clone());                     // B (new chunk only)
    pending.push((hash, Bytes::from(data)));             // A moved
} else {
    chunk_hashes.push(hash);                             // dup: move, no clone
}
```

Clone **C** is gone for every chunk; clone **B** is gone for every *duplicate*
(it moves the hex into the manifest instead). The set also holds 32-byte inline
keys instead of 64-byte heap Strings and hashes 32 bytes per membership test.
Net per chunk: **3 → 2 allocs (new) / 3 → 1 (duplicate)** — strictly fewer on
every input, unique or duplicate.

64-chunk, 1-in-2-duplicate batch: **214 → 149 allocs/op (65 fewer), 1.14×
wall**. Gate (in-harness, replica of the exact before/after loop bodies): the
observable output is **byte-for-byte equal** — the ordered `chunk_hashes`
manifest, the `chunk_sizes`, and the distinct write-set `pending` all match;
only the private set's key representation differs — plus the `gate_allocs`
rollback exit on any AFTER that fails to reduce allocations.

## [D1] `hash_chunk_sequence` — take the chunk Vec by value

The delta-sync commit (`delta_upload_service::commit`) verifies a client's
proposed manifest by streaming the pinned chunks back out of the backend and
recomputing the whole-file BLAKE3. The caller already builds a fresh owned
`Vec<(String,u64)>` (`request.chunks.iter().map(|c| (c.h.clone(), c.s)).collect()`),
but `hash_chunk_sequence` took it by `&[(String,u64)]` and then did
`futures::stream::iter(chunks.iter().cloned())` — **re-cloning every chunk hash
a second time** to feed the stream.

Taking `chunks: Vec<(String,u64)>` by value and `into_iter()`-ing it (the caller
drops one `&`) moves those Strings straight into the stream: zero internal
clones. The streamed `(hash, size)` pairs are byte-identical, so the recomputed
hash and every per-chunk size check are unchanged.

The section isolates exactly the clone the old signature forced (the caller's
`.collect()` is identical on both shapes and excluded): **65 → 0 allocs/op** for
a 64-chunk manifest — ~2.5 µs of clone work removed per verify (the AFTER arm is
a near-zero-cost read, so the wall ratio is large but noisy: 60–120×). Gate: the
old internal clone is asserted to be a pure copy (moving changes nothing
observable), plus `gate_allocs`.

## [V1] CardDAV vCard `TYPE=` tokens — `push_upper`

Both vCard emitters — `carddav_adapter::contact_to_vcard` (the CardDAV
REPORT/GET path) and `ContactService::generate_vcard` — wrote each address /
phone / email `TYPE=` parameter with `write!(…, "{}", ty.to_uppercase())`, and
`str::to_uppercase()` heap-allocates a fresh `String` for every token. A contact
with several emails/phones/addresses pays one alloc per token, per emit, on
every address-book sync.

New shared helper `common::fmt::push_upper(buf, s)` writes the upper-cased chars
(`char::to_uppercase`, so byte-identical to `str::to_uppercase` — including
ß → SS and dotless-i) straight into the vCard buffer; the five call sites push
the fixed prefix, the upper-cased token, and the value directly. Zero
temporaries.

8-token contact: **13 → 5 allocs/op (8 fewer), 1.19× wall**. Gates:
`push_upper` is unit-tested byte-equal to `str::to_uppercase` across ASCII /
mixed / multi-char-uppercase / dotless-i inputs (`fmt::tests`), the section
asserts the full emitted vCard is byte-identical before/after, and the existing
`carddav_adapter_test::test_contact_to_vcard_full` pins the whole document.

## Not shipped — deferred to a later round

Surfaced during the Round-17 audit but not landed (each wants its own decision,
a Postgres fixture, or a streaming-I/O benchmark):

- **Storage I/O — `encrypted_blob_backend` frame size (evaluated, kept):** the
  ROUND15 note floated 64 KiB → 256 KiB plaintext emit frames. `PLAINTEXT_EMIT_SIZE`
  is a *deliberate* match to the 64 KiB the unencrypted backends stream, so
  downstream consumers see the same backpressure shape; changing it is a
  behaviour change that needs a streaming-throughput A/B (TTFB + syscalls +
  peak RSS), not an alloc micro-bench. Left as-is pending that harness.
- **Storage I/O — `CachedBlobBackend` write path (needs an fs harness):**
  per-write `create_dir_all` even when the shard dir exists, and inline
  eviction `remove_file` on the reactor thread (carried from ROUND15).
- **Backend query-shape (needs Postgres):** `music_storage_adapter::list_public_playlists`
  1 + N `COUNT(*)` fold; contact REST listings over-fetch the multi-KB `vcard`
  TEXT though the `ContactDto` mappers never read it (wants a *lite* row mapper).
- **Backend CPU/alloc (no Postgres, next micro-pack):** the two WebDAV PROPFIND
  surfaces still quote `d:getetag` into a fresh `String` per row and `format!`
  the per-row href per child (the CalDAV reused-buffer treatment never reached
  them); REST calendar-event edit re-`format!`s the whole `ical_data` body once
  per changed property.
- **Frontend (vitest-benchmarkable):** `VirtualRows.offsets` prefix-sum rebuilt
  in full on every photos-timeline page; the dotfile filter and
  `ResourceList.itemIndexById` re-scan the whole accumulated list per page.

## Environment / methodology

- `cargo run --release --features bench --example bench_round17_micro`
  — counting global allocator, no Postgres. Tunables: `BENCH_ITERS` (100000),
  `BENCH_CHUNKS` (64), `BENCH_DUP_RATIO` (2).
- Each section is BEFORE (verbatim replica of the shipped-before shape) vs AFTER
  (verbatim replica of the shipped-after shape) with a byte/-value equivalence
  gate; the shipped source now matches each AFTER arm.
- Roll-back rule encoded per section: `std::process::exit(1)` with
  `GATE FAIL … rollback` if an AFTER arm fails to reduce allocations.
