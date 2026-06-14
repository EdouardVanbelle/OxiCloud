#!/usr/bin/env node
// Heading-order guardrail: each page has at least one <h1> and never skips a
// level going deeper (e.g. h1 → h3 without an h2). Fails (exit 1) on violations.
//
//   node scripts/check-headings.mjs
import { readFileSync, readdirSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const staticDir = join(dirname(fileURLToPath(import.meta.url)), '..', 'static');
const pages = readdirSync(staticDir).filter((f) => f.endsWith('.html'));
let problems = 0;

for (const page of pages) {
    const html = readFileSync(join(staticDir, page), 'utf8');
    const levels = [...html.matchAll(/<h([1-6])\b/gi)].map((m) => Number(m[1]));
    if (!levels.length) continue; // heading-less shells (error pages) are fine
    const issues = [];
    if (!levels.includes(1)) issues.push('no <h1>');
    let prev = 0;
    for (const lvl of levels) {
        if (prev && lvl > prev + 1) issues.push(`skips h${prev}→h${lvl}`);
        prev = lvl;
    }
    if (issues.length) {
        problems++;
        console.error(`${page}: ${issues.join(', ')}  [order: ${levels.join(',')}]`);
    }
}

if (problems) {
    console.error(`\n✖ ${problems} page(s) with heading-order issues.`);
    process.exit(1);
}
console.log('✓ All pages have an h1 and no skipped heading levels.');
