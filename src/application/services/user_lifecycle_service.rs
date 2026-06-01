//! User-lifecycle dispatcher + the always-on `AuditLifecycleHook`.
//!
//! [`UserLifecycleService`] aggregates every registered
//! [`UserLifecycleHook`] and fans out each lifecycle event with
//! per-event failure semantics. See `user_lifecycle.rs` for the trait
//! contract and tips for implementors.
//!
//! [`AuditLifecycleHook`] lives in this file (not under
//! `infrastructure/services/`) because it's cross-cutting — no domain
//! service owns "user-lifecycle audit", and the hook is small enough that
//! a separate module would be ceremony. Every other hook lives with the
//! service that owns its work (see `architecture/user-lifecycle.md`).

use std::sync::Arc;

use async_trait::async_trait;

use crate::application::ports::user_lifecycle::{DeletionMode, LogoutReason, UserLifecycleHook};
use crate::common::errors::DomainError;
use crate::domain::entities::user::User;

/// Composite dispatcher for user-lifecycle events.
///
/// Mirrors the [`FileLifecycleService`] shape: a `Vec<Arc<dyn ...>>` and a
/// builder. The per-event failure semantics differ from the file-side
/// (file events are sync fire-and-forget; user events have per-method
/// rules — see the trait docstring).
pub struct UserLifecycleService {
    hooks: Vec<Arc<dyn UserLifecycleHook>>,
}

impl Default for UserLifecycleService {
    fn default() -> Self {
        Self::new()
    }
}

impl UserLifecycleService {
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    pub fn with_hook(mut self, hook: Arc<dyn UserLifecycleHook>) -> Self {
        self.hooks.push(hook);
        self
    }

    /// Created: log-and-continue. If a hook returns `Err`, the user is
    /// still created — the next login's `on_user_login` will retry
    /// idempotently. See tip #6 in the trait docstring.
    pub async fn dispatch_created(&self, user: &User) {
        for h in &self.hooks {
            if let Err(e) = h.on_user_created(user).await {
                tracing::error!(
                    target: "user_lifecycle",
                    hook = h.name(),
                    user_id = %user.id(),
                    error = %e,
                    "on_user_created failed; will retry on next login"
                );
            }
        }
    }

    /// Login: log-and-continue. Same reasoning as `dispatch_created`.
    /// Must fire BEFORE `user.register_login()` so that hooks observing
    /// `last_login_at().is_none()` correctly detect the first-ever login.
    pub async fn dispatch_login(&self, user: &User) {
        for h in &self.hooks {
            if let Err(e) = h.on_user_login(user).await {
                tracing::error!(
                    target: "user_lifecycle",
                    hook = h.name(),
                    user_id = %user.id(),
                    error = %e,
                    "on_user_login failed; will retry on next login"
                );
            }
        }
    }

    /// Logout: fire-and-forget. Spawned so the HTTP response doesn't wait
    /// for downstream cache flushes. Takes ownership of `User` because the
    /// spawn outlives the caller's borrow.
    pub fn dispatch_logout(&self, user: User, reason: LogoutReason) {
        let hooks = self.hooks.clone();
        tokio::spawn(async move {
            for h in &hooks {
                if let Err(e) = h.on_user_logout(&user, reason).await {
                    tracing::error!(
                        target: "user_lifecycle",
                        hook = h.name(),
                        reason = ?reason,
                        user_id = %user.id(),
                        error = %e,
                        "on_user_logout failed"
                    );
                }
            }
        });
    }

    /// Deleted: log-and-continue (post-commit today). PR 4 refactors
    /// `delete_user_admin` to expose a transaction handle and switches
    /// this to abort-on-first-Err to make cleanup atomic with the user
    /// DELETE. See tip #7 in the trait docstring.
    pub async fn dispatch_deleted(&self, user: &User, mode: DeletionMode) {
        for h in &self.hooks {
            if let Err(e) = h.on_user_deleted(user, mode).await {
                tracing::error!(
                    target: "user_lifecycle",
                    hook = h.name(),
                    mode = ?mode,
                    user_id = %user.id(),
                    error = %e,
                    "on_user_deleted failed"
                );
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditLifecycleHook
//
// Always-on observer. Emits one structured `tracing::info!(target: "audit",
// ...)` line per event. The only hook registered in PR 1; subsequent PRs
// add HomeFolderLifecycleHook, AuthzCacheLifecycleHook, etc., each living
// next to the service it works for.
// ─────────────────────────────────────────────────────────────────────────────

/// Cross-cutting audit observer for user-lifecycle events. Co-located with
/// the dispatcher because audit has no domain owner.
pub struct AuditLifecycleHook;

#[async_trait]
impl UserLifecycleHook for AuditLifecycleHook {
    fn name(&self) -> &'static str {
        "audit"
    }

    async fn on_user_created(&self, user: &User) -> Result<(), DomainError> {
        tracing::info!(
            target: "audit",
            event = "user.created",
            user_id = %user.id(),
            username = %user.username(),
            is_external = user.is_external(),
        );
        Ok(())
    }

    async fn on_user_login(&self, user: &User) -> Result<(), DomainError> {
        tracing::info!(
            target: "audit",
            event = "user.login",
            user_id = %user.id(),
            username = %user.username(),
            is_external = user.is_external(),
            first_login = user.last_login_at().is_none(),
        );
        Ok(())
    }

    async fn on_user_logout(&self, user: &User, reason: LogoutReason) -> Result<(), DomainError> {
        tracing::info!(
            target: "audit",
            event = "user.logout",
            user_id = %user.id(),
            username = %user.username(),
            is_external = user.is_external(),
            reason = ?reason,
        );
        Ok(())
    }

    async fn on_user_deleted(&self, user: &User, mode: DeletionMode) -> Result<(), DomainError> {
        tracing::info!(
            target: "audit",
            event = "user.deleted",
            user_id = %user.id(),
            username = %user.username(),
            is_external = user.is_external(),
            mode = ?mode,
        );
        Ok(())
    }
}
