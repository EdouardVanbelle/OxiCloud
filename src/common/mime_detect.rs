//! MIME type detection using magic bytes (infer) + extension fallback (mime_guess).
//!
//! Priority order:
//! 1. If the claimed Content-Type is specific (not `application/octet-stream`), trust it.
//! 2. Read first bytes of the file and detect via magic bytes (`infer` crate).
//! 3. Fall back to extension-based detection (`mime_guess`).
//! 4. If nothing matches, return the original claimed type.
//!
//! Performance: < 1µs for the `infer` check (reads only header bytes, no allocation).

use std::path::Path;
use tokio::io::AsyncReadExt;

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

/// Detect the `Content-Type` to serve for an already-encoded thumbnail.
///
/// The slow encode path re-encodes to JPEG, but the fast path stores the
/// source image as-is (PNG / GIF / WebP), so the handler must not blindly
/// claim `image/jpeg`. Detects the real format from magic bytes, defaulting
/// to `image/jpeg` (the slow-path output) when detection is inconclusive.
pub fn thumbnail_content_type(data: &[u8]) -> &'static str {
    infer::get(data)
        .map(|kind| kind.mime_type())
        .filter(|mime| mime.starts_with("image/"))
        .unwrap_or("image/jpeg")
}

/// Async helper: reads the first bytes of a file on disk and refines the MIME type.
///
/// Designed for the upload path where the file has been spooled to a temp path.
pub async fn refine_content_type_from_file(
    temp_path: &Path,
    filename: &str,
    claimed: &str,
) -> String {
    // Fast path: if the client gave us a specific type, trust it
    if !claimed.is_empty()
        && claimed != "application/octet-stream"
        && claimed != "binary/octet-stream"
    {
        return claimed.to_string();
    }

    // Read only the first bytes needed for magic detection (not the whole file).
    match tokio::fs::File::open(temp_path).await {
        Ok(mut file) => {
            let mut buf = vec![0u8; MAGIC_BYTES_LEN];
            let n = file.read(&mut buf).await.unwrap_or(0);
            refine_content_type(&buf[..n], filename, claimed)
        }
        Err(e) => {
            tracing::warn!(
                "MIME detection: failed to read {} for magic bytes: {}",
                temp_path.display(),
                e
            );
            // Fall back to extension
            let guess = mime_guess::from_path(filename);
            if let Some(mime) = guess.first() {
                return mime.to_string();
            }
            claimed.to_string()
        }
    }
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
    fn thumbnail_content_type_detects_real_format() {
        // Fast-path thumbnails keep the source format — serve it accurately.
        let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";
        assert_eq!(thumbnail_content_type(png), "image/png");

        let jpeg = b"\xff\xd8\xff\xe0\x00\x10JFIF";
        assert_eq!(thumbnail_content_type(jpeg), "image/jpeg");

        // Inconclusive bytes default to JPEG (the slow-path encoder output).
        assert_eq!(thumbnail_content_type(b"garbage"), "image/jpeg");
        assert_eq!(thumbnail_content_type(b""), "image/jpeg");
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
