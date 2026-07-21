/** Display helpers shared across list views. */

/**
 * Map an `icon_class` (e.g. "fas fa-folder", "fa-file-pdf") to an icon
 * registry name (the FA token without the `fa-` prefix).
 */
export function iconNameFromClass(iconClass: string | undefined | null): string {
	if (!iconClass) return 'file';
	const token = iconClass
		.split(/\s+/)
		.find((c) => c.startsWith('fa-') && c !== 'fa-fw' && c !== 'fa-lg');
	return token ? token.slice(3) : 'file';
}

/**
 * Coarse colour bucket for a resolved icon name (see {@link iconNameFromClass}).
 * One hue per broad file family so the grid/list glyphs render in type-specific
 * colours instead of a flat monochrome. Each bucket is backed by a
 * `--file-kind-*` token (variables.css) and consumed via the `.file-icon--*`
 * modifier classes (resourceList.css).
 */
export function fileIconKind(iconName: string): string {
	switch (iconName) {
		case 'folder':
		case 'folder-open':
			return 'folder';
		case 'file-pdf':
			return 'pdf';
		case 'file-word':
			return 'doc';
		case 'file-excel':
			return 'sheet';
		case 'file-powerpoint':
			return 'slides';
		case 'file-archive':
		case 'file-zipper':
			return 'archive';
		case 'file-code':
			return 'code';
		case 'file-image':
			return 'image';
		case 'file-video':
			return 'video';
		case 'file-audio':
			return 'audio';
		case 'file-alt':
		case 'file-lines':
			return 'text';
		default:
			return 'generic';
	}
}

/** `.file-icon` colour-bucket modifier class for a resolved icon name. */
export function fileIconKindClass(iconName: string): string {
	return `file-icon--${fileIconKind(iconName)}`;
}

/**
 * Module-scope cache of `Intl.DateTimeFormat` instances, keyed by
 * `(locale, options signature)`. Constructing a formatter runs the full ICU
 * locale/pattern resolution (~50–200µs) while a `format()` call is ~1µs, and
 * {@link formatDate} runs roughly twice per row as large file lists render
 * and scroll — so a construct-per-call implementation (what
 * `toLocaleDateString(locale, options)` does under the hood) dominated list
 * fill. Entries are keyed by the locale actually requested — never frozen at
 * first use — so a runtime locale change just resolves a different entry.
 */
const dateTimeFormatCache = new Map<string, Intl.DateTimeFormat>();

// Entries built with `locale === undefined` snapshot the environment default
// locale at construction time. `toLocaleDateString(undefined, …)` re-reads the
// default on every call, so drop the cache if the default changes to keep the
// cached path behaviourally identical.
if (typeof window !== 'undefined') {
	window.addEventListener('languagechange', () => dateTimeFormatCache.clear());
}

/**
 * Cached equivalent of `new Intl.DateTimeFormat(locale, options)`.
 *
 * `date.toLocaleDateString(locale, options)` / `toLocaleTimeString(…)` are
 * specified (ECMA-402) as building exactly this formatter per call — and
 * their component defaulting is a no-op once `options` names any date/time
 * component — so `dateTimeFormatFor(locale, options).format(date)` is
 * output-identical while paying construction once per (locale, options).
 *
 * The options signature uses `JSON.stringify`, so pass options as a hoisted
 * const or an inline literal (stable key order per callsite); a differently
 * ordered but equal object would only create a redundant entry, never a wrong
 * result.
 */
export function dateTimeFormatFor(
	locale: string | undefined,
	options?: Intl.DateTimeFormatOptions
): Intl.DateTimeFormat {
	const key = `${locale ?? ''}|${options ? JSON.stringify(options) : ''}`;
	let fmt = dateTimeFormatCache.get(key);
	if (!fmt) {
		fmt = new Intl.DateTimeFormat(locale, options);
		dateTimeFormatCache.set(key, fmt);
	}
	return fmt;
}

/** Options for {@link formatDate}, hoisted so every call shares one cache key. */
const FORMAT_DATE_OPTS: Intl.DateTimeFormatOptions = {
	year: 'numeric',
	month: 'short',
	day: 'numeric'
};

/** Format a timestamp (epoch seconds/ms or ISO-8601 string) as a local date. */
export function formatDate(value: number | string | null | undefined): string {
	if (value === null || value === undefined) return '';
	let d: Date;
	if (typeof value === 'number') {
		// Heuristic: seconds vs milliseconds.
		d = new Date(value < 1e12 ? value * 1000 : value);
	} else {
		d = new Date(value);
	}
	if (Number.isNaN(d.getTime())) return '';
	return dateTimeFormatFor(undefined, FORMAT_DATE_OPTS).format(d);
}
