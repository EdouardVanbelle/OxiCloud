/**
 * OxiCloud - Delta upload ("upload only what changed").
 *
 * Main-thread orchestrator for `workers/deltaWorker.js`, which runs the
 * whole client side of the delta protocol off the UI thread: FastCDC
 * chunking + BLAKE3 (the same WASM crate and parameters as the server,
 * so boundaries match bit for bit), per-batch negotiation, upload of
 * only the missing chunks, and the commit.
 *
 * This SUBSUMES the previous whole-file instant upload: a fully known
 * file negotiates to "nothing missing" and the commit short-circuits on
 * possession of the file hash — same zero-byte outcome, one pipeline.
 *
 * Performance posture:
 * - Stages overlap inside the worker (hash ‖ negotiate ‖ upload), so
 *   wall-clock approaches max(hash, upload) instead of their sum.
 * - RAM stays flat: 8 MiB read slices; chunk bytes are re-sliced from
 *   the File at upload time, never hoarded.
 * - Files below {@link DELTA_UPLOAD_MIN_SIZE} skip the pipeline: the
 *   round-trips cost more than the bytes.
 * - Any failure falls back silently to the normal byte upload — delta
 *   is an optimization, never a gate.
 */

import { getCsrfToken } from '../../core/csrf.js';

/**
 * Files smaller than this upload normally: hashing + negotiation
 * round-trips outweigh the transfer.
 */
export const DELTA_UPLOAD_MIN_SIZE = 8 * 1024 * 1024;

// Absolute URL on purpose — works in dev and in the release IIFE bundle
// (same pattern as the pdf.js loader in thumbnail.js).
const DELTA_WORKER_URL = '/js/workers/deltaWorker.js';

/** Budget: 120 s base + 90 s per GB (hashing + uploading the delta). */
const DELTA_TIMEOUT_BASE_MS = 120000;
const DELTA_TIMEOUT_PER_GB_MS = 90000;

/**
 * `false` once the environment proved unable to run the worker/WASM —
 * later files skip straight to the byte upload. `null` = not yet known.
 * @type {boolean | null}
 */
let _deltaUploadUsable = null;

/**
 * Result contract shared with the uploaders' `UploadAnswer`, plus the
 * bandwidth accounting the UI surfaces.
 * @typedef {Object} DeltaUploadAnswer
 * @property {boolean} ok
 * @property {any} [data] FileDto on success
 * @property {string} [errorMsg]
 * @property {boolean} [isQuotaError]
 * @property {number} [savedBytes] bytes NOT transferred thanks to dedup
 */

/**
 * Try to upload `file` through the delta protocol.
 *
 * Resolves `null` whenever the plain byte upload should proceed (file too
 * small, environment unusable, any transport/protocol failure). Resolves
 * a {@link DeltaUploadAnswer} when the outcome is conclusive — success,
 * quota exceeded, or a name conflict a byte upload would also hit.
 *
 * @param {File} file
 * @param {string | null | undefined} folderId
 * @param {(pct: number) => void} [onProgress] 0-99 while transferring
 * @returns {Promise<DeltaUploadAnswer | null>}
 */
export function tryDeltaUpload(file, folderId, onProgress) {
    if (!folderId || file.size < DELTA_UPLOAD_MIN_SIZE || _deltaUploadUsable === false || typeof Worker === 'undefined') {
        return Promise.resolve(null);
    }

    return new Promise((resolve) => {
        /** @type {Worker} */
        let worker;
        try {
            worker = new Worker(DELTA_WORKER_URL, { type: 'module' });
        } catch (_) {
            _deltaUploadUsable = false;
            resolve(null);
            return;
        }

        const sizeGB = file.size / (1024 * 1024 * 1024);
        const timeoutMs = DELTA_TIMEOUT_BASE_MS + Math.ceil(sizeGB) * DELTA_TIMEOUT_PER_GB_MS;

        let savedBytes = 0;

        /** @param {DeltaUploadAnswer | null} answer */
        const settle = (answer) => {
            clearTimeout(timer);
            worker.terminate();
            resolve(answer);
        };
        const timer = setTimeout(() => settle(null), timeoutMs);

        worker.onmessage = (event) => {
            const msg = /** @type {any} */ (event.data);
            if (msg.type === 'progress') {
                savedBytes = msg.reusedBytes;
                if (onProgress && msg.totalBytes > 0) {
                    const pct = Math.min(99, Math.round((100 * (msg.reusedBytes + msg.uploadedBytes)) / msg.totalBytes));
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
                /** @type {string} */
                const errorMsg = msg.body?.message || msg.body?.error || `Delta upload failed (HTTP ${msg.status})`;
                if (msg.status === 507) {
                    settle({ ok: false, isQuotaError: true, errorMsg });
                    return;
                }
                if (msg.status === 409 && !msg.body?.still_missing) {
                    // Duplicate name — a byte upload would hit the same wall.
                    settle({ ok: false, errorMsg });
                    return;
                }
                // still_missing exhausted, 4xx/5xx oddities: byte upload is
                // the safe road (the server dedups it on write anyway).
                settle(null);
            }
        };
        worker.onerror = () => {
            // Worker script failed to load/parse — permanent environment trait.
            _deltaUploadUsable = false;
            settle(null);
        };

        worker.postMessage({
            file,
            folderId,
            name: file.name,
            csrfToken: getCsrfToken() || ''
        });
    });
}

/**
 * Bilingual one-line summary for the bandwidth saved by a batch.
 * @param {number} savedBytes
 * @param {string} locale
 * @returns {string}
 */
export function formatSavedSummary(savedBytes, locale) {
    const mb = (savedBytes / (1024 * 1024)).toFixed(1);
    return locale.startsWith('es') ? `Deduplicación: ${mb} MB no necesitaron subirse` : `Deduplication: ${mb} MB didn't need uploading`;
}
