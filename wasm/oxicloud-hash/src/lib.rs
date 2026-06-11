//! BLAKE3 for the OxiCloud web frontend.
//!
//! Compiled from the same `blake3` crate the server uses, so a hash
//! computed in the browser equals the server's content address bit for
//! bit — the property the instant-upload path depends on.
//!
//! The API is incremental on purpose: the worker feeds the file in
//! slices (`Blob.slice().arrayBuffer()`), keeping RAM constant no matter
//! how large the file is.

use wasm_bindgen::prelude::*;

/// Incremental BLAKE3 hasher.
///
/// ```js
/// const h = new Blake3Hasher();
/// h.update(chunkBytes);   // repeat per slice
/// const hex = h.finalizeHex();
/// ```
#[wasm_bindgen]
pub struct Blake3Hasher {
    inner: blake3::Hasher,
}

#[wasm_bindgen]
impl Blake3Hasher {
    /// Create a fresh hasher.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Blake3Hasher {
        Blake3Hasher {
            inner: blake3::Hasher::new(),
        }
    }

    /// Feed one slice of the file.
    pub fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    /// Finish and return the lowercase hex digest (64 chars). The hasher
    /// can keep receiving `update` calls afterwards (BLAKE3 finalization
    /// is non-destructive), but the frontend treats it as terminal.
    #[wasm_bindgen(js_name = finalizeHex)]
    pub fn finalize_hex(&self) -> String {
        self.inner.finalize().to_hex().to_string()
    }

    /// Bytes hashed so far — lets the worker report progress without
    /// tracking its own counter.
    pub fn count(&self) -> f64 {
        self.inner.count() as f64
    }
}

impl Default for Blake3Hasher {
    fn default() -> Self {
        Self::new()
    }
}

/// One-shot convenience for small buffers.
#[wasm_bindgen(js_name = blake3Hex)]
pub fn blake3_hex(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}

// ── Delta-upload chunker ─────────────────────────────────────────────────────

/// CDC parameters — MUST mirror `dedup_service.rs` on the server
/// (`CDC_MIN_CHUNK` / `CDC_AVG_CHUNK` / `CDC_MAX_CHUNK`). Identical
/// parameters + identical crate ⇒ identical boundaries, which is what
/// makes a chunk hashed in the browser deduplicate against a chunk the
/// server cut from a byte upload.
const CDC_MIN_CHUNK: usize = 65_536;
const CDC_AVG_CHUNK: usize = 262_144;
const CDC_MAX_CHUNK: usize = 1_048_576;

/// Incremental FastCDC chunker + whole-file BLAKE3, for the delta-upload
/// worker. Feed the file in slices; every call returns the chunks that
/// became FINAL; `finish()` flushes the tail and returns the file hash.
///
/// ```js
/// const c = new DeltaChunker();
/// for (const slice of slices) {
///     for (const [h, s] of JSON.parse(c.update(bytes))) { … }
/// }
/// const { chunks, file_hash } = JSON.parse(c.finish());
/// ```
///
/// Correctness of the incremental split: FastCDC decides each cut by
/// scanning at most `CDC_MAX_CHUNK` bytes from the chunk's start. When
/// the chunker runs over the buffered prefix of a longer file, every
/// produced chunk except the LAST ended on a content/max-size condition
/// — its decision window was fully available, so the full-file chunker
/// makes the same cut. Only the last chunk (cut by "end of buffer") is
/// provisional: it stays buffered and is re-examined when more bytes
/// arrive. By induction the emitted boundaries equal a single FastCDC
/// pass over the whole file — the mirror test below proves it.
#[wasm_bindgen]
pub struct DeltaChunker {
    /// Provisional tail: bytes after the last FINAL cut.
    buf: Vec<u8>,
    file_hasher: blake3::Hasher,
    total: u64,
}

/// Append one `["<hex>",len]` item to a hand-rolled JSON array — hashes
/// are hex and sizes are integers, so manual JSON is unambiguous and
/// keeps a serde dependency out of the wasm binary.
fn push_chunk_json(out: &mut String, hash: &str, len: usize) {
    if !out.ends_with('[') {
        out.push(',');
    }
    out.push_str("[\"");
    out.push_str(hash);
    out.push_str("\",");
    out.push_str(&len.to_string());
    out.push(']');
}

#[wasm_bindgen]
impl DeltaChunker {
    /// Create a chunker with the server's CDC parameters.
    #[wasm_bindgen(constructor)]
    pub fn new() -> DeltaChunker {
        DeltaChunker {
            buf: Vec::with_capacity(2 * CDC_MAX_CHUNK),
            file_hasher: blake3::Hasher::new(),
            total: 0,
        }
    }

    /// Feed one slice. Returns a JSON array of the chunks that became
    /// final: `[["<blake3-hex>", size], …]` (possibly empty).
    pub fn update(&mut self, data: &[u8]) -> String {
        self.file_hasher.update(data);
        self.total += data.len() as u64;
        self.buf.extend_from_slice(data);

        let mut out = String::from("[");
        let mut consumed = 0usize;
        {
            let chunks: Vec<fastcdc::v2020::Chunk> = fastcdc::v2020::FastCDC::new(
                &self.buf,
                CDC_MIN_CHUNK,
                CDC_AVG_CHUNK,
                CDC_MAX_CHUNK,
            )
            .collect();
            // Every chunk but the last ended on a content/max condition →
            // final. The last one ended because the buffer did → keep it.
            for chunk in chunks.iter().take(chunks.len().saturating_sub(1)) {
                let bytes = &self.buf[chunk.offset..chunk.offset + chunk.length];
                push_chunk_json(
                    &mut out,
                    &blake3::hash(bytes).to_hex().to_string(),
                    chunk.length,
                );
                consumed = chunk.offset + chunk.length;
            }
        }
        if consumed > 0 {
            self.buf.drain(..consumed);
        }
        out.push(']');
        out
    }

    /// Flush the provisional tail and return
    /// `{"chunks":[["<hex>",size]…],"file_hash":"<hex>","total":N}`.
    /// `chunks` holds at most one entry (the tail); an empty file has none
    /// and its `file_hash` is BLAKE3 of the empty input.
    pub fn finish(&mut self) -> String {
        let mut out = String::from("{\"chunks\":[");
        if !self.buf.is_empty() {
            push_chunk_json(
                &mut out,
                &blake3::hash(&self.buf).to_hex().to_string(),
                self.buf.len(),
            );
            self.buf.clear();
        }
        out.push_str("],\"file_hash\":\"");
        out.push_str(&self.file_hasher.finalize().to_hex().to_string());
        out.push_str("\",\"total\":");
        out.push_str(&self.total.to_string());
        out.push('}');
        out
    }
}

impl Default for DeltaChunker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vector the frontend smoke test uses — also proves the wasm
    /// build hashes identically to the server (same crate, same output).
    #[test]
    fn hello_world_vector() {
        let hasher = {
            let mut h = Blake3Hasher::new();
            h.update(b"Hello, ");
            h.update(b"World!");
            h
        };
        assert_eq!(
            hasher.finalize_hex(),
            "288a86a79f20a3d6dccdca7713beaed178798296bdfa7913fa2a62d9727bf8f8"
        );
        assert_eq!(
            blake3_hex(b"Hello, World!"),
            "288a86a79f20a3d6dccdca7713beaed178798296bdfa7913fa2a62d9727bf8f8"
        );
    }

    #[test]
    fn empty_input_vector() {
        assert_eq!(
            blake3_hex(b""),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
    }

    // ── DeltaChunker mirror test ─────────────────────────────────
    //
    // The client-side twin of the server's
    // `test_stream_chunking_matches_slice_chunking`: incremental chunking
    // with adversarial slice sizes must produce exactly the boundaries of
    // one FastCDC pass over the whole buffer — the property cross-version
    // dedup between byte uploads and delta uploads hangs on.

    fn run_chunker(data: &[u8], slice: usize) -> (Vec<(String, usize)>, String) {
        let mut chunker = DeltaChunker::new();
        let mut chunks: Vec<(String, usize)> = Vec::new();
        let mut parse = |json: &str, into: &mut Vec<(String, usize)>| {
            // items look like ["<hex>",N] — split on '[' groups.
            for item in json.split("[\"").skip(1) {
                let hash = &item[..64];
                let size: usize = item[66..item.find(']').unwrap()].parse().unwrap();
                into.push((hash.to_string(), size));
            }
        };
        for piece in data.chunks(slice.max(1)) {
            let emitted = chunker.update(piece);
            parse(&emitted, &mut chunks);
        }
        let fin = chunker.finish();
        let tail_json = &fin[fin.find('[').unwrap()..=fin.find(']').unwrap()];
        parse(tail_json, &mut chunks);
        let file_hash = fin.split("\"file_hash\":\"").nth(1).unwrap()[..64].to_string();
        (chunks, file_hash)
    }

    #[test]
    fn incremental_chunking_matches_single_pass() {
        // 4 MiB of xorshift noise — genuinely content-defined cut points
        // (a byte-periodic generator would only ever hit max-size cuts).
        let mut state: u64 = 0x243F_6A88_85A3_08D3;
        let mut data = Vec::with_capacity(4 * 1024 * 1024);
        while data.len() < 4 * 1024 * 1024 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            data.extend_from_slice(&state.to_le_bytes());
        }

        let reference: Vec<(String, usize)> =
            fastcdc::v2020::FastCDC::new(&data, CDC_MIN_CHUNK, CDC_AVG_CHUNK, CDC_MAX_CHUNK)
                .map(|c| {
                    (
                        blake3::hash(&data[c.offset..c.offset + c.length])
                            .to_hex()
                            .to_string(),
                        c.length,
                    )
                })
                .collect();
        assert!(reference.len() > 4, "test data must span several chunks");

        // Slice sizes chosen to stress every refill path: tiny (7 B),
        // typical worker slice (8 MiB > file), page-ish, and exactly the
        // CDC max so provisional tails land on boundaries.
        for slice in [7usize, 4096, CDC_MAX_CHUNK, 8 * 1024 * 1024] {
            let (chunks, file_hash) = run_chunker(&data, slice);
            assert_eq!(
                chunks, reference,
                "boundaries must not depend on slicing (slice={slice})"
            );
            assert_eq!(
                file_hash,
                blake3_hex(&data),
                "file hash must match one-shot BLAKE3 (slice={slice})"
            );
        }
    }

    #[test]
    fn delta_chunker_empty_and_tiny_inputs() {
        let (chunks, file_hash) = run_chunker(b"", 1024);
        assert!(chunks.is_empty());
        assert_eq!(
            file_hash,
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );

        let tiny = b"below the CDC minimum";
        let (chunks, file_hash) = run_chunker(tiny, 4);
        assert_eq!(chunks.len(), 1, "tiny input is one (tail) chunk");
        assert_eq!(chunks[0].1, tiny.len());
        assert_eq!(chunks[0].0, blake3_hex(tiny));
        assert_eq!(file_hash, blake3_hex(tiny));
    }
}
