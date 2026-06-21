# WebP thumbnails — codec comparison (bandwidth vs quality)

WebP (lossy) is the **primary** thumbnail codec: generated eagerly on upload and
served to the ~97% of clients that advertise `Accept: image/webp`. JPEG is the
lazy fallback for older clients and NextCloud. This doc records the before/after
that justified the change and the `WEBP_QUALITY` choice.

## What it buys

For "hundreds of photos", thumbnail **bytes on the wire** are the dominant cost.
WebP at q82 ships visually-equivalent thumbnails (SSIM within ~0.005 of JPEG q80,
imperceptible at thumbnail scale) for **~65% fewer bytes** on the bench corpus.

> ⚠️ **Honest caveat — the corpus is smooth.** The synthetic corpus is now
> photo-realistic (summed low-frequency sinusoids = smooth color fields + mild
> grain), which compresses far more like a real photo than the old white-noise
> corpus did. But it has **no hard edges / text / foliage** — exactly the
> high-frequency content where both codecs grow and the WebP-vs-JPEG ratio
> narrows toward the classic **~25–40%**. So treat ~65% as an upper bound and
> ~25–40% as the realistic real-photo expectation. Drop real photos into
> `benches/corpus/` (same filenames) to measure on real data.

## Reproduce

```bash
cargo run --release --features bench --example bench_thumbnails_mem
# Tables E1 (WebP quality sweep) and E2 (production codec) at the bottom.
```

SSIM is mean over non-overlapping 8×8 luma blocks vs the **uncompressed**
full-decode source resized to the thumbnail's exact dims (`reference_luma_at`),
so it isolates codec fidelity (no second lossy step). Note this 8×8 metric
slightly favours JPEG's 8×8 DCT blocks, so WebP's SSIM reads a hair low.

## E2 — production codec at `WEBP_QUALITY = 82` (14 cores)

| case      | size    | jpeg B | webp B | save% | ssim jpeg | ssim webp |
|-----------|---------|-------:|-------:|------:|----------:|----------:|
| jpeg_12mp | Icon    |   4608 |   1966 | 57.3% |    0.9960 |    0.9916 |
| jpeg_12mp | Preview |  17564 |   6614 | 62.3% |    0.9931 |    0.9871 |
| jpeg_12mp | Large   |  47254 |  15580 | 67.0% |    0.9865 |    0.9808 |
| jpeg_24mp | Icon    |   5518 |   2588 | 53.1% |    0.9970 |    0.9935 |
| jpeg_24mp | Preview |  19294 |   7556 | 60.8% |    0.9949 |    0.9904 |
| jpeg_24mp | Large   |  51705 |  18904 | 63.4% |    0.9918 |    0.9863 |
| jpeg_48mp | Icon    |   4554 |   1870 | 58.9% |    0.9957 |    0.9895 |
| jpeg_48mp | Preview |  16567 |   5704 | 65.6% |    0.9930 |    0.9866 |
| jpeg_48mp | Large   |  45938 |  12540 | 72.7% |    0.9910 |    0.9866 |

**Total: JPEG 213.0 KB → WebP 73.3 KB = 65.6% smaller.** WebP SSIM trails JPEG by
≤0.006 everywhere — imperceptible at thumbnail scale.

Encode (Preview/12 MP, full pipeline incl. the shared decode): **JPEG 34.4 ms vs
WebP 39.5 ms** (+5 ms, +15%). The WebP encoder is marginally slower but the cost
is paid once, eagerly, in the background generator — it never sits in the
request path (served thumbnails are cache hits).

## E1 — why q82 (quality sweep, Preview/400px)

Even at q90 WebP's 8×8-block SSIM stays a touch under JPEG q80 (the metric favours
JPEG's DCT grid), but the gap is ≤0.008 at the 0.99 level while the byte savings
are 50–68%. q82 is the chosen balance: SSIM 0.987–0.990 (within ~0.005 of JPEG,
imperceptible) at ~60–66% fewer bytes.

| source    | JPEG q80 (B / ssim) | webp q78    | webp q82    | webp q86    | webp q90    |
|-----------|---------------------|-------------|-------------|-------------|-------------|
| jpeg_12mp | 17564 / 0.9931      | −65% 0.9858 | −62% 0.9871 | −58% 0.9900 | −51% 0.9912 |
| jpeg_24mp | 19294 / 0.9949      | −64% 0.9891 | −61% 0.9904 | −57% 0.9921 | −50% 0.9930 |
| jpeg_48mp | 16567 / 0.9930      | −68% 0.9855 | −66% 0.9866 | −61% 0.9888 | −55% 0.9911 |

Tune `WEBP_QUALITY` (in `thumbnail_service.rs`) up for more fidelity, down for
more bandwidth savings.

## How it's served (Strategy B)

- **Eager**: on upload the background generator renders all 3 sizes as WebP
  (`{blob_hash}.webp`).
- **Lazy fallback**: a request without `Accept: image/webp` (or NextCloud, which
  pins JPEG) generates `{blob_hash}.jpg` on first hit, then caches it like WebP.
- **Negotiation**: `GET /api/files/{id}/thumbnail/{size}` reads `Accept`,
  serves WebP or JPEG, sets `Vary: Accept` on every response (incl. 304) and a
  format-keyed ETag so shared caches never hand the wrong codec to a client.
  `Content-Type` is byte-sniffed (`infer`), so it always matches the bytes.
- Dedup, the moka cache (keyed by `(file_id, size, format)`), and cleanup all
  carry both formats.
