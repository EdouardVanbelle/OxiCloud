/**
 * Replace a live `Set`'s contents in place. For a reactive `SvelteSet` this
 * keeps the same instance (per-key reactivity intact) instead of allocating a
 * fresh copy and invalidating every `.has()` reader at once.
 */
export function replaceSet<T>(set: Set<T>, values: Iterable<T>): void {
	set.clear();
	for (const v of values) set.add(v);
}
