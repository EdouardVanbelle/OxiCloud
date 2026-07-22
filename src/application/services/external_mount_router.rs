//! Classifies file/folder ids into native vs. external-mount handling.
//!
//! This is the single, cheap hook the service layer calls before any
//! `Uuid::parse_str`, so synthetic `ext:` ids and mount-root UUIDs branch to the
//! provider while everything else flows to the PostgreSQL repositories unchanged.

use std::sync::Arc;

use uuid::Uuid;

use crate::application::services::mount_registry::{MountConfig, MountRegistry};
use crate::domain::services::external_mount_id::{NodeId, is_external_id, parse_child_id};

/// The result of classifying an id.
pub enum ResolvedId {
    /// Plain native resource (UUID not registered as a mount root, or an
    /// unrecognized id). Handle exactly as today.
    Regular,
    /// A real UUID that IS a mount root. Listing/metadata branch to the provider;
    /// the row itself still exists natively.
    MountRoot { cfg: Arc<MountConfig> },
    /// A synthetic id addressing an entry inside a mount.
    MountChild {
        cfg: Arc<MountConfig>,
        node_id: NodeId,
    },
}

/// Thin, cloneable classifier over the mount registry.
#[derive(Clone)]
pub struct MountRouter {
    registry: Arc<MountRegistry>,
}

impl MountRouter {
    /// Construct from the shared registry.
    pub fn new(registry: Arc<MountRegistry>) -> Self {
        Self { registry }
    }

    /// Borrow the underlying registry (for path-based resolution / admin reload).
    pub fn registry(&self) -> &Arc<MountRegistry> {
        &self.registry
    }

    /// Fast path: are there no mounts at all? Lets callers skip classification.
    pub fn is_empty(&self) -> bool {
        self.registry.is_empty()
    }

    /// Classify an id. Never parses a provider `node_id` — only the envelope.
    pub fn classify(&self, id: &str) -> ResolvedId {
        if is_external_id(id) {
            if let Some(child) = parse_child_id(id)
                && let Some(cfg) = self.registry.get(&child.mount_id)
            {
                return ResolvedId::MountChild {
                    cfg,
                    node_id: child.node_id,
                };
            }
            // Malformed or dangling `ext:` id — fall through to Regular so it
            // surfaces a clean NotFound downstream rather than hitting the repos.
            return ResolvedId::Regular;
        }
        if let Ok(uuid) = Uuid::parse_str(id)
            && let Some(cfg) = self.registry.get(&uuid)
        {
            return ResolvedId::MountRoot { cfg };
        }
        ResolvedId::Regular
    }

    /// True when `id` addresses anything inside a mount (root or child).
    pub fn is_mount_id(&self, id: &str) -> bool {
        matches!(
            self.classify(id),
            ResolvedId::MountRoot { .. } | ResolvedId::MountChild { .. }
        )
    }

    /// Path-based lookup for the protocol surfaces (WebDAV / NextCloud): does
    /// `internal_path` descend into a mount within `drive_id`? Returns the mount
    /// config plus the remainder relpath (empty when the path IS the mount root).
    pub fn find_path(
        &self,
        drive_id: uuid::Uuid,
        internal_path: &str,
    ) -> Option<(Arc<MountConfig>, String)> {
        self.registry.find_mount_for_path(drive_id, internal_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::external_mount_ports::{
        ExternalMountRecord, ExternalMountRepositoryPort,
    };
    use crate::domain::errors::DomainError;
    use crate::domain::services::external_mount_id::encode_child_id;
    use crate::infrastructure::services::mount_provider_factory::DefaultMountProviderFactory;
    use async_trait::async_trait;
    use tempfile::TempDir;

    struct FakeRepo(Vec<ExternalMountRecord>);
    #[async_trait]
    impl ExternalMountRepositoryPort for FakeRepo {
        async fn list_all(&self) -> Result<Vec<ExternalMountRecord>, DomainError> {
            Ok(self.0.clone())
        }
    }

    async fn router_with_mount(mount_id: Uuid, dir: &TempDir) -> MountRouter {
        let repo = FakeRepo(vec![ExternalMountRecord {
            mount_folder_id: mount_id,
            kind: "local_fs".to_string(),
            config: serde_json::json!({ "path": dir.path().to_str().unwrap() }),
            name: "M".to_string(),
            owner_id: Uuid::new_v4(),
            read_only: false,
            drive_id: Uuid::new_v4(),
            mount_path: "Personal/M".to_string(),
        }]);
        let reg = Arc::new(MountRegistry::empty());
        reg.reload(&repo, &DefaultMountProviderFactory::new()).await;
        MountRouter::new(reg)
    }

    #[test]
    fn empty_registry_classifies_everything_regular() {
        let router = MountRouter::new(Arc::new(MountRegistry::empty()));
        assert!(router.is_empty());
        assert!(matches!(
            router.classify(&Uuid::new_v4().to_string()),
            ResolvedId::Regular
        ));
        assert!(matches!(
            router.classify("ext:deadbeef:dG9rZW4"),
            ResolvedId::Regular
        ));
        assert!(matches!(router.classify("garbage"), ResolvedId::Regular));
    }

    #[tokio::test]
    async fn classifies_mount_root_uuid() {
        let dir = TempDir::new().unwrap();
        let mount_id = Uuid::new_v4();
        let router = router_with_mount(mount_id, &dir).await;

        match router.classify(&mount_id.to_string()) {
            ResolvedId::MountRoot { cfg } => assert_eq!(cfg.mount_id, mount_id),
            _ => panic!("expected MountRoot"),
        }
        assert!(router.is_mount_id(&mount_id.to_string()));
    }

    #[tokio::test]
    async fn classifies_ext_child_id() {
        let dir = TempDir::new().unwrap();
        let mount_id = Uuid::new_v4();
        let router = router_with_mount(mount_id, &dir).await;

        let child = encode_child_id(mount_id, "docs/a.txt");
        match router.classify(&child) {
            ResolvedId::MountChild { cfg, node_id } => {
                assert_eq!(cfg.mount_id, mount_id);
                assert_eq!(node_id.as_str(), "docs/a.txt");
            }
            _ => panic!("expected MountChild"),
        }
        assert!(router.is_mount_id(&child));
    }

    #[tokio::test]
    async fn ext_id_for_unregistered_mount_is_regular() {
        let dir = TempDir::new().unwrap();
        let mount_id = Uuid::new_v4();
        let router = router_with_mount(mount_id, &dir).await;

        // A well-formed ext: id but for a DIFFERENT (unknown) mount → Regular,
        // so it 404s downstream rather than hitting the repos.
        let dangling = encode_child_id(Uuid::new_v4(), "x");
        assert!(matches!(router.classify(&dangling), ResolvedId::Regular));

        // A plain (non-mount) UUID is also Regular.
        assert!(matches!(
            router.classify(&Uuid::new_v4().to_string()),
            ResolvedId::Regular
        ));
    }
}
