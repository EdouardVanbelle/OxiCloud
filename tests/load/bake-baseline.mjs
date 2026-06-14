#!/usr/bin/env node
// bake-baseline.mjs — convert the latest k6 --summary-export into the
// baseline shape that compare.mjs reads.
//
// Tolerance values from the existing baseline are preserved; only the p50/
// p95/p99 numbers are overwritten. Run this after `just load` to lock in a
// new accepted bar, then commit the result deliberately.
//
// Usage:
//   node bake-baseline.mjs                       # auto-pick latest summary, target baseline/load.json
//   node bake-baseline.mjs <summary> <baseline>  # explicit paths (e.g. baseline/smoke.json)

import { readFileSync, writeFileSync, readdirSync, statSync } from 'node:fs';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { argv, exit } from 'node:process';

const HERE = dirname(fileURLToPath(import.meta.url));

function latestSummary() {
  const dir = join(HERE, 'results');
  const candidates = readdirSync(dir)
    .filter((f) => f.startsWith('run-') && f.endsWith('.json'))
    .map((f) => ({ f, mtime: statSync(join(dir, f)).mtimeMs }))
    .sort((a, b) => b.mtime - a.mtime);
  if (candidates.length === 0) {
    console.error('bake-baseline: no run-*.json under tests/load/results/');
    exit(2);
  }
  return join(dir, candidates[0].f);
}

const summaryPath = argv[2] ? resolve(argv[2]) : latestSummary();
const baselinePath = argv[3] ? resolve(argv[3]) : join(HERE, 'baseline/load.json');

const summary = JSON.parse(readFileSync(summaryPath, 'utf8'));
const baseline = JSON.parse(readFileSync(baselinePath, 'utf8'));

const metricKeyFor = (op) => `http_req_duration{op:${op}}`;
const PERCENTILES = [
  { baseline: 'p50', k6: 'med' },
  { baseline: 'p95', k6: 'p(95)' },
  { baseline: 'p99', k6: 'p(99)' },
];

let updated = 0;
let missing = 0;

for (const [op, entry] of Object.entries(baseline)) {
  if (op.startsWith('_')) continue;
  const metric = summary.metrics?.[metricKeyFor(op)];
  if (!metric) {
    console.warn(`  missing: ${op} (no data in ${summaryPath})`);
    missing++;
    continue;
  }
  // k6 --summary-export puts percentile values directly on the metric object
  // (alongside `thresholds`), not inside a `.values` wrapper.
  let touched = false;
  for (const p of PERCENTILES) {
    const cur = metric[p.k6];
    if (cur === undefined) continue;
    entry[p.baseline] = Number(cur.toFixed(2));
    touched = true;
  }
  if (touched) updated++;
}

writeFileSync(baselinePath, `${JSON.stringify(baseline, null, 2)}\n`);
console.log(`bake-baseline: updated ${updated} metric(s), missing ${missing} in ${baselinePath}`);
console.log(`Source: ${summaryPath}`);
console.log('Commit deliberately: chore(load): accept new baseline for <reason>');
