/**
 * Incremental `id → position` index for `ResourceList`, extracted so the O(N²)
 * accumulation of its `itemIndexById` `$derived` (and the reap-stale effect's
 * per-page `new Set(items.map(…))`) can be replaced with an append-aware
 * builder — and unit/benchmark-tested off the Svelte reactive graph.
 *
 * `ResourceList` pages its list in via infinite scroll (`items = [...items,
 * ...page]`) and rebuilt `new Map(items.map((i, idx) => [i.id, idx]))` on every
 * page — O(N) per page, Σ ≈ O(N²) across a P-page drain, and a fresh Map each
 * page (so the reap-stale effect that reference-diffs it re-ran on every append
 * too, allocating another O(N) id Set for a reap that an append can never
 * trigger). This is the same class ROUND6 fixed for the files listing, ROUND14
 * §F2 for favorites, and ROUND15/16 for the grouped/shared lanes.
 *
 * Because a fresh page only ever *appends* (server order is stable; existing
 * rows keep their index), {@link ItemIndexBuilder} extends the persistent Map
 * with just the new tail on an append and returns the SAME Map reference; any
 * other change (reload, deletion, non-append) rebuilds into a NEW Map. That
 * reference contract is load-bearing for the two `ResourceList` consumers:
 *
 *   - `selectedItems` re-derives on every `items` change regardless (it indexes
 *     `items[idx]`), so it always reads the freshly-extended Map — a stable ref
 *     on append costs it nothing.
 *   - the reap-stale `$effect` reference-diffs the Map, so a stable ref on
 *     append means it does NOT re-run there (an append never removes an id, so
 *     there is nothing to reap), while a rebuild (delete / reload) yields a new
 *     ref and DOES re-run it — exactly when stale selections must be dropped.
 *
 * The pure {@link buildItemIndex} is the verbatim reference (what the old
 * `itemIndexById` derive produced); the benchmark gate holds the builder equal
 * to it at every page.
 */

import { isAppendExtension } from './appendExtension';

/** Minimal shape the index needs: a stable string `id`. */
export interface HasId {
	id: string;
}

/**
 * Verbatim reference: the `Map<id, index>` the old `itemIndexById` `$derived`
 * produced — `new Map(items.map((i, idx) => [i.id, idx]))`. On a duplicate id
 * the highest index wins (last insertion), matching `Map`'s own semantics.
 */
export function buildItemIndex<T extends HasId>(items: readonly T[]): Map<string, number> {
	const index = new Map<string, number>();
	for (let i = 0; i < items.length; i++) index.set(items[i].id, i);
	return index;
}

/**
 * Append-aware `id → index` builder. Call {@link sync} with the current item
 * list on every change; it detects the common case — the list grew by appending
 * a page — and indexes only the fresh tail, reusing the persistent Map (same
 * reference). Any other change rebuilds into a new Map, so the result is always
 * deep-equal to {@link buildItemIndex} and the reference changes exactly when a
 * reap-stale pass is warranted.
 */
export class ItemIndexBuilder<T extends HasId> {
	/** Last synced list — the append cursor and the append-detection baseline. */
	#items: readonly T[] = [];
	/** id → index; a stable reference across appends, a fresh one on rebuild. */
	#index = new Map<string, number>();

	sync(items: readonly T[]): Map<string, number> {
		if (isAppendExtension(this.#items, items)) {
			// Append: the prefix is unchanged (existing ids keep their index), so
			// only the fresh tail needs indexing. A duplicate id in the tail
			// overwrites to its higher index — identical to the full rebuild's
			// last-wins. Same Map reference is returned (see the class doc).
			for (let i = this.#items.length; i < items.length; i++) {
				this.#index.set(items[i].id, i);
			}
		} else {
			// Reload / deletion / non-append / first run: rebuild into a NEW Map so
			// the reap-stale effect (which reference-diffs it) re-runs.
			this.#index = buildItemIndex(items);
		}
		this.#items = items;
		return this.#index;
	}
}
