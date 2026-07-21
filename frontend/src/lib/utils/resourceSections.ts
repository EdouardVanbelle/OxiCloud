/**
 * Incremental swimlane bucketing for `ResourceList`, extracted from the
 * component so the O(N²) accumulation of its `sections` `$derived` can be
 * replaced with an append-aware builder (and unit/benchmark-tested off the
 * Svelte reactive graph).
 *
 * `ResourceList` pages its list in via infinite scroll (`raw = [...raw,
 * ...page]`), and `sections` was `$derived` over the WHOLE accumulated list —
 * so paging to item N re-buckets everything loaded so far, Σ ≈ O(N²/page)
 * main-thread work during the scroll (the same class ROUND6 fixed for the
 * files listing and ROUND14 §F2 fixed for favorites, and PhotoTimeline fixed
 * for the photos grid).
 *
 * Grouped listings sort by the active group's `orderBy`, so a fresh page only
 * ever extends existing buckets or appends new ones — it never reorders an
 * already-emitted bucket. {@link ResourceSectionsBuilder} exploits that: an
 * append re-buckets only the fresh page and hands back the SAME array
 * reference for every untouched bucket (so `VirtualList`, which diffs its
 * `items` prop by reference, skips re-rendering it) while emitting a fresh
 * array for each bucket the page actually grew.
 *
 * Correctness does not depend on bucket contiguity: even a group-by whose
 * `bucketOf` is not monotonic in server order (e.g. trash grouped by drive but
 * ordered by name) stays byte-for-byte equal to the full rebuild — it just
 * touches more buckets per page. The pure {@link buildResourceSections} is the
 * verbatim reference (what the old `sections` derive produced); the benchmark
 * gate asserts the incremental builder stays deep-equal to it at every page.
 */

import { isAppendExtension } from './appendExtension';

/** One swimlane: a bucket key, its (possibly async-resolved) header label, and its rows. */
export interface ResourceSection<T> {
	key: string;
	label: string;
	rows: T[];
}

/**
 * The grouping inputs the builder needs, mirroring `ResourceList`'s active
 * `GroupByDef` plus its per-item context accessor. `bucketOf` undefined means
 * "flat list" (a single unlabelled section). Generic over the item type `T`
 * and the per-item context envelope `C` so the module stays independent of the
 * component's concrete types.
 */
export interface SectionGrouping<T, C> {
	/** Map an item + its context to a bucket key; null → the `∅` catch-all bucket. */
	bucketOf?: (item: T, ctx: C | undefined) => string | null;
	/** Map a bucket key to its header label; identity when absent. */
	labelOf?: (key: string) => string;
	/** Resolve an item's context envelope (e.g. `contextMap.get(item.id)`). */
	ctxOf: (item: T) => C | undefined;
}

/** The `∅` catch-all key the old derive used for a null bucket (kept byte-identical). */
const NULL_BUCKET = '∅';

/**
 * Verbatim reference: the `ResourceSection[]` the old `sections` `$derived.by`
 * produced for `items` under `grouping`. Bucket order is first-appearance;
 * within a bucket, server order is preserved. The benchmark gate holds the
 * incremental builder equal to this.
 */
export function buildResourceSections<T, C>(
	items: T[],
	grouping: SectionGrouping<T, C>
): ResourceSection<T>[] {
	const bucketOf = grouping.bucketOf;
	if (!bucketOf) return [{ key: '', label: '', rows: items }];
	const order: string[] = [];
	const map = new Map<string, T[]>();
	for (const item of items) {
		const k = bucketOf(item, grouping.ctxOf(item)) ?? NULL_BUCKET;
		let arr = map.get(k);
		if (arr === undefined) {
			arr = [];
			map.set(k, arr);
			order.push(k);
		}
		arr.push(item);
	}
	return order.map((k) => ({ key: k, label: grouping.labelOf?.(k) ?? k, rows: map.get(k)! }));
}

/**
 * Incremental swimlane builder. Call {@link sync} with the current (already
 * dotfile-filtered) item list and grouping on every change; it detects the
 * common case — the list grew by appending a page while the group-by is
 * unchanged — and re-buckets only the fresh items, reusing every untouched
 * bucket's array reference so `VirtualList` skips it. Any other change
 * (group-by switch, deletion, filter toggle, non-append) falls back to a full
 * rebuild, so the result is always deep-equal to {@link buildResourceSections}.
 *
 * Header labels are recomputed on every sync (never cached) because a
 * group-by's `labelOf` may resolve asynchronously — owner / sharer names
 * arrive after the rows do, and a cached label would freeze the header at its
 * fallback. Only the `rows` arrays are reference-stabilised; that is what
 * `VirtualList` diffs.
 */
export class ResourceSectionsBuilder<T, C> {
	/** Last synced list — the append cursor and the append-detection baseline. */
	#items: T[] = [];
	/** Bucket keys in first-appearance order. */
	#order: string[] = [];
	/** key → the bucket's rows array (a fresh reference whenever it grows). */
	#rows = new Map<string, T[]>();
	/** The `bucketOf` identity of the last grouped sync; a change forces a rebuild. */
	#bucketOf: SectionGrouping<T, C>['bucketOf'] = undefined;
	/** False until a grouped sync has populated the accumulation state. */
	#grouped = false;

	#rebuild(items: T[], grouping: SectionGrouping<T, C>): void {
		const bucketOf = grouping.bucketOf!;
		this.#order = [];
		this.#rows = new Map();
		for (const item of items) {
			const k = bucketOf(item, grouping.ctxOf(item)) ?? NULL_BUCKET;
			let arr = this.#rows.get(k);
			if (arr === undefined) {
				arr = [];
				this.#rows.set(k, arr);
				this.#order.push(k);
			}
			arr.push(item);
		}
		this.#items = items;
	}

	#extend(items: T[], grouping: SectionGrouping<T, C>): void {
		const bucketOf = grouping.bucketOf!;
		const fresh = items.slice(this.#items.length);
		// Collect the fresh page's items per touched bucket, preserving order and
		// first-appearance for brand-new buckets. Each touched bucket's array is
		// then rebuilt exactly once (a fresh reference so VirtualList re-renders
		// it); untouched buckets keep their existing reference untouched.
		const freshByKey = new Map<string, T[]>();
		const newKeys: string[] = [];
		for (const item of fresh) {
			const k = bucketOf(item, grouping.ctxOf(item)) ?? NULL_BUCKET;
			let arr = freshByKey.get(k);
			if (arr === undefined) {
				arr = [];
				freshByKey.set(k, arr);
				if (!this.#rows.has(k)) newKeys.push(k);
			}
			arr.push(item);
		}
		for (const [k, add] of freshByKey) {
			const existing = this.#rows.get(k);
			this.#rows.set(k, existing ? existing.concat(add) : add);
		}
		for (const k of newKeys) this.#order.push(k);
		this.#items = items;
	}

	sync(items: T[], grouping: SectionGrouping<T, C>): ResourceSection<T>[] {
		if (!grouping.bucketOf) {
			// Flat list: a single pass-through section. Reset accumulation so a
			// later switch back to a grouped view rebuilds from scratch.
			this.#grouped = false;
			this.#bucketOf = undefined;
			this.#items = items;
			return [{ key: '', label: '', rows: items }];
		}
		if (
			this.#grouped &&
			this.#bucketOf === grouping.bucketOf &&
			isAppendExtension(this.#items, items)
		) {
			this.#extend(items, grouping);
		} else {
			this.#rebuild(items, grouping);
		}
		this.#grouped = true;
		this.#bucketOf = grouping.bucketOf;
		return this.#order.map((k) => ({
			key: k,
			label: grouping.labelOf?.(k) ?? k,
			rows: this.#rows.get(k)!
		}));
	}
}
