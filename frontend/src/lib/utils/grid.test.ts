import { describe, it, expect, vi } from 'vitest';

/**
 * `gridColumns` reads the phone breakpoint from ONE module-level
 * MediaQueryList (fed by its `change` listener) instead of constructing a
 * fresh `matchMedia` per call — so tests set the media state BEFORE
 * importing the module (a fresh import per state via `vi.resetModules`),
 * and flips are delivered through the captured `change` listener, exactly
 * as the browser does.
 */
type MqlListener = (e: { matches: boolean }) => void;

async function importWithMedia(matches: boolean) {
	const listeners: MqlListener[] = [];
	vi.stubGlobal(
		'matchMedia',
		vi.fn().mockReturnValue({
			matches,
			media: '',
			addEventListener: (_t: string, fn: MqlListener) => listeners.push(fn),
			removeEventListener: vi.fn()
		})
	);
	vi.resetModules();
	const mod = await import('./grid');
	return {
		gridColumns: mod.gridColumns,
		fire: (m: boolean) => listeners.forEach((l) => l({ matches: m }))
	};
}

describe('gridColumns', () => {
	it('returns 1 for non-positive width', async () => {
		const { gridColumns } = await importWithMedia(false);
		expect(gridColumns(0)).toBe(1);
		expect(gridColumns(-100)).toBe(1);
	});

	it('computes columns at desktop sizing (cardMin 200, gap 20)', async () => {
		const { gridColumns } = await importWithMedia(false);
		expect(gridColumns(220)).toBe(1); // floor(240/220)
		expect(gridColumns(440)).toBe(2); // floor(460/220)
		expect(gridColumns(900)).toBe(4); // floor(920/220)
	});

	it('uses mobile sizing when the phone media query matches', async () => {
		const { gridColumns } = await importWithMedia(true);
		expect(gridColumns(300)).toBe(2); // floor(308/148)
		expect(gridColumns(600)).toBe(4); // floor(608/148)
	});

	it('breakpoint crossings propagate through the change listener', async () => {
		const { gridColumns, fire } = await importWithMedia(false);
		expect(gridColumns(600)).toBe(2); // desktop sizing
		fire(true); // viewport crossed under 640px
		expect(gridColumns(600)).toBe(4); // mobile sizing
		fire(false);
		expect(gridColumns(600)).toBe(2);
	});
});
