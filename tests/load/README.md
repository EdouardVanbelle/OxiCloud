# tests/load/

K6 load-test suite for OxiCloud. Detects performance regressions by comparing
each run's p50/p95/p99 against a committed baseline.

## Suites

- **smoke** — `just load-smoke`. Single VU, single iteration of one scenario.
  Verifies the harness still builds and the server boots. ~1 minute. Run on
  every PR. No regression gate.
- **full** — `just load`. Runs every scenario under `scenarios/` against a
  seeded database. Compares results against `baseline/baseline.json`; exits
  non-zero on regression beyond the per-metric tolerance. Run nightly on
  `main` and manually.

## Scenarios

| File                              | What it measures                                                                |
| --------------------------------- | ------------------------------------------------------------------------------- |
| `scenarios/smoke.js`              | Login + create folder + upload + list root + delete. Liveness only.             |
| `scenarios/folder_cascade.js`     | `GET /contents`, `PUT /move`, batch copy, `DELETE` on a depth-8 fanout-5 tree.  |
| `scenarios/share_cascade_rebac.js`| `POST /grants` on a folder, then descendants fetched by the grantee.            |
| `scenarios/subject_group_nested.js`| Grant via a 3-level nested group chain, then descendants fetched by a member.   |

Add new scenarios as `scenarios/<name>.js`; register their metric names in
`baseline/baseline.json`.

## Seeding

`src/bin/load-seed.rs` (invoked by `run.sh`) bulk-inserts fixtures directly
via sqlx: users, deep folder tree, files (all sharing one dedup'd blob),
nested subject groups, ReBAC grants. Only the resources each scenario
actively touches (the grant being created, the move target, etc.) go
through the REST API at run time — that is the measured hot path.

## Baseline & regression detection

`baseline/baseline.json` is the source of truth. Shape:

```json
{
  "folder_cascade.list_depth8": { "p50": 12.0, "p95": 45.0, "p99": 80.0, "tolerance_pct": 10 }
}
```

K6 scenarios load this file at startup and set `thresholds` from it, so a
regression fails the K6 run directly. `compare.mjs` also prints a
human-readable diff table after the run and exits non-zero if any metric
exceeds tolerance.

**Updating the baseline is deliberate.** Run `just load-baseline` to rewrite
`baseline.json` from the latest run, then commit it as
`chore(load): accept new baseline for <reason>`. Never auto-update.

## Local workflow

```bash
just db                  # start the test postgres (port 5433)
just load-seed           # seed alone (poking around in psql)
just load-smoke          # fast liveness check
just load                # full suite + regression diff
just load-baseline       # rerun, accept current numbers as the new bar
```

## CI

- `.github/workflows/load-smoke.yml` — every PR. ~1 min. No regression gate.
- `.github/workflows/load-nightly.yml` — cron daily + `workflow_dispatch`.
  Runs the full suite, uploads results as artifact, opens an issue on
  regression. Currently runs on `ubuntu-latest`; replace with a stable
  self-hosted runner for trustworthy regression signal (shared GitHub
  runners produce noisy timings).

## Why K6 and not Goose/drill

K6 is Go-based (JS scripting in `goja`), not Node. For scenario A, the
client adds ~50–200µs per request — negligible vs. multi-ms server work,
and regression deltas only need *consistency*. K6 also gives us
thresholds-as-DSL, native InfluxDB/Prometheus output, and faster
scenario iteration than a Rust tester would. Reassess when scenario B
(many concurrent users) demonstrates K6 saturation issues.
