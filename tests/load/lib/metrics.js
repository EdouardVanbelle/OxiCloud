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
const BASELINE_PATH = import.meta.resolve('../baseline/baseline.json');
const MANIFEST_PATH = import.meta.resolve('../results/seed-manifest.json');

/**
 * Read the baseline JSON. K6's `open()` is only valid in init context, so
 * scenarios must call this at module top-level, never inside default().
 */
export function loadBaseline() {
  const raw = open(BASELINE_PATH);
  return JSON.parse(raw);
}

/**
 * Build a k6 `thresholds` object from the baseline file, filtered to the
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
  const baseline = loadBaseline();
  const thresholds = {};
  for (const [key, val] of Object.entries(baseline)) {
    if (key.startsWith('_')) continue; // skip _comment etc.
    if (!key.startsWith(`${scenarioPrefix}.`)) continue;
    const tol = (val.tolerance_pct || 10) / 100;
    const p95Limit = val.p95 * (1 + tol);
    const p99Limit = val.p99 * (1 + tol);
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
