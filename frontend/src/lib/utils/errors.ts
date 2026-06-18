/** Error-handling helpers shared across pages and components. */
import { ui } from '$lib/stores/ui.svelte';

/** Normalise an unknown thrown value into a human-readable message. */
export function errorMessage(e: unknown): string {
	return e instanceof Error ? e.message : String(e);
}

/**
 * Raise an error toast for a caught value — the canonical catch-block handler.
 * Replaces the repeated `ui.notify(e instanceof Error ? e.message : String(e), 'error')`.
 */
export function errorToast(e: unknown): void {
	ui.notify(errorMessage(e), 'error');
}
