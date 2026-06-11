//! Admin role guard — shared across handlers that gate on `claims.role == "admin"`.
//!
//! Extracted from `admin_handler.rs::admin_guard` so the subject-group
//! handler (and any future admin-only surface) can reuse the same code path
//! without duplication.
//!
//! Returns `(user_id, role)` on success so callers have the caller's UUID
//! for audit / ownership purposes.

use axum::http::{HeaderMap, StatusCode, header};
use uuid::Uuid;

use crate::application::ports::auth_ports::TokenServicePort;
use crate::common::di::AppState;
use crate::interfaces::api::cookie_auth::{ACCESS_COOKIE, extract_cookie_value};
use crate::interfaces::errors::AppError;

/// Validate the request's JWT (from the `Authorization: Bearer …` header
/// or the access-token cookie) and require `claims.role == "admin"`.
///
/// On success returns `(user_id, role)`; on failure returns:
///   - 401 if no token / invalid token,
///   - 403 if the token is valid but the role is not `admin`,
///   - 500 if the auth service is not configured.
pub async fn require_admin(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(Uuid, String), AppError> {
    let auth = state
        .auth_service
        .as_ref()
        .ok_or_else(|| AppError::internal_error("Auth service not configured"))?;

    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ").map(|s| s.to_string()))
        .or_else(|| extract_cookie_value(headers, ACCESS_COOKIE))
        .ok_or_else(|| AppError::unauthorized("Authorization token required"))?;

    let claims = auth
        .token_service
        .validate_token(&token)
        .map_err(|e| AppError::unauthorized(format!("Invalid token: {}", e)))?;

    if claims.role != "admin" {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "Admin access required",
            "Forbidden",
        ));
    }

    Ok((
        Uuid::parse_str(&claims.sub)
            .map_err(|_| AppError::internal_error("Invalid user ID in token"))?,
        claims.role.clone(),
    ))
}

/// Validate the request's JWT (any role) and return `(user_id, role)`.
///
/// Like `require_admin` but does not enforce the admin role — useful for
/// share-dialog autocomplete and similar surfaces that need a logged-in
/// caller but don't care about their role.
pub async fn require_authenticated(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(Uuid, String), AppError> {
    let auth = state
        .auth_service
        .as_ref()
        .ok_or_else(|| AppError::internal_error("Auth service not configured"))?;

    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ").map(|s| s.to_string()))
        .or_else(|| extract_cookie_value(headers, ACCESS_COOKIE))
        .ok_or_else(|| AppError::unauthorized("Authorization token required"))?;

    let claims = auth
        .token_service
        .validate_token(&token)
        .map_err(|e| AppError::unauthorized(format!("Invalid token: {}", e)))?;

    Ok((
        Uuid::parse_str(&claims.sub)
            .map_err(|_| AppError::internal_error("Invalid user ID in token"))?,
        claims.role.clone(),
    ))
}
