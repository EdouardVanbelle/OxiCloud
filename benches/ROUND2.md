# Round 2 — read path, upload path, archives (before/after gates)

Five backend changes + one frontend change, each gated by a before/after
benchmark (`examples/bench_round2.rs`; frontend gate in
`frontend/src/lib/api/endpoints/deltaUpload.hash.test.ts`). Rule of the
round: an AFTER that doesn't beat its BEFORE gets rolled back — none did.

Reproduce:

```bash
BENCH_SECTIONS=1,2,3,5 cargo run --release --features bench --example bench_round2
OXICLOUD_INGEST_OVERLAP=0 BENCH_SECTIONS=4 cargo run --release --features bench --example bench_round2
OXICLOUD_INGEST_OVERLAP=1 BENCH_SECTIONS=4 cargo run --release --features bench --example bench_round2
cd frontend && npx vitest run src/lib/api/endpoints/deltaUpload.hash.test.ts
```

## [1] Range requests served from the content cache — 2,156×

Media players and PDF viewers fetch files *exclusively* via Range requests
(a `bytes=0-` probe, then seeks). All three range paths (REST, DAV helper,
public shares) went straight to `get_file_range_stream`: a PG blob-hash
resolve + chunk open/seek/read per seek — even when the whole sub-10 MB blob
sat in the moka content cache as contiguous `Bytes`.
`FileRetrievalService::get_file_range_preloaded` now answers from the cache
(`Bytes::slice` = refcount bump; a miss populates it via the same
single-flight loader Tier 1 uses, so one probe warms every later seek).

| per 256 KiB seek (6 MiB file) | seeks/s | p50 µs | p99 µs |
|-------------------------------|--------:|-------:|-------:|
| BEFORE — PG + open/seek/read  |   1,730 | 552.5  | 818.8  |
| AFTER — cache hit + slice     | 3,730,560 | 0.15 | 2.85   |

## [2] NC chunked-upload gate: O(N²) directory scan → O(1) counter — 357×

`handle_put_chunk` recomputed "session bytes so far" on EVERY chunk PUT by
listing the session directory and stat-ing every existing chunk — chunk k
scans k files; a 1,000-chunk (10 GB) upload does ~500k stats.
`NextcloudChunkedUploadService` now keeps an in-RAM per-session counter
(seeded on MKCOL, bumped per accepted chunk, dropped on cleanup/overwrite,
lazily rebuilt from the listing on cold start — crash semantics unchanged).

Cumulative gate cost across a 1,000-chunk upload: **33,063 ms → 93 ms**.

## [3] Delta download / commit-verify read-ahead — 8.7× (latency-bound)

`delta_download_chunks` and `hash_chunk_sequence` drained chunks strictly
sequentially — every chunk-open's round-trip paid serially — while the main
CDC download path already overlaps opens with `buffered(read_prefetch)`.
Both now use the same combinator (order preserved — `buffered` yields in
input order).

64-chunk drain with 5 ms per-open latency (object-store model):
**440 ms → 51 ms**. On local disk the same combinator measured +7–12 %
(benches/BLOB-PREFETCH.md).

## [4] CDC ingest: settle overlapped with reading — +7–25 %

`ingest_chunks_from_stream` awaited each batch settle (PG pin round-trip +
up to 8 MiB of backend writes) INLINE — the HTTP source was not polled at
all during the settle, so read and settle phases alternated instead of
overlapping. The settle now runs on a spawned task (depth-1 pipeline) that
records into the guard's shared, lock-serialized state — rollback stays
exact even if the request future is dropped mid-settle.
`OXICLOUD_INGEST_OVERLAP=0` restores the inline behaviour (the bench's
BEFORE side, and an ops escape hatch).

512 MiB unique-content ingest, source paced at 300 MB/s, two reps:
**60 / 69 MB/s (inline) → 75 / 74 MB/s (overlapped)**.

## [5] Streaming ZIP: constant time-to-first-byte — 779× on this corpus

`create_folder_zip` built the ENTIRE archive into a temp file before the
handler sent byte one — TTFB grew with folder size (a multi-GB folder =
minutes of "waiting for server"). `create_folder_zip_stream` plans inline
(planning errors still surface as proper HTTP errors), then writes the
archive on a spawned task through `tokio::io::duplex`, streaming bytes as
they are produced. Folder downloads and public-share ZIPs both use it; a
mid-archive blob error truncates the stream (no central directory → clients
detect corruption) — the standard streamed-ZIP tradeoff. Content-Length is
no longer sent (size unknown up front).

48 × 4 MiB media corpus: TTFB **326.1 ms → 0.4 ms**; total wall also
improved (484 ms → 55 ms — no disk round-trip through the temp file).
TTFB in BEFORE scales linearly with archive size; AFTER is constant.

## [6] Frontend: instant-upload hashing on a worker pool

`resolveOwnedHashes` hashed every small file of a drop sequentially on the
MAIN THREAD (synchronous WASM BLAKE3 per file) before any upload lane
started — seconds of UI jank on large drops. Hashing now fans out over a
bounded pool of dedicated Web Workers (`static/workers/hashWorker.js`,
`File` handles passed by reference, reads happen inside the worker), with
the old inline loop kept as fallback where `Worker` is unavailable.

Architecture gate (node worker_threads, read+hash 24 × 4 MiB, file
references — faithful to the browser shape): 3-lane pool beats the
sequential loop; asserted by `deltaUpload.hash.test.ts` so a regression
fails CI. First model of this gate (posting BUFFERS instead of file
references) was 2.6× SLOWER — structured-clone copies dominated — and was
rewritten; kept here as a reminder that the gate must model the real
data-flow.

## Skipped this round

- **Swimlane (group-by) view virtualization** — needs interactive browser
  measurement (frame times while scrolling) that this environment can't
  produce; deferred rather than shipped unverified.
