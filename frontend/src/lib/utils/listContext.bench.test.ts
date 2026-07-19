import { describe, expect, it } from 'vitest';
import { primeContextPage } from './listContext';

/**
 * Benchmark gate for the incremental route-level `contextMap` maintenance
 * (primeContextPage) that replaced `contextMap = $derived(new Map(raw.map(...)))`
 * on the trash / recent / favorites / shared-with-me routes.
 *
 * Audit finding (ROUND16 §F2, the route-level half of the class ROUND15 §F1
 * fixed inside ResourceList): each route paged its rows in via
 * `raw = [...raw, ...page.items]` and rebuilt a brand-new Map — hashing every
 * accumulated id — on EVERY page. O(N) per page ⇒ Σ O(N²/page) across a drain,
 * plus a fresh Map instance each page. The fix holds one persistent map and
 * sets only the fresh page's entries (mirrors the shipped `favoriteIds`
 * SvelteSet, ROUND14 §F2).
 *
 * Gates (rollback rule: an AFTER that fails to beat its BEFORE fails CI):
 *  1. Equivalence — at EVERY page, the incrementally-primed map is deep-equal
 *     to a full `new Map(cumulative.map(entry))` rebuild, including skipped
 *     entries (drives → null) and the reset path.
 *  2. Perf — `entry` work collapses from Σ O(N²/page) to O(N) across the drain
 *     (deterministic call count) and wall drops ≥3x.
 */

interface Ctx {
	date: string | null;
	ownerId: string | null;
}
interface Raw {
	resource: { id: string; updated_by: string | null };
	resource_type: 'file' | 'folder' | 'drive';
	accessed_at: string;
}

const pad = (i: number) => i.toString().padStart(6, '0');

/** Item `i`; every 10th is a `drive` (skipped by the shared-with-me-style entry). */
function raw(i: number): Raw {
	return {
		resource: { id: `res-${pad(i)}`, updated_by: `user-${i % 8}` },
		resource_type: i % 10 === 0 ? 'drive' : i % 3 === 0 ? 'folder' : 'file',
		accessed_at: `2026-07-${pad((i % 27) + 1).slice(-2)}`
	};
}

/** Maps a raw item to its `[id, ctx]`, skipping drives (returns null) — counts calls. */
function makeEntry(counter?: { n: number }): (it: Raw) => readonly [string, Ctx] | null {
	return (it) => {
		if (counter) counter.n++;
		if (it.resource_type === 'drive') return null;
		return [it.resource.id, { date: it.accessed_at, ownerId: it.resource.updated_by }];
	};
}

/** Verbatim BEFORE: the old derive — a fresh Map hashing the whole cumulative list. */
function rebuild(
	cumulative: Raw[],
	entry: (it: Raw) => readonly [string, Ctx] | null
): Map<string, Ctx> {
	const m = new Map<string, Ctx>();
	for (const it of cumulative) {
		const e = entry(it);
		if (e !== null) m.set(e[0], e[1]);
	}
	return m;
}

const PAGE = 50;
const PAGES = 50; // 2 500-item drain

describe('incremental contextMap (benchmark gate)', () => {
	it('stays deep-equal to the full rebuild at every page (incl. skipped drives)', () => {
		const all = Array.from({ length: PAGE * PAGES }, (_, i) => raw(i));
		const entry = makeEntry();
		const map = new Map<string, Ctx>();
		for (let p = 1; p <= PAGES; p++) {
			const page = all.slice((p - 1) * PAGE, p * PAGE);
			primeContextPage(map, p === 1, page, entry);
			const reference = rebuild(all.slice(0, p * PAGE), entry);
			expect(new Map(map), `page ${p}`).toEqual(reference);
		}
	});

	it('clears on reset and re-primes to the reset page only', () => {
		const all = Array.from({ length: 200 }, (_, i) => raw(i));
		const entry = makeEntry();
		const map = new Map<string, Ctx>();
		primeContextPage(map, true, all.slice(0, 100), entry);
		primeContextPage(map, false, all.slice(100, 150), entry);
		// Reset with a disjoint page: prior ids must be gone.
		const resetPage = all.slice(150, 200);
		primeContextPage(map, true, resetPage, entry);
		expect(new Map(map)).toEqual(rebuild(resetPage, entry));
	});

	it('collapses entry work from Σ O(N²/page) to O(N) and runs ≥3x faster', () => {
		const N = PAGE * PAGES;
		const all = Array.from({ length: N }, (_, i) => raw(i));

		// Deterministic call-count gate (the hard rollback gate): incremental
		// computes each item's entry exactly once; the rebuild is quadratic. This
		// holds regardless of machine load.
		const afterCounter = { n: 0 };
		const afterEntry = makeEntry(afterCounter);
		const countMap = new Map<string, Ctx>();
		for (let p = 1; p <= PAGES; p++) {
			primeContextPage(countMap, p === 1, all.slice((p - 1) * PAGE, p * PAGE), afterEntry);
		}
		const beforeCounter = { n: 0 };
		const beforeEntry = makeEntry(beforeCounter);
		for (let p = 1; p <= PAGES; p++) rebuild(all.slice(0, p * PAGE), beforeEntry);
		expect(afterCounter.n).toBe(N);
		expect(beforeCounter.n).toBe((PAGES * (PAGES + 1) * PAGE) / 2);
		expect(afterCounter.n).toBeLessThan(beforeCounter.n / 5);

		// Wall gate — best-of-3 (min) per arm to shrug off scheduler / GC noise
		// under a saturated test runner (mirrors round14 §F1's `Math.min` pattern).
		const entry = makeEntry();
		const runAfter = () => {
			const m = new Map<string, Ctx>();
			const t = performance.now();
			for (let p = 1; p <= PAGES; p++) {
				primeContextPage(m, p === 1, all.slice((p - 1) * PAGE, p * PAGE), entry);
			}
			return performance.now() - t;
		};
		const runBefore = () => {
			const t = performance.now();
			for (let p = 1; p <= PAGES; p++) rebuild(all.slice(0, p * PAGE), entry);
			return performance.now() - t;
		};
		const afterMs = Math.min(runAfter(), runAfter(), runAfter());
		const beforeMs = Math.min(runBefore(), runBefore(), runBefore());

		console.info(
			`contextMap ${PAGES}×${PAGE}: before ${beforeCounter.n} entry calls / ${beforeMs.toFixed(1)} ms — after ${afterCounter.n} calls / ${afterMs.toFixed(1)} ms (${(beforeCounter.n / afterCounter.n).toFixed(1)}x fewer calls, ${(beforeMs / afterMs).toFixed(1)}x wall)`
		);

		expect(afterMs).toBeLessThan(beforeMs / 3);
	});
});
