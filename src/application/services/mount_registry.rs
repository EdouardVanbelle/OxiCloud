//! In-memory registry of configured external mounts.
//!
//! Holds, per mount-root folder UUID, the constructed provider plus the metadata
//! the service layer needs to authorize and synthesize DTOs. Reads are lock-free
//! (`arc-swap`) so the hot path ("is this UUID a mount root?") never blocks; the
//! whole index is rebuilt on admin mutation via [`MountRegistry::reload`].

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use uuid::Uuid;

use crate::application::ports::external_mount_ports::{
    ExternalMountProvider, ExternalMountRepositoryPort, MountProviderFactory,
};

/// One configured mount, with its live provider.
pub struct MountConfig {
    /// Mount-root folder UUID — the mount's identity and the authz resource.
    pub mount_id: Uuid,
    /// Provider kind.
    pub kind: String,
    /// Display name.
    pub name: String,
    /// Owner of the mount configuration.
    pub owner_id: Uuid,
    /// Drive the mount root belongs to.
    pub drive_id: Uuid,
    /// Whether the mount refuses mutations.
    pub read_only: bool,
    /// Materialized internal path of the mount root (e.g. `"Personal/Media"`),
    /// used by path-based resolution for WebDAV / NextCloud.
    pub mount_path: String,
    /// The bound provider for this mount's backend.
    pub provider: Arc<dyn ExternalMountProvider>,
}

/// Immutable snapshot swapped atomically on reload.
#[derive(Default)]
struct MountIndex {
    /// mount-root folder UUID → config.
    by_folder: HashMap<Uuid, Arc<MountConfig>>,
    /// (drive_id, mount_path) → mount-root UUID, for path resolution (P3).
    by_path: HashMap<(Uuid, String), Uuid>,
}

/// Lock-free registry of mounts.
pub struct MountRegistry {
    inner: ArcSwap<MountIndex>,
}

impl Default for MountRegistry {
    fn default() -> Self {
        Self::empty()
    }
}

impl MountRegistry {
    /// An empty registry (no mounts).
    pub fn empty() -> Self {
        Self {
            inner: ArcSwap::from_pointee(MountIndex::default()),
        }
    }

    /// Look up a mount by its root folder UUID.
    pub fn get(&self, mount_id: &Uuid) -> Option<Arc<MountConfig>> {
        self.inner.load().by_folder.get(mount_id).cloned()
    }

    /// Is this UUID the root of a configured mount?
    pub fn is_mount_root(&self, id: &Uuid) -> bool {
        self.inner.load().by_folder.contains_key(id)
    }

    /// True when no mounts are configured (lets callers skip work entirely).
    pub fn is_empty(&self) -> bool {
        self.inner.load().by_folder.is_empty()
    }

    /// Find the mount whose root path is a segment-aligned prefix of
    /// `internal_path` within `drive_id`. Returns the config plus the remainder
    /// path relative to the mount root (`""` when the path IS the mount root).
    ///
    /// Used by path-based resolution (WebDAV / NextCloud) in P3.
    pub fn find_mount_for_path(
        &self,
        drive_id: Uuid,
        internal_path: &str,
    ) -> Option<(Arc<MountConfig>, String)> {
        let index = self.inner.load();
        // Normalize away a leading slash: materialized folder paths arrive both
        // as `Personal/Media` (raw `folders.path`) and `/Personal/Media`
        // (FolderDto / WebDAV internal paths). The index keys are stored without
        // a leading slash (see `reload`).
        let internal_path = internal_path.trim_start_matches('/');
        // Walk ancestor paths from the full path up to the root, longest first,
        // so the deepest matching mount wins.
        let mut candidate = internal_path;
        loop {
            if let Some(mount_id) = index.by_path.get(&(drive_id, candidate.to_string()))
                && let Some(cfg) = index.by_folder.get(mount_id)
            {
                let remainder = internal_path
                    .strip_prefix(candidate)
                    .map(|r| r.trim_start_matches('/').to_string())
                    .unwrap_or_default();
                return Some((cfg.clone(), remainder));
            }
            match candidate.rsplit_once('/') {
                Some((parent, _)) => candidate = parent,
                None => return None,
            }
        }
    }

    /// Rebuild the registry from persisted records, constructing each provider
    /// via the factory. A mount whose provider fails to build is skipped (logged)
    /// rather than failing the whole reload.
    pub async fn reload(
        &self,
        repo: &dyn ExternalMountRepositoryPort,
        factory: &dyn MountProviderFactory,
    ) {
        let records = match repo.list_all().await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(
                    target: "oxicloud::external_mounts",
                    "failed to load external mounts: {e}"
                );
                return;
            }
        };

        let mut by_folder = HashMap::with_capacity(records.len());
        let mut by_path = HashMap::with_capacity(records.len());
        for rec in records {
            let provider = match factory.build(&rec.kind, &rec.config).await {
                Ok(p) => p,
                Err(e) => {
                    tracing::error!(
                        target: "oxicloud::external_mounts",
                        mount_id = %rec.mount_folder_id,
                        kind = %rec.kind,
                        "skipping mount: provider build failed: {e}"
                    );
                    continue;
                }
            };
            // Store the path key without a leading slash so lookups normalize
            // consistently (see `find_mount_for_path`).
            by_path.insert(
                (
                    rec.drive_id,
                    rec.mount_path.trim_start_matches('/').to_string(),
                ),
                rec.mount_folder_id,
            );
            by_folder.insert(
                rec.mount_folder_id,
                Arc::new(MountConfig {
                    mount_id: rec.mount_folder_id,
                    kind: rec.kind,
                    name: rec.name,
                    owner_id: rec.owner_id,
                    drive_id: rec.drive_id,
                    read_only: rec.read_only,
                    mount_path: rec.mount_path,
                    provider,
                }),
            );
        }

        let count = by_folder.len();
        self.inner
            .store(Arc::new(MountIndex { by_folder, by_path }));
        tracing::info!(
            target: "oxicloud::external_mounts",
            count, "external mount registry loaded"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::external_mount_ports::{
        ExternalMountRecord, ExternalMountRepositoryPort,
    };
    use crate::domain::errors::DomainError;
    use crate::infrastructure::services::mount_provider_factory::DefaultMountProviderFactory;
    use async_trait::async_trait;
    use std::path::Path;
    use tempfile::TempDir;

    struct FakeRepo {
        records: Vec<ExternalMountRecord>,
    }

    #[async_trait]
    impl ExternalMountRepositoryPort for FakeRepo {
        async fn list_all(&self) -> Result<Vec<ExternalMountRecord>, DomainError> {
            Ok(self.records.clone())
        }
    }

    fn record(
        mount_id: Uuid,
        drive_id: Uuid,
        mount_path: &str,
        path: &Path,
    ) -> ExternalMountRecord {
        ExternalMountRecord {
            mount_folder_id: mount_id,
            kind: "local_fs".to_string(),
            config: serde_json::json!({ "path": path.to_str().unwrap() }),
            name: "Test Mount".to_string(),
            owner_id: Uuid::new_v4(),
            read_only: false,
            drive_id,
            mount_path: mount_path.to_string(),
        }
    }

    #[test]
    fn empty_registry_is_inert() {
        let r = MountRegistry::empty();
        assert!(r.is_empty());
        assert!(!r.is_mount_root(&Uuid::new_v4()));
        assert!(r.get(&Uuid::new_v4()).is_none());
    }

    #[tokio::test]
    async fn reload_populates_from_records() {
        let dir = TempDir::new().unwrap();
        let mount_id = Uuid::new_v4();
        let drive_id = Uuid::new_v4();
        let repo = FakeRepo {
            records: vec![record(mount_id, drive_id, "Personal/Media", dir.path())],
        };
        let factory = DefaultMountProviderFactory::new();

        let reg = MountRegistry::empty();
        reg.reload(&repo, &factory).await;

        assert!(!reg.is_empty());
        assert!(reg.is_mount_root(&mount_id));
        let cfg = reg.get(&mount_id).expect("present");
        assert_eq!(cfg.kind, "local_fs");
        assert_eq!(cfg.drive_id, drive_id);
        assert_eq!(cfg.mount_path, "Personal/Media");
    }

    #[tokio::test]
    async fn reload_skips_mount_whose_provider_fails_to_build() {
        let mount_id = Uuid::new_v4();
        // Point at a path that doesn't exist → LocalFsMountProvider::new errors.
        let repo = FakeRepo {
            records: vec![ExternalMountRecord {
                mount_folder_id: mount_id,
                kind: "local_fs".to_string(),
                config: serde_json::json!({ "path": "/nonexistent/path/xyz-123" }),
                name: "Bad".to_string(),
                owner_id: Uuid::new_v4(),
                read_only: false,
                drive_id: Uuid::new_v4(),
                mount_path: "Personal/Bad".to_string(),
            }],
        };
        let factory = DefaultMountProviderFactory::new();
        let reg = MountRegistry::empty();
        reg.reload(&repo, &factory).await;
        // The bad mount is skipped, not fatal.
        assert!(reg.is_empty());
        assert!(!reg.is_mount_root(&mount_id));
    }

    #[tokio::test]
    async fn find_mount_for_path_matches_prefix_and_remainder() {
        let dir = TempDir::new().unwrap();
        let mount_id = Uuid::new_v4();
        let drive_id = Uuid::new_v4();
        let repo = FakeRepo {
            records: vec![record(mount_id, drive_id, "Personal/Media", dir.path())],
        };
        let reg = MountRegistry::empty();
        reg.reload(&repo, &DefaultMountProviderFactory::new()).await;

        // Exact match → empty remainder.
        let (cfg, rem) = reg
            .find_mount_for_path(drive_id, "Personal/Media")
            .expect("exact match");
        assert_eq!(cfg.mount_id, mount_id);
        assert_eq!(rem, "");

        // Nested path → remainder is the suffix.
        let (_cfg, rem) = reg
            .find_mount_for_path(drive_id, "Personal/Media/docs/a.txt")
            .expect("nested match");
        assert_eq!(rem, "docs/a.txt");

        // Non-matching path within the drive → None.
        assert!(
            reg.find_mount_for_path(drive_id, "Personal/Other")
                .is_none()
        );

        // Same path but a DIFFERENT drive → None (drive-scoped).
        assert!(
            reg.find_mount_for_path(Uuid::new_v4(), "Personal/Media")
                .is_none()
        );

        // A sibling that merely shares a name prefix must NOT match
        // (segment-aligned only).
        assert!(
            reg.find_mount_for_path(drive_id, "Personal/MediaLibrary")
                .is_none()
        );
    }

    #[tokio::test]
    async fn reload_replaces_previous_state() {
        let dir = TempDir::new().unwrap();
        let drive = Uuid::new_v4();
        let first = Uuid::new_v4();
        let reg = MountRegistry::empty();

        reg.reload(
            &FakeRepo {
                records: vec![record(first, drive, "Personal/A", dir.path())],
            },
            &DefaultMountProviderFactory::new(),
        )
        .await;
        assert!(reg.is_mount_root(&first));

        // A second reload with a different mount set replaces the first entirely.
        let second = Uuid::new_v4();
        reg.reload(
            &FakeRepo {
                records: vec![record(second, drive, "Personal/B", dir.path())],
            },
            &DefaultMountProviderFactory::new(),
        )
        .await;
        assert!(reg.is_mount_root(&second));
        assert!(
            !reg.is_mount_root(&first),
            "stale mount must be gone after reload"
        );
    }

    #[tokio::test]
    async fn find_mount_for_path_deepest_wins() {
        let outer = TempDir::new().unwrap();
        let inner = TempDir::new().unwrap();
        let drive_id = Uuid::new_v4();
        let outer_id = Uuid::new_v4();
        let inner_id = Uuid::new_v4();
        let repo = FakeRepo {
            records: vec![
                record(outer_id, drive_id, "Personal", outer.path()),
                record(inner_id, drive_id, "Personal/Media", inner.path()),
            ],
        };
        let reg = MountRegistry::empty();
        reg.reload(&repo, &DefaultMountProviderFactory::new()).await;

        // A path under the deeper mount resolves to the deeper mount.
        let (cfg, rem) = reg
            .find_mount_for_path(drive_id, "Personal/Media/x")
            .expect("match");
        assert_eq!(cfg.mount_id, inner_id);
        assert_eq!(rem, "x");

        // A path under the shallower mount (but not the deeper one) resolves
        // to the shallower mount.
        let (cfg, rem) = reg
            .find_mount_for_path(drive_id, "Personal/Other/y")
            .expect("match");
        assert_eq!(cfg.mount_id, outer_id);
        assert_eq!(rem, "Other/y");
    }
}
