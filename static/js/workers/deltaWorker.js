/**
 * OxiCloud — delta-upload worker ("upload only what changed").
 *
 * Runs the whole client side of the delta protocol off the main thread:
 *
 *   read 8 MiB slices ─► FastCDC chunk + BLAKE3 (WASM, same crate and
 *   parameters as the server) ─► negotiate hash batches ─► upload only
 *   the missing chunks (framed, bounded concurrency) ─► commit.
 *
 * The stages OVERLAP: negotiation of batch N and uploads of its missing
 * chunks run while batch N+1 is still being hashed, so wall-clock time
 * approaches max(hash time, upload time) instead of their sum. RAM stays
 * flat: chunk bytes are re-sliced from the File at upload time, never
 * hoarded.
 *
 * Protocol with the spawner:
 *   in  : { file: File, folderId: string, name: string, csrfToken: string }
 *   out : { type: 'progress', hashedBytes, reusedBytes, uploadedBytes, totalBytes }
 *         { type: 'done', status, body }      — conclusive HTTP outcome
 *         { type: 'fallback', reason }        — do a plain byte upload
 */

// Absolute URLs on purpose: vendors/workers are served verbatim in both
// dev and the release IIFE bundle (same pattern as the pdf.js loader).
const WASM_GLUE_URL = '/js/vendors/hash-wasm/oxicloud_hash_wasm.js';

/** File read granularity — large enough to amortize Blob→ArrayBuffer. */
const SLICE_BYTES = 8 * 1024 * 1024;
/** Negotiate after this many freshly hashed chunks (~64 MiB of content). */
const NEGOTIATE_BATCH = 256;
/** Group missing chunks into PUT bodies of at most this many bytes. */
const UPLOAD_BATCH_BYTES = 8 * 1024 * 1024;
/** Concurrent chunk-PUT requests. */
const UPLOAD_CONCURRENCY = 2;
/** Re-commit attempts when the server answers 409 still_missing. */
const COMMIT_RETRIES = 2;

/**
 * Typed view of the dedicated-worker global scope (jsconfig targets the
 * DOM lib, where `self` is a Window — cast to what this worker uses).
 * @type {{ onmessage: ((event: MessageEvent) => void) | null,
 *          postMessage: (message: unknown) => void }}
 */
const workerScope = /** @type {any} */ (self);

/**
 * One chunk occurrence, in file order.
 * @typedef {{ h: string, s: number, offset: number }} WorkerChunk
 */

/** @returns {Promise<any>} the initialized WASM module */
async function loadWasm() {
    const mod = await import(WASM_GLUE_URL);
    await mod.default();
    return mod;
}

workerScope.onmessage = async (event) => {
    const { file, folderId, name, csrfToken } = /** @type {{ file: File, folderId: string, name: string, csrfToken: string }} */ (event.data);

    /** @param {string} reason */
    const fallback = (reason) => workerScope.postMessage({ type: 'fallback', reason });

    /** @type {Record<string, string>} */
    const mutHeaders = { 'Content-Type': 'application/json' };
    if (csrfToken) mutHeaders['X-CSRF-Token'] = csrfToken;

    let wasm;
    try {
        wasm = await loadWasm();
    } catch (err) {
        fallback(`wasm unavailable: ${err instanceof Error ? err.message : String(err)}`);
        return;
    }

    // ── Shared pipeline state ─────────────────────────────────────
    /** @type {WorkerChunk[]} */
    const chunks = []; // every occurrence, in file order
    /** @type {Set<string>} */
    const seenForNegotiate = new Set(); // distinct hashes already sent to negotiate
    let reusedBytes = 0;
    let uploadedBytes = 0;
    let hashedBytes = 0;
    let failed = /** @type {string | null} */ (null);

    let lastProgress = 0;
    const progress = (force = false) => {
        const now = Date.now();
        if (!force && now - lastProgress < 150) return;
        lastProgress = now;
        workerScope.postMessage({
            type: 'progress',
            hashedBytes,
            reusedBytes,
            uploadedBytes,
            totalBytes: file.size
        });
    };

    // ── Upload stage: bounded-concurrency drain of uploadByHash ──
    /** @type {WorkerChunk[]} */
    const uploadQueue = [];
    /** @type {Promise<void>[]} */
    const uploadWorkers = [];
    let uploadsClosed = false;
    /** @type {(() => void) | null} */
    let wakeUploader = null;
    const signalUploaders = () => {
        if (wakeUploader) {
            const w = wakeUploader;
            wakeUploader = null;
            w();
        }
    };

    /** Encode a batch of chunks as [u32 BE len][bytes] frames. */
    const encodeFrames = async (/** @type {WorkerChunk[]} */ batch) => {
        const total = batch.reduce((n, c) => n + 4 + c.s, 0);
        const wire = new Uint8Array(total);
        const view = new DataView(wire.buffer);
        let at = 0;
        for (const c of batch) {
            // eslint-disable-next-line no-await-in-loop -- sequential by design: constant RAM
            const bytes = new Uint8Array(await file.slice(c.offset, c.offset + c.s).arrayBuffer());
            view.setUint32(at, c.s, false);
            wire.set(bytes, at + 4);
            at += 4 + c.s;
        }
        return wire;
    };

    const uploadLoop = async () => {
        while (!failed) {
            // Take up to UPLOAD_BATCH_BYTES from the queue.
            /** @type {WorkerChunk[]} */
            const batch = [];
            let bytes = 0;
            while (uploadQueue.length > 0 && bytes < UPLOAD_BATCH_BYTES) {
                const c = /** @type {WorkerChunk} */ (uploadQueue.shift());
                batch.push(c);
                bytes += c.s;
            }
            if (batch.length === 0) {
                if (uploadsClosed) return;
                // eslint-disable-next-line no-await-in-loop -- queue wait
                await new Promise((resolve) => {
                    wakeUploader = /** @type {() => void} */ (resolve);
                });
                continue;
            }
            try {
                // eslint-disable-next-line no-await-in-loop -- bounded by pool size
                const wire = await encodeFrames(batch);
                // eslint-disable-next-line no-await-in-loop -- bounded by pool size
                const response = await fetch('/api/files/delta/chunks', {
                    method: 'PUT',
                    headers: {
                        'Content-Type': 'application/octet-stream',
                        ...(csrfToken ? { 'X-CSRF-Token': csrfToken } : {})
                    },
                    body: wire
                });
                if (!response.ok) {
                    failed = `chunk PUT failed (HTTP ${response.status})`;
                    return;
                }
                for (const c of batch) uploadedBytes += c.s;
                progress();
            } catch (err) {
                failed = `chunk PUT failed: ${err instanceof Error ? err.message : String(err)}`;
                return;
            }
        }
    };
    for (let i = 0; i < UPLOAD_CONCURRENCY; i++) uploadWorkers.push(uploadLoop());

    // ── Negotiate stage ───────────────────────────────────────────
    /** @type {Promise<void>[]} */
    const negotiations = [];
    const negotiate = (/** @type {WorkerChunk[]} */ fresh) => {
        if (fresh.length === 0 || failed) return;
        negotiations.push(
            (async () => {
                try {
                    const response = await fetch('/api/files/delta/negotiate', {
                        method: 'POST',
                        headers: mutHeaders,
                        body: JSON.stringify({ chunks: fresh.map(({ h, s }) => ({ h, s })) })
                    });
                    if (!response.ok) {
                        failed = failed || `negotiate failed (HTTP ${response.status})`;
                        return;
                    }
                    const missing = new Set(/** @type {{missing: string[]}} */ (await response.json()).missing);
                    for (const c of fresh) {
                        if (missing.has(c.h)) {
                            uploadQueue.push(c);
                        } else {
                            reusedBytes += c.s;
                        }
                    }
                    signalUploaders();
                    progress();
                } catch (err) {
                    failed = failed || `negotiate failed: ${err instanceof Error ? err.message : String(err)}`;
                }
            })()
        );
    };

    // ── Chunking stage (drives the other two) ────────────────────
    try {
        const chunker = new wasm.DeltaChunker();
        /** @type {WorkerChunk[]} */
        let freshBatch = [];
        let offset = 0;

        /** @param {[string, number][]} emitted */
        const onChunks = (emitted) => {
            for (const [h, s] of emitted) {
                /** @type {WorkerChunk} */
                const chunk = { h, s, offset };
                offset += s;
                chunks.push(chunk);
                if (seenForNegotiate.has(h)) {
                    // Repeated content inside the same file: the first
                    // occurrence decides upload vs reuse; later ones are
                    // pure reuse for accounting.
                    reusedBytes += s;
                } else {
                    seenForNegotiate.add(h);
                    freshBatch.push(chunk);
                    if (freshBatch.length >= NEGOTIATE_BATCH) {
                        negotiate(freshBatch);
                        freshBatch = [];
                    }
                }
            }
        };

        for (let read = 0; read < file.size && !failed; read += SLICE_BYTES) {
            const end = Math.min(read + SLICE_BYTES, file.size);
            // eslint-disable-next-line no-await-in-loop -- sequential by design: constant RAM
            const slice = new Uint8Array(await file.slice(read, end).arrayBuffer());
            onChunks(JSON.parse(chunker.update(slice)));
            hashedBytes = end;
            progress();
        }
        const fin = JSON.parse(chunker.finish());
        chunker.free();
        onChunks(fin.chunks);
        negotiate(freshBatch);
        const fileHash = /** @type {string} */ (fin.file_hash);
        hashedBytes = file.size;
        progress(true);

        // ── Drain: negotiations → uploads → commit ───────────────
        await Promise.all(negotiations);
        uploadsClosed = true;
        signalUploaders();
        await Promise.all(uploadWorkers);
        if (failed) {
            fallback(failed);
            return;
        }

        const commitBody = {
            file_hash: fileHash,
            chunks: chunks.map(({ h, s }) => ({ h, s })),
            name,
            folder_id: folderId
        };
        for (let attempt = 0; ; attempt++) {
            // eslint-disable-next-line no-await-in-loop -- retry loop
            const response = await fetch('/api/files/delta/commit', {
                method: 'POST',
                headers: mutHeaders,
                body: JSON.stringify(commitBody)
            });
            /** @type {any} */
            let body = null;
            try {
                // eslint-disable-next-line no-await-in-loop -- retry loop
                body = await response.json();
            } catch (_) {}

            const stillMissing = response.status === 409 && Array.isArray(body?.still_missing);
            if (stillMissing && attempt < COMMIT_RETRIES) {
                // GC race or a chunk we wrongly assumed claimable: upload
                // exactly what the server names and try again.
                const byHash = new Map(chunks.map((c) => [c.h, c]));
                /** @type {WorkerChunk[]} */
                const retry = [];
                for (const h of body.still_missing) {
                    const c = byHash.get(h);
                    if (!c) {
                        fallback('server requested an unknown chunk');
                        return;
                    }
                    retry.push(c);
                }
                const wire = await encodeFrames(retry);
                // eslint-disable-next-line no-await-in-loop -- retry loop
                const put = await fetch('/api/files/delta/chunks', {
                    method: 'PUT',
                    headers: {
                        'Content-Type': 'application/octet-stream',
                        ...(csrfToken ? { 'X-CSRF-Token': csrfToken } : {})
                    },
                    body: wire
                });
                if (!put.ok) {
                    fallback(`retry chunk PUT failed (HTTP ${put.status})`);
                    return;
                }
                for (const c of retry) uploadedBytes += c.s;
                progress(true);
                continue;
            }

            // Conclusive: 201 created, or a real error (quota, name
            // conflict, validation). The spawner maps it to the uploaders'
            // UploadAnswer contract.
            workerScope.postMessage({ type: 'done', status: response.status, body });
            return;
        }
    } catch (err) {
        fallback(err instanceof Error ? err.message : String(err));
    }
};
