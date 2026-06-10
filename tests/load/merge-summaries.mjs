#!/usr/bin/env node
// merge-summaries.mjs — combine multiple k6 --summary-export outputs into
// one. Used because k6 accepts a single script per invocation, but our
// regression-diff tooling (compare.mjs, bake-baseline.mjs) expects one
// summary per run.
//
// Per-scenario summaries are disjoint in their metric tags
// (folder_cascade.* vs share_cascade_rebac.* vs subject_group_nested.*),
// so merging is just a union of the `metrics` maps. The top-level
// envelope (root_group, options, etc.) is taken from the first input.
//
// Usage:
//   node merge-summaries.mjs <in1.json> [in2.json …] <out.json>

import { readFileSync, writeFileSync } from 'node:fs';
import { argv, exit } from 'node:process';

if (argv.length < 5) {
  console.error('usage: merge-summaries.mjs <in1.json> [in2.json …] <out.json>');
  exit(2);
}

const inputs = argv.slice(2, -1);
const output = argv[argv.length - 1];

const merged = JSON.parse(readFileSync(inputs[0], 'utf8'));
merged.metrics = { ...merged.metrics };

for (const path of inputs.slice(1)) {
  const next = JSON.parse(readFileSync(path, 'utf8'));
  for (const [k, v] of Object.entries(next.metrics ?? {})) {
    // If two scenarios both report a global metric (e.g. http_req_duration
    // with no op tag), the second wins — these aren't gated by baseline.json,
    // so it's only the per-`op` metrics that need to be preserved precisely.
    merged.metrics[k] = v;
  }
}

writeFileSync(output, `${JSON.stringify(merged, null, 2)}\n`);
console.log(`merged ${inputs.length} summaries → ${output}`);
