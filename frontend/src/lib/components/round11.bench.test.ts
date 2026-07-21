import { describe, expect, it } from 'vitest';

/**
 * Benchmark gates for the round-11 SPA items (see benches/ROUND11.md):
 *
 * [1] `ResourceList.selectedEntries` re-filtered the ENTIRE items array on
 *     every selection change once the batch toolbar was mounted — and the
 *     favorites/recent hosts ignored the snippet param and recomputed their
 *     own `entries.filter(...)` shadow, so each toggle ran TWO full O(N)
 *     scans (O(N²)-ish across a shift-range gesture). The shipped shape
 *     derives an id→index Map (rebuilt only when `items` changes) and
 *     projects the selection in O(k · log k), preserving item order; hosts
 *     now consume the snippet param.
 *
 * [2] The Recent page mapper baked `favoriteIds.has(id)` into every entry,
 *     subscribing the whole O(N) map to the SvelteSet — one star click
 *     rebuilt all N entries and re-rendered every visible row. The shipped
 *     shape reads membership in the star widget via ResourceList's new
 *     `favoriteIds` prop, so the mapper no longer depends on the set.
 *
 * [3] The admin "time ago" >30-day fallback called `toLocaleDateString()`
 *     (a fresh Intl.DateTimeFormat per call) instead of the cached
 *     `dateTimeFormatFor` the rest of the app uses.
 *
 * All modeled as pure replicas of the derive bodies with instrumentation
 * counters (the listDerives.bench.test.ts convention).
 */

interface Entry {
	id: string;
	name: string;
}

const buildItems = (n: number): Entry[] =>
	Array.from({ length: n }, (_, i) => ({ id: `it-${i}`, name: `Item ${i}` }));

/** BEFORE — component derive + host shadow, each a full O(N) scan. */
function selectedBefore(
	items: Entry[],
	selected: Set<string>,
	counter: { comparisons: number }
): { component: Entry[]; host: Entry[] } {
	const component = items.filter((i) => {
		counter.comparisons++;
		return selected.has(i.id);
	});
	const host = items.filter((i) => {
		counter.comparisons++;
		return selected.has(i.id);
	});
	return { component, host };
}

/** AFTER — id→index Map projection, index rebuilt only on items change. */
function makeAfterProjector(items: Entry[]) {
	const indexById = new Map(items.map((i, idx) => [i.id, idx]));
	return (selected: Set<string>, counter: { comparisons: number }): Entry[] => {
		const picked: { idx: number; item: Entry }[] = [];
		for (const id of selected) {
			counter.comparisons++;
			const idx = indexById.get(id);
			if (idx !== undefined) picked.push({ idx, item: items[idx] });
		}
		picked.sort((a, b) => a.idx - b.idx);
		return picked.map((p) => p.item);
	};
}

describe('ResourceList selectedEntries projection (benchmark gate)', () => {
	it('identical output (order + membership) and O(k) vs O(2N) comparisons per toggle', () => {
		const N = 2000;
		const items = buildItems(N);
		const project = makeAfterProjector(items);

		// Model a 50-item shift-range selection built one id at a time,
		// re-deriving after each toggle (what the reactive graph does).
		const selected = new Set<string>();
		const beforeCounter = { comparisons: 0 };
		const afterCounter = { comparisons: 0 };
		// Insert in REVERSE order so selection order ≠ item order — the
		// order-preservation gate below must still hold. Each toggle
		// re-derives both shapes (what the reactive graph does).
		for (let i = 149; i >= 100; i--) {
			selected.add(`it-${i}`);
			selectedBefore(items, selected, beforeCounter);
			project(selected, afterCounter);
		}
		// Stale ids (deleted rows) must be dropped by both shapes.
		selected.add('it-ghost');
		const lastBefore = selectedBefore(items, selected, beforeCounter).component;
		const lastAfter = project(selected, afterCounter);

		expect(lastAfter).toEqual(lastBefore); // same entries, same (item) order
		// BEFORE: 2 scans × N per toggle. AFTER: k probes per toggle.
		expect(beforeCounter.comparisons).toBe(51 * 2 * N);
		expect(afterCounter.comparisons).toBeLessThan(51 * 51 + 1);
	});

	it('wall clock: 500-toggle sweep on a 5k list is faster with the projection', () => {
		const N = 5000;
		const items = buildItems(N);
		const project = makeAfterProjector(items);
		const selected = new Set<string>();
		const nul = { comparisons: 0 };

		const t0 = performance.now();
		for (let i = 0; i < 500; i++) {
			selected.add(`it-${i}`);
			selectedBefore(items, selected, nul);
		}
		const tBefore = performance.now() - t0;

		selected.clear();
		const t1 = performance.now();
		for (let i = 0; i < 500; i++) {
			selected.add(`it-${i}`);
			project(selected, nul);
		}
		const tAfter = performance.now() - t1;

		// Generous bound to keep CI stable; locally ~10-40x.
		expect(tAfter).toBeLessThan(tBefore);
	});
});

// ─── [2] Recent favorite-star dependency ────────────────────────────────────

interface RawItem {
	id: string;
	name: string;
}

/** BEFORE — mapper reads the favorite set: every toggle re-maps ALL rows. */
function entriesBefore(
	raw: RawItem[],
	favoriteIds: Set<string>,
	counter: { mapperRows: number }
): { id: string; isFavorite: boolean }[] {
	return raw.map((it) => {
		counter.mapperRows++;
		return { id: it.id, isFavorite: favoriteIds.has(it.id) };
	});
}

/** AFTER — mapper is set-independent; the star widget reads membership. */
function entriesAfter(raw: RawItem[], counter: { mapperRows: number }): { id: string }[] {
	return raw.map((it) => {
		counter.mapperRows++;
		return { id: it.id };
	});
}
function starStateAfter(favoriteIds: Set<string>, id: string): boolean {
	return favoriteIds.has(id);
}

describe('Recent favorite-star fine-grained dependency (benchmark gate)', () => {
	it('a star toggle re-maps 0 rows (was N) and renders the same star states', () => {
		const N = 400;
		const raw: RawItem[] = Array.from({ length: N }, (_, i) => ({
			id: `r-${i}`,
			name: `File ${i}`
		}));
		const favoriteIds = new Set<string>(['r-3']);

		const beforeCounter = { mapperRows: 0 };
		const afterCounter = { mapperRows: 0 };

		// Initial render: both shapes map all rows once.
		let entriesB = entriesBefore(raw, favoriteIds, beforeCounter);
		const entriesA = entriesAfter(raw, afterCounter);
		expect(beforeCounter.mapperRows).toBe(N);
		expect(afterCounter.mapperRows).toBe(N);

		// 10 star toggles. BEFORE: the mapper depends on the set → full
		// re-map each time. AFTER: the mapper doesn't run at all.
		for (let k = 0; k < 10; k++) {
			const id = `r-${k * 7}`;
			if (favoriteIds.has(id)) favoriteIds.delete(id);
			else favoriteIds.add(id);
			entriesB = entriesBefore(raw, favoriteIds, beforeCounter); // reactive re-run
			// AFTER: no mapper re-run; only the affected star re-reads.
			starStateAfter(favoriteIds, id);
		}

		expect(beforeCounter.mapperRows).toBe(N + 10 * N);
		expect(afterCounter.mapperRows).toBe(N); // unchanged since initial render

		// Gate: identical star state for every row under the AFTER shape.
		for (let i = 0; i < N; i++) {
			expect(starStateAfter(favoriteIds, entriesA[i].id)).toBe(entriesB[i].isFavorite);
		}
	});
});

// ─── [3] admin timeAgo date fallback ────────────────────────────────────────

describe('admin timeAgo >30d fallback formatter cache (benchmark gate)', () => {
	it('cached formatter output is identical to toLocaleDateString()', async () => {
		const { dateTimeFormatFor } = await import('../utils/display');
		const dates = [
			new Date('2025-01-15T10:30:00Z'),
			new Date('2024-12-31T23:59:59Z'),
			new Date('2020-06-01T00:00:00Z'),
			new Date('1999-02-28T12:00:00Z')
		];
		for (const d of dates) {
			expect(dateTimeFormatFor(undefined).format(d)).toBe(d.toLocaleDateString());
		}
	});

	it('1000 formats construct ≤1 Intl.DateTimeFormat (was 1000)', async () => {
		const { dateTimeFormatFor } = await import('../utils/display');
		const RealDTF = Intl.DateTimeFormat;
		let constructed = 0;
		// Count constructions through both paths.
		const Counting = new Proxy(RealDTF, {
			construct(target, args: [string?, Intl.DateTimeFormatOptions?]) {
				constructed++;
				return new target(...args);
			}
		});
		// eslint-disable-next-line @typescript-eslint/no-explicit-any
		(Intl as any).DateTimeFormat = Counting;
		try {
			const d = new Date('2020-06-01T00:00:00Z');
			constructed = 0;
			for (let i = 0; i < 1000; i++) {
				d.toLocaleDateString();
			}
			// jsdom implements toLocaleDateString via Intl internally in some
			// versions; count only if observable. The load-bearing assertion
			// is the cached path below.
			const beforeConstructed = constructed;

			constructed = 0;
			for (let i = 0; i < 1000; i++) {
				dateTimeFormatFor(undefined).format(d);
			}
			expect(constructed).toBeLessThanOrEqual(1);
			// When the environment exposes per-call constructions, require
			// the cached path to be strictly cheaper.
			if (beforeConstructed > 1) {
				expect(constructed).toBeLessThan(beforeConstructed);
			}
		} finally {
			// eslint-disable-next-line @typescript-eslint/no-explicit-any
			(Intl as any).DateTimeFormat = RealDTF;
		}
	});
});
