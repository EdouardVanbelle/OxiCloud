/**
 * Bench harness for the selection/badge-set reactivity patterns compared in
 * `selectionPatterns.bench.test.ts` (runes only compile in `.svelte.ts`
 * modules, so the models live here; the app never imports this file — it is
 * test-only and tree-shaken from the bundle).
 *
 * `copyReassignModel` is the pre-fix files-view pattern, verbatim: a
 * `$state<Set>` where every toggle copies the whole set into a fresh
 * `SvelteSet` and reassigns. `inPlaceModel` is the post-fix pattern: one
 * `SvelteSet` mutated in place.
 */
import { flushSync } from 'svelte';
import { SvelteSet } from 'svelte/reactivity';

export interface SelectionModel {
	has(id: string): boolean;
	toggle(id: string): void;
	seed(ids: Iterable<string>): void;
	readonly size: number;
}

/** Pre-fix pattern (files view `toggleSelected`, verbatim copy-and-reassign). */
export function copyReassignModel(): SelectionModel {
	// eslint-disable-next-line svelte/prefer-svelte-reactivity -- BEFORE arm replicates the pre-fix plain-Set pattern verbatim
	let selected = $state<Set<string>>(new Set());
	return {
		has: (id) => selected.has(id),
		toggle(id) {
			const next = new SvelteSet(selected);
			if (next.has(id)) next.delete(id);
			else next.add(id);
			selected = next;
		},
		seed(ids) {
			// eslint-disable-next-line svelte/prefer-svelte-reactivity -- BEFORE arm replicates the pre-fix plain-Set pattern verbatim
			selected = new Set(ids);
		},
		get size() {
			return selected.size;
		}
	};
}

/** Post-fix pattern: one live `SvelteSet` mutated in place (per-key sources
 * for present keys; absent-key reads track the version signal). */
export function inPlaceModel(): SelectionModel {
	const selected = new SvelteSet<string>();
	return {
		has: (id) => selected.has(id),
		toggle(id) {
			if (selected.has(id)) selected.delete(id);
			else selected.add(id);
		},
		seed(ids) {
			selected.clear();
			for (const id of ids) selected.add(id);
		},
		get size() {
			return selected.size;
		}
	};
}

/**
 * Mount one effect per row reading `model.has(rowId)` — the shape of a row's
 * checkbox/star binding — run `mutate`, and report how many row effects re-ran
 * (the invalidation fan-out of the mutation).
 */
export function measureFanout(model: SelectionModel, rowIds: string[], mutate: () => void): number {
	let runs = 0;
	const destroy = $effect.root(() => {
		for (const id of rowIds) {
			$effect(() => {
				void model.has(id);
				runs += 1;
			});
		}
	});
	flushSync(); // initial run of every row effect
	const baseline = runs;
	mutate();
	flushSync();
	destroy();
	return runs - baseline;
}
