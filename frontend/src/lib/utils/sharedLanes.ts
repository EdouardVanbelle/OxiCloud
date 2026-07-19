/**
 * Incremental swimlane builder for the "My shares" page (`shared/+page.svelte`).
 *
 * The ROUND15-deferred follow-up to the F1 flagship (`resourceSections.ts`):
 * that fix replaced `ResourceList`'s `sections` derive; this one replaces the
 * `lanes` `$derived.by` on the shares page, which had the same O(N²/page)
 * shape. The page pages its outgoing grants in via infinite scroll
 * (`raw = [...raw, ...page.items]`), and `lanes` re-bucketed the WHOLE
 * accumulated (filtered) list on every page — and on every grant edit —
 * allocating a brand-new lane object and a brand-new `rows` array for every
 * lane each time. Σ ≈ O(N²/page) `emit` calls across an infinite-scroll drain.
 *
 * The lanes shape differs from `resourceSections` in two ways, so it gets its
 * own builder rather than reusing `ResourceSectionsBuilder` (only the O(1)
 * append test is genuinely shared — see {@link isAppendExtension}):
 *
 *  - **Fan-out.** One input item contributes 0..N rows across 0..M lanes (in
 *    the "shared with" group-by a resource's grants scatter across one lane per
 *    distinct subject), whereas a resource section maps one item to exactly one
 *    bucket with the item itself as the row.
 *  - **Header captured at first appearance.** A lane's header (a tagged union
 *    identifying the resource / subject / link kind) is fixed by the lane's
 *    first-seen member and never recomputed — unlike a section's `label`, which
 *    is recomputed every sync because it resolves async. (The shares page mirrors
 *    that: it renders the header's *label* live via `resolveLabel(...)` at render
 *    time from the stable header, so only the header identity is cached here.)
 *
 * Correctness does not depend on lane contiguity in server order. The
 * "shared with" group-by is non-monotonic — a fresh page sprays rows across
 * already-emitted subject lanes — exactly like F1's "trash by drive" case, and
 * stays byte-for-byte equal to a full rebuild (it just refreshes more lanes per
 * page). The pure {@link buildLanes} is the verbatim reference (what the old
 * `lanes` derive produced); the benchmark gate holds the incremental builder
 * deep-equal to it at every page.
 */

import { isAppendExtension } from './appendExtension';

/** One swimlane: a stable key, its first-appearance header, and its rows. */
export interface Lane<H, R> {
	key: string;
	header: H;
	rows: R[];
}

/**
 * Sink an item's {@link LaneGrouping.emit} writes its contributions to.
 * `open` ensures a lane exists (0 rows is valid — mirrors the old derive's
 * unconditional `ensure(...)` in the by-files group-by); `push` ensures the
 * lane and appends a row. The `header` is consulted only when the key is first
 * seen.
 */
export interface LaneSink<H, R> {
	open(key: string, header: H): void;
	push(key: string, header: H, row: R): void;
}

/**
 * The grouping the builder needs: a `groupKey` identity (a change forces a full
 * rebuild) and an `emit` that maps one item to its lane contributions via the
 * {@link LaneSink}. Generic over item `T`, header `H` and row `R` so the module
 * stays independent of the shares page's concrete types.
 */
export interface LaneGrouping<T, H, R> {
	/** Identity of the active grouping; a change between syncs forces a rebuild. */
	groupKey: string;
	/** Emit an item's lane contributions, in order, into `sink`. */
	emit: (item: T, sink: LaneSink<H, R>) => void;
}

/**
 * Verbatim reference: the `Lane[]` the old `lanes` `$derived.by` produced for
 * `items` under `grouping`. Lane order is first-appearance; within a lane, row
 * order is (item, then emit) order. The benchmark gate holds the incremental
 * builder equal to this at every page.
 */
export function buildLanes<T, H, R>(items: T[], grouping: LaneGrouping<T, H, R>): Lane<H, R>[] {
	const out: Lane<H, R>[] = [];
	const byKey = new Map<string, Lane<H, R>>();
	const ensure = (key: string, header: H): Lane<H, R> => {
		let lane = byKey.get(key);
		if (lane === undefined) {
			lane = { key, header, rows: [] };
			byKey.set(key, lane);
			out.push(lane);
		}
		return lane;
	};
	const sink: LaneSink<H, R> = {
		open: (key, header) => void ensure(key, header),
		push: (key, header, row) => ensure(key, header).rows.push(row)
	};
	for (const item of items) grouping.emit(item, sink);
	return out;
}

/**
 * Incremental lanes builder. Call {@link sync} with the current (already
 * kind-filtered) item list and grouping on every change; it detects the common
 * case — the list grew by appending a page under an unchanged group-by — and
 * re-emits only the fresh items, appending to the touched lanes (each of which
 * gets a fresh `rows` array so a keyed `{#each}` re-renders it) while every
 * untouched lane keeps its exact array reference. Any other change (group-by
 * switch, grant edit / deletion, kind-filter toggle, non-append) falls back to
 * a full rebuild, so the result is always deep-equal to {@link buildLanes}.
 */
export class SharedLanesBuilder<T, H, R> {
	/** Last synced list — the append cursor and the append-detection baseline. */
	#items: T[] = [];
	/** Lane keys in first-appearance order. */
	#order: string[] = [];
	/** key → the lane's first-appearance header. */
	#headers = new Map<string, H>();
	/** key → the lane's rows array (a fresh reference whenever it grows). */
	#rows = new Map<string, R[]>();
	/** The `groupKey` of the last sync; a change forces a rebuild. */
	#groupKey: string | null = null;

	#rebuild(items: T[], grouping: LaneGrouping<T, H, R>): void {
		this.#order = [];
		this.#headers = new Map();
		this.#rows = new Map();
		const ensure = (key: string, header: H): R[] => {
			let arr = this.#rows.get(key);
			if (arr === undefined) {
				arr = [];
				this.#rows.set(key, arr);
				this.#headers.set(key, header);
				this.#order.push(key);
			}
			return arr;
		};
		const sink: LaneSink<H, R> = {
			open: (key, header) => void ensure(key, header),
			push: (key, header, row) => ensure(key, header).push(row)
		};
		for (const item of items) grouping.emit(item, sink);
		this.#items = items;
	}

	#extend(items: T[], grouping: LaneGrouping<T, H, R>): void {
		const fresh = items.slice(this.#items.length);
		// Collect the fresh page's rows per touched lane, plus the keys the page
		// newly introduces (in first-appearance order). Untouched lanes are never
		// entered here, so they keep their exact existing `rows` reference.
		const freshByKey = new Map<string, R[]>();
		const newKeys: string[] = [];
		const touch = (key: string, header: H): R[] => {
			let add = freshByKey.get(key);
			if (add === undefined) {
				add = [];
				freshByKey.set(key, add);
				if (!this.#rows.has(key)) {
					newKeys.push(key);
					this.#headers.set(key, header);
				}
			}
			return add;
		};
		const sink: LaneSink<H, R> = {
			open: (key, header) => void touch(key, header),
			push: (key, header, row) => touch(key, header).push(row)
		};
		for (const item of fresh) grouping.emit(item, sink);
		for (const [k, add] of freshByKey) {
			const existing = this.#rows.get(k);
			// New lane → adopt the fresh array; grown lane → fresh concat (new
			// reference, so a keyed `{#each}` refreshes exactly the grown lanes).
			this.#rows.set(k, existing === undefined ? add : existing.concat(add));
		}
		for (const k of newKeys) this.#order.push(k);
		this.#items = items;
	}

	sync(items: T[], grouping: LaneGrouping<T, H, R>): Lane<H, R>[] {
		if (this.#groupKey === grouping.groupKey && isAppendExtension(this.#items, items)) {
			this.#extend(items, grouping);
		} else {
			this.#rebuild(items, grouping);
		}
		this.#groupKey = grouping.groupKey;
		return this.#order.map((k) => ({
			key: k,
			header: this.#headers.get(k)!,
			rows: this.#rows.get(k)!
		}));
	}
}
