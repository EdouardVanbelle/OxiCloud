//! Locale newtype and registry.
//!
//! Replaces the closed `enum Locale { English, Spanish, … }` that used
//! to live in `domain::services::i18n_service`. Locales are now a
//! string-backed newtype validated at construction against a
//! [`LocaleRegistry`] that is built **once at startup** by listing the
//! files under `static/locales/*.json`.
//!
//! Adding a 17th locale is a JSON-file-drop: no Rust patch, no
//! re-compile. The trade-off is that all locale matching is done by
//! exact string compare against a hash-set; tag negotiation (matching
//! `fr-FR` to a registry containing only `fr`) is the [extractor]'s
//! responsibility, not this type's.
//!
//! Construction always goes through the registry to guarantee an
//! unknown code can never end up in a `Locale` value — fallback to the
//! server default happens at parse time, not at use time. That keeps
//! every consumer dumb: if you have a `Locale`, the underlying string
//! is known good.
//!
//! [extractor]: crate::interfaces::middleware::locale

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

/// A validated locale code (e.g. `"en"`, `"fr"`, `"zh-TW"`).
///
/// Construction goes through [`LocaleRegistry`] so the contained string
/// is always present in `static/locales/`. Two locales compare equal
/// iff their canonical codes are equal — case-insensitively normalised
/// at registry-build time (see [`LocaleRegistry::canonicalise`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Locale(SmolStr);

impl Locale {
    /// The canonical English locale. Used as the universal fallback.
    /// Safe to call without a registry: every install is required to
    /// ship `static/locales/en.json`, and the canonical form is fixed
    /// at `"en"`.
    pub fn english() -> Self {
        Self(SmolStr::new_static("en"))
    }

    /// Borrow the underlying canonical code, e.g. `"en"`, `"zh-TW"`.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// True iff this is `Locale::english()`.
    pub fn is_english(&self) -> bool {
        self.0.as_str() == "en"
    }

    /// Format-only parse: accepts strings that look like RFC 5646
    /// language tags (`fr`, `en-US`, `zh-TW`), returns the
    /// canonicalised newtype. **Does not check the registry** — the
    /// result may not be a locale this server has translations for.
    /// Callers that need that guarantee should use
    /// [`LocaleRegistry::parse`] instead.
    ///
    /// Returns `None` for empty input, non-ASCII characters, or
    /// shapes outside `^[A-Za-z]{2,3}(-[A-Za-z0-9]{2,8})*$`.
    pub fn from_code(code: &str) -> Option<Self> {
        if code.is_empty() || code.len() > 35 {
            return None;
        }
        let mut parts = code.split('-');
        let primary = parts.next()?;
        if !(2..=3).contains(&primary.len()) || !primary.chars().all(|c| c.is_ascii_alphabetic()) {
            return None;
        }
        for sub in parts {
            if !(2..=8).contains(&sub.len()) || !sub.chars().all(|c| c.is_ascii_alphanumeric()) {
                return None;
            }
        }
        Some(Self(SmolStr::new(code.to_ascii_lowercase())))
    }
}

impl Default for Locale {
    fn default() -> Self {
        Self::english()
    }
}

impl std::fmt::Display for Locale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.as_str())
    }
}

/// Validated set of supported locales, built once at startup by
/// listing `static/locales/*.json`.
///
/// Stored on `AppState` and consulted by:
/// - [`Locale::from_code`] when parsing user-supplied / claim-derived
///   codes.
/// - The `Accept-Language` extractor when negotiating an anonymous
///   request's preference.
/// - The OIDC JIT provisioning path when storing a `locale` claim on
///   a freshly created user row.
///
/// Locales not present here are treated as unknown — callers fall back
/// to the configured server default.
#[derive(Debug, Clone)]
pub struct LocaleRegistry {
    /// Canonicalised codes (e.g. `"en"`, `"zh-tw"`). Lookups are
    /// case-insensitive: input is canonicalised, then probed against
    /// this set.
    canonical: Arc<HashSet<SmolStr>>,
    /// The same codes as an owned `Vec<String>`, materialized ONCE at
    /// [`Self::discover`] time. The `Accept-Language` extractor needs a
    /// `&[&str]` supported-list per anonymous request; without this it
    /// rebuilt N heap `String`s from the registry on every such request
    /// (the ROUND10 §15 "process-invariant rebuilt per request" class;
    /// benches/ROUND13.md §L1). Borrowed via [`Self::supported_codes`].
    supported_codes: Arc<Vec<String>>,
    /// The configured fallback locale. Resolved from
    /// `OXICLOUD_DEFAULT_LOCALE` at startup; defaults to English when
    /// unset.
    default: Locale,
}

impl LocaleRegistry {
    /// Scan `dir` for `*.json` files; the filename stem (less the
    /// `.json` extension) is treated as a locale code. Each file is
    /// parsed eagerly as JSON — a syntactically broken file aborts the
    /// boot with [`LocaleRegistryError::ParseFailure`] so the operator
    /// sees the path + parse error immediately, not after a translator
    /// notices half a UI is missing.
    ///
    /// Per-key English fallback at translate time (see
    /// [`crate::infrastructure::services::file_system_i18n_service`]) is
    /// still the safety net for *partial* translations — a file
    /// shipped with five out of twenty keys works fine. What we will
    /// not tolerate is a file that the JSON parser rejects outright,
    /// because that drops every key for that locale at once with no
    /// surface signal beyond a buried warn log.
    ///
    /// `default` is the configured fallback. It must resolve against
    /// the discovered codes; if not, the registry build fails so the
    /// operator notices their config typo at boot rather than mid-flow.
    pub fn discover(dir: &Path, default_code: &str) -> Result<Self, LocaleRegistryError> {
        let mut canonical: HashSet<SmolStr> = HashSet::new();

        let entries = fs::read_dir(dir).map_err(|e| LocaleRegistryError::ReadDir {
            path: dir.to_path_buf(),
            source: e,
        })?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };

            // Eager parse — full content is loaded lazily by the I18n
            // service later, but a quick parse here catches
            // syntactically broken files at boot. Failures propagate
            // (don't silently skip) so a translator's stray comma is
            // visible at the first restart, not at the first user
            // request.
            let content =
                fs::read_to_string(&path).map_err(|e| LocaleRegistryError::ReadFailure {
                    path: path.clone(),
                    source: e,
                })?;
            serde_json::from_str::<serde_json::Value>(&content).map_err(|e| {
                LocaleRegistryError::ParseFailure {
                    path: path.clone(),
                    source: e,
                }
            })?;
            canonical.insert(Self::canonicalise(stem));
        }

        if canonical.is_empty() {
            return Err(LocaleRegistryError::Empty(dir.to_path_buf()));
        }

        let default_canon = Self::canonicalise(default_code);
        if !canonical.contains(&default_canon) {
            return Err(LocaleRegistryError::DefaultNotPresent {
                requested: default_code.to_string(),
                available: canonical.iter().map(|s| s.to_string()).collect(),
            });
        }

        let default = Locale(default_canon);

        let mut sorted: Vec<&str> = canonical.iter().map(|s| s.as_str()).collect();
        sorted.sort();
        tracing::info!(
            target: "oxicloud::i18n",
            "Loaded {} locales: {}",
            sorted.len(),
            sorted.join(", ")
        );

        // Materialize the supported-codes list once. Order is irrelevant —
        // `accept_language::intersection` ranks by the request header's
        // q-values, not by this list's order.
        let supported_codes: Vec<String> =
            canonical.iter().map(|s| s.as_str().to_string()).collect();

        Ok(Self {
            canonical: Arc::new(canonical),
            supported_codes: Arc::new(supported_codes),
            default,
        })
    }

    /// Parse a code, returning a [`Locale`] iff it's in the registry.
    /// Matching is case-insensitive on both sides — `"FR"`, `"fr"`,
    /// `"Fr"` all collapse to the same canonical form.
    pub fn parse(&self, code: &str) -> Option<Locale> {
        let canon = Self::canonicalise(code);
        if self.canonical.contains(&canon) {
            Some(Locale(canon))
        } else {
            None
        }
    }

    /// Parse a code, falling back to the configured default when the
    /// code is unknown. The common shape for callers that want a
    /// `Locale` no matter what.
    pub fn parse_or_default(&self, code: &str) -> Locale {
        self.parse(code).unwrap_or_else(|| self.default.clone())
    }

    /// Borrow the configured fallback locale.
    pub fn default_locale(&self) -> &Locale {
        &self.default
    }

    /// Iterate every locale in the registry, in arbitrary order. Used
    /// by the preload step at startup.
    pub fn iter(&self) -> impl Iterator<Item = Locale> + '_ {
        self.canonical.iter().map(|s| Locale(s.clone()))
    }

    /// The registry's codes as a borrowable `&[String]`, precomputed at
    /// [`Self::discover`] time. Feeds the per-request `Accept-Language`
    /// negotiation without re-allocating the list (benches/ROUND13.md §L1).
    pub fn supported_codes(&self) -> &[String] {
        &self.supported_codes
    }

    /// Number of locales in the registry. Used by tests + startup logs.
    pub fn len(&self) -> usize {
        self.canonical.len()
    }

    /// True iff the registry has no entries. Convenience for tests;
    /// production builds always have ≥1 (English is mandatory).
    pub fn is_empty(&self) -> bool {
        self.canonical.is_empty()
    }

    /// Canonical form for matching: ASCII-lowercase. This means `fr-FR`
    /// and `fr-fr` collapse to the same key, which is the right policy
    /// — RFC 5646 says language tags are case-insensitive, and storing
    /// a single canonical form keeps the hash-set small and predictable.
    fn canonicalise(code: &str) -> SmolStr {
        SmolStr::new(code.to_ascii_lowercase())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LocaleRegistryError {
    #[error("Failed to read locale directory {path}: {source}")]
    ReadDir {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to read locale file {path}: {source}")]
    ReadFailure {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Locale file {path} is not valid JSON: {source}")]
    ParseFailure {
        path: std::path::PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("Locale directory {0} contains no valid *.json files")]
    Empty(std::path::PathBuf),

    #[error(
        "Configured default locale {requested:?} is not in the registry. \
         Available: {available:?}"
    )]
    DefaultNotPresent {
        requested: String,
        available: Vec<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp_dir_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for (name, body) in files {
            let path = dir.path().join(name);
            let mut f = fs::File::create(&path).expect("create file");
            f.write_all(body.as_bytes()).expect("write");
        }
        dir
    }

    #[test]
    fn from_code_accepts_well_formed_tags() {
        assert_eq!(Locale::from_code("en").unwrap().as_str(), "en");
        assert_eq!(Locale::from_code("FR").unwrap().as_str(), "fr");
        assert_eq!(Locale::from_code("zh-TW").unwrap().as_str(), "zh-tw");
        assert_eq!(Locale::from_code("en-US").unwrap().as_str(), "en-us");
    }

    #[test]
    fn from_code_rejects_garbage() {
        assert!(Locale::from_code("").is_none());
        assert!(Locale::from_code("e").is_none()); // too short
        assert!(Locale::from_code("toolong").is_none()); // primary > 3
        assert!(Locale::from_code("en_US").is_none()); // underscore not allowed
        assert!(Locale::from_code("12").is_none()); // digits in primary
        assert!(Locale::from_code("en-X").is_none()); // subtag too short
    }

    #[test]
    fn default_is_english() {
        assert_eq!(Locale::default().as_str(), "en");
    }

    #[test]
    fn english_is_always_canonical_en() {
        assert_eq!(Locale::english().as_str(), "en");
        assert!(Locale::english().is_english());
    }

    #[test]
    fn discover_lists_only_json_files() {
        let dir = tmp_dir_with(&[
            ("en.json", "{}"),
            ("fr.json", "{}"),
            ("README.md", "not a locale"),
            ("backup.txt", "ignored"),
        ]);
        let reg = LocaleRegistry::discover(dir.path(), "en").expect("registry");
        assert_eq!(reg.len(), 2);
        assert!(reg.parse("en").is_some());
        assert!(reg.parse("fr").is_some());
        assert!(reg.parse("README").is_none());
    }

    #[test]
    fn discover_fails_fast_on_broken_json() {
        // A translator's stray comma must take the server down on the
        // next restart rather than silently dropping their locale —
        // see [`LocaleRegistry::discover`] doc for the rationale.
        let dir = tmp_dir_with(&[
            ("en.json", "{}"),
            ("broken.json", "{ not valid json"),
            ("fr.json", r#"{"hello":"world"}"#),
        ]);
        let err = LocaleRegistry::discover(dir.path(), "en").unwrap_err();
        match err {
            LocaleRegistryError::ParseFailure { path, .. } => {
                assert_eq!(
                    path.file_name().and_then(|s| s.to_str()),
                    Some("broken.json")
                );
            }
            other => panic!("expected ParseFailure, got {:?}", other),
        }
    }

    #[test]
    fn parse_is_case_insensitive() {
        let dir = tmp_dir_with(&[("en.json", "{}"), ("zh-TW.json", "{}")]);
        let reg = LocaleRegistry::discover(dir.path(), "en").expect("registry");
        assert_eq!(
            reg.parse("ZH-tw").map(|l| l.as_str().to_string()),
            Some("zh-tw".to_string())
        );
        assert_eq!(
            reg.parse("zh-tw").map(|l| l.as_str().to_string()),
            Some("zh-tw".to_string())
        );
        assert_eq!(
            reg.parse("zh-TW").map(|l| l.as_str().to_string()),
            Some("zh-tw".to_string())
        );
    }

    #[test]
    fn parse_or_default_falls_back() {
        let dir = tmp_dir_with(&[("en.json", "{}"), ("fr.json", "{}")]);
        let reg = LocaleRegistry::discover(dir.path(), "en").expect("registry");
        assert_eq!(reg.parse_or_default("klingon").as_str(), "en");
        assert_eq!(reg.parse_or_default("fr").as_str(), "fr");
    }

    #[test]
    fn empty_directory_is_error() {
        let dir = tmp_dir_with(&[]);
        let err = LocaleRegistry::discover(dir.path(), "en").unwrap_err();
        assert!(matches!(err, LocaleRegistryError::Empty(_)));
    }

    #[test]
    fn default_must_be_in_registry() {
        let dir = tmp_dir_with(&[("en.json", "{}"), ("fr.json", "{}")]);
        let err = LocaleRegistry::discover(dir.path(), "de").unwrap_err();
        match err {
            LocaleRegistryError::DefaultNotPresent { requested, .. } => {
                assert_eq!(requested, "de");
            }
            _ => panic!("expected DefaultNotPresent"),
        }
    }
}
