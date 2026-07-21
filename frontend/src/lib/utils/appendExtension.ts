/**
 * O(1) append detection shared by the incremental grouped-list builders
 * (`resourceSections`'s `ResourceSectionsBuilder`, `sharedLanes`'s
 * `SharedLanesBuilder`): true iff `next` is a strict prefix extension of
 * `prev` — strictly longer, and sharing prev's boundary element by identity.
 *
 * Both builders use it to choose between their O(N) incremental `extend` and a
 * full rebuild. The accumulated lists they guard are only ever mutated by
 * appending a page (infinite scroll: `raw = [...raw, ...page]`) or replaced by
 * a filtered copy that preserves element identity — so a matching boundary
 * object is a sound witness that only fresh items were appended. Any other
 * change (deletion, filter toggle, reorder) fails the boundary check and falls
 * back to a rebuild, keeping the output byte-for-byte equal to a full pass.
 */
export function isAppendExtension<T>(prev: readonly T[], next: readonly T[]): boolean {
	if (next.length <= prev.length) return false;
	// Prefix identity via the boundary object — O(1). If the element that used
	// to be last is still at that index, the prefix was untouched and next just
	// grew at the tail.
	return prev.length === 0 || next[prev.length - 1] === prev[prev.length - 1];
}
