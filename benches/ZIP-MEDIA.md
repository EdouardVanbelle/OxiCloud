# ZIP export — Stored for already-compressed media (vs Deflate-always)

Every ZIP export path (`ZipService::create_folder_zip` for folder downloads +
public share ZIPs, `BatchOperations::download_zip` for batch downloads) used to
build **every** file entry with `Compression::Deflate`. The dominant "download
folder" payload is photos/video (JPEG/HEIC/MP4/WebP), which deflate cannot
shrink (~0 %) while costing ~40 MB/s of CPU per core — and `async_zip` runs
deflate **inline on the writing tokio task** (inside `poll_write`), so a media
folder download monopolised ~1 core for its whole duration.

The change picks the entry compression from the file's MIME type at plan time:
`Stored` for already-compressed content, `Deflate` otherwise. The shared
predicate is `common::mime_detect::is_precompressed_mime` /
`zip_entry_compression` — it mirrors the HTTP `CompressionLayer` exclusion
list in `main.rs` (keep in sync), minus `x-tar`/`octet-stream` (containers of
possibly-compressible data stay on Deflate so nothing ever gets bigger).

## Reproduce

```bash
cargo run --release --features bench --example bench_zip_media
# tunables: BENCH_MEDIA_FILES=48 BENCH_MEDIA_MB=4 BENCH_TEXT_FILES=24 BENCH_TEXT_MB=2 BENCH_REPS=3
```

Rebuilds the exact production writer stack (`ZipFileWriter::with_tokio(BufWriter(File))`,
`write_entry_stream`, 64 KiB chunks) over a mixed corpus: 192 MiB incompressible
"media" + 48 MiB compressible text (80/20 by bytes, a realistic media folder).

## Results (4 cores, this container)

| mode                  | wall s | cpu s | MB/s   | out MiB | speedup |
|-----------------------|-------:|------:|-------:|--------:|--------:|
| all-Deflate (BEFORE)  |  5.786 |  5.88 |   41.5 |   198.8 |   1.00× |
| mime-aware (AFTER)    |  1.341 |  1.38 |  178.9 |   198.7 | **4.31×** |
| all-Stored (bound)    |  0.150 |  0.19 | 1601.5 |   240.0 |  38.6×  |

- **4.31× faster wall clock and 4.3× less CPU** on the mixed corpus, with the
  archive **0.05 % smaller** (media never deflated anyway; text keeps Deflate).
- The remaining 1.38 s CPU in mime-aware is the text deflate + CRC32 — the
  irreducible part. Pure-media folders approach the all-Stored bound (the
  archive becomes blob-read-bound instead of CPU-bound).
- Side effect on the runtime: the writing task no longer occupies ~a full core
  per media download — on a 4-core box that's ~25 % of total CPU handed back
  to other requests for the duration of every archive.
