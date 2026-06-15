//! Per-request NextCloud session context.
//!
//! Bundles WHO the caller is, the raw wire username they presented,
//! and (for path-scoped endpoints) WHERE they're confined to. Built
//! by `basic_auth_middleware` and stashed in request extensions as
//! `Arc<NcSession>`; handlers extract it via the [`FromRequestParts`]
//! impl below — just declare `session: NcSession` in the signature.
//!
//! ## Source of truth
//!
//! - `user`: authenticated identity (id, canonical username, role).
//! - `raw_username`: the opaque wire identifier from the Basic Auth
//!   header. Today: plain `user` (single-drive) or `user~{drive_uuid}`
//!   (multi-drive POC). May look different again when future auth
//!   schemes land. **Handlers MUST NOT parse it** — it's used verbatim
//!   only for echoing back into DAV/OCS URLs the client expects to
//!   see (notably OCS `cloud/user`'s `id` field, which NC desktop
//!   splices into every subsequent DAV path it builds) and for
//!   audit logs.
//! - `chroot`: folder the request is jailed inside. `Some` for every
//!   authenticated NC request today (the home folder when no drive
//!   marker is present, or the resolved drive when one is). `None`
//!   is reserved for future routes that don't operate on a single
//!   folder (admin / cross-drive queries).
//!
//! ## Why this lives in middleware, not routes.rs
//!
//! The auth step already has every input needed (raw username from
//! header + drive marker after `~` + authenticated user). Resolving
//! the chroot there means every NC handler — DAV, OCS, uploads,
//! trashbin, sharees, … — gets a uniform `NcSession` regardless of
//! whether its URL carries a `{user}` segment. The URL `{user}`
//! segment becomes informational; the auth header is canonical.

use std::sync::Arc;

use axum::{
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};

use crate::application::dtos::folder_dto::FolderDto;
use crate::interfaces::errors::AppError;
use crate::interfaces::middleware::auth::CurrentUser;

#[derive(Debug, Clone)]
pub struct NcSession {
    pub user: CurrentUser,
    pub raw_username: String,
    pub chroot: Option<FolderDto>,
}

impl NcSession {
    /// Return the chroot, or 500 if a path-scoped handler is reached
    /// without one. Documents the invariant that every NC route
    /// today is path-scoped — if this fires, route wiring is wrong.
    pub fn require_chroot(&self) -> Result<&FolderDto, AppError> {
        self.chroot.as_ref().ok_or_else(|| {
            AppError::internal_error(
                "NcSession: path-scoped handler reached without a chroot — route wiring bug",
            )
        })
    }

    /// True when the session is scoped to the user's home folder
    /// (no drive marker in the Basic Auth username). Useful for
    /// handlers that want to render a friendlier display when the
    /// user is on their default drive.
    pub fn is_home(&self) -> bool {
        !self.raw_username.contains('~')
    }
}

/// Pull the `{user}` segment out of a NC DAV URL.
///
/// Expected URL shapes:
/// - `/remote.php/dav/files/{user}` (root)
/// - `/remote.php/dav/files/{user}/{*subpath}`
/// - `/remote.php/dav/uploads/{user}/{upload_id}[/{*rest}]`
/// - `/remote.php/dav/trashbin/{user}[/{*subpath}]`
///
/// Returns `None` for anything that doesn't follow this shape (notably
/// the OCS surfaces, where there is no `{user}` segment to compare).
fn extract_url_user(path: &str) -> Option<String> {
    let mut segments = path.split('/');
    if !segments.next()?.is_empty() {
        return None;
    }
    if segments.next()? != "remote.php" {
        return None;
    }
    if segments.next()? != "dav" {
        return None;
    }
    let _surface = segments.next()?; // files / uploads / trashbin
    let user_seg = segments.next()?;
    if user_seg.is_empty() {
        return None;
    }
    urlencoding::decode(user_seg).ok().map(|s| s.into_owned())
}

/// Axum extractor: pulls the `Arc<NcSession>` that
/// `basic_auth_middleware` stashed in request extensions and clones
/// it (cheap — one `Arc` increment, no field copy) into an owned
/// `NcSession` for handler use.
///
/// On path-scoped DAV routes (`/remote.php/dav/{files,uploads,
/// trashbin}/{user}/…`), the URL `{user}` segment is cross-checked
/// against `session.raw_username` and 403'd on mismatch. This is a
/// consistency check, NOT a security boundary — the chroot ACL
/// (`get_folder_with_perms`) is what actually prevents cross-user
/// access. It just surfaces malformed requests early (403) instead
/// of silently letting them through.
impl<S: Send + Sync> FromRequestParts<S> for NcSession {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let session = parts
            .extensions
            .get::<Arc<NcSession>>()
            .map(|arc| (**arc).clone())
            .ok_or_else(|| StatusCode::UNAUTHORIZED.into_response())?;

        if let Some(url_user) = extract_url_user(parts.uri.path())
            && url_user != session.raw_username
        {
            return Err(StatusCode::FORBIDDEN.into_response());
        }

        Ok(session)
    }
}
