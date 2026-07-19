/**
 * Incremental maintenance of a grouped-listing route's per-item `contextMap`
 * (the `id → ItemContext` envelope `ResourceList` reads via `ctxOf`).
 *
 * The trash / recent / favorites / shared-with-me routes page their rows in via
 * infinite scroll (`raw = [...raw, ...page.items]`) and each derived its
 * contextMap as `new Map(raw.map((it) => [id, ctx]))` — rebuilding a brand-new
 * Map, hashing every accumulated id, on EVERY page. O(N) per page ⇒ O(N²)
 * across a drain, and a fresh Map instance each page invalidated every reader
 * (ROUND15 landed the `sections` half of this class inside `ResourceList` but
 * left the route-level projection that feeds it untouched).
 *
 * {@link primeContextPage} mirrors the shipped `favoriteIds` fix (ROUND14 §F2,
 * `SvelteSet` primed per page): the route holds ONE persistent reactive map
 * (`SvelteMap`) for the component's lifetime and, in `load()`, clears it on a
 * reset and sets only the freshly-fetched page's entries — O(page) per page,
 * O(N) across the drain, one stable instance. The map only ever needs to be a
 * superset of the currently-displayed ids: rows removed by a delete are no
 * longer rendered, so their now-stale entries are never read (identical
 * reasoning to `favoriteIds`). Every id entering `raw` comes through a
 * `load()` page, so the map always covers what is on screen.
 *
 * The param is typed `Map` (not `SvelteMap`) so the benchmark can drive the
 * exact same update logic against a plain Map, decoupled from Svelte
 * reactivity — the same way `round14.bench.test.ts` benches the `favoriteIds`
 * set. Callers pass their `SvelteMap` at runtime.
 */
export function primeContextPage<Raw, C>(
	map: Map<string, C>,
	reset: boolean,
	page: Iterable<Raw>,
	/** Map one fetched item to its `[id, ctx]` entry, or `null` to skip it (e.g. drives). */
	entry: (item: Raw) => readonly [string, C] | null
): void {
	if (reset) map.clear();
	for (const item of page) {
		const e = entry(item);
		if (e !== null) map.set(e[0], e[1]);
	}
}
