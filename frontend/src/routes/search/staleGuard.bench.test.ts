import { describe, expect, it } from 'vitest';

/**
 * Benchmark gate for the search stale-response guard + AbortController
 * (search/+page.svelte `run()` and AppShell's suggest fetch).
 *
 * Audit finding (open since ROUND7): every query/sort/scope/filter change
 * re-fired `run(query)` with NO sequence token and NO abort — so (a) a slow
 * earlier response could resolve after a newer one and overwrite `results`
 * with stale hits, and (b) every superseded server search ran to completion
 * (wasted recursive-search CPU + bandwidth on the backend).
 *
 * Gates:
 *  1. Correctness — with responses resolving in REVERSE order, the unguarded
 *     BEFORE shape ends showing the FIRST (stale) query's results; the
 *     guarded AFTER shape always ends with the LAST query's results.
 *  2. Perf — the AFTER shape aborts every superseded request: for N
 *     rapid-fire queries only 1 reaches full completion (N-1 aborted), where
 *     BEFORE always pays N complete round-trips.
 */

interface FakeResults {
	forQuery: string;
}

/** A fetch whose resolution order and abort behaviour we control. */
function fakeSearch(
	q: string,
	delayMs: number,
	completed: { count: number },
	signal?: AbortSignal
): Promise<FakeResults> {
	return new Promise((resolve, reject) => {
		const timer = setTimeout(() => {
			completed.count++;
			resolve({ forQuery: q });
		}, delayMs);
		signal?.addEventListener('abort', () => {
			clearTimeout(timer);
			reject(new DOMException('aborted', 'AbortError'));
		});
	});
}

/** BEFORE — verbatim old `run()` shape: fire and assign, no token, no abort. */
function makeBefore(completed: { count: number }) {
	const state = { results: null as FakeResults | null };
	return {
		state,
		run: async (q: string, delayMs: number) => {
			try {
				state.results = await fakeSearch(q, delayMs, completed);
			} catch {
				/* unreachable in this harness */
			}
		}
	};
}

/** AFTER — the shipped shape: seq token + AbortController per run. */
function makeAfter(completed: { count: number }) {
	const state = { results: null as FakeResults | null };
	let runSeq = 0;
	let inflight: AbortController | null = null;
	return {
		state,
		run: async (q: string, delayMs: number) => {
			const seq = ++runSeq;
			inflight?.abort();
			const ctl = new AbortController();
			inflight = ctl;
			try {
				const fresh = await fakeSearch(q, delayMs, completed, ctl.signal);
				if (seq !== runSeq) return;
				state.results = fresh;
			} catch {
				if (seq !== runSeq || ctl.signal.aborted) return;
			}
		}
	};
}

describe('search stale-response guard (benchmark gate)', () => {
	it('BEFORE clobbers with stale results; AFTER keeps the latest query', async () => {
		// Query "a" resolves SLOWLY (60 ms), "ab" (30 ms), "abc" fast (1 ms):
		// resolution order is the reverse of issue order.
		const beforeDone = { count: 0 };
		const before = makeBefore(beforeDone);
		const pBefore = [before.run('a', 60), before.run('ab', 30), before.run('abc', 1)];
		await Promise.all(pBefore);
		// The slowest (oldest) response lands last and wins — the bug.
		expect(before.state.results?.forQuery).toBe('a');
		expect(beforeDone.count).toBe(3); // every superseded search ran to completion

		const afterDone = { count: 0 };
		const after = makeAfter(afterDone);
		const pAfter = [after.run('a', 60), after.run('ab', 30), after.run('abc', 1)];
		await Promise.all(pAfter);
		expect(after.state.results?.forQuery).toBe('abc'); // latest wins, always
		expect(afterDone.count).toBe(1); // superseded requests were aborted
	});

	it('rapid-fire burst: completed round-trips collapse N → 1', async () => {
		const N = 10;
		const beforeDone = { count: 0 };
		const before = makeBefore(beforeDone);
		await Promise.all(
			Array.from({ length: N }, (_, i) => before.run(`q${i}`, (N - i) * 5)) // reverse order
		);
		const afterDone = { count: 0 };
		const after = makeAfter(afterDone);
		await Promise.all(Array.from({ length: N }, (_, i) => after.run(`q${i}`, (N - i) * 5)));

		expect(beforeDone.count).toBe(N);
		expect(afterDone.count).toBe(1);
		expect(after.state.results?.forQuery).toBe(`q${N - 1}`);
		console.log(
			`[bench] ${N} rapid-fire searches — completed round-trips BEFORE=${beforeDone.count} AFTER=${afterDone.count}; final result BEFORE="${before.state.results?.forQuery}" (stale) AFTER="${after.state.results?.forQuery}" (fresh)`
		);
	});
});
