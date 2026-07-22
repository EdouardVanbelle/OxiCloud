//! The single place external mount provider kinds are registered.
//!
//! Adding a new backend (`sftp`, `webdav`, …) is: implement
//! [`ExternalMountProvider`](crate::application::ports::external_mount_ports::ExternalMountProvider)
//! and add one arm to [`DefaultMountProviderFactory::build`]. Nothing else in the
//! router / listing / authz / path-resolution layers changes.

use std::sync::Arc;

use async_trait::async_trait;

use crate::application::ports::external_mount_ports::{
    ExternalMountProvider, MountProviderFactory,
};
use crate::domain::errors::DomainError;
use crate::infrastructure::services::local_fs_mount_provider::LocalFsMountProvider;

/// Default factory: knows the built-in provider kinds.
#[derive(Default)]
pub struct DefaultMountProviderFactory;

impl DefaultMountProviderFactory {
    /// Construct the factory.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl MountProviderFactory for DefaultMountProviderFactory {
    async fn build(
        &self,
        kind: &str,
        config: &serde_json::Value,
    ) -> Result<Arc<dyn ExternalMountProvider>, DomainError> {
        match kind {
            "local_fs" => {
                let path = config.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                    DomainError::validation_error(
                        "local_fs mount config requires a string \"path\"",
                    )
                })?;
                let read_only = config
                    .get("read_only")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let provider = LocalFsMountProvider::new(path, read_only)?;
                Ok(Arc::new(provider))
            }
            other => Err(DomainError::operation_not_supported(
                "ExternalMount",
                format!("unknown mount provider kind: {other}"),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::errors::{DomainError, ErrorKind};

    /// `Arc<dyn ExternalMountProvider>` isn't `Debug`, so `unwrap_err` won't
    /// compile — extract the error by matching instead.
    fn expect_err(r: Result<Arc<dyn ExternalMountProvider>, DomainError>) -> DomainError {
        match r {
            Ok(_) => panic!("expected an error"),
            Err(e) => e,
        }
    }

    #[tokio::test]
    async fn builds_local_fs_provider_from_valid_config() {
        let dir = tempfile::tempdir().unwrap();
        let factory = DefaultMountProviderFactory::new();
        let cfg = serde_json::json!({ "path": dir.path().to_str().unwrap() });
        let provider = factory.build("local_fs", &cfg).await.expect("builds");
        assert_eq!(provider.kind(), "local_fs");
    }

    #[tokio::test]
    async fn local_fs_honours_read_only_flag() {
        let dir = tempfile::tempdir().unwrap();
        let factory = DefaultMountProviderFactory::new();
        let cfg = serde_json::json!({ "path": dir.path().to_str().unwrap(), "read_only": true });
        let provider = factory.build("local_fs", &cfg).await.unwrap();
        assert!(provider.capabilities().read_only);
    }

    #[tokio::test]
    async fn unknown_kind_is_unsupported() {
        let factory = DefaultMountProviderFactory::new();
        let err = expect_err(factory.build("sftp", &serde_json::json!({})).await);
        assert_eq!(err.kind, ErrorKind::UnsupportedOperation);
    }

    #[tokio::test]
    async fn local_fs_missing_path_is_validation_error() {
        let factory = DefaultMountProviderFactory::new();
        let err = expect_err(
            factory
                .build("local_fs", &serde_json::json!({ "read_only": true }))
                .await,
        );
        assert_eq!(err.kind, ErrorKind::InvalidInput);
    }

    #[tokio::test]
    async fn local_fs_nonexistent_path_errors() {
        let factory = DefaultMountProviderFactory::new();
        let err = expect_err(
            factory
                .build(
                    "local_fs",
                    &serde_json::json!({ "path": "/no/such/dir/xyz123" }),
                )
                .await,
        );
        // Propagated from LocalFsMountProvider::new (canonicalize failure).
        assert_eq!(err.kind, ErrorKind::InternalError);
    }
}
