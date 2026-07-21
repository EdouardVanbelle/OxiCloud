import { describe, expect, it } from 'vitest';

/**
 * Benchmark gates for two per-page derive cleanups (round 9):
 *
 * [1] ResourceList's selection-prune `$effect` built an O(N) id `Set` on
 *     EVERY `items` change (every infinite-scroll page) even when nothing
 *     was selected — the loop it feeds never runs in that case. The shipped
 *     guard (`if (selected.size === 0) return`) makes the empty-selection
 *     page append free while keeping the pruned result byte-identical when
 *     a selection exists.
 *
 * [2] The photos timeline derive called `window.matchMedia(...)` on every
 *     recompute (every 60-photo page append) for a boolean that changes
 *     only on viewport-class crossings. The shipped code hoists it into
 *     state fed by a single MediaQueryList `change` listener.
 *
 * Both are modeled as pure replicas of the effect/derive bodies (no jsdom
 * mounting needed) with instrumentation counters, mirroring the shipped
 * control flow exactly.
 */

interface Item {
	id: string;
}

const page = (start: number, n: number): Item[] =>
	Array.from({ length: n }, (_, i) => ({ id: `it-${start + i}` }));

/** BEFORE — verbatim effect body: unconditional Set build. */
function pruneBefore(items: Item[], selected: Set<string>, counter: { setBuilds: number }) {
	counter.setBuilds++;
	const ids = new Set(items.map((i) => i.id));
	for (const id of [...selected]) {
		if (!ids.has(id)) selected.delete(id);
	}
}

/** AFTER — the shipped body: skip entirely while nothing is selected. */
function pruneAfter(items: Item[], selected: Set<string>, counter: { setBuilds: number }) {
	if (selected.size === 0) return;
	counter.setBuilds++;
	const ids = new Set(items.map((i) => i.id));
	for (const id of [...selected]) {
		if (!ids.has(id)) selected.delete(id);
	}
}

describe('selection-prune guard (benchmark gate)', () => {
	it('empty selection: zero Set builds across a 100-page drain (was 100)', () => {
		const beforeCounter = { setBuilds: 0 };
		const afterCounter = { setBuilds: 0 };
		let items: Item[] = [];
		for (let p = 0; p < 100; p++) {
			items = [...items, ...page(p * 50, 50)];
			pruneBefore(items, new Set(), beforeCounter);
			pruneAfter(items, new Set(), afterCounter);
		}
		expect(beforeCounter.setBuilds).toBe(100);
		expect(afterCounter.setBuilds).toBe(0);
	});

	it('active selection: pruned set identical to the unguarded version', () => {
		const items = page(0, 200);
		// Selection holds survivors + ids that vanished on reload.
		const seed = ['it-3', 'it-77', 'gone-1', 'it-150', 'gone-2'];
		const a = new Set(seed);
		const b = new Set(seed);
		pruneBefore(items, a, { setBuilds: 0 });
		pruneAfter(items, b, { setBuilds: 0 });
		expect([...b].sort()).toEqual([...a].sort());
		expect(b.has('gone-1')).toBe(false);
		expect(b.has('it-3')).toBe(true);
	});
});

// ── [2] matchMedia hoist ────────────────────────────────────────────────────

interface MqlStub {
	matches: boolean;
	listeners: ((e: { matches: boolean }) => void)[];
}

function makeMatchMedia(counter: { calls: number }, stub: MqlStub) {
	return () => {
		counter.calls++;
		return {
			get matches() {
				return stub.matches;
			},
			addEventListener: (_: 'change', fn: (e: { matches: boolean }) => void) => {
				stub.listeners.push(fn);
			},
			removeEventListener: () => {}
		};
	};
}

describe('photos matchMedia hoist (benchmark gate)', () => {
	it('P recomputes: 1 matchMedia call instead of P, identical booleans', () => {
		const P = 50;
		const stub: MqlStub = { matches: false, listeners: [] };

		// BEFORE — the derive body queries per recompute.
		const beforeCounter = { calls: 0 };
		const mmBefore = makeMatchMedia(beforeCounter, stub);
		const beforeValues: boolean[] = [];
		for (let i = 0; i < P; i++) {
			beforeValues.push(mmBefore().matches);
		}
		expect(beforeCounter.calls).toBe(P);

		// AFTER — one query + listener; recomputes read the state boolean.
		const afterCounter = { calls: 0 };
		const mmAfter = makeMatchMedia(afterCounter, stub);
		const mql = mmAfter();
		let isMobile = mql.matches;
		mql.addEventListener('change', (e) => {
			isMobile = e.matches;
		});
		const afterValues: boolean[] = [];
		for (let i = 0; i < P; i++) {
			afterValues.push(isMobile);
		}
		expect(afterCounter.calls).toBe(1);
		expect(afterValues).toEqual(beforeValues);

		// A viewport-class crossing propagates through the listener.
		stub.matches = true;
		for (const fn of stub.listeners) fn({ matches: true });
		expect(isMobile).toBe(true);
	});
});
