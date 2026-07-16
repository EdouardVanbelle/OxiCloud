# Static assets & API responses — precompressed siblings + explicit Brotli level

Two related findings, one root cause: tower-http's `CompressionLayer` default
maps to **Brotli QUALITY 11** (`async-compression Level::Default` →
`BrotliEncoderParams::default()`, brotli-8.0.2 `encode.rs:323` — verified in
source and empirically below). Quality 11 is a deploy-time setting; it was
running per request on:

- every SPA asset (`interfaces/web/mod.rs` layer): ~1.3 s CPU per 700 KiB
  bundle per request;
- every compressible API response (`main.rs` global layer): ~90 ms CPU per
  64 KiB JSON response.

Changes:

1. **Precompressed statics.** `frontend/scripts/precompress.mjs` (build step,
   node:zlib only) emits `.br`/`.gz` siblings for text assets; `ServeDir` now
   uses `precompressed_br()/precompressed_gzip()` — a request costs a file
   read, and clients get the *better* q11 bytes, paid once per deploy
   (~1.4 s for the whole bundle).
2. **Explicit level 4** on both `CompressionLayer`s
   (`CompressionLevel::Precise(4)`) — the on-the-fly fallback for statics
   without siblings, and the global API layer.

## Reproduce

```bash
cargo run --release --features bench --example bench_static_precompress
# tunables: BENCH_ASSET_KB=700 BENCH_REPS=30
```

## Results (4 cores, this container)

**Per-request cost, 700 KiB JS-like asset (94 % compressible):**

| mode                        | ms/request | speedup |
|-----------------------------|-----------:|--------:|
| BEFORE — on-the-fly Brotli  |   1,324.31 |   1.0×  |
| AFTER — precompressed read  |      0.657 | **2016×** |

**Brotli level sweep, 64 KiB JSON-like API response:**

| level                   | ms/resp | out KiB |
|-------------------------|--------:|--------:|
| Default (= quality 11!) |   90.10 |     5.4 |
| **Precise(4)** (chosen) |    0.91 |     6.2 |
| Fastest                 |    0.15 |     9.3 |

- Statics: 3 orders of magnitude less CPU per request, while shipping
  *smaller* bytes than the runtime default would at any reasonable level.
- API responses: **99× less CPU** for ~15 % more bytes (5.4 → 6.2 KiB) —
  `Precise(4)` is the classic dynamic-content operating point; `Fastest`
  gives up too much density (9.3 KiB).
- Historical note: an earlier review round REFUTED the "default is q11"
  claim twice; the source line and the 90 ms/64 KiB measurement above settle
  it the other way. Measure before believing — in both directions.
