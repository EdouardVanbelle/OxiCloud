//! MIME type detection using magic bytes (infer) + extension fallback (mime_guess).
//!
//! Priority order:
//! 1. If the claimed Content-Type is specific (not `application/octet-stream`), trust it.
//! 2. Read first bytes of the file and detect via magic bytes (`infer` crate).
//! 3. Fall back to extension-based detection (`mime_guess`).
//! 4. If nothing matches, return the original claimed type.
//!
//! Performance: < 1µs for the `infer` check (reads only header bytes, no allocation).

/// Maximum bytes needed for magic-byte detection. Upload ingestion peeks
/// this many bytes off the stream before forwarding them unchanged.
pub const MAGIC_BYTES_LEN: usize = 8192;

/// Extract the filename component from a `/`-separated path.
pub fn filename_from_path(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Whether a claimed Content-Type is too generic to trust — these trigger
/// magic-byte detection on the upload path.
pub fn is_generic_mime(claimed: &str) -> bool {
    claimed.is_empty() || claimed == "application/octet-stream" || claimed == "binary/octet-stream"
}

/// Refine a claimed MIME type using magic bytes and filename extension.
///
/// This is a synchronous function — the caller should already have the first
/// bytes of the content available (upload ingestion peeks them in-flight).
///
/// # Arguments
/// * `buf` — first bytes of the file (at least 8192 for best results)
/// * `filename` — original filename (used for extension fallback)
/// * `claimed` — the Content-Type sent by the client
pub fn refine_content_type(buf: &[u8], filename: &str, claimed: &str) -> String {
    // If the client sent a specific type (not generic), trust it
    if !is_generic_mime(claimed) {
        return claimed.to_string();
    }

    // 1. Try magic bytes detection
    if let Some(kind) = infer::get(buf) {
        return kind.mime_type().to_string();
    }

    // 2. Try extension-based detection
    let guess = mime_guess::from_path(filename);
    if let Some(mime) = guess.first() {
        return mime.to_string();
    }

    // 3. Fall back to claimed type
    claimed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── refine_content_type (sync) ──────────────────────────────

    #[test]
    fn specific_claimed_type_is_trusted() {
        let result = refine_content_type(b"garbage", "file.txt", "image/png");
        assert_eq!(result, "image/png");
    }

    #[test]
    fn octet_stream_triggers_magic_detection_png() {
        // PNG magic bytes
        let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";
        let result = refine_content_type(png, "noext", "application/octet-stream");
        assert_eq!(result, "image/png");
    }

    #[test]
    fn octet_stream_triggers_magic_detection_jpeg() {
        let jpeg = b"\xff\xd8\xff\xe0\x00\x10JFIF";
        let result = refine_content_type(jpeg, "noext", "application/octet-stream");
        assert_eq!(result, "image/jpeg");
    }

    #[test]
    fn binary_octet_stream_also_triggers_detection() {
        let jpeg = b"\xff\xd8\xff\xe0\x00\x10JFIF";
        let result = refine_content_type(jpeg, "noext", "binary/octet-stream");
        assert_eq!(result, "image/jpeg");
    }

    #[test]
    fn extension_fallback_when_no_magic_match() {
        let result = refine_content_type(b"plain text", "style.css", "application/octet-stream");
        assert_eq!(result, "text/css");
    }

    #[test]
    fn falls_back_to_claimed_when_nothing_matches() {
        let result = refine_content_type(b"unknown stuff", "noext", "application/octet-stream");
        assert_eq!(result, "application/octet-stream");
    }

    #[test]
    fn empty_claimed_triggers_detection() {
        let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";
        let result = refine_content_type(png, "photo.png", "");
        assert_eq!(result, "image/png");
    }

    // ── is_generic_mime ─────────────────────────────────────────

    #[test]
    fn generic_mime_detection() {
        assert!(is_generic_mime(""));
        assert!(is_generic_mime("application/octet-stream"));
        assert!(is_generic_mime("binary/octet-stream"));
        assert!(!is_generic_mime("image/png"));
        assert!(!is_generic_mime("text/plain"));
    }

    // ── filename_from_path ──────────────────────────────────────

    #[test]
    fn extracts_filename_from_deep_path() {
        assert_eq!(filename_from_path("a/b/c/photo.jpg"), "photo.jpg");
    }

    #[test]
    fn returns_input_when_no_slash() {
        assert_eq!(filename_from_path("photo.jpg"), "photo.jpg");
    }

    #[test]
    fn handles_trailing_slash() {
        assert_eq!(filename_from_path("a/b/"), "");
    }
}
