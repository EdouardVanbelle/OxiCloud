#!/usr/bin/env node
// Generates docs/TOKENS.md — a grouped reference of every design token defined
// in static/css/base/variables.css. Run after changing tokens:
//   node scripts/gen-token-docs.mjs
import { readFileSync, writeFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const repo = join(dirname(fileURLToPath(import.meta.url)), '..');
const src = readFileSync(join(repo, 'static', 'css', 'base', 'variables.css'), 'utf8');

const tokens = [];
for (const m of src.matchAll(/^\s*(--[a-z0-9-]+)\s*:\s*([^;]+);/gim)) {
    if (!tokens.some((t) => t[0] === m[1])) tokens.push([m[1], m[2].trim().replace(/\s+/g, ' ')]);
}

const CATEGORIES = [
    ['Spacing (4px grid)', /^--space-/],
    ['Radius', /^--radius/],
    ['Typography', /^--(text-|leading-|weight-|tracking-|font-|measure-|icon-)/],
    ['Z-index layers', /^--z-/],
    ['Motion (durations + easing)', /^--(motion-|ease-|spin-)/],
    ['Elevation (composed shadows)', /^--shadow-/],
    ['Breakpoints (reference)', /^--bp-/],
    ['Density', /^--density-/],
    ['Layout shell', /^--(sidebar-width|gutter|grid-card)/],
    ['Color — text', /^--color-text/],
    ['Color — background', /^--color-bg/],
    ['Color — border', /^--color-border/],
    ['Color — accent / brand', /^--color-(accent|logo|focus|on-accent|primary)/],
    ['Color — semantic (success/warn/danger/info)', /^--color-(success|error|danger|warning|info)/],
    ['Color — shadow alphas / overlays', /^--color-(shadow|overlay|on-overlay)/],
    ['Color — sidebar', /^--color-sidebar/],
    ['Color — file types', /^--color-ft/],
    ['Color — calendar dots', /^--color-cal/],
    ['Color — badges', /^--color-badge/],
    ['Color — other', /^--color-/],
    ['Other', /.*/]
];

const buckets = new Map(CATEGORIES.map(([name]) => [name, []]));
for (const [name, value] of tokens) {
    const cat = CATEGORIES.find(([, re]) => re.test(name))[0];
    buckets.get(cat).push([name, value]);
}

let md = `# Design tokens

> Auto-generated from \`static/css/base/variables.css\` by \`scripts/gen-token-docs.mjs\`.
> Do not edit by hand — re-run the generator after changing tokens.

**${tokens.length} tokens** across ${[...buckets].filter(([, v]) => v.length).length} groups.
\n`;

for (const [name, rows] of buckets) {
    if (!rows.length) continue;
    md += `\n## ${name}\n\n| Token | Value |\n| --- | --- |\n`;
    for (const [tok, val] of rows) md += `| \`${tok}\` | \`${val.replace(/\|/g, '\\|')}\` |\n`;
}

writeFileSync(join(repo, 'docs', 'TOKENS.md'), md);
console.log(`✓ Wrote docs/TOKENS.md (${tokens.length} tokens).`);
