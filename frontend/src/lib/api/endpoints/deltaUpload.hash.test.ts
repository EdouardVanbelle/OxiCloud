import { describe, expect, it } from 'vitest';
import { Worker } from 'node:worker_threads';
import { createHash } from 'node:crypto';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

/**
 * Benchmark gate for the worker-pool hashing in `resolveOwnedHashes`.
 *
 * The browser change moves per-file BLAKE3 hashing from a sequential
 * main-thread WASM loop onto a small pool of Web Workers. This test measures
 * the same architecture on this machine with node's worker_threads and a
 * CPU-bound digest as the stand-in workload: N buffers hashed sequentially
 * on one thread vs the same work fanned over a 3-lane pool. If the pool
 * doesn't beat sequential wall-clock, the frontend change must be rolled
 * back (it would be pure complexity).
 */
describe('worker-pool hashing (architecture gate)', () => {
	it('a 3-lane pool beats sequential main-thread hashing on wall clock', async () => {
		// Faithful to the browser shape: the main thread hands each worker a
		// FILE REFERENCE (browser: the File handle; here: its path) and the
		// worker does read + hash. The old shape reads + hashes every file
		// on the main thread, serially.
		const nFiles = 24;
		const size = 4 * 1024 * 1024;
		const dir = await fs.mkdtemp(join(tmpdir(), 'hashbench-'));
		const paths: string[] = [];
		for (let i = 0; i < nFiles; i++) {
			const p = join(dir, `f${i}`);
			const b = Buffer.alloc(size);
			b.fill(i + 1);
			await fs.writeFile(p, b);
			paths.push(p);
		}

		// Sequential (old): read + hash on the calling thread.
		const t0 = performance.now();
		for (const p of paths) {
			const b = await fs.readFile(p);
			createHash('sha256').update(b).digest('hex');
		}
		const seqMs = performance.now() - t0;

		// 3-lane pool (new): each worker reads + hashes its own files.
		const lanes = 3;
		const workerSrc = `
			const { parentPort } = require('node:worker_threads');
			const { createHash } = require('node:crypto');
			const { readFileSync } = require('node:fs');
			parentPort.on('message', (path) => {
				const b = readFileSync(path);
				parentPort.postMessage(createHash('sha256').update(b).digest('hex'));
			});
		`;
		const workers = Array.from({ length: lanes }, () => new Worker(workerSrc, { eval: true }));
		let next = 0;
		const t1 = performance.now();
		await Promise.all(
			workers.map(
				(w) =>
					new Promise<void>((resolve, reject) => {
						const feed = () => {
							if (next >= paths.length) {
								resolve();
								return;
							}
							const i = next++;
							w.once('message', () => feed());
							w.once('error', reject);
							w.postMessage(paths[i]);
						};
						feed();
					})
			)
		);
		const poolMs = performance.now() - t1;
		await Promise.all(workers.map((w) => w.terminate()));
		await fs.rm(dir, { recursive: true, force: true });

		// eslint-disable-next-line no-console
		console.info(
			`read+hash ${nFiles} x 4 MiB: sequential ${seqMs.toFixed(0)} ms vs 3-lane pool ${poolMs.toFixed(0)} ms (${(seqMs / poolMs).toFixed(1)}x)`
		);
		expect(poolMs).toBeLessThan(seqMs);
	});
});
