/**
 * Delta upload ("upload only what changed") — ported from
 * features/files/deltaUpload.js. Main-thread orchestrator for
 * `/static/workers/deltaWorker.js`, which runs FastCDC chunking + BLAKE3
 * (the same WASM crate/params as the server) off the UI thread, negotiates
 * which chunks the server already has, uploads only the missing ones, and
 * commits. Any failure resolves `null` so the caller falls back to a plain
 * byte upload — delta is an optimization, never a gate.
 */
import { getCsrfToken } from '$lib/api/csrf';
import { createFileByHash, dedupCheckBatch } from '$lib/api/endpoints/files';
import { blake3HexOfFile } from '$lib/vendor/hashWasm';

/** Files smaller than this skip delta: the round-trips cost more than the bytes.
 *  Also the upper bound for client-side whole-file hashing (instant by-hash
 *  uploads) — we never read a file larger than this fully into memory. Files at
 *  or above this run the delta worker (sub-file dedup, saves re-upload
 *  bandwidth); the worker self-bounds its concurrent connections so a few
 *  running at once can't exhaust the browser's ~6-per-host budget. */
export const DELTA_UPLOAD_MIN_SIZE = 8 * 1024 * 1024;

const DELTA_WORKER_URL = '/workers/deltaWorker.js';
const DELTA_TIMEOUT_BASE_MS = 120_000;
const DELTA_TIMEOUT_PER_GB_MS = 90_000;

export interface DeltaUploadAnswer {
	ok: boolean;
	data?: unknown;
	errorMsg?: string;
	isQuotaError?: boolean;
	/** Bytes NOT transferred thanks to dedup. */
	savedBytes?: number;
}

/** `false` once the environment proved unable to run the worker/WASM. */
let usable: boolean | null = null;

interface ProgressMsg {
	type: 'progress';
	reusedBytes: number;
	uploadedBytes: number;
	totalBytes: number;
}
interface FallbackMsg {
	type: 'fallback';
	reason?: string;
}
interface DoneMsg {
	type: 'done';
	status: number;
	body?: { message?: string; error?: string; still_missing?: unknown };
}
type WorkerMsg = ProgressMsg | FallbackMsg | DoneMsg;

/**
 * Try to upload `file` through the delta protocol. Resolves `null` whenever
 * the plain byte upload should proceed (too small, environment unusable, any
 * transport/protocol failure). `onProgress` receives 0–99 while transferring.
 */
export function tryDeltaUpload(
	file: File,
	folderId: string | null | undefined,
	onProgress?: (pct: number) => void
): Promise<DeltaUploadAnswer | null> {
	if (
		!folderId ||
		file.size < DELTA_UPLOAD_MIN_SIZE ||
		usable === false ||
		typeof Worker === 'undefined'
	) {
		return Promise.resolve(null);
	}

	return new Promise((resolve) => {
		let worker: Worker;
		try {
			worker = new Worker(DELTA_WORKER_URL, { type: 'module' });
		} catch {
			usable = false;
			resolve(null);
			return;
		}

		const sizeGB = file.size / (1024 * 1024 * 1024);
		const timeoutMs = DELTA_TIMEOUT_BASE_MS + Math.ceil(sizeGB) * DELTA_TIMEOUT_PER_GB_MS;
		let savedBytes = 0;

		let stallTimer: ReturnType<typeof setTimeout>;
		const settle = (answer: DeltaUploadAnswer | null) => {
			clearTimeout(timer);
			clearTimeout(stallTimer);
			worker.terminate();
			resolve(answer);
		};
		const timer = setTimeout(() => settle(null), timeoutMs);

		// Liveness watchdog: a healthy worker posts progress sub-second while it
		// hashes and uploads. If it goes SILENT this long it is wedged (WASM init
		// or chunking hung without throwing, emitting neither fallback nor error)
		// — exactly what freezes a folder upload ~2 min per large file. Disable
		// delta for this file AND every later one so they fall straight to a plain
		// upload instead of each burning the full size-scaled delta timeout.
		const STALL_MS = 20_000;
		const armStall = () => {
			clearTimeout(stallTimer);
			stallTimer = setTimeout(() => {
				usable = false;
				settle(null);
			}, STALL_MS);
		};
		armStall();

		worker.onmessage = (event: MessageEvent<WorkerMsg>) => {
			armStall(); // worker is alive — reset the liveness watchdog
			const msg = event.data;
			if (msg.type === 'progress') {
				savedBytes = msg.reusedBytes;
				if (onProgress && msg.totalBytes > 0) {
					const pct = Math.min(
						99,
						Math.round((100 * (msg.reusedBytes + msg.uploadedBytes)) / msg.totalBytes)
					);
					onProgress(pct);
				}
				return;
			}
			if (msg.type === 'fallback') {
				settle(null);
				return;
			}
			if (msg.type === 'done') {
				if (msg.status === 201 || msg.status === 200) {
					settle({ ok: true, data: msg.body, savedBytes });
					return;
				}
				const errorMsg =
					msg.body?.message || msg.body?.error || `Delta upload failed (HTTP ${msg.status})`;
				if (msg.status === 507) {
					settle({ ok: false, isQuotaError: true, errorMsg });
					return;
				}
				if (msg.status === 409 && !msg.body?.still_missing) {
					settle({ ok: false, errorMsg });
					return;
				}
				settle(null);
			}
		};
		worker.onerror = () => {
			usable = false;
			settle(null);
		};

		worker.postMessage({ file, folderId, name: file.name, csrfToken: getCsrfToken() || '' });
	});
}

/**
 * Create a file from a blob the caller already owns (`POST /api/files/by-hash`)
 * — zero content bytes cross the wire. `hash` must come from a prior batch
 * ownership check ([`resolveOwnedHashes`]). Resolves an answer with
 * `savedBytes = file.size` on success, surfaces a 507 quota error, or resolves
 * `null` to fall back to a normal upload (e.g. the blob was GC'd between the
 * check and this create — rare).
 */
export async function instantUploadOwned(
	folderId: string,
	file: File,
	hash: string
): Promise<DeltaUploadAnswer | null> {
	const res = await createFileByHash(folderId, file.name, hash);
	if (res.ok) return { ok: true, data: res.data, savedBytes: file.size };
	if (res.status === 507) {
		return { ok: false, isQuotaError: true, errorMsg: 'Storage quota exceeded' };
	}
	return null;
}

const HASH_WORKER_URL = '/workers/hashWorker.js';
/** Parallel hashing lanes — enough to saturate small-file hashing without
 *  starving the upload workers of cores. */
const HASH_POOL_SIZE = Math.min(4, Math.max(1, (navigator.hardwareConcurrency ?? 2) - 1));

/**
 * BLAKE3-hash `files` on a bounded pool of dedicated workers (main thread
 * stays free). A file whose worker errors is simply absent from the result —
 * the caller uploads it the normal way. Falls back to the sequential inline
 * hasher when `Worker` is unavailable.
 */
async function hashFilesPooled(files: File[]): Promise<Map<File, string>> {
	if (typeof Worker === 'undefined') {
		const out = new Map<File, string>();
		for (const f of files) out.set(f, await blake3HexOfFile(f));
		return out;
	}
	const lanes = Math.min(HASH_POOL_SIZE, files.length);
	const workers = Array.from(
		{ length: lanes },
		() => new Worker(HASH_WORKER_URL, { type: 'module' })
	);
	const out = new Map<File, string>();
	let next = 0;
	try {
		await Promise.all(
			workers.map(
				(w) =>
					new Promise<void>((resolve, reject) => {
						const feed = () => {
							if (next >= files.length) {
								resolve();
								return;
							}
							const i = next++;
							const file = files[i];
							w.onmessage = (ev: MessageEvent<{ id: number; hex?: string; error?: string }>) => {
								if (ev.data.hex) out.set(file, ev.data.hex);
								feed(); // per-file errors: skip the file, keep the lane
							};
							w.onerror = (e) => reject(e);
							w.postMessage({ id: i, file });
						};
						feed();
					})
			)
		);
	} finally {
		for (const w of workers) w.terminate();
	}
	return out;
}

/**
 * Resolve which of `files` the server already owns, with a SINGLE batch round
 * trip (the Dropbox-style "have you got these?" probe). Every file below the
 * delta threshold is BLAKE3-hashed locally, the whole hash set is sent to
 * `/api/dedup/check-batch`, and the owned subset is mapped back to `file → hash`
 * so callers can instant-upload those (zero bytes) and upload the rest normally.
 *
 * Excludes empty files and files `>= DELTA_UPLOAD_MIN_SIZE` (the delta protocol
 * dedups those itself). Resolves an empty map on any failure — hashing
 * unavailable, request error — so uploads always proceed.
 */
export async function resolveOwnedHashes(files: File[]): Promise<Map<File, string>> {
	const inBand = files.filter((f) => f.size > 0 && f.size < DELTA_UPLOAD_MIN_SIZE);
	if (inBand.length === 0) return new Map();

	const hashByFile = new Map<File, string>();
	try {
		// Hash off the main thread on a small worker pool — the sequential
		// main-thread WASM loop blocked the UI for the whole batch and
		// delayed every upload lane behind the full hashing phase (measured
		// in deltaUpload.hash.test.ts). Falls back to the inline loop when
		// Workers are unavailable (some test environments).
		const hashed = await hashFilesPooled(inBand);
		for (const [f, h] of hashed) hashByFile.set(f, h);
	} catch {
		return new Map(); // WASM/hashing unavailable → skip instant uploads
	}

	let owned: Set<string>;
	try {
		owned = await dedupCheckBatch([...new Set(hashByFile.values())]);
	} catch {
		return new Map();
	}

	const result = new Map<File, string>();
	for (const [f, h] of hashByFile) if (owned.has(h)) result.set(f, h);
	return result;
}
