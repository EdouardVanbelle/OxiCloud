#!/usr/bin/env node
// compare.mjs — regression diff for k6 load-test runs.
//
// Reads a k6 --summary-export JSON and the committed baseline, diffs the
// p50/p95/p99 of every baseline metric against the current run, prints a
// human-readable table, and exits non-zero if any metric regresses beyond
// its tolerance.
//
// Usage:
//   node compare.mjs <summary.json> <baseline.json>
//
// Exit codes:
//   0 — every metric within tolerance.
//   1 — one or more metrics regressed, or a baseline metric is absent from
//       the current summary (suite drift — either a scenario was deleted
//       or a tag was renamed without updating baseline.json).

import { readFileSync } from 'node:fs';
import { argv, exit } from 'node:process';

if (argv.length < 4) {
  console.error('usage: compare.mjs <summary.json> <baseline.json>');
  exit(2);
}

const summaryPath = argv[2];
const baselinePath = argv[3];

const summary = JSON.parse(readFileSync(summaryPath, 'utf8'));
const baseline = JSON.parse(readFileSync(baselinePath, 'utf8'));

// k6 summary metrics are keyed verbatim with the tag: e.g.
// "http_req_duration{op:smoke.login}".
const metricKeyFor = (op) => `http_req_duration{op:${op}}`;

// Map k6 percentile field names → baseline field names.
const PERCENTILES = [
  { baseline: 'p50', k6: 'med' },
  { baseline: 'p95', k6: 'p(95)' },
  { baseline: 'p99', k6: 'p(99)' },
];

const rows = [];
let anyRegression = false;
let anyMissing = false;

for (const [op, base] of Object.entries(baseline)) {
  if (op.startsWith('_')) continue; // skip _comment etc.
  const metric = summary.metrics?.[metricKeyFor(op)];
  if (!metric) {
    rows.push({ op, status: 'MISSING', detail: 'no data for this tag in current summary' });
    anyMissing = true;
    continue;
  }
  const tol = (base.tolerance_pct ?? 10) / 100;
  // Noise-floor guard: at sub-millisecond percentiles, a 10% tolerance is
  // smaller than a single context switch. Require the absolute delta to
  // exceed `min_delta_ms` too — otherwise the swing is below the measurement
  // floor and we don't flag it. Default 0.5ms is conservative for the load
  // suite's typical 0.3–3ms range. Override per metric in baseline.json.
  const minDelta = base.min_delta_ms ?? 0.5;

  for (const p of PERCENTILES) {
    const baseVal = base[p.baseline];
    // k6 --summary-export puts percentile values directly on the metric, not
    // inside a `.values` wrapper.
    const curVal = metric[p.k6];
    if (baseVal === undefined || curVal === undefined) continue;

    const deltaPct = ((curVal - baseVal) / baseVal) * 100;
    const deltaAbs = curVal - baseVal;
    const limitPct = baseVal * (1 + tol);
    // Regression only if BOTH the % rule AND the absolute floor are breached.
    const regressed = curVal > limitPct && deltaAbs > minDelta;
    if (regressed) anyRegression = true;
    rows.push({
      op,
      percentile: p.baseline,
      base: baseVal,
      cur: curVal,
      deltaPct,
      regressed,
      tol: base.tolerance_pct ?? 10,
    });
  }
}

// ── Render table ──────────────────────────────────────────────────────────

const ms = (v) => `${v.toFixed(1)}ms`;
const pct = (v) => `${v >= 0 ? '+' : ''}${v.toFixed(1)}%`;

function pad(s, n) {
  if (s.length >= n) return s;
  return s + ' '.repeat(n - s.length);
}

const colOp = Math.max(20, ...rows.map((r) => r.op.length));
const colP = 5;
const colVal = 11;

console.log();
console.log(
  pad('metric', colOp + 1) +
    pad('pctl', colP + 1) +
    pad('baseline', colVal + 1) +
    pad('current', colVal + 1) +
    pad('delta', 9) +
    'status',
);
console.log('-'.repeat(colOp + colP + colVal * 2 + 9 + 12));

for (const r of rows) {
  if (r.status === 'MISSING') {
    console.log(`${pad(r.op, colOp + 1)}${pad('-', colP + 1)}${pad('-', colVal + 1)}${pad('-', colVal + 1)}${pad('-', 9)}MISSING — ${r.detail}`);
    continue;
  }
  const status = r.regressed ? `REGRESSION (tol ±${r.tol}%)` : 'ok';
  console.log(
    pad(r.op, colOp + 1) +
      pad(r.percentile, colP + 1) +
      pad(ms(r.base), colVal + 1) +
      pad(ms(r.cur), colVal + 1) +
      pad(pct(r.deltaPct), 9) +
      status,
  );
}

console.log();
if (anyRegression || anyMissing) {
  if (anyRegression) console.error('compare.mjs: one or more metrics regressed beyond tolerance.');
  if (anyMissing) console.error('compare.mjs: one or more baseline metrics are missing from the current summary.');
  exit(1);
}
console.log('compare.mjs: all metrics within tolerance.');
exit(0);
