use std::collections::HashMap;
use std::sync::Arc;

use moka::future::Cache;
use uuid::Uuid;

use crate::common::errors::{DomainError, ErrorKind, Result};
use crate::infrastructure::repositories::pg::NextcloudObjectIdRepository;

/// Capacity of the in-memory UUID→numeric-id cache. The mapping is immutable
/// once created, so a warm entry never goes stale and eviction only costs a
/// re-query; ~100k entries is a few MB.
const ID_CACHE_CAPACITY: u64 = 100_000;

#[derive(Clone)]
pub struct NextcloudFileIdService {
    repo: Option<Arc<NextcloudObjectIdRepository>>,
    instance_id: String,
    /// Object-UUID → stable numeric id. `moka` caches are `Arc`-backed, so all
    /// clones of the service share one cache and the per-child resolution in a
    /// listing costs zero queries once warm.
    cache: Cache<Uuid, i64>,
}

impl NextcloudFileIdService {
    pub fn new(repo: Arc<NextcloudObjectIdRepository>, instance_id: String) -> Self {
        Self {
            repo: Some(repo),
            instance_id,
            cache: Cache::new(ID_CACHE_CAPACITY),
        }
    }

    pub fn new_stub() -> Self {
        Self {
            repo: None,
            instance_id: "ocnca".to_string(),
            cache: Cache::new(ID_CACHE_CAPACITY),
        }
    }

    /// Resolve — creating when absent — stable numeric file IDs for many
    /// UUIDs at once. Cache hits cost nothing; the misses are resolved with a
    /// single backing query. The returned map is keyed by the caller's
    /// original id strings; unresolvable inputs are simply absent (mirroring
    /// the `.ok()` behaviour the callers relied on).
    pub async fn get_or_create_file_ids(
        &self,
        file_ids: &[String],
    ) -> Result<HashMap<String, i64>> {
        self.get_or_create_many("file", file_ids).await
    }

    /// Folder counterpart of [`Self::get_or_create_file_ids`].
    pub async fn get_or_create_folder_ids(
        &self,
        folder_ids: &[String],
    ) -> Result<HashMap<String, i64>> {
        self.get_or_create_many("folder", folder_ids).await
    }

    async fn get_or_create_many(
        &self,
        object_type: &str,
        raw_ids: &[String],
    ) -> Result<HashMap<String, i64>> {
        let mut result = HashMap::with_capacity(raw_ids.len());
        // Parsed-UUID → caller's original string; also dedupes the miss list.
        let mut pending: HashMap<Uuid, String> = HashMap::new();

        for raw in raw_ids {
            let Ok(uuid) = Uuid::parse_str(raw) else {
                continue; // Unparseable ids never had a mapping — skip silently.
            };
            if let Some(id) = self.cache.get(&uuid).await {
                result.insert(raw.clone(), id);
            } else {
                pending.entry(uuid).or_insert_with(|| raw.clone());
            }
        }

        if !pending.is_empty() {
            let misses: Vec<Uuid> = pending.keys().copied().collect();
            let resolved = self
                .repo()?
                .get_or_create_many(object_type, &misses)
                .await?;
            for (uuid, id) in resolved {
                self.cache.insert(uuid, id).await;
                if let Some(original) = pending.get(&uuid) {
                    result.insert(original.clone(), id);
                }
            }
        }

        Ok(result)
    }

    fn repo(&self) -> Result<&Arc<NextcloudObjectIdRepository>> {
        self.repo.as_ref().ok_or_else(|| {
            DomainError::internal_error("NextcloudFileId", "Repository not initialized")
        })
    }

    /// Get the OxiCloud file UUID from a Nextcloud numeric ID.
    pub async fn get_oxicloud_id(&self, nc_file_id: i64) -> Result<String> {
        self.repo()?.get_object_id(nc_file_id, "file").await
    }

    pub fn format_oc_id(&self, id: i64) -> String {
        format!("{:08}{}", id, self.instance_id)
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    #[cfg(test)]
    pub fn new_test(instance_id: &str) -> Self {
        Self {
            repo: None,
            instance_id: instance_id.to_string(),
            cache: Cache::new(ID_CACHE_CAPACITY),
        }
    }

    pub fn ensure_ready(&self) -> Result<()> {
        if self.repo.is_none() {
            return Err(DomainError::new(
                ErrorKind::InternalError,
                "NextcloudFileId",
                "Repository not initialized",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_oc_id_default_instance() {
        let svc = NextcloudFileIdService::new_stub();
        assert_eq!(svc.format_oc_id(42), "00000042ocnca");
    }

    #[test]
    fn test_format_oc_id_custom_instance() {
        let svc = NextcloudFileIdService::new_test("myinst");
        assert_eq!(svc.format_oc_id(1), "00000001myinst");
    }

    #[test]
    fn test_format_oc_id_large_number() {
        let svc = NextcloudFileIdService::new_stub();
        assert_eq!(svc.format_oc_id(123456789), "123456789ocnca");
    }

    #[test]
    fn test_instance_id() {
        let svc = NextcloudFileIdService::new_stub();
        assert_eq!(svc.instance_id(), "ocnca");
    }

    #[test]
    fn test_ensure_ready_fails_on_stub() {
        let svc = NextcloudFileIdService::new_stub();
        assert!(svc.ensure_ready().is_err());
    }

    // Empty input resolves to an empty map without ever touching the repo, so
    // it succeeds even on the repo-less stub.
    #[tokio::test]
    async fn test_get_or_create_file_ids_empty_is_noop() {
        let svc = NextcloudFileIdService::new_stub();
        let map = svc.get_or_create_file_ids(&[]).await.unwrap();
        assert!(map.is_empty());
    }

    // Unparseable ids never had a mapping, so they are skipped before any repo
    // call — the stub (no repo) must not error on them.
    #[tokio::test]
    async fn test_get_or_create_file_ids_skips_unparseable() {
        let svc = NextcloudFileIdService::new_stub();
        let map = svc
            .get_or_create_file_ids(&["not-a-uuid".to_string()])
            .await
            .unwrap();
        assert!(map.is_empty());
    }
}
