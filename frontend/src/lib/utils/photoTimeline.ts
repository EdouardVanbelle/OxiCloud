/**
 * Photo-timeline grouping + row layout, extracted from the photos view so the
 * O(N²) accumulation of its `groups`/`photoRows` derives can be replaced with
 * an incremental builder (and unit/benchmark-tested off the Svelte reactive
 * graph).
 *
 * Photos arrive newest-first (`media_sort_date DESC`), so each fetched page
 * only ever extends the last date bucket or appends new buckets after it —
 * never mutates an earlier group. {@link PhotoTimeline} exploits that: an
 * append re-buckets only the new page and recomputes rows only for the groups
 * that actually changed, keeping a full scroll O(N) instead of O(N²).
 *
 * The pure {@link buildPhotoRows} is the verbatim reference (what the old
 * `groups`→`photoRows` derive chain produced); the benchmark gate asserts the
 * incremental builder stays byte-for-byte equal to it.
 */
import type { PhotoItem } from '$lib/api/endpoints/photos';

export type GroupMode = 'day' | 'month' | 'year';
export type LayoutMode = 'square' | 'justified';

export interface JustifiedTile {
	file: PhotoItem;
	w: number;
	h: number;
}

export type PhotoRow =
	| { kind: 'header'; key: string; height: number; label: string; count: number }
	| { kind: 'tiles'; key: string; height: number; gap: number; tiles: JustifiedTile[] };

/** Layout constants — mirror the photos view's original values exactly. */
export const SQUARE_GAP = 4; // .25rem, matches the old grid gap
export const SQUARE_MIN = 144; // 9rem minmax floor
export const JUSTIFIED_GAP = 8; // .photos-jrow margin-bottom
export const HEADER_H = 44;

export interface TimelineConfig {
	groupMode: GroupMode;
	layoutMode: LayoutMode;
	/** Usable content width of the grid, in px. */
	width: number;
	/** `(max-width: 768px)` — selects the 150px vs 200px justified target. */
	mobile: boolean;
	/** EXIF-aware capture timestamp (ms). Injected so the module stays pure. */
	timestampOf: (p: PhotoItem) => number;
	/** Locale-aware bucket label for a group's representative date. */
	labelOf: (d: Date, mode: GroupMode) => string;
}

interface Group {
	key: string;
	label: string;
	photos: PhotoItem[];
}

/** Year/month/day bucket key for a date under `groupMode` (verbatim). */
export function bucketKey(d: Date, groupMode: GroupMode): string {
	const y = d.getFullYear();
	if (groupMode === 'year') return `${y}`;
	const m = `${d.getMonth() + 1}`.padStart(2, '0');
	if (groupMode === 'month') return `${y}-${m}`;
	return `${y}-${m}-${`${d.getDate()}`.padStart(2, '0')}`;
}

/**
 * Pack files into justified rows (Flickr-style): each full row is scaled to
 * fill `width` while preserving every tile's aspect ratio. Missing dimensions
 * fall back to 1:1. Verbatim port of the photos view's `justifiedRows`, with
 * the `matchMedia` read hoisted to the `mobile` flag so it's testable.
 */
export function justifiedRows(
	files: PhotoItem[],
	width: number,
	mobile: boolean
): Array<{ height: number; tiles: JustifiedTile[] }> {
	const gap = 8;
	const target = mobile ? 150 : 200;
	const rows: Array<{ height: number; tiles: JustifiedTile[] }> = [];
	let cur: Array<{ file: PhotoItem; aspect: number }> = [];
	let aspectSum = 0;
	for (const file of files) {
		let aspect = file.width && file.height ? file.width / file.height : 1;
		if (!Number.isFinite(aspect) || aspect <= 0) aspect = 1;
		aspect = Math.min(Math.max(aspect, 0.4), 3);
		cur.push({ file, aspect });
		aspectSum += aspect;
		const rowWidth = aspectSum * target + (cur.length - 1) * gap;
		if (rowWidth >= width) {
			const h = (width - (cur.length - 1) * gap) / aspectSum;
			rows.push({
				height: Math.round(h),
				tiles: cur.map((tt) => ({
					file: tt.file,
					w: Math.max(1, Math.round(tt.aspect * h)),
					h: Math.round(h)
				}))
			});
			cur = [];
			aspectSum = 0;
		}
	}
	if (cur.length) {
		rows.push({
			height: target,
			tiles: cur.map((tt) => ({
				file: tt.file,
				w: Math.max(1, Math.round(tt.aspect * target)),
				h: target
			}))
		});
	}
	return rows;
}

/** Columns + cell size for the square layout at width `W` (verbatim). */
function squareGeometry(W: number): { cols: number; cell: number } {
	const cols = Math.max(1, Math.floor((W + SQUARE_GAP) / (SQUARE_MIN + SQUARE_GAP)));
	const cell = (W - (cols - 1) * SQUARE_GAP) / cols;
	return { cols, cell };
}

/** Flatten one group into its header + tile rows (verbatim per-group body). */
function groupToRows(g: Group, cfg: TimelineConfig, cols: number, cell: number): PhotoRow[] {
	const rows: PhotoRow[] = [
		{ kind: 'header', key: `h:${g.key}`, height: HEADER_H, label: g.label, count: g.photos.length }
	];
	if (cfg.layoutMode === 'justified') {
		const jrows = justifiedRows(g.photos, cfg.width, cfg.mobile);
		for (let ri = 0; ri < jrows.length; ri++) {
			rows.push({
				kind: 'tiles',
				key: `${g.key}:j${ri}`,
				height: jrows[ri].height + JUSTIFIED_GAP,
				gap: JUSTIFIED_GAP,
				tiles: jrows[ri].tiles
			});
		}
	} else {
		for (let i = 0; i < g.photos.length; i += cols) {
			const tiles = g.photos.slice(i, i + cols).map((file) => ({ file, w: cell, h: cell }));
			rows.push({
				kind: 'tiles',
				key: `${g.key}:s${i}`,
				height: cell + SQUARE_GAP,
				gap: SQUARE_GAP,
				tiles
			});
		}
	}
	return rows;
}

/** Bucket `items` into date groups, first-appearance order (verbatim). */
function buildGroups(items: PhotoItem[], cfg: TimelineConfig): Group[] {
	const out: Group[] = [];
	const index = new Map<string, number>();
	for (const p of items) {
		const d = new Date(cfg.timestampOf(p));
		const key = bucketKey(d, cfg.groupMode);
		let i = index.get(key);
		if (i === undefined) {
			i = out.length;
			index.set(key, i);
			out.push({ key, label: cfg.labelOf(d, cfg.groupMode), photos: [] });
		}
		out[i].photos.push(p);
	}
	return out;
}

/**
 * Verbatim reference: the flat `PhotoRow[]` the old `groups`→`photoRows`
 * derive chain produced for `items` under `cfg`. Returns `[]` for a
 * non-positive width, matching the old guard. The benchmark gate holds the
 * incremental builder equal to this.
 */
export function buildPhotoRows(items: PhotoItem[], cfg: TimelineConfig): PhotoRow[] {
	if (cfg.width <= 0) return [];
	const { cols, cell } = squareGeometry(cfg.width);
	const rows: PhotoRow[] = [];
	for (const g of buildGroups(items, cfg)) {
		rows.push(...groupToRows(g, cfg, cols, cell));
	}
	return rows;
}

function configEq(a: TimelineConfig, b: TimelineConfig): boolean {
	return (
		a.groupMode === b.groupMode &&
		a.layoutMode === b.layoutMode &&
		a.width === b.width &&
		a.mobile === b.mobile &&
		a.timestampOf === b.timestampOf &&
		a.labelOf === b.labelOf
	);
}

/**
 * Incremental photo-timeline builder. Call {@link sync} with the current item
 * list and config on every change; it detects the common case — the list grew
 * by appending a page while config is unchanged — and re-buckets only the new
 * items + re-lays-out only the groups that changed, reusing every untouched
 * group's cached rows. Any other change (config, deletion, filter toggle,
 * non-append) falls back to a full rebuild, so the result is always identical
 * to {@link buildPhotoRows}.
 */
export class PhotoTimeline {
	#cfg: TimelineConfig | null = null;
	#groups: Group[] = [];
	/** Items already bucketed — the append cursor into the last synced list. */
	#groupedItems: PhotoItem[] = [];
	/** group.key → its cached rows for the current config. */
	#rowCache = new Map<string, PhotoRow[]>();
	#geom = { cols: 1, cell: 0 };

	/** Whether `next` extends `prev` (same prefix objects + strictly longer). */
	#isAppend(prev: PhotoItem[], next: PhotoItem[]): boolean {
		if (next.length <= prev.length) return false;
		// Prefix identity via the boundary object — O(1), the list is only ever
		// mutated by appending or by replacing with a filtered copy.
		return prev.length === 0 || next[prev.length - 1] === prev[prev.length - 1];
	}

	#rebuild(items: PhotoItem[], cfg: TimelineConfig): void {
		this.#cfg = cfg;
		this.#groups = cfg.width > 0 ? buildGroups(items, cfg) : [];
		this.#groupedItems = items;
		this.#rowCache.clear();
		this.#geom = squareGeometry(cfg.width);
	}

	#extend(items: PhotoItem[], cfg: TimelineConfig): void {
		const fresh = items.slice(this.#groupedItems.length);
		// The last existing group may grow, so its cached rows are stale.
		if (this.#groups.length > 0) {
			this.#rowCache.delete(this.#groups[this.#groups.length - 1].key);
		}
		for (const p of fresh) {
			const d = new Date(cfg.timestampOf(p));
			const key = bucketKey(d, cfg.groupMode);
			const last = this.#groups[this.#groups.length - 1];
			if (last && last.key === key) {
				last.photos.push(p);
			} else {
				this.#groups.push({ key, label: cfg.labelOf(d, cfg.groupMode), photos: [p] });
			}
		}
		this.#groupedItems = items;
	}

	sync(items: PhotoItem[], cfg: TimelineConfig): PhotoRow[] {
		if (cfg.width <= 0) {
			// Keep the item cursor so a later positive width rebuilds from scratch.
			this.#cfg = cfg;
			this.#groups = [];
			this.#groupedItems = items;
			this.#rowCache.clear();
			return [];
		}
		if (this.#cfg && configEq(this.#cfg, cfg) && this.#isAppend(this.#groupedItems, items)) {
			this.#extend(items, cfg);
		} else {
			this.#rebuild(items, cfg);
		}

		const { cols, cell } = this.#geom;
		const out: PhotoRow[] = [];
		for (const g of this.#groups) {
			let rows = this.#rowCache.get(g.key);
			if (rows === undefined) {
				rows = groupToRows(g, cfg, cols, cell);
				this.#rowCache.set(g.key, rows);
			}
			for (const r of rows) out.push(r);
		}
		return out;
	}
}
