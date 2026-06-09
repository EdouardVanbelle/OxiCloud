use crate::common::config::AppConfig;
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::time::Duration;

/// Database initialization error.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct DbError(String);

type Result<T> = std::result::Result<T, DbError>;

/// Segmented database pools.
///
/// `primary` is used for all user-facing request paths (REST, WebDAV, CalDAV,
/// CardDAV).  `maintenance` is a smaller, isolated pool reserved for
/// background / batch operations (verify_integrity, garbage_collect,
/// update_all_users_storage_usage, trash cleanup) so they can never starve
/// interactive requests.
pub struct DbPools {
    /// Pool for user-facing request paths.
    pub primary: PgPool,
    /// Pool for background / batch maintenance tasks.
    pub maintenance: PgPool,
}

/// Create both the primary and maintenance database pools.
///
/// Pending migrations are applied via the primary pool on startup.
/// The maintenance pool shares the same connection string but has its
/// own, smaller budget.
pub async fn create_database_pools(config: &AppConfig) -> Result<DbPools> {
    tracing::info!(
        "Initializing PostgreSQL connections with URL: {}",
        config
            .database
            .connection_string
            .replace("postgres://", "postgres://[user]:[pass]@")
    );

    // --- primary pool ---
    let primary = create_pool_with_retries(
        &config.database.connection_string,
        config.database.max_connections,
        config.database.min_connections,
        config.database.connect_timeout_secs,
        config.database.idle_timeout_secs,
        config.database.max_lifetime_secs,
        "primary",
    )
    .await?;

    // Run pending migrations (idempotent, tracked in _sqlx_migrations table)
    tracing::info!("Running database migrations...");
    if let Err(e) = run_migrations(&primary).await {
        return Err(DbError(format!(
            "Database migrations failed: {}. \
             Check the migrations/ directory for issues.",
            e
        )));
    }
    tracing::info!("Database migrations complete");

    // --- maintenance pool ---
    let maintenance = create_pool_with_retries(
        &config.database.connection_string,
        config.database.maintenance_max_connections,
        config.database.maintenance_min_connections,
        config.database.connect_timeout_secs,
        config.database.idle_timeout_secs,
        config.database.max_lifetime_secs,
        "maintenance",
    )
    .await?;

    tracing::info!(
        "Database pools ready — primary: {} max / {} min, maintenance: {} max / {} min",
        config.database.max_connections,
        config.database.min_connections,
        config.database.maintenance_max_connections,
        config.database.maintenance_min_connections,
    );

    Ok(DbPools {
        primary,
        maintenance,
    })
}

/// Internal helper: create a single pool with retry logic.
async fn create_pool_with_retries(
    connection_string: &str,
    max_connections: u32,
    min_connections: u32,
    connect_timeout_secs: u64,
    idle_timeout_secs: u64,
    max_lifetime_secs: u64,
    label: &str,
) -> Result<PgPool> {
    let mut attempt = 0;
    const MAX_ATTEMPTS: usize = 5;

    while attempt < MAX_ATTEMPTS {
        attempt += 1;
        tracing::info!(
            "PostgreSQL {} pool connection attempt #{}/{}",
            label,
            attempt,
            MAX_ATTEMPTS
        );

        match PgPoolOptions::new()
            .max_connections(max_connections)
            .min_connections(min_connections)
            .acquire_timeout(Duration::from_secs(connect_timeout_secs))
            .idle_timeout(Duration::from_secs(idle_timeout_secs))
            .max_lifetime(Duration::from_secs(max_lifetime_secs))
            // Skip the liveness ping sqlx issues on every acquire() (on by
            // default): with warm min_connections and a bounded max_lifetime,
            // that extra round-trip per checkout costs more than the rare dead
            // connection it catches. A stale socket surfaces as a query error
            // and the pool recycles it either way.
            .test_before_acquire(false)
            .connect(connection_string)
            .await
        {
            Ok(pool) => match sqlx::query("SELECT 1").execute(&pool).await {
                Ok(_) => {
                    tracing::info!("PostgreSQL {} pool established successfully", label);
                    return Ok(pool);
                }
                Err(e) => {
                    tracing::error!("Error verifying {} pool connection: {}", label, e);
                    if attempt >= MAX_ATTEMPTS {
                        return Err(DbError(format!(
                            "Error verifying PostgreSQL {} pool connection: {}",
                            label, e
                        )));
                    }
                }
            },
            Err(e) => {
                tracing::error!(
                    "Error connecting to PostgreSQL {} pool (attempt {}/{}): {}",
                    label,
                    attempt,
                    MAX_ATTEMPTS,
                    e
                );
                if attempt >= MAX_ATTEMPTS {
                    return Err(DbError(format!(
                        "Error in PostgreSQL {} pool connection: {}",
                        label, e
                    )));
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }

    Err(DbError(format!(
        "Could not establish PostgreSQL {} pool connection after {} attempts",
        label, MAX_ATTEMPTS
    )))
}

/// Run pending migrations from the `migrations/` directory.
///
/// Uses sqlx's built-in migration system which tracks applied migrations
/// in a `_sqlx_migrations` table. Each migration runs in its own transaction.
/// Migration files are embedded at compile time via `sqlx::migrate!()`.
async fn run_migrations(pool: &PgPool) -> Result<()> {
    // ── One-time pre-flight cleanup for the 20260625000000 collision ──
    //
    // Two migrations landed on the same day from parallel branches with
    // the same version prefix:
    //   - 20260625000000_files_user_size_index.sql    (Dio)
    //   - 20260625000000_folder_tree_modified_at.sql  (Ed)
    // They were renamed to ...0001 and ...0002 (disjoint versions), and
    // both bodies were made idempotent so they re-run safely against
    // databases that already applied either original under the shared
    // version. However sqlx 0.8's default strict mode errors on boot
    // when `_sqlx_migrations` contains a row whose version no longer
    // maps to a source file ("previously applied but is missing in the
    // resolved migrations") — which is exactly the state of every
    // contributor DB that booted before the rename.
    //
    // This DELETE silently clears that stale bookkeeping row. The
    // schema effects of whichever original ran are preserved
    // (idempotent re-application via ...0001 / ...0002 is a no-op on
    // already-modified schemas). On fresh databases the table doesn't
    // exist yet, the query errors, and the `let _` swallows it —
    // sqlx::migrate!() then creates the table cleanly on its first
    // pass.
    //
    // Sunset: drop this block once the contributor base has rolled
    // past the affected commit window. Suggested review date 2026-12.
    let _ = sqlx::query("DELETE FROM _sqlx_migrations WHERE version = 20260625000000")
        .execute(pool)
        .await;

    match sqlx::migrate!().run(pool).await {
        Ok(()) => Ok(()),
        Err(e) => Err(DbError(format!("Migration error: {}", e))),
    }
}
