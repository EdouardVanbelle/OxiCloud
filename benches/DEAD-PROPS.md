# WebDAV dead-properties — batched per-page fetch (vs per-child N+1)

The streaming PROPFIND walkers (native `webdav_handler.rs`, NextCloud
`nextcloud/webdav_handler.rs`, plus both NC REPORT handlers) fetched dead
properties **one child at a time, sequentially** — one DB round-trip per file
and per subfolder of every Depth:1 listing. On top, every
`DeadPropertyStore` query filtered with `folder_id IS NOT DISTINCT FROM $1 AND
file_id IS NOT DISTINCT FROM $2`, which PostgreSQL cannot serve from a B-tree
index (`IS NOT DISTINCT FROM` is not an indexable operator) — so each of those
N round-trips also degraded to a **sequential scan** as the table grew.

Changes:

1. `DeadPropertyStore::get_all_for_files / get_all_for_folders` — ONE
   `file_id = ANY($1)` round-trip per 500-child PROPFIND page (indexable via
   the partial unique indexes from migration 20260830000001).
2. All single-resource queries (`get`, `get_all`, `remove`) now filter on the
   concrete column (`file_id = $1` / `folder_id = $1`) instead of the
   NULL-tolerant pair — index scans instead of seq scans.
3. All four handler loops replaced with one batched map lookup per page.

## Reproduce

```bash
cargo run --release --features bench --example bench_dead_props
# tunables: BENCH_CHILDREN=2000 BENCH_PAGE=500 BENCH_NOISE_ROWS=20000 BENCH_REPS=5
```

Measures exactly the dead-prop portion of one Depth:1 PROPFIND of a
2,000-child folder (what the walker adds on top of the listing queries).

## Results (4 cores, local PG16, this container)

**Table with only the 2,000 seeded rows:**

| mode                          | queries | total ms | vs OLD |
|-------------------------------|--------:|---------:|-------:|
| OLD — seq, IS NOT DISTINCT    |    2000 |  1072.41 |  1.0×  |
| EQ  — seq, `file_id = $1`     |    2000 |   509.89 |  2.1×  |
| BATCH — `= ANY($1)` per page  |       4 |     4.15 | **258×** |

**Table with 22,000 rows (realistic volume — seq scans hurt):**

| mode                          | queries | total ms | vs OLD |
|-------------------------------|--------:|---------:|-------:|
| OLD — seq, IS NOT DISTINCT    |    2000 |  4543.74 |  1.0×  |
| EQ  — seq, `file_id = $1`     |    2000 |   515.84 |  8.8×  |
| BATCH — `= ANY($1)` per page  |       4 |     5.88 | **773×** |

- A Depth:1 PROPFIND of a 2,000-child folder was spending **1.1–4.5 s** on
  dead-prop chatter alone — now **~5 ms**. This is per folder per sync poll,
  on the hottest path desktop sync clients have.
- The `EQ` row isolates the indexability fix (2.1–8.8×); the batching is the
  rest. Both are applied.
- Same unit economics apply to the other N+1s fixed alongside (search ReBAC
  batch, ZIP batch authz): each eliminated sequential point query is worth
  ~0.25–2.3 ms of the numbers above depending on table size.
