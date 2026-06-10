//! Caller-id-based user guards.
//!
//! All guards in this module take `(auth, caller_id) → Result<(), AppError>`
//! so handlers compose them uniformly as one-liners. They assume the
//! caller has already been authenticated by the
//! [`AuthUser`](super::auth::AuthUser) extractor, and pull the current
//! user flags via `AuthApplicationService::get_user_flags` — a
//! lightweight, short-TTL-cached lookup (no `image` column) — so role /
//! external-flag changes take effect within seconds without waiting
//! for token rotation, while the hot DAV paths stop paying one full-row
//! DB fetch per request.
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

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::sync::Arc;
use uuid::Uuid;

use crate::application::services::auth_application_service::AuthApplicationService;
use crate::common::di::AppState;
use crate::domain::entities::user::UserRole;
use crate::interfaces::errors::AppError;
use crate::interfaces::middleware::auth::CurrentUser;

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
    match auth.get_user_flags(caller_id).await {
        Ok(flags) if flags.is_external => Err(AppError::new(
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
/// claims) so a role change takes effect within the flags-cache TTL —
/// or immediately when changed through `change_user_role`, which
/// invalidates the entry — without waiting for token rotation. Mirrors
/// [`require_internal_user`]'s shape so handlers compose either of
/// them as a one-liner via `?`.
///
/// Use this in handlers that already have an
/// [`AuthUser`](super::auth::AuthUser) extractor (and thus a validated
/// `caller_id`); use the legacy [`super::admin::require_admin`] variant
/// when the handler signature is `headers: HeaderMap` instead.
pub async fn require_admin_user(
    auth: &AuthApplicationService,
    caller_id: Uuid,
) -> Result<(), AppError> {
    let flags = auth
        .get_user_flags(caller_id)
        .await
        .map_err(AppError::from)?;

    if flags.role != UserRole::Admin {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "Admin access required",
            "Forbidden",
        ));
    }
    Ok(())
}

/// Axum middleware layer that blocks external users from a whole route
/// subtree. Apply via `.layer(from_fn_with_state(state, require_internal_user_layer))`
/// on the protocol nests (CalDAV / CardDAV / WebDAV) that have no
/// semantic meaning for externals — they own no calendars, no address
/// books, no home folder.
///
/// Must run AFTER the auth middleware so `CurrentUser` is in the
/// request extensions; in tower order that means the auth layer is
/// added LAST (outermost). If the layer fires on an unauthenticated
/// path (no `CurrentUser` populated), it simply passes through — the
/// inner handler is then responsible for the 401, and we don't blanket-
/// 403 traffic the auth layer would have rejected anyway.
///
/// Emits an `authz.external_user_blocked` audit event on rejection so
/// operators can spot which surfaces externals are probing.
pub async fn require_internal_user_layer(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let caller_id = request
        .extensions()
        .get::<Arc<CurrentUser>>()
        .map(|cu| cu.id);

    let (Some(caller_id), Some(svc)) = (
        caller_id,
        state
            .auth_service
            .as_ref()
            .map(|s| &*s.auth_application_service),
    ) else {
        // No auth populated, or auth disabled globally — pass through.
        return next.run(request).await;
    };

    if let Err(err) = require_internal_user(svc, caller_id).await {
        let path = request.uri().path().to_owned();
        tracing::info!(
            target: "audit",
            event = "authz.external_user_blocked",
            reason = "internal_only_surface",
            caller_id = %caller_id,
            path = %path,
            "👮🏻‍♂️ External user blocked from internal-only route subtree"
        );
        return err.into_response();
    }

    next.run(request).await
}
