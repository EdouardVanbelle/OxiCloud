//! Locale negotiation for anonymous HTTP requests.
//!
//! Resolves the request's preferred locale, in priority order:
//!
//! 1. `?lang=fr` query parameter — explicit override, used by manual
//!    testing and any future "language switcher" link on a public
//!    page. Must match the registry; unknown values fall through.
//! 2. `Accept-Language` header — RFC 9110 quality-weighted list, the
//!    standard browser-driven signal.
//! 3. The configured server default (`OXICLOUD_DEFAULT_LOCALE`), which
//!    is always present in the registry by construction.
//!
//! Wire it as a regular Axum extractor on a handler that needs the
//! caller's locale: the `AppState` carries the [`LocaleRegistry`], so
//! handlers don't have to plumb anything else through.
//!
//! Authenticated requests should NOT use this extractor — their locale
//! comes from `user.preferred_locale` resolved at the service layer.
//! This extractor is for anonymous surfaces (magic-link landing pages,
//! the public login page) where no user row is available yet.

use std::sync::Arc;

use axum::extract::{FromRequestParts, Query};
use axum::http::request::Parts;
use serde::Deserialize;

use crate::common::di::AppState;
use crate::common::locale::Locale;

/// Negotiated locale for the current request.
#[derive(Debug, Clone)]
pub struct RequestLocale(pub Locale);

#[derive(Debug, Deserialize)]
struct LangOverride {
    lang: Option<String>,
}

impl FromRequestParts<Arc<AppState>> for RequestLocale {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let registry = &state.locale_registry;

        // Priority 1 — explicit `?lang=` override. Parse failures or
        // missing values just fall through to the next signal.
        if let Ok(Query(LangOverride { lang: Some(code) })) =
            Query::<LangOverride>::try_from_uri(&parts.uri)
            && let Some(locale) = registry.parse(&code)
        {
            return Ok(RequestLocale(locale));
        }

        // Priority 2 — Accept-Language. Use the `accept-language` crate
        // for RFC-9110 q-value parsing; we pass the registry's codes
        // as the supported list, so the crate hands us back the
        // strongest match. The empty-list case falls through.
        if let Some(header_value) = parts
            .headers
            .get(axum::http::header::ACCEPT_LANGUAGE)
            .and_then(|v| v.to_str().ok())
        {
            // Borrow the precomputed supported-codes list (materialized
            // once at registry build) instead of rebuilding N heap Strings
            // per anonymous request (benches/ROUND13.md §L1). Only the
            // `&[&str]` view the crate needs is built here.
            let supported: Vec<&str> = registry
                .supported_codes()
                .iter()
                .map(String::as_str)
                .collect();
            if let Some(matched) = accept_language::intersection(header_value, &supported).first()
                && let Some(locale) = registry.parse(matched)
            {
                return Ok(RequestLocale(locale));
            }

            // Some browsers send only a primary tag (`fr`) when the
            // user is on `fr-FR`; `intersection` is exact-tag, so a
            // server that ships `fr-FR.json` but not `fr.json` (or
            // vice-versa) needs a fallback. Walk the parsed list once
            // more, this time stripping the subtag.
            for raw in accept_language::parse(header_value) {
                let primary = raw.split('-').next().unwrap_or(&raw);
                if let Some(locale) = registry.parse(primary) {
                    return Ok(RequestLocale(locale));
                }
            }
        }

        // Priority 3 — configured default. Guaranteed to be in the
        // registry (validated at startup).
        Ok(RequestLocale(registry.default_locale().clone()))
    }
}

impl RequestLocale {
    /// Borrow the resolved locale.
    pub fn locale(&self) -> &Locale {
        &self.0
    }

    /// Move the resolved locale out of the extractor.
    pub fn into_inner(self) -> Locale {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::locale::LocaleRegistry;
    use std::fs;
    use std::io::Write;

    /// Smoke test for the priority logic, exercised against the
    /// registry's `parse` directly so we don't need a full `AppState`
    /// to assert behaviour. The extractor's prose above describes the
    /// negotiation order; this test just locks in the building blocks.
    fn registry_with(codes: &[&str], default: &str) -> Arc<LocaleRegistry> {
        let dir = tempfile::tempdir().expect("tempdir");
        for code in codes {
            let path = dir.path().join(format!("{}.json", code));
            let mut f = fs::File::create(&path).expect("create");
            f.write_all(b"{}").expect("write");
        }
        let reg = LocaleRegistry::discover(dir.path(), default).expect("registry");
        // Leak the tempdir for the lifetime of the test — `discover`
        // already finished its filesystem work, so we just need the
        // registry to outlive the call.
        std::mem::forget(dir);
        Arc::new(reg)
    }

    #[test]
    fn registry_supplies_supported_list_for_intersection() {
        let reg = registry_with(&["en", "fr", "de"], "en");
        let owned: Vec<String> = reg.iter().map(|l| l.as_str().to_string()).collect();
        let supported: Vec<&str> = owned.iter().map(String::as_str).collect();
        let pick = accept_language::intersection("de, fr;q=0.9", &supported);
        assert_eq!(pick.first().map(String::as_str), Some("de"));
    }

    #[test]
    fn primary_tag_fallback_when_subtag_missing() {
        let reg = registry_with(&["en", "fr"], "en");
        let owned: Vec<String> = reg.iter().map(|l| l.as_str().to_string()).collect();
        let supported: Vec<&str> = owned.iter().map(String::as_str).collect();
        // Exact `fr-FR` is not in the registry; intersection returns
        // empty, but the primary-tag walk hits `fr`.
        let pick = accept_language::intersection("fr-FR", &supported);
        assert!(pick.is_empty());
        for raw in accept_language::parse("fr-FR") {
            let primary = raw.split('-').next().unwrap_or(&raw);
            if reg.parse(primary).is_some() {
                return; // hit the fallback
            }
        }
        panic!("primary-tag fallback did not match `fr`");
    }
}
