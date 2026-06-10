use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::common::errors::{DomainError, ErrorKind, Result};

pub struct NextcloudObjectIdRepository {
    pool: Arc<PgPool>,
}

impl NextcloudObjectIdRepository {
    pub fn new(pool: Arc<PgPool>) -> Self {
        Self { pool }
    }

    /// Resolve — creating when absent — stable numeric IDs for a batch of
    /// object UUIDs sharing one `object_type`.
    ///
    /// Two statements instead of one per id: an idempotent bulk insert that
    /// leaves existing rows untouched (`ON CONFLICT DO NOTHING` — no row
    /// rewrite, no WAL churn, no dead tuples, unlike the former `DO UPDATE`),
    /// followed by a single read of every requested mapping. The insert
    /// auto-commits before the read, so the read observes both our own rows
    /// and any created concurrently. Returns a map keyed by object UUID;
    /// unresolvable inputs are simply absent.
    pub async fn get_or_create_many(
        &self,
        object_type: &str,
        object_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, i64>> {
        if object_ids.is_empty() {
            return Ok(HashMap::new());
        }

        // 1. Create missing mappings only. `DO NOTHING` skips the write for
        //    UUIDs that already map, eliminating the per-listing row rewrite.
        sqlx::query(
            r#"
            INSERT INTO storage.nextcloud_object_ids (object_type, object_id)
            SELECT $1, u FROM unnest($2::uuid[]) AS u
            ON CONFLICT (object_type, object_id) DO NOTHING
            "#,
        )
        .bind(object_type)
        .bind(object_ids)
        .execute(&*self.pool)
        .await
        .map_err(|e| {
            DomainError::new(
                ErrorKind::DatabaseError,
                "NextcloudFileId",
                format!("Failed to create Nextcloud IDs: {}", e),
            )
        })?;

        // 2. Read every requested mapping back in a single round-trip.
        let rows = sqlx::query(
            r#"
            SELECT id, object_id
            FROM storage.nextcloud_object_ids
            WHERE object_type = $1 AND object_id = ANY($2::uuid[])
            "#,
        )
        .bind(object_type)
        .bind(object_ids)
        .fetch_all(&*self.pool)
        .await
        .map_err(|e| {
            DomainError::new(
                ErrorKind::DatabaseError,
                "NextcloudFileId",
                format!("Failed to load Nextcloud IDs: {}", e),
            )
        })?;

        let mut map = HashMap::with_capacity(rows.len());
        for row in rows {
            let object_id: Uuid = row.get("object_id");
            let id: i64 = row.get("id");
            map.insert(object_id, id);
        }
        Ok(map)
    }

    /// Get the OxiCloud object ID from a Nextcloud numeric ID.
    pub async fn get_object_id(&self, nc_id: i64, object_type: &str) -> Result<String> {
        let row = sqlx::query(
            r#"
            SELECT object_id
            FROM storage.nextcloud_object_ids
            WHERE id = $1 AND object_type = $2
            "#,
        )
        .bind(nc_id)
        .bind(object_type)
        .fetch_optional(&*self.pool)
        .await
        .map_err(|e| {
            DomainError::new(
                ErrorKind::DatabaseError,
                "NextcloudFileId",
                format!("Failed to lookup Nextcloud ID: {}", e),
            )
        })?;

        match row {
            Some(row) => {
                let uuid: sqlx::types::Uuid = row.get("object_id");
                Ok(uuid.to_string())
            }
            None => Err(DomainError::new(
                ErrorKind::NotFound,
                "NextcloudFileId",
                format!("No mapping found for Nextcloud ID: {}", nc_id),
            )),
        }
    }
}
