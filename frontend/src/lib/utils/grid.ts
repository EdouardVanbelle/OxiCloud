/**
 * Number of columns a `.files-grid-view` (and the photos square grid share the
 * idea) renders at a given container width. Mirrors the CSS
 * `repeat(auto-fill, minmax(var(--grid-card-min), 1fr))` so a windowing list can
 * compute row counts that match the browser's actual wrapping exactly.
 *
 * Card-min / gap track the tokens in `lib/styles/base/variables.css` and the
 * ≤640px phone override in `lib/styles/ported/resourceList.css`.
 *
 * The phone breakpoint is watched by ONE module-level MediaQueryList listener
 * — constructing a fresh `matchMedia` per call (a style read) was the same
 * anti-pattern the photos timeline already hoisted. A flip of the media query
 * always coincides with a width change, so callers re-run anyway.
 */
let isMobile = false;
// `typeof window.matchMedia` (not just `window`): jsdom test environments
// expose `window` without implementing matchMedia.
if (typeof window !== 'undefined' && typeof window.matchMedia === 'function') {
	const mql = window.matchMedia('(max-width: 640px)');
	isMobile = mql.matches;
	mql.addEventListener('change', (e) => {
		isMobile = e.matches;
	});
}

export function gridColumns(width: number): number {
	if (width <= 0) return 1;
	const cardMin = isMobile ? 140 : 200;
	const gap = isMobile ? 8 : 20;
	return Math.max(1, Math.floor((width + gap) / (cardMin + gap)));
}
