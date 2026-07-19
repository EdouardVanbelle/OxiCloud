import { describe, expect, it } from 'vitest';
import { SharedLanesBuilder, buildLanes, type Lane, type LaneGrouping } from './sharedLanes';

/**
 * Benchmark gate for the incremental lanes builder (SharedLanesBuilder) that
 * replaced the `lanes` `$derived.by` on the "My shares" page
 * (shared/+page.svelte).
 *
 * Audit finding (ROUND15 deferred): the shares page pages its outgoing grants
 * in via `raw = [...raw, ...page.items]`, and `lanes` re-bucketed the WHOLE
 * accumulated (filtered) list on every page — and on every grant edit —
 * allocating a fresh lane object + a fresh `rows` array for every lane each
 * time. Σ ≈ O(N²/page) `emit` calls during an infinite-scroll drain. Same
 * class as the F1 flagship (ResourceList.sections), but the lanes shape fans
 * one item out to many rows across many lanes and caches a header at first
 * appearance — see sharedLanes.ts.
 *
 * Gates (rollback rule: an AFTER that fails to beat its BEFORE fails CI):
 *  1. Equivalence — at EVERY page of the drain, the incremental output is
 *     deep-equal to the verbatim full-rebuild reference (buildLanes), for the
 *     by-files group-by (1 lane per resource, contiguous) AND the by-subject
 *     group-by (a resource's grants scatter across subject lanes, so a fresh
 *     page sprays rows into already-emitted lanes — non-contiguous).
 *  2. Reference stability — untouched lanes keep their exact `rows` array
 *     reference across a page append; a grown lane gets a fresh one.
 *  3. Fallback — group-by switch, grant edit / deletion and kind-filter toggle
 *     fall back to a correct full rebuild.
 *  4. Perf — `emit` work collapses from Σ O(N²/page) to O(N) across the drain
 *     (deterministic call count) and wall drops ≥3x.
 */

interface Grant {
	grant_id: string;
	subject_type: 'user' | 'group' | 'link';
	subject_id: string;
	has_password: boolean;
}
interface Item {
	resource: { id: string; name: string };
	grants: Grant[];
}
type Header =
	| { kind: 'resource'; item: Item }
	| { kind: 'user'; id: string }
	| { kind: 'group'; id: string }
	| { kind: 'linkPublic' }
	| { kind: 'linkPassword' };
type Row = { grant: Grant; item: Item };

const pad = (i: number) => i.toString().padStart(6, '0');

/**
 * Item `i` with 3 grants: two user grants whose subject round-robins across a
 * small pool (so by-subject buckets repeat across items → non-contiguous), and
 * one link grant (public / password alternating). Mirrors the shape the shares
 * endpoint returns.
 */
function item(i: number): Item {
	return {
		resource: { id: `res-${pad(i)}`, name: `file-${pad(i)}` },
		grants: [
			{
				grant_id: `g-${pad(i)}-0`,
				subject_type: 'user',
				subject_id: `user-${i % 8}`,
				has_password: false
			},
			{
				grant_id: `g-${pad(i)}-1`,
				subject_type: 'group',
				subject_id: `group-${i % 5}`,
				has_password: false
			},
			{
				grant_id: `g-${pad(i)}-2`,
				subject_type: 'link',
				subject_id: '',
				has_password: i % 2 === 0
			}
		]
	};
}

/** By-files group-by: one lane per resource; the old derive's unconditional `ensure`. */
function itemsGrouping(counter?: { n: number }): LaneGrouping<Item, Header, Row> {
	return {
		groupKey: 'items',
		emit: (it, sink) => {
			if (counter) counter.n++;
			const key = `resource:${it.resource.id}`;
			const header: Header = { kind: 'resource', item: it };
			sink.open(key, header);
			for (const grant of it.grants) sink.push(key, header, { grant, item: it });
		}
	};
}

/** By-subject group-by: a resource's grants scatter across one lane per subject / link kind. */
function sharedWithGrouping(counter?: { n: number }): LaneGrouping<Item, Header, Row> {
	return {
		groupKey: 'sharedWith',
		emit: (it, sink) => {
			if (counter) counter.n++;
			for (const grant of it.grants) {
				let key: string;
				let header: Header;
				if (grant.subject_type === 'user') {
					key = `user:${grant.subject_id}`;
					header = { kind: 'user', id: grant.subject_id };
				} else if (grant.subject_type === 'group') {
					key = `group:${grant.subject_id}`;
					header = { kind: 'group', id: grant.subject_id };
				} else if (grant.has_password) {
					key = 'links:password';
					header = { kind: 'linkPassword' };
				} else {
					key = 'links:public';
					header = { kind: 'linkPublic' };
				}
				sink.push(key, header, { grant, item: it });
			}
		}
	};
}

const PAGE = 50;
const PAGES = 50; // 2 500-item drain

describe('incremental shared lanes (benchmark gate)', () => {
	for (const [name, mk] of [
		['by-files (contiguous, 1 lane/resource)', itemsGrouping],
		['by-subject (non-contiguous fan-out)', sharedWithGrouping]
	] as const) {
		it(`stays deep-equal to the full rebuild at every page — ${name}`, () => {
			const all = Array.from({ length: PAGE * PAGES }, (_, i) => item(i));
			const builder = new SharedLanesBuilder<Item, Header, Row>();
			const g = mk();
			for (let p = 1; p <= PAGES; p++) {
				const cumulative = all.slice(0, p * PAGE);
				const incremental = builder.sync(cumulative, g);
				const reference = buildLanes(cumulative, g);
				expect(incremental, `page ${p}`).toEqual(reference);
			}
		});
	}

	it('keeps untouched lane arrays reference-stable and refreshes grown ones', () => {
		// A grouping that yields both stable and grown lanes on append: each item
		// contributes to a per-block lane (block = ⌊i/40⌋, so older blocks are
		// untouched by a later page) AND a single global lane (grows every page).
		const grouping: LaneGrouping<Item, Header, Row> = {
			groupKey: 'blocks',
			emit: (it, sink) => {
				const i = Number(it.resource.id.slice(4));
				const blockKey = `block:${Math.floor(i / 40)}`;
				sink.push(blockKey, { kind: 'user', id: blockKey }, { grant: it.grants[0], item: it });
				sink.push('all', { kind: 'user', id: 'all' }, { grant: it.grants[1], item: it });
			}
		};
		const all = Array.from({ length: 200 }, (_, i) => item(i));
		const builder = new SharedLanesBuilder<Item, Header, Row>();

		const first = builder.sync(all.slice(0, 120), grouping);
		const refBefore = new Map(first.map((l) => [l.key, l.rows]));

		const second = builder.sync(all.slice(0, 160), grouping);
		const refAfter = new Map(second.map((l) => [l.key, l.rows]));

		// Old blocks (0,1,2 = items 0..119) are untouched → same array reference.
		expect(refAfter.get('block:0')).toBe(refBefore.get('block:0'));
		expect(refAfter.get('block:2')).toBe(refBefore.get('block:2'));
		// The global lane grew → a fresh reference (a keyed {#each} re-renders it).
		expect(refAfter.get('all')).not.toBe(refBefore.get('all'));
		// And a brand-new block appeared for items 120..159.
		expect(refBefore.has('block:3')).toBe(false);
		expect(refAfter.has('block:3')).toBe(true);
	});

	it('falls back to a correct full rebuild on group-by switch, edit and filter toggle', () => {
		const all = Array.from({ length: 300 }, (_, i) => item(i));
		const builder = new SharedLanesBuilder<Item, Header, Row>();
		const byItems = itemsGrouping();
		const bySubject = sharedWithGrouping();

		// Drain a few pages by-files, then switch to by-subject (groupKey change → rebuild).
		builder.sync(all.slice(0, 150), byItems);
		builder.sync(all.slice(0, 300), byItems);
		expect(builder.sync(all.slice(0, 300), bySubject)).toEqual(
			buildLanes(all.slice(0, 300), bySubject)
		);

		// Grant edit under the SAME group-by: an item's grants change but the item
		// list length is unchanged → not a strict append → rebuild. Mutate a copy.
		const edited = all
			.slice(0, 300)
			.map((it, i) => (i === 10 ? { ...it, grants: it.grants.slice(0, 1) } : it));
		expect(builder.sync(edited, bySubject)).toEqual(buildLanes(edited, bySubject));

		// Deletion (list shrinks) → rebuild.
		const shrunk = edited.filter((_, i) => i % 9 !== 0);
		expect(builder.sync(shrunk, bySubject)).toEqual(buildLanes(shrunk, bySubject));

		// Kind-filter toggle: the filtered list becomes a different (reordered)
		// subset → boundary mismatch → rebuild, still equal to a full pass.
		const filtered = all.slice(0, 300).filter((_, i) => i % 3 === 0);
		expect(builder.sync(filtered, byItems)).toEqual(buildLanes(filtered, byItems));
	});

	it('collapses emit work from Σ O(N²/page) to O(N) and runs ≥3x faster', () => {
		const N = PAGE * PAGES;
		const all = Array.from({ length: N }, (_, i) => item(i));

		// Deterministic call-count gate (the hard rollback gate): the incremental
		// builder emits each item exactly once across the drain; the full rebuild
		// is quadratic (Σ_{p=1..P} p·PAGE). This is noise-free — it holds regardless
		// of machine load.
		const afterCounter = { n: 0 };
		const gAfterCount = sharedWithGrouping(afterCounter);
		const countBuilder = new SharedLanesBuilder<Item, Header, Row>();
		for (let p = 1; p <= PAGES; p++) countBuilder.sync(all.slice(0, p * PAGE), gAfterCount);
		const beforeCounter = { n: 0 };
		const gBeforeCount = sharedWithGrouping(beforeCounter);
		for (let p = 1; p <= PAGES; p++) buildLanes(all.slice(0, p * PAGE), gBeforeCount);
		expect(afterCounter.n).toBe(N);
		expect(beforeCounter.n).toBe((PAGES * (PAGES + 1) * PAGE) / 2);
		expect(afterCounter.n).toBeLessThan(beforeCounter.n / 5);

		// Wall gate — best-of-3 (min) per arm to shrug off scheduler / GC noise
		// under a saturated test runner (mirrors round14 §F1's `Math.min` pattern);
		// the tiny incremental arm is otherwise vulnerable to a single GC pause.
		// The O(N²)→O(N) collapse leaves ample headroom over the 3x floor.
		const runAfter = () => {
			const b = new SharedLanesBuilder<Item, Header, Row>();
			const g = sharedWithGrouping();
			const t = performance.now();
			for (let p = 1; p <= PAGES; p++) b.sync(all.slice(0, p * PAGE), g);
			return performance.now() - t;
		};
		const runBefore = () => {
			const g = sharedWithGrouping();
			const t = performance.now();
			for (let p = 1; p <= PAGES; p++) buildLanes(all.slice(0, p * PAGE), g);
			return performance.now() - t;
		};
		const afterMs = Math.min(runAfter(), runAfter(), runAfter());
		const beforeMs = Math.min(runBefore(), runBefore(), runBefore());

		console.info(
			`shared lanes ${PAGES}×${PAGE}: before ${beforeCounter.n} emit calls / ${beforeMs.toFixed(1)} ms — after ${afterCounter.n} calls / ${afterMs.toFixed(1)} ms (${(beforeCounter.n / afterCounter.n).toFixed(1)}x fewer calls, ${(beforeMs / afterMs).toFixed(1)}x wall)`
		);

		expect(afterMs).toBeLessThan(beforeMs / 3);
	});
});

// Keep the exported types referenced so a stray unused-import lint can't creep in.
export type { Lane };
