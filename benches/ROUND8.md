# Round 8 — shared-album thumbnail authz: cache the folder-grant cascade decision

Benchmark-gated, same rule as ROUND2-7: every change ships with a BEFORE/AFTER
benchmark and equivalence/safety gates; an AFTER that doesn't beat its BEFORE
gets rolled back. This round touches the authorization engine, so the bench
carries hard **safety gates** (recipient allowed, outsider denied, and a
revoke-denies-immediately test) and the change is additionally validated
against the full `--cfg integration_tests` authz suite.

Measured on 4 cores / 15 GiB, local PostgreSQL 16 (fsync off), release profile.

## Summary

| # | change | key metric | before → after |
|--:|---|---|---|
| 1 | `cascade_grant_cache` for File/Folder Read checks | shared-album thumbnail revalidation (100-photo) | 2576 → 2.70 µs/thumb (**~950x**); 257.6 → 0.27 ms/view |

## [1] Shared-album thumbnails — folder-grant cascade query per thumbnail → cached

`get_thumbnail_impl` runs `require_permission(Read, file)` on every request,
ahead of the ETag-304 and moka/disk cache short-circuits. For the **owner** (or
any drive member) that's a `drive_role_cache` hit — ~1 µs, no query. But a
**shared-album recipient** — someone granted a *folder* (the album), not drive
membership — fails the drive-role precheck in `PgAclEngine::check_inner` and
falls through to `file_cascade_grant_exists`: an `role_grants ⋈ folders`
ltree-ancestor (`lpath @>`) query, once per file. Browsers revalidate immutable
thumbnails constantly (`If-None-Match`), so the same `(recipient, file, Read)`
decision was recomputed on every thumbnail of every view — a shared 100-photo
album cost ~100 grant queries per "navigate away and back".

The safe fix keeps the check exactly where it is — **authz is never skipped**,
the ordering is unchanged — and memoises only its *result* in a new
`cascade_grant_cache` (`(Subject, Resource, Permission) → bool`, 30 s TTL). It's
consulted only after the drive-role precheck fails, so a caller who later gains
a drive grant short-circuits above it and can't be shadowed by a stale entry.

**Invalidation** mirrors `drive_role_cache`'s documented convention exactly:
explicit `invalidate_all` on every File/Folder `set_role` / `clear_role` (the
direct share/revoke path — infrequent next to thumbnail reads, so a full flush
is cheap and keeps a revoke *immediate*); the indirect paths (group-membership
changes, resource moves, grant `expires_at` expiry) are caught by the 30 s TTL,
"rather than a deep invalidation tree".

Safety gates in the bench (hard asserts): the folder-grant recipient is allowed
on every album file, an outsider is denied, and — critically — after a warm
cache serves `allowed`, a `clear_role` on the shared folder makes the very next
check **deny** (proving the grant-write flush; without it the stale `true`
would still serve). Also validated against the full `--cfg integration_tests`
authz suite (grants, nested groups, drive membership, read-only freeze).

```
cargo run --release --features bench --example bench_thumbnail_cascade_cache
# thumbs=100 (recipient holds a folder grant, no drive membership)
# arm                          wall ms    µs/thumb
# BEFORE (query/thumb)          257.60     2576.04   <- folder-cascade query per thumbnail
# AFTER cold (first view)        84.18      841.76   <- distinct files miss+populate the cache
# AFTER warm (revalidation)       0.27        2.70   <- all cache hits (~950x vs BEFORE)
# Safety gates PASSED: recipient allowed, outsider denied, clear_role revoke
# denies immediately (grant write flushed the cache).
```

## Notes

- The batched search Read path (`check_files_read_batch`) is unchanged — it
  already resolves a page of files in one round-trip and isn't the
  per-thumbnail hot path; it neither reads nor writes this cache, so no
  consistency coupling is introduced.
- First-view cost is unchanged (distinct files are cache misses that populate
  the cache); the win is on revalidation + repeat views, which is where the
  thumbnail traffic concentrates. A folder-level cascade cache would also cut
  the first-view N-queries to one-per-folder, but needs a file→parent-folder
  resolution and a wider invalidation story — deferred.
- The ACL-before-304 *ordering* (running authz before the 304/cache
  short-circuits) is left intact — with the cascade decision now cached, the
  authz on the revalidation path is a memory hit, so the "zero DB work on a
  304" intent is restored without moving (and thus without weakening) the
  security check.
