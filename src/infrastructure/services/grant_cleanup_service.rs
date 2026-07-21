//! Background daemon that purges expired `storage.role_grants` rows.
//!
//! The AuthZ engine already filters expired grants out of every
//! permission check at read time (`expires_at IS NULL OR
//! expires_at > NOW()` on every `check` / `list_grants_*` path in
//! `PgAclEngine`), so expired rows never leak permission. They just
//! accumulate. This daemon garbage-collects them once per
//! [`GrantCleanupService::interval_hours`], with a grace window past
//! `expires_at` that preserves the audit / support answer to "what
//! happened to my access?" for a few weeks.
//!
//! Shape mirrors [`TrashCleanupService`] verbatim (fire-and-forget
//! `tokio::spawn`, `tokio::time::interval`, first-tick-immediate). The
//! authoritative pattern for background daemons in this codebase; see
//! the plan doc `docs/plan/` (deferred future work: fold all daemons
//! into a central `JobRegistry` that plugins can also register into).
//!
//! [`TrashCleanupService`]: crate::infrastructure::services::trash_cleanup_service::TrashCleanupService

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time;
use tracing::{error, info};

use crate::application::ports::authorization_ports::AuthorizationEngine;
use crate::infrastructure::services::pg_acl_engine::PgAclEngine;

/// Daemon that periodically deletes expired grants.
///
/// Owns an `Arc<PgAclEngine>` (not a `dyn AuthorizationEngine`) to avoid
/// the wrapper allocation on every SQL call — the daemon is the sole
/// caller of `purge_expired_grants` outside of the admin trigger
/// endpoint, both statically dispatched.
pub struct GrantCleanupService {
    authz: Arc<PgAclEngine>,
    grace_days: u32,
    interval_hours: u64,
}

impl GrantCleanupService {
    pub fn new(authz: Arc<PgAclEngine>, grace_days: u32, interval_hours: u64) -> Self {
        Self {
            authz,
            grace_days,
            // Minimum 1 hour — matches TrashCleanupService's clamp so
            // a mis-set `0` doesn't spin a hot loop.
            interval_hours: interval_hours.max(1),
        }
    }

    /// Grace period the daemon uses on its scheduled ticks. Exposed
    /// for the admin trigger's default-response field.
    pub fn grace_days(&self) -> u32 {
        self.grace_days
    }

    /// Fire-and-forget the periodic purge. Never joins; killed
    /// implicitly at `tokio::runtime::shutdown`.
    pub async fn start_cleanup_job(self: Arc<Self>) {
        let interval_hours = self.interval_hours;
        let grace_days = self.grace_days;
        info!(
            "Starting grant-cleanup daemon: every {}h, grace = {}d",
            interval_hours, grace_days
        );

        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(interval_hours * 60 * 60));
            // First tick fires immediately — matches TrashCleanupService.
            // Any accumulated backlog at boot gets flushed straight away.
            loop {
                interval.tick().await;
                self.run_once().await;
            }
        });
    }

    /// One scheduled pass. Also called by the admin trigger endpoint
    /// (via a shared `Arc<GrantCleanupService>` on `AppState`).
    ///
    /// `grace_override`:
    /// - `None` → use the configured grace (`self.grace_days`).
    /// - `Some(n)` → override with `n`. The admin `?force=true` trigger
    ///   passes `Some(0)` so Hurl regressions can hit expired grants
    ///   without waiting the configured grace out.
    pub async fn purge(&self, grace_override: Option<u32>) -> u64 {
        let grace = grace_override.unwrap_or(self.grace_days);
        let start = Instant::now();
        match self.authz.purge_expired_grants(grace).await {
            Ok(count) => {
                // Audit-channel logging: bulk deletion of authorization
                // rows is security-relevant enough to keep it in the
                // audit stream even when the count is zero (proves the
                // daemon is reachable).
                info!(
                    target: "audit",
                    event = "grant_cleanup.purged",
                    count = count,
                    grace_days = grace,
                    elapsed_ms = start.elapsed().as_millis() as u64,
                    "👮🏻‍♂️ Purged {} expired grant(s) older than {} days",
                    count,
                    grace,
                );
                count
            }
            Err(e) => {
                error!(
                    target: "audit",
                    event = "grant_cleanup.failed",
                    grace_days = grace,
                    error = %e,
                    "Grant cleanup failed"
                );
                0
            }
        }
    }

    /// Convenience for the scheduled loop.
    async fn run_once(&self) {
        let _ = self.purge(None).await;
    }
}
