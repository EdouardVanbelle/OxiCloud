use std::sync::Arc;
use std::time::Duration;
use tokio::time;
use tracing::{debug, error, info, instrument};

use crate::common::errors::Result;
use crate::domain::repositories::trash_repository::TrashRepository;
use crate::infrastructure::repositories::pg::trash_db_repository::TrashDbRepository;
use crate::infrastructure::services::dedup_service::DedupService;

/// Service for automatic cleanup of expired items in the trash.
///
/// Uses `TrashRepository::delete_expired_bulk` to purge all expired items
/// in **2 SQL statements inside a single transaction**, instead of the
/// previous N+1 pattern that issued 3 queries per expired item.
///
/// Each sweep ends with a dedup `garbage_collect()` pass: it reclaims the
/// blobs the expiry just dereferenced AND any other zero-reference rows —
/// notably chunks left behind by aborted streaming uploads, whose rollback
/// registers them at ref_count 0 precisely so this sweep can find them.
/// Without it, orphans would only be collected when a user happens to
/// empty their trash by hand.
pub struct TrashCleanupService {
    trash_repository: Arc<TrashDbRepository>,
    dedup_service: Arc<DedupService>,
    cleanup_interval_hours: u64,
}

impl TrashCleanupService {
    pub fn new(
        trash_repository: Arc<TrashDbRepository>,
        dedup_service: Arc<DedupService>,
        cleanup_interval_hours: u64,
    ) -> Self {
        Self {
            trash_repository,
            dedup_service,
            cleanup_interval_hours: cleanup_interval_hours.max(1), // Minimum 1 hour
        }
    }

    /// Starts the periodic cleanup job
    #[instrument(skip(self))]
    pub async fn start_cleanup_job(&self) {
        let trash_repository = self.trash_repository.clone();
        let dedup_service = self.dedup_service.clone();
        let interval_hours = self.cleanup_interval_hours;

        info!(
            "Starting trash cleanup job with interval of {} hours",
            interval_hours
        );

        tokio::spawn(async move {
            let interval_duration = Duration::from_secs(interval_hours * 60 * 60);
            let mut interval = time::interval(interval_duration);

            // First immediate execution
            Self::cleanup_expired_items(trash_repository.clone(), dedup_service.clone())
                .await
                .unwrap_or_else(|e| error!("Error in initial trash cleanup: {:?}", e));

            loop {
                interval.tick().await;
                debug!("Running scheduled trash cleanup task");

                if let Err(e) =
                    Self::cleanup_expired_items(trash_repository.clone(), dedup_service.clone())
                        .await
                {
                    error!("Error in scheduled trash cleanup: {:?}", e);
                }
            }
        });
    }

    /// Bulk-delete all expired trash items in a single transaction, then
    /// garbage-collect every zero-reference manifest/blob (expired content
    /// plus aborted-upload orphans).
    #[instrument(skip(trash_repository, dedup_service))]
    async fn cleanup_expired_items(
        trash_repository: Arc<TrashDbRepository>,
        dedup_service: Arc<DedupService>,
    ) -> Result<()> {
        debug!("Starting bulk cleanup of expired trash items");

        let (files, folders) = trash_repository.delete_expired_bulk().await?;

        if files == 0 && folders == 0 {
            debug!("No expired items to clean up");
        } else {
            info!(
                "Trash cleanup completed: {} files + {} folders purged",
                files, folders
            );
        }

        // Runs on the maintenance pool; batched (500 rows/iteration) with
        // yield points, so it never starves request-path queries.
        match dedup_service.garbage_collect().await {
            Ok((0, _)) => debug!("Trash cleanup GC: nothing to collect"),
            Ok((items, bytes)) => {
                info!("Trash cleanup GC: reclaimed {items} orphaned blobs ({bytes} bytes)");
            }
            Err(e) => error!("Trash cleanup GC failed: {:?}", e),
        }

        Ok(())
    }
}
