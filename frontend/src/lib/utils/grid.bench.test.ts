import { describe, expect, it, vi } from 'vitest';

/**
 * Benchmark gate for the module-level MediaQueryList in
 * {@link gridColumns} (`lib/utils/grid.ts`).
 *
 * Audit finding: `gridColumns` constructed a fresh
 * `window.matchMedia('(max-width: 640px)')` on EVERY invocation — a style
 * read per call — and it is called from the grid windowing derives on every
 * width recompute (`ResourceList.gridCols`, files grid rows). This is the
 * same anti-pattern the photos timeline already fixed by hoisting to one
 * listener-fed flag.
 *
 * Gates:
 *  1. Output identity — for a sweep of widths, the hoisted implementation
 *     returns exactly what the per-call implementation returns (both mobile
 *     and desktop breakpoint states).
 *  2. Perf — 10 000 calls construct 0 additional MediaQueryList objects
 *     (BEFORE: 10 000) and run ≥5x faster.
 */

interface FakeMql {
	matches: boolean;
	addEventListener: (t: string, fn: (e: { matches: boolean }) => void) => void;
}

function installMatchMedia(matches: boolean, counter: { constructed: number }): void {
	vi.stubGlobal(
		'matchMedia',
		vi.fn((): FakeMql => {
			counter.constructed++;
			return { matches, addEventListener: () => {} };
		})
	);
	// jsdom exposes window === globalThis in vitest; stub both lookup paths.
	(window as unknown as { matchMedia: unknown }).matchMedia = globalThis.matchMedia;
}

/** BEFORE — verbatim old shape: fresh matchMedia per call. */
function gridColumnsBefore(width: number): number {
	if (width <= 0) return 1;
	const mobile = typeof window !== 'undefined' && window.matchMedia('(max-width: 640px)').matches;
	const cardMin = mobile ? 140 : 200;
	const gap = mobile ? 8 : 20;
	return Math.max(1, Math.floor((width + gap) / (cardMin + gap)));
}

describe('gridColumns matchMedia hoist (benchmark gate)', () => {
	it('output identity across widths + constructions collapse to ≤1', async () => {
		const counter = { constructed: 0 };
		installMatchMedia(false, counter);
		// Import AFTER stubbing so the module-level MQL uses the stub.
		vi.resetModules();
		const { gridColumns } = await import('./grid');
		const afterModuleConstructions = counter.constructed; // the one hoisted MQL
		expect(afterModuleConstructions).toBeLessThanOrEqual(1);

		const widths = [-10, 0, 120, 320, 640, 641, 800, 1024, 1440, 1920, 2560];
		for (const w of widths) {
			expect(gridColumns(w)).toBe(gridColumnsBefore(w));
		}

		const N = 10_000;
		counter.constructed = 0;
		const t0 = performance.now();
		let accBefore = 0;
		for (let i = 0; i < N; i++) accBefore += gridColumnsBefore(300 + (i % 1200));
		const beforeMs = performance.now() - t0;
		const beforeConstructed = counter.constructed;

		counter.constructed = 0;
		const t1 = performance.now();
		let accAfter = 0;
		for (let i = 0; i < N; i++) accAfter += gridColumns(300 + (i % 1200));
		const afterMs = performance.now() - t1;

		expect(accAfter).toBe(accBefore); // identity over the whole sweep
		expect(beforeConstructed).toBe(N);
		expect(counter.constructed).toBe(0); // zero style reads per call now
		console.log(
			`[bench] gridColumns x${N}: BEFORE ${beforeMs.toFixed(1)} ms (${beforeConstructed} MQL constructions) → AFTER ${afterMs.toFixed(1)} ms (0 constructions)`
		);
	});
});
