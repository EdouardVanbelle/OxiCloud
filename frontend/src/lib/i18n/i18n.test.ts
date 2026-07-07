import { describe, expect, it, vi, beforeEach } from 'vitest';
import {
	getNestedValue,
	interpolate,
	resolveBrowserLocale,
	initI18n,
	t,
	i18n
} from './index.svelte';

describe('resolveBrowserLocale', () => {
	it('matches an exact full tag', () => {
		expect(resolveBrowserLocale(['zh-TW'])).toBe('zh-TW');
		expect(resolveBrowserLocale(['fr-FR', 'fr'])).toBe('fr');
	});

	it('maps Traditional Chinese variants to zh-TW', () => {
		expect(resolveBrowserLocale(['zh-Hant'])).toBe('zh-TW');
		expect(resolveBrowserLocale(['zh-HK'])).toBe('zh-TW');
		expect(resolveBrowserLocale(['zh-MO'])).toBe('zh-TW');
	});

	it('maps Simplified/other Chinese to zh', () => {
		expect(resolveBrowserLocale(['zh-CN'])).toBe('zh');
		expect(resolveBrowserLocale(['zh'])).toBe('zh');
	});

	it('falls back to the primary subtag', () => {
		expect(resolveBrowserLocale(['de-AT'])).toBe('de');
	});

	it('defaults to en when nothing matches', () => {
		expect(resolveBrowserLocale(['xx-YY'])).toBe('en');
	});
});

describe('getNestedValue', () => {
	const dict = {
		'flat.key': 'flat value',
		nav: { files: 'Files', shared: 'Shared' },
		button: { save_changes: 'Save changes' }
	};

	it('resolves a direct key that contains dots', () => {
		expect(getNestedValue(dict, 'flat.key')).toBe('flat value');
	});

	it('resolves dotted nested paths', () => {
		expect(getNestedValue(dict, 'nav.files')).toBe('Files');
	});

	it('returns null for missing keys', () => {
		expect(getNestedValue(dict, 'nav.missing')).toBeNull();
		expect(getNestedValue(undefined, 'nav.files')).toBeNull();
	});

	it('applies the prefix_suffix underscore fallback', () => {
		expect(getNestedValue(dict, 'button_save_changes')).toBe('Save changes');
	});
});

describe('interpolate', () => {
	it('replaces {{param}} placeholders', () => {
		expect(interpolate('Hello {{name}}', { name: 'Ada' })).toBe('Hello Ada');
	});

	it('trims whitespace inside placeholders', () => {
		expect(interpolate('Send to {{ email }}', { email: 'a@b.c' })).toBe('Send to a@b.c');
	});

	it('leaves unknown placeholders intact', () => {
		expect(interpolate('Hi {{name}}', {})).toBe('Hi {{name}}');
	});

	it('coerces non-string params', () => {
		expect(interpolate('{{count}} items', { count: 5 })).toBe('5 items');
	});
});

describe('initI18n — lazy English fallback', () => {
	let resolveEn: () => void;

	beforeEach(() => {
		localStorage.setItem('oxi-locale', 'es');
		resolveEn = () => {};
		globalThis.fetch = vi.fn((input: RequestInfo | URL) => {
			const url = String(input);
			if (url.includes('/es.json')) {
				return Promise.resolve(new Response(JSON.stringify({ greeting: 'Hola' }), { status: 200 }));
			}
			if (url.includes('/en.json')) {
				// Deferred: only resolves when the test flips it, proving init didn't wait.
				return new Promise<Response>((res) => {
					resolveEn = () =>
						res(new Response(JSON.stringify({ only_en: 'English only' }), { status: 200 }));
				});
			}
			return Promise.resolve(new Response('{}', { status: 404 }));
		}) as unknown as typeof fetch;
	});

	it('is ready after only the active locale and warms en in the background', async () => {
		// Resolves even though the en fetch is still pending — it isn't awaited.
		await initI18n();
		expect(i18n.loaded).toBe(true);
		expect(i18n.locale).toBe('es');
		expect(t('greeting')).toBe('Hola');

		const urls = vi.mocked(globalThis.fetch).mock.calls.map((c) => String(c[0]));
		expect(urls.some((u) => u.includes('/es.json'))).toBe(true);
		expect(urls.some((u) => u.includes('/en.json'))).toBe(true); // en was kicked off

		// A key missing from es is unresolved until en arrives, then falls back.
		expect(t('only_en')).toBe('only_en');
		resolveEn();
		await new Promise((r) => setTimeout(r, 0));
		expect(t('only_en')).toBe('English only');
	});
});
