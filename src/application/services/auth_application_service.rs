use crate::application::dtos::user_dto::{
    AuthResponseDto, ChangePasswordDto, LoginDto, RefreshTokenDto, RegisterDto, UserDto,
};
use crate::application::ports::auth_ports::{
    OidcIdClaims, OidcServicePort, PasswordHasherPort, SessionStoragePort, TokenServicePort,
    UserStoragePort,
};
use crate::application::ports::user_lifecycle::{DeletionMode, LogoutReason};
use crate::application::services::user_lifecycle_service::UserLifecycleService;
use crate::common::config::OidcConfig;
use crate::common::errors::{DomainError, ErrorKind};
use crate::domain::entities::magic_link_token::{MagicLinkResourceKind, MagicLinkStatus};
use crate::domain::entities::session::Session;
use crate::domain::entities::user::{User, UserRole};
use crate::domain::repositories::magic_link_token_repository::MagicLinkTokenRepository;
use crate::infrastructure::repositories::pg::SessionPgRepository;
use crate::infrastructure::repositories::pg::UserPgRepository;
use crate::infrastructure::services::jwt_service::JwtTokenService;
use crate::infrastructure::services::oidc_service::OidcService;
use crate::infrastructure::services::password_hasher::Argon2PasswordHasher;
use moka::sync::Cache;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;
use std::time::Duration;
use uuid::Uuid;

/// Result of a successful OIDC callback. The handler layer inspects this to
/// decide whether to redirect to the regular frontend or complete a Nextcloud
/// Login Flow v2 session.
pub enum OidcCallbackResult {
    /// Regular web login — contains a one-time exchange code for the frontend.
    WebLogin { exchange_code: String },
    /// Nextcloud Login Flow v2 — the user authenticated via OIDC but the flow
    /// was initiated from the Nextcloud login page. The handler must create an
    /// app password and complete the NC login flow.
    NextcloudLogin {
        nc_flow_token: String,
        user_id: Uuid,
        username: String,
    },
}

/// Outcome of a successful magic-link redemption. The auth tokens are
/// the same shape as a password login; the optional resource fields tell
/// the handler whether to deep-link to the invited resource or fall back
/// to the generic `/shared-with-me` landing.
#[derive(Debug, Clone)]
pub struct MagicLinkRedemption {
    pub auth: AuthResponseDto,
    pub resource_kind: Option<MagicLinkResourceKind>,
    pub resource_id: Option<Uuid>,
}

/// Tracks a pending OIDC authorization flow (CSRF + PKCE + nonce)
#[derive(Clone)]
struct PendingOidcFlow {
    pkce_verifier: String,
    nonce: String,
    /// When set, this OIDC flow was initiated from the Nextcloud Login Flow v2
    /// page. On successful callback the flow will mint an app-password and
    /// complete the Nextcloud login flow instead of issuing internal JWTs.
    nc_flow_token: Option<String>,
}

/// Tracks a pending one-time token exchange after successful OIDC callback
#[derive(Clone)]
struct PendingOidcToken {
    auth_response: AuthResponseDto,
}

/// Interior state for OIDC — protected by RwLock for hot-reload.
struct OidcState {
    service: Option<Arc<OidcService>>,
    config: Option<OidcConfig>,
}

/// Default quota: 100 GB
const DEFAULT_ADMIN_QUOTA: i64 = 107_374_182_400;
const DEFAULT_USER_QUOTA: i64 = 1_073_741_824; // 1 GB

pub struct AuthApplicationService {
    user_storage: Arc<UserPgRepository>,
    session_storage: Arc<SessionPgRepository>,
    password_hasher: Arc<Argon2PasswordHasher>,
    token_service: Arc<JwtTokenService>,
    /// Dispatcher for user-lifecycle events. `None` only in tests that don't
    /// exercise the lifecycle path; production DI always wires this.
    /// HomeFolderLifecycleHook (registered on this dispatcher) owns the
    /// per-user folder provisioning that AuthApplicationService used to do
    /// inline pre-PR 3.
    user_lifecycle: Option<Arc<UserLifecycleService>>,
    /// Path to the storage directory, used for disk-space–aware quota calculation
    storage_path: PathBuf,
    oidc: RwLock<OidcState>,
    /// Pending OIDC authorization flows keyed by state token (CSRF + PKCE + nonce).
    /// Auto-expires after 10 minutes via moka TTL; max 10 000 entries for DoS protection.
    pending_oidc_flows: Cache<String, PendingOidcFlow>,
    /// Pending one-time token codes for secure token delivery after OIDC callback.
    /// Auto-expires after 60 seconds via moka TTL; max 10 000 entries for DoS protection.
    pending_oidc_tokens: Cache<String, PendingOidcToken>,
    /// Magic-link token repository — populated when the magic-link feature
    /// is enabled (PR 8+). `None` means redemption endpoints return 503.
    magic_link_repo: Option<Arc<dyn MagicLinkTokenRepository>>,
}

impl AuthApplicationService {
    pub fn new(
        user_storage: Arc<UserPgRepository>,
        session_storage: Arc<SessionPgRepository>,
        password_hasher: Arc<Argon2PasswordHasher>,
        token_service: Arc<JwtTokenService>,
        storage_path: PathBuf,
    ) -> Self {
        Self {
            user_storage,
            session_storage,
            password_hasher,
            token_service,
            user_lifecycle: None,
            storage_path,
            oidc: RwLock::new(OidcState {
                service: None,
                config: None,
            }),
            pending_oidc_flows: Cache::builder()
                .max_capacity(10_000)
                .time_to_live(Duration::from_secs(600))
                .build(),
            pending_oidc_tokens: Cache::builder()
                .max_capacity(10_000)
                .time_to_live(Duration::from_secs(60))
                .build(),
            magic_link_repo: None,
        }
    }

    /// Wire the magic-link token repository. Called from the DI factory
    /// when the magic-link feature is configured. Mirrors the
    /// `with_oidc` / `with_user_lifecycle` builder pattern.
    pub fn with_magic_link_repo(mut self, repo: Arc<dyn MagicLinkTokenRepository>) -> Self {
        self.magic_link_repo = Some(repo);
        self
    }

    /// Whether magic-link redemption is wired. Handlers should check this
    /// before attempting to redeem a token; `false` → return 503.
    pub fn magic_link_enabled(&self) -> bool {
        self.magic_link_repo.is_some()
    }

    /// Returns the default quota for the given role, capped to the available
    /// disk space on the filesystem that hosts the storage directory.
    fn capped_quota(&self, role: &UserRole) -> i64 {
        let base_quota = match role {
            UserRole::Admin => DEFAULT_ADMIN_QUOTA,
            _ => DEFAULT_USER_QUOTA,
        };

        match Self::available_disk_space(&self.storage_path) {
            Some(avail) => {
                let avail_i64 = avail as i64;
                if avail_i64 < base_quota {
                    tracing::info!(
                        "Available disk space ({} bytes) is less than default {} quota ({} bytes) — capping quota",
                        avail_i64,
                        if *role == UserRole::Admin {
                            "admin"
                        } else {
                            "user"
                        },
                        base_quota,
                    );
                    avail_i64
                } else {
                    base_quota
                }
            }
            None => {
                tracing::warn!("Could not determine available disk space, using default quota");
                base_quota
            }
        }
    }

    /// Query the available space on the filesystem that contains `path`.
    fn available_disk_space(path: &std::path::Path) -> Option<u64> {
        use fs2::available_space;
        match available_space(path) {
            Ok(space) => Some(space),
            Err(e) => {
                tracing::warn!("Failed to query disk space for {:?}: {}", path, e);
                None
            }
        }
    }

    /// Configures the user-lifecycle dispatcher. Wired by the DI factory
    /// after core services are up. PR 1: only AuditLifecycleHook is
    /// registered, so calls without this configured silently no-op.
    pub fn with_user_lifecycle(mut self, lifecycle: Arc<UserLifecycleService>) -> Self {
        self.user_lifecycle = Some(lifecycle);
        self
    }

    /// Configures the OIDC service
    pub fn with_oidc(self, oidc_service: Arc<OidcService>, oidc_config: OidcConfig) -> Self {
        {
            let mut state = self.oidc.write().unwrap();
            state.service = Some(oidc_service);
            state.config = Some(oidc_config);
        }
        self
    }

    /// Hot-reload OIDC configuration at runtime (called from admin settings service)
    pub fn reload_oidc(&self, oidc_service: Arc<OidcService>, oidc_config: OidcConfig) {
        let mut state = self.oidc.write().unwrap();
        state.service = Some(oidc_service);
        state.config = Some(oidc_config);
    }

    /// Disable OIDC at runtime (called from admin settings service)
    pub fn disable_oidc(&self) {
        let mut state = self.oidc.write().unwrap();
        state.service = None;
        state.config = None;
    }

    /// Returns whether OIDC is configured and enabled
    pub fn oidc_enabled(&self) -> bool {
        let state = self.oidc.read().unwrap();
        state.service.is_some() && state.config.as_ref().is_some_and(|c| c.enabled)
    }

    /// Returns whether password login is disabled (OIDC-only mode)
    pub fn password_login_disabled(&self) -> bool {
        let state = self.oidc.read().unwrap();
        state
            .config
            .as_ref()
            .is_some_and(|c| c.disable_password_login)
    }

    /// Returns a clone of the OIDC config if available
    pub fn oidc_config(&self) -> Option<OidcConfig> {
        let state = self.oidc.read().unwrap();
        state.config.clone()
    }

    /// Returns an Arc clone of the OIDC service if available
    pub fn oidc_service(&self) -> Option<Arc<OidcService>> {
        let state = self.oidc.read().unwrap();
        state.service.clone()
    }

    pub async fn register(&self, dto: RegisterDto) -> Result<UserDto, DomainError> {
        // Check for duplicate user
        if self
            .user_storage
            .get_user_by_username(&dto.username)
            .await
            .is_ok()
        {
            return Err(DomainError::new(
                ErrorKind::AlreadyExists,
                "User",
                format!("User '{}' already exists", dto.username),
            ));
        }

        if self
            .user_storage
            .get_user_by_email(&dto.email)
            .await
            .is_ok()
        {
            return Err(DomainError::new(
                ErrorKind::AlreadyExists,
                "User",
                format!("Email '{}' is already registered", dto.email),
            ));
        }

        // SECURITY: Public registration ALWAYS creates regular users.
        // Admin users can only be created via:
        //   1. The one-time /api/setup endpoint (first boot)
        //   2. The admin panel (admin_create_user)
        let role = UserRole::User;

        // Quota based on role, capped to available disk space
        let quota = self.capped_quota(&role);

        // Validate password length before hashing
        if dto.password.len() < 8 {
            return Err(DomainError::new(
                ErrorKind::InvalidInput,
                "User",
                "Password must be at least 8 characters long",
            ));
        }

        // Hash the password using the infrastructure service
        let password_hash = self.password_hasher.hash_password(&dto.password).await?;

        // Create user with the pre-generated hash
        let user = User::new(dto.username.clone(), dto.email, password_hash, role, quota).map_err(
            |e| {
                DomainError::new(
                    ErrorKind::InvalidInput,
                    "User",
                    format!("Error creating user: {}", e),
                )
            },
        )?;

        // Save user
        let created_user = self.user_storage.create_user(user).await?;

        // Lifecycle: HomeFolderLifecycleHook handles personal-folder
        // creation (was inlined here pre-PR 3); audit log + future
        // provisioning steps land here too.
        if let Some(lc) = &self.user_lifecycle {
            lc.dispatch_created(&created_user).await;
        }

        tracing::info!("User registered: {}", created_user.id());
        Ok(UserDto::from(created_user))
    }

    /// Create the first admin user during initial system setup.
    ///
    /// This is called by the `/api/setup` endpoint after verifying the setup
    /// token. It unconditionally creates an admin user. The caller (handler)
    /// is responsible for:
    ///   1. Verifying the setup token
    ///   2. Checking that the system is not already initialized
    ///   3. Marking the system as initialized after this call succeeds
    pub async fn setup_create_admin(
        &self,
        username: String,
        email: String,
        password: String,
    ) -> Result<UserDto, DomainError> {
        // Validate username
        if username.len() < 3 || username.len() > 254 {
            return Err(DomainError::new(
                ErrorKind::InvalidInput,
                "User",
                "Username must be between 3 and 254 characters".to_string(),
            ));
        }

        // Check for duplicate username
        if self
            .user_storage
            .get_user_by_username(&username)
            .await
            .is_ok()
        {
            return Err(DomainError::new(
                ErrorKind::AlreadyExists,
                "User",
                format!("User '{}' already exists", username),
            ));
        }

        // Check email uniqueness
        if self.user_storage.get_user_by_email(&email).await.is_ok() {
            return Err(DomainError::new(
                ErrorKind::AlreadyExists,
                "User",
                format!("Email '{}' is already registered", email),
            ));
        }

        // Validate password
        if password.len() < 8 {
            return Err(DomainError::new(
                ErrorKind::InvalidInput,
                "User",
                "Password must be at least 8 characters long".to_string(),
            ));
        }

        let role = UserRole::Admin;
        let quota = self.capped_quota(&role);
        let password_hash = self.password_hasher.hash_password(&password).await?;

        let user = User::new(username.clone(), email, password_hash, role, quota).map_err(|e| {
            DomainError::new(
                ErrorKind::InvalidInput,
                "User",
                format!("Error creating admin user: {}", e),
            )
        })?;

        let created_user = self.user_storage.create_user(user).await?;

        // Lifecycle: notify hooks. PR 3 moves home-folder creation into
        // HomeFolderLifecycleHook fired here.
        // Lifecycle: HomeFolderLifecycleHook provisions the admin's
        // home folder. Audit logs the creation event.
        if let Some(lc) = &self.user_lifecycle {
            lc.dispatch_created(&created_user).await;
        }

        tracing::info!(
            "Initial admin created via setup: {} ({})",
            username,
            created_user.id()
        );
        Ok(UserDto::from(created_user))
    }

    pub async fn login(&self, dto: LoginDto) -> Result<AuthResponseDto, DomainError> {
        // Find user
        let mut user = self
            .user_storage
            .get_user_by_username(&dto.username)
            .await
            .map_err(|_| {
                // Audit: unknown-username login attempt. Reason key kept
                // stable so log search can aggregate without parsing the
                // human-readable message. Caller's client IP + request id
                // are attached automatically by the request-scope span.
                tracing::info!(
                    target: "audit",
                    event = "auth.login_rejected",
                    reason = "unknown_user",
                    attempted_username = %dto.username,
                    "🔐 login rejected: no such user '{}'",
                    dto.username,
                );
                DomainError::new(ErrorKind::AccessDenied, "Auth", "Invalid credentials")
            })?;

        // Check if user is active
        if !user.is_active() {
            tracing::info!(
                target: "audit",
                event = "auth.login_rejected",
                reason = "account_deactivated",
                user_id = %user.id(),
                username = %user.username(),
                "🔐 login rejected: account deactivated for '{}'",
                user.username(),
            );
            return Err(DomainError::new(
                ErrorKind::AccessDenied,
                "Auth",
                "Account deactivated",
            ));
        }

        // Verify password using the injected hasher
        let is_valid = self
            .password_hasher
            .verify_password(&dto.password, user.password_hash())
            .await?;

        if !is_valid {
            tracing::info!(
                target: "audit",
                event = "auth.login_rejected",
                reason = "bad_password",
                user_id = %user.id(),
                username = %user.username(),
                "🔐 login rejected: bad password for '{}'",
                user.username(),
            );
            return Err(DomainError::new(
                ErrorKind::AccessDenied,
                "Auth",
                "Invalid credentials",
            ));
        }

        // Lifecycle: dispatch login BEFORE register_login() so hooks
        // observing `last_login_at().is_none()` see "first ever login"
        // correctly. See tip #1 in user_lifecycle.rs.
        if let Some(lc) = &self.user_lifecycle {
            lc.dispatch_login(&user).await;
        }

        // Update last login
        user.register_login();
        self.user_storage.update_user(user.clone()).await?;

        // Generate tokens using the injected token service
        let access_token = self.token_service.generate_access_token(&user)?;

        let refresh_token = self.token_service.generate_refresh_token();

        // Save session — new login starts a new token family
        let session = Session::new(
            user.id(),
            refresh_token.clone(),
            None, // IP (can be added from the HTTP layer)
            None, // User-Agent (can be added from the HTTP layer)
            self.token_service.refresh_token_expiry_days(),
            Uuid::new_v4(),
        );

        self.session_storage.create_session(session).await?;

        // Authentication response
        Ok(AuthResponseDto {
            user: UserDto::from(user),
            access_token,
            refresh_token,
            token_type: "Bearer".to_string(),
            expires_in: self.token_service.refresh_token_expiry_secs(),
        })
    }

    /// Redeem a magic-link token and emit a fresh session in one shot.
    ///
    /// The flow:
    /// 1. Look up the token in the repo. Unknown token → `NotFound`.
    /// 2. Atomically transition `Pending → Used` via the repo's
    ///    `mark_used()` (single SQL UPDATE with `WHERE status='pending'`).
    ///    A second redemption attempt receives `Ok(false)` and is rejected
    ///    as `AccessDenied`.
    /// 3. Load the user, verify they're active.
    /// 4. Dispatch `on_user_login` (so HomeFolderLifecycleHook can
    ///    safety-net any internal user whose first credential happens
    ///    to be a magic link — externals short-circuit by `is_external()`).
    /// 5. Register login + persist + issue session in the same pipeline
    ///    as password login.
    ///
    /// The returned `MagicLinkRedemption` carries the resource target so
    /// the handler can build the redirect URL.
    ///
    /// Returns `ServiceUnavailable` (mapped from `NotImplemented`) when
    /// the magic-link repo isn't wired — the handler maps that to HTTP 503.
    pub async fn redeem_magic_link(&self, token: &str) -> Result<MagicLinkRedemption, DomainError> {
        let repo = self.magic_link_repo.as_ref().ok_or_else(|| {
            DomainError::new(
                ErrorKind::NotImplemented,
                "MagicLink",
                "magic-link feature is not configured on this server",
            )
        })?;

        let mlt = repo.find_by_token(token).await?.ok_or_else(|| {
            // Audit: unknown / forged magic-link redemption. The first
            // 8 chars of the bogus token are logged so a recurring
            // probe pattern is recognisable without dumping the full
            // secret to the log stream.
            let token_preview: String = token.chars().take(8).collect();
            tracing::info!(
                target: "audit",
                event = "magic_link.redemption_rejected",
                reason = "unknown_token",
                token_prefix = %token_preview,
                "🔗 magic-link rejected: unknown token (prefix='{}…')",
                token_preview,
            );
            DomainError::new(
                ErrorKind::NotFound,
                "MagicLink",
                "unknown or invalid magic link",
            )
        })?;

        // Friendly early-rejection messages. The atomic `mark_used`
        // below is the canonical single-use guard.
        if mlt.status() == MagicLinkStatus::Used {
            tracing::info!(
                target: "audit",
                event = "magic_link.redemption_rejected",
                reason = "already_used",
                token_id = %mlt.id(),
                user_id = %mlt.user_id(),
                "🔗 magic-link rejected: token already used for user {}",
                mlt.user_id(),
            );
            return Err(DomainError::new(
                ErrorKind::AccessDenied,
                "MagicLink",
                "this magic link has already been used",
            ));
        }
        if mlt.is_expired() {
            tracing::info!(
                target: "audit",
                event = "magic_link.redemption_rejected",
                reason = "expired",
                token_id = %mlt.id(),
                user_id = %mlt.user_id(),
                "🔗 magic-link rejected: token expired for user {}",
                mlt.user_id(),
            );
            return Err(DomainError::new(
                ErrorKind::AccessDenied,
                "MagicLink",
                "this magic link has expired",
            ));
        }

        let consumed = repo.mark_used(mlt.id()).await?;
        if !consumed {
            // Either a concurrent redemption beat us, or the row was
            // marked expired by the sweeper between our find and update.
            tracing::info!(
                target: "audit",
                event = "magic_link.redemption_rejected",
                reason = "race_or_swept",
                token_id = %mlt.id(),
                user_id = %mlt.user_id(),
                "🔗 magic-link rejected: lost race to mark_used (user {})",
                mlt.user_id(),
            );
            return Err(DomainError::new(
                ErrorKind::AccessDenied,
                "MagicLink",
                "this magic link has already been used",
            ));
        }

        let mut user = self.user_storage.get_user_by_id(mlt.user_id()).await?;
        if !user.is_active() {
            tracing::info!(
                target: "audit",
                event = "magic_link.redemption_rejected",
                reason = "account_deactivated",
                token_id = %mlt.id(),
                user_id = %user.id(),
                username = %user.username(),
                "🔗 magic-link rejected: account deactivated for '{}'",
                user.username(),
            );
            return Err(DomainError::new(
                ErrorKind::AccessDenied,
                "Auth",
                "Account deactivated",
            ));
        }

        // Dispatch BEFORE register_login so hooks observing
        // `last_login_at().is_none()` see "first ever login" correctly.
        if let Some(lc) = &self.user_lifecycle {
            lc.dispatch_login(&user).await;
        }
        user.register_login();
        self.user_storage.update_user(user.clone()).await?;

        let access_token = self.token_service.generate_access_token(&user)?;
        let refresh_token = self.token_service.generate_refresh_token();
        let session = Session::new(
            user.id(),
            refresh_token.clone(),
            None,
            None,
            self.token_service.refresh_token_expiry_days(),
            Uuid::new_v4(),
        );
        self.session_storage.create_session(session).await?;

        tracing::info!(
            target: "audit",
            event = "magic_link.redeemed",
            user_id = %user.id(),
            username = %user.username(),
            is_external = user.is_external(),
            resource_kind = ?mlt.resource_kind(),
            resource_id = ?mlt.resource_id(),
        );

        let auth = AuthResponseDto {
            user: UserDto::from(user),
            access_token,
            refresh_token,
            token_type: "Bearer".to_string(),
            expires_in: self.token_service.refresh_token_expiry_secs(),
        };

        Ok(MagicLinkRedemption {
            auth,
            resource_kind: mlt.resource_kind(),
            resource_id: mlt.resource_id(),
        })
    }

    /// Verifies username/password credentials without creating a session.
    pub async fn verify_credentials(
        &self,
        username: &str,
        password: &str,
    ) -> Result<crate::application::dtos::user_dto::CurrentUser, DomainError> {
        let user = self
            .user_storage
            .get_user_by_username(username)
            .await
            .map_err(|_| {
                DomainError::new(ErrorKind::AccessDenied, "Auth", "Invalid credentials")
            })?;

        if !user.is_active() {
            return Err(DomainError::new(
                ErrorKind::AccessDenied,
                "Auth",
                "Account deactivated",
            ));
        }

        let is_valid = self
            .password_hasher
            .verify_password(password, user.password_hash())
            .await?;

        if !is_valid {
            return Err(DomainError::new(
                ErrorKind::AccessDenied,
                "Auth",
                "Invalid credentials",
            ));
        }

        Ok(crate::application::dtos::user_dto::CurrentUser {
            id: user.id(),
            username: user.username().to_string(),
            email: user.email().to_string(),
            role: user.role().to_string(),
        })
    }

    pub async fn refresh_token(
        &self,
        dto: RefreshTokenDto,
    ) -> Result<AuthResponseDto, DomainError> {
        // Get valid session
        let session = self
            .session_storage
            .get_session_by_refresh_token(&dto.refresh_token)
            .await?;

        // Reuse detection: a revoked token being replayed indicates the token was
        // stolen after rotation. Invalidate the entire family to protect all devices.
        if session.is_revoked() {
            tracing::warn!(
                user_id = %session.user_id(),
                family_id = %session.family_id(),
                "Refresh token reuse detected — revoking entire token family"
            );
            self.session_storage
                .revoke_session_family(session.family_id())
                .await?;
            // Lifecycle: TokenReused logout — fired once per logical
            // revoke-family call. PR 4 may refine to per-session firing.
            if let Some(lc) = &self.user_lifecycle
                && let Ok(user) = self.user_storage.get_user_by_id(session.user_id()).await
            {
                lc.dispatch_logout(user, LogoutReason::TokenReused);
            }
            return Err(DomainError::new(
                ErrorKind::AccessDenied,
                "Auth",
                "Session expired or invalid",
            ));
        }

        if session.is_expired() {
            return Err(DomainError::new(
                ErrorKind::AccessDenied,
                "Auth",
                "Session expired or invalid",
            ));
        }

        // Get user
        let user = self.user_storage.get_user_by_id(session.user_id()).await?;

        // Check if user is active
        if !user.is_active() {
            return Err(DomainError::new(
                ErrorKind::AccessDenied,
                "Auth",
                "Account deactivated",
            ));
        }

        // Revoke current session before issuing the next token in the family
        self.session_storage.revoke_session(session.id()).await?;

        // Generate new tokens
        let access_token = self.token_service.generate_access_token(&user)?;
        let new_refresh_token = self.token_service.generate_refresh_token();

        // New session inherits the family_id so reuse of any ancestor triggers
        // full-family revocation
        let new_session = Session::new(
            user.id(),
            new_refresh_token.clone(),
            None,
            None,
            self.token_service.refresh_token_expiry_days(),
            session.family_id(),
        );

        self.session_storage.create_session(new_session).await?;

        Ok(AuthResponseDto {
            user: UserDto::from(user),
            access_token,
            refresh_token: new_refresh_token,
            token_type: "Bearer".to_string(),
            expires_in: self.token_service.refresh_token_expiry_secs(),
        })
    }

    pub async fn logout(&self, user_id: Uuid, refresh_token: &str) -> Result<(), DomainError> {
        // Get session
        let session = match self
            .session_storage
            .get_session_by_refresh_token(refresh_token)
            .await
        {
            Ok(s) => s,
            // If the session doesn't exist, we consider the logout successful
            Err(_) => return Ok(()),
        };

        // Verify that the session belongs to the user
        if session.user_id() != user_id {
            return Err(DomainError::new(
                ErrorKind::AccessDenied,
                "Auth",
                "The session does not belong to the user",
            ));
        }

        // Revoke session
        self.session_storage.revoke_session(session.id()).await?;

        // Lifecycle: notify hooks. One extra DB roundtrip per logout
        // (user load) is acceptable — logout is rare. Failure to load
        // the user is non-fatal: we already revoked the session.
        if let Some(lc) = &self.user_lifecycle
            && let Ok(user) = self.user_storage.get_user_by_id(user_id).await
        {
            lc.dispatch_logout(user, LogoutReason::UserInitiated);
        }

        Ok(())
    }

    pub async fn logout_all(&self, user_id: Uuid) -> Result<u64, DomainError> {
        // Revoke all user sessions
        let revoked_count = self
            .session_storage
            .revoke_all_user_sessions(user_id)
            .await?;

        Ok(revoked_count)
    }

    pub async fn change_password(
        &self,
        user_id: Uuid,
        dto: ChangePasswordDto,
    ) -> Result<(), DomainError> {
        // Get user
        let mut user = self.user_storage.get_user_by_id(user_id).await?;

        // Block password changes for OIDC-provisioned users
        if user.is_oidc_user() {
            return Err(DomainError::new(
                ErrorKind::AccessDenied,
                "Auth",
                "Password changes are not available for SSO/OIDC accounts. Your password is managed by your identity provider.",
            ));
        }

        // Verify current password using the injected hasher
        let is_valid = self
            .password_hasher
            .verify_password(&dto.current_password, user.password_hash())
            .await?;

        if !is_valid {
            return Err(DomainError::new(
                ErrorKind::AccessDenied,
                "Auth",
                "Current password is incorrect",
            ));
        }

        // Validate new password
        if dto.new_password.len() < 8 {
            return Err(DomainError::new(
                ErrorKind::InvalidInput,
                "User",
                "Password must be at least 8 characters long",
            ));
        }

        // Hash new password and update user
        let new_hash = self
            .password_hasher
            .hash_password(&dto.new_password)
            .await?;
        user.update_password_hash(new_hash);

        // Save updated user
        self.user_storage.update_user(user.clone()).await?;

        // Optional: revoke all sessions to force re-login with new password
        self.session_storage
            .revoke_all_user_sessions(user_id)
            .await?;

        // Lifecycle: PasswordChanged logout — fired once per logical
        // revoke-all call. PR 4 may refine to per-session firing.
        if let Some(lc) = &self.user_lifecycle {
            lc.dispatch_logout(user, LogoutReason::PasswordChanged);
        }

        Ok(())
    }

    /// Update the profile image for a non-OIDC user.
    pub async fn update_user_image(
        &self,
        caller_id: Uuid,
        image: Option<String>,
    ) -> Result<(), DomainError> {
        let user = self.user_storage.get_user_by_id(caller_id).await?;

        if user.is_oidc_user() {
            return Err(DomainError::new(
                ErrorKind::AccessDenied,
                "User",
                "Avatar is managed by your identity provider and cannot be changed here",
            ));
        }

        if let Some(ref img) = image {
            const MAX_BYTES: usize = 524_288; // 512 KiB
            if img.len() > MAX_BYTES {
                return Err(DomainError::new(
                    ErrorKind::InvalidInput,
                    "User",
                    "Image exceeds maximum allowed size (512 KiB)",
                ));
            }
            let valid = img.starts_with("https://")
                || img.starts_with("http://")
                || img.starts_with("data:image/png;base64,")
                || img.starts_with("data:image/webp;base64,")
                || img.starts_with("data:image/jpeg;base64,");
            if !valid {
                return Err(DomainError::new(
                    ErrorKind::InvalidInput,
                    "User",
                    "Image must be an https/http URL or a data URI (png, webp, jpeg)",
                ));
            }
        }

        self.user_storage
            .update_image(caller_id, image)
            .await
            .map_err(DomainError::from)?;

        Ok(())
    }

    pub async fn get_user(&self, user_id: Uuid) -> Result<UserDto, DomainError> {
        let user = self.user_storage.get_user_by_id(user_id).await?;
        Ok(UserDto::from(user))
    }

    // Alias for consistency with handler method
    pub async fn get_user_by_id(&self, user_id: Uuid) -> Result<UserDto, DomainError> {
        self.get_user(user_id).await
    }

    /// Visibility-checked profile lookup for `GET /api/users/{id}`.
    ///
    /// Returns `NotFound` (not `AccessDenied`) when the caller has no
    /// legitimate relationship with the target — anti-enumeration: an
    /// attacker probing random UUIDs cannot distinguish "user doesn't
    /// exist" from "exists but you can't see them".
    ///
    /// Visibility rule, evaluated top-to-bottom:
    ///   1. **Self lookup** — `caller_id == target_id` always succeeds.
    ///   2. **Shared-grant relationship** — caller and target appear
    ///      together on at least one row of `storage.access_grants`,
    ///      either direction (caller-as-granter / target-as-subject,
    ///      or target-as-granter / caller-as-subject). Applies to both
    ///      internal and external callers. This is what lets an
    ///      external user resolve the display name + photo of the
    ///      internal user who shared a folder with them — the
    ///      `granted_by` column on the grant Bob received is Alice's
    ///      user_id, and SharedWithMe needs to render her vignette.
    ///   3. **External callers stop here.** Any remaining check would
    ///      let them enumerate the user directory; they have no
    ///      legitimate need beyond resolving people they're already in
    ///      a grant relationship with.
    ///   4. *(Internal callers only)* Target is internal AND
    ///      `expose_system_users` is on → already broadly visible via
    ///      the system address book; no extra check.
    ///   5. *(Internal callers only)* Caller is admin → always visible.
    ///   6. Anything else → `NotFound`.
    ///
    /// Subject-group co-membership is intentionally NOT a visibility
    /// path in v1; can be added later if a concrete need surfaces.
    pub async fn get_user_profile(
        &self,
        caller_id: Uuid,
        target_id: Uuid,
        expose_system_users: bool,
        pool: &sqlx::PgPool,
    ) -> Result<UserDto, DomainError> {
        let caller = self.user_storage.get_user_by_id(caller_id).await?;

        // (1) Self.
        if caller_id == target_id {
            return Ok(UserDto::from(caller));
        }

        // Anti-enumeration: NotFound for everything that doesn't pass.
        // Convert a real NotFound on `target` to the same anonymous 404,
        // so existence isn't leaked through differential responses.
        let target = match self.user_storage.get_user_by_id(target_id).await {
            Ok(u) => u,
            Err(e) if e.kind == ErrorKind::NotFound => {
                tracing::info!(
                    target: "audit",
                    event = "user_profile.rejected",
                    reason = "target_not_found",
                    caller_id = %caller_id,
                    caller_is_external = caller.is_external(),
                    target_id = %target_id,
                    "👮🏻‍♂️ user-profile rejected: target '{}' does not exist (caller {})",
                    target_id,
                    caller_id,
                );
                return Err(DomainError::new(
                    ErrorKind::NotFound,
                    "User",
                    "User not found",
                ));
            }
            Err(e) => return Err(e),
        };

        // (2) Shared-grant relationship — works for both internal and
        // external callers. LIMIT 1 + the (granted_by) and
        // (subject_type, subject_id) indexes keep this cheap.
        let related: Option<i32> = sqlx::query_scalar(
            r#"
            SELECT 1
              FROM storage.access_grants
             WHERE (granted_by = $1 AND subject_type = 'user' AND subject_id = $2)
                OR (granted_by = $2 AND subject_type = 'user' AND subject_id = $1)
             LIMIT 1
            "#,
        )
        .bind(caller_id)
        .bind(target_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| {
            DomainError::internal_error("UserProfile", format!("visibility query: {}", e))
        })?;

        if related.is_some() {
            return Ok(UserDto::from(target));
        }

        // (3) External callers stop here — no directory enumeration.
        if caller.is_external() {
            // Audit: an external user tried to look up someone they
            // don't share a grant with. Surfaces enumeration probes
            // from compromised magic-link sessions.
            tracing::info!(
                target: "audit",
                event = "user_profile.rejected",
                reason = "external_caller_no_relationship",
                caller_id = %caller_id,
                target_id = %target_id,
                target_is_external = target.is_external(),
                "👮🏻‍♂️ user-profile rejected: external user '{}' has no grant relationship with '{}'",
                caller_id,
                target_id,
            );
            return Err(DomainError::new(
                ErrorKind::NotFound,
                "User",
                "User not found",
            ));
        }

        // (4) Internal target + system-address-book exposed: already public.
        if !target.is_external() && expose_system_users {
            return Ok(UserDto::from(target));
        }

        // (5) Admin caller: always visible.
        if caller.role() == UserRole::Admin {
            return Ok(UserDto::from(target));
        }

        // (6) No relationship — anti-enumeration NotFound.
        // Audit: an internal user with no visibility path probed a user
        // they don't share with. Usually benign (stale UI state), but
        // recurring patterns from the same caller are worth surfacing.
        tracing::info!(
            target: "audit",
            event = "user_profile.rejected",
            reason = "no_visibility_path",
            caller_id = %caller_id,
            target_id = %target_id,
            target_is_external = target.is_external(),
            "👮🏻‍♂️ user-profile rejected: internal user '{}' has no visibility on '{}' (target is_external={})",
            caller_id,
            target_id,
            target.is_external(),
        );
        Err(DomainError::new(
            ErrorKind::NotFound,
            "User",
            "User not found",
        ))
    }

    // New method to get user by username - needed for admin user handling
    pub async fn get_user_by_username(&self, username: &str) -> Result<UserDto, DomainError> {
        let user = self.user_storage.get_user_by_username(username).await?;
        Ok(UserDto::from(user))
    }

    // Method to count how many admin users exist in the system
    // Used to determine if we have multiple admins or just the default one
    pub async fn count_admin_users(&self) -> Result<i64, DomainError> {
        // Use the list_users_by_role method or similar from user_storage port
        // For now, we'll use a basic implementation that counts all users with role = "admin"
        let admin_users = self
            .user_storage
            .list_users_by_role("admin")
            .await
            .map_err(|e| {
                DomainError::new(
                    ErrorKind::InternalError,
                    "User",
                    format!("Error counting admin users: {}", e),
                )
            })?;

        Ok(admin_users.len() as i64)
    }

    /// Lists internal users only. External (grant-only) users are filtered
    /// out so that internal-user surfaces — system address book, OCS
    /// sharee search, etc. — never expose external identities. Admin
    /// surfaces that need the full list should call
    /// [`list_users_including_external`] instead.
    pub async fn list_users(&self, limit: i64, offset: i64) -> Result<Vec<UserDto>, DomainError> {
        let users = self.user_storage.list_users(limit, offset, false).await?;
        Ok(users.into_iter().map(UserDto::from).collect())
    }

    /// Admin-only: lists users including external (grant-only) recipients.
    /// Used by the admin user-management UI.
    pub async fn list_users_including_external(
        &self,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<UserDto>, DomainError> {
        let users = self.user_storage.list_users(limit, offset, true).await?;
        Ok(users.into_iter().map(UserDto::from).collect())
    }

    /// Searches internal users only. See [`list_users`] for the rationale.
    pub async fn search_users(&self, query: &str, limit: i64) -> Result<Vec<UserDto>, DomainError> {
        let users = self.user_storage.search_users(query, limit, false).await?;
        Ok(users.into_iter().map(UserDto::from).collect())
    }

    // ========================================================================
    // Admin User Management Methods
    // ========================================================================

    /// Admin-only: create a user bypassing registration guards.
    pub async fn admin_create_user(
        &self,
        dto: crate::application::dtos::settings_dto::AdminCreateUserDto,
    ) -> Result<UserDto, DomainError> {
        // Validate username length
        if dto.username.len() < 3 || dto.username.len() > 254 {
            return Err(DomainError::new(
                ErrorKind::InvalidInput,
                "User",
                "Username must be between 3 and 254 characters".to_string(),
            ));
        }

        // Check for duplicate username
        if self
            .user_storage
            .get_user_by_username(&dto.username)
            .await
            .is_ok()
        {
            return Err(DomainError::new(
                ErrorKind::AlreadyExists,
                "User",
                format!("User '{}' already exists", dto.username),
            ));
        }

        // Email: use provided or generate placeholder
        let email = dto
            .email
            .filter(|e| !e.trim().is_empty())
            .unwrap_or_else(|| format!("{}@oxicloud.local", dto.username));

        // Check email uniqueness
        if self.user_storage.get_user_by_email(&email).await.is_ok() {
            return Err(DomainError::new(
                ErrorKind::AlreadyExists,
                "User",
                format!("Email '{}' is already registered", email),
            ));
        }

        // Validate password
        if dto.password.len() < 8 {
            return Err(DomainError::new(
                ErrorKind::InvalidInput,
                "User",
                "Password must be at least 8 characters long".to_string(),
            ));
        }

        // Determine role
        let role = match dto.role.as_deref() {
            Some("admin") => UserRole::Admin,
            _ => UserRole::User,
        };

        let is_external = dto.is_external.unwrap_or(false);

        // Forbid external + admin combo. The DB `users_external_not_admin`
        // CHECK constraint would catch this too, but a 400 with an
        // explanatory message is friendlier than a generic 500 from a
        // constraint violation. See the CHECK definition in
        // migrations/20260612000002_auth_users_is_external.sql for the
        // rationale.
        if is_external && matches!(role, UserRole::Admin) {
            return Err(DomainError::new(
                ErrorKind::InvalidInput,
                "User",
                "External users cannot be admins. To promote an external user to admin, \
                 first convert them to internal (set is_external = false), then update \
                 the role separately."
                    .to_string(),
            ));
        }

        // External users never own storage. The DB `users_external_no_storage`
        // CHECK constraint enforces this; setting quota=0 here keeps the
        // domain consistent and matches `User::new_external`.
        let quota = if is_external {
            0
        } else {
            dto.quota_bytes.unwrap_or_else(|| self.capped_quota(&role))
        };

        // Hash password (kept for both internal and external users — for
        // external users it's currently unused since they authenticate via
        // magic-link / OIDC, but the DB column is NOT NULL).
        let password_hash = self.password_hasher.hash_password(&dto.password).await?;

        // Create domain entity. External path uses `new_external` so the
        // is_external flag is set + the EXTERNAL placeholder password
        // marker is applied for clarity in DB inspection. `new_external`
        // forces role=User (the admin+external combo was rejected above).
        let user = if is_external {
            User::new_external(dto.username.clone(), email).map(|mut u| {
                // The hashed password from the request is unused for auth
                // but is persisted so audit-trail integrity is preserved.
                u.update_password_hash(password_hash);
                u
            })
        } else {
            User::new(dto.username.clone(), email, password_hash, role, quota)
        }
        .map_err(|e| {
            DomainError::new(
                ErrorKind::InvalidInput,
                "User",
                format!("Error creating user: {}", e),
            )
        })?;

        // Persist
        let created = self.user_storage.create_user(user).await?;

        // Deactivate if requested (User::new always sets active=true)
        if let Some(false) = dto.active {
            self.user_storage
                .set_user_active_status(created.id(), false)
                .await?;
        }

        // Lifecycle: HomeFolderLifecycleHook handles the home-folder
        // provisioning (idempotent + short-circuits on is_external).
        // Audit logs the creation event.
        if let Some(lc) = &self.user_lifecycle {
            lc.dispatch_created(&created).await;
        }

        tracing::info!(
            "Admin created user: {} ({}, is_external={})",
            dto.username,
            created.id(),
            created.is_external()
        );
        Ok(UserDto::from(created))
    }

    /// Admin-only: reset a user's password.
    pub async fn admin_reset_password(
        &self,
        user_id: Uuid,
        new_password: &str,
    ) -> Result<(), DomainError> {
        // Block password reset for OIDC-provisioned users
        let user = self.user_storage.get_user_by_id(user_id).await?;
        if user.is_oidc_user() {
            return Err(DomainError::new(
                ErrorKind::InvalidInput,
                "Auth",
                "Cannot reset password for SSO/OIDC accounts. The user's password is managed by their identity provider.",
            ));
        }

        if new_password.len() < 8 {
            return Err(DomainError::new(
                ErrorKind::InvalidInput,
                "User",
                "Password must be at least 8 characters long".to_string(),
            ));
        }
        let hash = self.password_hasher.hash_password(new_password).await?;
        self.user_storage.change_password(user_id, &hash).await?;

        // Invalidate all existing sessions so the user must re-login
        // with the new password.  Mirrors the behaviour of change_password().
        self.session_storage
            .revoke_all_user_sessions(user_id)
            .await?;

        tracing::info!(user_id = %user_id, "Admin reset password — all sessions revoked");
        Ok(())
    }

    /// Get a single user by ID (for admin panel)
    pub async fn get_user_admin(&self, user_id: Uuid) -> Result<UserDto, DomainError> {
        let user = self.user_storage.get_user_by_id(user_id).await?;
        Ok(UserDto::from(user))
    }

    /// Delete a user by ID (admin only).
    ///
    /// Runs the whole flow in a single transaction so the lifecycle
    /// hooks (`SessionRevocationLifecycleHook` revoking sessions with
    /// audit, `AuthzCacheLifecycleHook` invalidating the Moka cache,
    /// `HomeFolderLifecycleHook` for future trash policy, …) can do
    /// their work atomically with the user DELETE. If any hook returns
    /// `Err`, the transaction rolls back and the user remains intact.
    pub async fn delete_user_admin(&self, user_id: Uuid) -> Result<(), DomainError> {
        let user = self.user_storage.get_user_by_id(user_id).await?;
        tracing::info!("Admin deleting user: {} ({})", user.username(), user_id);

        let mut tx = self
            .user_storage
            .pool()
            .begin()
            .await
            .map_err(|e| DomainError::internal_error("Auth", format!("begin tx: {}", e)))?;

        // Hooks run inside the tx, BEFORE the user DELETE. They see the
        // row still present and can write cleanup queries against the
        // same tx (e.g. session revocation with per-session audit).
        if let Some(lc) = &self.user_lifecycle {
            lc.dispatch_deleted(&user, DeletionMode::AdminDelete, &mut tx)
                .await?;
        }

        // Now the DELETE — FK CASCADE handles the downstream cleanup
        // (sessions, folders, files, …) for anything the hooks didn't
        // explicitly remove.
        sqlx::query("DELETE FROM auth.users WHERE id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| DomainError::internal_error("Auth", format!("delete user: {}", e)))?;

        tx.commit()
            .await
            .map_err(|e| DomainError::internal_error("Auth", format!("commit: {}", e)))?;

        Ok(())
    }

    /// Activate or deactivate a user (admin only)
    pub async fn set_user_active(&self, user_id: Uuid, active: bool) -> Result<(), DomainError> {
        self.user_storage
            .set_user_active_status(user_id, active)
            .await
    }

    /// Change user role (admin only)
    pub async fn change_user_role(&self, user_id: Uuid, role: &str) -> Result<(), DomainError> {
        if role != "admin" && role != "user" {
            return Err(DomainError::new(
                ErrorKind::InvalidInput,
                "User",
                format!("Invalid role: {}. Must be 'admin' or 'user'", role),
            ));
        }
        self.user_storage.change_role(user_id, role).await
    }

    /// Update user's storage quota (admin only)
    pub async fn update_user_quota(
        &self,
        user_id: Uuid,
        quota_bytes: i64,
    ) -> Result<(), DomainError> {
        if quota_bytes < 0 {
            return Err(DomainError::new(
                ErrorKind::InvalidInput,
                "User",
                "Quota must be non-negative".to_string(),
            ));
        }
        self.user_storage
            .update_storage_quota(user_id, quota_bytes)
            .await
    }

    /// Check if a user has enough quota for an upload of the given size
    pub async fn check_quota(
        &self,
        user_id: Uuid,
        additional_bytes: i64,
    ) -> Result<bool, DomainError> {
        let user = self.user_storage.get_user_by_id(user_id).await?;
        let quota = user.storage_quota_bytes();
        if quota <= 0 {
            // 0 or negative means unlimited
            return Ok(true);
        }
        Ok(user.storage_used_bytes() + additional_bytes <= quota)
    }

    /// Count users efficiently
    pub async fn count_users_efficient(&self) -> Result<i64, DomainError> {
        self.user_storage.count_users().await
    }

    // ========================================================================
    // OIDC Methods
    // ========================================================================

    /// Prepare the OIDC authorization flow: generates CSRF state, PKCE pair,
    /// nonce, stores them in pending_oidc_flows, and returns the authorize URL.
    pub async fn prepare_oidc_authorize(&self) -> Result<String, DomainError> {
        let oidc = self.oidc_service().ok_or_else(|| {
            DomainError::new(
                ErrorKind::InternalError,
                "OIDC",
                "OIDC service not configured",
            )
        })?;

        // Generate CSRF state token
        use rand_core::{OsRng, RngCore};
        let mut state_bytes = [0u8; 32];
        OsRng.fill_bytes(&mut state_bytes);
        let state_token = hex::encode(state_bytes);

        // Generate nonce for ID token binding
        let mut nonce_bytes = [0u8; 32];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = hex::encode(nonce_bytes);

        // Generate PKCE pair (RFC 7636, S256)
        let mut verifier_bytes = [0u8; 32];
        OsRng.fill_bytes(&mut verifier_bytes);
        let pkce_verifier = base64_url_encode(&verifier_bytes);
        let pkce_challenge = {
            use sha2::{Digest, Sha256};
            let hash = Sha256::digest(pkce_verifier.as_bytes());
            base64_url_encode(&hash)
        };

        // Store pending flow (auto-expires after 10 min via moka TTL)
        self.pending_oidc_flows.insert(
            state_token.clone(),
            PendingOidcFlow {
                pkce_verifier,
                nonce: nonce.clone(),
                nc_flow_token: None,
            },
        );

        // Build authorization URL with state, nonce, and PKCE challenge
        let authorize_url = oidc
            .get_authorize_url(&state_token, &nonce, &pkce_challenge)
            .await?;

        tracing::info!(
            "OIDC authorize flow prepared (state={}...)",
            &state_token[..8]
        );

        Ok(authorize_url)
    }

    /// Prepare an OIDC authorization flow for a Nextcloud Login Flow v2 session.
    ///
    /// Works like [`prepare_oidc_authorize`] but associates the Nextcloud flow
    /// token with the OIDC state so that [`oidc_callback`] can complete the
    /// Nextcloud login flow (app-password + poll result) instead of issuing
    /// internal JWTs.
    pub async fn prepare_oidc_authorize_for_nextcloud(
        &self,
        nc_flow_token: &str,
    ) -> Result<String, DomainError> {
        let oidc = self.oidc_service().ok_or_else(|| {
            DomainError::new(
                ErrorKind::InternalError,
                "OIDC",
                "OIDC service not configured",
            )
        })?;

        use rand_core::{OsRng, RngCore};
        let mut state_bytes = [0u8; 32];
        OsRng.fill_bytes(&mut state_bytes);
        let state_token = hex::encode(state_bytes);

        let mut nonce_bytes = [0u8; 32];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = hex::encode(nonce_bytes);

        let mut verifier_bytes = [0u8; 32];
        OsRng.fill_bytes(&mut verifier_bytes);
        let pkce_verifier = base64_url_encode(&verifier_bytes);
        let pkce_challenge = {
            use sha2::{Digest, Sha256};
            let hash = Sha256::digest(pkce_verifier.as_bytes());
            base64_url_encode(&hash)
        };

        // Store pending flow (auto-expires after 10 min via moka TTL)
        self.pending_oidc_flows.insert(
            state_token.clone(),
            PendingOidcFlow {
                pkce_verifier,
                nonce: nonce.clone(),
                nc_flow_token: Some(nc_flow_token.to_string()),
            },
        );

        let authorize_url = oidc
            .get_authorize_url(&state_token, &nonce, &pkce_challenge)
            .await?;

        tracing::info!(
            "OIDC authorize flow prepared for Nextcloud Login Flow v2 (state={}...)",
            &state_token[..8]
        );

        Ok(authorize_url)
    }

    /// Handle the OIDC callback: validate CSRF state, exchange code with PKCE,
    /// validate ID token nonce, find or create user (JIT provisioning),
    /// issue internal tokens, and return a one-time exchange code.
    ///
    /// If the pending flow carries a Nextcloud flow token, this method returns
    /// `Err(NcOidcComplete { .. })` with a special error kind so the handler
    /// layer can complete the Nextcloud flow instead.
    pub async fn oidc_callback(
        &self,
        code: &str,
        state: &str,
    ) -> Result<OidcCallbackResult, DomainError> {
        // 0. Validate CSRF state and retrieve PKCE verifier + nonce + optional NC token
        //    (entry is auto-expired by moka TTL — remove returns None if expired)
        let flow = self.pending_oidc_flows.remove(state).ok_or_else(|| {
            tracing::warn!("OIDC callback with invalid/expired state token");
            DomainError::new(
                ErrorKind::AccessDenied, "OIDC",
                "Invalid or expired OIDC state — possible CSRF attack. Please try logging in again.",
            )
        })?;
        let (pkce_verifier, nonce, nc_flow_token) =
            (flow.pkce_verifier, flow.nonce, flow.nc_flow_token);

        // Clone the Arc and config out of the RwLock so we don't hold the lock across await points
        let (oidc, oidc_config) = {
            let state = self.oidc.read().unwrap();
            let svc = state.service.clone().ok_or_else(|| {
                DomainError::new(
                    ErrorKind::InternalError,
                    "OIDC",
                    "OIDC service not configured",
                )
            })?;
            let cfg = state.config.clone().ok_or_else(|| {
                DomainError::new(
                    ErrorKind::InternalError,
                    "OIDC",
                    "OIDC config not available",
                )
            })?;
            (svc, cfg)
        };

        // 1. Exchange authorization code for tokens (with PKCE verifier)
        let token_set = oidc.exchange_code(code, &pkce_verifier).await?;

        // 2. Validate ID token and extract claims (with nonce verification)
        let claims = oidc
            .validate_id_token(&token_set.id_token, Some(&nonce))
            .await?;

        // 3. Try to enrich claims from UserInfo endpoint if email is missing
        let claims = if claims.email.is_none() {
            match oidc.fetch_user_info(&token_set.access_token).await {
                Ok(user_info) => OidcIdClaims {
                    email: user_info.email.or(claims.email),
                    preferred_username: user_info.preferred_username.or(claims.preferred_username),
                    name: user_info.name.or(claims.name),
                    given_name: user_info.given_name.or(claims.given_name),
                    family_name: user_info.family_name.or(claims.family_name),
                    email_verified: user_info.email_verified.or(claims.email_verified),
                    groups: if user_info.groups.is_empty() {
                        claims.groups
                    } else {
                        user_info.groups
                    },
                    ..claims
                },
                Err(e) => {
                    tracing::warn!(
                        "Failed to fetch UserInfo (continuing with ID token claims): {}",
                        e
                    );
                    claims
                }
            }
        } else {
            claims
        };

        let provider_name = oidc.provider_name().to_string();
        // Check email_verified - only if email is present in claims
        if let Some(email) = &claims.email {
            let verified = claims.email_verified.unwrap_or(false);
            if !verified {
                tracing::warn!(
                    "OIDC login rejected: email not verified (provider: {}, email: {})",
                    provider_name,
                    email
                );
                return Err(DomainError::new(
                    ErrorKind::AccessDenied,
                    "OIDC",
                    "Email verification required. Please verify your email at the identity provider.",
                ));
            }
        }

        // 4. Determine username and email
        let oidc_username = claims
            .preferred_username
            .clone()
            .or(claims.name.clone())
            .unwrap_or_else(|| format!("oidc_{}", &claims.sub[..8.min(claims.sub.len())]));
        let oidc_email = claims
            .email
            .clone()
            .unwrap_or_else(|| format!("{}@oidc.local", oidc_username));

        // 5. Look up existing user by OIDC subject
        let user = match self
            .user_storage
            .get_user_by_oidc_subject(&provider_name, &claims.sub)
            .await
        {
            Ok(mut existing_user) => {
                // User exists — dispatch login BEFORE register_login() so
                // hooks observe `last_login_at = None` on the very first
                // login (see tip #1 in the trait docstring).
                if let Some(lc) = &self.user_lifecycle {
                    lc.dispatch_login(&existing_user).await;
                }
                existing_user.register_login();
                existing_user.set_image(claims.picture.clone());
                self.user_storage.update_user(existing_user.clone()).await?;
                existing_user
            }
            Err(_) => {
                // User doesn't exist — try to match by email
                let matched_user = self.user_storage.get_user_by_email(&oidc_email).await.ok();

                if let Some(_existing) = matched_user {
                    // Email match but no OIDC link — for security, don't auto-link
                    return Err(DomainError::new(
                        ErrorKind::AlreadyExists,
                        "OIDC",
                        format!(
                            "A user with email '{}' already exists. Contact admin to link your OIDC identity.",
                            oidc_email
                        ),
                    ));
                }

                // No match — JIT provision if enabled
                if !oidc_config.auto_provision {
                    return Err(DomainError::new(
                        ErrorKind::AccessDenied,
                        "OIDC",
                        "Auto-provisioning is disabled. Contact admin to create your account.",
                    ));
                }

                // Determine role from OIDC groups
                let role = self.map_oidc_role(&claims.groups, &oidc_config);

                let quota = self.capped_quota(&role);

                // Sanitize username: if it looks like an email, extract the local part
                // (some OIDC providers like Keycloak use email as the preferred username)
                let base_username = if oidc_username.contains('@') {
                    oidc_username.split('@').next().unwrap_or(&oidc_username)
                } else {
                    &oidc_username
                };

                // Filter to valid username characters only, then truncate to 32 chars
                let mut username = base_username
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
                    .take(32)
                    .collect::<String>();

                // Filter helper: removes any chars that are not valid in a username
                let filter_username_chars = |s: &str| {
                    s.chars()
                        .filter(|c| {
                            c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.'
                        })
                        .take(32)
                        .collect::<String>()
                };

                // Ensure minimum length (the padding suffix must also be filtered)
                if username.len() < 3 {
                    let filtered_sub = filter_username_chars(&claims.sub);
                    username = format!("user_{}", &filtered_sub[..filtered_sub.len().min(8)]);
                }

                // Check for username collision
                if self
                    .user_storage
                    .get_user_by_username(&username)
                    .await
                    .is_ok()
                {
                    let filtered_sub = filter_username_chars(&claims.sub);
                    let suffix = &filtered_sub[..filtered_sub.len().min(4)];
                    username = format!("{}_{}", &username[..username.len().min(27)], suffix);
                }

                let mut new_user = User::new_oidc(
                    username.clone(),
                    oidc_email,
                    role,
                    quota,
                    provider_name.clone(),
                    claims.sub.clone(),
                )
                .map_err(|e| {
                    DomainError::new(
                        ErrorKind::InvalidInput,
                        "OIDC",
                        format!("Failed to create OIDC user: {}", e),
                    )
                })?;
                new_user.set_image(claims.picture.clone());
                new_user.set_given_name(claims.given_name.clone());
                new_user.set_family_name(claims.family_name.clone());

                let created_user = self.user_storage.create_user(new_user).await?;

                // Lifecycle: created (audit + home-folder provisioning) +
                // login (no register_login() for a fresh OIDC user means
                // `last_login_at` is naturally None → first-login detection
                // works). HomeFolderLifecycleHook creates the home folder.
                if let Some(lc) = &self.user_lifecycle {
                    lc.dispatch_created(&created_user).await;
                    lc.dispatch_login(&created_user).await;
                }

                tracing::info!(
                    "OIDC user provisioned: {} (provider: {}, sub: {})",
                    created_user.id(),
                    provider_name,
                    claims.sub
                );

                created_user
            }
        };

        // ── Branch: Nextcloud Login Flow v2 vs regular web login ──
        if let Some(nc_token) = nc_flow_token {
            // Nextcloud path: return user info so the handler can mint an
            // app-password and complete the NC login flow.
            tracing::info!(
                user = %user.username(),
                "OIDC login successful for Nextcloud Login Flow v2"
            );
            return Ok(OidcCallbackResult::NextcloudLogin {
                nc_flow_token: nc_token,
                user_id: user.id(),
                username: user.username().to_string(),
            });
        }

        // 6. Issue internal tokens (same as regular login)
        let access_token = self.token_service.generate_access_token(&user)?;
        let refresh_token = self.token_service.generate_refresh_token();

        let session = Session::new(
            user.id(),
            refresh_token.clone(),
            None,
            None,
            self.token_service.refresh_token_expiry_days(),
            Uuid::new_v4(),
        );
        self.session_storage.create_session(session).await?;

        let auth_response = AuthResponseDto {
            user: UserDto::from(user),
            access_token,
            refresh_token,
            token_type: "Bearer".to_string(),
            expires_in: self.token_service.refresh_token_expiry_secs(),
        };

        // 7. Store auth response behind a one-time exchange code (Fix #4: no tokens in URL)
        let mut code_bytes = [0u8; 32];
        use rand_core::{OsRng, RngCore};
        OsRng.fill_bytes(&mut code_bytes);
        let exchange_code = hex::encode(code_bytes);

        // Store auth response (auto-expires after 60 s via moka TTL)
        self.pending_oidc_tokens
            .insert(exchange_code.clone(), PendingOidcToken { auth_response });

        tracing::info!("OIDC login successful, one-time exchange code generated");

        Ok(OidcCallbackResult::WebLogin { exchange_code })
    }

    /// Exchange a one-time code for the authentication tokens.
    /// The code is single-use and expires after 60 seconds (moka TTL).
    pub fn exchange_oidc_token(&self, one_time_code: &str) -> Result<AuthResponseDto, DomainError> {
        let pending = self
            .pending_oidc_tokens
            .remove(one_time_code)
            .ok_or_else(|| {
                DomainError::new(
                    ErrorKind::AccessDenied,
                    "OIDC",
                    "Invalid or expired exchange code. Please try logging in again.",
                )
            })?;

        Ok(pending.auth_response)
    }

    /// Map OIDC groups to internal role
    fn map_oidc_role(&self, groups: &[String], config: &OidcConfig) -> UserRole {
        if config.admin_groups.is_empty() {
            return UserRole::User;
        }
        let admin_groups: Vec<&str> = config.admin_groups.split(',').map(|s| s.trim()).collect();
        for group in groups {
            if admin_groups.iter().any(|ag| ag.eq_ignore_ascii_case(group)) {
                return UserRole::Admin;
            }
        }
        UserRole::User
    }

    // `create_personal_folder` was removed in PR 3 of the
    // UserLifecycleHook migration — home-folder provisioning is now
    // owned by `HomeFolderLifecycleHook` in folder_service.rs and runs
    // via `dispatch_created` / `dispatch_login`.
}

/// URL-safe base64 encoding without padding (RFC 4648 §5)
fn base64_url_encode(input: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(input)
}
