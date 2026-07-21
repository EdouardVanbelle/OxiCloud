import { describe, expect, it } from 'vitest';
import {
	ResourceSectionsBuilder,
	buildResourceSections,
	type SectionGrouping
} from './resourceSections';

/**
 * Benchmark gate for the incremental swimlane builder (ResourceSectionsBuilder)
 * that replaced ResourceList's `sections` `$derived.by`.
 *
 * Audit finding (ROUND14 deferred flagship): every grouped listing (trash,
 * recent, favorites, shared-with-me) pages in via `raw = [...raw, ...page]`,
 * and `sections` re-bucketed the WHOLE accumulated list on every page — Σ ≈
 * O(N²/page) `bucketOf` + `ctxOf` calls during an infinite-scroll drain, and a
 * brand-new rows array for EVERY bucket each page (so VirtualList re-diffed
 * every swimlane every page). The builder re-buckets only the fresh page and
 * hands back the same array reference for untouched buckets.
 *
 * Gates:
 *  1. Equivalence — at EVERY page of the drain, the incremental output is
 *     deep-equal to the verbatim full-rebuild reference (buildResourceSections),
 *     for a contiguous group-by (date, bucket aligned with order) AND a
 *     non-contiguous one (trash-by-drive: name-ordered, drive-bucketed); plus
 *     group-by switch, deletion and the flat pass-through fall back correctly.
 *  2. Reference stability — untouched buckets keep their exact array reference
 *     across a page append (the property VirtualList relies on to skip them),
 *     while a grown bucket gets a fresh one.
 *  3. Perf — bucketing work collapses from Σ O(N²/page) to O(N) across the
 *     drain (deterministic `bucketOf`-call count) and wall drops ≥3x.
 */

interface Item {
	id: string;
	name: string;
	driveId: string;
	/** ms epoch; descending with index (newest-first, like the server pages). */
	date: number;
}

interface Ctx {
	date: number;
	driveId: string;
}

const DAY = 86_400_000;

/** Item `i`: newest-first date, name in a fixed lexical order, round-robin drive. */
function item(i: number): Item {
	return {
		id: `it-${i.toString().padStart(6, '0')}`,
		// Zero-padded so lexical name order is a stable, well-defined sequence.
		name: `file-${i.toString().padStart(6, '0')}`,
		driveId: `drive-${i % 4}`,
		date: 1_700_000_000_000 - i * (DAY / 2)
	};
}

const contextMap = new Map<string, Ctx>();
function ctxOf(it: Item): Ctx | undefined {
	let c = contextMap.get(it.id);
	if (!c) {
		c = { date: it.date, driveId: it.driveId };
		contextMap.set(it.id, c);
	}
	return c;
}

/** Month bucket key from a ctx date (contiguous under date order). */
function monthKey(d: number): string {
	const dt = new Date(d);
	return `${dt.getUTCFullYear()}-${`${dt.getUTCMonth() + 1}`.padStart(2, '0')}`;
}

/** Contiguous group-by: date-ordered pages, date buckets. Counts bucketOf calls. */
function dateGrouping(counter?: { n: number }): SectionGrouping<Item, Ctx> {
	return {
		bucketOf: (_it, ctx) => {
			if (counter) counter.n++;
			return ctx ? monthKey(ctx.date) : null;
		},
		labelOf: (k) => `📅 ${k}`,
		ctxOf
	};
}

/**
 * Non-contiguous group-by mirroring trash "by drive": pages arrive in NAME
 * order but bucket by driveId, so a fresh page sprays items across every
 * already-emitted drive bucket. Equivalence must still hold.
 */
function driveGrouping(counter?: { n: number }): SectionGrouping<Item, Ctx> {
	return {
		bucketOf: (_it, ctx) => {
			if (counter) counter.n++;
			return ctx ? ctx.driveId : null;
		},
		labelOf: (k) => `💾 ${k}`,
		ctxOf
	};
}

const PAGE = 50;
const PAGES = 50; // 2 500-item drain

describe('incremental resource sections (benchmark gate)', () => {
	for (const [name, mk] of [
		['contiguous date buckets', dateGrouping],
		['non-contiguous drive buckets', driveGrouping]
	] as const) {
		it(`stays deep-equal to the full rebuild at every page — ${name}`, () => {
			const all = Array.from({ length: PAGE * PAGES }, (_, i) => item(i));
			const builder = new ResourceSectionsBuilder<Item, Ctx>();
			// ONE stable grouping across the drain — mirrors the component, where
			// `activeGroup.bucketOf` is a fixed closure from the page's once-defined
			// `groupBys`. This is what lets the builder take its incremental path,
			// so this loop genuinely exercises it (not the rebuild fallback).
			const g = mk();
			for (let p = 1; p <= PAGES; p++) {
				const cumulative = all.slice(0, p * PAGE);
				const incremental = builder.sync(cumulative, g);
				const reference = buildResourceSections(cumulative, g);
				expect(incremental, `page ${p}`).toEqual(reference);
			}
		});
	}

	it('keeps untouched bucket arrays reference-stable and refreshes grown ones', () => {
		const all = Array.from({ length: 600 }, (_, i) => item(i));
		const builder = new ResourceSectionsBuilder<Item, Ctx>();
		const g = dateGrouping();

		const first = builder.sync(all.slice(0, 300), g);
		const refBefore = new Map(first.map((s) => [s.key, s.rows]));

		const second = builder.sync(all.slice(0, 350), g);
		let stable = 0;
		let refreshed = 0;
		for (const s of second) {
			const prev = refBefore.get(s.key);
			if (prev === undefined) continue; // brand-new bucket
			if (prev === s.rows) stable++;
			else refreshed++;
		}
		// Date-ordered append only grows the boundary bucket(s): most earlier
		// buckets must be handed back by the SAME reference (VirtualList skips
		// them), and at least one bucket must be refreshed (it grew).
		expect(stable).toBeGreaterThan(0);
		expect(refreshed).toBeGreaterThan(0);
		expect(stable).toBeGreaterThan(refreshed);
	});

	it('falls back to a correct full rebuild on group-by switch, deletion and flat', () => {
		const all = Array.from({ length: 600 }, (_, i) => item(i));
		const builder = new ResourceSectionsBuilder<Item, Ctx>();
		const byDate = dateGrouping();
		const byDrive = driveGrouping();

		// Drain a few pages under date grouping, then switch to drive grouping
		// (a different bucketOf reference → rebuild).
		builder.sync(all.slice(0, 300), byDate);
		expect(builder.sync(all.slice(0, 300), byDrive)).toEqual(
			buildResourceSections(all.slice(0, 300), byDrive)
		);

		// Deletion under the SAME grouping (list shrinks / prefix changes) →
		// rebuild via the append check, not a grouping-ref change.
		const shrunk = all.slice(0, 300).filter((_, i) => i % 7 !== 0);
		expect(builder.sync(shrunk, byDrive)).toEqual(buildResourceSections(shrunk, byDrive));

		// Flat pass-through (no bucketOf) yields one section and doesn't wedge the
		// next grouped sync.
		const flat: SectionGrouping<Item, Ctx> = { ctxOf };
		const flatOut = builder.sync(shrunk, flat);
		expect(flatOut).toEqual([{ key: '', label: '', rows: shrunk }]);
		expect(flatOut[0].rows).toBe(shrunk); // pass-through, no copy
		expect(builder.sync(shrunk, byDate)).toEqual(buildResourceSections(shrunk, byDate));
	});

	it('collapses bucketing work from Σ O(N²/page) to O(N) and runs ≥3x faster', () => {
		const N = PAGE * PAGES;
		const all = Array.from({ length: N }, (_, i) => item(i));

		// AFTER: incremental — each item is bucketed exactly once across the drain.
		// ONE stable grouping (fixed bucketOf), exactly as the component supplies.
		const afterCounter = { n: 0 };
		const gAfter = dateGrouping(afterCounter);
		const builder = new ResourceSectionsBuilder<Item, Ctx>();
		const t1 = performance.now();
		for (let p = 1; p <= PAGES; p++) builder.sync(all.slice(0, p * PAGE), gAfter);
		const afterMs = performance.now() - t1;

		// BEFORE: full rebuild per page — re-buckets the whole cumulative list.
		const beforeCounter = { n: 0 };
		const gBefore = dateGrouping(beforeCounter);
		const t0 = performance.now();
		for (let p = 1; p <= PAGES; p++) buildResourceSections(all.slice(0, p * PAGE), gBefore);
		const beforeMs = performance.now() - t0;

		console.info(
			`resource sections ${PAGES}×${PAGE}: before ${beforeCounter.n} bucketOf calls / ${beforeMs.toFixed(1)} ms — after ${afterCounter.n} calls / ${afterMs.toFixed(1)} ms (${(beforeCounter.n / afterCounter.n).toFixed(1)}x fewer calls, ${(beforeMs / afterMs).toFixed(1)}x wall)`
		);

		// Incremental buckets each item once: exactly N calls.
		expect(afterCounter.n).toBe(N);
		// Full rebuild is quadratic: Σ_{p=1..P} p·PAGE.
		expect(beforeCounter.n).toBe((PAGES * (PAGES + 1) * PAGE) / 2);
		expect(afterCounter.n).toBeLessThan(beforeCounter.n / 5);
		expect(afterMs).toBeLessThan(beforeMs / 3);
	});
});
