# People tab — grouped COUNT (vs full faces scan with embeddings)

`PeopleService::list_people` (GET `/api/people`, fetched on every People-tab
mount) called `faces_for_user`, which SELECTs every face row for the caller —
each carrying a 2,048-byte embedding BYTEA that gets decoded into a fresh
`Vec<f32>` — only to (a) count faces per person and (b) resolve a handful of
cover faces to file ids. A 10k-face library moved ~21 MB of embeddings per
request. `merge()` had the same over-fetch plus one UPDATE per face.

Changes (`FaceRepository` + `PeopleService`):

- `person_face_stats`: `SELECT person_id, COUNT(*) … GROUP BY person_id`.
- `file_ids_for_faces`: one `id = ANY($1)` over just the cover face ids.
- `reassign_person_faces`: merge as ONE set-based UPDATE (was: load all
  faces, filter in Rust, one UPDATE per face).

## Reproduce

```bash
cargo run --release --features bench --example bench_people_list
# tunables: BENCH_FACES=10000 BENCH_PERSONS=20 BENCH_REPS=5
```

## Results (4 cores, local PG16, 10,000 faces / 20 persons)

| mode                     | total ms | bytes moved |
|--------------------------|---------:|------------:|
| BEFORE — full face rows  |    30.40 |  20,960,000 |
| AFTER — COUNT + covers   |     3.76 |       1,280 |

- **8.1× faster** and **~16,000× fewer bytes** off the wire per People-tab
  mount. The heap never materialises 10k embedding `Vec<f32>`s.
- The BEFORE row also allocated ~21 MB per request on the server; under a
  handful of concurrent mounts that was tens of MB of transient RSS for a
  page that shows 20 avatars.
