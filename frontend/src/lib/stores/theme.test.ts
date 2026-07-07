import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, it, expect, beforeEach } from 'vitest';
import { theme, setTheme, THEME_STORAGE_KEY } from './theme.svelte';

describe('theme store', () => {
	beforeEach(() => {
		localStorage.clear();
		document.documentElement.removeAttribute('data-color-scheme');
	});
	it('sets light/dark, persists, and reflects on <html>', () => {
		setTheme('light');
		expect(theme.current).toBe('light');
		expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe('light');
		expect(document.documentElement.getAttribute('data-color-scheme')).toBe('light');
		setTheme('dark');
		expect(document.documentElement.getAttribute('data-color-scheme')).toBe('dark');
		expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe('dark');
	});
	it('auto clears storage and removes the attribute', () => {
		setTheme('dark');
		setTheme('auto');
		expect(theme.current).toBe('auto');
		expect(localStorage.getItem(THEME_STORAGE_KEY)).toBeNull();
		expect(document.documentElement.hasAttribute('data-color-scheme')).toBe(false);
	});
	it('theme.set is an alias for setTheme', () => {
		theme.set('light');
		expect(theme.current).toBe('light');
	});

	// Drift guard: `src/app.html` inlines an anti-FOUC theme reader that
	// reads the SAME localStorage key. Because that script runs before
	// any JS bundle loads, it can't `import { THEME_STORAGE_KEY }`;
	// the key is hardcoded there. This test reads the file verbatim
	// and refuses drift.
	it('app.html theme key matches THEME_STORAGE_KEY', () => {
		// Resolve against Vitest's cwd (the `frontend/` dir per its
		// invocation) — jsdom rewrites `import.meta.url` to `http://…`,
		// so file-URL conversion doesn't work in this environment.
		const appHtmlPath = resolve('src/app.html');
		const html = readFileSync(appHtmlPath, 'utf-8');
		expect(
			html.includes(`localStorage.getItem('${THEME_STORAGE_KEY}')`),
			`app.html must call localStorage.getItem('${THEME_STORAGE_KEY}') — ` +
				`update the inline script when THEME_STORAGE_KEY changes.`
		).toBe(true);
	});
});
