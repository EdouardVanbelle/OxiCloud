//! Caller-id-based user guards.
//!
//! All guards in this module take `(auth, caller_id) → Result<(), AppError>`
//! so handlers compose them uniformly as one-liners. They assume the
//! caller has already been authenticated by the
//! [`AuthUser`](super::auth::AuthUser) extractor, and pull the current
//! user state from the database via `AuthApplicationService` so role /
//! external-flag changes take effect on the next request without
//! waiting for token rotation.
//!
//! ```ignore
//! let caller_id = auth_user.id;
//! require_internal_user(&auth, caller_id).await?;
//! require_admin_user(&auth, caller_id).await?;
//! ```
//!
//! Future role-based guards (e.g. `require_active_user`) should follow
//! the same shape so they slot in next to these without ceremony.
//!
//! For the legacy header-based admin guard (`require_admin`), see
//! [`super::admin`] — that variant exists because some handlers take
//! `headers: HeaderMap` directly instead of `AuthUser`.

use axum::http::StatusCode;
use uuid::Uuid;

use crate::application::services::auth_application_service::AuthApplicationService;
use crate::interfaces::errors::AppError;

/// Require the caller to be an internal user. Returns `Ok(())` for
/// internal callers, `Err(403)` for externals.
///
/// External users authenticate via magic-link / OIDC-only / OCM and
/// exist solely to interact with resources they were explicitly
/// granted. They have no business enumerating the user directory, the
/// address book, subject groups, or any other instance-wide listing —
/// this guard locks them out of those surfaces.
///
/// DB lookup errors fall back to `Ok(())` so a transient outage doesn't
/// lock everyone out — this guard is defense in depth. The canonical
/// filter is at the service / repository layer (`include_external =
/// false` on `list_users`, the visibility rule in `get_user_profile`,
/// etc.); this helper just opts a surface in to "internal only" with
/// one extra line.
///
/// The 403 status is honest (not 404 stealth) because the caller's own
/// `is_external` flag is not a secret to themselves — the UI already
/// surfaces "you came in through a magic link".
pub async fn require_internal_user(
    auth: &AuthApplicationService,
    caller_id: Uuid,
) -> Result<(), AppError> {
    match auth.get_user_by_id(caller_id).await {
        Ok(dto) if dto.is_external => Err(AppError::new(
            StatusCode::FORBIDDEN,
            "External users cannot access this endpoint",
            "Forbidden",
        )),
        _ => Ok(()),
    }
}

/// Require the caller to hold the admin role. Returns `Ok(())` for
/// admins, `Err(403)` otherwise.
///
/// The check pulls the role from the user record (not from JWT
/// claims) so a role change takes effect on the next request without
/// waiting for token rotation. Mirrors [`require_internal_user`]'s
/// shape so handlers compose either of them as a one-liner via `?`.
///
/// Use this in handlers that already have an
/// [`AuthUser`](super::auth::AuthUser) extractor (and thus a validated
/// `caller_id`); use the legacy [`super::admin::require_admin`] variant
/// when the handler signature is `headers: HeaderMap` instead.
pub async fn require_admin_user(
    auth: &AuthApplicationService,
    caller_id: Uuid,
) -> Result<(), AppError> {
    let user = auth
        .get_user_by_id(caller_id)
        .await
        .map_err(AppError::from)?;

    if user.role != "admin" {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "Admin access required",
            "Forbidden",
        ));
    }
    Ok(())
}
