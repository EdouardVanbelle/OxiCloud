use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{Request, StatusCode},
    middleware,
    response::{IntoResponse, Response},
    routing::{any, delete, get, post},
};
use std::sync::Arc;

use crate::interfaces::middleware::auth::{AuthUser, CurrentUser};
use crate::interfaces::middleware::rate_limit::{RateLimiter, rate_limit_login};
use crate::interfaces::nextcloud::avatar_handler;
use crate::interfaces::nextcloud::basic_auth_middleware::basic_auth_middleware;
use crate::interfaces::nextcloud::login_v2_handler;
use crate::interfaces::nextcloud::ocs_handler;
use crate::interfaces::nextcloud::preview_handler;
use crate::interfaces::nextcloud::status_handler;
use crate::interfaces::nextcloud::trashbin_handler;
use crate::interfaces::nextcloud::uploads_handler;
use crate::interfaces::nextcloud::webdav_handler;
use crate::{application::dtos::folder_dto::FolderDto, common::di::AppState};

/// Build Nextcloud routes with a pre-built `Arc<AppState>` for the middleware layer.
///
/// This is the preferred entry point — pass the real state so the Basic Auth
/// middleware can look up app passwords from the database.
pub fn nextcloud_routes_with_state(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // Rate limiter for NC login submit (reuses auth config values)
    let nc_login_limiter = {
        let rl = &state.core.config.auth.rate_limit;
        Arc::new(RateLimiter::new(
            rl.login_max_requests,
            rl.login_window_secs,
            100_000,
        ))
    };

    // Public routes — no auth required.
    let public = Router::new()
        .route("/status.php", get(status_handler::handle_status))
        // NC connectivity check — app expects 204 to confirm server is reachable.
        .route("/index.php/204", get(handle_connectivity_check))
        // Bare /remote.php/dav — NC clients probe this to confirm WebDAV is available.
        .route("/remote.php/dav", any(handle_dav_discovery))
        .route("/remote.php/dav/", any(handle_dav_discovery))
        .route(
            "/index.php/login/v2",
            post(login_v2_handler::handle_login_initiate),
        )
        .route(
            "/login/v2/flow/{token}",
            get(login_v2_handler::handle_login_page)
                .post(login_v2_handler::handle_login_submit)
                .layer(axum::middleware::from_fn_with_state(
                    nc_login_limiter,
                    rate_limit_login,
                )),
        )
        // Drive picker submission — finalises a multi-drive flow that
        // paused after password verification. Public route by design:
        // the flow token + single-use `pending_user_id` slot is the
        // proof of authentication. See `login_v2_handler::handle_drive_pick`.
        .route(
            "/login/v2/flow/{token}/drive",
            post(login_v2_handler::handle_drive_pick),
        )
        // OIDC initiation from Nextcloud login page
        .route(
            "/login/v2/flow/{token}/oidc",
            get(login_v2_handler::handle_login_oidc),
        )
        .route(
            "/index.php/login/v2/poll",
            post(login_v2_handler::handle_login_poll),
        )
        .route("/login/v2/poll", post(login_v2_handler::handle_login_poll))
        // Capabilities are public — iOS app fetches them before having credentials.
        .route(
            "/ocs/v1.php/cloud/capabilities",
            get(ocs_handler::handle_capabilities_v1),
        )
        .route(
            "/ocs/v2.php/cloud/capabilities",
            get(ocs_handler::handle_capabilities_v2),
        );

    // Protected routes — require Basic Auth via app passwords.
    let protected = Router::new()
        // Both v1 and v2 of the singular cloud/user endpoint return the
        // same payload shape — NC's URL-versioning is a transport
        // convention, not a protocol break for this endpoint. Older
        // NC clients (and some third-party libraries) still hit v1
        // first; without this route they get a 404 even though the
        // handler exists.
        .route("/ocs/v1.php/cloud/user", get(ocs_handler::handle_user_info))
        .route("/ocs/v2.php/cloud/user", get(ocs_handler::handle_user_info))
        .route(
            "/ocs/v1.php/cloud/users/{userid}",
            get(ocs_handler::handle_user_provisioning_v1),
        )
        .route(
            "/ocs/v2.php/cloud/users/{userid}",
            get(ocs_handler::handle_user_provisioning_v2),
        )
        .route(
            "/ocs/v2.php/core/apppassword",
            delete(ocs_handler::handle_revoke_apppassword),
        )
        .route(
            "/ocs/v2.php/apps/notifications/api/v2/notifications",
            get(ocs_handler::handle_notifications_list),
        )
        .route(
            "/ocs/v2.php/apps/notifications/api/v2/push",
            post(ocs_handler::handle_notifications_push),
        )
        .route(
            "/ocs/v2.php/apps/recommendations/api/v1/recommendations",
            get(ocs_handler::handle_recommendations),
        )
        .route(
            "/ocs/v2.php/apps/files_sharing/api/v1/sharees",
            get(ocs_handler::handle_sharees_search),
        )
        // Unified Search
        .route(
            "/ocs/v2.php/search/providers",
            get(ocs_handler::handle_search_providers),
        )
        .route(
            "/ocs/v2.php/search/providers/{provider_id}/search",
            get(ocs_handler::handle_search),
        )
        .route(
            "/index.php/core/preview",
            get(preview_handler::handle_preview),
        )
        .route(
            "/index.php/avatar/{user}/{size}",
            get(avatar_handler::handle_avatar),
        )
        // NC desktop + several mobile clients fetch avatars from the
        // DAV-shaped URL (with a literal `.png` extension on the
        // size segment). Same SVG payload, different URL shape — the
        // wrapper handler strips the extension and delegates.
        .route(
            "/remote.php/dav/avatars/{user}/{size}",
            get(avatar_handler::handle_dav_avatar),
        )
        .route(
            "/remote.php/dav/files/{user}/{*subpath}",
            any(handle_dav_files),
        )
        .route("/remote.php/dav/files/{user}/", any(handle_dav_files_root))
        .route("/remote.php/dav/files/{user}", any(handle_dav_files_root))
        .route(
            "/remote.php/dav/uploads/{user}/{upload_id}/{*rest}",
            any(handle_dav_uploads),
        )
        .route(
            "/remote.php/dav/uploads/{user}/{upload_id}",
            any(handle_dav_uploads_root),
        )
        // Trashbin WebDAV
        .route(
            "/remote.php/dav/trashbin/{user}/{*subpath}",
            any(handle_dav_trashbin),
        )
        .route(
            "/remote.php/dav/trashbin/{user}/",
            any(handle_dav_trashbin_root),
        )
        .route(
            "/remote.php/dav/trashbin/{user}",
            any(handle_dav_trashbin_root),
        )
        .route("/remote.php/webdav/{*subpath}", any(handle_legacy_webdav))
        .route("/remote.php/webdav/", any(handle_legacy_webdav_root))
        .route("/remote.php/webdav", any(handle_legacy_webdav_root))
        .layer(middleware::from_fn_with_state(state, basic_auth_middleware));

    Router::new().merge(public).merge(protected)
}

// ──────────────── Handler glue ────────────────

/// Parse the URL `{user}` segment as a `{username}~{drive_marker}`
/// composite, cross-validate both halves against the authenticated
/// session, and resolve the request's storage chroot.
///
/// Validation:
/// - URL prefix (before the first `~`) must equal the authenticated
///   `CurrentUser.username`. "URL says someone else" → 403.
/// - URL marker (after `~`, or `None`) must match the marker stashed
///   by `basic_auth_middleware`. "Auth says drive A, URL says B" → 403.
/// - Empty-half forms (`name~`, `~marker`) are already 401'd in the
///   middleware; here we only see well-formed pairs.
///
/// Chroot resolution:
/// - URL has no marker, OR marker equals `home_folder_id` →
///   `"My Folder - {username}"` (no DB lookup).
/// - URL has any other marker → `get_folder_with_perms(marker, user)`
///   → returns the folder's stored `path`. 404 if the folder is
///   missing or the caller can't read it (anti-enumeration: same
///   response for both cases so non-owners can't probe which UUIDs
///   exist).
#[allow(clippy::result_large_err)]
async fn verify_url_user_and_resolve_chroot(
    state: &Arc<AppState>,
    url_user: &str,
    auth_user: &CurrentUser,
    auth_drive: Option<&str>,
) -> Result<FolderDto, Response> {
    let (url_prefix, url_marker) = match url_user.split_once('~') {
        Some((p, m)) => (p, Some(m)),
        None => (url_user, None),
    };
    if url_prefix != auth_user.username {
        return Err(StatusCode::FORBIDDEN.into_response());
    }
    if url_marker != auth_drive {
        return Err(StatusCode::FORBIDDEN.into_response());
    }
    match url_marker {
        None => {
            // FIXME this if old way to find home folder
            let expected = format!("My Folder - {}", auth_user.username);
            use crate::application::ports::folder_ports::FolderUseCase;
            let home: FolderDto = state
                .applications
                .folder_service
                .list_folders_with_perms(None, auth_user.id) // parent_id=None → root folders
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?
                .into_iter()
                .find(|f| f.name == expected)
                .ok_or_else(|| StatusCode::NOT_FOUND.into_response())?;
            Ok(home)
        }
        //Some(m) if m == home_folder_id => Ok(format!("My Folder - {}", auth_user.username)),
        Some(folder_id) => {
            use crate::application::ports::folder_ports::FolderUseCase;
            let folder = state
                .applications
                .folder_service
                .get_folder_with_perms(folder_id, auth_user.id)
                .await
                .map_err(|_| StatusCode::NOT_FOUND.into_response())?;
            Ok(folder)
        }
    }
}

/// Extract the Basic-Auth-side drive marker (`NcDriveHint`) that
/// `basic_auth_middleware` stashes in request extensions.
fn auth_drive_hint(req: &Request<Body>) -> Option<&str> {
    req.extensions()
        .get::<crate::interfaces::nextcloud::basic_auth_middleware::NcDriveHint>()
        .and_then(|h| h.0.as_deref())
}

async fn handle_dav_files(
    State(state): State<Arc<AppState>>,
    Path((url_user, subpath)): Path<(String, String)>,
    user_ext: AuthUser,
    req: Request<Body>,
) -> Result<Response, Response> {
    let chroot =
        verify_url_user_and_resolve_chroot(&state, &url_user, &user_ext, auth_drive_hint(&req))
            .await?;
    webdav_handler::handle_nc_webdav(state, req, user_ext, chroot, url_user, subpath)
        .await
        .map_err(|e| e.into_response())
}

async fn handle_dav_files_root(
    State(state): State<Arc<AppState>>,
    Path(url_user): Path<String>,
    user_ext: AuthUser,
    req: Request<Body>,
) -> Result<Response, Response> {
    let chroot =
        verify_url_user_and_resolve_chroot(&state, &url_user, &user_ext, auth_drive_hint(&req))
            .await?;
    webdav_handler::handle_nc_webdav(state, req, user_ext, chroot, url_user, String::new())
        .await
        .map_err(|e| e.into_response())
}

async fn handle_dav_uploads(
    State(state): State<Arc<AppState>>,
    Path((url_user, upload_id, rest)): Path<(String, String, String)>,
    user_ext: AuthUser,
    req: Request<Body>,
) -> Result<Response, Response> {
    //let home_id = resolve_home_folder_id(&state, user_ext.id).await?;
    let _chroot =
        verify_url_user_and_resolve_chroot(&state, &url_user, &user_ext, auth_drive_hint(&req))
            .await?;
    // POC: uploads_handler doesn't yet take a drive marker. Chunked
    // uploads land on the user's home regardless. Plumbing the
    // chroot through the uploads / trashbin handlers is deferred
    // until the POC validates the file-DAV wire shape end-to-end.
    uploads_handler::handle_nc_uploads(state, req, user_ext, upload_id, rest)
        .await
        .map_err(|e| e.into_response())
}

async fn handle_dav_uploads_root(
    State(state): State<Arc<AppState>>,
    Path((url_user, upload_id)): Path<(String, String)>,
    user_ext: AuthUser,
    req: Request<Body>,
) -> Result<Response, Response> {
    let _chroot =
        verify_url_user_and_resolve_chroot(&state, &url_user, &user_ext, auth_drive_hint(&req))
            .await?;
    uploads_handler::handle_nc_uploads(state, req, user_ext, upload_id, String::new())
        .await
        .map_err(|e| e.into_response())
}

/// Legacy /remote.php/webdav/* — redirect to /remote.php/dav/files/{user}/*
async fn handle_legacy_webdav(Path(subpath): Path<String>, user_ext: AuthUser) -> Response {
    let location = format!("/remote.php/dav/files/{}/{}", user_ext.username, subpath);
    Response::builder()
        .status(StatusCode::MOVED_PERMANENTLY)
        .header("location", location)
        .body(Body::empty())
        .unwrap()
}

async fn handle_legacy_webdav_root(user_ext: AuthUser) -> Response {
    let location = format!("/remote.php/dav/files/{}/", user_ext.username);
    Response::builder()
        .status(StatusCode::MOVED_PERMANENTLY)
        .header("location", location)
        .body(Body::empty())
        .unwrap()
}

async fn handle_dav_trashbin(
    State(state): State<Arc<AppState>>,
    Path((url_user, subpath)): Path<(String, String)>,
    user_ext: AuthUser,
    req: Request<Body>,
) -> Result<Response, Response> {
    let _chroot =
        verify_url_user_and_resolve_chroot(&state, &url_user, &user_ext, auth_drive_hint(&req))
            .await?;
    trashbin_handler::handle_nc_trashbin(state, req, user_ext, subpath)
        .await
        .map_err(|e| e.into_response())
}

async fn handle_dav_trashbin_root(
    State(state): State<Arc<AppState>>,
    Path(url_user): Path<String>,
    user_ext: AuthUser,
    req: Request<Body>,
) -> Result<Response, Response> {
    let _chroot =
        verify_url_user_and_resolve_chroot(&state, &url_user, &user_ext, auth_drive_hint(&req))
            .await?;
    trashbin_handler::handle_nc_trashbin(state, req, user_ext, String::new())
        .await
        .map_err(|e| e.into_response())
}

/// `GET /index.php/204` — NC app connectivity check. Returns 204 No Content.
async fn handle_connectivity_check() -> Response {
    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Body::empty())
        .unwrap()
}

/// Bare `/remote.php/dav` — NC clients (especially Android) probe this endpoint
/// during server discovery to confirm WebDAV is available.
async fn handle_dav_discovery() -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header("DAV", "1, 3")
        .header("Allow", "OPTIONS, GET, HEAD, PROPFIND")
        .body(Body::empty())
        .unwrap()
}
