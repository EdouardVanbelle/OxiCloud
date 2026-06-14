#!/usr/bin/env node
// WCAG AA contrast guardrail.
//
// Resolves every text/background design token (through light-dark() and var()
// aliases) and fails (exit 1) if any text-on-surface or semantic text-on-tint
// pair drops below 4.5:1 in either light or dark mode. Keeps the palette from
// silently regressing into unreadable greys.
//
//   node scripts/check-contrast.mjs
import { readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const varsPath = join(
    dirname(fileURLToPath(import.meta.url)),
    '..',
    'static',
    'css',
    'base',
    'variables.css'
);
const src = readFileSync(varsPath, 'utf8');

/** First (\:root) definition of each token. */
const raw = {};
for (const m of src.matchAll(/(--[a-z0-9-]+)\s*:\s*([^;]+);/gi)) {
    if (!(m[1] in raw)) raw[m[1]] = m[2].trim();
}

function resolve(val, mode, depth = 0) {
    if (depth > 12 || val == null) return null;
    val = String(val).trim();
    let m = val.match(/^light-dark\(\s*(.+?),\s*(.+)\)\s*$/);
    if (m) return resolve(mode === 'light' ? m[1] : m[2], mode, depth + 1);
    m = val.match(/^var\(\s*(--[a-z0-9-]+)\s*\)/);
    if (m) return resolve(raw[m[1]], mode, depth + 1);
    m = val.match(/^#([0-9a-fA-F]{3,8})\b/);
    if (m) {
        let h = m[1];
        if (h.length === 3)
            h = [...h].map((c) => c + c).join('');
        return '#' + h.slice(0, 6).toLowerCase();
    }
    if (val === 'white') return '#ffffff';
    if (val === 'black') return '#000000';
    return null;
}

const lin = (c) => {
    c /= 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
};
const lum = (h) =>
    0.2126 * lin(parseInt(h.slice(1, 3), 16)) +
    0.7152 * lin(parseInt(h.slice(3, 5), 16)) +
    0.0722 * lin(parseInt(h.slice(5, 7), 16));
const ratio = (fg, bg) => {
    const a = lum(fg);
    const b = lum(bg);
    return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
};

const texts = [
    '--color-text', '--color-text-heading', '--color-text-secondary', '--color-text-muted',
    '--color-text-subtle', '--color-text-faint', '--color-text-placeholder', '--color-text-gray',
    '--color-text-medium', '--color-text-light', '--color-text-faint2', '--color-text-dim', '--color-text-dark'
];
const bgs = [
    '--color-bg-surface', '--color-bg-page', '--color-bg-hover', '--color-bg-input',
    '--color-bg-subtle', '--color-bg-muted', '--color-bg-input-alt'
];
const semantic = [
    ['--color-success-text', '--color-success-bg'],
    ['--color-error-text', '--color-error-bg'],
    ['--color-warning-text', '--color-warning-bg'],
    ['--color-info-text', '--color-info-bg'],
    ['--color-accent-text', '--color-bg-surface'],
    ['--color-accent-text', '--color-bg-page'],
    ['--color-accent-text', '--color-bg-muted']
];

const fails = [];
for (const mode of ['light', 'dark']) {
    for (const t of texts)
        for (const b of bgs) {
            const fg = resolve(raw[t], mode);
            const bg = resolve(raw[b], mode);
            if (fg && bg && ratio(fg, bg) < 4.5)
                fails.push(`${mode}: ${t}(${fg}) on ${b}(${bg}) = ${ratio(fg, bg).toFixed(2)}`);
        }
    for (const [t, b] of semantic) {
        const fg = resolve(raw[t], mode);
        const bg = resolve(raw[b], mode);
        if (fg && bg && ratio(fg, bg) < 4.5)
            fails.push(`${mode}: ${t}(${fg}) on ${b}(${bg}) = ${ratio(fg, bg).toFixed(2)}`);
    }
}

if (fails.length) {
    console.error('✖ WCAG AA contrast failures (<4.5:1):\n  ' + fails.join('\n  '));
    process.exit(1);
}
console.log('✓ All text/background token pairs pass WCAG AA (4.5:1) in light and dark.');
