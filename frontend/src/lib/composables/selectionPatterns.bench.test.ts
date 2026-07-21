import { describe, expect, it } from 'vitest';
import {
	copyReassignModel,
	inPlaceModel,
	measureFanout,
	type SelectionModel
} from './selectionBench.svelte';

/**
 * Benchmark gate for the in-place `SvelteSet` selection/badge sets in the
 * files and recent views.
 *
 * Audit finding: `selected`, `favoriteIds` and `sharedIds` were plain
 * `$state<Set>`s rebuilt from a full copy on every single-item toggle
 * (`new SvelteSet(selected)` + reassign). That costs (a) an O(N) copy per
 * toggle — N unbounded under "select all → refine" — and (b) reassigning the
 * state reference invalidates EVERY mounted row's `.has(id)` read, so the
 * whole viewport re-renders for a one-row change. The fix keeps one
 * `SvelteSet` per set and mutates it in place; `SvelteSet` tracks per-key, so
 * a toggle re-runs only the toggled row's readers. The composable
 * `useSelection` already shipped this pattern — the views now match it.
 *
 * `SvelteSet` granularity (svelte/src/reactivity/set.js): present keys get a
 * per-key source; `.has()` on an ABSENT key tracks the set's version signal
 * ("don't create sources willy-nilly"), so miss-readers re-run on any
 * mutation in both patterns. The in-place win is therefore: no O(N) copy, and
 * every OTHER present-key reader is spared — copy-reassign re-runs all rows.
 *
 * Gates: (1) both patterns agree on membership across a deterministic toggle
 * script; (2) fan-out under 40 mounted row-effects matches those exact
 * semantics (misses+1 in place vs all 40 copied — 3 vs 40 when the list is
 * mostly selected, the "select all → refine" case); (3) 1 000 toggles over a
 * 5 000-id selection run ≥5x faster in place.
 */

/** Deterministic PRNG so both models replay the identical script. */
function mulberry32(seed: number): () => number {
	let a = seed >>> 0;
	return () => {
		a = (a + 0x6d2b79f5) | 0;
		let t = Math.imul(a ^ (a >>> 15), 1 | a);
		t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
		return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
	};
}

const ids = (n: number): string[] => Array.from({ length: n }, (_, i) => `id-${i}`);

describe('in-place SvelteSet selection (benchmark gate)', () => {
	it('membership after a 500-op toggle script is identical in both patterns', () => {
		const universe = ids(1_000);
		const a = copyReassignModel();
		const b = inPlaceModel();
		a.seed(universe.slice(0, 100));
		b.seed(universe.slice(0, 100));

		const rand = mulberry32(0xc0ffee);
		for (let i = 0; i < 500; i++) {
			const id = universe[Math.floor(rand() * universe.length)];
			a.toggle(id);
			b.toggle(id);
		}
		expect(a.size).toBe(b.size);
		for (const id of universe) {
			expect(b.has(id), id).toBe(a.has(id));
		}
	});

	it('fan-out of one toggle across 40 mounted rows matches per-key semantics', () => {
		const rows = ids(40);
		const scenario = (seeded: number): { copy: number; inplace: number } => {
			const copy = copyReassignModel();
			copy.seed(rows.slice(0, seeded));
			const copyFanout = measureFanout(copy, rows, () => copy.toggle('id-7'));

			const inplace = inPlaceModel();
			inplace.seed(rows.slice(0, seeded));
			const inplaceFanout = measureFanout(inplace, rows, () => inplace.toggle('id-7'));
			return { copy: copyFanout, inplace: inplaceFanout };
		};

		// 10/40 selected (sparse selection): misses (30) + the toggled row.
		const sparse = scenario(10);
		// 38/40 selected ("select all → refine"): misses (2) + the toggled row.
		const dense = scenario(38);

		console.info(
			`fan-out of 1 toggle across 40 row effects — 10/40 selected: copy ${sparse.copy} vs in-place ${sparse.inplace}; 38/40 selected: copy ${dense.copy} vs in-place ${dense.inplace}`
		);
		// Copy-reassign invalidates every row that reads `.has` on the state.
		expect(sparse.copy).toBeGreaterThanOrEqual(rows.length);
		expect(dense.copy).toBeGreaterThanOrEqual(rows.length);
		// In place: absent-key readers track the version signal (SvelteSet
		// design), present-key readers other than the toggled row are spared.
		expect(sparse.inplace).toBe(40 - 10 + 1);
		expect(dense.inplace).toBe(40 - 38 + 1);
		// The refine-after-select-all case is where the win is decisive.
		expect(dense.inplace).toBeLessThan(dense.copy / 10);
	});

	it('1 000 toggles over a 5 000-id selection are ≥5x faster in place (perf gate)', () => {
		const N = 5_000;
		const TOGGLES = 1_000;
		const universe = ids(N);

		const run = (model: SelectionModel): number => {
			model.seed(universe);
			const rand = mulberry32(0xbeef);
			const t0 = performance.now();
			for (let i = 0; i < TOGGLES; i++) {
				model.toggle(universe[Math.floor(rand() * N)]);
			}
			return performance.now() - t0;
		};

		// Warm-up (JIT) then measure.
		run(copyReassignModel());
		run(inPlaceModel());
		const copyMs = run(copyReassignModel());
		const inplaceMs = run(inPlaceModel());

		console.info(
			`${TOGGLES} toggles @ N=${N}: copy-reassign ${copyMs.toFixed(1)} ms vs in-place ${inplaceMs.toFixed(1)} ms (${(copyMs / inplaceMs).toFixed(1)}x)`
		);
		expect(inplaceMs).toBeLessThan(copyMs / 5);
	});
});
