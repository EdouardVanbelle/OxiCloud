/**
 * Reactive cache of owner-id → display-name, with memoised parallel resolution.
 *
 * Replaces the identical `ownerNames` record + `resolveOwners()` block in the
 * favorites and recent views. The id→name resolver is injected so the cache
 * stays decoupled from any specific API endpoint.
 */
export class OwnerCache {
	#names = $state<Record<string, string>>({});
	#resolver: (id: string) => Promise<string>;

	constructor(resolver: (id: string) => Promise<string>) {
		this.#resolver = resolver;
	}

	/** Resolved names so far (id → display name). */
	get names(): Record<string, string> {
		return this.#names;
	}

	/** Display name for an id, or `null` when unknown/empty (for cell rendering). */
	name(id: string | null | undefined): string | null {
		if (!id) return null;
		return this.#names[id] ?? null;
	}

	/** Display name for an id, falling back to the id itself (for group labels). */
	label(id: string): string {
		return this.#names[id] ?? id;
	}

	/** Resolve every not-yet-cached id in parallel; nullish ids are skipped. */
	async resolve(ids: Iterable<string | null | undefined>): Promise<void> {
		const unique = [...new Set([...ids].filter((id): id is string => !!id))];
		await Promise.all(
			unique.map(async (id) => {
				if (this.#names[id]) return;
				const name = await this.#resolver(id);
				this.#names = { ...this.#names, [id]: name };
			})
		);
	}
}

/** Create a reactive {@link OwnerCache} backed by `resolver`. */
export function useOwnerCache(resolver: (id: string) => Promise<string>): OwnerCache {
	return new OwnerCache(resolver);
}
