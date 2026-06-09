//! StoragePath - Domain Value Object for representing storage paths
//!
//! This module contains only the StoragePath Value Object which is part of the pure domain.
//! PathService (which implements StoragePort and StorageMediator) was moved to
//! infrastructure/services/path_service.rs because it has file system dependencies.

use std::path::PathBuf;
use unicode_normalization::UnicodeNormalization;

/// NFC-normalize a single file or folder name component.
///
/// The storage layer (PostgreSQL `storage.files.name` and
/// `storage.folders.name`) compares bytes literally — there is no
/// Unicode-aware collation in either UNIQUE index. macOS APFS stores
/// filenames in NFD (decomposed: `é` = `e` + U+0301), while browsers
/// and most other clients post NFC (`é` = U+00E9). Without
/// normalization, the same logical filename can land as two distinct
/// rows: one from a web upload, one from a NextCloud desktop client
/// re-upload of the round-tripped name. The UNIQUE index does not
/// catch it because the bytes differ.
///
/// This function is called at every name-receiving boundary (entity
/// constructors, repository path lookups) so the database invariant
/// becomes "every stored name is NFC". A one-shot migration
/// (`migrate-nfc-filenames`) cleans up rows that pre-date this rule.
///
/// Pure function — no I/O, allocates one `String`.
pub fn normalize_storage_name(name: &str) -> String {
    name.nfc().collect()
}

/// Validates a single file or folder name component.
///
/// Returns `Err` with a human-readable reason if the name is rejected.
/// Callers should wrap the reason into their own error type.
pub fn validate_storage_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("name cannot be empty");
    }
    if name.contains('/') || name.contains('\\') {
        return Err("name must not contain '/' or '\\'");
    }
    if name.contains('\0') {
        return Err("name must not contain null bytes");
    }
    if name == "." || name == ".." {
        return Err("'.' and '..' are not valid names");
    }
    Ok(())
}

/// Represents a storage path in the domain (Value Object)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StoragePath {
    segments: Vec<String>,
}

impl StoragePath {
    /// Checks whether a single segment is safe (no traversal, no slashes)
    fn is_safe_segment(s: &str) -> bool {
        !s.is_empty() && s != "." && s != ".." && !s.contains('/')
    }

    /// Creates a new storage path, silently dropping any traversal segments
    pub fn new(segments: Vec<String>) -> Self {
        Self {
            segments: segments
                .into_iter()
                .filter(|s| Self::is_safe_segment(s))
                .collect(),
        }
    }

    /// Creates an empty path (root)
    pub fn root() -> Self {
        Self {
            segments: Vec::new(),
        }
    }

    /// Creates a path from a string with segments separated by /
    ///
    /// Traversal segments (`.`, `..`) are silently stripped to prevent
    /// path-traversal attacks.
    pub fn from_string(path: &str) -> Self {
        let segments = path
            .split('/')
            .filter(|s| Self::is_safe_segment(s))
            .map(|s| s.to_string())
            .collect();
        Self { segments }
    }

    /// Creates a path from a PathBuf
    pub fn from(path_buf: PathBuf) -> Self {
        let segments = path_buf
            .components()
            .filter_map(|c| match c {
                std::path::Component::Normal(os_str) => Some(os_str.to_string_lossy().to_string()),
                _ => None,
            })
            .collect();
        Self { segments }
    }

    /// Appends a segment to the path, consuming `self` so the existing
    /// segment buffer is reused instead of deep-cloned.
    ///
    /// Traversal segments (`.`, `..`) and segments containing `/` are
    /// silently ignored to prevent path-traversal attacks.
    pub fn join(mut self, segment: &str) -> Self {
        if Self::is_safe_segment(segment) {
            self.segments.push(segment.to_string());
        }
        self
    }

    /// Gets the file name (last segment)
    pub fn file_name(&self) -> Option<String> {
        self.segments.last().cloned()
    }

    /// Gets the parent directory path
    pub fn parent(&self) -> Option<Self> {
        if self.segments.is_empty() {
            None
        } else {
            let parent_segments = self.segments[..self.segments.len() - 1].to_vec();
            Some(Self {
                segments: parent_segments,
            })
        }
    }

    /// Checks if the path is empty (is the root)
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }
}

impl std::fmt::Display for StoragePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.segments.is_empty() {
            write!(f, "/")
        } else {
            write!(f, "/{}", self.segments.join("/"))
        }
    }
}

impl StoragePath {
    /// Returns the path representation as a string
    pub fn as_str(&self) -> &str {
        // Note: The implementation should really store the string,
        // but here we do a temporary implementation that always returns "/"
        // This is only used for the get_folder_path_str implementation
        "/"
    }

    /// Gets the path segments
    pub fn segments(&self) -> &[String] {
        &self.segments
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_path_from_string() {
        let path = StoragePath::from_string("folder/subfolder/file.txt");
        assert_eq!(path.segments(), &["folder", "subfolder", "file.txt"]);
        assert_eq!(path.to_string(), "/folder/subfolder/file.txt");
    }

    #[test]
    fn test_storage_path_join() {
        let path = StoragePath::from_string("folder");
        let joined = path.join("file.txt");
        assert_eq!(joined.to_string(), "/folder/file.txt");
    }

    #[test]
    fn test_storage_path_parent() {
        let path = StoragePath::from_string("folder/file.txt");
        let parent = path.parent().unwrap();
        assert_eq!(parent.to_string(), "/folder");
    }

    #[test]
    fn test_storage_path_root() {
        let root = StoragePath::root();
        assert!(root.is_empty());
        assert_eq!(root.to_string(), "/");
    }

    #[test]
    fn test_storage_path_file_name() {
        let path = StoragePath::from_string("folder/file.txt");
        assert_eq!(path.file_name(), Some("file.txt".to_string()));
    }

    // ── Path-traversal hardening tests (VULN-02) ──────────────

    #[test]
    fn test_from_string_strips_dot_dot() {
        let path = StoragePath::from_string("../../etc/passwd");
        assert_eq!(path.segments(), &["etc", "passwd"]);
    }

    #[test]
    fn test_from_string_strips_single_dot() {
        let path = StoragePath::from_string("folder/./file.txt");
        assert_eq!(path.segments(), &["folder", "file.txt"]);
    }

    #[test]
    fn test_from_string_strips_mixed_traversal() {
        let path = StoragePath::from_string("a/../b/./c/../../d");
        assert_eq!(path.segments(), &["a", "b", "c", "d"]);
    }

    #[test]
    fn test_from_string_all_traversal_yields_root() {
        let path = StoragePath::from_string("../../..");
        assert!(path.is_empty());
        assert_eq!(path.to_string(), "/");
    }

    #[test]
    fn test_new_strips_traversal_segments() {
        let path = StoragePath::new(vec!["..".into(), "etc".into(), ".".into(), "passwd".into()]);
        assert_eq!(path.segments(), &["etc", "passwd"]);
    }

    #[test]
    fn test_new_strips_empty_segments() {
        let path = StoragePath::new(vec!["a".into(), "".into(), "b".into()]);
        assert_eq!(path.segments(), &["a", "b"]);
    }

    #[test]
    fn test_join_rejects_dot_dot() {
        let base = StoragePath::from_string("folder");
        let joined = base.join("..");
        // ".." is silently ignored — path stays unchanged
        assert_eq!(joined.segments(), &["folder"]);
    }

    #[test]
    fn test_join_rejects_single_dot() {
        let base = StoragePath::from_string("folder");
        let joined = base.join(".");
        assert_eq!(joined.segments(), &["folder"]);
    }

    #[test]
    fn test_join_rejects_slash_in_segment() {
        let base = StoragePath::from_string("folder");
        let joined = base.join("sub/../../etc/passwd");
        // Segment contains '/' → silently ignored
        assert_eq!(joined.segments(), &["folder"]);
    }

    #[test]
    fn test_from_pathbuf_strips_traversal() {
        let path = StoragePath::from(PathBuf::from("a/../b/./c"));
        // PathBuf Component::Normal only yields the normal parts
        // On most platforms this strips . and ..
        // but regardless, our from() only accepts Component::Normal
        assert!(!path.segments().contains(&"..".to_string()));
        assert!(!path.segments().contains(&".".to_string()));
    }

    // ── NFC normalization tests ─────────────────────────────────

    /// Plain ASCII names must round-trip identical bytes.
    #[test]
    fn test_normalize_ascii_unchanged() {
        assert_eq!(normalize_storage_name("file.txt"), "file.txt");
        assert_eq!(normalize_storage_name("My Documents"), "My Documents");
    }

    /// The macOS APFS / NextCloud-desktop pathological case: `é`
    /// decomposed as `e` + combining acute (U+0301). Stored bytes
    /// `65 cc 81` collapse to NFC `c3 a9`.
    #[test]
    fn test_normalize_nfd_to_nfc() {
        let nfd = "caf\u{0065}\u{0301}";
        let nfc = "caf\u{00E9}";
        assert_ne!(nfd.as_bytes(), nfc.as_bytes());
        assert_eq!(normalize_storage_name(nfd), nfc);
    }

    /// Already-NFC input must round-trip unchanged. This is the
    /// idempotence property the boundary normalization relies on.
    #[test]
    fn test_normalize_nfc_idempotent() {
        let nfc = "Capture d\u{2019}\u{00E9}cran.png";
        assert_eq!(normalize_storage_name(nfc), nfc);
        // And applying twice is the same as once.
        assert_eq!(normalize_storage_name(&normalize_storage_name(nfc)), nfc);
    }

    /// Multi-codepoint NFD sequences (combining acute + grave +
    /// typographic apostrophe) all converge to a single NFC form.
    #[test]
    fn test_normalize_mixed_accents() {
        let nfd = "Capture d\u{2019}\u{0065}\u{0301}cran a\u{0300}.png";
        let nfc = "Capture d\u{2019}\u{00E9}cran \u{00E0}.png";
        assert_eq!(normalize_storage_name(nfd), nfc);
    }
}
