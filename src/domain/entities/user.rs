use chrono::{DateTime, Utc};
use uuid::Uuid;

// Re-export entity errors from the centralized module
pub use super::entity_errors::{UserError, UserResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// We'll handle conversion manually for now until the type is properly set up in the database
pub enum UserRole {
    Admin,
    User,
}

impl std::fmt::Display for UserRole {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            UserRole::Admin => write!(f, "admin"),
            UserRole::User => write!(f, "user"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct User {
    id: Uuid,
    username: String,
    email: String,
    password_hash: String,
    role: UserRole,
    storage_quota_bytes: i64,
    storage_used_bytes: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    last_login_at: Option<DateTime<Utc>>,
    active: bool,
    oidc_provider: Option<String>,
    oidc_subject: Option<String>,
    image: Option<String>,
    /// TRUE = grant-only external recipient (magic-link, OIDC-only, OCM
    /// federated). FALSE = storage-owning internal user. Hooks that
    /// provision per-user resources (home folder, default calendar, …)
    /// must short-circuit when `is_external` is TRUE — see tip #2 in
    /// `application/ports/user_lifecycle.rs`. The DB CHECK constraint
    /// `users_external_no_storage` is the schema-level safety net.
    is_external: bool,
}

impl User {
    /// Create a new user with a pre-hashed password.
    ///
    /// The password hashing should be done externally using PasswordHasherPort
    /// to maintain clean architecture and keep cryptographic dependencies
    /// out of the domain layer.
    ///
    /// # Arguments
    /// * `username` - User's username (3-32 characters)
    /// * `email` - User's email address
    /// * `password_hash` - Pre-hashed password (from PasswordHasherPort)
    /// * `role` - User's role
    /// * `storage_quota_bytes` - Storage quota in bytes
    pub fn new(
        username: String,
        email: String,
        password_hash: String,
        role: UserRole,
        storage_quota_bytes: i64,
    ) -> UserResult<Self> {
        // Validations
        Self::validate_username(&username)?;
        Self::validate_email(&email)?;

        if password_hash.is_empty() {
            return Err(UserError::InvalidPassword(
                "Password hash cannot be empty".to_string(),
            ));
        }

        let now = Utc::now();

        Ok(Self {
            id: Uuid::new_v4(),
            username,
            email,
            password_hash,
            role,
            storage_quota_bytes,
            storage_used_bytes: 0,
            created_at: now,
            updated_at: now,
            last_login_at: None,
            active: true,
            oidc_provider: None,
            oidc_subject: None,
            image: None,
            is_external: false,
        })
    }

    /// Create a new OIDC-authenticated user (no password required).
    pub fn new_oidc(
        username: String,
        email: String,
        role: UserRole,
        storage_quota_bytes: i64,
        oidc_provider: String,
        oidc_subject: String,
    ) -> UserResult<Self> {
        Self::validate_username(&username)?;
        Self::validate_email(&email)?;
        let now = Utc::now();
        Ok(Self {
            id: Uuid::new_v4(),
            username,
            email,
            password_hash: "__OIDC_NO_PASSWORD__".to_string(),
            role,
            storage_quota_bytes,
            storage_used_bytes: 0,
            created_at: now,
            updated_at: now,
            last_login_at: None,
            active: true,
            oidc_provider: Some(oidc_provider),
            oidc_subject: Some(oidc_subject),
            image: None,
            is_external: false,
        })
    }

    /// Create a new external user — magic-link / OIDC-only / OCM-federated
    /// recipient who does NOT own storage. The `CHECK (NOT is_external OR
    /// storage_used_bytes = 0)` DB constraint enforces the no-storage rule
    /// at the schema level.
    ///
    /// **External users are always `UserRole::User`** — there is no role
    /// parameter because admin + external is an explicitly forbidden
    /// combination enforced by the `users_external_not_admin` DB CHECK
    /// constraint. Granting admin to a federated principal would let
    /// external identity providers indirectly manage the local instance.
    /// To make an external user an admin: first convert them to internal
    /// (`UPDATE auth.users SET is_external = FALSE`), then update role.
    /// The two-step process is intentional friction.
    ///
    /// Quota is set to 0 because external users can't upload content
    /// into any folder they own (they have no folder). They can only
    /// act on grants the resource owner provides — which counts against
    /// the owner's quota, not theirs.
    pub fn new_external(username: String, email: String) -> UserResult<Self> {
        Self::validate_username(&username)?;
        Self::validate_email(&email)?;
        let now = Utc::now();
        Ok(Self {
            id: Uuid::new_v4(),
            username,
            email,
            password_hash: "__EXTERNAL_NO_PASSWORD__".to_string(),
            role: UserRole::User,
            storage_quota_bytes: 0,
            storage_used_bytes: 0,
            created_at: now,
            updated_at: now,
            last_login_at: None,
            active: true,
            oidc_provider: None,
            oidc_subject: None,
            image: None,
            is_external: true,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn from_data(
        id: Uuid,
        username: String,
        email: String,
        password_hash: String,
        role: UserRole,
        storage_quota_bytes: i64,
        storage_used_bytes: i64,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
        last_login_at: Option<DateTime<Utc>>,
        active: bool,
    ) -> Self {
        Self {
            id,
            username,
            email,
            password_hash,
            role,
            storage_quota_bytes,
            storage_used_bytes,
            created_at,
            updated_at,
            last_login_at,
            active,
            oidc_provider: None,
            oidc_subject: None,
            image: None,
            // `from_data` is the minimal-args reconstruction path used by
            // tests and JWT-claim-based principal hydration (which doesn't
            // carry `is_external`). Default to FALSE — JWT-validated
            // principals are existing internal users; magic-link external
            // sessions take a different path that hydrates from DB via
            // `from_data_full`.
            is_external: false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn from_data_full(
        id: Uuid,
        username: String,
        email: String,
        password_hash: String,
        role: UserRole,
        storage_quota_bytes: i64,
        storage_used_bytes: i64,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
        last_login_at: Option<DateTime<Utc>>,
        active: bool,
        oidc_provider: Option<String>,
        oidc_subject: Option<String>,
        image: Option<String>,
        is_external: bool,
    ) -> Self {
        Self {
            id,
            username,
            email,
            password_hash,
            role,
            storage_quota_bytes,
            storage_used_bytes,
            created_at,
            updated_at,
            last_login_at,
            active,
            oidc_provider,
            oidc_subject,
            image,
            is_external,
        }
    }

    // Getters
    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn email(&self) -> &str {
        &self.email
    }

    pub fn role(&self) -> UserRole {
        self.role
    }

    pub fn storage_quota_bytes(&self) -> i64 {
        self.storage_quota_bytes
    }

    pub fn storage_used_bytes(&self) -> i64 {
        self.storage_used_bytes
    }

    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    pub fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    pub fn last_login_at(&self) -> Option<DateTime<Utc>> {
        self.last_login_at
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn password_hash(&self) -> &str {
        &self.password_hash
    }

    pub fn oidc_provider(&self) -> Option<&str> {
        self.oidc_provider.as_deref()
    }

    pub fn oidc_subject(&self) -> Option<&str> {
        self.oidc_subject.as_deref()
    }

    pub fn image(&self) -> Option<&str> {
        self.image.as_deref()
    }

    /// `TRUE` for grant-only external recipients (magic-link, OIDC-only,
    /// OCM federated). Hooks provisioning per-user resources must
    /// short-circuit when this returns `true` — see tip #2 in
    /// `application/ports/user_lifecycle.rs`.
    pub fn is_external(&self) -> bool {
        self.is_external
    }

    pub fn set_image(&mut self, image: Option<String>) {
        self.image = image;
        self.updated_at = Utc::now();
    }

    /// Returns true if this is an OIDC-only user (no password)
    pub fn is_oidc_user(&self) -> bool {
        self.oidc_provider.is_some()
    }

    /// Update the password hash.
    ///
    /// The new password should be hashed externally using PasswordHasherPort
    /// before calling this method.
    pub fn update_password_hash(&mut self, new_hash: String) {
        self.password_hash = new_hash;
        self.updated_at = Utc::now();
    }

    // Update storage usage
    pub fn update_storage_used(&mut self, storage_used_bytes: i64) {
        self.storage_used_bytes = storage_used_bytes;
        self.updated_at = Utc::now();
    }

    // Register login
    pub fn register_login(&mut self) {
        let now = Utc::now();
        self.last_login_at = Some(now);
        self.updated_at = now;
    }

    // Deactivate user
    pub fn deactivate(&mut self) {
        self.active = false;
        self.updated_at = Utc::now();
    }

    // Activate user
    pub fn activate(&mut self) {
        self.active = true;
        self.updated_at = Utc::now();
    }

    // ── Shared validation helpers ──────────────────────────────────────

    /// Usernames must be 3-32 chars and contain only ASCII alphanumerics,
    /// hyphens, underscores, and dots.  This prevents XSS payloads like
    /// `<img/src=x>` from being stored as usernames.
    fn validate_username(username: &str) -> UserResult<()> {
        if username.len() < 3 || username.len() > 32 {
            return Err(UserError::InvalidUsername(
                "Username must be between 3 and 32 characters".to_string(),
            ));
        }
        if !username
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return Err(UserError::InvalidUsername(
                "Username may only contain letters, digits, hyphens, underscores, and dots"
                    .to_string(),
            ));
        }
        // Disallow leading/trailing dots or hyphens
        if username.starts_with('.')
            || username.starts_with('-')
            || username.ends_with('.')
            || username.ends_with('-')
        {
            return Err(UserError::InvalidUsername(
                "Username must not start or end with a dot or hyphen".to_string(),
            ));
        }
        Ok(())
    }

    /// Basic but meaningful email validation:
    /// - Must contain exactly one `@`
    /// - Local part and domain must be non-empty
    /// - Domain must contain at least one dot
    /// - No angle brackets, spaces, or other characters used in XSS payloads
    fn validate_email(email: &str) -> UserResult<()> {
        let parts: Vec<&str> = email.splitn(2, '@').collect();
        if parts.len() != 2 {
            return Err(UserError::ValidationError(
                "Invalid email: missing @".to_string(),
            ));
        }
        let (local, domain) = (parts[0], parts[1]);
        if local.is_empty() || domain.is_empty() {
            return Err(UserError::ValidationError(
                "Invalid email: empty local part or domain".to_string(),
            ));
        }
        if !domain.contains('.') {
            return Err(UserError::ValidationError(
                "Invalid email: domain must contain a dot".to_string(),
            ));
        }
        // Reject characters commonly used in XSS / header injection
        let forbidden = [
            '<', '>', '"', '\'', '\\', ' ', '\t', '\n', '\r', '(', ')', ',', ';',
        ];
        if email.chars().any(|c| forbidden.contains(&c)) {
            return Err(UserError::ValidationError(
                "Invalid email: contains forbidden characters".to_string(),
            ));
        }
        if email.len() > 254 {
            return Err(UserError::ValidationError(
                "Invalid email: too long (max 254 characters)".to_string(),
            ));
        }
        Ok(())
    }
}
