import { describe, expect, it } from 'vitest';

/**
 * Benchmark gate for the files view's batch-operation rework
 * (`batchDelete` / `moveInto` / `selectionTargets` / `batchDownload` in
 * `[...path]/+page.svelte`).
 *
 * Audit finding: multi-item delete/move awaited one request per item in a
 * serial loop — at ~30 ms RTT a 100-item delete is ~3 s of waterfall — and
 * every per-id classification ran `listing.folders.find(...)` /
 * `listing.files.some(...)`, an O(N·M) scan over the listing per selected id.
 * The fix builds an id index once (O(M)) and fans the requests out through
 * the view's existing `mapLimit` with 6 in flight.
 *
 * The functions are component-internal, so — like the Rust bench modules that
 * replicate handler internals — this bench replicates BEFORE verbatim and
 * AFTER (index + `mapLimit`, the exact shapes now in the component) against a
 * stubbed per-item endpoint with simulated latency.
 *
 * Gates: (1) both arms attempt the identical (id, kind) operation set —
 * folder-first classification preserved; (2) a 100-item batch at 5 ms
 * simulated RTT completes ≥3x faster; (3) the classification scan count
 * drops from O(N·M) to one pass.
 */

const M = 2_000; // listing size
const N = 100; // selection size
const RTT_MS = 5;

const listing = {
	folders: Array.from({ length: M / 4 }, (_, i) => ({ id: `d-${i}`, name: `dir ${i}` })),
	files: Array.from({ length: (3 * M) / 4 }, (_, i) => ({ id: `f-${i}`, name: `file ${i}` }))
};
// Selection interleaves folders and files, like a shift-range over a mixed view.
const selectedIds = [
	...listing.folders.slice(40, 40 + N / 4).map((f) => f.id),
	...listing.files.slice(900, 900 + (3 * N) / 4).map((f) => f.id)
];

/** Stubbed per-item endpoint: RTT_MS latency, records the attempted op. */
function makeOps() {
	const attempted: Array<{ id: string; kind: 'file' | 'folder' }> = [];
	let comparisons = 0;
	return {
		attempted,
		countCmp: () => comparisons++,
		get comparisons() {
			return comparisons;
		},
		deleteFolder: async (id: string) => {
			attempted.push({ id, kind: 'folder' });
			await new Promise((r) => setTimeout(r, RTT_MS));
		},
		deleteFile: async (id: string) => {
			attempted.push({ id, kind: 'file' });
			await new Promise((r) => setTimeout(r, RTT_MS));
		}
	};
}
type Ops = ReturnType<typeof makeOps>;

/** BEFORE, verbatim shape: serial await + `find` per id. */
async function batchDeleteBefore(ids: string[], ops: Ops): Promise<void> {
	for (const id of ids) {
		const folder = listing.folders.find((f) => {
			ops.countCmp();
			return f.id === id;
		});
		if (folder) await ops.deleteFolder(id);
		else await ops.deleteFile(id);
	}
}

/** The view's `mapLimit`, verbatim. */
async function mapLimit<T, R>(
	items: T[],
	limit: number,
	fn: (item: T) => Promise<R>
): Promise<R[]> {
	const out = new Array<R>(items.length);
	let next = 0;
	const worker = async () => {
		while (next < items.length) {
			const i = next++;
			out[i] = await fn(items[i]);
		}
	};
	await Promise.all(Array.from({ length: Math.min(limit, items.length) }, worker));
	return out;
}

/** AFTER, verbatim shape: one O(M) index pass + bounded fan-out of 6. */
async function batchDeleteAfter(ids: string[], ops: Ops): Promise<void> {
	const folderIdSet = new Set(
		listing.folders.map((f) => {
			ops.countCmp();
			return f.id;
		})
	);
	await mapLimit(ids, 6, async (id) => {
		if (folderIdSet.has(id)) await ops.deleteFolder(id);
		else await ops.deleteFile(id);
	});
}

const opKey = (o: { id: string; kind: string }) => `${o.kind}:${o.id}`;

describe('files-view batch operations (benchmark gate)', () => {
	it(
		'both arms attempt the identical operation set, ≥3x faster fanned out',
		{ timeout: 30_000 },
		async () => {
			const before = makeOps();
			const t0 = performance.now();
			await batchDeleteBefore(selectedIds, before);
			const beforeMs = performance.now() - t0;

			const after = makeOps();
			const t1 = performance.now();
			await batchDeleteAfter(selectedIds, after);
			const afterMs = performance.now() - t1;

			// Equivalence: same ops, same folder/file classification. Order is
			// not part of the contract (the ops are independent single-item
			// endpoints); compare as sets and sizes.
			expect(after.attempted.length).toBe(before.attempted.length);
			expect(new Set(after.attempted.map(opKey))).toEqual(new Set(before.attempted.map(opKey)));
			expect(before.attempted.filter((o) => o.kind === 'folder').length).toBe(N / 4);

			// Scan work: O(N·M) probes collapse to one O(M) pass.
			expect(after.comparisons).toBe(listing.folders.length);
			expect(before.comparisons).toBeGreaterThan(after.comparisons * 10);

			console.info(
				`batch delete ${N} items @ ${RTT_MS} ms RTT: serial ${beforeMs.toFixed(0)} ms (${before.comparisons} id probes) vs mapLimit(6) ${afterMs.toFixed(0)} ms (${after.comparisons} probes) — ${(beforeMs / afterMs).toFixed(1)}x`
			);
			expect(afterMs).toBeLessThan(beforeMs / 3);
		}
	);

	it('selectionTargets index matches the per-id find, folder-first on collision', () => {
		// BEFORE: folder probed first per id. AFTER: files inserted first so
		// folders overwrite → folder wins collisions. Same observable result.
		const shadow = { id: listing.files[0].id, name: 'shadow-folder' };
		const foldersPlus = [...listing.folders, shadow];
		const wanted = [shadow.id, listing.folders[5].id, listing.files[10].id, 'missing-id'];

		const beforeTargets = wanted
			.map((id) => {
				const folder = foldersPlus.find((f) => f.id === id);
				if (folder) return { id, name: folder.name, kind: 'folder' as const };
				const file = listing.files.find((f) => f.id === id);
				return file ? { id, name: file.name, kind: 'file' as const } : null;
			})
			.filter((x): x is NonNullable<typeof x> => x !== null);

		const byId = new Map<string, { id: string; name: string; kind: 'file' | 'folder' }>();
		for (const f of listing.files) byId.set(f.id, { id: f.id, name: f.name, kind: 'file' });
		for (const f of foldersPlus) byId.set(f.id, { id: f.id, name: f.name, kind: 'folder' });
		const afterTargets = wanted
			.map((id) => byId.get(id) ?? null)
			.filter((x): x is NonNullable<typeof x> => x !== null);

		expect(afterTargets).toEqual(beforeTargets);
	});
});
