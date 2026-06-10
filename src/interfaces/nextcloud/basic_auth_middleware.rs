use axum::{
    extract::{FromRequestParts, Request, State},
    http::{HeaderMap, StatusCode, header, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::Engine;
use std::convert::Infallible;
use std::sync::Arc;

use crate::common::di::AppState;
use crate::interfaces::middleware::auth::CurrentUser;

#[derive(Debug, thiserror::Error)]
pub enum NextcloudAuthError {
    #[error("Unauthorized")]
    Unauthorized,
    #[error("Nextcloud services unavailable")]
    ServiceUnavailable,
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Drive selector parsed from a composite Basic-Auth username of the
/// form `{username}~{drive_marker}`. Stored in request extensions by
/// `basic_auth_middleware` so the route glue can cross-validate it
/// against the URL's marker. `None` means the caller used the
/// legacy single-drive (no `~`) login form.
///
/// Wrapping `Option<String>` in a named tuple keeps the request
/// extension type unambiguous: a future middleware can't shadow it
/// by inserting another bare `Option<String>` with different
/// semantics.
#[derive(Debug, Clone)]
pub struct NcDriveHint(pub Option<String>);

/// Axum extractor for the Basic-Auth-side drive marker. Reads the
/// [`NcDriveHint`] that `basic_auth_middleware` parks in request
/// extensions and unwraps it into a plain `Option<String>`. When the
/// middleware did not run (e.g. on a public route), the extractor
/// yields `None` rather than failing — handlers that need the marker
/// should treat its absence as "no drive selector".
///
/// Use in any handler that needs to echo the composite username back
/// (notably OCS `cloud/user`, which NC desktop relies on for path
/// construction):
///
/// ```ignore
/// pub async fn handle_user_info(
///     user: AuthUser,
///     NcDrive(drive): NcDrive,
/// ) -> Response { ... }
/// ```
pub struct NcDrive(pub Option<String>);

impl<S: Sync> FromRequestParts<S> for NcDrive {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(NcDrive(
            parts
                .extensions
                .get::<NcDriveHint>()
                .and_then(|h| h.0.clone()),
        ))
    }
}

impl IntoResponse for NextcloudAuthError {
    fn into_response(self) -> Response {
        match self {
            NextcloudAuthError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, "Basic realm=\"OxiCloud\"")],
                "Unauthorized",
            )
                .into_response(),
            NextcloudAuthError::ServiceUnavailable => {
                (StatusCode::SERVICE_UNAVAILABLE, "Nextcloud unavailable").into_response()
            }
            NextcloudAuthError::Internal(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
            }
        }
    }
}

pub async fn basic_auth_middleware(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut request: Request,
    next: Next,
) -> Result<Response, NextcloudAuthError> {
    tracing::debug!("[NC] {} {}", request.method(), request.uri());

    let auth_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            tracing::warn!(
                "[NC] 401 no auth header: {} {}",
                request.method(),
                request.uri()
            );
            NextcloudAuthError::Unauthorized
        })?;

    let (raw_username, password) =
        parse_basic_auth(auth_header).ok_or(NextcloudAuthError::Unauthorized)?;

    // ── Multi-drive composite-username parse ────────────────────────
    // POC wire shape: `{username}~{drive_marker}` may appear in the
    // Basic Auth header. `~` was chosen because it needs no URL
    // encoding and doesn't collide with UUID hyphens. Split once;
    // anything to the left of the first `~` is the real username
    // used for app-password lookup, anything to the right is the
    // drive selector (opaque to the middleware — the handlers use
    // it to pick the drive). When no `~` is present, the request is
    // a plain single-drive ("home") NC sync and behaves identically
    // to the pre-POC implementation.
    //
    // Reject `name~` (empty marker) and `~marker` (empty username)
    // at the auth boundary rather than treating them as "missing
    // marker" — they are unambiguous typos that would otherwise
    // silently fall into a different code path.
    let (username, drive_marker) = match raw_username.split_once('~') {
        Some(("", _)) => {
            tracing::warn!(
                "[NC] 401 malformed composite username (empty prefix): {}",
                raw_username
            );
            return Err(NextcloudAuthError::Unauthorized);
        }
        Some((_, "")) => {
            tracing::warn!(
                "[NC] 401 malformed composite username (empty marker): {}",
                raw_username
            );
            return Err(NextcloudAuthError::Unauthorized);
        }
        Some((u, m)) => (u.to_string(), Some(m.to_string())),
        None => (raw_username.clone(), None),
    };

    // Check account lockout before attempting password verification (saves CPU).
    // The lockout is per (account, IP), see #323 for rationale.
    let client_ip =
        crate::interfaces::middleware::rate_limit::extract_client_ip(&request);

    // Check account lockout before attempting password verification (saves CPU)
    if let Some(auth_svc) = state.auth_service.as_ref()
        && let Err(secs) = auth_svc.login_lockout.check(&username, &client_ip)
    {
        tracing::warn!(
            username = %username,
            client_ip = %client_ip,
            lockout_remaining_secs = secs,
            "[NC] Account locked, too many failed attempts from this IP"
        );
        return Err(NextcloudAuthError::Unauthorized);
    }

    let nextcloud = state
        .nextcloud
        .as_ref()
        .ok_or(NextcloudAuthError::ServiceUnavailable)?;

    match nextcloud
        .app_passwords
        .verify_basic_auth(&username, &password)
        .await
    {
        Ok((user_id, uname, email, role)) => {
            // Reset lockout counter on success
            if let Some(auth_svc) = state.auth_service.as_ref() {
                auth_svc.login_lockout.record_success(&username, &client_ip);
            }
            // External users must never authenticate against the NC
            // surface — that whole subtree (WebDAV files, uploads,
            // trashbin, OCS user info, sharees autocomplete, etc.) has
            // no semantic meaning for a magic-link-only principal, and
            // an app password would be a persistent credential
            // bypassing the magic-link-eligibility rule. POST
            // /api/auth/app-passwords also gates externals upfront;
            // this is the belt-and-braces check in case one slipped
            // through (e.g. user later flipped to is_external).
            if let Some(auth_svc) = state.auth_service.as_ref()
                && let Ok(user) = auth_svc
                    .auth_application_service
                    .get_user_by_id(user_id)
                    .await
                && user.is_external
            {
                tracing::info!(
                    target: "audit",
                    event = "auth.nc_basic_rejected",
                    reason = "external_user",
                    user_id = %user_id,
                    "👮🏻‍♂️ External user attempted NC Basic auth — rejected"
                );
                return Err(NextcloudAuthError::Unauthorized);
            }
            // Populate the deferred `user_id` field on the request
            // tracing span (declared in `middleware/trace_span.rs::ClientIpMakeSpan`).
            // Mirrors what `interfaces/middleware/auth.rs` does for the
            // JWT path so the two auth surfaces produce log lines with
            // the same structured shape — without this, every NC
            // request would appear in the logs with `user_id=-`,
            // making it harder to correlate WebDAV / OCS activity to
            // a specific principal.
            tracing::Span::current().record("user_id", user_id.to_string());
            request.extensions_mut().insert(Arc::new(CurrentUser {
                id: user_id,
                username: uname,
                email,
                role,
            }));
            // Stash the Basic-Auth-side drive marker so the route
            // glue (`routes.rs::handle_dav_*`) can cross-validate it
            // against the URL's marker. Kept as a typed wrapper —
            // never as a bare `Option<String>` — to avoid an
            // accidental extension-type collision elsewhere.
            request.extensions_mut().insert(NcDriveHint(drive_marker));
            Ok(next.run(request).await)
        }
        Err(_) => {
            // Record failed attempt for lockout tracking
            if let Some(auth_svc) = state.auth_service.as_ref() {
                auth_svc.login_lockout.record_failure(&username, &client_ip);
            }
            Err(NextcloudAuthError::Unauthorized)
        }
    }
}

/// Parse a `Basic` Authorization header into `(username, password)`.
pub fn parse_basic_auth(header_value: &str) -> Option<(String, String)> {
    let mut parts = header_value.splitn(2, ' ');
    let scheme = parts.next()?.trim();
    let encoded = parts.next()?.trim();

    if !scheme.eq_ignore_ascii_case("Basic") {
        return None;
    }

    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (user, pass) = decoded.split_once(':')?;

    Some((user.to_string(), pass.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_basic_auth() {
        let encoded = base64::engine::general_purpose::STANDARD.encode("alice:secret123");
        let header = format!("Basic {}", encoded);
        let (user, pass) = parse_basic_auth(&header).expect("should parse");
        assert_eq!(user, "alice");
        assert_eq!(pass, "secret123");
    }

    #[test]
    fn test_parse_basic_auth_with_colon_in_password() {
        let encoded = base64::engine::general_purpose::STANDARD.encode("user:pass:with:colons");
        let header = format!("Basic {}", encoded);
        let (user, pass) = parse_basic_auth(&header).expect("should parse");
        assert_eq!(user, "user");
        assert_eq!(pass, "pass:with:colons");
    }

    #[test]
    fn test_parse_basic_auth_bearer_scheme_rejected() {
        let encoded = base64::engine::general_purpose::STANDARD.encode("user:pass");
        let header = format!("Bearer {}", encoded);
        assert!(parse_basic_auth(&header).is_none());
    }

    #[test]
    fn test_parse_basic_auth_missing_colon() {
        let encoded = base64::engine::general_purpose::STANDARD.encode("nocolon");
        let header = format!("Basic {}", encoded);
        assert!(parse_basic_auth(&header).is_none());
    }

    #[test]
    fn test_parse_basic_auth_invalid_base64() {
        assert!(parse_basic_auth("Basic not-valid-base64!!!").is_none());
    }

    #[test]
    fn test_parse_basic_auth_case_insensitive_scheme() {
        let encoded = base64::engine::general_purpose::STANDARD.encode("user:pass");
        let header = format!("BASIC {}", encoded);
        let result = parse_basic_auth(&header);
        assert!(result.is_some());
    }
}
