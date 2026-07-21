import { describe, expect, it } from 'vitest';
import { dateTimeFormatFor, formatDate } from './display';

/**
 * Benchmark gate for the module-scope `Intl.DateTimeFormat` cache in
 * `display.ts` ({@link formatDate} / {@link dateTimeFormatFor}).
 *
 * Audit finding: `formatDate` built a fresh `Intl.DateTimeFormat` on every
 * call (`toLocaleDateString(undefined, opts)` constructs one internally), and
 * it runs ~twice per row while file lists render and scroll — a 10k-item
 * folder paid tens of thousands of ICU formatter constructions (~50–200µs
 * each) during list fill. The fix caches formatters in a Map keyed by
 * (locale, options signature).
 *
 * This gate asserts (1) the cached path is byte-identical to the
 * construct-per-call code it replaced, across dates, option shapes, and
 * locales (including an RTL one), and (2) it is decisively (≥3x) faster. If
 * the perf assertion fails, the cache is not delivering and the change
 * should be rolled back (it would be pure complexity).
 */

/** The option shapes the app actually uses (display.ts + component callsites). */
const DATE_OPTS: Intl.DateTimeFormatOptions = { year: 'numeric', month: 'short', day: 'numeric' };
const MONTH_OPTS: Intl.DateTimeFormatOptions = { year: 'numeric', month: 'long' };
const FULL_DATE_OPTS: Intl.DateTimeFormatOptions = {
	weekday: 'long',
	year: 'numeric',
	month: 'long',
	day: 'numeric'
};
const DATE_TIME_OPTS: Intl.DateTimeFormatOptions = {
	year: 'numeric',
	month: 'short',
	day: 'numeric',
	hour: '2-digit',
	minute: '2-digit'
};
const TIME_OPTS: Intl.DateTimeFormatOptions = { hour: '2-digit', minute: '2-digit' };

/**
 * The pre-fix `formatDate`, verbatim: `toLocaleDateString` constructs a new
 * `Intl.DateTimeFormat` internally on every call. This is the uncached
 * reference the cached implementation must match and beat.
 */
function referenceFormatDate(value: number | string | null | undefined): string {
	if (value === null || value === undefined) return '';
	let d: Date;
	if (typeof value === 'number') {
		// Heuristic: seconds vs milliseconds.
		d = new Date(value < 1e12 ? value * 1000 : value);
	} else {
		d = new Date(value);
	}
	if (Number.isNaN(d.getTime())) return '';
	return d.toLocaleDateString(undefined, DATE_OPTS);
}

/** ~20 inputs exercising the seconds/ms heuristic, ISO parsing, and edge cases. */
const DATE_VALUES: Array<number | string | null | undefined> = [
	0, // epoch, seconds branch
	1, // seconds
	86_399, // seconds, last second of 1970-01-01 UTC
	951_782_400, // seconds, 2000-02-29 (leap day)
	1_700_000_000, // seconds
	999_999_999_999, // just under the 1e12 cutoff → seconds branch, far future
	1_000_000_000_000, // exactly 1e12 → milliseconds branch, 2001
	1_700_000_000_000, // milliseconds
	1_766_620_800_000, // milliseconds, 2025-12-25
	Date.UTC(1999, 11, 31, 23, 59, 59), // ms, century boundary
	Date.UTC(2038, 0, 19, 3, 14, 7), // ms, past the 32-bit epoch rollover
	'2024-01-15', // date-only ISO (parsed as UTC midnight)
	'2024-02-29T12:34:56Z', // leap day, UTC
	'1999-12-31T23:59:59.999Z',
	'2020-06-15T10:00:00+05:30', // non-UTC offset
	'2031-11-05T08:15:30-05:00',
	'0001-01-01T00:00:00Z', // extreme past
	'2024-07-04T00:00:00', // no offset (local time)
	'definitely not a date', // invalid → ''
	'', // invalid → ''
	null, // → ''
	undefined // → ''
];

/** Locales the app ships (see SUPPORTED_LOCALES); 'ar' renders RTL. */
const SAMPLE_LOCALES = ['en', 'es', 'ar', 'ja'] as const;

describe('cached Intl.DateTimeFormat (benchmark gate)', () => {
	it('formatDate output is identical to the uncached reference', () => {
		for (const value of DATE_VALUES) {
			expect(formatDate(value), `formatDate(${JSON.stringify(value)})`).toBe(
				referenceFormatDate(value)
			);
		}
	});

	it('cached formatters match per-call construction across locales and option shapes', () => {
		const dates = DATE_VALUES.filter((v): v is number | string => v !== null && v !== undefined)
			.map((v) => (typeof v === 'number' ? new Date(v < 1e12 ? v * 1000 : v) : new Date(v)))
			.filter((d) => !Number.isNaN(d.getTime()));
		expect(dates.length).toBeGreaterThanOrEqual(18);

		for (const locale of SAMPLE_LOCALES) {
			for (const d of dates) {
				// Each toLocale*String call below is specified as constructing a
				// fresh Intl.DateTimeFormat — the uncached reference behaviour.
				expect(dateTimeFormatFor(locale, DATE_OPTS).format(d)).toBe(
					d.toLocaleDateString(locale, DATE_OPTS)
				);
				expect(dateTimeFormatFor(locale, MONTH_OPTS).format(d)).toBe(
					d.toLocaleDateString(locale, MONTH_OPTS)
				);
				expect(dateTimeFormatFor(locale, FULL_DATE_OPTS).format(d)).toBe(
					d.toLocaleDateString(locale, FULL_DATE_OPTS)
				);
				expect(dateTimeFormatFor(locale, DATE_TIME_OPTS).format(d)).toBe(
					d.toLocaleDateString(locale, DATE_TIME_OPTS)
				);
				expect(dateTimeFormatFor(locale, TIME_OPTS).format(d)).toBe(
					d.toLocaleTimeString(locale, TIME_OPTS)
				);
				expect(dateTimeFormatFor(undefined, DATE_OPTS).format(d)).toBe(
					d.toLocaleDateString(undefined, DATE_OPTS)
				);
			}
		}
	});

	it('reuses one instance per (locale, options) and never freezes the first locale', () => {
		// Same key → same instance (this is where the speedup comes from).
		expect(dateTimeFormatFor('es', DATE_OPTS)).toBe(dateTimeFormatFor('es', DATE_OPTS));
		expect(dateTimeFormatFor(undefined, DATE_OPTS)).toBe(dateTimeFormatFor(undefined, DATE_OPTS));
		// Different locale or options → different instance: a runtime locale
		// change must not keep formatting with the first locale seen.
		expect(dateTimeFormatFor('ar', DATE_OPTS)).not.toBe(dateTimeFormatFor('es', DATE_OPTS));
		expect(dateTimeFormatFor('es', TIME_OPTS)).not.toBe(dateTimeFormatFor('es', DATE_OPTS));
		const d = new Date(Date.UTC(2024, 4, 17, 12, 0, 0));
		expect(dateTimeFormatFor('ar', DATE_OPTS).format(d)).toBe(
			d.toLocaleDateString('ar', DATE_OPTS)
		);
		expect(dateTimeFormatFor('es', DATE_OPTS).format(d)).toBe(
			d.toLocaleDateString('es', DATE_OPTS)
		);
	});

	it(
		'formats 20k dates ≥3x faster than per-call construction (perf gate)',
		{ timeout: 30_000 },
		() => {
			const N = 20_000;
			const base = Date.UTC(2020, 0, 1);
			// Deterministic spread of distinct ms timestamps across ~30 years.
			const values = Array.from({ length: N }, (_, i) => base + i * 47_777_777);

			// Warm up both paths so JIT tiering and first-call construction sit
			// outside the measured windows. `sink` defeats dead-code elimination.
			let sink = 0;
			for (let i = 0; i < 500; i++) {
				sink += formatDate(values[i]).length;
				sink += referenceFormatDate(values[i]).length;
			}

			const t0 = performance.now();
			for (const v of values) sink += formatDate(v).length;
			const cachedMs = performance.now() - t0;

			const t1 = performance.now();
			for (const v of values) sink += referenceFormatDate(v).length;
			const uncachedMs = performance.now() - t1;

			expect(sink).toBeGreaterThan(0);
			console.info(
				`formatDate x ${N}: cached ${cachedMs.toFixed(1)} ms vs construct-per-call ${uncachedMs.toFixed(1)} ms (${(uncachedMs / cachedMs).toFixed(1)}x)`
			);
			expect(cachedMs).toBeLessThan(uncachedMs / 3);
		}
	);
});
