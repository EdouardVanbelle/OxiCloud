# PROPFIND folder paging — keyset cursor + (folder_id, name) index

`list_files_batch` walks a folder's children in name order, 500 per page
(native + NextCloud PROPFIND streamers). The old shape was `ORDER BY name
LIMIT 500 OFFSET k` with **no supporting index** — the initial schema's
`(folder_id, name, user_id)` index that served it was dropped by migration
20260902000000 (user_id → nullable), leaving only `idx_files_folder_id`. So
every page bitmap-scanned all N children and top-sorted them: a full listing
of an N-file folder cost O(N²/500) row visits + ⌈N/500⌉ sorts.

Changes:

1. Migration `20260917000000_files_folder_name_index.sql`: partial composite
   `idx_files_folder_name (folder_id, name) WHERE NOT is_trashed`.
2. `list_files_batch` cursor switched from OFFSET to keyset
   (`name > $last`, names are unique per folder via the
   `(drive_id, folder_id, name)` unique index) across the port trait, the
   repository and both handler loops. The cursor predicate is only emitted
   when a cursor exists — a `$2 IS NULL OR …` disjunction would block the
   index condition under the extended protocol's generic plans.

## Reproduce

```bash
cargo run --release --features bench --example bench_propfind_paging
# tunables: BENCH_FILES=20000 BENCH_PAGE=500 BENCH_REPS=3
```

Times the FULL page-by-page walk of a 20,000-file folder (the listing
portion of one Depth:1 PROPFIND).

## Results (4 cores, local PG16)

| mode                             | total ms | vs OLD |
|----------------------------------|---------:|-------:|
| OFFSET, no index (true BEFORE)   |  1,266.3 |  1.0×  |
| OFFSET + index (index alone)     |    482.7 |  2.6×  |
| KEYSET + index (AFTER)           |     76.7 | **16.5×** |

- Full-folder listing cost drops **16.5×**; unlike OFFSET (even indexed),
  keyset stays O(page) at any depth, so the gap widens with folder size.
- Companion fix in the same commit: the Photos timeline cursor
  (`list_media_files`) wrapped its keyset column in
  `EXTRACT(EPOCH FROM …)::bigint` plus an `IS NULL OR` disjunction —
  non-sargable, so page k re-scanned all k·limit rows already scrolled past.
  It now compares the raw `media_sort_date` against a timestamptz bind
  (identical row semantics — the cursor is whole seconds) and splits the
  cursor/no-cursor query shapes, restoring the
  `idx_files_media_timeline_by_drive` boundary condition the index was built
  for. Same mechanism as measured above (index-boundary vs per-row filter);
  the deep-scroll effect mirrors the OFFSET column.
