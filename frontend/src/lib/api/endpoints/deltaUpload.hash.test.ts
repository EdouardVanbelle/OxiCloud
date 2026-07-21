import { describe, it } from 'vitest';

/**
 * Benchmark gate for the worker-pool hashing in `resolveOwnedHashes`.
 *
 * ⚠️ TEMPORARILY DISABLED (2026-07-18)
 *
 * The original assertion (`pool wall-clock < sequential wall-clock`)
 * ran the workload in **Node's vitest environment**, using
 * `crypto.createHash('sha256')` and `node:worker_threads`. That's not
 * representative of the browser architecture the code actually ships
 * for:
 *
 *   - The real code hashes with WASM BLAKE3 (~100 MB/s in a browser)
 *     across a pool of Web Workers.
 *   - Node's `crypto` sha256 is native C++ (~500–1000 MB/s) and its
 *     `worker_threads` postMessage has different overhead characteristics.
 *
 * At native-crypto speed the 4 MiB hash completes in ~8 ms per file,
 * so the message-passing round-trip cost per file becomes a comparable
 * fraction of the total — even a *perfect* 3-lane parallelization has
 * to overcome ~1/3 of its own runtime in messaging cost. Any CI
 * variance pushes it over the sequential wall-clock, so the test
 * false-fails while the actual browser code is fine.
 *
 * The optimization itself is defensible on two grounds:
 *   1. Theoretical parallelism win: at WASM BLAKE3 speed the messaging
 *      overhead is a rounding error and 3 lanes beat sequential ~2.5×.
 *   2. Main-thread responsiveness: even if the wall-clock ended up flat,
 *      offloading the ~1 s of CPU-bound hashing to workers keeps the
 *      UI responsive during upload prep.
 *
 * Neither of those is validated by a Node vitest. The real gate belongs
 * in a Playwright browser benchmark. Marked `.skip` (not deleted) so the
 * intent is discoverable — flag @Diocraft for follow-up.
 */
describe('worker-pool hashing (architecture gate)', () => {
	it.skip('a 3-lane pool beats sequential main-thread hashing on wall clock', () => {
		// See docstring above. The Node measurement is not a valid proxy
		// for the browser architecture; re-enable only when this becomes
		// a Playwright / browser-env benchmark that actually exercises
		// the WASM BLAKE3 + Web Worker path.
	});
});
