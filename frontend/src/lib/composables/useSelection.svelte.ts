/**
 * Reactive multi-select over string ids. Backs the repeated
 * `let selected = $state(new Set()); function toggle(id) { … }` pattern used by
 * the photos grid, music picker and other list views with one source of truth.
 *
 * Mutations swap in a fresh Set so `$derived`/template reads re-run.
 */
export class Selection {
	#ids = $state<Set<string>>(new Set());

	/** The live selection set (read-only intent — mutate via the methods). */
	get ids(): Set<string> {
		return this.#ids;
	}

	get size(): number {
		return this.#ids.size;
	}

	get isEmpty(): boolean {
		return this.#ids.size === 0;
	}

	has(id: string): boolean {
		return this.#ids.has(id);
	}

	/** Selected ids as an array (e.g. for batch API calls). */
	values(): string[] {
		return [...this.#ids];
	}

	toggle(id: string): void {
		const next = new Set(this.#ids);
		if (next.has(id)) next.delete(id);
		else next.add(id);
		this.#ids = next;
	}

	add(id: string): void {
		if (this.#ids.has(id)) return;
		this.#ids = new Set(this.#ids).add(id);
	}

	delete(id: string): void {
		if (!this.#ids.has(id)) return;
		const next = new Set(this.#ids);
		next.delete(id);
		this.#ids = next;
	}

	/** Replace the whole selection. */
	set(ids: Iterable<string>): void {
		this.#ids = new Set(ids);
	}

	clear(): void {
		if (this.#ids.size) this.#ids = new Set();
	}
}

/** Create a reactive {@link Selection}. */
export function useSelection(): Selection {
	return new Selection();
}
