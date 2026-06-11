/**
 * Incremental BLAKE3 hasher.
 *
 * ```js
 * const h = new Blake3Hasher();
 * h.update(chunkBytes);   // repeat per slice
 * const hex = h.finalizeHex();
 * ```
 */
export class Blake3Hasher {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        Blake3HasherFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_blake3hasher_free(ptr, 0);
    }
    /**
     * Bytes hashed so far — lets the worker report progress without
     * tracking its own counter.
     * @returns {number}
     */
    count() {
        const ret = wasm.blake3hasher_count(this.__wbg_ptr);
        return ret;
    }
    /**
     * Finish and return the lowercase hex digest (64 chars). The hasher
     * can keep receiving `update` calls afterwards (BLAKE3 finalization
     * is non-destructive), but the frontend treats it as terminal.
     * @returns {string}
     */
    finalizeHex() {
        let deferred1_0;
        let deferred1_1;
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.blake3hasher_finalizeHex(retptr, this.__wbg_ptr);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            deferred1_0 = r0;
            deferred1_1 = r1;
            return getStringFromWasm0(r0, r1);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
            wasm.__wbindgen_export2(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Create a fresh hasher.
     */
    constructor() {
        const ret = wasm.blake3hasher_new();
        this.__wbg_ptr = ret;
        Blake3HasherFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * Feed one slice of the file.
     * @param {Uint8Array} data
     */
    update(data) {
        const ptr0 = passArray8ToWasm0(data, wasm.__wbindgen_export);
        const len0 = WASM_VECTOR_LEN;
        wasm.blake3hasher_update(this.__wbg_ptr, ptr0, len0);
    }
}
if (Symbol.dispose) Blake3Hasher.prototype[Symbol.dispose] = Blake3Hasher.prototype.free;

/**
 * Incremental FastCDC chunker + whole-file BLAKE3, for the delta-upload
 * worker. Feed the file in slices; every call returns the chunks that
 * became FINAL; `finish()` flushes the tail and returns the file hash.
 *
 * ```js
 * const c = new DeltaChunker();
 * for (const slice of slices) {
 *     for (const [h, s] of JSON.parse(c.update(bytes))) { … }
 * }
 * const { chunks, file_hash } = JSON.parse(c.finish());
 * ```
 *
 * Correctness of the incremental split: FastCDC decides each cut by
 * scanning at most `CDC_MAX_CHUNK` bytes from the chunk's start. When
 * the chunker runs over the buffered prefix of a longer file, every
 * produced chunk except the LAST ended on a content/max-size condition
 * — its decision window was fully available, so the full-file chunker
 * makes the same cut. Only the last chunk (cut by "end of buffer") is
 * provisional: it stays buffered and is re-examined when more bytes
 * arrive. By induction the emitted boundaries equal a single FastCDC
 * pass over the whole file — the mirror test below proves it.
 */
export class DeltaChunker {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        DeltaChunkerFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_deltachunker_free(ptr, 0);
    }
    /**
     * Flush the provisional tail and return
     * `{"chunks":[["<hex>",size]…],"file_hash":"<hex>","total":N}`.
     * `chunks` holds at most one entry (the tail); an empty file has none
     * and its `file_hash` is BLAKE3 of the empty input.
     * @returns {string}
     */
    finish() {
        let deferred1_0;
        let deferred1_1;
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.deltachunker_finish(retptr, this.__wbg_ptr);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            deferred1_0 = r0;
            deferred1_1 = r1;
            return getStringFromWasm0(r0, r1);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
            wasm.__wbindgen_export2(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Create a chunker with the server's CDC parameters.
     */
    constructor() {
        const ret = wasm.deltachunker_new();
        this.__wbg_ptr = ret;
        DeltaChunkerFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * Feed one slice. Returns a JSON array of the chunks that became
     * final: `[["<blake3-hex>", size], …]` (possibly empty).
     * @param {Uint8Array} data
     * @returns {string}
     */
    update(data) {
        let deferred2_0;
        let deferred2_1;
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            const ptr0 = passArray8ToWasm0(data, wasm.__wbindgen_export);
            const len0 = WASM_VECTOR_LEN;
            wasm.deltachunker_update(retptr, this.__wbg_ptr, ptr0, len0);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            deferred2_0 = r0;
            deferred2_1 = r1;
            return getStringFromWasm0(r0, r1);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
            wasm.__wbindgen_export2(deferred2_0, deferred2_1, 1);
        }
    }
}
if (Symbol.dispose) DeltaChunker.prototype[Symbol.dispose] = DeltaChunker.prototype.free;

/**
 * One-shot convenience for small buffers.
 * @param {Uint8Array} data
 * @returns {string}
 */
export function blake3Hex(data) {
    let deferred2_0;
    let deferred2_1;
    try {
        const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
        const ptr0 = passArray8ToWasm0(data, wasm.__wbindgen_export);
        const len0 = WASM_VECTOR_LEN;
        wasm.blake3Hex(retptr, ptr0, len0);
        var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
        var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
        deferred2_0 = r0;
        deferred2_1 = r1;
        return getStringFromWasm0(r0, r1);
    } finally {
        wasm.__wbindgen_add_to_stack_pointer(16);
        wasm.__wbindgen_export2(deferred2_0, deferred2_1, 1);
    }
}
function __wbg_get_imports() {
    const import0 = {
        __proto__: null,
        __wbg___wbindgen_throw_bbadd78c1bac3a77: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
    };
    return {
        __proto__: null,
        "./oxicloud_hash_wasm_bg.js": import0,
    };
}

const Blake3HasherFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_blake3hasher_free(ptr, 1));
const DeltaChunkerFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_deltachunker_free(ptr, 1));

let cachedDataViewMemory0 = null;
function getDataViewMemory0() {
    if (cachedDataViewMemory0 === null || cachedDataViewMemory0.buffer.detached === true || (cachedDataViewMemory0.buffer.detached === undefined && cachedDataViewMemory0.buffer !== wasm.memory.buffer)) {
        cachedDataViewMemory0 = new DataView(wasm.memory.buffer);
    }
    return cachedDataViewMemory0;
}

function getStringFromWasm0(ptr, len) {
    return decodeText(ptr >>> 0, len);
}

let cachedUint8ArrayMemory0 = null;
function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
}

function passArray8ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 1, 1) >>> 0;
    getUint8ArrayMemory0().set(arg, ptr / 1);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
cachedTextDecoder.decode();
const MAX_SAFARI_DECODE_BYTES = 2146435072;
let numBytesDecoded = 0;
function decodeText(ptr, len) {
    numBytesDecoded += len;
    if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
        cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
        cachedTextDecoder.decode();
        numBytesDecoded = len;
    }
    return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
}

let WASM_VECTOR_LEN = 0;

let wasmModule, wasmInstance, wasm;
function __wbg_finalize_init(instance, module) {
    wasmInstance = instance;
    wasm = instance.exports;
    wasmModule = module;
    cachedDataViewMemory0 = null;
    cachedUint8ArrayMemory0 = null;
    return wasm;
}

async function __wbg_load(module, imports) {
    if (typeof Response === 'function' && module instanceof Response) {
        if (typeof WebAssembly.instantiateStreaming === 'function') {
            try {
                return await WebAssembly.instantiateStreaming(module, imports);
            } catch (e) {
                const validResponse = module.ok && expectedResponseType(module.type);

                if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                    console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                } else { throw e; }
            }
        }

        const bytes = await module.arrayBuffer();
        return await WebAssembly.instantiate(bytes, imports);
    } else {
        const instance = await WebAssembly.instantiate(module, imports);

        if (instance instanceof WebAssembly.Instance) {
            return { instance, module };
        } else {
            return instance;
        }
    }

    function expectedResponseType(type) {
        switch (type) {
            case 'basic': case 'cors': case 'default': return true;
        }
        return false;
    }
}

function initSync(module) {
    if (wasm !== undefined) return wasm;


    if (module !== undefined) {
        if (Object.getPrototypeOf(module) === Object.prototype) {
            ({module} = module)
        } else {
            console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
        }
    }

    const imports = __wbg_get_imports();
    if (!(module instanceof WebAssembly.Module)) {
        module = new WebAssembly.Module(module);
    }
    const instance = new WebAssembly.Instance(module, imports);
    return __wbg_finalize_init(instance, module);
}

async function __wbg_init(module_or_path) {
    if (wasm !== undefined) return wasm;


    if (module_or_path !== undefined) {
        if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
            ({module_or_path} = module_or_path)
        } else {
            console.warn('using deprecated parameters for the initialization function; pass a single object instead')
        }
    }

    if (module_or_path === undefined) {
        module_or_path = new URL('oxicloud_hash_wasm_bg.wasm', import.meta.url);
    }
    const imports = __wbg_get_imports();

    if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
        module_or_path = fetch(module_or_path);
    }

    const { instance, module } = await __wbg_load(await module_or_path, imports);

    return __wbg_finalize_init(instance, module);
}

export { initSync, __wbg_init as default };
