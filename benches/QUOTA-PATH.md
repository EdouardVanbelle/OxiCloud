# Quota path — narrow 2-column read + skip-when-not-requested

Two independent fixes on the quota resolution that runs on every upload check
and every quota-reporting folder PROPFIND:

1. **Narrow read.** `check_storage_quota` / `get_user_storage_info` called
   `get_user_by_id`, whose SELECT drags the entire `auth.users` row —
   including `image`, an avatar data URI of up to 512 KiB — to read two i64s.
   New `UserPgRepository::get_storage_usage` reads exactly
   `(storage_used_bytes, storage_quota_bytes)` (same pattern as the existing
   `get_user_flags`).
2. **Skip entirely when not asked.** `resolve_webdav_quota` (2 round-trips:
   drive row + user row) ran on EVERY folder PROPFIND on both surfaces, even
   when the client's `<D:prop>` list named no quota property — which is the
   common shape for sync-client polls. `PropFindRequest::wants_quota()` now
   gates it: `AllProp`/`PropName` keep quota (the writers emit RFC 4331 props
   there), explicit prop lists trigger the lookups only if they name
   `quota-used-bytes` / `quota-available-bytes`. Responses are byte-identical
   for every request that names quota or asks for allprop.

## Reproduce

```bash
cargo run --release --features bench --example bench_quota_path
# tunables: BENCH_SECONDS=4 BENCH_CONCURRENCIES=8,64 BENCH_IMAGE_KB=512
```

## Results (4 cores, local PG16, pool=20, 512 KiB avatar on the row)

| conc | mode   |  ops/s |   p50 µs |   p99 µs |
|-----:|--------|-------:|---------:|---------:|
|    8 | FULL   |  2,222 |  3,369.4 |  8,452.2 |
|    8 | NARROW | 25,118 |    294.9 |    867.9 |
|   64 | FULL   |  2,567 | 24,642.3 | 36,164.0 |
|   64 | NARROW | 40,195 |  1,468.9 |  3,964.9 |

- **11–16× throughput, p50 3.4 ms → 0.29 ms** for the user-row half of every
  quota resolution (the avatar bytes dominated the wire+decode cost).
- With `wants_quota()` the common PROPFIND pays **zero** quota queries — the
  numbers above then only apply to requests that actually ask for quota.
- The same narrow read protects every upload (`check_storage_quota` gates all
  upload paths), where the FULL row was pure overhead per file.
