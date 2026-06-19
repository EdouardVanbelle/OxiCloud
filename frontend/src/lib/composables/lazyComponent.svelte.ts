/**
 * Defer loading a heavy Svelte component until it is first needed.
 *
 * The dynamic `import()` puts the component in its own chunk, keeping it out of
 * the initial bundle. The component type is inferred from the module's default
 * export, so binding and prop typing at the call site stay fully checked. Call
 * `load()` right before the component is shown, then render it once `component`
 * is non-null:
 *
 * ```svelte
 * const viewer = lazyComponent(() => import('$lib/components/FileViewer.svelte'));
 * $effect(() => { if (open) void viewer.load(); });
 * …
 * {#if viewer.component}
 *   {@const Viewer = viewer.component}
 *   <Viewer bind:open {file} />
 * {/if}
 * ```
 */
export function lazyComponent<C>(loader: () => Promise<{ default: C }>) {
	let component = $state<C | null>(null);
	let pending: Promise<void> | null = null;

	return {
		get component() {
			return component;
		},
		/** Idempotent: kicks off the import once, resolves when the chunk is ready. */
		load(): Promise<void> {
			if (component) return Promise.resolve();
			if (!pending) {
				pending = loader().then((mod) => {
					component = mod.default;
				});
			}
			return pending;
		}
	};
}
