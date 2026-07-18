import { describe, expect, it, vi, beforeEach } from 'vitest';

vi.mock('$lib/api/client', () => ({ apiFetch: vi.fn(), apiJson: vi.fn() }));

import { apiFetch } from '$lib/api/client';
import type { FileItem, FolderItem, ItemType } from '$lib/api/types';
import { fetchFolderListing, invalidateFolderCache, type FolderListing } from './folders';

/**
 * Benchmark gate for the coalesced progressive-render emissions in
 * {@link fetchFolderListing}.
 *
 * Audit finding: the loader invoked `onPage` after EVERY 200-item page with a
 * fresh copy of the whole accumulated listing, and the files view re-derives
 * its filtered + sorted view (two `localeCompare` sorts + entry rebuild) from
 * each emission. For a folder of N items that is Σ page sizes ≈ O(N²/200)
 * elements re-sorted on the main thread during a single load — hundreds of ms
 * of jank on exactly the large folders progressive rendering was meant to
 * help. The fix emits page one (first paint) and the final page always, and
 * intermediate pages at most once per PAGE_EMIT_MIN_INTERVAL_MS.
 *
 * Gates:
 *  1. Equivalence — final listing identical to the emit-every-page reference,
 *     first emission still after page one (first paint preserved), last
 *     emission still `done === true` with the complete listing.
 *  2. Perf — on a fast connection (pages resolve in ≪150 ms) the consumer-side
 *     derive work collapses from 25 full re-sorts to ≤3; wall time of the
 *     load+derive cycle must drop accordingly (≥3x on the derive term).
 */

type ResourceItem = { resource_type: ItemType; resource: { id: string; name: string } };
type ResourcePage = { items?: ResourceItem[]; next_cursor?: string };

const PAGE_SIZE = 200;
const PAGES = 25; // 5 000-item folder

/** Deterministic shuffled names so the consumer sort actually works. */
function pageBody(page: number): ResourcePage {
	const items: ResourceItem[] = [];
	for (let i = 0; i < PAGE_SIZE; i++) {
		const n = page * PAGE_SIZE + i;
		const id = `f-${n.toString().padStart(5, '0')}`;
		// Mix folders into the first page like a real listing (folders first).
		const isFolder = page === 0 && i < 20;
		items.push({
			resource_type: isFolder ? 'folder' : 'file',
			resource: { id, name: `item ${((n * 7919) % 100000).toString().padStart(5, '0')}.txt` }
		});
	}
	return { items, next_cursor: page + 1 < PAGES ? `c${page + 1}` : undefined };
}

function fakeRes(body: ResourcePage): Response {
	return {
		status: 200,
		ok: true,
		json: async () => body,
		headers: { get: () => null }
	} as unknown as Response;
}

function mockPagedFetch(): void {
	let call = 0;
	vi.mocked(apiFetch).mockImplementation(async () => fakeRes(pageBody(call++)));
}

/**
 * The pre-fix loader, verbatim shape: accumulate pages and emit a fresh copy
 * of the whole accumulated listing after every page.
 */
async function referenceFetchFolderListing(
	folderId: string,
	onPage: (partial: FolderListing, done: boolean) => void
): Promise<FolderListing> {
	const folders: FolderItem[] = [];
	const files: FileItem[] = [];
	let cursor: string | undefined;
	do {
		const params = new URLSearchParams({ order_by: 'name', limit: '200' });
		if (cursor) params.set('cursor', cursor);
		const res = await apiFetch(`/api/folders/${folderId}/resources?${params.toString()}`, {
			credentials: 'same-origin',
			cache: 'no-store'
		});
		if (!res.ok) throw new Error(`listing failed: ${res.status}`);
		const page = (await res.json()) as ResourcePage;
		for (const it of page.items ?? []) {
			if (it.resource_type === 'folder') folders.push(it.resource as FolderItem);
			else files.push(it.resource as FileItem);
		}
		cursor = page.next_cursor;
		onPage({ folders: [...folders], files: [...files], favoriteIds: [], sharedIds: [] }, !cursor);
	} while (cursor);
	return { folders, files, favoriteIds: [], sharedIds: [] };
}

/**
 * The files view's per-emission derive chain, reduced to its dominant costs:
 * dotfile filter pass + two localeCompare sorts + ordered-entry rebuild
 * (`sortedFolders`/`sortedFiles`/`entries`/`orderedIds` in +page.svelte).
 * Returns the number of elements that went through the sort — the O(N²) term.
 */
function consumerDerive(partial: FolderListing): number {
	const visF = partial.folders.filter((f) => !f.name.startsWith('.'));
	const visX = partial.files.filter((f) => !f.name.startsWith('.'));
	const sortedF = [...visF].sort((a, b) => a.name.localeCompare(b.name));
	const sortedX = [...visX].sort((a, b) => a.name.localeCompare(b.name));
	const orderedIds = [...sortedF.map((f) => f.id), ...sortedX.map((f) => f.id)];
	return orderedIds.length;
}

beforeEach(() => {
	vi.clearAllMocks();
	invalidateFolderCache();
});

describe('coalesced progressive listing emissions (benchmark gate)', () => {
	it('final listing, first-paint page and done-flag match the emit-every-page reference', async () => {
		mockPagedFetch();
		const refEmits: Array<{ n: number; done: boolean }> = [];
		const refFinal = await referenceFetchFolderListing('bench', (p, done) =>
			refEmits.push({ n: p.folders.length + p.files.length, done })
		);

		mockPagedFetch();
		const emits: Array<{ n: number; done: boolean; partial: FolderListing }> = [];
		const r = await fetchFolderListing('bench', {
			onPage: (partial, done) =>
				emits.push({ n: partial.folders.length + partial.files.length, done, partial })
		});

		// Identical complete listing.
		expect(r.listing).toEqual(refFinal);
		// First paint unchanged: the first emission is still page one.
		expect(emits[0].n).toBe(refEmits[0].n);
		expect(emits[0].n).toBe(PAGE_SIZE);
		// Exactly one done emission, last, carrying the full listing — as before.
		expect(emits.filter((e) => e.done).length).toBe(1);
		expect(emits[emits.length - 1].done).toBe(true);
		expect(emits[emits.length - 1].n).toBe(PAGES * PAGE_SIZE);
		expect(refEmits[refEmits.length - 1].done).toBe(true);
		// Emissions are a subset of what the reference produced (never more).
		expect(emits.length).toBeLessThanOrEqual(refEmits.length);
		// Every emitted partial is a prefix-accumulation (monotone growth).
		for (let i = 1; i < emits.length; i++) expect(emits[i].n).toBeGreaterThan(emits[i - 1].n);
	});

	it('single-page folders still emit exactly once, done=true (fast path untouched)', async () => {
		vi.mocked(apiFetch).mockResolvedValue(
			fakeRes({ items: pageBody(PAGES - 1).items }) // no next_cursor
		);
		const emits: boolean[] = [];
		await fetchFolderListing('one', { onPage: (_p, done) => emits.push(done) });
		expect(emits).toEqual([true]);
	});

	it(
		`collapses the O(N²) consumer re-derive on a fast ${PAGES}-page load (perf gate)`,
		{ timeout: 30_000 },
		async () => {
			// Warm-up both paths, twice each, so V8's tiering has fully
			// settled before we measure. A single warm-up was enough on
			// developer laptops but bursty CPU steals on shared CI
			// runners can leave one path un-tiered during measurement,
			// skewing the wall-time ratio at line ~202 below.
			for (let i = 0; i < 2; i++) {
				mockPagedFetch();
				await referenceFetchFolderListing('warm', (p) => consumerDerive(p));
				mockPagedFetch();
				await fetchFolderListing('warm', { onPage: (p) => consumerDerive(p) });
			}

			mockPagedFetch();
			let refSorted = 0;
			let refEmits = 0;
			const t0 = performance.now();
			await referenceFetchFolderListing('bench', (p) => {
				refEmits++;
				refSorted += consumerDerive(p);
			});
			const refMs = performance.now() - t0;

			mockPagedFetch();
			let sorted = 0;
			let emitsN = 0;
			const t1 = performance.now();
			await fetchFolderListing('bench', {
				onPage: (p) => {
					emitsN++;
					sorted += consumerDerive(p);
				}
			});
			const ms = performance.now() - t1;

			console.info(
				`progressive load ${PAGES}×${PAGE_SIZE}: before ${refEmits} emissions / ${refSorted} sorted elements / ${refMs.toFixed(1)} ms — after ${emitsN} emissions / ${sorted} sorted elements / ${ms.toFixed(1)} ms (${(refMs / ms).toFixed(1)}x wall, ${(refSorted / sorted).toFixed(1)}x fewer sorted elements)`
			);

			// The reference re-derived every page: Σ = P(P+1)/2 pages of elements.
			expect(refEmits).toBe(PAGES);
			expect(refSorted).toBe((PAGES * (PAGES + 1) * PAGE_SIZE) / 2);
			// Coalesced: page 1 + final (+ occasionally one mid emission if the
			// stubbed pages ever take >150 ms — they don't on any healthy runner).
			expect(emitsN).toBeLessThanOrEqual(3);
			// ≥5x less consumer sort work is the point of the change.
			// This is a pure DETERMINISTIC count (sum of `consumerDerive`
			// return values) — hardware-independent, so catches an
			// actual O(N²) → O(N) regression cleanly.
			expect(sorted).toBeLessThan(refSorted / 5);
			// And it must show up as wall time on the combined load+
			// derive cycle. 2x floor (loosened from 3x on 2026-07-18
			// after a shared-CI-runner false alarm at 2.63x — bursty
			// CPU steals eat headroom on the fine-grained
			// `performance.now()` measurements). Still catches an
			// O(N²) regression (which would be ~10x slower, not 2x)
			// — the deterministic count above at line 200 is the real
			// algorithmic gate.
			expect(ms).toBeLessThan(refMs / 2);
		}
	);
});
