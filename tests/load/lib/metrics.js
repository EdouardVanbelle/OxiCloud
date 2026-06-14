// Baseline-driven thresholds and shared helpers for k6 scenarios.
//
// Each scenario tags its requests with `op: '<scenario>.<op>'` (see http.js
// `jsonParams` / `authParams`). The baseline file lists one entry per
// `<scenario>.<op>` with p50/p95/p99 and a tolerance percentage; we convert
// every relevant entry to a k6 threshold so the run fails directly on
// regression — `compare.mjs` then produces the human-readable diff.

// Resolve relative to THIS file (lib/metrics.js), not the importer. Future
// k6 versions will align open()'s path-resolution with ES module semantics;
// using import.meta.resolve() future-proofs against the warning logged by
// k6 ≥ 0.50.
const LOAD_BASELINE_PATH = import.meta.resolve('../baseline/load.json');
const SMOKE_BASELINE_PATH = import.meta.resolve('../baseline/smoke.json');
const MANIFEST_PATH = import.meta.resolve('../results/seed-manifest.json');

// Eagerly load both baseline files at module init (open() is only allowed
// in init context). Keys are disjoint by scenario prefix, so merging the
// two maps is safe; `thresholdsFromBaseline(prefix)` filters from the union.
const BASELINE = {
  ...JSON.parse(open(LOAD_BASELINE_PATH)),
  ...JSON.parse(open(SMOKE_BASELINE_PATH)),
};

/**
 * Return the merged baseline (load + smoke) — convenient for tooling that
 * wants to inspect everything; scenarios should use `thresholdsFromBaseline`.
 */
export function loadBaseline() {
  return BASELINE;
}

/**
 * Build a k6 `thresholds` object from the baseline files, filtered to the
 * given scenario prefix (e.g. 'folder_cascade').
 *
 * Result shape (k6 expects metric-name → threshold-expression-array):
 *   {
 *     'http_req_duration{op:folder_cascade.list_depth8}': [
 *       'p(95)<49.5',   // 45 * (1 + 10/100)
 *       'p(99)<88.0',
 *     ],
 *   }
 *
 * @param {string} scenarioPrefix
 */
export function thresholdsFromBaseline(scenarioPrefix) {
  const baseline = BASELINE;
  const thresholds = {};
  for (const [key, val] of Object.entries(baseline)) {
    if (key.startsWith('_')) continue; // skip _comment etc.
    if (!key.startsWith(`${scenarioPrefix}.`)) continue;
    const tol = (val.tolerance_pct || 10) / 100;
    // Mirror compare.mjs: use the larger of the %-based limit and the
    // absolute-floor limit (baseline + min_delta_ms). Below sub-millisecond
    // scale, the % rule alone fires on noise — the floor stops that.
    const minDelta = val.min_delta_ms ?? 0.5;
    const p95Limit = Math.max(val.p95 * (1 + tol), val.p95 + minDelta);
    const p99Limit = Math.max(val.p99 * (1 + tol), val.p99 + minDelta);
    thresholds[`http_req_duration{op:${key}}`] = [
      `p(95)<${p95Limit.toFixed(2)}`,
      `p(99)<${p99Limit.toFixed(2)}`,
    ];
  }
  // `abortOnFail: false` (default) keeps the run going so we collect all
  // regressions in one pass; the non-zero exit at the end still fails CI.
  return thresholds;
}

/**
 * Read the seed manifest written by `cargo run --bin load-seed`.
 */
export function loadManifest() {
  const raw = open(MANIFEST_PATH);
  return JSON.parse(raw);
}
