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

    /// Upgraded: log-and-continue. Called by
    /// `AuthApplicationService::upgrade_to_internal` after the
    /// `is_external = false` UPDATE persists. Same log-and-continue
    /// semantics as `dispatch_created` — the row is already updated,
    /// hook failure at (e.g.) home-drive provisioning is recoverable
    /// on the next login via `PersonalDriveLifecycleHook::on_user_login`
    /// (its safety-net path already handles the "user is internal but
    /// no drive yet" case idempotently).
    pub async fn dispatch_upgraded_to_internal(&self, user: &User) {
        for h in &self.hooks {
            if let Err(e) = h.on_upgraded_to_internal(user).await {
                tracing::error!(
                    target: "user_lifecycle",
                    hook = h.name(),
                    user_id = %user.id(),
                    error = %e,
                    "on_upgraded_to_internal failed; drive provisioning will retry on next login"
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

    /// Deleted: runs inside the `delete_user_admin` transaction. First
    /// `Err` propagates and aborts the transaction — the user is NOT
    /// deleted. Hooks must keep their cleanup conservative. See tip #7
    /// in the trait docstring.
    pub async fn dispatch_deleted(
        &self,
        user: &User,
        mode: DeletionMode,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), DomainError> {
        for h in &self.hooks {
            if let Err(e) = h.on_user_deleted(user, mode, tx).await {
                tracing::error!(
                    target: "user_lifecycle",
                    hook = h.name(),
                    mode = ?mode,
                    user_id = %user.id(),
                    error = %e,
                    "on_user_deleted failed — aborting transaction"
                );
                return Err(e);
            }
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditLifecycleHook
//
// Always-on observer. Emits one structured `tracing::info!(target: "audit",
// ...)` line per event. The only hook registered in PR 1; subsequent PRs
// add PersonalDriveLifecycleHook, AuthzCacheLifecycleHook, etc., each living
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
            username = %user.display_for_audit(),
            is_external = user.is_external(),
        );
        Ok(())
    }

    async fn on_user_login(&self, user: &User) -> Result<(), DomainError> {
        tracing::info!(
            target: "audit",
            event = "user.login",
            user_id = %user.id(),
            username = %user.display_for_audit(),
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
            username = %user.display_for_audit(),
            is_external = user.is_external(),
            reason = ?reason,
        );
        Ok(())
    }

    async fn on_user_deleted(
        &self,
        user: &User,
        mode: DeletionMode,
        _tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), DomainError> {
        // Audit hook doesn't write to the DB — only emits a tracing
        // event. The `_tx` is intentionally ignored.
        tracing::info!(
            target: "audit",
            event = "user.deleted",
            user_id = %user.id(),
            username = %user.display_for_audit(),
            is_external = user.is_external(),
            mode = ?mode,
        );
        Ok(())
    }

    async fn on_upgraded_to_internal(&self, user: &User) -> Result<(), DomainError> {
        // Post-upgrade state — `is_external` is already `false` here
        // (the service persisted before dispatching), so we don't log
        // it as a field; the event name carries the transition.
        tracing::info!(
            target: "audit",
            event = "user.upgraded_to_internal",
            user_id = %user.id(),
            username = %user.display_for_audit(),
        );
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SessionRevocationLifecycleHook
//
// Replaces the silent FK CASCADE on `auth.sessions.user_id` with an
// explicit `revoke_all_user_sessions` call inside the delete transaction
// — emits an aggregate audit event ("user.sessions_revoked_on_delete,
// count=N") so the deletion of N sessions is observable, instead of N
// rows quietly vanishing via CASCADE.
//
// Co-located with the dispatcher because there is no dedicated session
// service today; the session-storage port is the only consumer. If a
// `SessionService` ever emerges, this hook moves there.
// ─────────────────────────────────────────────────────────────────────────────

use crate::application::ports::auth_ports::SessionStoragePort;
use crate::infrastructure::repositories::pg::SessionPgRepository;

/// Lifecycle hook: explicit per-user session revocation on delete with
/// audit trail. On any other event: explicit no-op.
pub struct SessionRevocationLifecycleHook {
    session_storage: Arc<SessionPgRepository>,
}

impl SessionRevocationLifecycleHook {
    pub fn new(session_storage: Arc<SessionPgRepository>) -> Self {
        Self { session_storage }
    }
}

#[async_trait]
impl UserLifecycleHook for SessionRevocationLifecycleHook {
    fn name(&self) -> &'static str {
        "session_revocation"
    }

    async fn on_user_created(&self, _user: &User) -> Result<(), DomainError> {
        Ok(())
    }

    async fn on_user_login(&self, _user: &User) -> Result<(), DomainError> {
        Ok(())
    }

    async fn on_user_logout(&self, _user: &User, _reason: LogoutReason) -> Result<(), DomainError> {
        // The session causing this logout has already been revoked by
        // the caller (logout / change_password / etc.). Nothing for this
        // hook to do.
        Ok(())
    }

    async fn on_user_deleted(
        &self,
        user: &User,
        mode: DeletionMode,
        _tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), DomainError> {
        // NOTE on `_tx`: ideally this would use the transaction so the
        // session revocation is atomic with the user DELETE. The current
        // SessionStoragePort surface doesn't expose a tx-accepting
        // variant of `revoke_all_user_sessions`, so we revoke against
        // the same pool. The FK CASCADE on `auth.sessions.user_id`
        // would clean up any sessions left behind by a rollback anyway,
        // so the safety net holds.
        let count = self
            .session_storage
            .revoke_all_user_sessions(user.id())
            .await?;
        tracing::info!(
            target: "audit",
            event = "user.sessions_revoked_on_delete",
            user_id = %user.id(),
            username = %user.display_for_audit(),
            mode = ?mode,
            count = count,
        );
        Ok(())
    }
}
