# NC chroot / default-drive resolution — moka caches (vs 2 queries/request)

With app-password verification already cached (5 min) and user flags cached
(30 s), the NextCloud basic-auth middleware still resolved the chroot from
scratch on EVERY protected NC request: `find_default_for_user` (drives JOIN
folders) + `get_folder(root_id)` (folders by PK) — 2 uncached round-trips + 2
pool checkouts before the handler even ran, for values that change only on
provisioning / drive deletion / a root-folder rename. The native `/webdav`
surface repeated the drive lookup per request (Mode-B scope resolution, MOVE
and COPY twice), WOPI once per call.

Changes:

1. `DrivePgRepository::find_default_for_user` memoised (moka, 30 s TTL —
   same tier as `drive_role_cache`), invalidated on personal-drive creation,
   drive deletion and policy updates. Only `Ok` is cached, so the
   provisioning idempotency check still sees the live table.
2. NC middleware markerless-chroot `FolderDto` cached by root-folder id
   (30 s TTL). Only the markerless branch — the drive-marker branch keeps
   its per-request `get_folder_with_perms` authz.

Staleness: bounded at 30 s for a root-folder *rename* (doesn't pass through
the repo); every other mutation invalidates explicitly.

## Reproduce

```bash
cargo run --release --features bench --example bench_chroot_cache
# tunables: BENCH_POOL=20 BENCH_SECONDS=4 BENCH_CONCURRENCIES=8,64
```

## Results (4 cores, local PG16, pool=20)

| conc | mode   |     req/s |  p50 µs |  p95 µs |  p99 µs | queries |
|-----:|--------|----------:|--------:|--------:|--------:|--------:|
|    8 | BEFORE |    11,013 |   696.8 | 1,203.2 | 1,642.6 |  88,102 |
|    8 | AFTER  | 2,011,191 |    0.69 |    1.97 |    8.47 |       0 |
|   64 | BEFORE |    16,952 | 3,633.3 | 5,617.6 | 7,189.1 | 135,618 |
|   64 | AFTER  | 2,337,233 |    0.93 |    2.23 |   11.30 |       0 |

- The fixed per-request DB tax of the whole NC surface (sync PROPFIND storms,
  per-chunk uploads, previews, OCS polls) drops from **0.7–3.6 ms p50 (and 2
  pool checkouts)** to a **sub-µs moka hit**.
- Under sync-storm concurrency (64 in-flight) the BEFORE p99 was 7.2 ms of
  pure chroot overhead per request — that whole term vanishes.
