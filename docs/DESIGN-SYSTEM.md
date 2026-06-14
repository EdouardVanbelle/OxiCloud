# OxiCloud Design System

The single source of truth for how the frontend looks, behaves, and stays
accessible. Pairs with [TOKENS.md](TOKENS.md) (auto-generated token reference)
and [UIUX-ROADMAP.md](UIUX-ROADMAP.md) (the work plan).

---

## 1. Design tokens

Everything visual routes through a token in
[`static/css/base/variables.css`](../static/css/base/variables.css). Six scales,
plus a curated color system. **Never hand-write a raw px/hex for these** — consume
a token so the next change happens in one place.

| Axis | Tokens | Notes |
| --- | --- | --- |
| Spacing | `--space-0 … --space-24` | 4px grid (+ half-steps 2/6/10/14). |
| Radius | `--radius-xs … --radius-4xl`, `--radius-full` | `--radius` aliases `--radius-2xl`. |
| Type | `--text-2xs … --text-6xl`, `--leading-*`, `--weight-*`, `--tracking-*` | rem-based (honors zoom). `.heading-page/-section/-card` utilities decouple role from element. |
| Z-index | `--z-base … --z-max` | Semantic layers with gaps; no magic numbers. |
| Motion | `--motion-fast/base/moderate/slow`, `--ease-standard/-emphasized/...` | `--ease-standard` is the decelerate default for entrances. |
| Elevation | `--shadow-xs … --shadow-2xl` | Composed recipes layered on the `--color-shadow-*` alphas. |

**Color.** One brand accent (orange `#ff5e3a`), one neutral slate ramp, and five
semantics (success/warning/danger/info + accent), each unified to a single hue.
Text tiers collapse to a handful that all clear **WCAG AA (4.5:1)** — verified by
[`scripts/check-contrast.mjs`](../scripts/check-contrast.mjs). Use
`--color-accent-text` (not `--color-accent`) for accent *text/links*.

Governance: **one brand accent, one neutral ramp, five semantics — no new hero
hues.** Run `node scripts/gen-token-docs.mjs` after editing tokens.

---

## 2. Accessibility rules (non-negotiable)

These are enforced/aided by CI scripts and must hold for every new surface.

- **Keyboard.** Everything interactive is reachable and operable by keyboard.
  Use `<button>`/`<a>` (not clickable `<div>`s). Nav exposes `aria-current="page"`.
- **Focus.** A global `:focus-visible` ring lives in
  [`base/a11y.css`](../static/css/base/a11y.css). Never `outline: none` without a
  paired `:focus-visible` style. Mouse focus stays ring-free; keyboard focus never.
- **Contrast.** Every text/background pair clears 4.5:1 in **light AND dark** —
  `node scripts/check-contrast.mjs` fails the build otherwise.
- **Landmarks + skip link.** `<nav aria-label>`, `role="main"`/`<main id="main">`,
  a skip link as the first focusable element, and `lang` on `<html>`.
- **Headings.** Exactly one `h1` per page, no skipped levels —
  `node scripts/check-headings.mjs`.
- **Motion.** `@media (prefers-reduced-motion: reduce)` neutralizes animation
  globally. Also honor `prefers-contrast` and `forced-colors`.
- **Dialogs.** `role="dialog"` + `aria-modal` + `aria-labelledby`, focus trapped
  while open, focus restored to the trigger on close (see
  [`modal.js`](../static/js/components/modal.js)).
- **Icon-only buttons.** Always an `aria-label`; decorative icons get
  `aria-hidden="true"`.
- **Touch targets.** ≥44×44px on phones.

---

## 3. Brand

- **Mark.** The cloud glyph ([`logo-plain.svg`](../static/logo/logo-plain.svg)).
  Always rendered from the real SVG (never a stock `fa-cloud`). On surfaces it
  sits in an accent-gradient tile via the `.brand-mark` component.
- **Wordmark.** "OxiCloud", weight 700, tight tracking. The accent-coloured
  "Oxi" lockup is the ownable treatment.
- **Accent.** Orange→coral `#ff5e3a → #ff2d55` — warm and distinctive in a
  blue-dominated cloud-storage market; nods to Rust oxidation. It is the *only*
  brand/primary hue (blue/indigo/purple were demoted to the single info ramp).
- **Logo gradient.** One token (`--color-logo-gradient`) everywhere.
- **Clear-space / min-size.** Keep ≥ the tile's corner-radius of padding around
  the mark; don't render the wordmark below ~16px.
- **Don't:** recolor the mark, stretch it, place it on a low-contrast background,
  or introduce a second "primary" hue.
- **Maskable / OG.** [`logo-maskable.svg`](../static/logo/logo-maskable.svg) (PWA,
  safe-zone) and [`og-image.svg`](../static/logo/og-image.svg) (social). Export
  both to PNG for full platform support (see roadmap).

---

## 4. CI guardrails (what keeps the score from regressing)

Run all the dependency-free checks with **`just frontend-check`**:

| Script | Enforces |
| --- | --- |
| `check-contrast.mjs` | WCAG AA on every text/bg + semantic token pair (light + dark). |
| `check-headings.mjs` | One `h1`, no skipped levels, per HTML page. |
| `check-locales.mjs` | Locale completeness + `{placeholder}` integrity across all 16 locales. |
| `check-dead-tokens.mjs` | Reports tokens defined but never referenced (cleanup aid). |
| `check-brand-drift.mjs` | Locks the logo SVG hash + `--color-logo-gradient` against silent change. |
| `gen-token-docs.mjs` | Regenerates [TOKENS.md](TOKENS.md). |

**Build pipeline.** `build.rs` (release) bundles + content-hashes + minifies all
CSS/JS into `static-dist/` using the Rust crates **lightningcss** and **oxc** — no
npm. It also injects early `<link rel="preload">` / `<link rel="modulepreload">`
hints for the hashed bundles. Debug builds (`cargo run`) serve raw `static/`
unprocessed, so any source CSS must be valid as-authored (e.g. `@custom-media`
is off the table — it has no native browser support).

Still to wire (genuinely need npm devDependencies / browsers): stylelint rules
banning raw px/hex outside `:root`, axe-core + Playwright (keyboard,
visual-regression, dark-parity, cross-browser), and Lighthouse-CI perf budgets.
