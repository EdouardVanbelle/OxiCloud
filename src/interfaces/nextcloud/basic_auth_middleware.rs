use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::Engine;
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
    // encoding and doesn't collide with UUID hyphens. The marker
    // after `~` is a chroot SELECTOR (handled by `NcSession` via the
    // URL `{user}` segment), NOT an auth credential — the password
    // is verified against the username PREFIX. The middleware just
    // peels the prefix off so the app-password lookup uses the
    // canonical name. When no `~` is present, the request is a
    // plain single-drive ("home") NC sync.
    //
    // Reject `name~` (empty marker) and `~marker` (empty username)
    // at the auth boundary rather than treating them as "missing
    // marker" — they are unambiguous typos that would otherwise
    // silently fall into a different code path.
    let (username, drive_marker): (String, Option<String>) = match raw_username.split_once('~') {
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
    let client_ip = crate::interfaces::middleware::rate_limit::extract_client_ip(&request);
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
                && let Ok(flags) = auth_svc
                    .auth_application_service
                    .get_user_flags(user_id)
                    .await
                && flags.is_external
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
            let current_user = CurrentUser {
                id: user_id,
                username: uname,
                email,
                role,
            };

            // ── Resolve chroot from the Basic Auth drive marker ─────
            // No marker → user's home folder. With a marker →
            // `get_folder_with_perms` enforces per-folder access (404
            // anti-enumeration on miss / no-read). Today this is the
            // sole chroot source; tomorrow it'll come from the
            // app-password row instead.
            use crate::application::ports::folder_ports::FolderUseCase;
            let chroot = match drive_marker.as_deref() {
                None => {
                    let expected = format!("My Folder - {}", current_user.username);
                    match state
                        .applications
                        .folder_service
                        .list_folders_with_perms(None, current_user.id)
                        .await
                    {
                        Ok(folders) => folders.into_iter().find(|f| f.name == expected),
                        Err(_) => None,
                    }
                }
                Some(folder_id) => state
                    .applications
                    .folder_service
                    .get_folder_with_perms(folder_id, current_user.id)
                    .await
                    .ok(),
            };
            if chroot.is_none() {
                tracing::warn!(
                    "[NC] 404 chroot not resolvable: user={} marker={:?}",
                    current_user.username,
                    drive_marker
                );
                return Err(NextcloudAuthError::Unauthorized);
            }

            request
                .extensions_mut()
                .insert(Arc::new(current_user.clone()));
            request.extensions_mut().insert(Arc::new(
                crate::interfaces::nextcloud::session::NcSession {
                    user: current_user,
                    raw_username: raw_username.clone(),
                    chroot,
                },
            ));
            tracing::Span::current().record(
                "chroot_id",
                request
                    .extensions()
                    .get::<Arc<crate::interfaces::nextcloud::session::NcSession>>()
                    .and_then(|s| s.chroot.as_ref())
                    .map(|c| c.id.to_string())
                    .unwrap_or_default(),
            );
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
